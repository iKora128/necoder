//! プレビュータブ（O26・D02）。
//!
//! エクスプローラで 1 回クリックしたファイルは**プレビュータブ**で開く（名前が斜体）。次に 1 回
//! クリックしたファイルがこのタブを置き換えるので、ファイルを眺めて回ってもタブが溜まらない。
//! 普通のタブになる（プレビューが外れる）のは: 編集した・タブをダブルクリックした・エクスプローラで
//! ダブルクリックした・⌘P などで普通に開き直した・ピン留めした。設定 `preview_tabs`（既定 on）で切れる。
//! プレビューは窓セッションに残さない（再起動で戻したタブは普通のタブ）。

use crate::workspace::*;

impl Workspace {
    /// エクスプローラの 1 回クリック: プレビュータブで開く（設定が off なら普通に開く）。
    pub(crate) fn open_file_preview(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = cx
            .try_global::<settings::SettingsGlobal>()
            .map(|global| global.settings().preview_tabs)
            .unwrap_or(true);
        self.open_file_as(path, enabled, window, cx);
    }

    /// 読み終えて足したタブ（`path`）をプレビューにし、前のプレビューがあればその位置で置き換える。
    pub(crate) fn settle_preview_tab(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(new_index) = self.tabs.iter().position(|tab| tab.path == path) else {
            return;
        };
        self.tabs[new_index].preview = true;
        let previous = self
            .tabs
            .iter()
            .position(|tab| tab.preview && tab.path != path);
        if let Some(previous) = previous {
            if self.tabs[previous].is_dirty(cx) {
                // 編集で外れているはずだが、念のため: 未保存の編集があるタブは閉じずに残す。
                self.tabs[previous].preview = false;
            } else if previous < new_index {
                // 前のプレビューの場所へ動かしてから前の方を閉じる（タブ列の同じ位置に出る）。
                self.reorder_tab(new_index, previous);
                self.close_tab_at(previous + 1, window, cx);
            } else {
                self.close_tab_at(previous, window, cx);
            }
        }
        self.sync_active_slot();
        self.save_state(cx);
        cx.notify();
    }

    /// `index` 番目のタブを普通のタブにする（プレビューを外す）。
    pub(crate) fn keep_preview_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(index).filter(|tab| tab.preview) {
            tab.preview = false;
            cx.notify();
        }
    }

    /// 編集が始まったタブのプレビューを外す（`on_editor_changed` から・未保存になった時）。
    pub(crate) fn keep_edited_preview(
        &mut self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.preview && tab.editor() == Some(editor))
        else {
            return;
        };
        if editor.read(cx).buffer().is_dirty() {
            self.keep_preview_tab(index, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with_files<'a>(
        name: &str,
        files: &[&str],
        preview_tabs: bool,
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("necoder_preview_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("作業フォルダを作れる");
        for file in files {
            std::fs::write(project.join(file), format!("{file}\n")).expect("ファイルを書ける");
        }
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            format!(r#"{{"onboarded":true,"agent_prewarm":false,"preview_tabs":{preview_tabs}}}"#),
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, _window, _cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        (workspace, cx, root)
    }

    /// タブ列を「名前（プレビューは ~ 付き）」で。
    fn tabs(workspace: &Entity<Workspace>, cx: &mut gpui::VisualTestContext) -> Vec<String> {
        workspace.read_with(cx, |workspace, _| {
            workspace
                .tabs
                .iter()
                .map(|tab| {
                    let name = tab
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if tab.preview {
                        format!("~{name}")
                    } else {
                        name
                    }
                })
                .collect()
        })
    }

    fn click(
        workspace: &Entity<Workspace>,
        path: PathBuf,
        preview: bool,
        cx: &mut gpui::VisualTestContext,
    ) {
        workspace.update_in(cx, |workspace, window, cx| {
            if preview {
                workspace.open_file_preview(path, window, cx);
            } else {
                workspace.open_file(path, window, cx);
            }
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn single_clicks_reuse_one_preview_tab(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) =
            workspace_with_files("reuse", &["a.txt", "b.txt", "c.txt", "d.txt"], true, cx);
        let project = root.join("project");
        click(&workspace, project.join("a.txt"), false, cx);
        click(&workspace, project.join("b.txt"), true, cx);
        assert_eq!(tabs(&workspace, cx), ["a.txt", "~b.txt"]);
        click(&workspace, project.join("c.txt"), true, cx);
        assert_eq!(
            tabs(&workspace, cx),
            ["a.txt", "~c.txt"],
            "次の 1 回クリックがプレビューを置き換える"
        );

        // 編集したら普通のタブになり、次のプレビューは別に開く。
        let editor = workspace.read_with(cx, |workspace, _| {
            workspace.tabs[1].editor().cloned().expect("エディタ")
        });
        editor.update(cx, |editor, cx| editor.insert_text("x", cx));
        cx.run_until_parked();
        click(&workspace, project.join("d.txt"), true, cx);
        assert_eq!(tabs(&workspace, cx), ["a.txt", "c.txt", "~d.txt"]);

        // 普通に開き直す（⌘P・ダブルクリック）とプレビューが外れる。
        click(&workspace, project.join("d.txt"), false, cx);
        assert_eq!(tabs(&workspace, cx), ["a.txt", "c.txt", "d.txt"]);

        // もう一度 1 回クリックしただけでは、開いているタブの種類は変えない。
        click(&workspace, project.join("b.txt"), true, cx);
        click(&workspace, project.join("b.txt"), true, cx);
        assert_eq!(tabs(&workspace, cx), ["a.txt", "c.txt", "d.txt", "~b.txt"]);

        // ピン留めしても外れる。
        workspace.update_in(cx, |workspace, _window, cx| {
            let index = workspace
                .tabs
                .iter()
                .position(|tab| tab.path.ends_with("b.txt"))
                .expect("b.txt");
            workspace.toggle_tab_pin(index, cx);
        });
        assert_eq!(tabs(&workspace, cx), ["b.txt", "a.txt", "c.txt", "d.txt"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn a_double_click_while_loading_opens_a_normal_tab(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = workspace_with_files("double", &["a.txt"], true, cx);
        let path = root.join("project/a.txt");
        // 1 回目（プレビュー）の読み込みが終わる前に 2 回目（普通）が来る。
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_file_preview(path.clone(), window, cx);
            workspace.open_file(path.clone(), window, cx);
        });
        cx.run_until_parked();
        assert_eq!(tabs(&workspace, cx), ["a.txt"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn turning_preview_tabs_off_opens_normal_tabs(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = workspace_with_files("off", &["a.txt", "b.txt"], false, cx);
        let project = root.join("project");
        click(&workspace, project.join("a.txt"), true, cx);
        click(&workspace, project.join("b.txt"), true, cx);
        assert_eq!(tabs(&workspace, cx), ["a.txt", "b.txt"]);
        std::fs::remove_dir_all(&root).ok();
    }
}
