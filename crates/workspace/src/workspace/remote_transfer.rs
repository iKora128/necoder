//! 接続先（SSH）の project とのファイルの受け渡し（O37・G09）。
//!
//! - **アップロード**: Finder からエクスプローラのフォルダ（行・余白）へ落とす、または右クリックの
//!   「このフォルダへアップロード…」でファイル / フォルダを選ぶ。送る先に同じ名前があれば断る
//!   （上書きしない・手元の project へ Finder から落とした時と同じ）。
//! - **ダウンロード**: 右クリックの「ダウンロード…」。保存ダイアログ（既定は ~/Downloads）で保存先と
//!   名前を決める。フォルダは中身ごと（保存先に同じ名前のフォルダがあれば断る）。
//!
//! 送受信は背景で回し（SSH の往復で UI を止めない）、始めた時と終わった時にトーストを出す。
//! ダウンロードの終わりのトーストは押すと Finder で見せる。実際の読み書きは `project::transfer`
//! （Host の read / write だけで組む）。手元の project では出さない（Finder からのドロップは今まで通り
//! 手元のコピー）。

use super::image_view::human_size;
use crate::workspace::*;
use project::transfer::{self, TransferSummary};

/// 保存ダイアログを開く場所（~/Downloads が無ければホーム）。
fn download_directory() -> PathBuf {
    let home = paths::home_dir().unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    if downloads.is_dir() {
        downloads
    } else {
        home
    }
}

/// パスの最後の名前（トーストに出す）。
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// 飛ばしたものがあれば文の後ろに足す（シンボリックリンクなど）。
fn with_skipped(text: String, summary: &TransferSummary) -> SharedString {
    if summary.skipped == 0 {
        return SharedString::from(text);
    }
    SharedString::from(format!(
        "{text}{}",
        i18n::t!("explorer.transfer_skipped", "n" => summary.skipped)
    ))
}

