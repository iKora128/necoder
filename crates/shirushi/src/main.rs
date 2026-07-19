//! Shirushi（しるし）— GPUI ベースの自作エディタ。エントリポイント。
//!
//! `shirushi [<path>...]` で各 path をプロジェクト（レール項目）として開く。file を渡すと親を
//! プロジェクト、その file をエディタに開く。引数無しは前回状態（`state.json`）を復元する。

use gpui::{
    App, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, actions, point,
    prelude::*, px, size,
};
use gpui_platform::application;
use host::{RemoteHost, SshProject};
use std::path::{Path, PathBuf};

/// MCP サーバ（`shirushi mcp`）。AI エージェントがプロジェクトを操作する口。
mod mcp;
use std::time::Instant;
use workspace::{ProjectSource, RestoredTabs, Workspace};

actions!(shirushi, [Quit]);

/// SSH project を接続して source にする。互換 server が無ければ bootstrap が自動配備する。
fn connect_ssh_project(uri: &str) -> anyhow::Result<ProjectSource> {
    let project = SshProject::parse(uri)?;
    let server_command = std::env::var("SHIRUSHI_REMOTE_SERVER_COMMAND")
        .unwrap_or_else(|_| "shirushi-remote-server".to_string());
    let host = RemoteHost::connect_ssh(&project, &server_command)?;
    let root = host.root().to_path_buf();
    Ok(ProjectSource::new(host, root))
}

/// コマンドライン引数（or 前回状態）から host 付き project と復元タブ列を決める。
fn resolve_projects() -> (Vec<ProjectSource>, Vec<RestoredTabs>, usize) {
    let mut sources = Vec::new();
    let mut open_files: Vec<RestoredTabs> = Vec::new();
    let mut active = 0;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has_explicit_args = !args.is_empty();

    for arg in args {
        if arg.starts_with("ssh://") {
            match connect_ssh_project(&arg) {
                Ok(source) => {
                    sources.push(source);
                    open_files.push(RestoredTabs::default());
                }
                Err(error) => eprintln!("Remote SSH 接続に失敗（スキップ）: {error:#}"),
            }
            continue;
        }
        let path = PathBuf::from(&arg);
        if path.is_file() {
            let file = std::fs::canonicalize(&path).unwrap_or(path);
            let parent = file
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            sources.push(ProjectSource::local(parent));
            open_files.push(RestoredTabs::single(file));
        } else if path.is_dir() {
            sources.push(ProjectSource::local(std::fs::canonicalize(&path).unwrap_or(path)));
            open_files.push(RestoredTabs::default());
        } else {
            eprintln!("見つからない（スキップ）: {arg}");
        }
    }

    // 引数無し → 前回状態を復元
    if sources.is_empty() && !has_explicit_args {
        if let Some(state_path) = workspace::state_path() {
            if let Some((saved_projects, saved_active)) = workspace::load_saved_state(&state_path) {
                for (saved_index, saved) in saved_projects.into_iter().enumerate() {
                    let source = match saved.remote_uri.as_deref() {
                        Some(uri) => connect_ssh_project(uri),
                        None => Ok(ProjectSource::local(saved.root)),
                    };
                    match source {
                        Ok(source) => {
                            if saved_index == saved_active {
                                active = sources.len();
                            }
                            sources.push(source);
                            open_files.push(RestoredTabs {
                                files: saved.open_files,
                                active: saved.active_file,
                            });
                        }
                        Err(error) => eprintln!("前回の Remote SSH 接続を復元できない: {error:#}"),
                    }
                }
            }
        }
    }
    // 最後の砦: カレントディレクトリ
    if sources.is_empty() && !has_explicit_args {
        if let Ok(cwd) = std::env::current_dir() {
            sources.push(ProjectSource::local(cwd));
            open_files.push(RestoredTabs::default());
        }
    }
    (sources, open_files, active)
}

