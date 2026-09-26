//! キー割り当ての画面（O27・D23 / H09）。⌘K ⌘S のショートカット一覧で、行のキーを押すと次に押した
//! キーへ割り当て直し、↺ で既定に戻す。書くのはユーザーの keymap.json だけ（既定の keymap は触らない）。
//! 書いた後はアプリの keymap の監視が束を全部張り直す（消した束も外れる・`necoder` の main）。
//!
//! キーは `cx.intercept_keystrokes` で受ける: 一覧の `on_key_down` では、⌘P のように既に割り当てた
//! キーが先にアクションになって届かない。待っている間だけ受け、Esc でやめる。
//! 他のアクションが使っているキーなら、置き換えるかを先に聞く（黙って奪わない）。

use crate::workspace::*;
use keymap_core::user_keymap::{self, ActionBinding};
use keymap_core::KeymapSection;

/// 一覧を開いている間の編集の状態。
pub(crate) struct KeymapEditing {
    /// ユーザーの keymap.json（設定の置き場の隣・`None` = 置き場が分からない＝書けない）。
    pub(crate) path: Option<PathBuf>,
    /// 読めたユーザーの keymap。書いたらここも差し替える。
    pub(crate) user: Vec<KeymapSection>,
    /// 既定と重ねた行（読んだ時と書いた時に作り直す・描画のたびに重ね直さない）。
    pub(crate) rows: Vec<ActionBinding>,
    /// 読めなかった理由。壊れた keymap.json は書き換えない（直すまで割り当ては変えられない）。
    pub(crate) unreadable: Option<String>,
    /// キーを待っている行（context, action）。
    pub(crate) capturing: Option<(String, String)>,
    /// 他のアクションが使っているキーを選んだ（置き換えるかを聞いている）。
    pub(crate) pending: Option<PendingRebind>,
    /// キーを待っている間だけの受け口（落とすと受けなくなる）。
    interceptor: Option<gpui::Subscription>,
}

/// 置き換えるかを聞いている割り当て。
pub(crate) struct PendingRebind {
    pub(crate) context: String,
    pub(crate) action: String,
    pub(crate) keystroke: String,
    /// そのキーをいま使っている行（context, action）。
    pub(crate) taken: Vec<(String, String)>,
}

impl KeymapEditing {
    /// keymap.json を読む（無い = 空。読めない・壊れていれば理由を持ち、何も書かない）。
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let (user, unreadable) = match path.as_ref().map(std::fs::read_to_string) {
            None => (Vec::new(), None),
            Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
            Some(Err(error)) => (Vec::new(), Some(error.to_string())),
            Some(Ok(text)) => match user_keymap::parse_user(&text) {
                Ok(user) => (user, None),
                Err(error) => (Vec::new(), Some(format!("{error:#}"))),
            },
        };
        let rows = user_keymap::effective_bindings(&default_sections(), &user);
        Self {
            path,
            user,
            rows,
            unreadable,
            capturing: None,
            pending: None,
            interceptor: None,
        }
    }
}

/// このプラットフォームの既定の keymap（セクション列）。
pub(crate) fn default_sections() -> Vec<KeymapSection> {
    keymap_core::parse(&keymap_core::default_keymap_json(
        keymap_core::KeymapPlatform::current(),
    ))
    .unwrap_or_default()
}

/// 待っている時に来たキーを keymap の書き方（`cmd-shift-p`）にする。修飾キーだけの押下は `None`
/// （そのまま次のキーを待つ）。
pub(crate) fn keystroke_text(keystroke: &gpui::Keystroke) -> Option<String> {
    const MODIFIER_KEYS: [&str; 12] = [
        "shift", "control", "ctrl", "alt", "option", "platform", "cmd", "command", "super", "win",
        "function", "fn",
    ];
    let key = keystroke.key.as_str();
    if key.is_empty() || MODIFIER_KEYS.contains(&key) {
        return None;
    }
    Some(user_keymap::keystroke_text(keystroke))
}

impl Workspace {
    /// いま効いている割り当て（既定 + ユーザーの keymap.json）。一覧の行。
    pub(crate) fn keymap_rows(&self) -> Vec<ActionBinding> {
        match self.overlays.keymap_editing.as_ref() {
            Some(editing) => editing.rows.clone(),
            None => user_keymap::effective_bindings(&default_sections(), &[]),
        }
    }

