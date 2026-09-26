//! 自動保存（O26・D01）。設定 `auto_save`（既定 `"off"` = ⌘S だけ）。
//!
//! - **他へ移った時**（`focus_change` と `after_delay`）: エディタのタブからフォーカスが外れた時に
//!   そのタブを、窓を離れた時（別の窓・別のアプリ）にこの窓の未保存のタブを保存する。
//! - **手を止めた時**（`after_delay`）: 最後の編集から [`AUTO_SAVE_DELAY`] 何も打たなければ、この窓の
//!   未保存のタブを保存する。
//!
//! 保存しないもの: 無題・読み取り専用・一時タブ（diff 等）・**外で変わったと警告が出ているタブ**
//! （上書きするかは人が決める。保存そのものも、読み込み時の revision と食い違えば断られる）。
//! 手を止めた時の保存は、necoder が読み直す設定ファイル（設定フォルダの中と `.necoder/` の中）を除く —
//! 書きかけの JSON を保存すると、読み直した設定が既定に戻ってしまう（他へ移った時は保存する）。
//! 自動保存ではフォーマットしない（打っている途中の行を動かさない・⌘S の `format_on_save` は別）。

use crate::workspace::*;
use settings_core::AutoSave;

/// 手を止めてから保存するまでの間。
pub(crate) const AUTO_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// 何をきっかけに保存するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutoSaveTrigger {
    /// 他へ移った（エディタからフォーカスが外れた・窓を離れた）。
    FocusChange,
    /// 手を止めた。
    Pause,
}

/// この設定で、このきっかけの時に保存するか。
pub(crate) fn auto_save_applies(mode: AutoSave, trigger: AutoSaveTrigger) -> bool {
    match mode {
        AutoSave::Off => false,
        AutoSave::OnFocusChange => trigger == AutoSaveTrigger::FocusChange,
        AutoSave::AfterDelay => true,
    }
}

/// 手を止めた時に保存してよいファイルか。necoder が読み直す設定ファイル（設定フォルダの中・
/// `.necoder/` の中）は除く。
pub(crate) fn saves_on_pause(path: &Path, config_dir: Option<&Path>) -> bool {
    let in_config_dir = config_dir.is_some_and(|dir| path.starts_with(dir));
    let in_project_config = path
        .components()
        .any(|component| component.as_os_str() == ".necoder");
    !in_config_dir && !in_project_config
}

/// いまの設定の自動保存（設定を丸ごと複製しない・編集のたびに読むので）。
fn auto_save_mode(cx: &App) -> AutoSave {
    cx.try_global::<settings::SettingsGlobal>()
        .map(|global| global.settings().auto_save_mode())
        .unwrap_or(AutoSave::Off)
}

impl EditorTab {
    /// 自動保存するタブか: 未保存・ファイルがある・読み取り専用でない・一時タブでない・外で変わった
    /// 警告が出ていない（手を止めた時は、設定ファイルでもない）。
    fn wants_auto_save(
        &self,
        trigger: AutoSaveTrigger,
        config_dir: Option<&Path>,
        cx: &App,
    ) -> bool {
        if self.transient {
            return false;
        }
        let Some(editor) = self.editor() else {
            return false;
        };
        let view = editor.read(cx);
        let buffer = view.buffer();
        let Some(path) = buffer.path() else {
            return false;
        };
        buffer.is_dirty()
            && !buffer.is_read_only()
            && !view.is_externally_changed()
            && (trigger == AutoSaveTrigger::FocusChange || saves_on_pause(path, config_dir))
    }
}

impl Workspace {
    /// 編集のたびに呼ぶ（ファイルのバッファの version が変わった時）: 手を止めた時の保存を予約し直す。
    pub(crate) fn schedule_auto_save(&mut self, cx: &mut Context<Self>) {
        if !auto_save_applies(auto_save_mode(cx), AutoSaveTrigger::Pause) {
            return;
        }
        self.auto_save_generation = self.auto_save_generation.wrapping_add(1);
        let generation = self.auto_save_generation;
        cx.spawn(async move |workspace, cx| {
            cx.background_executor().timer(AUTO_SAVE_DELAY).await;
            // Err = 待っている間に窓が閉じた（保存するタブももう無い）。
            workspace
                .update(cx, |workspace, cx| {
                    // 後から編集が来ている＝その回に任せる。
                    if workspace.auto_save_generation == generation {
                        workspace.auto_save_all(AutoSaveTrigger::Pause, cx);
                    }
                })
                .ok();
        })
        .detach();
    }

    /// タブのエディタからフォーカスが外れたら自動保存する購読（タブごとに持つ）。
    pub(crate) fn auto_save_on_blur(
        &mut self,
        editor: &Entity<EditorView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let handle = editor.read(cx).focus_handle(cx);
        let editor = editor.downgrade();
        cx.on_blur(&handle, window, move |workspace, _window, cx| {
            if let Some(editor) = editor.upgrade() {
                workspace.auto_save_blurred(&editor, cx);
            }
        })
    }

