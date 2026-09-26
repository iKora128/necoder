//! 衝突の解決（O19・E05 の一部）。git がファイルに残した衝突の印（`<<<<<<<` 〜 `>>>>>>>`・diff3 の
//! `|||||||` 付きも）を、エディタの上の帯で 1 つずつ「今の側 / 入ってくる側 / 両方」に解決する
//! （VS Code のインラインの解決と同じ考え）。パレットにも同じ操作と「次の衝突へ」がある。
//!
//! 見るのはアクティブなエディタだけで、バッファの版が変わった時に数え直す（描画のたびには読まない・
//! 4 MB を超えるファイルは見ない）。操作の対象はキャレットのある衝突（無ければキャレットより後ろの
//! 最初の衝突・末尾を過ぎたら先頭）。解決は 1 回の編集＝⌘Z 1 回で戻る。
//!
//! マージ / リベースの中止（`git merge --abort` / `git rebase --abort`）はパレットから。どちらが進行中かを
//! 確かめてから、OS のダイアログで確認して流す（ソース管理パネルは O18 で作り直し中なので触らない）。

use crate::workspace::*;
use gpui::{PromptButton, PromptLevel};

/// これより大きいファイルは衝突を探さない（打鍵ごとに全文を読むので）。
const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;

/// ファイルの中の 1 つの衝突。範囲はバイト（改行込み）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConflictBlock {
    /// `<<<<<<<` の行頭から `>>>>>>>` の行の終わり（改行込み）まで。
    pub(crate) whole: Range<usize>,
    /// 今の側（`<<<<<<<` の次の行から、`|||||||` か `=======` の前まで）。
    pub(crate) ours: Range<usize>,
    /// 入ってくる側（`=======` の次の行から `>>>>>>>` の前まで）。
    pub(crate) theirs: Range<usize>,
    /// 印の後ろの名前（`HEAD`・ブランチ名など。無ければ空）。
    pub(crate) ours_label: String,
    pub(crate) theirs_label: String,
    /// `<<<<<<<` の行（0 始まり）。
    pub(crate) line: usize,
}

/// どちらで解決するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictSide {
    Ours,
    Theirs,
    Both,
}

/// 印の行か（`<<<<<<<` のように 7 つ並び、その後ろは空白か行末）。後ろの名前を返す。
fn marker<'a>(line: &'a str, character: char) -> Option<&'a str> {
    let line = line.trim_end_matches(['\n', '\r']);
    let rest = line.strip_prefix(&character.to_string().repeat(7))?;
    if rest.is_empty() {
        return Some("");
    }
    rest.strip_prefix([' ', '\t']).map(str::trim)
}

/// 衝突を全部拾う。閉じていない衝突（`>>>>>>>` が無い）は拾わない。
pub(crate) fn parse_conflicts(text: &str) -> Vec<ConflictBlock> {
    enum State {
        Outside,
        Ours {
            start: usize,
            line: usize,
            label: String,
            body: usize,
        },
        Base {
            start: usize,
            line: usize,
            label: String,
            ours: Range<usize>,
        },
        Theirs {
            start: usize,
            line: usize,
            label: String,
            ours: Range<usize>,
            body: usize,
        },
    }
    let mut blocks = Vec::new();
    let mut state = State::Outside;
    let mut offset = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let line_start = offset;
        let line_end = offset + line.len();
        offset = line_end;
        state = match state {
            State::Outside => match marker(line, '<') {
                Some(label) => State::Ours {
                    start: line_start,
                    line: index,
                    label: label.to_string(),
                    body: line_end,
                },
                None => State::Outside,
            },
            State::Ours {
                start,
                line: first,
                label,
                body,
            } => {
                if marker(line, '|').is_some() {
                    State::Base {
                        start,
                        line: first,
                        label,
                        ours: body..line_start,
                    }
                } else if marker(line, '=').is_some() {
                    State::Theirs {
                        start,
                        line: first,
                        label,
                        ours: body..line_start,
                        body: line_end,
                    }
                } else if let Some(label) = marker(line, '<') {
                    // 閉じる前に次の衝突が始まった: 前のは捨てて、ここから数え直す。
                    State::Ours {
                        start: line_start,
                        line: index,
                        label: label.to_string(),
                        body: line_end,
                    }
                } else {
                    State::Ours {
                        start,
                        line: first,
                        label,
                        body,
                    }
                }
            }
            State::Base {
                start,
                line: first,
                label,
                ours,
            } => {
                if marker(line, '=').is_some() {
                    State::Theirs {
                        start,
                        line: first,
                        label,
                        ours,
                        body: line_end,
                    }
                } else {
                    State::Base {
                        start,
                        line: first,
                        label,
                        ours,
                    }
                }
            }
            State::Theirs {
                start,
                line: first,
                label,
                ours,
                body,
            } => {
                if let Some(theirs_label) = marker(line, '>') {
                    blocks.push(ConflictBlock {
                        whole: start..line_end,
                        ours,
                        theirs: body..line_start,
                        ours_label: label,
                        theirs_label: theirs_label.to_string(),
                        line: first,
                    });
                    State::Outside
                } else {
                    State::Theirs {
                        start,
                        line: first,
                        label,
                        ours,
                        body,
                    }
                }
            }
        };
    }
    blocks
}

