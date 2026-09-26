//! タブのピン留め（O26・D28）。
//!
//! ピン留めしたタブは常にタブ列の左端にまとまる（`tabs[..pinned_tab_count()]`）。× の代わりにピンを
//! 出し、押すと外す。まとめて閉じる操作（他のタブを閉じる・右側を閉じる・全部閉じる）と ⌘W では
//! 閉じない（VS Code・Zed の既定と同じ。閉じるなら先に外す・タブメニューの「タブを閉じる」は閉じる）。
//! 窓セッションの `pinned_files` に残り、再起動とレールの切り替えを越える。
//!
//! 一時タブ（diff・変更レビュー）はピン留めしない（窓セッションに残らないので、留めても次に消える）。

use crate::workspace::*;

impl Workspace {
    /// ピン留めの数。左端からこの数だけがピン留め。
    pub(crate) fn pinned_tab_count(&self) -> usize {
        self.tabs.iter().take_while(|tab| tab.pinned).count()
    }

    /// ⌘K ⇧⏎ / パレット: アクティブなタブのピン留めを付ける / 外す。
    pub(crate) fn toggle_pin_active_tab(
        &mut self,
        _: &TogglePinTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_tab_pin(self.active_tab, cx);
    }

    /// `index` 番目のタブのピン留めを付ける / 外す。付けたらピン留めの末尾へ、外したらピン留めで
    /// ないタブの先頭へ動かす（どちらも左端のまとまりを崩さない）。
    pub(crate) fn toggle_tab_pin(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        if tab.transient {
            return;
        }
        let pinned_count = self.pinned_tab_count();
        let pin = !tab.pinned;
        self.tabs[index].pinned = pin;
        let target = if pin {
            pinned_count
        } else {
            pinned_count.saturating_sub(1)
        };
        self.reorder_tab(index, target);
        self.sync_active_slot();
        self.save_state(cx);
        cx.notify();
    }

    /// タブを `from` から `to` へ動かす（ピン留めの区切りは見ない・呼び手が守る）。アクティブは
    /// 同じタブを指し続ける。
    pub(crate) fn reorder_tab(&mut self, from: usize, to: usize) {
        let count = self.tabs.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        // active が指すタブを追従させる（remove→insert のインデックスずれを補正）。
        self.active_tab = if self.active_tab == from {
            to
        } else {
            let mut active = self.active_tab;
            if from < active {
                active -= 1;
            }
            if to <= active {
                active += 1;
            }
            active
        };
    }

    /// ドラッグで動かせる先へ寄せる: ピン留めはピン留めの中、そうでないタブはその外だけ。
    pub(crate) fn clamp_tab_move(&self, from: usize, to: usize) -> usize {
        let pinned_count = self.pinned_tab_count();
        match self.tabs.get(from) {
            Some(tab) if tab.pinned => to.min(pinned_count.saturating_sub(1)),
            Some(_) => to.max(pinned_count),
            None => to,
        }
    }

