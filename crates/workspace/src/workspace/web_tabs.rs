//! web_tabs — URL を開く入口と Web タブ（`web_preview_view`）の開閉。
//!
//! URL を開く経路はここの [`Workspace::open_url`] 1 本に集める: localhost 系（`webview_view::localhost`
//! の範囲）は Web タブ、それ以外の http(s) は既定のブラウザ。エージェントの transcript のリンクと
//! Markdown プレビューのリンクがここを通る（端末の URL クリックも統合時にここへ繋ぐ）。
//!
//! URL は**出どころ**（[`UrlOrigin`]）と一緒に渡す。SSH のプロジェクトではエージェントも端末も
//! リモートで動くので、そこに出た `localhost:5173` はリモートの localhost — 手元で開くと、手元で
//! 同じポートを使う別のアプリを見てしまう。ポート転送（計画 O5）が入るまでは開かずに案内する。

use crate::workspace::*;
use webview_view::localhost;

/// URL が出てきた機械（＝その URL の `localhost` が指す先）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UrlOrigin {
    /// 手元の機械（ローカルのプロジェクト・Chat・手元のファイル）。
    Local,
    /// SSH の接続先。`host_id` は [`host::Host::id`]（接続の識別・再接続を跨いで同じ）、
    /// `destination` は案内に出す `user@host`（[`host::Host::display_name`]）。
    Ssh {
        host_id: SharedString,
        destination: SharedString,
    },
}

impl UrlOrigin {
    /// その Host の上で動く物（エージェント・端末・ファイル）から出た URL の出どころ。
    /// 読むのは Host の固定の欄だけ（I/O 無し）。
    pub(crate) fn of_host(host: &dyn host::Host) -> UrlOrigin {
        if host.is_remote() {
            UrlOrigin::Ssh {
                host_id: SharedString::from(host.id().to_string()),
                destination: SharedString::from(host.display_name().to_string()),
            }
        } else {
            UrlOrigin::Local
        }
    }
}

/// Markdown のリンクの行き先をファイルのパスへ（`base` = その `.md` のフォルダ）。
/// URL（`https:` など scheme 付き）・ページ内アンカー（`#…`）・空は `None`。`?` / `#` 以降は落とし、
/// `%20` などのエスケープは戻す。
pub(crate) fn resolve_link_path(base: Option<&Path>, destination: &str) -> Option<PathBuf> {
    let destination = destination.trim();
    let without_suffix = destination
        .split(['#', '?'])
        .next()
        .unwrap_or_default()
        .trim();
    if without_suffix.is_empty() || has_scheme(without_suffix) {
        return None;
    }
    let decoded = percent_decode(without_suffix).unwrap_or_else(|| without_suffix.to_string());
    let path = PathBuf::from(&decoded);
    if path.is_absolute() {
        return Some(path);
    }
    base.map(|base| base.join(path))
}

/// `mailto:` / `https:` のような scheme を持つか（Windows のドライブ `C:\` は scheme と見なさない）。
fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
}

/// `%XX` を戻す。不正なエスケープや UTF-8 として読めない結果は `None`（呼び側は原文のまま使う）。
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

impl Workspace {
    /// プロジェクトの session（添字）の上で出た URL の出どころ。エージェントも端末もプロジェクトの
    /// Host で動くので、その Host で決まる。
    pub(crate) fn url_origin_of_session(&self, session_index: usize) -> UrlOrigin {
        self.project_sessions
            .projects
            .get(session_index)
            .map(|slot| UrlOrigin::of_host(slot.worktree.host().as_ref()))
            .unwrap_or(UrlOrigin::Local)
    }