/// ユーザー keymap.json を読み込んで bind する（存在しなければ何もしない）。
/// 既定 keymap の後に呼ぶ＝後勝ちで上書きできる。壊れた項目は keymap_core が skip して警告する。
fn load_user_keymap(path: &Path, cx: &mut App) {
    let Ok(json) = std::fs::read_to_string(path) else {
        return;
    };
    match keymap_core::load_bindings(&json, cx) {
        Ok(bindings) => {
            if !bindings.is_empty() {
                println!("ユーザー keymap を適用: {}（{} 束）", path.display(), bindings.len());
                cx.bind_keys(bindings);
            }
        }
        Err(error) => eprintln!("ユーザー keymap の読込に失敗（既定で継続）: {error:#}"),
    }
}

/// ユーザー keymap.json の live reload（保存したら即キーが差し替わる・M10-13）。
/// gpui の keymap は後から bind したものが勝つので、再読込 = 再 bind でよい。
fn watch_user_keymap(path: PathBuf, cx: &mut App) {
    let Some(parent) = path.parent().map(Path::to_path_buf) else {
        return;
    };
    let (sender, mut receiver) = futures::channel::mpsc::unbounded::<()>();
    let target = path.clone();
    let watch = project::watch_root(&parent, move |paths| {
        if paths.iter().any(|changed| changed == &target) {
            let _ = sender.unbounded_send(());
        }
    });
    let watch = match watch {
        Ok(watch) => watch,
        Err(error) => {
            eprintln!("keymap の監視を開始できない: {error:#}");
            return;
        }
    };
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        let _keep_alive = watch; // drop で監視停止（アプリ終了まで保持）
        while receiver.next().await.is_some() {
            // 連続保存を 200ms 合流。
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            while receiver.try_recv().is_ok() {}
            let path = path.clone();
            cx.update(|cx| load_user_keymap(&path, cx));
        }
    })
    .detach();
}

/// 同梱フォントを読み込む。UI = IBM Plex Sans JP、コード = Guguru Sans Code（Google Sans Code +
/// IBM Plex Sans JP の等幅）。どちらも SIL OFL（`assets/fonts/` 参照）。バイナリに埋め込む（`include_bytes!`）。
fn load_fonts(cx: &App) {
    use std::borrow::Cow;
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/IBMPlexSansJP-Regular.ttf"
        ))),
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/IBMPlexSansJP-SemiBold.ttf"
        ))),
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/GuguruSansCode-Regular.ttf"
        ))),
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/GuguruSansCode-Bold.ttf"
        ))),
    ];
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        eprintln!("フォント読み込みに失敗（既定フォントで継続）: {error}");
    }
}

/// レール等のアイコン SVG（Lucide・ISC ライセンス）をバイナリに埋め込んで gpui の `svg()` に供給する。
/// `svg().path("icons/…")` がここを引く。単色マスクとして描かれ `text_color` で着色される。
struct Assets;

impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        macro_rules! icon {
            ($name:literal) => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/", $name)).as_slice()
            };
        }
        let bytes: &'static [u8] = match path {
            "icons/panel-left.svg" => icon!("panel-left.svg"),
            "icons/search.svg" => icon!("search.svg"),
            "icons/git-branch.svg" => icon!("git-branch.svg"),
            "icons/sparkles.svg" => icon!("sparkles.svg"),
            "icons/square-terminal.svg" => icon!("square-terminal.svg"),
            "icons/folder-plus.svg" => icon!("folder-plus.svg"),
            "icons/folder-tree.svg" => icon!("folder-tree.svg"),
            "icons/settings.svg" => icon!("settings.svg"),
            "icons/square-check.svg" => icon!("square-check.svg"),
            "icons/server.svg" => icon!("server.svg"),
            "icons/list.svg" => icon!("list.svg"),
            "icons/columns-3.svg" => icon!("columns-3.svg"),
            "icons/layout-grid.svg" => icon!("layout-grid.svg"),
            // AI エージェントのブランドロゴ（Simple Icons・CC0・設定画面の識別用）。
            "icons/brand-claude.svg" => icon!("brand-claude.svg"),
            "icons/brand-gemini.svg" => icon!("brand-gemini.svg"),
            "icons/brand-copilot.svg" => icon!("brand-copilot.svg"),
            "icons/brand-qwen.svg" => icon!("brand-qwen.svg"),
            "icons/brand-opencode.svg" => icon!("brand-opencode.svg"),
            "icons/brand-kimi.svg" => icon!("brand-kimi.svg"),
            _ => return Ok(None),
        };
        Ok(Some(std::borrow::Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<gpui::SharedString>> {
        Ok(Vec::new())
    }
}

/// `shirushi config <get|set|list> …` サブコマンド。設定を CLI から読み書きする（settings.json が真実）。
/// 書き込みは即 settings.json に反映＝**起動中のアプリは watcher で live 適用**、次回起動でも効く。
/// CLI を処理したら true（GUI を開かず終了）。設定の「書き手」の一つ（UI トグル / MCP と同じ経路）。
fn run_config_cli() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("config") {
        return false;
    }
    let user_path = settings_core::user_settings_path();
    match args.get(1).map(String::as_str) {
        Some("list") => {
            let store = settings_core::SettingsStore::load(user_path.as_deref(), None);
            match serde_json::to_string_pretty(store.merged()) {
                Ok(text) => println!("{text}"),
                Err(error) => eprintln!("設定の表示に失敗: {error}"),
            }
        }
        Some("get") => match args.get(2) {
            Some(key) => {
                let store = settings_core::SettingsStore::load(user_path.as_deref(), None);
                match store.merged().get(key) {
                    Some(value) => println!("{value}"),
                    None => eprintln!("キーが無い: {key}"),
                }
            }
            None => eprintln!("使い方: shirushi config get <key>"),
        },
        Some("set") => match (args.get(2), args.get(3), user_path) {
            (Some(key), Some(raw), Some(path)) => {
                // 値は JSON として解釈（true/false/数値/"文字列"/配列）。失敗時は文字列扱い。
                let value = serde_json::from_str(raw)
                    .unwrap_or_else(|_| serde_json::Value::String(raw.clone()));
                match settings_core::persist_user_value(&path, key, value) {
                    Ok(()) => println!("set {key} = {raw}"),
                    Err(error) => eprintln!("保存に失敗: {error:#}"),
                }
            }
            _ => eprintln!("使い方: shirushi config set <key> <value>"),
        },
        _ => eprintln!("使い方: shirushi config <list | get <key> | set <key> <value>>"),
    }
    true
}