/// その衝突を `side` で解決した後の文（`whole` と置き換える）。
pub(crate) fn resolution(text: &str, block: &ConflictBlock, side: ConflictSide) -> String {
    let ours = &text[block.ours.clone()];
    let theirs = &text[block.theirs.clone()];
    match side {
        ConflictSide::Ours => ours.to_string(),
        ConflictSide::Theirs => theirs.to_string(),
        ConflictSide::Both => format!("{ours}{theirs}"),
    }
}

/// キャレットで選ぶ衝突: キャレットを含む物、無ければ後ろで最初の物、それも無ければ先頭。
pub(crate) fn conflict_at(blocks: &[ConflictBlock], caret: usize) -> Option<usize> {
    if blocks.is_empty() {
        return None;
    }
    blocks
        .iter()
        .position(|block| block.whole.contains(&caret) || block.whole.start >= caret)
        .or(Some(0))
}

/// アクティブなエディタの衝突（版ごとに覚える）。
pub(crate) struct ConflictCache {
    pub(crate) editor: gpui::EntityId,
    pub(crate) version: u64,
    pub(crate) blocks: Rc<[ConflictBlock]>,
}

/// 進行中の git の操作（中止の対象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InProgress {
    Merge,
    Rebase,
}

impl Workspace {
    /// アクティブなエディタの衝突を数え直す（版が変わった時だけ・`on_editor_changed` から）。
    pub(crate) fn refresh_conflicts(
        &mut self,
        editor: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) {
        if self.active_editor().as_ref() != Some(editor) {
            return;
        }
        let id = editor.entity_id();
        let view = editor.read(cx);
        let version = view.buffer().version();
        if self
            .chrome
            .conflicts
            .as_ref()
            .is_some_and(|cache| cache.editor == id && cache.version == version)
        {
            return;
        }
        let blocks = if view.buffer().len_bytes() > MAX_SCAN_BYTES {
            Vec::new()
        } else {
            let text = view.buffer().text();
            if text.contains("<<<<<<<") {
                parse_conflicts(&text)
            } else {
                Vec::new()
            }
        };
        self.chrome.conflicts = Some(ConflictCache {
            editor: id,
            version,
            blocks: blocks.into(),
        });
    }

    /// アクティブなエディタとその衝突（無い・数え直し前なら `None`）。
    fn active_conflicts(&self) -> Option<(Entity<EditorView>, Rc<[ConflictBlock]>)> {
        let editor = self.active_editor()?;
        let cache = self.chrome.conflicts.as_ref()?;
        (cache.editor == editor.entity_id() && !cache.blocks.is_empty())
            .then(|| (editor, cache.blocks.clone()))
    }

    /// キャレットのある衝突（無ければ後ろ・先頭）を `side` で解決する（帯のボタン・パレット）。
    pub(crate) fn resolve_conflict(&mut self, side: ConflictSide, cx: &mut Context<Self>) {
        let Some((editor, blocks)) = self.active_conflicts() else {
            let color = self.accent();
            self.push_toast(i18n::t!("conflict.none").into(), color, cx);
            return;
        };
        let (range, text) = {
            let view = editor.read(cx);
            let caret = view
                .buffer()
                .selections()
                .first()
                .map_or(0, |selection| selection.head);
            let Some(index) = conflict_at(&blocks, caret) else {
                return;
            };
            let block = &blocks[index];
            (
                block.whole.clone(),
                resolution(&view.buffer().text(), block, side),
            )
        };
        editor.update(cx, |view, cx| {
            view.replace_ranges(&[range.clone()], &text, cx);
            view.select_byte_range(range.start..range.start, cx);
        });
        self.refresh_conflicts(&editor, cx);
        cx.notify();
    }