    /// エディタのタブからフォーカスが外れた: そのタブを保存する（他へ移った時）。
    fn auto_save_blurred(&mut self, editor: &Entity<EditorView>, cx: &mut Context<Self>) {
        if !auto_save_applies(auto_save_mode(cx), AutoSaveTrigger::FocusChange) {
            return;
        }
        let config_dir = paths::config_dir();
        let wants = self
            .all_editor_tabs()
            .find(|tab| tab.editor() == Some(editor))
            .is_some_and(|tab| {
                tab.wants_auto_save(AutoSaveTrigger::FocusChange, config_dir.as_deref(), cx)
            });
        if wants {
            editor.update(cx, |view, cx| view.save_now(cx));
        }
    }

    /// 窓を離れた（別の窓・別のアプリへ）: この窓の未保存のタブを保存する（他へ移った時）。
    pub(crate) fn auto_save_window_left(&mut self, cx: &mut Context<Self>) {
        self.auto_save_all(AutoSaveTrigger::FocusChange, cx);
    }

    /// この窓の全 session のタブのうち、自動保存するものを保存する。
    fn auto_save_all(&mut self, trigger: AutoSaveTrigger, cx: &mut Context<Self>) {
        if !auto_save_applies(auto_save_mode(cx), trigger) {
            return;
        }
        let config_dir = paths::config_dir();
        let editors: Vec<Entity<EditorView>> = self
            .all_editor_tabs()
            .filter(|tab| tab.wants_auto_save(trigger, config_dir.as_deref(), cx))
            .filter_map(|tab| tab.editor().cloned())
            .collect();
        for editor in editors {
            editor.update(cx, |view, cx| view.save_now(cx));
        }
    }