impl Workspace {
    /// 手元の `sources` を、接続先の project の `target_dir` の中へ送る（Finder からのドロップ・
    /// アップロードのダイアログ）。手元の project では何もしない。
    pub(crate) fn upload_paths(
        &mut self,
        sources: Vec<PathBuf>,
        target_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if !worktree.is_remote() || sources.is_empty() {
            return;
        }
        let host = worktree.host().clone();
        let host_id = host.id().to_string();
        let root = worktree.root().to_path_buf();
        let index = self.project_sessions.active;
        let text = match sources.as_slice() {
            [single] => i18n::t!("explorer.uploading_one", "name" => display_name(single)),
            _ => i18n::t!("explorer.uploading_many", "n" => sources.len()),
        };
        self.push_toast(SharedString::from(text), self.accent(), cx);
        cx.spawn(async move |workspace, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    sources
                        .iter()
                        .map(|source| transfer::upload_into_on(host.as_ref(), source, &target_dir))
                        .collect::<Vec<_>>()
                })
                .await;
            let finished = workspace.update(cx, |workspace, cx| {
                workspace.finish_upload(index, &host_id, &root, results, cx)
            });
            if let Err(error) = finished {
                eprintln!("アップロードの後始末ができない: {error:#}");
            }
        })
        .detach();
    }

    fn finish_upload(
        &mut self,
        index: usize,
        host_id: &str,
        root: &Path,
        results: Vec<anyhow::Result<(PathBuf, TransferSummary)>>,
        cx: &mut Context<Self>,
    ) {
        let mut destinations = Vec::new();
        let mut summaries = Vec::new();
        let mut first_error = None;
        for result in results {
            match result {
                Ok((destination, summary)) => {
                    destinations.push(destination);
                    summaries.push(summary);
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        // 送っている間に project を閉じた・入れ替えた時は、ツリーに触らない（知らせだけ出す）。
        let same_project = self
            .project_sessions
            .projects
            .get(index)
            .is_some_and(|slot| {
                slot.worktree.host().id() == host_id && slot.worktree.root() == root
            });
        if same_project {
            if let Some(destination) = destinations.last().cloned() {
                if let Some(slot) = self.project_sessions.slot_mut(index) {
                    slot.explorer.selected = Some(destination);
                }
                self.refresh_explorer_for(index, cx);
                self.refresh_git_status_for(index, cx);
            }
        }
        if !destinations.is_empty() {
            let total = transfer::total_of(&summaries);
            let text = i18n::t!(
                "explorer.uploaded",
                "n" => total.files,
                "size" => human_size(total.bytes as usize)
            );
            self.push_toast(with_skipped(text, &total), self.accent(), cx);
        }
        if let Some(error) = first_error {
            self.push_failure_toast(
                SharedString::from(
                    i18n::t!("explorer.upload_failed", "error" => format!("{error:#}")),
                ),
                None,
                cx,
            );
        }
        cx.notify();
    }

    /// 右クリックの「アップロード…」: 手元のファイル / フォルダを選んで `target_dir` へ送る。
    pub(crate) fn upload_via_dialog(&mut self, target_dir: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(SharedString::from(i18n::t!("explorer.upload_prompt"))),
        });
        cx.spawn(async move |workspace, cx| {
            let sources = match receiver.await {
                Ok(Ok(Some(sources))) => sources,
                // 取り消した・ダイアログが閉じられた。
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    eprintln!("アップロードするファイルを選べない: {error:#}");
                    return;
                }
            };
            let started = workspace.update(cx, |workspace, cx| {
                workspace.upload_paths(sources, target_dir, cx)
            });
            if let Err(error) = started {
                eprintln!("アップロードを始められない: {error:#}");
            }
        })
        .detach();
    }

    /// 右クリックの「ダウンロード…」: 接続先の `source`（ファイル / フォルダ）を、保存ダイアログで
    /// 選んだ手元の場所へ保存する。手元の project では何もしない。
    pub(crate) fn download_entry(&mut self, source: PathBuf, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if !worktree.is_remote() {
            return;
        }
        let host = worktree.host().clone();
        let name = display_name(&source);
        let receiver = cx.prompt_for_new_path(&download_directory(), Some(&name));
        cx.spawn(async move |workspace, cx| {
            let target = match receiver.await {
                Ok(Ok(Some(target))) => target,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    eprintln!("保存先を選べない: {error:#}");
                    return;
                }
            };
            let started = workspace.update(cx, |workspace, cx| {
                workspace.start_download(host, source, target, cx)
            });
            if let Err(error) = started {
                eprintln!("ダウンロードを始められない: {error:#}");
            }
        })
        .detach();
    }

    fn start_download(
        &mut self,
        host: Arc<dyn Host>,
        source: PathBuf,
        target: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.push_toast(
            SharedString::from(i18n::t!("explorer.downloading", "name" => display_name(&source))),
            self.accent(),
            cx,
        );
        cx.spawn(async move |workspace, cx| {
            let saved = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move { transfer::download_to_on(host.as_ref(), &source, &saved) })
                .await;
            let finished = workspace.update(cx, |workspace, cx| {
                workspace.finish_download(target, result, cx)
            });
            if let Err(error) = finished {
                eprintln!("ダウンロードの後始末ができない: {error:#}");
            }
        })
        .detach();
    }

    fn finish_download(
        &mut self,
        target: PathBuf,
        result: anyhow::Result<TransferSummary>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(summary) => {
                let text = i18n::t!(
                    "explorer.downloaded",
                    "name" => display_name(&target),
                    "n" => summary.files,
                    "size" => human_size(summary.bytes as usize)
                );
                self.push_toast_revealing(with_skipped(text, &summary), target, cx);
            }
            Err(error) => self.push_failure_toast(
                SharedString::from(
                    i18n::t!("explorer.download_failed", "error" => format!("{error:#}")),
                ),
                None,
                cx,
            ),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::notifications::ToastAction;
    use crate::workspace::tests::RenderAuditHost;

    #[gpui::test]
    fn files_go_up_by_drop_and_come_down_by_the_save_dialog(cx: &mut gpui::TestAppContext) {
        let base =
            std::env::temp_dir().join(format!("necoder_remote_transfer_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("remote/src")).unwrap();
        std::fs::create_dir_all(base.join("local")).unwrap();
        let base = paths::canonicalize(&base).unwrap();
        let remote_root = base.join("remote");
        let local = base.join("local");
        std::fs::write(remote_root.join("src/app.log"), "from the server\n").unwrap();
        std::fs::write(local.join("notes.txt"), "from this machine\n").unwrap();
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let host: Arc<dyn Host> = Arc::new(RenderAuditHost::new(true));
        let sources = vec![ProjectSource::new(host, remote_root.clone())];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        cx.run_until_parked();

        // Finder から src へ落とす = アップロード。同じ名前をもう一度落としても上書きしない。
        workspace.update(cx, |workspace, cx| {
            workspace.upload_paths(vec![local.join("notes.txt")], remote_root.join("src"), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(remote_root.join("src/notes.txt")).unwrap(),
            "from this machine\n"
        );
        workspace.update(cx, |workspace, _cx| {
            let slot = workspace.active_slot().expect("接続先の project");
            assert_eq!(
                slot.explorer.selected.as_deref(),
                Some(remote_root.join("src/notes.txt").as_path()),
                "送ったものを選ぶ"
            );
        });
        std::fs::write(local.join("notes.txt"), "changed\n").unwrap();
        workspace.update(cx, |workspace, cx| {
            workspace.upload_paths(vec![local.join("notes.txt")], remote_root.join("src"), cx)
        });
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(remote_root.join("src/notes.txt")).unwrap(),
            "from this machine\n",
            "上書きしない"
        );
        let failure = workspace.read_with(cx, |workspace, _| {
            workspace
                .notifications
                .toasts
                .last()
                .map(|toast| toast.text.to_string())
                .unwrap_or_default()
        });
        assert!(
            failure.contains(&i18n::t!("explorer.upload_failed", "error" => "")),
            "断ったことを知らせる: {failure}"
        );

        // 右クリックの「ダウンロード…」→ 保存ダイアログ → 手元へ。終わりの知らせは押すと Finder。
        workspace.update(cx, |workspace, cx| {
            workspace.download_entry(remote_root.join("src/app.log"), cx)
        });
        cx.run_until_parked();
        assert!(cx.did_prompt_for_new_path(), "保存先を聞く");
        cx.simulate_new_path_selection(|_directory| Some(local.join("app.log")));
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(local.join("app.log")).unwrap(),
            "from the server\n"
        );
        workspace.read_with(cx, |workspace, _| {
            let toast = workspace.notifications.toasts.last().expect("知らせ");
            assert!(
                matches!(&toast.action, Some(ToastAction::Reveal { path }) if *path == local.join("app.log")),
                "押すと保存したものを見せる"
            );
        });

        workspace.update(cx, |workspace, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        let _ = std::fs::remove_dir_all(&base);
    }
}
