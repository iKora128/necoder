//! web_preview_view — localhost の開発サーバを見る **Web タブ**（`TabContent::Web`・Pane/Item の第 4 の具体型）。
//!
//! 中身は `webview_view` の Web タブ（[`WebViewView::localhost`]）で、読み込める範囲の線引き
//! （ループバックの http(s) だけ・外への移動と新しい窓は既定のブラウザへ）はそちらが持つ。
//! この器が持つのは、エディタのパンくずの位置に置くツールバー（戻る / 進む / 再読込 / URL /
//! ズーム / ビューポート幅 / DevTools / 既定のブラウザで開く）と、ビューポート幅を絞った時に
//! WebView の矩形を中央へ寄せることだけ。汎用ブラウザにはしない — 任意の URL を打つ欄は置かない
//! （URL は表示だけ。開く入口はパレット「プレビュー: localhost を開く…」と、エージェントの
//! transcript・Markdown プレビューのリンク = `Workspace::open_url`）。
//!
//! 色は識別に集約（UI-SPEC §1.3）: ツールバーは bg1・fg2/fg0 と中立の選択面 bg3 だけで、識別色は使わない。

use crate::workspace::*;
use webview_view::{localhost, WebViewEvent, WebViewView};

/// Web タブの鍵（`EditorTab.path`）。タブ列は path で同一判定・永続化するので、URL（正規形）を
/// そのまま入れる。ファイルの鍵は絶対パス（`/…` / `C:\…`）なので `http://` で始まる鍵と衝突しない。
/// 保存形式（窓セッションの `open_files`）は変えない — 古い版は URL の行を「無いファイル」として飛ばす。
pub(crate) fn web_tab_key(url: &str) -> PathBuf {
    PathBuf::from(url)
}

/// 鍵が Web タブの物ならその URL（許可範囲の正規形）。ファイルの鍵・範囲外の URL は `None`。
pub(crate) fn web_tab_url(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if !(text.starts_with("http://") || text.starts_with("https://")) {
        return None;
    }
    localhost::normalize(text)
}

/// ビューポート幅（WebView の矩形の幅を絞って中央に置く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Viewport {
    /// ペインの幅いっぱい。
    Full,
    /// 指定の幅（CSS px）。ペインより狭い時だけ効く。
    Width(u32),
}

/// ツールバーに並べるビューポート幅（スマホ / タブレット / ノート PC の代表値）。
pub(crate) const VIEWPORT_WIDTHS: [u32; 3] = [375, 768, 1280];

/// タブ名に出す文字数の上限（ページのタイトルは長いことがある）。
const TAB_LABEL_MAX_CHARS: usize = 32;

fn truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_string();
    }
    let mut truncated: String = label.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// ズームの段（ブラウザの ⌘+ / ⌘- と同じ刻み）。
const ZOOM_STEPS: [f64; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// 今のズームから `direction`（+1 / -1）へ 1 段動かした値。端では止まる。
fn next_zoom(current: f64, direction: i32) -> f64 {
    if direction > 0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current + 0.001)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current - 0.001)
            .unwrap_or(ZOOM_STEPS[0])
    }
}

/// Web タブ。OS の WebView を載せた [`WebViewView`] を 1 枚抱え、上にツールバーを置く。
/// 編集・保存・LSP・hot exit は関与しない（画像 / PDF タブと同じ表示専用タブ）。
pub(crate) struct WebPreviewView {
    /// タブの鍵にした URL（開いた時の正規形）。ページの中で移動しても変えない＝同じ URL を開き直すと
    /// このタブへ戻る。いま見ている URL は [`Self::current_url`]。
    url: String,
    /// OS の WebView。載せられない環境（Linux）では None でフォールバック表示。
    viewer: Option<Entity<WebViewView>>,
    viewport: Viewport,
    theme: Theme,
    focus_handle: FocusHandle,
    _viewer_subscriptions: Vec<Subscription>,
}