    /// この窓の全 session（レールのプロジェクトと Chat）のタブ。
    fn all_editor_tabs(&self) -> impl Iterator<Item = &EditorTab> {
        self.project_sessions
            .sessions
            .iter()
            .chain(self.project_sessions.chat.iter())
            .flat_map(|session| session.tabs.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn each_mode_saves_on_its_own_triggers() {
        use AutoSaveTrigger::{FocusChange, Pause};
        assert!(!auto_save_applies(AutoSave::Off, FocusChange));
        assert!(!auto_save_applies(AutoSave::Off, Pause));
        assert!(auto_save_applies(AutoSave::OnFocusChange, FocusChange));
        assert!(!auto_save_applies(AutoSave::OnFocusChange, Pause));
        assert!(auto_save_applies(AutoSave::AfterDelay, FocusChange));
        assert!(auto_save_applies(AutoSave::AfterDelay, Pause));
    }

    #[test]
    fn settings_files_are_not_saved_mid_edit() {
        let config = Path::new("/home/me/.config/necoder");
        assert!(saves_on_pause(
            Path::new("/work/app/src/main.rs"),
            Some(config)
        ));
        assert!(!saves_on_pause(
            Path::new("/home/me/.config/necoder/settings.json"),
            Some(config)
        ));
        assert!(!saves_on_pause(
            Path::new("/work/app/.necoder/settings.json"),
            Some(config)
        ));
        assert!(saves_on_pause(Path::new("/work/app/settings.json"), None));
    }

    /// 設定 `auto_save` を `mode` にした窓で `files`（中身は "one\n"）を開く。
    fn auto_save_workspace<'a>(
        name: &str,
        mode: &str,
        files: &[&str],
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("necoder_auto_save_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("作業フォルダを作れる");
        for file in files {
            std::fs::write(project.join(file), "one\n").expect("ファイルを書ける");
        }
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            format!(r#"{{"onboarded":true,"agent_prewarm":false,"auto_save":"{mode}"}}"#),
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            for file in files {
                workspace.open_file(project.join(file), window, cx);
            }
        });
        // テストの窓は前に出ていない（前に出ていない窓ではフォーカスの出入りが起きない）。
        cx.update(|window, _cx| window.activate_window());
        cx.run_until_parked();
        (workspace, cx, root)
    }

    fn editor_for(
        workspace: &Entity<Workspace>,
        name: &str,
        cx: &mut gpui::VisualTestContext,
    ) -> Entity<EditorView> {
        workspace.read_with(cx, |workspace, _| {
            workspace
                .tabs
                .iter()
                .find(|tab| tab.path.ends_with(name))
                .and_then(|tab| tab.editor().cloned())
                .expect("そのファイルのエディタのタブ")
        })
    }

    fn on_disk(root: &Path, name: &str) -> String {
        std::fs::read_to_string(root.join("project").join(name)).expect("ファイルを読める")
    }

    #[gpui::test]
    fn a_pause_in_typing_saves_the_file(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = auto_save_workspace("pause", "after_delay", &["a.txt"], cx);
        let editor = editor_for(&workspace, "a.txt", cx);
        editor.update(cx, |editor, cx| editor.insert_text("two ", cx));
        cx.run_until_parked();
        assert_eq!(on_disk(&root, "a.txt"), "one\n", "打った直後はまだ書かない");

        cx.executor().advance_clock(AUTO_SAVE_DELAY / 2);
        editor.update(cx, |editor, cx| editor.insert_text("three ", cx));
        cx.executor()
            .advance_clock(AUTO_SAVE_DELAY / 2 + Duration::from_millis(100));
        cx.run_until_parked();
        assert_eq!(
            on_disk(&root, "a.txt"),
            "one\n",
            "打ち続けている間は書かない"
        );

        cx.executor().advance_clock(AUTO_SAVE_DELAY);
        cx.run_until_parked();
        assert_eq!(on_disk(&root, "a.txt"), "two three one\n");
        assert!(!editor.read_with(cx, |editor, _| editor.buffer().is_dirty()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn leaving_a_tab_or_the_window_saves_what_was_left(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) =
            auto_save_workspace("focus", "focus_change", &["a.txt", "b.txt", "c.txt"], cx);
        let a = editor_for(&workspace, "a.txt", cx);
        let b = editor_for(&workspace, "b.txt", cx);
        let c = editor_for(&workspace, "c.txt", cx);
        // c.txt が前（最後に開いた）。
        c.update(cx, |editor, cx| editor.insert_text("c ", cx));
        cx.executor().advance_clock(AUTO_SAVE_DELAY * 3);
        cx.run_until_parked();
        assert_eq!(
            on_disk(&root, "c.txt"),
            "one\n",
            "他へ移った時だけの設定では、手を止めても書かない"
        );

        // a.txt へ移る = c.txt を離れた。
        cx.update(|window, cx| {
            let handle = a.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.run_until_parked();
        assert_eq!(on_disk(&root, "c.txt"), "c one\n", "離れたタブを保存する");
        assert_eq!(
            on_disk(&root, "a.txt"),
            "one\n",
            "触っていないタブは書かない"
        );

        // b.txt を（前に出さずに）書き換え、a.txt にも打ってから窓を離れる = 両方保存する。
        b.update(cx, |editor, cx| editor.insert_text("b ", cx));
        a.update(cx, |editor, cx| editor.insert_text("a ", cx));
        cx.deactivate_window();
        cx.run_until_parked();
        assert_eq!(on_disk(&root, "a.txt"), "a one\n");
        assert_eq!(
            on_disk(&root, "b.txt"),
            "b one\n",
            "前に出ていないタブも窓を離れたら保存する"
        );
        for editor in [&a, &b, &c] {
            assert!(!editor.read_with(cx, |editor, _| editor.buffer().is_dirty()));
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn files_changed_outside_are_not_overwritten(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = auto_save_workspace("outside", "after_delay", &["a.txt"], cx);
        let editor = editor_for(&workspace, "a.txt", cx);
        editor.update(cx, |editor, cx| editor.insert_text("mine ", cx));
        // 打っている間に、外（エージェント・別のエディタ）が書き換えた。
        std::fs::write(root.join("project/a.txt"), "theirs, longer\n").expect("書ける");
        editor.update(cx, |editor, cx| editor.handle_external_change(cx));
        assert!(editor.read_with(cx, |editor, _| editor.is_externally_changed()));

        cx.executor().advance_clock(AUTO_SAVE_DELAY * 2);
        cx.run_until_parked();
        cx.deactivate_window();
        cx.run_until_parked();
        assert_eq!(
            on_disk(&root, "a.txt"),
            "theirs, longer\n",
            "外で変わった警告が出ているタブは、どのきっかけでも上書きしない"
        );
        assert!(editor.read_with(cx, |editor, _| editor.buffer().is_dirty()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test(iterations = 10)]
    fn a_save_asked_while_writing_waits_for_the_first(cx: &mut gpui::TestAppContext) {
        // 手を止めた時の保存と、窓を離れた時の保存（や ⌘S の連打）が重なる。2 本同時に書くと、後の方が
        // 読み込み時の revision で書こうとして断られ、未保存のまま残っていた。
        let (workspace, cx, root) = auto_save_workspace("twice", "off", &["a.txt"], cx);
        let editor = editor_for(&workspace, "a.txt", cx);
        editor.update(cx, |editor, cx| {
            editor.insert_text("x", cx);
            editor.save_now(cx);
            editor.insert_text("y", cx);
            editor.save_now(cx);
        });
        cx.run_until_parked();
        let text = editor.read_with(cx, |editor, _| editor.buffer().text());
        assert_eq!(text, "xyone\n");
        assert_eq!(on_disk(&root, "a.txt"), text, "後から頼んだ分も書かれる");
        assert!(!editor.read_with(cx, |editor, _| editor.buffer().is_dirty()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn off_never_writes(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = auto_save_workspace("off", "off", &["a.txt", "b.txt"], cx);
        let a = editor_for(&workspace, "a.txt", cx);
        let b = editor_for(&workspace, "b.txt", cx);
        b.update(cx, |editor, cx| editor.insert_text("b ", cx));
        cx.executor().advance_clock(AUTO_SAVE_DELAY * 3);
        cx.update(|window, cx| {
            let handle = a.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        cx.deactivate_window();
        cx.run_until_parked();
        assert_eq!(on_disk(&root, "b.txt"), "one\n");
        assert!(b.read_with(cx, |editor, _| editor.buffer().is_dirty()));
        std::fs::remove_dir_all(&root).ok();
    }
}