    /// 行のキーを押した: 次に押したキーを待つ。
    pub(crate) fn start_key_capture(
        &mut self,
        context: String,
        action: String,
        cx: &mut Context<Self>,
    ) {
        let workspace = cx.entity().downgrade();
        let Some(editing) = self.overlays.keymap_editing.as_mut() else {
            return;
        };
        if editing.unreadable.is_some() || editing.path.is_none() {
            return;
        }
        editing.pending = None;
        editing.capturing = Some((context, action));
        editing.interceptor = Some(cx.intercept_keystrokes(move |event, _window, cx| {
            let keystroke = event.keystroke.clone();
            workspace
                .update(cx, |workspace, cx| {
                    workspace.capture_keystroke(&keystroke, cx)
                })
                .ok();
            // 待っている間のキーはアクションにしない（⌘P を割り当てたいのにファイル検索が開く、を防ぐ）。
            cx.stop_propagation();
        }));
        cx.notify();
    }

    /// 待つのをやめる（Esc・別の行を押した・一覧を閉じた）。
    pub(crate) fn cancel_key_capture(&mut self, cx: &mut Context<Self>) {
        if let Some(editing) = self.overlays.keymap_editing.as_mut() {
            editing.capturing = None;
            editing.interceptor = None;
            cx.notify();
        }
    }

