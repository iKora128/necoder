//! shortcut_sheet — キーボードショートカット一覧オーバーレイ（⌘K⌘S）。
//!
//! データは **既定 keymap（`keymap_core::DEFAULT_KEYMAP_JSON`）が唯一の正**。ここを parse して
//! `(context, key, action)` を全列挙し、キーは `pretty_keystroke_for`（mac 記号 / Windows 綴り）で
//! 整形、表示名は `COMMAND_REGISTRY`（workspace 系）＋ 下の `ACTION_LABELS`（editor/agent/その他）で
//! 引く。キーを足すと keymap を触るので**一覧が自動追従**する（二重管理しない）。
//!
//! 実行はしない。コマンドパレットが「コマンド起点・実行できる」のに対し、こちらは「キー起点・網羅」。
//! UI レイアウトは UI-SPEC §7（カテゴリ fg2 + 名前 + キー右寄せ）。
//!
//! 出すのは既定とユーザーの keymap.json を重ねた、いま効いている割り当て。行を押すと割り当てを
//! 変えられる（キー割り当ての画面・O27・`keymap_editing`）。

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
    ("editor::DeleteWordForward", "key.delete_word_forward"),
    ("editor::DeleteToLineStart", "key.delete_to_line_start"),
    ("editor::DeleteToLineEnd", "key.delete_to_line_end"),
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
    (
        "editor::ToggleRenderedMarkdown",
        "key.toggle_rendered_markdown",
    ),
    ("editor::ToggleSidePreview", "key.toggle_side_preview"),
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
    ("editor::SelectToLineStart", "key.select_to_line_start"),
    ("editor::SelectToLineEnd", "key.select_to_line_end"),
    ("editor::SelectToStart", "key.select_to_start"),
    ("editor::SelectToEnd", "key.select_to_end"),
    ("editor::SelectNext", "key.select_next"),
    ("editor::AddCursorAbove", "key.add_cursor_above"),
    ("editor::AddCursorBelow", "key.add_cursor_below"),
    ("editor::Cancel", "key.cancel"),
    // ── エクスプローラ ──
    ("workspace::UndoFileOperation", "key.undo_file_operation"),
    // ── AI チャット ──
    ("agent::SubmitPrompt", "key.submit_prompt"),
    ("agent::CloseActiveThread", "key.close_active_thread"),
    ("agent::FindInTranscript", "key.find_in_transcript"),
    // ── ターミナル ──
    ("terminal::Copy", "key.terminal_copy"),
    ("terminal::Paste", "key.terminal_paste"),
    ("terminal::SelectAll", "key.terminal_select_all"),
    ("terminal::Clear", "key.terminal_clear"),
    ("terminal::Find", "key.terminal_find"),
    ("terminal::NewTab", "key.terminal_new_tab"),
    ("terminal::CloseTab", "key.terminal_close_tab"),
    ("terminal::Split", "key.terminal_split"),
    // ── Fleet（FLEET-V2 §7 のキー表と同じ語を使う）──
    ("workspace::NewTask", "fleet.new_task"),
    ("workspace::StageOne", "fleet.stage_one"),
    ("workspace::StageTwo", "fleet.stage_two"),
    ("workspace::StageThree", "fleet.stage_three"),
    ("workspace::ToggleLineage", "fleet.lineage"),
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