    /// 次の衝突へ（キャレットより後ろ・末尾を過ぎたら先頭）。
    pub(crate) fn next_conflict(&mut self, cx: &mut Context<Self>) {
        let Some((editor, blocks)) = self.active_conflicts() else {
            let color = self.accent();
            self.push_toast(i18n::t!("conflict.none").into(), color, cx);
            return;
        };
        let caret = editor
            .read(cx)
            .buffer()
            .selections()
            .first()
            .map_or(0, |selection| selection.head);
        let target = blocks
            .iter()
            .find(|block| block.whole.start > caret)
            .or_else(|| blocks.first())
            .map(|block| block.whole.start);
        if let Some(start) = target {
            editor.update(cx, |view, cx| view.select_byte_range(start..start, cx));
        }
    }

    /// パレット「Git: マージ / リベースを中止」: 進行中の方を確かめ、確認してから中止する。
    pub(crate) fn abort_merge_or_rebase(
        &mut self,
        _: &AbortMergeOrRebase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        let host = worktree.host().clone();
        let root = worktree.root().to_path_buf();
        cx.spawn_in(window, async move |workspace, cx| {
            let found = cx
                .background_executor()
                .spawn({
                    let host = host.clone();
                    let root = root.clone();
                    async move { in_progress(host.as_ref(), &root) }
                })
                .await;
            let Some(kind) = found else {
                workspace
                    .update(cx, |workspace, cx| {
                        let color = workspace.accent();
                        workspace.push_toast(
                            i18n::t!("conflict.nothing_to_abort").into(),
                            color,
                            cx,
                        );
                    })
                    .ok();
                return;
            };
            let (message, detail, button) = match kind {
                InProgress::Merge => (
                    i18n::t!("conflict.abort_merge_title"),
                    i18n::t!("conflict.abort_merge_detail"),
                    i18n::t!("conflict.abort_merge_confirm"),
                ),
                InProgress::Rebase => (
                    i18n::t!("conflict.abort_rebase_title"),
                    i18n::t!("conflict.abort_rebase_detail"),
                    i18n::t!("conflict.abort_rebase_confirm"),
                ),
            };
            let Ok(answer) = workspace.update_in(cx, |_, window, cx| {
                window.prompt(
                    PromptLevel::Warning,
                    &message,
                    Some(&detail),
                    &[
                        PromptButton::ok(button),
                        PromptButton::cancel(i18n::t!("conflict.abort_cancel")),
                    ],
                    cx,
                )
            }) else {
                return;
            };
            if answer.await != Ok(0) {
                return;
            }
            let aborted = cx
                .background_executor()
                .spawn(async move { abort(host.as_ref(), &root, kind) })
                .await;
            workspace
                .update(cx, |workspace, cx| {
                    let color = workspace.accent();
                    match aborted {
                        Ok(()) => {
                            workspace.push_toast(i18n::t!("conflict.aborted").into(), color, cx);
                            workspace.refresh_git_status(cx);
                        }
                        Err(error) => workspace.push_failure_toast(
                            SharedString::from(format!("{error:#}")),
                            None,
                            cx,
                        ),
                    }
                })
                .ok();
        })
        .detach();
    }

    /// エディタの上の帯（衝突がある時だけ）: `⑂ 衝突 N か所（M 行目）` + 今の側 / 入ってくる側 / 両方 / 次へ。
    pub(crate) fn render_conflict_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (editor, blocks) = self.active_conflicts()?;
        let caret = editor
            .read(cx)
            .buffer()
            .selections()
            .first()
            .map_or(0, |selection| selection.head);
        let block = &blocks[conflict_at(&blocks, caret)?];
        let theme = self.theme.clone();
        let label = |side: &str, name: &str| {
            if name.is_empty() {
                side.to_string()
            } else {
                format!("{side}（{name}）")
            }
        };
        let ours = label(&i18n::t!("conflict.ours"), &block.ours_label);
        let theirs = label(&i18n::t!("conflict.theirs"), &block.theirs_label);
        let button = |id: &'static str, text: String| {
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
                .child(SharedString::from(text))
        };
        Some(
            div()
                .id("conflict-bar")
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
                .child(SharedString::from(i18n::t!(
                    "conflict.bar",
                    "count" => blocks.len(),
                    "line" => block.line + 1
                )))
                .child(div().flex_1())
                .child(button("conflict-ours", ours).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.resolve_conflict(ConflictSide::Ours, cx)
                    }),
                ))
                .child(button("conflict-theirs", theirs).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.resolve_conflict(ConflictSide::Theirs, cx)
                    }),
                ))
                .child(
                    button("conflict-both", i18n::t!("conflict.both")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            this.resolve_conflict(ConflictSide::Both, cx)
                        }),
                    ),
                )
                .child(
                    button("conflict-next", i18n::t!("conflict.next")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.next_conflict(cx)),
                    ),
                )
                .into_any_element(),
        )
    }
}