    /// 復元したタブ列にピン留めを戻す（`pinned` に載っているパスのタブ）。ピン留めを左端へ寄せ直し、
    /// アクティブは同じタブのまま。
    pub(crate) fn restore_tab_pins(&mut self, pinned: &[PathBuf]) {
        if pinned.is_empty() {
            return;
        }
        let active_path = self.tabs.get(self.active_tab).map(|tab| tab.path.clone());
        for tab in self.tabs.iter_mut() {
            tab.pinned = !tab.transient && pinned.contains(&tab.path);
        }
        // 安定な分け方（ピン留めを前へ・それぞれの中の順は保つ）。
        let tabs = std::mem::take(&mut self.tabs);
        let (mut front, back): (Vec<EditorTab>, Vec<EditorTab>) =
            tabs.into_iter().partition(|tab| tab.pinned);
        front.extend(back);
        self.tabs = front;
        if let Some(index) =
            active_path.and_then(|path| self.tabs.iter().position(|tab| tab.path == path))
        {
            self.active_tab = index;
        }
        self.sync_active_slot();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_with_tabs<'a>(
        name: &str,
        files: &[&str],
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
        let root = std::env::temp_dir().join(format!("necoder_pins_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("作業フォルダを作れる");
        for file in files {
            std::fs::write(project.join(file), format!("{file}\n")).expect("ファイルを書ける");
        }
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
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
        cx.run_until_parked();
        (workspace, cx, root)
    }

    fn names(workspace: &Workspace) -> Vec<String> {
        workspace
            .tabs
            .iter()
            .map(|tab| {
                let name = tab
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                if tab.pinned {
                    format!("📌{name}")
                } else {
                    name
                }
            })
            .collect()
    }

    fn index_of(workspace: &Workspace, name: &str) -> usize {
        workspace
            .tabs
            .iter()
            .position(|tab| tab.path.ends_with(name))
            .expect("そのタブ")
    }

    #[gpui::test]
    fn pinned_tabs_stay_left_and_survive_bulk_closes(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) =
            workspace_with_tabs("bulk", &["a.txt", "b.txt", "c.txt", "d.txt"], cx);
        workspace.update_in(cx, |workspace, window, cx| {
            let c = index_of(workspace, "c.txt");
            workspace.toggle_tab_pin(c, cx);
            let a = index_of(workspace, "a.txt");
            workspace.toggle_tab_pin(a, cx);
            assert_eq!(
                names(workspace),
                ["📌c.txt", "📌a.txt", "b.txt", "d.txt"],
                "留めた順にピン留めの末尾へ並ぶ"
            );
            assert_eq!(
                workspace.active_slot().map(|slot| slot.pinned_files.len()),
                Some(2),
                "窓セッションへ渡す slot にも載る"
            );

            // ドラッグでピン留めの中へは入れない / ピン留めは外へ出ない。
            let d = index_of(workspace, "d.txt");
            let to = workspace.clamp_tab_move(d, 0);
            workspace.move_tab(d, to, cx);
            assert_eq!(names(workspace), ["📌c.txt", "📌a.txt", "d.txt", "b.txt"]);
            let to = workspace.clamp_tab_move(0, 3);
            workspace.move_tab(0, to, cx);
            assert_eq!(names(workspace), ["📌a.txt", "📌c.txt", "d.txt", "b.txt"]);

            // 他を閉じる・右側を閉じる・全部閉じるはピン留めを閉じない。
            let d = index_of(workspace, "d.txt");
            workspace.close_other_tabs(d, window, cx);
            assert_eq!(names(workspace), ["📌a.txt", "📌c.txt", "d.txt"]);
            workspace.close_tabs_to_right(0, window, cx);
            assert_eq!(names(workspace), ["📌a.txt", "📌c.txt"]);
            workspace.close_saved_tabs(window, cx);
            assert_eq!(names(workspace), ["📌a.txt", "📌c.txt"]);

            // ⌘W でも閉じない（知らせる）。外せば閉じる。
            workspace.select_tab(0, window, cx);
            workspace.close_active_editor(window, cx);
            assert_eq!(names(workspace), ["📌a.txt", "📌c.txt"]);
            let toast = workspace
                .notifications
                .toasts
                .last()
                .map(|toast| toast.text.to_string())
                .unwrap_or_default();
            assert!(!toast.is_empty(), "黙って何もしないのではなく知らせる");
            workspace.toggle_tab_pin(0, cx);
            assert_eq!(names(workspace), ["📌c.txt", "a.txt"]);
            let a = index_of(workspace, "a.txt");
            workspace.select_tab(a, window, cx);
            workspace.close_active_editor(window, cx);
            assert_eq!(names(workspace), ["📌c.txt"]);
        });
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn pins_come_back_with_the_restored_tabs(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = workspace_with_tabs("restore", &["a.txt", "b.txt"], cx);
        let project = root.join("project");
        workspace.update_in(cx, |workspace, window, cx| {
            let b = index_of(workspace, "b.txt");
            workspace.toggle_tab_pin(b, cx);
            let state = workspace.persisted_state();
            assert_eq!(state.projects[0].pinned_files, vec![project.join("b.txt")]);
            // 同じ窓のままタブ列を開き直す（レールの切り替え・再起動の復元と同じ経路）。
            workspace.restore_open_file(
                &[RestoredTabs {
                    files: vec![project.join("a.txt"), project.join("b.txt")],
                    active: 0,
                    pinned: vec![project.join("b.txt")],
                }],
                window,
                cx,
            );
            assert_eq!(names(workspace), ["📌b.txt", "a.txt"]);
            assert!(workspace.tabs[workspace.active_tab].path.ends_with("a.txt"));
        });
        std::fs::remove_dir_all(&root).ok();
    }
}