    /// URL を開く（唯一の入口）。localhost 系は Web タブ、それ以外は既定のブラウザ。
    ///
    /// `origin` が SSH の接続先で、URL がその機械自身（`localhost`・ループバック・`0.0.0.0` など）を
    /// 指す時は**開かない**: 手元の Web タブでもブラウザでも手元の機械の同じポートを見てしまうので、
    /// 理由をトーストで示す（「既定のブラウザで開く」も同じ理由で取り違えるので出さない）。
    ///
    /// Window を取らない形にしてあるので、Window の無いイベント購読（エージェントのパネル・端末）からも
    /// そのまま呼べる。Web タブを開くのは次の effect cycle（`process_pending_shell_effects`）。
    pub(crate) fn open_url(&mut self, url: &str, origin: UrlOrigin, cx: &mut Context<Self>) {
        match origin {
            UrlOrigin::Ssh { destination, .. } => {
                if localhost::points_to_origin_machine(url) {
                    let color = self.accent();
                    self.push_toast(
                        i18n::t!(
                            "webtab.remote_localhost",
                            "host" => destination,
                            "target" => localhost::display_label(url)
                        )
                        .into(),
                        color,
                        cx,
                    );
                    return;
                }
                // 公開の URL はどこで開いても同じ所を指す＝既定のブラウザへ（下へ落ちる）。
            }
            UrlOrigin::Local => {
                if let Some(url) = localhost::normalize(url) {
                    self.pending_web_url = Some(url);
                    cx.notify();
                    return;
                }
            }
        }
        if let Err(error) = crate::crash::open_url(url) {
            eprintln!("URL を開けない: {error:#}");
            let color = self.accent();
            self.push_toast(
                i18n::t!("link.open_failed", "target" => url).into(),
                color,
                cx,
            );
        }
    }

    /// Web タブを開く（同じ URL のタブがあればそこへ移る）。対話での入口なので、設定ホーム・
    /// AI 全画面・Fleet を畳んでエディタ領域を前に出す（Fleet には Web タブを見せる面が無い）。
    pub(crate) fn open_web_tab(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.chrome.show_settings = false;
        self.exit_agent_full_screen(cx);
        if self.chrome.fleet_mode {
            self.chrome.fleet_mode = false;
        }
        self.agent_active = false;
        self.show_web_tab(url, window, cx);
    }