/// 進行中のマージ / リベース（どちらでもなければ `None`）。worktree でも正しい場所を git に聞く。
fn in_progress(host: &dyn host::Host, root: &Path) -> Option<InProgress> {
    let git = |args: &[&str]| {
        host.run_command(&host::CommandSpec::new("git", root).args(args.iter().copied()))
            .ok()
            .filter(|output| output.status_code == Some(0))
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    if git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_some() {
        return Some(InProgress::Merge);
    }
    for directory in ["rebase-merge", "rebase-apply"] {
        let Some(path) = git(&["rev-parse", "--git-path", directory]) else {
            continue;
        };
        let path = root.join(path);
        if host.metadata(&path).is_ok() {
            return Some(InProgress::Rebase);
        }
    }
    None
}

fn abort(host: &dyn host::Host, root: &Path, kind: InProgress) -> anyhow::Result<()> {
    let args: &[&str] = match kind {
        InProgress::Merge => &["merge", "--abort"],
        InProgress::Rebase => &["rebase", "--abort"],
    };
    let output =
        host.run_command(&host::CommandSpec::new("git", root).args(args.iter().copied()))?;
    anyhow::ensure!(
        output.status_code == Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_WAY: &str = "fn a() {}\n<<<<<<< HEAD\nlet x = 1;\n=======\nlet x = 2;\nlet y = 3;\n>>>>>>> feature/x\nfn b() {}\n";

    #[test]
    fn conflicts_are_found_with_both_sides_and_labels() {
        let blocks = parse_conflicts(TWO_WAY);
        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];
        assert_eq!(&TWO_WAY[block.ours.clone()], "let x = 1;\n");
        assert_eq!(&TWO_WAY[block.theirs.clone()], "let x = 2;\nlet y = 3;\n");
        assert_eq!(block.ours_label, "HEAD");
        assert_eq!(block.theirs_label, "feature/x");
        assert_eq!(block.line, 1);
        assert!(TWO_WAY[block.whole.clone()].starts_with("<<<<<<<"));
        assert!(TWO_WAY[block.whole.clone()].ends_with("feature/x\n"));
    }

    #[test]
    fn each_side_replaces_the_whole_block() {
        let block = &parse_conflicts(TWO_WAY)[0];
        let apply = |side| {
            let mut text = TWO_WAY.to_string();
            text.replace_range(block.whole.clone(), &resolution(TWO_WAY, block, side));
            text
        };
        assert_eq!(
            apply(ConflictSide::Ours),
            "fn a() {}\nlet x = 1;\nfn b() {}\n"
        );
        assert_eq!(
            apply(ConflictSide::Theirs),
            "fn a() {}\nlet x = 2;\nlet y = 3;\nfn b() {}\n"
        );
        assert_eq!(
            apply(ConflictSide::Both),
            "fn a() {}\nlet x = 1;\nlet x = 2;\nlet y = 3;\nfn b() {}\n"
        );
    }

    /// diff3（`|||||||` の元の版）は今の側に入れない。CRLF・終わりの改行無し・閉じていない印・
    /// 印に似た行（8 つ並び・途中の `=======`）も読み違えない。
    #[test]
    fn diff3_crlf_and_near_markers_are_read_right() {
        let diff3 = "<<<<<<< ours\r\na\r\n||||||| base\r\nold\r\n=======\r\nb\r\n>>>>>>> theirs";
        let blocks = parse_conflicts(diff3);
        assert_eq!(blocks.len(), 1);
        assert_eq!(&diff3[blocks[0].ours.clone()], "a\r\n");
        assert_eq!(&diff3[blocks[0].theirs.clone()], "b\r\n");
        assert_eq!(blocks[0].whole.end, diff3.len());

        assert!(
            parse_conflicts("<<<<<<< HEAD\nx\n=======\ny\n").is_empty(),
            "閉じていない"
        );
        assert!(parse_conflicts("<<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n").is_empty());
        assert!(parse_conflicts("a\n=======\nb\n").is_empty());
        // 閉じる前に次の衝突が始まったら、前のは捨てる。
        let restarted = "<<<<<<< a\nx\n<<<<<<< b\ny\n=======\nz\n>>>>>>> c\n";
        let blocks = parse_conflicts(restarted);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].ours_label, "b");
    }

    /// エディタの上の帯: 開いたファイルの衝突を数え、キャレットの衝突から 1 つずつ解決する。
    /// 解決は 1 回の編集。全部片付いたら帯は消え、操作は「衝突はありません」と知らせる。
    #[gpui::test]
    fn conflicts_in_the_active_file_are_resolved_one_at_a_time(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("necoder_conflicts_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        let file = root.join("merge.rs");
        std::fs::write(&file, format!("{TWO_WAY}{TWO_WAY}")).unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.open_file_sync(file.clone(), window, cx);
            let editor = workspace.active_editor().expect("エディタ");
            workspace.refresh_conflicts(&editor, cx);
            assert_eq!(workspace.active_conflicts().map(|(_, blocks)| blocks.len()), Some(2));
            assert!(workspace.render_conflict_bar(cx).is_some(), "帯が出る");

            workspace.resolve_conflict(ConflictSide::Ours, cx);
            assert_eq!(workspace.active_conflicts().map(|(_, blocks)| blocks.len()), Some(1));
            workspace.next_conflict(cx);
            workspace.resolve_conflict(ConflictSide::Both, cx);
            assert_eq!(
                editor.read(cx).plain_text(),
                "fn a() {}\nlet x = 1;\nfn b() {}\nfn a() {}\nlet x = 1;\nlet x = 2;\nlet y = 3;\nfn b() {}\n"
            );
            assert!(workspace.active_conflicts().is_none());
            assert!(workspace.render_conflict_bar(cx).is_none(), "片付いたら帯は消える");
            workspace.resolve_conflict(ConflictSide::Ours, cx);
            assert!(workspace
                .notifications
                .toasts
                .iter()
                .any(|toast| toast.text.as_ref() == i18n::t!("conflict.none")));
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 進行中のマージを見つけて中止する（本物の git で衝突を作る）。git が無い環境では飛ばす。
    #[test]
    fn a_merge_in_progress_is_found_and_aborted() {
        let repo =
            std::env::temp_dir().join(format!("necoder_conflicts_git_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=necoder",
                    "-c",
                    "user.email=necoder@example.com",
                ])
                .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
                .args(args)
                .current_dir(&repo)
                .output()
        };
        let Ok(init) = git(&["init", "-q", "-b", "main"]) else {
            return;
        };
        if !init.status.success() {
            return;
        }
        let file = repo.join("a.txt");
        std::fs::write(&file, "base\n").unwrap();
        git(&["add", "a.txt"]).unwrap();
        git(&["commit", "-q", "-m", "base"]).unwrap();
        git(&["checkout", "-q", "-b", "other"]).unwrap();
        std::fs::write(&file, "other\n").unwrap();
        git(&["commit", "-q", "-am", "other"]).unwrap();
        git(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(&file, "main\n").unwrap();
        git(&["commit", "-q", "-am", "main"]).unwrap();
        let merged = git(&["merge", "--no-edit", "other"]).unwrap();
        assert!(!merged.status.success(), "衝突する");
        let host = host::LocalHost::shared();
        assert_eq!(in_progress(host.as_ref(), &repo), Some(InProgress::Merge));
        assert_eq!(
            parse_conflicts(&std::fs::read_to_string(&file).unwrap()).len(),
            1,
            "git の印を読める"
        );
        abort(host.as_ref(), &repo, InProgress::Merge).unwrap();
        assert_eq!(in_progress(host.as_ref(), &repo), None);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "main\n");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn the_caret_picks_the_conflict_it_is_in_or_the_next() {
        let text = format!("{TWO_WAY}{TWO_WAY}");
        let blocks = parse_conflicts(&text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(conflict_at(&blocks, 0), Some(0), "前 → 最初");
        assert_eq!(
            conflict_at(&blocks, blocks[0].whole.start + 3),
            Some(0),
            "中"
        );
        assert_eq!(
            conflict_at(&blocks, blocks[0].whole.end + 1),
            Some(1),
            "間 → 次"
        );
        assert_eq!(
            conflict_at(&blocks, text.len()),
            Some(0),
            "最後の後ろ → 先頭"
        );
        assert_eq!(conflict_at(&[], 0), None);
    }
}