fn main() {
    // GUI を開く前に CLI サブコマンドを処理（`shirushi config …` / `shirushi mcp …`）。
    if run_config_cli() {
        return;
    }
    // MCP サーバ（AI エージェントがプロジェクトを操作する口）。stdio を占有するので GUI は開かない。
    if mcp::run() {
        return;
    }
    let startup = Instant::now();
    application().with_assets(Assets).run(move |cx: &mut App| {
        load_fonts(cx);
        i18n::init_from_os_locale();

        // プロジェクトを解決（roots + 起動時に開くファイル）。先頭 root を project 設定の対象にする。
        let (sources, open_files, active_project) = resolve_projects();

        // 設定（default → user → project）を **反応的 global** に載せてファイル監視を開始する。
        // 以後 UI トグル / CLI / MCP / 手編集はすべてこの 1 つの store を更新し、live で波及する。
        let local_settings_root = sources
            .first()
            .filter(|source| !source.is_remote())
            .map(|source| source.root().to_path_buf());
        settings::init(settings_core::user_settings_path(), local_settings_root, cx);
        let settings = settings::get(cx);
        if let Some(locale) = &settings.locale {
            i18n::set_locale(locale);
        }
        // theme 名を解決（組み込み → 設定フォルダ themes/ のユーザーテーマ → dark）。
        // 開発用: SHIRUSHI_THEME=<名> で設定を上書き（撮影確認・非破壊）。
        let themes_dir = settings_core::user_settings_path()
            .as_deref()
            .and_then(Path::parent)
            .map(|dir| dir.join("themes"));
        let theme_name = std::env::var("SHIRUSHI_THEME").unwrap_or_else(|_| settings.theme.clone());
        let theme = theme_core::resolve(&theme_name, themes_dir.as_deref());

        match keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx) {
            Ok(bindings) => cx.bind_keys(bindings),
            Err(error) => eprintln!("keymap のロードに失敗: {error:#}"),
        }
        // ユーザー keymap（~/Library/Application Support/Shirushi/keymap.json・M10-13）。
        // 既定の**後**に bind ＝ 同じキーはユーザー側が勝つ。ファイル監視で live reload。
        let user_keymap_path = settings_core::user_settings_path()
            .as_deref()
            .and_then(Path::parent)
            .map(|dir| dir.join("keymap.json"));
        if let Some(keymap_path) = user_keymap_path {
            load_user_keymap(&keymap_path, cx);
            watch_user_keymap(keymap_path, cx);
        }
        // Quit の後始末（hot exit のクリア）は window 生成後に登録する（下方）。

        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);

        let build_sources = sources.clone();
        let build_theme = theme.clone();
        let open = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // 自前 titlebar（UI-SPEC §3）を描くため既定のシステム titlebar を隠す。
                // 信号機は残し、38px の titlebar 内に収まる位置へ寄せる。
                titlebar: Some(TitlebarOptions {
                    title: Some("Shirushi".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(13.0), px(13.0))),
                }),
                // custom titlebar で自前ドラッグ（start_window_move）を使うため false。
                // true のままだと macOS が titlebar を system 所有扱いしてクリック遅延や
                // ダブルクリック判定の不具合になる（gpui WindowOptions のコメント参照）。
                is_movable: false,
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|cx| {
                    Workspace::new_sources_with_active(
                        build_sources.clone(),
                        active_project,
                        build_theme.clone(),
                        workspace::state_path(),
                        cx,
                    )
                })
            },
        );

        let window = match open {
            Ok(window) => window,
            Err(error) => {
                eprintln!("ウィンドウを開けない: {error}");
                return;
            }
        };

        // 正常終了（⌘Q）= hot exit スナップショットを破棄してから quit（仕様: 正常終了で破棄）。
        cx.on_action(move |_: &Quit, cx: &mut App| {
            let _ = window.update(cx, |workspace, _window, _cx| workspace.prepare_quit());
            cx.quit();
        });

        // 開発用: SHIRUSHI_OPEN_TABS=a.rs,b.rs,… でアクティブプロジェクトに複数タブを開く（複数タブ検証）。
        let extra_tabs = std::env::var("SHIRUSHI_OPEN_TABS").ok().map(|value| {
            let root = sources
                .get(active_project)
                .map(|source| source.root().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            value
                .split(',')
                .filter(|entry| !entry.is_empty())
                .map(|entry| root.join(entry))
                .collect::<Vec<_>>()
        });
        if let Err(error) = window.update(cx, |workspace, window, cx| {
            workspace.restore_open_file(&open_files, window, cx);
            workspace.check_hot_exit_restore(cx); // 前回の未保存スナップショットがあれば復元バーを出す
            // 開発用: SHIRUSHI_NAMING_CONFIRM=1 で命名入力を 1s 後に Enter 確定する（ファイル生成の検証）。
            if std::env::var_os("SHIRUSHI_NAMING_CONFIRM").is_some() {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_confirm_naming(window, cx);
                        });
                    })
                    .detach();
                }
            }
                // 開発用: SHIRUSHI_RENAME_PROBE="row:col:newname" で rename を実行（既定 8s 後）。
            if let Ok(probe) = std::env::var("SHIRUSHI_RENAME_PROBE") {
                let parts: Vec<&str> = probe.splitn(3, ':').collect();
                if let [row, column, name] = parts[..] {
                    if let (Ok(row), Ok(column)) = (row.parse::<usize>(), column.parse::<usize>()) {
                        let name = name.to_string();
                        if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                            let delay_ms = std::env::var("SHIRUSHI_TYPE_PROBE_DELAY_MS")
                                .ok()
                                .and_then(|value| value.parse::<u64>().ok())
                                .unwrap_or(8000);
                            cx.spawn(async move |_workspace, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(delay_ms))
                                    .await;
                                let _ = handle.update(cx, |workspace, window, cx| {
                                    workspace.debug_rename_probe(row, column, name, window, cx);
                                });
                            })
                            .detach();
                        }
                    }
                }
            }
            // 開発用: SHIRUSHI_INLINE_PROBE="<指示>" で全選択 → ⌘I → 実行（2s 後・M12-8）。
            // SHIRUSHI_INLINE_ACCEPT=1 なら提案到着を待って適用+保存まで（受入の round trip）。
            if let Ok(instruction) = std::env::var("SHIRUSHI_INLINE_PROBE") {
                if !instruction.trim().is_empty() {
                    let accept = std::env::var("SHIRUSHI_INLINE_ACCEPT")
                        .is_ok_and(|value| value == "1");
                    if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                        cx.spawn(async move |_workspace, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(2000))
                                .await;
                            let _ = handle.update(cx, |workspace, window, cx| {
                                workspace.debug_inline_probe(instruction, accept, window, cx);
                            });
                        })
                        .detach();
                    }
                }
            }
            // 開発用: SHIRUSHI_SWITCHER_PROBE=<ms> で ⌘O スイッチャーを開く（M12-12 の描画検証。
            // ACP_PROBE と併用すると実行中ドットも写る）。
            if let Ok(delay) = std::env::var("SHIRUSHI_SWITCHER_PROBE") {
                if let Ok(delay_ms) = delay.parse::<u64>() {
                    if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                        cx.spawn(async move |_workspace, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(delay_ms))
                                .await;
                            let _ = handle.update(cx, |workspace, window, cx| {
                                workspace.debug_open_switcher(window, cx);
                            });
                        })
                        .detach();
                    }
                }
            }
            // 開発用: SHIRUSHI_SSH_PROBE=1 で SSH 入力バーを開く（2s 後・M13 の描画検証）。
            if std::env::var("SHIRUSHI_SSH_PROBE").is_ok_and(|value| value == "1") {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_open_ssh_input(window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_SSH_HOST_PROBE=1 で SSH ホストピッカーを開く（2s 後・M13 の描画検証）。
            if std::env::var("SHIRUSHI_SSH_HOST_PROBE").is_ok_and(|value| value == "1") {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_open_ssh_host_picker(window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_TAB_RENAME_PROBE=1 で Agent タブの改名入力を開く（2s 後・#4 の描画検証）。
            if std::env::var("SHIRUSHI_TAB_RENAME_PROBE").is_ok_and(|value| value == "1") {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, _window, cx| {
                            workspace.debug_tab_rename(cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_HISTORY_PROBE=1 でスレッド履歴 Picker を開く（2s 後・#5 の描画検証）。
            if std::env::var("SHIRUSHI_HISTORY_PROBE").is_ok_and(|value| value == "1") {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_open_history(window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_TERM_LINK_PROBE="path:line" でターミナルを開き、リンククリック相当の
            // イベントを直接発火（emit → ジャンプの結線検証・M13。座標→リンクの hit 判定は人の手番）。
            if let Ok(probe) = std::env::var("SHIRUSHI_TERM_LINK_PROBE") {
                if let Some((path, line)) = probe.rsplit_once(':') {
                    if let (path, Ok(line)) = (path.to_string(), line.parse::<u32>()) {
                        if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                            cx.spawn(async move |_workspace, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(2500))
                                    .await;
                                let _ = handle.update(cx, |workspace, window, cx| {
                                    workspace.debug_terminal_link(path, line, window, cx);
                                });
                            })
                            .detach();
                        }
                    }
                }
            }
            // 開発用: SHIRUSHI_PALETTE_PROBE="<query>" で ⌘⇧P を開く（2s 後・M13 の描画検証）。
            // SHIRUSHI_PALETTE_CONFIRM=1 で先頭候補を確定（dispatch まで通す）。
            if let Ok(query) = std::env::var("SHIRUSHI_PALETTE_PROBE") {
                let confirm = std::env::var("SHIRUSHI_PALETTE_CONFIRM").is_ok_and(|v| v == "1");
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_palette_probe(query, confirm, window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_TODOS_PROBE=1 で Todo ボードを開く（2s 後・M12-10 の描画検証）。
            if std::env::var_os("SHIRUSHI_TODOS_PROBE").is_some() {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_open_todo_board(window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_DIFF_PROBE=1 でアクティブファイルの diff タブを開く（2s 後）。
            if std::env::var_os("SHIRUSHI_DIFF_PROBE").is_some() {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_open_diff(window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_OUTLINE_PROBE=1 で ⌘⇧O アウトラインを開く（2s 後・LSP 不要）。
            if std::env::var_os("SHIRUSHI_OUTLINE_PROBE").is_some() {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2000))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_outline_probe(window, cx);
                        });
                    })
                    .detach();
                }
            }
            // 開発用: SHIRUSHI_REFERENCES_PROBE="row:col" で ⇧F12 参照検索（既定 8s 後）。
            if let Ok(probe) = std::env::var("SHIRUSHI_REFERENCES_PROBE") {
                if let Some((row, column)) = probe
                    .split_once(':')
                    .and_then(|(r, c)| Some((r.parse::<usize>().ok()?, c.parse::<usize>().ok()?)))
                {
                    if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                        let delay_ms = std::env::var("SHIRUSHI_TYPE_PROBE_DELAY_MS")
                            .ok()
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(8000);
                        cx.spawn(async move |_workspace, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(delay_ms))
                                .await;
                            let _ = handle.update(cx, |workspace, window, cx| {
                                workspace.debug_references_probe(row, column, window, cx);
                            });
                        })
                        .detach();
                    }
                }
            }
            // 開発用: SHIRUSHI_CODEACTION_PROBE="row:col" で ⌘. を開く（既定 8s 後）。
            if let Ok(probe) = std::env::var("SHIRUSHI_CODEACTION_PROBE") {
                if let Some((row, column)) = probe
                    .split_once(':')
                    .and_then(|(r, c)| Some((r.parse::<usize>().ok()?, c.parse::<usize>().ok()?)))
                {
                    if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                        let delay_ms = std::env::var("SHIRUSHI_TYPE_PROBE_DELAY_MS")
                            .ok()
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(8000);
                        let confirm = std::env::var_os("SHIRUSHI_CODEACTION_CONFIRM").is_some();
                        cx.spawn(async move |_workspace, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(delay_ms))
                                .await;
                            let _ = handle.update(cx, |workspace, window, cx| {
                                workspace.debug_code_actions_probe(row, column, window, cx);
                            });
                            if confirm {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(2500))
                                    .await;
                                let _ = handle.update(cx, |workspace, window, cx| {
                                    workspace.debug_confirm_code_action(window, cx);
                                });
                                // resolve → 適用 → 保存の余韻。
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(1500))
                                    .await;
                                let _ = handle.update(cx, |workspace, window, cx| {
                                    workspace.debug_confirm_code_action(window, cx);
                                });
                            }
                        })
                        .detach();
                    }
                }
            }
            // 開発用: SHIRUSHI_FORMAT_PROBE=1 で LSP 初期化後にフォーマット→保存を実行（既定 8s 後）。
            if std::env::var_os("SHIRUSHI_FORMAT_PROBE").is_some() {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    let delay_ms = std::env::var("SHIRUSHI_TYPE_PROBE_DELAY_MS")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(8000);
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(delay_ms))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_format_probe(window, cx);
                        });
                    })
                    .detach();
                }
            }
        // 開発用: SHIRUSHI_HOTEXIT_AUTORESTORE=1 で復元バーの「復元」を自動で押す（1.5s 後）。
            if std::env::var_os("SHIRUSHI_HOTEXIT_AUTORESTORE").is_some() {
                if let Some(handle) = window.window_handle().downcast::<Workspace>() {
                    cx.spawn(async move |_workspace, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1500))
                            .await;
                        let _ = handle.update(cx, |workspace, window, cx| {
                            workspace.debug_restore_hot_exit(window, cx);
                        });
                    })
                    .detach();
                }
            }
            if let Some(paths) = extra_tabs {
                workspace.open_paths(paths, window, cx);
            }
            // 開発用: SHIRUSHI_BUFFER_SEARCH=<query> で ⌘F バーを開いた状態で撮る
            // （SHIRUSHI_BUFFER_REPLACE=<text> があれば置換行も開く）。
            if let Ok(query) = std::env::var("SHIRUSHI_BUFFER_SEARCH") {
                let replace = std::env::var("SHIRUSHI_BUFFER_REPLACE").ok();
                workspace.debug_open_buffer_search(query, replace, window, cx);
            } else {
                let handle = workspace.focus_handle(cx);
                window.focus(&handle, cx);
            }
        }) {
            eprintln!("初期化に失敗: {error}");
        }
        // 開発用: SHIRUSHI_TYPE_PROBE="row:col:text"（0 始まり）で起動後にタイプを注入する
        // （補完の自動トリガ検証。LSP の初期化を待つため SHIRUSHI_TYPE_PROBE_DELAY_MS 後・既定 5000）。
        if let Ok(probe) = std::env::var("SHIRUSHI_TYPE_PROBE") {
            let parts: Vec<&str> = probe.splitn(3, ':').collect();
            if let [row, column, text] = parts[..] {
                if let (Ok(row), Ok(column)) = (row.parse::<usize>(), column.parse::<usize>()) {
                    let text = text.to_string();
                    let delay_ms = std::env::var("SHIRUSHI_TYPE_PROBE_DELAY_MS")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(5000);
                    let probe_window = window;
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(delay_ms))
                            .await;
                        let result = probe_window.update(cx, |workspace, window, cx| {
                            workspace.debug_type_probe(row, column, text, window, cx);
                        });
                        if let Err(error) = result {
                            eprintln!("type probe 失敗: {error:?}");
                        }
                    })
                    .detach();
                }
            }
        }
        // 開発用: SHIRUSHI_HOVER_PROBE="row:col"（0 始まり）でキャレットを置き ⌘K ⌘I 相当の hover を出す。
        if let Ok(probe) = std::env::var("SHIRUSHI_HOVER_PROBE") {
            if let Some((row, column)) = probe
                .split_once(':')
                .and_then(|(row, column)| Some((row.parse::<usize>().ok()?, column.parse::<usize>().ok()?)))
            {
                let delay_ms = std::env::var("SHIRUSHI_TYPE_PROBE_DELAY_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(5000);
                let probe_window = window;
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(delay_ms))
                        .await;
                    let result = probe_window.update(cx, |workspace, window, cx| {
                        workspace.debug_hover_probe(row, column, window, cx);
                    });
                    if let Err(error) = result {
                        eprintln!("hover probe 失敗: {error:?}");
                    }
                })
                .detach();
            }
        }
        // 開発用: SHIRUSHI_RAIL_PROBE=open-branch:<name>|remove-active でレールのフローを実駆動（M10-2 検証）。
        if let Ok(probe) = std::env::var("SHIRUSHI_RAIL_PROBE") {
            let delay_ms = std::env::var("SHIRUSHI_RAIL_PROBE_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(400);
            let probe_window = window;
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(delay_ms))
                    .await;
                let result = probe_window.update(cx, |workspace, window, cx| {
                    workspace.debug_rail_probe(&probe, window, cx);
                });
                if let Err(error) = result {
                    eprintln!("rail probe 失敗: {error:?}");
                }
            })
            .detach();
        }
        if std::env::var_os("SHIRUSHI_STARTUP_LOG").is_some() {
            println!("startup_ms={:.1}", startup.elapsed().as_secs_f64() * 1000.0);
        }
        cx.activate(true);
        maybe_capture_screenshot(window, cx);
    });
}