impl WebPreviewView {
    pub(crate) fn new(url: &str, theme: Theme, cx: &mut Context<Self>) -> Self {
        let viewer = webview_view::is_supported().then(|| {
            let viewer_theme = theme.clone();
            let url = url.to_string();
            cx.new(move |cx| WebViewView::localhost(&url, viewer_theme, cx))
        });
        let mut subscriptions = Vec::new();
        if let Some(viewer) = &viewer {
            // 読み込み・タイトル・失敗でツールバーとタブ名を描き直す。
            subscriptions.push(cx.observe(viewer, |_, _, cx| cx.notify()));
            subscriptions.push(cx.subscribe(viewer, Self::on_viewer_event));
        }
        Self {
            url: url.to_string(),
            viewer,
            viewport: Viewport::Full,
            theme,
            focus_handle: cx.focus_handle(),
            _viewer_subscriptions: subscriptions,
        }
    }

    fn on_viewer_event(
        &mut self,
        _viewer: Entity<WebViewView>,
        event: &WebViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            WebViewEvent::PageLoad { .. } | WebViewEvent::TitleChanged(_) => cx.notify(),
        }
    }

    /// タブの鍵にした URL。
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// いま見ている URL（ページの中の移動に追従）。WebView が無ければ開いた時の URL。
    pub(crate) fn current_url(&self, cx: &App) -> String {
        self.viewer
            .as_ref()
            .and_then(|viewer| viewer.read(cx).current_url().map(str::to_string))
            .unwrap_or_else(|| self.url.clone())
    }

    /// タブ名: 文書のタイトルがあればそれ、無ければ `localhost:5173/app` の短い表記。
    /// タブ列の幅を 1 枚で食わないよう [`TAB_LABEL_MAX_CHARS`] 文字で切る。
    pub(crate) fn tab_label(&self, cx: &App) -> String {
        let label = self
            .viewer
            .as_ref()
            .and_then(|viewer| viewer.read(cx).page_title().map(str::to_string))
            .unwrap_or_else(|| localhost::display_label(&self.current_url(cx)));
        truncate_label(&label, TAB_LABEL_MAX_CHARS)
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_theme(theme));
        }
        cx.notify();
    }

    /// 親レイアウト上の可視性をネイティブ子ビューへ同期する（HTML プレビュー / PDF と同じ規律）。
    pub(crate) fn set_surface_active(&mut self, active: bool, focus: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| viewer.set_active(active, focus, cx));
        }
    }

    /// OS のキーボードフォーカスを WebView に渡す / GPUI へ返す（表示状態は変えない）。
    pub(crate) fn set_key_focus(&mut self, owns: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_key_focus(owns));
        }
    }

    /// 設定 `html_preview_evict_minutes` を中継する（非表示 WebView の回収弁は HTML / PDF と共用）。
    pub(crate) fn set_evict_minutes(&mut self, minutes: u64, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_evict_minutes(minutes));
        }
    }

    pub(crate) fn has_native_viewer(&self) -> bool {
        self.viewer.is_some()
    }

    /// WebView を出してよい状態として同期されているか（オーバーレイ中は false・テストの観測点）。
    #[cfg(test)]
    pub(crate) fn is_surface_active(&self, cx: &App) -> bool {
        self.viewer
            .as_ref()
            .is_some_and(|viewer| viewer.read(cx).is_active())
    }

    /// WebView がキーボードフォーカスを持つ想定か（要求の記録・テストの観測点）。
    #[cfg(test)]
    pub(crate) fn wants_key_focus(&self, cx: &App) -> bool {
        self.viewer
            .as_ref()
            .is_some_and(|viewer| viewer.read(cx).wants_key_focus())
    }

    pub(crate) fn set_viewport(&mut self, viewport: Viewport, cx: &mut Context<Self>) {
        if self.viewport != viewport {
            self.viewport = viewport;
            cx.notify();
        }
    }

    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| {
                viewer.reload();
                cx.notify();
            });
        }
    }

    fn zoom(&self, cx: &App) -> f64 {
        self.viewer
            .as_ref()
            .map_or(1.0, |viewer| viewer.read(cx).zoom())
    }

    /// ズームを 1 段動かす（`direction` = +1 / -1）。0 で 100% に戻す。
    pub(crate) fn step_zoom(&mut self, direction: i32, cx: &mut Context<Self>) {
        let next = if direction == 0 {
            1.0
        } else {
            next_zoom(self.zoom(cx), direction)
        };
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| {
                viewer.set_zoom(next);
                cx.notify();
            });
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.go_back());
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.go_forward());
        }
    }

    fn open_devtools(&self, cx: &App) {
        if let Some(viewer) = &self.viewer {
            viewer.read(cx).open_devtools();
        }
    }

    /// いま見ている画面を既定のブラウザで開く。
    fn open_in_browser(&self, cx: &App) {
        localhost::open_in_browser(&self.current_url(cx));
    }

    /// 開発用: 読み込まれたページの中身をスクリプトで読み出して標準エラーへ出す
    /// （WKWebView の中身は offscreen の画像に写らないので、ページ側はこれで確かめる）。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_evaluate(&self, label: String, script: &str, cx: &App) {
        let Some(viewer) = &self.viewer else {
            eprintln!("WEB_PREVIEW_PROBE {label}: WebView が無い");
            return;
        };
        let ran = viewer
            .read(cx)
            .evaluate_script_with_callback(script, move |result| {
                eprintln!("WEB_PREVIEW_PROBE {label}: {result}");
            });
        if !ran {
            eprintln!("WEB_PREVIEW_PROBE: WebView がまだ作られていない（表示前）");
        }
    }

    /// ツールバーの四角いアイコンボタン（19px・hover で bg2）。
    fn icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        enabled: bool,
        tip: String,
    ) -> Stateful<Div> {
        let theme = &self.theme;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(19.))
            .rounded(px(5.))
            .when(enabled, |button| {
                button.cursor_pointer().hover(|style| style.bg(theme.bg2))
            })
            .child(
                svg()
                    .path(icon)
                    .size(px(12.))
                    .flex_none()
                    .text_color(if enabled {
                        theme.fg1
                    } else {
                        theme.fg2.alpha(0.5)
                    }),
            )
            .tooltip(Tooltip::text(tip, theme.clone()))
    }

    /// 文字のチップ（ビューポート幅・ズーム率）。選択中は中立の面 bg3 + fg0。
    fn text_chip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        selected: bool,
    ) -> Stateful<Div> {
        let theme = &self.theme;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(19.))
            .px(px(6.))
            .rounded(px(5.))
            .text_size(px(10.5))
            .cursor_pointer()
            .text_color(if selected { theme.fg0 } else { theme.fg2 })
            .when(selected, |chip| chip.bg(theme.bg3))
            .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
            .child(label)
    }

    fn separator(&self) -> Div {
        div()
            .flex_none()
            .w(px(1.))
            .h(px(12.))
            .mx(px(4.))
            .bg(self.theme.border)
    }

    /// エディタのパンくずの位置（26px）に置くツールバー。
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let (can_back, can_forward, loading) = self
            .viewer
            .as_ref()
            .map(|viewer| {
                let viewer = viewer.read(cx);
                (
                    viewer.can_go_back(),
                    viewer.can_go_forward(),
                    viewer.is_loading(),
                )
            })
            .unwrap_or((false, false, false));
        let current = self.current_url(cx);
        let zoom = self.zoom(cx);
        let mut viewport_chips = vec![self
            .text_chip(
                "web-viewport-full",
                i18n::t!("webtab.viewport_full"),
                self.viewport == Viewport::Full,
            )
            .tooltip(Tooltip::text(
                i18n::t!("webtab.viewport_full_tip"),
                theme.clone(),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    cx.stop_propagation();
                    this.set_viewport(Viewport::Full, cx);
                }),
            )];
        for width in VIEWPORT_WIDTHS {
            viewport_chips.push(
                self.text_chip(
                    ("web-viewport", width as usize),
                    width.to_string(),
                    self.viewport == Viewport::Width(width),
                )
                .tooltip(Tooltip::text(
                    i18n::t!("webtab.viewport_tip", "width" => width),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        cx.stop_propagation();
                        this.set_viewport(Viewport::Width(width), cx);
                    }),
                ),
            );
        }
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(BREADCRUMB_HEIGHT))
            .px(px(8.))
            .flex_none()
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg2)
            .child(
                self.icon_button(
                    "web-back",
                    "icons/arrow-left.svg",
                    can_back,
                    i18n::t!("webtab.back_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.go_back(cx);
                    }),
                ),
            )
            .child(
                self.icon_button(
                    "web-forward",
                    "icons/arrow-right.svg",
                    can_forward,
                    i18n::t!("webtab.forward_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.go_forward(cx);
                    }),
                ),
            )
            .child(
                self.icon_button(
                    "web-reload",
                    "icons/refresh-cw.svg",
                    true,
                    i18n::t!("webtab.reload_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.reload(cx);
                    }),
                ),
            )
            // URL は表示だけ（任意の URL を打つ欄にはしない）。
            .child(
                div()
                    .id("web-url")
                    .flex_1()
                    .min_w_0()
                    .ml(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.fg1)
                    .child(SharedString::from(if loading {
                        format!("{} …", localhost::display_label(&current))
                    } else {
                        localhost::display_label(&current)
                    }))
                    .tooltip(Tooltip::text(current.clone(), theme.clone())),
            )
            .child(
                self.text_chip("web-zoom-out", "−".to_string(), false)
                    .tooltip(Tooltip::text(
                        i18n::t!("webtab.zoom_out_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.step_zoom(-1, cx);
                        }),
                    ),
            )
            .child(
                self.text_chip(
                    "web-zoom-reset",
                    format!("{}%", (zoom * 100.0).round() as i64),
                    false,
                )
                .tooltip(Tooltip::text(
                    i18n::t!("webtab.zoom_reset_tip"),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.step_zoom(0, cx);
                    }),
                ),
            )
            .child(
                self.text_chip("web-zoom-in", "+".to_string(), false)
                    .tooltip(Tooltip::text(i18n::t!("webtab.zoom_in_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.step_zoom(1, cx);
                        }),
                    ),
            )
            .child(self.separator())
            .children(viewport_chips)
            .child(self.separator())
            .child(
                self.icon_button(
                    "web-devtools",
                    "icons/code-xml.svg",
                    self.viewer.is_some(),
                    i18n::t!("webtab.devtools_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.open_devtools(cx);
                    }),
                ),
            )
            .child(
                self.icon_button(
                    "web-open-browser",
                    "icons/external-link.svg",
                    true,
                    i18n::t!("webtab.open_browser_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.open_in_browser(cx);
                    }),
                ),
            )
    }
}