    /// Web タブを出す（あれば切り替え・無ければ末尾に足してアクティブに）。モードは触らない
    /// （起動時の復元・レール切替のタブ復元もここを通る）。
    pub(crate) fn show_web_tab(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = web_tab_key(&url);
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == key) {
            self.select_tab(index, window, cx);
            return;
        }
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let theme = self.theme.clone();
        let view = cx.new(|cx| WebPreviewView::new(&url, theme, cx));
        let evict_minutes = settings::get(cx).html_preview_evict_minutes;
        view.update(cx, |view, cx| view.set_evict_minutes(evict_minutes, cx));
        // タイトル・読み込み状態の変化でタブ名を描き直す。
        let observation = cx.observe(&view, |_, _, cx| cx.notify());
        self.tabs.push(EditorTab {
            path: key,
            content: TabContent::Web {
                view: view.clone(),
                _observation: observation,
            },
            transient: false,
        });
        self.active_tab = self.tabs.len() - 1;
        let handle = view.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // WebView は最初の描画で作られ、その時に OS のキーボードフォーカスを受け取る。
        view.update(cx, |view, cx| view.set_surface_active(true, true, cx));
        self.sync_active_slot();
        self.save_state(cx);
        cx.notify();
    }

    /// パレット「プレビュー: localhost を開く…」。入力欄にポート番号か localhost の URL を打つ
    /// （それ以外は通さない＝任意の URL を開く欄にはしない）。開くのは**手元の** localhost —
    /// SSH のプロジェクトにいる時は、入力欄でそれを言う（SSH 先のサーバは手元へ転送してから）。
    pub(crate) fn open_localhost_preview(
        &mut self,
        _: &OpenLocalhostPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Chat 中は `active_slot` が無い＝手元扱い。
        let remote = self
            .active_slot()
            .is_some_and(|slot| slot.remote_host.is_some());
        let placeholder = if remote {
            i18n::t!("webtab.picker_placeholder_remote")
        } else {
            i18n::t!("webtab.picker_placeholder")
        };
        self.open_picker(PickerMode::PreviewUrl, placeholder, Vec::new(), window, cx);
        if let Some(picker) = self.overlays.picker.clone() {
            picker.update(cx, |picker, cx| {
                picker.set_query_action(0, i18n::t!("webtab.picker_action"), cx)
            });
        }
    }

    /// パレットの入力を確定した（`on_picker_event`）。
    pub(crate) fn confirm_localhost_input(
        &mut self,
        input: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match localhost::parse_input(input) {
            Some(url) => self.open_web_tab(url, window, cx),
            None => {
                let color = self.accent();
                self.push_toast(
                    i18n::t!("webtab.invalid_url", "input" => input.trim()).into(),
                    color,
                    cx,
                );
            }
        }
    }

    /// Markdown プレビューのリンクを開く。URL は [`Self::open_url`]、ファイルへのリンクはそのファイルを
    /// 開く（`.md` ならプレビュー表示のまま）。ページ内アンカーと未知の scheme は何もしない。
    pub(crate) fn on_preview_link(
        &mut self,
        editor: &Entity<EditorView>,
        event: &editor_view::PreviewLinkClicked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = event.destination.trim();
        let (base, origin) = {
            let view = editor.read(cx);
            (
                view.buffer()
                    .path()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf),
                // SSH 先の Markdown に書かれた localhost は、SSH 先で動かすサーバの物。
                UrlOrigin::of_host(view.buffer().host().as_ref()),
            )
        };
        if destination.starts_with("http://") || destination.starts_with("https://") {
            self.open_url(destination, origin, cx);
            return;
        }
        let local = origin == UrlOrigin::Local;
        let Some(path) = resolve_link_path(base.as_deref(), destination) else {
            return;
        };
        // ローカルで見つからないリンクは黙って捨てず、開けなかったと言う。
        if local && !path.is_file() {
            let color = self.accent();
            self.push_toast(
                i18n::t!("link.open_failed", "target" => path.display().to_string()).into(),
                color,
                cx,
            );
            return;
        }
        self.record_nav_position(cx);
        if lang::language_for_path(&path) == Some(lang::LanguageId::Markdown) {
            self.open_file_then(path, window, cx, |editor, cx| {
                editor.set_rendered_markdown(true, cx)
            });
        } else {
            self.open_file(path, window, cx);
        }
    }

    /// 開発用: Web タブを検証する（`NECODER_WEB_PREVIEW_PROBE`・`;` 区切りで順に実行）。
    ///
    /// `open:<url>` = open_url と同じ入口（手元から来た URL）/ `open-ssh:<url>` = SSH 先
    /// （`probe@devbox`）から来た URL として open_url へ / `palette:<入力>` = パレットの確定と同じ入口 /
    /// `picker:<入力>` = 入力欄を開いて文字を入れる（確定しない）/
    /// `viewport:<full|幅>` / `zoom:<+|-|0>` / `reload` / `eval:<式>` = ページで式を評価して結果を標準エラーへ /
    /// `menu` = タブの右クリックメニュー / `overlay` = ⌘⇧P（オーバーレイ中は WebView を隠す）/
    /// `close-overlay` /
    /// `state` = タブ列と WebView の状態を標準エラーへ。
    #[cfg(debug_assertions)]
    pub fn debug_web_preview_probe(
        &mut self,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, argument) = command.split_once(':').unwrap_or((command, ""));
        let active_web = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.web().cloned());
        match name {
            "open" => self.open_url(argument, UrlOrigin::Local, cx),
            "open-ssh" => self.open_url(
                argument,
                UrlOrigin::Ssh {
                    host_id: "ssh://probe@devbox".into(),
                    destination: "probe@devbox".into(),
                },
                cx,
            ),
            "palette" => self.confirm_localhost_input(argument, window, cx),
            // 入力欄を開いて文字を入れた状態（確定はしない）を撮る。
            "picker" => {
                self.open_localhost_preview(&OpenLocalhostPreview, window, cx);
                if let Some(picker) = self.overlays.picker.clone() {
                    picker.update(cx, |picker, cx| picker.set_query(argument, cx));
                }
            }
            "viewport" => {
                if let Some(web) = active_web {
                    let viewport = match argument.parse::<u32>() {
                        Ok(width) => web_preview_view::Viewport::Width(width),
                        Err(_) => web_preview_view::Viewport::Full,
                    };
                    web.update(cx, |web, cx| web.set_viewport(viewport, cx));
                }
            }
            "zoom" => {
                if let Some(web) = active_web {
                    let direction = match argument {
                        "+" => 1,
                        "-" => -1,
                        _ => 0,
                    };
                    web.update(cx, |web, cx| web.step_zoom(direction, cx));
                }
            }
            "reload" => {
                if let Some(web) = active_web {
                    web.update(cx, |web, cx| web.reload(cx));
                }
            }
            "eval" => {
                if let Some(web) = active_web {
                    web.read(cx)
                        .debug_evaluate(argument.to_string(), argument, cx);
                }
            }
            "menu" => {
                let index = self.active_tab;
                self.open_tab_menu(index, point(px(520.), px(76.)), cx);
            }
            "overlay" => self.open_command_palette(&CommandPalette, window, cx),
            "close-overlay" => self.close_picker(window, cx),
            "state" => {
                let tabs: Vec<String> = self
                    .tabs
                    .iter()
                    .map(|tab| tab.path.to_string_lossy().to_string())
                    .collect();
                let web = active_web.map(|web| {
                    let web = web.read(cx);
                    (
                        web.url().to_string(),
                        web.current_url(cx),
                        web.tab_label(cx),
                    )
                });
                eprintln!(
                    "WEB_PREVIEW_PROBE state: tabs={tabs:?} active={} web={web:?} fleet={} open_files={:?}",
                    self.active_tab,
                    self.chrome.fleet_mode,
                    self.active_slot().map(|slot| slot.open_files.clone())
                );
            }
            other => eprintln!("WEB_PREVIEW_PROBE: 未知のコマンド {other}"),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        project: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str, cx: &mut gpui::TestAppContext) -> Fixture {
            let root =
                std::env::temp_dir().join(format!("necoder_web_tab_{tag}_{}", std::process::id()));
            let project = root.join("site");
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("前回の残りを消せる");
            }
            std::fs::create_dir_all(&project).expect("一時フォルダを作れる");
            // 言語サーバの無い拡張子にする（LSP が別スレッドで動くと決定的なテストにならない）。
            std::fs::write(project.join("a.txt"), "a\n").expect("書ける");
            std::fs::write(project.join("b.txt"), "b\n").expect("書ける");
            let settings_path = root.join("settings.json");
            std::fs::write(&settings_path, r#"{"onboarded":true}"#).expect("書ける");
            cx.update(|cx| settings::init(Some(settings_path), None, cx));
            // テスト窓は raw window handle を持たない＝ネイティブ子ビューは作らずに配線だけ通す。
            ::webview_view::disable_native_webviews_for_tests();
            Fixture { root, project }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.root) {
                eprintln!("一時フォルダを消せない: {error}");
            }
        }
    }

    fn stop_watchers(workspace: &mut Workspace) {
        for session in workspace.project_sessions.sessions.iter_mut() {
            session._watch = None;
            session._watch_pump = None;
        }
    }

    /// SSH のプロジェクトを装う Host。名乗り（id / 表示名 / is_remote）だけリモートで、中身は手元の
    /// ファイルへ流す（URL の出どころの判定は名乗りしか見ない）。
    struct SshLikeHost {
        inner: Arc<dyn host::Host>,
    }

    impl SshLikeHost {
        fn shared() -> Arc<dyn host::Host> {
            Arc::new(SshLikeHost {
                inner: host::LocalHost::shared(),
            })
        }
    }

    impl host::Host for SshLikeHost {
        fn id(&self) -> &str {
            "ssh://me@devbox"
        }

        fn display_name(&self) -> &str {
            "me@devbox"
        }

        fn is_remote(&self) -> bool {
            true
        }

        fn project_uri(&self, path: &Path) -> Option<String> {
            Some(format!("ssh://me@devbox{}", path.display()))
        }

        fn host_for_project(&self, path: &Path) -> anyhow::Result<Arc<dyn host::Host>> {
            self.inner.host_for_project(path)
        }

        fn canonicalize(&self, path: &Path) -> anyhow::Result<PathBuf> {
            self.inner.canonicalize(path)
        }

        fn metadata(&self, path: &Path) -> anyhow::Result<host::HostMetadata> {
            self.inner.metadata(path)
        }

        fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<host::HostEntry>> {
            self.inner.read_dir(path)
        }

        fn read_file(&self, path: &Path) -> anyhow::Result<host::FileContent> {
            self.inner.read_file(path)
        }

        fn write_file(
            &self,
            path: &Path,
            bytes: &[u8],
            condition: host::WriteCondition,
        ) -> anyhow::Result<host::FileRevision> {
            self.inner.write_file(path, bytes, condition)
        }

        fn list_files(&self, root: &Path, limit: usize) -> anyhow::Result<Vec<PathBuf>> {
            self.inner.list_files(root, limit)
        }

        fn search_project(
            &self,
            root: &Path,
            spec: &host::TextSearchSpec,
            file_limit: usize,
        ) -> anyhow::Result<Vec<host::TextSearchHit>> {
            self.inner.search_project(root, spec, file_limit)
        }

        fn run_command(&self, spec: &host::CommandSpec) -> anyhow::Result<host::CommandOutput> {
            self.inner.run_command(spec)
        }

        fn spawn_process(&self, spec: &host::CommandSpec) -> anyhow::Result<host::HostProcess> {
            self.inner.spawn_process(spec)
        }

        fn terminal_launch(&self, cwd: &Path) -> anyhow::Result<Option<host::TerminalLaunch>> {
            self.inner.terminal_launch(cwd)
        }
    }

    fn toast_texts(workspace: &Workspace) -> Vec<String> {
        workspace
            .notifications
            .toasts
            .iter()
            .map(|(text, ..)| text.to_string())
            .collect()
    }

    fn web_tab_count(workspace: &Workspace) -> usize {
        workspace
            .project_sessions
            .sessions
            .iter()
            .flat_map(|session| session.tabs.iter())
            .filter(|tab| tab.web().is_some())
            .count()
    }

    /// 台帳 R06: SSH のプロジェクトのエージェントが出した localhost は SSH 先の物。手元の Web タブでも
    /// ブラウザでも開かず、理由を出す。手元のプロジェクトの同じ URL は Web タブで開く（取り違えない）。
    /// ブラウザなら手元へ繋がってしまう表記（`0.0.0.0`・`127.0.0.2`）も同じ扱い。
    #[gpui::test]
    fn transcript_localhost_from_an_ssh_project_is_not_opened_here(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("ssh_transcript", cx);
        let remote_root = fixture.root.join("remote");
        std::fs::create_dir_all(&remote_root).expect("一時フォルダを作れる");
        let sources = vec![
            ProjectSource::new(host::LocalHost::shared(), fixture.project.clone()),
            ProjectSource::new(SshLikeHost::shared(), remote_root),
        ];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        let (local_panel, remote_panel) = workspace.update_in(cx, |workspace, _window, _cx| {
            stop_watchers(workspace);
            assert_eq!(workspace.url_origin_of_session(0), UrlOrigin::Local);
            assert_eq!(
                workspace.url_origin_of_session(1),
                UrlOrigin::Ssh {
                    host_id: "ssh://me@devbox".into(),
                    destination: "me@devbox".into(),
                }
            );
            (
                workspace.project_sessions.sessions[0].agent_panel.clone(),
                workspace.project_sessions.sessions[1].agent_panel.clone(),
            )
        });
        for url in [
            "http://localhost:5173/",
            "http://0.0.0.0:5173/",
            "http://127.0.0.2:5173/",
        ] {
            remote_panel.update(cx, |_panel, cx| {
                cx.emit(agent_panel::PanelEvent::OpenUrlRequest { url: url.into() })
            });
            cx.run_until_parked();
        }
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.process_pending_shell_effects(window, cx);
            assert_eq!(
                web_tab_count(workspace),
                0,
                "SSH 先の localhost は手元で開かない"
            );
            let toasts = toast_texts(workspace);
            assert_eq!(toasts.len(), 3, "{toasts:?}");
            assert!(
                toasts[0].contains("me@devbox") && toasts[0].contains("localhost:5173"),
                "どこの何を開かなかったかを言う: {toasts:?}"
            );
            assert!(toasts[1].contains("0.0.0.0:5173"), "{toasts:?}");
            assert!(toasts[2].contains("127.0.0.2:5173"), "{toasts:?}");
        });
        local_panel.update(cx, |_panel, cx| {
            cx.emit(agent_panel::PanelEvent::OpenUrlRequest {
                url: "http://localhost:5173/".into(),
            })
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.process_pending_shell_effects(window, cx);
            assert_eq!(
                workspace.tabs.first().map(|tab| tab.path.clone()),
                Some(PathBuf::from("http://localhost:5173/")),
                "手元のプロジェクトの localhost は Web タブで開く"
            );
            stop_watchers(workspace);
        });
    }

    /// 台帳 R06: Markdown のリンクも、そのファイルがある機械で決まる。SSH 先の README に書かれた
    /// localhost は開かず、手元の README の物は Web タブで開く。
    #[gpui::test]
    fn markdown_localhost_links_follow_the_file_host(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("ssh_markdown", cx);
        let remote_root = fixture.root.join("remote");
        std::fs::create_dir_all(&remote_root).expect("一時フォルダを作れる");
        let readme = "[app](http://localhost:3000/)\n";
        std::fs::write(remote_root.join("README.md"), readme).expect("書ける");
        std::fs::write(fixture.project.join("README.md"), readme).expect("書ける");
        let link = editor_view::PreviewLinkClicked {
            destination: "http://localhost:3000/".to_string(),
        };
        let remote_buffer = Buffer::from_host(SshLikeHost::shared(), remote_root.join("README.md"))
            .expect("SSH 先を装った README を読める");
        let local_buffer =
            Buffer::from_file(fixture.project.join("README.md")).expect("手元の README を読める");
        let remote_editor =
            cx.new(|cx| EditorView::new(remote_buffer, Theme::dark(), project_color(0), cx));
        let local_editor =
            cx.new(|cx| EditorView::new(local_buffer, Theme::dark(), project_color(0), cx));
        let sources = vec![ProjectSource::new(
            host::LocalHost::shared(),
            fixture.project.clone(),
        )];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.on_preview_link(&remote_editor, &link, window, cx);
            workspace.process_pending_shell_effects(window, cx);
            assert_eq!(web_tab_count(workspace), 0);
            assert!(
                toast_texts(workspace)
                    .iter()
                    .any(|toast| toast.contains("me@devbox") && toast.contains("localhost:3000")),
                "{:?}",
                toast_texts(workspace)
            );
            workspace.on_preview_link(&local_editor, &link, window, cx);
            workspace.process_pending_shell_effects(window, cx);
            assert_eq!(
                web_tab_count(workspace),
                1,
                "手元の Markdown の localhost は開く"
            );
            stop_watchers(workspace);
        });
    }

    /// Web タブは窓セッションの `open_files` に URL のまま載り、再起動で**同じ位置に**戻ること。
    /// 範囲外の URL（保存形式に紛れ込んだ物）は Web タブとして開かない。
    #[gpui::test]
    fn web_tabs_come_back_after_a_restart_in_their_place(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("restore", cx);
        let (file_a, file_b) = (fixture.project.join("a.txt"), fixture.project.join("b.txt"));
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            let restored = RestoredTabs {
                files: vec![
                    file_a.clone(),
                    PathBuf::from("http://localhost:5173/"),
                    PathBuf::from("https://example.com/"),
                    file_b.clone(),
                ],
                active: 1,
            };
            workspace.restore_open_file(&[restored], window, cx);
            let paths: Vec<PathBuf> = workspace.tabs.iter().map(|tab| tab.path.clone()).collect();
            assert_eq!(
                paths,
                vec![
                    file_a.clone(),
                    PathBuf::from("http://localhost:5173/"),
                    file_b.clone()
                ],
                "localhost の Web タブは並びを保って戻り、外部の URL は開かない"
            );
            assert!(
                workspace.tabs[1].web().is_some(),
                "URL の鍵は Web タブになる"
            );
            assert_eq!(workspace.active_tab, 1, "アクティブも戻る");
            assert_eq!(
                workspace.persisted_state().projects[0].open_files,
                paths,
                "保存する時も URL のまま同じ並びで書く（保存形式は変えない）"
            );
            stop_watchers(workspace);
        });
    }

    /// transcript などから URL を開くと、localhost 系は Web タブになり、同じ URL は重複しない。
    /// オーバーレイ中は WebView を隠し（キーも GPUI へ返し）、閉じたら戻す。
    #[gpui::test]
    fn localhost_urls_open_one_web_tab_that_steps_aside_for_overlays(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = Fixture::new("open", cx);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        let view = workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.open_url("http://127.0.0.1:3000", UrlOrigin::Local, cx);
            workspace.process_pending_shell_effects(window, cx);
            workspace.open_url("http://127.0.0.1:3000/", UrlOrigin::Local, cx);
            workspace.process_pending_shell_effects(window, cx);
            assert_eq!(workspace.tabs.len(), 1, "同じ URL は同じタブへ戻る");
            let tab = &workspace.tabs[0];
            assert_eq!(tab.path, PathBuf::from("http://127.0.0.1:3000/"));
            assert!(
                workspace.active_editor().is_none(),
                "Web タブはバッファを持たない"
            );
            tab.web().cloned().expect("Web タブが開く")
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, cx| view.is_surface_active(cx)));
        assert!(
            view.read_with(cx, |view, cx| view.wants_key_focus(cx)),
            "開いた Web タブはキーを WebView へ渡す"
        );

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_command_palette(&CommandPalette, window, cx);
        });
        cx.run_until_parked();
        assert!(
            !view.read_with(cx, |view, cx| view.is_surface_active(cx)),
            "オーバーレイ中に WebView を残すと、手前に居座ってパレットを隠す"
        );
        assert!(!view.read_with(cx, |view, cx| view.wants_key_focus(cx)));

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.close_picker(window, cx);
        });
        cx.run_until_parked();
        assert!(
            view.read_with(cx, |view, cx| view.is_surface_active(cx)),
            "閉じたら戻る"
        );
        workspace.update_in(cx, |workspace, _window, _cx| stop_watchers(workspace));
    }

    /// パレットの入力欄は localhost しか通さない（任意の URL を開く欄にしない）。
    #[gpui::test]
    fn the_palette_input_only_accepts_localhost(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("palette", cx);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.confirm_localhost_input("example.com", window, cx);
            assert!(workspace.tabs.is_empty(), "外部のホストは開かない");
            workspace.confirm_localhost_input("5173", window, cx);
            assert_eq!(
                workspace.tabs.first().map(|tab| tab.path.clone()),
                Some(PathBuf::from("http://localhost:5173/"))
            );
            stop_watchers(workspace);
        });
    }

    #[test]
    fn markdown_links_resolve_against_the_file_folder() {
        let base = Path::new("/work/project/docs");
        assert_eq!(
            resolve_link_path(Some(base), "guide.md"),
            Some(PathBuf::from("/work/project/docs/guide.md"))
        );
        assert_eq!(
            resolve_link_path(Some(base), "../README.md#install"),
            Some(PathBuf::from("/work/project/docs/../README.md"))
        );
        assert_eq!(
            resolve_link_path(Some(base), "my%20notes.md?raw=1"),
            Some(PathBuf::from("/work/project/docs/my notes.md"))
        );
        assert_eq!(
            resolve_link_path(Some(base), "/etc/hosts"),
            Some(PathBuf::from("/etc/hosts"))
        );
        // 無題バッファ（フォルダが無い）の相対リンクは解決しない。
        assert_eq!(resolve_link_path(None, "guide.md"), None);
    }

    #[test]
    fn urls_and_anchors_are_not_files() {
        let base = Some(Path::new("/work"));
        for destination in [
            "",
            "#top",
            "https://example.com/",
            "mailto:someone@example.com",
            "javascript:alert(1)",
            "vscode://open",
        ] {
            assert_eq!(resolve_link_path(base, destination), None, "{destination}");
        }
        // Windows のドライブは scheme ではない。
        assert!(!has_scheme("C:\\work\\a.md"));
        assert!(has_scheme("mailto:someone@example.com"));
    }
}
