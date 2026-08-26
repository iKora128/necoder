//! shortcut_sheet — キーボードショートカット一覧オーバーレイ（⌘K⌘S）。
//!
//! データは **既定 keymap（`keymap_core::DEFAULT_KEYMAP_JSON`）が唯一の正**。ここを parse して
//! `(context, key, action)` を全列挙し、キーは `pretty_keystroke_for`（mac 記号 / Windows 綴り）で
//! 整形、表示名は `COMMAND_REGISTRY`（workspace 系）＋ 下の `ACTION_LABELS`（editor/agent/その他）で
//! 引く。キーを足すと keymap を触るので**一覧が自動追従**する（二重管理しない）。
//!
//! 参照専用（実行はしない）。コマンドパレットが「コマンド起点・実行できる」のに対し、こちらは
//! 「キー起点・網羅・参照」。UI レイアウトは UI-SPEC §7（カテゴリ fg2 + 名前 + キー右寄せ）。

use crate::workspace::*;

/// `COMMAND_REGISTRY` に無い bound アクションの表示名（i18n キー）。editor/agent/その他。
/// **既定 keymap の全 bound アクションは、レジストリかここで必ずラベルを持つ**（下の test が強制）。
const ACTION_LABELS: &[(&str, &str)] = &[
    // ── エディタ: 編集 ──
    ("editor::Backspace", "key.backspace"),
    ("editor::Delete", "key.delete"),
    ("editor::Newline", "key.newline"),
    ("editor::InsertNewline", "key.insert_newline"),
    ("editor::Copy", "key.copy"),
    ("editor::Cut", "key.cut"),
    ("editor::Paste", "key.paste"),
    ("editor::Undo", "key.undo"),
    ("editor::Redo", "key.redo"),
    ("editor::DeleteWordBackward", "key.delete_word_backward"),
    ("editor::DeleteLine", "key.delete_line"),
    ("editor::DuplicateLineUp", "key.duplicate_line_up"),
    ("editor::DuplicateLineDown", "key.duplicate_line_down"),
    ("editor::MoveLineUp", "key.move_line_up"),
    ("editor::MoveLineDown", "key.move_line_down"),
    ("editor::ToggleComment", "key.toggle_comment"),
    ("editor::TabIndent", "key.tab_indent"),
    ("editor::Indent", "key.indent"),
    ("editor::Outdent", "key.outdent"),
    ("editor::ToggleSoftWrap", "key.toggle_soft_wrap"),
    ("editor::ToggleRenderedMarkdown", "key.toggle_rendered_markdown"),
    // ── エディタ: 移動 ──
    ("editor::MoveLeft", "key.move_left"),
    ("editor::MoveRight", "key.move_right"),
    ("editor::MoveUp", "key.move_up"),
    ("editor::MoveDown", "key.move_down"),
    ("editor::MoveWordLeft", "key.move_word_left"),
    ("editor::MoveWordRight", "key.move_word_right"),
    ("editor::MoveToLineStart", "key.move_line_start"),
    ("editor::MoveToLineEnd", "key.move_line_end"),
    ("editor::MoveToStart", "key.move_to_start"),
    ("editor::MoveToEnd", "key.move_to_end"),
    // ── エディタ: 選択・カーソル ──
    ("editor::SelectAll", "key.select_all"),
    ("editor::SelectLeft", "key.select_left"),
    ("editor::SelectRight", "key.select_right"),
    ("editor::SelectUp", "key.select_up"),
    ("editor::SelectDown", "key.select_down"),
    ("editor::SelectWordLeft", "key.select_word_left"),
    ("editor::SelectWordRight", "key.select_word_right"),
    ("editor::SelectNext", "key.select_next"),
    ("editor::AddCursorAbove", "key.add_cursor_above"),
    ("editor::AddCursorBelow", "key.add_cursor_below"),
    ("editor::Cancel", "key.cancel"),
    // ── AI チャット ──
    ("agent::SubmitPrompt", "key.submit_prompt"),
    ("agent::CloseActiveThread", "key.close_active_thread"),
    // ── その他（レジストリ未収録の workspace / necoder）──
    ("workspace::CommandPalette", "key.command_palette"),
    ("workspace::ControlNext", "key.control_next"),
    ("workspace::Minimize", "key.minimize"),
    ("workspace::Hide", "key.hide"),
    ("workspace::HideOthers", "key.hide_others"),
    ("workspace::ActivateProject1", "key.activate_project"),
    ("workspace::ActivateProject2", "key.activate_project"),
    ("workspace::ActivateProject3", "key.activate_project"),
    ("workspace::ActivateProject4", "key.activate_project"),
    ("workspace::ActivateProject5", "key.activate_project"),
    ("workspace::ActivateProject6", "key.activate_project"),
    ("workspace::ActivateProject7", "key.activate_project"),
    ("workspace::ActivateProject8", "key.activate_project"),
    ("workspace::ActivateProject9", "key.activate_project"),
    ("necoder::Quit", "key.quit"),
];

