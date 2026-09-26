//! web_tabs — URL を開く入口と Web タブ（`web_preview_view`）の開閉。
//!
//! URL を開く経路はここの [`Workspace::open_url`] 1 本に集める: localhost 系（`webview_view::localhost`
//! の範囲）は Web タブ、それ以外の http(s) は既定のブラウザ。エージェントの transcript のリンクと
//! Markdown プレビューのリンクがここを通る（端末の URL クリックも統合時にここへ繋ぐ）。

use crate::workspace::*;
use webview_view::localhost;
use webview_view::static_server::StaticHosting;

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

/// 添付チップの名前は短く（要素の呼び名は長くなりうる）。
fn truncate_chip(label: &str) -> String {
    const MAX_CHARS: usize = 36;
    if label.chars().count() <= MAX_CHARS {
        return label.to_string();
    }
    let mut cut: String = label.chars().take(MAX_CHARS - 1).collect();
    cut.push('…');
    cut
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

/// URL をどこで開くか（R06）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UrlDestination {
    /// 手元の localhost の開発サーバ = necoder の Web タブ。
    WebTab,
    /// それ以外の URL = 既定のブラウザ。
    Browser,
    /// SSH 先で動いているプロジェクトから来た localhost の URL。手元の同じポートは別物なので
    /// 開かず、SSH 先のものだと知らせる（ポート転送は O5）。
    RemoteLocalhost { host: String, port: u16 },
}

/// URL の行き先を決める（R06・純関数）。`remote_host` は URL が出てきたプロジェクトの SSH 先
/// （手元のプロジェクト・Chat は `None`）。
pub(crate) fn url_destination(url: &str, remote_host: Option<&str>) -> UrlDestination {
    let Some(normalized) = localhost::normalize(url) else {
        return UrlDestination::Browser;
    };
    match remote_host {
        Some(host) => UrlDestination::RemoteLocalhost {
            host: host.to_string(),
            port: localhost_port(&normalized),
        },
        None => UrlDestination::WebTab,
    }
}

/// 正規形の localhost の URL のポート（省略時は http = 80 / https = 443）。
fn localhost_port(url: &str) -> u16 {
    let https = url.starts_with("https://");
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // `[::1]:5173` / `localhost:5173`（IPv6 の `:` を読み違えないよう、最後の `]` の後ろだけ見る）。
    let after_host = authority
        .rsplit_once(']')
        .map_or(authority, |(_, rest)| rest);
    after_host
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(if https { 443 } else { 80 })
}

impl Workspace {
    /// プロジェクト（session）から出てきた URL を開く（R06）。そのプロジェクトが SSH 先で動いていれば、
    /// localhost の URL は SSH 先の物なので手元では開かず、ポート転送の手順を添えて知らせる。
    /// それ以外は [`Self::open_url`]。
    pub(crate) fn open_url_from_session(
        &mut self,
        session_index: usize,
        url: &str,
        cx: &mut Context<Self>,
    ) {
        let remote_host = self
            .project_sessions
            .projects
            .get(session_index)
            .and_then(|slot| slot.remote_host.clone());
        match url_destination(url, remote_host.as_deref()) {
            UrlDestination::RemoteLocalhost { host, port } => {
                let details = i18n::t!(
                    "link.remote_localhost_details",
                    "url" => url,
                    "host" => &host,
                    "port" => port
                );
                self.push_failure_toast(
                    i18n::t!("link.remote_localhost", "url" => url, "host" => &host).into(),
                    Some((i18n::t!("link.remote_localhost_title").into(), details)),
                    cx,
                );
            }
            UrlDestination::WebTab | UrlDestination::Browser => self.open_url(url, cx),
        }
    }