/// 開発用ヘッドレススクショ: `SHIRUSHI_SCREENSHOT=<path>` で数フレーム後に render_to_image →
/// PNG 保存 → 終了。`--features screenshot` 時のみ（test-support で render_to_image を使うため）。
/// このオフスクリーン経路は font-kit 経由でグリフ（テキスト）も写る。
/// 連写: `SHIRUSHI_SCREENSHOT_FRAMES=<n>` + `SHIRUSHI_SCREENSHOT_INTERVAL_MS=<ms>`（既定 500）で
/// 1回の起動から n 枚を等間隔保存（`shot.png` → `shot-000.png`, `shot-001.png`, …）。
/// ACP ストリーミングやマスコットのアニメを GIF 化する素材撮りに使う。
#[cfg(feature = "screenshot")]
fn maybe_capture_screenshot(window: gpui::WindowHandle<Workspace>, cx: &mut App) {
    use std::time::Duration;
    let Ok(screenshot_path) = std::env::var("SHIRUSHI_SCREENSHOT") else {
        return;
    };
    cx.spawn(async move |cx| {
        // 既定 600ms。ACP プローブ検証時は SHIRUSHI_SCREENSHOT_DELAY_MS で応答到着まで延ばす。
        let delay_ms = std::env::var("SHIRUSHI_SCREENSHOT_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(600);
        let frames = std::env::var("SHIRUSHI_SCREENSHOT_FRAMES")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|count| *count >= 1)
            .unwrap_or(1);
        let interval_ms = std::env::var("SHIRUSHI_SCREENSHOT_INTERVAL_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(500);
        cx.background_executor().timer(Duration::from_millis(delay_ms)).await;
        let capture_started = std::time::Instant::now();
        for frame in 0..frames {
            if frame > 0 {
                cx.background_executor().timer(Duration::from_millis(interval_ms)).await;
            }
            // 連写時は `shot.png` → `shot-007.png` の形で連番を差し込む。
            let frame_path = if frames == 1 {
                screenshot_path.clone()
            } else {
                let path = std::path::Path::new(&screenshot_path);
                let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("shot");
                let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
                parent.join(format!("{stem}-{frame:03}.png")).to_string_lossy().into_owned()
            };
            let captured =
                cx.update(|cx| window.update(cx, |_root, window, _cx| window.render_to_image()));
            match captured {
                Ok(Ok(image)) => match image.save(&frame_path) {
                    // 経過 ms は連写を動画へ組み直すとき（実時間タイムスタンプ）に使う。
                    Ok(()) => eprintln!(
                        "スクショ保存: {frame_path} @{}ms",
                        capture_started.elapsed().as_millis()
                    ),
                    Err(error) => eprintln!("PNG 保存失敗: {error}"),
                },
                Ok(Err(error)) => eprintln!("render_to_image 失敗: {error:?}"),
                Err(error) => eprintln!("ウィンドウ更新失敗: {error:?}"),
            }
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

#[cfg(not(feature = "screenshot"))]
fn maybe_capture_screenshot(_window: gpui::WindowHandle<Workspace>, _cx: &mut App) {}