impl Focusable for WebPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WebPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let body = match self.viewer.clone() {
            // Linux（WebView 非対応）: 理由と代わりの手段を出す（黙って空にしない）。
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("webtab.unsupported")))
                .child(
                    div()
                        .text_size(px(10.))
                        .child(SharedString::from(self.url.clone())),
                )
                .into_any_element(),
            Some(viewer) => {
                let full = self.viewport == Viewport::Full;
                // 幅を絞った時は WebView の矩形を中央に置き、左右は bg0 の余白にする
                // （ネイティブの矩形はこの div の大きさで決まる＝webview_view の canvas）。
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .when(!full, |stage| stage.bg(theme.bg0))
                    .child(
                        div()
                            .h_full()
                            .map(|frame| match self.viewport {
                                Viewport::Full => frame.w_full(),
                                Viewport::Width(width) => frame
                                    .w(px(width as f32))
                                    .max_w_full()
                                    .border_x_1()
                                    .border_color(theme.border),
                            })
                            .child(viewer.cached(StyleRefinement::default().size_full())),
                    )
                    .into_any_element()
            }
        };
        div()
            .key_context("WebPreview")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .child(self.render_toolbar(cx))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_tab_keys_round_trip_and_never_match_files() {
        let key = web_tab_key("http://localhost:5173/");
        assert_eq!(web_tab_url(&key).as_deref(), Some("http://localhost:5173/"));
        assert_eq!(web_tab_url(Path::new("/tmp/project/index.html")), None);
        assert_eq!(web_tab_url(Path::new("C:\\work\\index.html")), None);
        // 保存形式に紛れ込んだ範囲外の URL は Web タブとして開かない。
        assert_eq!(web_tab_url(Path::new("https://example.com/")), None);
    }

    #[test]
    fn long_titles_are_cut_for_the_tab() {
        assert_eq!(truncate_label("Vite App", 32), "Vite App");
        let long = "あ".repeat(40);
        let cut = truncate_label(&long, 32);
        assert_eq!(cut.chars().count(), 32);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn zoom_steps_like_a_browser_and_stop_at_the_ends() {
        assert_eq!(next_zoom(1.0, 1), 1.1);
        assert_eq!(next_zoom(1.0, -1), 0.9);
        assert_eq!(next_zoom(2.0, 1), 2.0);
        assert_eq!(next_zoom(0.5, -1), 0.5);
        // 段の間の値からは近い段へ寄る。
        assert_eq!(next_zoom(1.05, 1), 1.1);
        assert_eq!(next_zoom(1.05, -1), 1.0);
    }
}