    /// URL を開く（唯一の入口）。localhost 系は Web タブ、それ以外は既定のブラウザ。
    /// プロジェクトから出てきた URL は [`Self::open_url_from_session`] を通す（SSH 先の localhost・R06）。
    ///
    /// Window を取らない形にしてあるので、Window の無いイベント購読（エージェントのパネル・端末）からも
    /// そのまま呼べる。Web タブを開くのは次の effect cycle（`process_pending_shell_effects`）。
    pub(crate) fn open_url(&mut self, url: &str, cx: &mut Context<Self>) {
        if let Some(url) = localhost::normalize(url) {
            self.pending_web_url = Some(url);
            cx.notify();
            return;
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
        let theme = self.theme.clone();
        let view = cx.new(|cx| WebPreviewView::new(&url, theme, cx));
        self.push_web_view(key, view, window, cx);
    }

    /// HTML ファイルを**内蔵の配信**で Web タブに開く（Design Mode が使える）。対話の入口
    /// （HTML プレビューのツールバー・エクスプローラの右クリック・パレット）なので、[`Self::open_web_tab`]
    /// と同じくエディタ領域を前に出す。
    pub(crate) fn open_static_web_tab(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(refusal) = self.static_serving_refusal() {
            let color = self.accent();
            self.push_toast(refusal.into(), color, cx);
            return;
        }
        self.chrome.show_settings = false;
        self.exit_agent_full_screen(cx);
        if self.chrome.fleet_mode {
            self.chrome.fleet_mode = false;
        }
        self.agent_active = false;
        self.show_static_web_tab(file, window, cx);
    }

    /// 内蔵の配信で配らない場面の理由（配れるなら `None`）。
    ///
    /// - WebView の無い環境（Linux）
    /// - Chat: Chat の HTML はエージェントが書いた物で、artifact として閉じ込めて見せる（`sandbox`）。
    ///   配信に載せると外へ通信でき、フォルダの他のファイルも読めてしまう
    /// - SSH のプロジェクト: 配信は手元のファイルだけ（リモートのファイルを手元で配ると、R06 と同じく
    ///   どこの物かが紛れる）。対応は今後
    fn static_serving_refusal(&self) -> Option<String> {
        // WebView の無い環境（Linux）で配信だけ動かしても見る物が無い。
        if !webview_view::is_supported() {
            return Some(i18n::t!("webtab.unsupported"));
        }
        if self.chat_mode() {
            return Some(i18n::t!("webtab.static_chat"));
        }
        self.active_slot()
            .is_some_and(|slot| slot.remote_host.is_some())
            .then(|| i18n::t!("webtab.static_remote"))
    }

    /// 内蔵の配信の Web タブを出す（同じファイルのタブがあればそこへ移る・無ければそのファイルの
    /// フォルダを配って末尾に足す）。モードは触らない（起動時の復元もここを通る）。
    pub(crate) fn show_static_web_tab(
        &mut self,
        file: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(refusal) = self.static_serving_refusal() {
            let color = self.accent();
            self.push_toast(refusal.into(), color, cx);
            return;
        }
        let Some(key) = static_tab_key(&file) else {
            return;
        };
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == key) {
            self.select_tab(index, window, cx);
            return;
        }
        // 無い（消された）ファイルは開かない（配信も起こさない）。
        let folder = file.parent().filter(|_| file.is_file());
        let Some(folder) = folder else {
            let color = self.accent();
            self.push_toast(
                i18n::t!("link.open_failed", "target" => file.display()).into(),
                color,
                cx,
            );
            return;
        };
        let mount = match cx.default_global::<StaticHosting>().mount(folder) {
            Ok(mount) => mount,
            Err(error) => {
                eprintln!("内蔵の配信を始められない: {error}");
                let color = self.accent();
                self.push_toast(
                    i18n::t!("webtab.static_failed", "error" => error).into(),
                    color,
                    cx,
                );
                return;
            }
        };
        // 確かめた後に消えた等。口はここで drop される（他に使う物が無ければ配信も止まる）。
        let Some(url) = mount.url_for(&file) else {
            let color = self.accent();
            self.push_toast(
                i18n::t!("link.open_failed", "target" => file.display()).into(),
                color,
                cx,
            );
            return;
        };
        let theme = self.theme.clone();
        let view = cx.new(|cx| WebPreviewView::new_static(file, mount, &url, theme, cx));
        self.push_web_view(key, view, window, cx);
    }

    /// パレット「プレビュー: HTML を Web タブで開く」。アクティブなタブの HTML ファイルを開く。
    pub(crate) fn open_html_in_web_tab(
        &mut self,
        _: &OpenHtmlInWebTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = self.active_editor().and_then(|editor| {
            let path = editor.read(cx).buffer().path()?;
            (lang::language_for_path(path) == Some(lang::LanguageId::Html))
                .then(|| path.to_path_buf())
        });
        match file {
            Some(file) => self.open_static_web_tab(file, window, cx),
            None => {
                let color = self.accent();
                self.push_toast(i18n::t!("webtab.static_needs_html").into(), color, cx);
            }
        }
    }

