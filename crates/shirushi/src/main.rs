//! Shirushi（しるし）— GPUI ベースの自作エディタ。エントリポイント。
//!
//! `shirushi [<path>...]` で各 path をプロジェクト（レール項目）として開く。file を渡すと親を
//! プロジェクト、その file をエディタに開く。引数無しは前回状態（`state.json`）を復元する。

use gpui::{
    App, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, actions, point,
    prelude::*, px, size,
};
use gpui_platform::application;
use std::path::{Path, PathBuf};
use std::time::Instant;
use workspace::Workspace;

actions!(shirushi, [Quit]);

/// コマンドライン引数（or 前回状態）から (プロジェクトルート群, 各プロジェクトの開ファイル) を決める。
fn resolve_projects() -> (Vec<PathBuf>, Vec<Option<PathBuf>>) {
    let mut roots = Vec::new();
    let mut open_files = Vec::new();

    for arg in std::env::args().skip(1) {
        let path = PathBuf::from(&arg);
        if path.is_file() {
            let file = std::fs::canonicalize(&path).unwrap_or(path);
            let parent = file
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            roots.push(parent);
            open_files.push(Some(file));
        } else if path.is_dir() {
            roots.push(std::fs::canonicalize(&path).unwrap_or(path));
            open_files.push(None);
        } else {
            eprintln!("見つからない（スキップ）: {arg}");
        }
    }

    // 引数無し → 前回状態を復元
    if roots.is_empty() {
        if let Some(state_path) = workspace::state_path() {
            if let Some((saved_roots, _active)) = workspace::load_state(&state_path) {
                let files = Workspace::persisted_open_files(&state_path);
                return (saved_roots, files);
            }
        }
    }
    // 最後の砦: カレントディレクトリ
    if roots.is_empty() {
        if let Ok(cwd) = std::env::current_dir() {
            roots.push(cwd);
            open_files.push(None);
        }
    }
    (roots, open_files)
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
    // GUI を開く前に CLI サブコマンドを処理（`shirushi config …`）。
    if run_config_cli() {
        return;
    }
    let startup = Instant::now();
    application().run(move |cx: &mut App| {
        load_fonts(cx);
        i18n::init_from_os_locale();

        // プロジェクトを解決（roots + 起動時に開くファイル）。先頭 root を project 設定の対象にする。
        let (roots, open_files) = resolve_projects();

        // 設定（default → user → project）を **反応的 global** に載せてファイル監視を開始する。
        // 以後 UI トグル / CLI / MCP / 手編集はすべてこの 1 つの store を更新し、live で波及する。
        settings::init(settings_core::user_settings_path(), roots.first().cloned(), cx);
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
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());

        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);

        let build_roots = roots.clone();
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
                    Workspace::new(
                        build_roots.clone(),
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

        if let Err(error) = window.update(cx, |workspace, window, cx| {
            workspace.restore_open_file(&open_files, window, cx);
            let handle = workspace.focus_handle(cx);
            window.focus(&handle, cx);
        }) {
            eprintln!("初期化に失敗: {error}");
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
/// 注意: このオフスクリーン経路はレイアウト・色・矩形は写るがグリフ（テキスト）は写らない。
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
        cx.background_executor().timer(Duration::from_millis(delay_ms)).await;
        let captured =
            cx.update(|cx| window.update(cx, |_root, window, _cx| window.render_to_image()));
        match captured {
            Ok(Ok(image)) => match image.save(&screenshot_path) {
                Ok(()) => eprintln!("スクショ保存: {screenshot_path}"),
                Err(error) => eprintln!("PNG 保存失敗: {error}"),
            },
            Ok(Err(error)) => eprintln!("render_to_image 失敗: {error:?}"),
            Err(error) => eprintln!("ウィンドウ更新失敗: {error:?}"),
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

#[cfg(not(feature = "screenshot"))]
fn maybe_capture_screenshot(_window: gpui::WindowHandle<Workspace>, _cx: &mut App) {}