/// keymap のコンテキスト述語 → セクション見出し（i18n）。既知 5 つ以外はコンテキスト名そのまま。
fn section_label(context: &str) -> SharedString {
    let key = match context {
        "Editor" => Some("key.section_editor"),
        "AgentPanel" => Some("key.section_agent"),
        "FleetControl" => Some("key.section_control"),
        keymap_core::TERMINAL_CONTEXT => Some("key.section_terminal"),
        "Explorer" => Some("key.section_explorer"),
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
        // 開くたびに keymap.json を読み直す（手で直した分も映す・O27）。
        self.overlays.keymap_editing = Some(super::keymap_editing::KeymapEditing::load(
            settings::user_keymap_path(cx),
        ));
        cx.notify();
    }

    pub(crate) fn close_shortcut_sheet(&mut self, cx: &mut Context<Self>) {
        // 編集の状態ごと落とす（キーを待っていれば、その受け口も外れる）。
        self.overlays.keymap_editing = None;
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
    /// 既定キーマップで **action に割り当てられているキー**の表示名（`⌘⇧M` / `Ctrl+Shift+M`）。
    /// 案内文にキーを直書きすると keymap と静かにズレるので、文言側はここから受け取る。
    pub(crate) fn shortcut_label_for(action_name: &str) -> Option<String> {
        let platform = keymap_core::KeymapPlatform::current();
        keymap_core::parse(&keymap_core::default_keymap_json(platform))
            .ok()?
            .iter()
            .flat_map(|section| section.bindings.iter())
            .find(|(_, action)| action.as_str() == action_name)
            .map(|(key, _)| keymap_core::pretty_keystroke_for(platform, key))
    }

    pub(crate) fn render_shortcut_sheet(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let focus = self.overlays.shortcut_sheet.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();
        let platform = keymap_core::KeymapPlatform::current();
        // 既定 + ユーザーの keymap.json（O27・いま効いている割り当て）。
        let rows = self.keymap_rows();
        let editing = self.overlays.keymap_editing.as_ref();
        let capturing = editing.and_then(|editing| editing.capturing.clone());
        let can_edit =
            editing.is_some_and(|editing| editing.unreadable.is_none() && editing.path.is_some());

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
        let mut contexts: Vec<&str> = Vec::new();
        for row in &rows {
            if !contexts.contains(&row.context.as_str()) {
                contexts.push(row.context.as_str());
            }
        }
        for context in contexts {
            // 読みやすさのため行は表示名でソート（keymap は keystroke 順で並ぶため）。
            let mut labeled: Vec<(SharedString, &keymap_core::user_keymap::ActionBinding)> = rows
                .iter()
                .filter(|row| row.context == context)
                .map(|row| (label_for_action(&row.action), row))
                .collect();
            labeled.sort_by(|left, right| left.0.cmp(&right.0));

            let mut group = div().flex().flex_col().gap(px(1.)).child(
                div()
                    .pt(px(8.))
                    .pb(px(3.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(section_label(context)),
            );
            for (label, row) in labeled {
                let waiting = capturing.as_ref().is_some_and(|(context, action)| {
                    *context == row.context && *action == row.action
                });
                let keys = if waiting {
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(accent)
                        .child(SharedString::from(i18n::t!("key.press_keys")))
                } else {
                    let text = if row.keys.is_empty() {
                        "—".to_string()
                    } else {
                        row.keys
                            .iter()
                            .map(|key| keymap_core::pretty_keystroke_for(platform, key))
                            .collect::<Vec<_>>()
                            .join("  ·  ")
                    };
                    div()
                        .flex_none()
                        .font_family(ui::code_font(cx))
                        .text_size(px(11.))
                        .text_color(if row.customized { theme.fg0 } else { theme.fg2 })
                        .child(SharedString::from(text))
                };
                let id = SharedString::from(format!("keymap-row-{}-{}", row.context, row.action));
                let (context, action) = (row.context.clone(), row.action.clone());
                let mut line = div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(4.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .when(waiting, |line| line.bg(theme.bg2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .text_color(theme.fg1)
                            .child(label),
                    );
                if row.customized && can_edit {
                    let (reset_context, reset_action) = (context.clone(), action.clone());
                    line = line.child(
                        div()
                            .id(SharedString::from(format!(
                                "keymap-reset-{}-{}",
                                row.context, row.action
                            )))
                            .flex_none()
                            .px(px(4.))
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.fg0))
                            .tooltip(ui::Tooltip::text(i18n::t!("key.reset"), theme.clone()))
                            .child("↺")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.reset_key_binding(&reset_context, &reset_action, cx);
                                }),
                            ),
                    );
                }
                line = line.child(keys);
                if can_edit {
                    line = line
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.start_key_capture(context.clone(), action.clone(), cx)
                            }),
                        );
                }
                group = group.child(line);
            }
            body = body.child(group);
        }

        // 置き換えるかの確認・読めない keymap.json の知らせ（見出しの下の帯）。
        let banner = editing.and_then(|editing| {
            if let Some(pending) = &editing.pending {
                let taken: Vec<String> = pending
                    .taken
                    .iter()
                    .map(|(context, action)| {
                        format!("{}（{}）", label_for_action(action), section_label(context))
                    })
                    .collect();
                let text = i18n::t!(
                    "key.conflict",
                    "keys" => keymap_core::pretty_keystroke_for(platform, &pending.keystroke),
                    "actions" => taken.join("・")
                );
                let button = |id: &'static str, label: String, primary: bool| {
                    div()
                        .id(id)
                        .flex_none()
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(if primary { accent } else { theme.border })
                        .text_size(px(11.5))
                        .text_color(if primary { accent } else { theme.fg1 })
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3))
                        .child(SharedString::from(label))
                };
                return Some(
                    div()
                        .mx(px(16.))
                        .mb(px(8.))
                        .p(px(10.))
                        .rounded(px(8.))
                        .bg(theme.bg2)
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.))
                                .text_color(theme.fg0)
                                .child(SharedString::from(text)),
                        )
                        .child(
                            button("keymap-keep", i18n::t!("key.keep"), false).on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _window, cx| {
                                    this.answer_pending_rebind(false, cx)
                                }),
                            ),
                        )
                        .child(
                            button("keymap-replace", i18n::t!("key.replace"), true).on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _window, cx| {
                                    this.answer_pending_rebind(true, cx)
                                }),
                            ),
                        )
                        .into_any_element(),
                );
            }
            editing.unreadable.as_ref().map(|error| {
                div()
                    .mx(px(16.))
                    .mb(px(8.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(theme.bg2)
                    .text_size(px(12.))
                    .text_color(theme.warn)
                    .child(SharedString::from(
                        i18n::t!("key.unreadable", "error" => error),
                    ))
                    .into_any_element()
            })
        });

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
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.fg0)
                                    .child(SharedString::from(i18n::t!("key.sheet_title"))),
                            )
                            .when(
                                editing.is_some_and(|editing| editing.path.is_some()),
                                |row| {
                                    row.child(
                                        div()
                                            .id("keymap-open-json")
                                            .flex_none()
                                            .text_size(px(11.))
                                            .text_color(theme.fg2)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(theme.fg0))
                                            .child(SharedString::from(i18n::t!("key.open_keymap")))
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                cx.listener(|this, _, window, cx| {
                                                    this.open_user_keymap(window, cx)
                                                }),
                                            ),
                                    )
                                },
                            ),
                    )
                    .when(can_edit, |header| {
                        header.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("key.sheet_hint"))),
                        )
                    }),
            )
            .children(banner)
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
        let sections = keymap_core::parse(keymap_core::DEFAULT_KEYMAP_JSON)
            .expect("既定 keymap がパースできる");
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

    /// セクション見出しは既知 5 つを i18n キーへ、未知はコンテキスト名そのまま。
    #[test]
    fn section_label_maps_known_contexts() {
        // i18n 未初期化でも panic しない（t! はキー欠落時もフォールバックする前提）だが、
        // ここでは「未知コンテキストはそのまま返す」ことだけ確認する。
        assert_eq!(section_label("MysteryContext").as_ref(), "MysteryContext");
    }
}