    /// 内蔵の配信で配っているフォルダの中が変わった（既存のファイル監視・合流済み）: その Web タブを
    /// 読み込み直す。ページに live reload のスクリプトは差し込まない（Rust の側から reload する）。
    /// `.` で始まる名前（`.git` など）は配らないので見ない。
    pub(crate) fn reload_static_web_tabs(
        &mut self,
        session_index: usize,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.project_sessions.sessions.get(session_index) else {
            return;
        };
        // 配っているフォルダの中で、`.` で始まる名前を通らないパス（フォルダ自体が隠しフォルダの
        // 下にあってもよい＝比べるのはフォルダからの相対）。
        let served = |path: &Path, root: &Path| {
            path.strip_prefix(root).is_ok_and(|relative| {
                !relative
                    .components()
                    .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
            })
        };
        let stale: Vec<Entity<WebPreviewView>> = session
            .tabs
            .iter()
            .filter_map(|tab| tab.web())
            .filter(|view| {
                let roots = view.read(cx).static_roots();
                paths
                    .iter()
                    .any(|path| roots.iter().any(|root| served(path, root)))
            })
            .cloned()
            .collect();
        for view in stale {
            view.update(cx, |view, cx| view.reload(cx));
        }
    }

    /// Web タブの view をタブ列の末尾に足してアクティブにする（localhost / 内蔵の配信で共通）。
    fn push_web_view(
        &mut self,
        key: PathBuf,
        view: Entity<WebPreviewView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_buffer_search(cx);
        self.close_hover(cx);
        let evict_minutes = settings::get(cx).html_preview_evict_minutes;
        view.update(cx, |view, cx| view.set_evict_minutes(evict_minutes, cx));
        // タイトル・読み込み状態の変化でタブ名を描き直す。
        let observation = cx.observe(&view, |_, _, cx| cx.notify());
        let events = cx.subscribe_in(&view, window, Self::on_web_preview_event);
        self.tabs.push(EditorTab {
            path: key,
            content: TabContent::Web {
                view: view.clone(),
                _observation: observation,
                _events: events,
            },
            transient: false,
            pinned: false,
            preview: false,
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

    /// Design Mode の開始 / 終了（⌘⇧D・パレット）。アクティブなタブが Web タブでなければ案内を出す。
    pub(crate) fn toggle_design_mode(
        &mut self,
        _: &ToggleDesignMode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.web().cloned())
        else {
            let color = self.accent();
            self.push_toast(i18n::t!("design.needs_web_tab").into(), color, cx);
            return;
        };
        let toggled = view.update(cx, |view, cx| view.toggle_design(cx));
        if !toggled {
            let color = self.accent();
            self.push_toast(i18n::t!("design.not_ready").into(), color, cx);
        }
    }

    /// Design Mode の知らせ。要素が選ばれた瞬間（`PickStarted`）に、その Web タブが居る session の
    /// アクティブなスレッドを宛先として控え、撮り終わったら（`ElementPicked`）**控えた宛先**の composer へ
    /// 切り抜き（画像の添付・チップ名は要素の呼び名）と説明の文章を足す。**送信はしない**。
    /// 選び終えたら composer へフォーカスを移す（続けて指示を打てる）。
    ///
    /// 撮っている間にスレッドを切り替えていたら、今のスレッドへは添えない（R05・別の会話へ入れない）。
    pub(crate) fn on_web_preview_event(
        &mut self,
        view: &Entity<WebPreviewView>,
        event: &WebPreviewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (capture, png, finished, target) = match event {
            WebPreviewEvent::PickStarted { serial } => {
                let Some(panel) = self.panel_for_web_view(view) else {
                    return;
                };
                let thread_id = {
                    let panel = panel.read(cx);
                    panel.thread_id(panel.active_thread())
                };
                if let Some(thread_id) = thread_id {
                    let target = PickTarget {
                        panel: panel.downgrade(),
                        thread_id,
                    };
                    let serial = *serial;
                    view.update(cx, |view, _| view.remember_pick_target(serial, target));
                }
                return;
            }
            WebPreviewEvent::PickDropped => return,
            WebPreviewEvent::ElementPicked {
                capture,
                png,
                finished,
                target,
            } => (capture, png, finished, target),
        };
        let Some(target) = target else {
            return;
        };
        let Some(panel) = target.panel.upgrade() else {
            return;
        };
        let (still_active, thread_name) = {
            let panel = panel.read(cx);
            let active = panel.thread_id(panel.active_thread());
            let name = panel
                .thread_position(&target.thread_id)
                .and_then(|index| panel.statuses().into_iter().nth(index))
                .map(|status| status.name);
            (active.as_ref() == Some(&target.thread_id), name)
        };
        if !still_active {
            let color = self.accent();
            let message = match thread_name {
                Some(name) => i18n::t!("design.target_moved", "thread" => name),
                None => i18n::t!("design.target_gone"),
            };
            self.push_toast(message.into(), color, cx);
            return;
        }
        let text = webview_view::design::format_for_prompt(capture);
        let chip = SharedString::from(format!(
            "◎ {}",
            truncate_chip(&webview_view::design::element_label(capture))
        ));
        let attached = panel.update(cx, |panel, cx| {
            let attached = png.as_ref().is_some_and(|png| {
                panel.attach_labeled_image(
                    &editor_view::PastedImage {
                        format: gpui::ImageFormat::Png,
                        bytes: png.clone(),
                    },
                    chip,
                    cx,
                )
            });
            panel.append_to_composer(&text, cx);
            attached
        });
        if !attached {
            let color = self.accent();
            self.push_toast(i18n::t!("design.capture_failed").into(), color, cx);
        }
        if !self.chat_mode() {
            self.chrome.show_right = true;
        }
        if *finished {
            self.agent_active = true;
            panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
        }
        cx.notify();
    }

    /// Web タブが居る session（プロジェクト / Chat）の Agent パネル。
    fn panel_for_web_view(&self, view: &Entity<WebPreviewView>) -> Option<Entity<AgentPanel>> {
        self.project_sessions
            .sessions
            .iter()
            .chain(self.project_sessions.chat.iter())
            .find(|session| session.tabs.iter().any(|tab| tab.web() == Some(view)))
            .map(|session| session.agent_panel.clone())
    }

    /// パレット「プレビュー: localhost を開く…」。入力欄にポート番号か localhost の URL を打つ
    /// （それ以外は通さない＝任意の URL を開く欄にはしない）。
    pub(crate) fn open_localhost_preview(
        &mut self,
        _: &OpenLocalhostPreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_picker(
            PickerMode::PreviewUrl,
            i18n::t!("webtab.picker_placeholder"),
            Vec::new(),
            window,
            cx,
        );
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
        if destination.starts_with("http://") || destination.starts_with("https://") {
            // プレビューしている文書のプロジェクト（SSH 先なら localhost は SSH 先の物・R06）。
            let session_index = self.project_sessions.active;
            self.open_url_from_session(session_index, destination, cx);
            return;
        }
        let (base, local) = {
            let view = editor.read(cx);
            (
                view.buffer()
                    .path()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf),
                !view.buffer().host().is_remote(),
            )
        };
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
    /// `open:<url>` = open_url と同じ入口 / `open-html:<path>` = HTML ファイルを内蔵の配信で
    /// Web タブに開く（ツールバー・右クリックと同じ入口）/ `open-file:<path>` / `html-preview` /
    /// `explorer-menu:<path>` / `close-tab` / `palette:<入力>` = パレットの確定と同じ入口 /
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
            "open" => self.open_url(argument, cx),
            "open-html" => self.open_static_web_tab(PathBuf::from(argument), window, cx),
            // HTML の入口の見た目の検証: `open-file:<path>` = エディタで開く / `html-preview` = アクティブな
            // .html を ⌘⇧V のプレビューにする / `explorer-menu:<path>` = そのファイルの右クリックメニュー。
            "open-file" => self.open_file(PathBuf::from(argument), window, cx),
            // `close-tab` = アクティブなタブを閉じる（内蔵の配信が止まるかを見る）。
            "close-tab" => {
                let index = self.active_tab;
                self.close_tab_at(index, window, cx);
            }
            "html-preview" => self.debug_html_preview(cx),
            "explorer-menu" => self.show_context_menu(
                PathBuf::from(argument),
                false,
                point(px(150.), px(120.)),
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
            // Design Mode（`design` = 開始 / 終了・`design-hover:<css>` / `design-pick:<css>` /
            // `design-pick-multi:<css>` = ページの debug 口で実際のクリックと同じ道を通す・
            // `snapshot-view:<path>` = 見えている範囲をまるごと撮る）。
            "design" => self.toggle_design_mode(&ToggleDesignMode, window, cx),
            "design-hover" | "design-pick" | "design-pick-multi" => {
                if let Some(web) = active_web {
                    let action = name.trim_start_matches("design-");
                    web.read(cx).debug_design(action, argument, cx);
                }
            }
            "snapshot-view" => {
                if let Some(web) = active_web {
                    web.read(cx)
                        .debug_snapshot_view(PathBuf::from(argument), cx);
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
                        web.is_designing(),
                        web.static_file().map(Path::to_path_buf),
                    )
                });
                eprintln!(
                    "WEB_PREVIEW_PROBE state: tabs={tabs:?} active={} web={web:?} fleet={} open_files={:?} static_server={}",
                    self.active_tab,
                    self.chrome.fleet_mode,
                    self.active_slot().map(|slot| slot.open_files.clone()),
                    cx.try_global::<StaticHosting>()
                        .is_some_and(StaticHosting::is_running)
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
                pinned: Vec::new(),
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

    /// Fixture のプロジェクトに、内蔵の配信で開く HTML を置く（`index.html` + `style.css`）。
    fn write_site(project: &Path) -> PathBuf {
        let site = project.join("public");
        std::fs::create_dir_all(&site).expect("フォルダを作れる");
        std::fs::write(
            site.join("index.html"),
            "<!doctype html><link rel=stylesheet href=style.css><h1>static</h1>",
        )
        .expect("書ける");
        std::fs::write(site.join("style.css"), "h1 { color: red }").expect("書ける");
        site.join("index.html")
    }

    fn hosting_is_running(cx: &mut gpui::VisualTestContext) -> bool {
        cx.update(|_window, cx| {
            cx.try_global::<StaticHosting>()
                .is_some_and(StaticHosting::is_running)
        })
    }

    /// 生の HTTP/1.1 で GET して本文を返す（配信が本当に配っているかを見る）。
    fn http_get(url: &str) -> String {
        use std::io::{Read as _, Write as _};
        let rest = url.strip_prefix("http://").expect("http の URL");
        let (host, path) = rest.split_once('/').expect("パスがある");
        let mut stream = std::net::TcpStream::connect(host).expect("繋がる");
        write!(stream, "GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").expect("送れる");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("読める");
        response
    }

    /// HTML ファイルは内蔵の配信で Web タブに開く: 鍵はファイルの `file://`、中身は 127.0.0.1 の
    /// token 付きの URL から配られ、同じファイルは同じタブへ戻る。タブを閉じたら配信も止まる。
    #[gpui::test]
    fn html_files_open_in_a_web_tab_through_the_built_in_server(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("static_open", cx);
        let file = write_site(&fixture.project);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        assert!(!hosting_is_running(cx), "使うまで配信は起きない");
        let (view, url) = workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.open_static_web_tab(file.clone(), window, cx);
            workspace.open_static_web_tab(file.clone(), window, cx);
            assert_eq!(workspace.tabs.len(), 1, "同じファイルは同じタブへ戻る");
            let key = static_tab_key(&file).expect("鍵");
            assert_eq!(workspace.tabs[0].path, key);
            assert!(key.to_string_lossy().starts_with("file://"), "{key:?}");
            assert_eq!(
                workspace.persisted_state().projects[0].open_files,
                vec![key],
                "保存するのはファイルの鍵（ポートと token は起動ごとに変わる）"
            );
            let view = workspace.tabs[0].web().cloned().expect("Web タブ");
            let url = view.read(cx).url().to_string();
            assert_eq!(view.read(cx).static_file(), Some(file.as_path()));
            (view, url)
        });
        assert!(hosting_is_running(cx));
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert!(url.ends_with("/index.html"), "{url}");
        assert!(
            webview_view::localhost::is_allowed(&url),
            "Web タブの範囲の URL"
        );
        let response = http_get(&url);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("<h1>static</h1>"), "{response}");
        assert_eq!(
            view.read_with(cx, |view, cx| view.tab_label(cx)),
            "public/index.html",
            "タイトルが出るまでは token の無い短い表記"
        );
        drop(view);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.close_tab_at(0, window, cx);
        });
        cx.run_until_parked();
        assert!(
            !hosting_is_running(cx),
            "使う Web タブが無くなったら配信は止まる"
        );
        workspace.update_in(cx, |workspace, _window, _cx| stop_watchers(workspace));
    }

    /// 内蔵の配信のページで選んだ要素は、token 付きの URL ではなくファイルのパスで composer に入る
    /// （起動ごとに変わる URL は AI には意味が無い・直すのはそのファイル）。
    #[gpui::test]
    fn a_static_page_is_named_by_its_file_in_the_prompt(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("static_prompt", cx);
        let file = write_site(&fixture.project);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        let panel = workspace.update(cx, |workspace, _cx| workspace.agent_panel.clone());
        let (view, url) = workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.open_static_web_tab(file.clone(), window, cx);
            let view = workspace.tabs[0].web().cloned().expect("Web タブ");
            let url = view.read(cx).url().to_string();
            (view, url)
        });
        let mut capture = sample_capture("h1", "static");
        capture.page.url = url.clone();
        // 選んだ瞬間（宛先を控える）→ 撮り終わった（テスト窓は切り抜きを撮れない）。
        let start = view.update(cx, |view, cx| {
            view.force_design_for_test();
            view.begin_pick(cx)
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.finish_pick(
                start,
                Box::new(capture),
                false,
                Err("テスト窓は撮れない".into()),
                cx,
            )
        });
        cx.run_until_parked();
        let text = panel.read_with(cx, |panel, cx| panel.composer_text(cx));
        assert!(text.contains(&file.display().to_string()), "{text}");
        assert!(!text.contains(&url), "token 付きの URL は入れない: {text}");
        workspace.update_in(cx, |workspace, _window, _cx| stop_watchers(workspace));
    }

    /// 内蔵の配信の Web タブも再起動で同じ位置に戻る（鍵のファイルから配り直す）。消えたファイルは戻さない。
    #[gpui::test]
    fn static_web_tabs_come_back_after_a_restart(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("static_restore", cx);
        let file = write_site(&fixture.project);
        let file_a = fixture.project.join("a.txt");
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            let key = static_tab_key(&file).expect("鍵");
            let gone = static_tab_key(&fixture.project.join("gone.html")).expect("鍵");
            let restored = RestoredTabs {
                files: vec![file_a.clone(), key.clone(), gone],
                active: 1,
                pinned: Vec::new(),
            };
            workspace.restore_open_file(&[restored], window, cx);
            let paths: Vec<PathBuf> = workspace.tabs.iter().map(|tab| tab.path.clone()).collect();
            assert_eq!(paths, vec![file_a.clone(), key]);
            let view = workspace.tabs[1].web().cloned().expect("Web タブで戻る");
            assert_eq!(view.read(cx).static_file(), Some(file.as_path()));
            assert_eq!(workspace.active_tab, 1);
            stop_watchers(workspace);
        });
        assert!(hosting_is_running(cx));
    }

    /// 配っているフォルダの中のファイルが変わったら（既存のファイル監視）、その Web タブを読み込み直す。
    /// フォルダの外・`.git` の変化では読み込み直さない。
    #[gpui::test]
    fn changes_in_the_served_folder_reload_the_web_tab(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("static_watch", cx);
        let file = write_site(&fixture.project);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        let view = workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.open_static_web_tab(file.clone(), window, cx);
            workspace.tabs[0].web().cloned().expect("Web タブ")
        });
        let site = file.parent().expect("フォルダ").to_path_buf();
        workspace.update(cx, |workspace, cx| {
            workspace.handle_watch_events(
                0,
                vec![
                    fixture.project.join("a.txt"),
                    fixture.project.join(".git/index"),
                    site.join(".cache/state.json"),
                ],
                cx,
            );
        });
        assert_eq!(view.read_with(cx, |view, _cx| view.reload_requests()), 0);
        workspace.update(cx, |workspace, cx| {
            workspace.handle_watch_events(
                0,
                vec![site.join("style.css"), site.join("index.html")],
                cx,
            );
        });
        assert_eq!(
            view.read_with(cx, |view, _cx| view.reload_requests()),
            1,
            "合流したイベントで 1 回だけ読み込み直す"
        );
        workspace.update_in(cx, |workspace, _window, _cx| stop_watchers(workspace));
    }

    /// SSH のプロジェクトの HTML は配らない（手元のファイルだけ）。配信も起こさない。
    #[gpui::test]
    fn html_in_an_ssh_project_is_not_served(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("static_ssh", cx);
        let file = write_site(&fixture.project);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            // SSH のプロジェクトを装う（R06 の `open_url_from_session` と同じく、見るのは slot の SSH 先）。
            workspace.project_sessions.projects[0].remote_host = Some("dev-box".into());
            workspace.open_static_web_tab(file, window, cx);
            assert!(
                workspace.tabs.iter().all(|tab| tab.web().is_none()),
                "Web タブは開かない"
            );
            assert!(
                workspace
                    .notifications
                    .toasts
                    .iter()
                    .any(|toast| toast.text.as_ref() == i18n::t!("webtab.static_remote")),
                "配らない理由を知らせる"
            );
            stop_watchers(workspace);
        });
        assert!(!hosting_is_running(cx));
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
            workspace.open_url("http://127.0.0.1:3000", cx);
            workspace.process_pending_shell_effects(window, cx);
            workspace.open_url("http://127.0.0.1:3000/", cx);
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

    /// Design Mode で選んだ要素は、その Web タブが居るプロジェクトのアクティブなスレッドの composer に
    /// 足される（送らない）。切り抜きが無い時はテキストだけ足して、撮れなかったと知らせる。
    #[gpui::test]
    fn picked_elements_land_in_the_composer_of_the_tab_session(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("design", cx);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        let capture: webview_view::design::ElementCapture = serde_json::from_value(serde_json::json!({
            "page": {"url": "http://localhost:5173/", "title": "App", "viewport_width": 800.0,
                     "viewport_height": 600.0, "device_pixel_ratio": 2.0},
            "element": {"tag": "button", "selector": "button#start", "path": "main > button#start",
                        "text": "Start", "nearby_text": [], "html": "<button id=\"start\">Start</button>",
                        "role": null, "accessible_name": null, "attributes": [],
                        "rect": {"x": 10.0, "y": 20.0, "width": 80.0, "height": 30.0},
                        "styles": {}, "components": [], "source": null}
        }))
        .expect("形どおり");
        let view = workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.chrome.show_right = false;
            workspace.open_url("http://localhost:5173/", cx);
            workspace.process_pending_shell_effects(window, cx);
            workspace.tabs[0].web().cloned().expect("Web タブ")
        });
        // 選んだ瞬間（宛先を控える）→ 撮り終わった（切り抜きは撮れなかった）。
        let start = view.update(cx, |view, cx| {
            view.force_design_for_test();
            view.begin_pick(cx)
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.finish_pick(
                start,
                Box::new(capture),
                false,
                Err("テスト窓は撮れない".into()),
                cx,
            )
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            let text = workspace.agent_panel.read(cx).composer_text(cx);
            assert!(text.contains("button#start \"Start\""), "{text}");
            assert!(text.contains("```html"), "{text}");
            assert!(
                workspace.chrome.show_right,
                "composer が見えるよう右のドックを出す"
            );
            assert!(
                workspace
                    .notifications
                    .toasts
                    .iter()
                    .any(|toast| toast.text.as_ref() == i18n::t!("design.capture_failed")),
                "切り抜きが無いことを知らせる"
            );
            stop_watchers(workspace);
        });
    }

    /// R06: SSH 先のプロジェクトから来た localhost の URL は手元で開かない（手元の同じポートは別物）。
    #[test]
    fn remote_localhost_urls_are_not_opened_locally() {
        assert_eq!(
            url_destination("http://localhost:5173/app", None),
            UrlDestination::WebTab
        );
        assert_eq!(
            url_destination("http://localhost:5173/app", Some("dev-box")),
            UrlDestination::RemoteLocalhost {
                host: "dev-box".into(),
                port: 5173
            }
        );
        assert_eq!(
            url_destination("https://[::1]/", Some("dev-box")),
            UrlDestination::RemoteLocalhost {
                host: "dev-box".into(),
                port: 443
            }
        );
        assert_eq!(
            url_destination("http://127.0.0.1:8080", Some("dev-box")),
            UrlDestination::RemoteLocalhost {
                host: "dev-box".into(),
                port: 8080
            }
        );
        assert_eq!(
            url_destination("https://example.com/", Some("dev-box")),
            UrlDestination::Browser,
            "localhost でない URL は手元のブラウザ"
        );
    }

    fn sample_capture(selector: &str, text: &str) -> webview_view::design::ElementCapture {
        serde_json::from_value(serde_json::json!({
            "page": {"url": "http://localhost:5173/", "title": "App", "viewport_width": 800.0,
                     "viewport_height": 600.0, "device_pixel_ratio": 2.0},
            "element": {"tag": "button", "selector": selector, "path": selector,
                        "text": text, "nearby_text": [], "html": "<button>x</button>",
                        "role": null, "accessible_name": null, "attributes": [],
                        "rect": {"x": 10.0, "y": 20.0, "width": 80.0, "height": 30.0},
                        "styles": {}, "components": [], "source": null}
        }))
        .expect("形どおり")
    }

    /// R05: 宛先は選んだ瞬間に決まる。撮っている間にスレッドを切り替えたら今のスレッドへは添えない。
    /// 読み直した後の古い結果は捨て、Design を始め直していたら新しい Design を止めない。
    /// 複数選んだ時は、撮り終わる順が逆でも選んだ順に添える。
    #[gpui::test]
    fn picked_elements_go_where_they_were_picked_and_in_order(cx: &mut gpui::TestAppContext) {
        i18n::set_locale("ja");
        let fixture = Fixture::new("design_target", cx);
        // エージェントを先張りしない（2 本目のスレッドを作っても子プロセスを立てない）。
        let settings_path = fixture.root.join("settings_no_prewarm.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .expect("書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        let view = workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.open_url("http://localhost:5173/", cx);
            workspace.process_pending_shell_effects(window, cx);
            workspace.tabs[0].web().cloned().expect("Web タブ")
        });
        let panel = workspace.read_with(cx, |workspace, _| workspace.agent_panel.clone());
        let composer = |cx: &mut gpui::VisualTestContext| {
            panel.read_with(cx, |panel, cx| panel.composer_text(cx))
        };

        // ① 選んだ後でスレッドを切り替える → 添えない（トーストで知らせる）。
        let start = view.update(cx, |view, cx| {
            view.force_design_for_test();
            view.begin_pick(cx)
        });
        cx.run_until_parked();
        panel.update(cx, |panel, cx| {
            panel.new_thread_index(cx);
        });
        view.update(cx, |view, cx| {
            view.finish_pick(
                start,
                Box::new(sample_capture("button#moved", "Moved")),
                false,
                Err("撮れない".into()),
                cx,
            )
        });
        cx.run_until_parked();
        assert!(
            !composer(cx).contains("button#moved"),
            "別のスレッドへは添えない"
        );
        let toasts: Vec<String> = workspace.read_with(cx, |workspace, _| {
            workspace
                .notifications
                .toasts
                .iter()
                .map(|toast| toast.text.to_string())
                .collect()
        });
        assert!(
            toasts
                .iter()
                .any(|toast| toast.contains("添えませんでした")),
            "{toasts:?}"
        );

        // ② 読み直した後の古い結果は捨てる。
        let start = view.update(cx, |view, cx| {
            view.force_design_for_test();
            view.begin_pick(cx)
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.begin_navigation_for_test(cx);
            view.finish_pick(
                start,
                Box::new(sample_capture("button#stale", "Stale")),
                false,
                Err("撮れない".into()),
                cx,
            )
        });
        cx.run_until_parked();
        assert!(!composer(cx).contains("button#stale"), "古いページの要素");

        // ③ Design を始め直していたら、古い撮影結果で新しい Design を止めない。
        let start = view.update(cx, |view, cx| {
            view.force_design_for_test();
            view.begin_pick(cx)
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.force_design_for_test(); // 始め直した
            view.finish_pick(
                start,
                Box::new(sample_capture("button#old", "Old")),
                false,
                Err("撮れない".into()),
                cx,
            );
            assert!(view.is_designing(), "新しい Design は続く");
        });
        cx.run_until_parked();

        // ④ 2 つ選んで、後の方が先に撮り終わっても、選んだ順に添える。
        let (first, second) = view.update(cx, |view, cx| {
            view.force_design_for_test();
            (view.begin_pick(cx), view.begin_pick(cx))
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.finish_pick(
                second,
                Box::new(sample_capture("button#second", "Second")),
                true,
                Err("撮れない".into()),
                cx,
            );
            view.finish_pick(
                first,
                Box::new(sample_capture("button#first", "First")),
                true,
                Err("撮れない".into()),
                cx,
            );
        });
        cx.run_until_parked();
        let text = composer(cx);
        let first_at = text.find("button#first").expect("1 つ目が添えられる");
        let second_at = text.find("button#second").expect("2 つ目が添えられる");
        assert!(first_at < second_at, "選んだ順: {text}");
        workspace.update_in(cx, |workspace, _window, _cx| stop_watchers(workspace));
    }

    /// Design 中に GPUI 側へキーが来ている時も Esc で Design を抜ける。Web タブが無い時の ⌘⇧D は案内だけ。
    #[gpui::test]
    fn escape_leaves_design_and_design_needs_a_web_tab(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("escape", cx);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.toggle_design_mode(&ToggleDesignMode, window, cx);
            assert!(
                workspace
                    .notifications
                    .toasts
                    .iter()
                    .any(|toast| toast.text.as_ref() == i18n::t!("design.needs_web_tab")),
                "Web タブが無ければ案内する"
            );
        });
        let view = workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_url("http://localhost:5173/", cx);
            workspace.process_pending_shell_effects(window, cx);
            let view = workspace.tabs[0].web().cloned().expect("Web タブ");
            view.update(cx, |view, _cx| view.force_design_for_test());
            let handle = view.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            view
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _cx| view.is_designing()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(
            !view.read_with(cx, |view, _cx| view.is_designing()),
            "Esc で Design を抜ける"
        );
        workspace.update_in(cx, |workspace, _window, _cx| stop_watchers(workspace));
    }

    /// ツールバーの `Design` は ⌘⇧D と同じ入口（action）を通る。WebView がまだ無い（テスト窓には
    /// ネイティブの WebView が作られない）時は始めずに案内する。
    #[gpui::test]
    fn the_design_chip_goes_through_the_same_action(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new("chip", cx);
        let project = fixture.project.clone();
        let (workspace, cx) = cx
            .add_window_view(|_window, cx| Workspace::new(vec![project], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            stop_watchers(workspace);
            workspace.open_url("http://localhost:5173/", cx);
            workspace.process_pending_shell_effects(window, cx);
        });
        cx.run_until_parked();
        let chip = cx
            .debug_bounds("web-design")
            .expect("Web タブのツールバーに Design が描かれている");
        cx.simulate_mouse_down(chip.center(), MouseButton::Left, gpui::Modifiers::none());
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(
                workspace
                    .notifications
                    .toasts
                    .iter()
                    .any(|toast| toast.text.as_ref() == i18n::t!("design.not_ready")),
                "WebView が無ければ始めずに案内する"
            );
            stop_watchers(workspace);
        });
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