/// アクション名 → 表示名。レジストリ優先、無ければ補助表、それも無ければアクション名を整形（fallback）。
fn label_for_action(action: &str) -> SharedString {
    if let Some(entry) = COMMAND_REGISTRY
        .entries()
        .iter()
        .find(|entry| entry.action_name == action)
    {
        return SharedString::from(i18n::t!(entry.label_key));
    }
    if let Some((_, key)) = ACTION_LABELS.iter().find(|(name, _)| *name == action) {
        return SharedString::from(i18n::t!(key));
    }
    SharedString::from(prettify_action(action))
}

/// keymap のコンテキスト述語 → セクション見出し（i18n）。既知 4 つ以外はコンテキスト名そのまま。
fn section_label(context: &str) -> SharedString {
    let key = match context {
        "Editor" => Some("key.section_editor"),
        "AgentPanel" => Some("key.section_agent"),
        "FleetControl" => Some("key.section_control"),
        "" => Some("key.section_global"),
        _ => None,
    };
    match key {
        Some(key) => SharedString::from(i18n::t!(key)),
        None => SharedString::from(context.to_string()),
    }
}

/// フォールバックの表示名（`editor::MoveWordLeft` → `Move word left`）。ラベル未整備のアクション用。
/// 正常時はここに来ない（test が全 bound アクションのラベルを保証する）。
fn prettify_action(action: &str) -> String {
    let name = action.rsplit("::").next().unwrap_or(action);
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

impl Workspace {
    /// ⌘K⌘S: ショートカット一覧を開く。Escape 受けに focus を持つ。
    pub(crate) fn open_shortcut_sheet(
        &mut self,
        _: &ShortcutSheet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.shortcut_sheet = Some(focus);
        cx.notify();
    }

    fn close_shortcut_sheet(&mut self, cx: &mut Context<Self>) {
        if self.overlays.shortcut_sheet.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn on_shortcut_sheet_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_shortcut_sheet(cx);
        }
    }

    /// ショートカット一覧の描画（開いている時のみ）。既定 keymap をセクションごとに表で見せる。
    pub(crate) fn render_shortcut_sheet(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let focus = self.overlays.shortcut_sheet.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let platform = keymap_core::KeymapPlatform::current();
        let sections =
            keymap_core::parse(&keymap_core::default_keymap_json(platform)).unwrap_or_default();

        let mut body = div()
            .id("shortcut-sheet-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.))
            .pb(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.));
        for section in &sections {
            if section.bindings.is_empty() {
                continue;
            }
            // 読みやすさのため行は表示名でソート（keymap は keystroke 順で並ぶため）。
            let mut rows: Vec<(SharedString, String)> = section
                .bindings
                .iter()
                .map(|(key, action)| {
                    (
                        label_for_action(action),
                        keymap_core::pretty_keystroke_for(platform, key),
                    )
                })
                .collect();
            rows.sort_by(|left, right| left.0.cmp(&right.0));

            let mut group = div().flex().flex_col().gap(px(1.)).child(
                div()
                    .pt(px(8.))
                    .pb(px(3.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(section_label(&section.context)),
            );
            for (label, keys) in rows {
                group = group.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .py(px(2.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.))
                                .text_color(theme.fg1)
                                .child(label),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family("Guguru Sans Code")
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(keys)),
                        ),
                );
            }
            body = body.child(group);
        }

        let panel = div()
            .w(px(600.))
            .max_h(px(640.))
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .border_1()
            .border_color(accent)
            .rounded(px(12.))
            .overflow_hidden()
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(10.),
                gpui::hsla(0., 0., 0., 0.45),
            )
            .blur_radius(px(28.))])
            .track_focus(focus)
            .on_key_down(cx.listener(Self::on_shortcut_sheet_key_down))
            // パネル内クリックは背景の「閉じる」に伝播させない。
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _window, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(10.))
                    .text_size(px(14.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("key.sheet_title"))),
            )
            .child(body);

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.35))
                // 背景クリックで閉じる。
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.close_shortcut_sheet(cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 既定 keymap の**全 bound アクション**が、レジストリ or 補助表でラベルを持つ（fallback に落ちない）。
    /// キーを足したのにラベルを追補し忘れると、この test が落ちて一覧に生アクション名が出るのを防ぐ。
    #[test]
    fn every_bound_action_has_a_label() {
        let sections =
            keymap_core::parse(keymap_core::DEFAULT_KEYMAP_JSON).expect("既定 keymap がパースできる");
        for section in &sections {
            for action in section.bindings.values() {
                let in_registry = COMMAND_REGISTRY
                    .entries()
                    .iter()
                    .any(|entry| entry.action_name == action);
                let in_table = ACTION_LABELS.iter().any(|(name, _)| name == action);
                assert!(
                    in_registry || in_table,
                    "アクション `{action}` にラベルが無い（ACTION_LABELS に追補せよ）"
                );
            }
        }
    }

    /// セクション見出しは既知 4 つを i18n キーへ、未知はコンテキスト名そのまま。
    #[test]
    fn section_label_maps_known_contexts() {
        // i18n 未初期化でも panic しない（t! はキー欠落時もフォールバックする前提）だが、
        // ここでは「未知コンテキストはそのまま返す」ことだけ確認する。
        assert_eq!(section_label("MysteryContext").as_ref(), "MysteryContext");
    }
}