    /// 待っている時に来たキー。空いていれば割り当て、他が使っていれば置き換えるかを聞く。
    pub(crate) fn capture_keystroke(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let Some((context, action)) = self
            .overlays
            .keymap_editing
            .as_ref()
            .and_then(|editing| editing.capturing.clone())
        else {
            return;
        };
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.cancel_key_capture(cx);
            return;
        }
        let Some(text) = keystroke_text(keystroke) else {
            return;
        };
        self.cancel_key_capture(cx);
        let rows = self.keymap_rows();
        let taken: Vec<(String, String)> =
            user_keymap::conflicts(&rows, &text, (&context, &action))
                .into_iter()
                .map(|row| (row.context.clone(), row.action.clone()))
                .collect();
        if taken.is_empty() {
            self.apply_rebind(&context, &action, &text, cx);
        } else if let Some(editing) = self.overlays.keymap_editing.as_mut() {
            editing.pending = Some(PendingRebind {
                context,
                action,
                keystroke: text,
                taken,
            });
            cx.notify();
        }
    }

    /// 置き換えるかの答え（`true` = 置き換える）。
    pub(crate) fn answer_pending_rebind(&mut self, replace: bool, cx: &mut Context<Self>) {
        let Some(pending) = self
            .overlays
            .keymap_editing
            .as_mut()
            .and_then(|editing| editing.pending.take())
        else {
            return;
        };
        if replace {
            self.apply_rebind(&pending.context, &pending.action, &pending.keystroke, cx);
        }
        cx.notify();
    }

    fn apply_rebind(
        &mut self,
        context: &str,
        action: &str,
        keystroke: &str,
        cx: &mut Context<Self>,
    ) {
        self.edit_user_keymap(cx, |user| {
            user_keymap::rebind(user, &default_sections(), context, action, keystroke)
        });
    }

    /// 行の ↺: 既定に戻す。
    pub(crate) fn reset_key_binding(
        &mut self,
        context: &str,
        action: &str,
        cx: &mut Context<Self>,
    ) {
        self.edit_user_keymap(cx, |user| {
            user_keymap::reset(user, &default_sections(), context, action)
        });
    }

    /// ユーザーの keymap を書き換えて keymap.json に書く。書けなければ知らせて元のまま。
    fn edit_user_keymap(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Vec<KeymapSection>),
    ) {
        let Some(editing) = self.overlays.keymap_editing.as_mut() else {
            return;
        };
        let (Some(path), None) = (editing.path.clone(), editing.unreadable.as_ref()) else {
            return;
        };
        let mut user = editing.user.clone();
        edit(&mut user);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, user_keymap::to_json(&user)));
        match written {
            Ok(()) => {
                editing.rows = user_keymap::effective_bindings(&default_sections(), &user);
                editing.user = user;
            }
            Err(error) => {
                let text = i18n::t!(
                    "key.write_failed",
                    "path" => path.display(),
                    "error" => error
                );
                self.push_failure_toast(SharedString::from(text), None, cx);
            }
        }
        cx.notify();
    }

    /// keymap.json をエディタのタブで開く（無ければ空の配列で作る）。一覧は閉じる。
    pub(crate) fn open_user_keymap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .overlays
            .keymap_editing
            .as_ref()
            .and_then(|editing| editing.path.clone())
        else {
            return;
        };
        if !path.exists() {
            let created = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, "[]\n"));
            if let Err(error) = created {
                let text = i18n::t!(
                    "key.write_failed",
                    "path" => path.display(),
                    "error" => error
                );
                self.push_failure_toast(SharedString::from(text), None, cx);
                return;
            }
        }
        self.close_shortcut_sheet(cx);
        self.open_file(path, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(text: &str) -> gpui::Keystroke {
        gpui::Keystroke::parse(text).expect("キー")
    }

    /// 一覧でキーを割り当て直す・置き換えるかを聞く・戻す。書くのは設定の隣の keymap.json だけ。
    #[gpui::test]
    fn keys_are_rebound_from_the_sheet(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_keymap_editing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        let keymap_path = root.join("keymap.json");
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.open_shortcut_sheet(&ShortcutSheet, window, cx);
            let editing = workspace
                .overlays
                .keymap_editing
                .as_ref()
                .expect("編集の状態");
            assert_eq!(editing.path.as_deref(), Some(keymap_path.as_path()));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // 空いているキー → そのまま割り当てる（修飾キーだけの押下は待ち続ける）。
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.start_key_capture(
                "Editor".to_string(),
                "editor::DuplicateLineDown".to_string(),
                cx,
            );
            let shift_alone = gpui::Keystroke {
                modifiers: gpui::Modifiers::shift(),
                key: "shift".to_string(),
                key_char: None,
            };
            workspace.capture_keystroke(&shift_alone, cx);
            assert!(workspace
                .overlays
                .keymap_editing
                .as_ref()
                .is_some_and(|editing| editing.capturing.is_some()));
            workspace.capture_keystroke(&keystroke("ctrl-alt-shift-f9"), cx);
            let rows = workspace.keymap_rows();
            let row = rows
                .iter()
                .find(|row| row.action == "editor::DuplicateLineDown")
                .expect("行");
            assert_eq!(row.keys, vec!["ctrl-alt-shift-f9".to_string()]);
            assert!(row.customized);
        });
        let written = std::fs::read_to_string(&keymap_path).expect("keymap.json を書いた");
        assert!(written.contains("ctrl-alt-shift-f9"), "{written}");

        // 実際のキーは受け口（intercept_keystrokes）から入る。
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.start_key_capture("Editor".to_string(), "editor::Undo".to_string(), cx);
        });
        cx.simulate_keystrokes("ctrl-alt-shift-f8");
        workspace.update_in(cx, |workspace, _window, cx| {
            let row = workspace
                .keymap_rows()
                .into_iter()
                .find(|row| row.context == "Editor" && row.action == "editor::Undo")
                .expect("行");
            assert_eq!(row.keys, vec!["ctrl-alt-shift-f8".to_string()]);
            workspace.reset_key_binding("Editor", "editor::Undo", cx);
        });

        // 使われているキー → 置き換えるかを聞き、「やめる」なら変えない。
        workspace.update_in(cx, |workspace, _window, cx| {
            let taken = workspace
                .keymap_rows()
                .into_iter()
                .find(|row| row.action == "editor::DuplicateLineUp")
                .and_then(|row| row.keys.first().cloned())
                .expect("既定のキーがある");
            workspace.start_key_capture(
                "Editor".to_string(),
                "editor::DuplicateLineDown".to_string(),
                cx,
            );
            workspace.capture_keystroke(&keystroke(&taken), cx);
            assert!(workspace
                .overlays
                .keymap_editing
                .as_ref()
                .is_some_and(|editing| editing.pending.is_some()));
            workspace.answer_pending_rebind(false, cx);
            let row = workspace
                .keymap_rows()
                .into_iter()
                .find(|row| row.action == "editor::DuplicateLineDown")
                .expect("行");
            assert_eq!(row.keys, vec!["ctrl-alt-shift-f9".to_string()], "変えない");

            // Esc はやめる。↺ で既定に戻す。
            workspace.start_key_capture(
                "Editor".to_string(),
                "editor::DuplicateLineDown".to_string(),
                cx,
            );
            workspace.capture_keystroke(&keystroke("escape"), cx);
            assert!(workspace
                .overlays
                .keymap_editing
                .as_ref()
                .is_some_and(|editing| editing.capturing.is_none()));
            workspace.reset_key_binding("Editor", "editor::DuplicateLineDown", cx);
            assert!(!workspace
                .keymap_rows()
                .into_iter()
                .any(|row| row.action == "editor::DuplicateLineDown" && row.customized));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            std::fs::read_to_string(&keymap_path)
                .expect("読める")
                .trim(),
            "[]",
            "戻すと何も残らない"
        );

        // 壊れた keymap.json は書き換えない。
        std::fs::write(&keymap_path, "{ broken").unwrap();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_shortcut_sheet(&ShortcutSheet, window, cx);
            assert!(workspace
                .overlays
                .keymap_editing
                .as_ref()
                .is_some_and(|editing| editing.unreadable.is_some()));
            workspace.reset_key_binding("Editor", "editor::Undo", cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            std::fs::read_to_string(&keymap_path).expect("読める"),
            "{ broken"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
