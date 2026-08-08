use crate::workspace::*;

impl Workspace {
    pub(crate) fn render_hot_exit_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let pending = self.hot_exit_pending.as_ref()?;
        let theme = self.theme.clone();
        let count = pending.len();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .h(px(20.))
                .px(px(8.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .border_1()
                .border_color(theme.warn.alpha(0.5))
                .text_size(px(11.))
                .text_color(theme.fg0)
                .cursor_pointer()
                .hover(|style| style.bg(theme.warn.alpha(0.18)))
                .child(SharedString::from(label))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(12.))
                .flex_none()
                .bg(theme.warn.alpha(0.12))
                .border_b_1()
                .border_color(theme.warn.alpha(0.4))
                .text_size(px(11.5))
                .text_color(theme.fg0)
                .child(SharedString::from(format!(
                    "⚠ {}（{count}）",
                    i18n::t!("hotexit.pending")
                )))
                .child(div().flex_1())
                .child(
                    button("hotexit-restore", i18n::t!("hotexit.restore")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.restore_hot_exit(window, cx)),
                    ),
                )
                .child(
                    button("hotexit-discard", i18n::t!("hotexit.discard")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.discard_hot_exit(cx)),
                    ),
                )
                .into_any_element(),
        )
    }

    /// 外部変更の警告バー（dirty バッファにディスク変更が来たとき・M10 watch）。
    /// 上書きは絶対にせず、ユーザーに「再読込 / このまま」を選ばせる。
    pub(crate) fn render_external_change_bar(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let editor = self.active_editor()?;
        if !editor.read(cx).is_externally_changed() {
            return None;
        }
        let theme = self.theme.clone();
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .h(px(20.))
                .px(px(8.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .border_1()
                .border_color(theme.warn.alpha(0.5))
                .text_size(px(11.))
                .text_color(theme.fg0)
                .cursor_pointer()
                .hover(|style| style.bg(theme.warn.alpha(0.18)))
                .child(SharedString::from(label))
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(12.))
                .flex_none()
                .bg(theme.warn.alpha(0.12))
                .border_b_1()
                .border_color(theme.warn.alpha(0.4))
                .text_size(px(11.5))
                .text_color(theme.fg0)
                .child(SharedString::from(format!(
                    "⚠ {}",
                    i18n::t!("watch.external_changed")
                )))
                .child(div().flex_1())
                .child(
                    button("external-reload", i18n::t!("watch.reload")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            if let Some(editor) = this.active_editor() {
                                editor.update(cx, |view, cx| view.reload_from_disk(cx));
                            }
                        }),
                    ),
                )
                .child(
                    button("external-keep", i18n::t!("watch.keep")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            if let Some(editor) = this.active_editor() {
                                editor.update(cx, |view, cx| view.dismiss_external_change(cx));
                            }
                        }),
                    ),
                )
                .into_any_element(),
        )
    }

    /// hover ポップアップ（LSP hover 結果・M10）。アンカーの上（入らなければ下）にコード字で出す。
    /// フォーカスは取らない。occlude なのでポップアップ上にマウスがある間は消えない。
    pub(crate) fn render_hover(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.hover.as_ref()?;
        let theme = self.theme.clone();
        const HOVER_LINE_HEIGHT: f32 = 17.0;
        let estimated_height = px(state.lines.len() as f32 * HOVER_LINE_HEIGHT + 18.0);
        // 原則アンカーの上。titlebar 等に食い込むなら下へ。
        let top = if state.position.y - estimated_height > px(70.) {
            state.position.y - estimated_height - px(8.)
        } else {
            state.position.y + px(20.)
        };
        let left = state.position.x.max(px(60.));
        Some(
            div()
                .absolute()
                .left(left)
                .top(top)
                .max_w(px(560.))
                .occlude()
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .rounded(px(8.))
                .px(px(10.))
                .py(px(7.))
                .shadow(vec![gpui::BoxShadow::new(
                    px(0.),
                    px(6.),
                    gpui::hsla(0., 0., 0., 0.4),
                )
                .blur_radius(px(16.))])
                .flex()
                .flex_col()
                .font_family("Guguru Sans Code")
                .text_size(px(11.5))
                .text_color(theme.fg1)
                .children(state.lines.iter().map(|line| {
                    let display = if line.is_empty() {
                        SharedString::from(" ")
                    } else {
                        line.clone()
                    };
                    div()
                        .h(px(HOVER_LINE_HEIGHT))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(display)
                }))
                .into_any_element(),
        )
    }

    /// LSP 補完ポップアップ（Ctrl-Space / 自動トリガ）。キャレット直下に prefix 絞り込み済み候補。
    /// 上下/Enter・Tab/Esc・印字キーは type-through で絞り込み継続。
    pub(crate) fn render_completion(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.completion.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let focus = state.focus.clone();
        let selected = state.selected;
        let filtered = state.filtered();

        let list = div()
            .flex()
            .flex_col()
            .max_h(px(260.))
            .overflow_hidden()
            .children(filtered.iter().take(12).enumerate().map(|(row, &index)| {
                let item = &state.items[index];
                let is_selected = row == selected;
                div()
                    .id(("completion", row))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(is_selected, |element| element.bg(accent.alpha(0.16)))
                    .hover(|style| style.bg(theme.bg3))
                    .child(
                        div()
                            .flex_none()
                            .w(px(34.))
                            .text_size(px(10.))
                            .text_color(accent)
                            .child(item.kind.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.5))
                            .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                            .child(item.label.clone()),
                    )
                    .when_some(item.detail.clone(), |element, detail| {
                        element.child(
                            div()
                                .flex_none()
                                .max_w(px(150.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(detail),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if let Some(state) = this.completion.as_mut() {
                                state.selected = row;
                            }
                            this.confirm_completion(window, cx)
                        }),
                    )
            }));

        Some(
            div()
                .absolute()
                .inset_0()
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_completion_key_down))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_completion(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(state.position.x)
                        .top(state.position.y + px(2.))
                        .w(px(360.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .p(px(4.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(6.),
                            gpui::hsla(0., 0., 0., 0.4),
                        )
                        .blur_radius(px(16.))])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(list),
                )
                .into_any_element(),
        )
    }

    // branch/worktree メニュー（titlebar の ⎇ クリックで開く）。ブランチ切替（in-place）と、
    // **worktree を別ウィンドウで開く**（並行ブランチ×別窓×スレッド色＝当初ビジョン）。
    // git 操作パネル（ソース管理・M8）。左カラムでエクスプローラと切り替えて出す。
    // 変更一覧（staged/unstaged）・コミット・push/pull・新規ブランチを 1 面にまとめる。
}
