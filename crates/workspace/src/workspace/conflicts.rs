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
//!
//! 「並べて見る」（帯のボタン・パレット）は、キャレットの衝突の今の側 / 元（diff3 の印にあれば）/ 入ってくる
//! 側を横に並べた 1 枚を出す。列は読むだけで、ボタンは帯と同じ解決（1 回の編集）をして次の衝突を見せる。
//! 列の下の結果の欄は普通のエディタで、今の側から始めて直し、「結果で解決」（欄の中の ⌘⏎）でその文に
//! 置き換える（VS Code の merge editor の結果の欄と同じ考えを、衝突 1 つずつで）。

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
    /// 元（共通の祖先・diff3 の `|||||||` の次の行から `=======` の前まで）。2 つだけの印なら `None`。
    pub(crate) base: Option<Range<usize>>,
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
            base: usize,
        },
        Theirs {
            start: usize,
            line: usize,
            label: String,
            ours: Range<usize>,
            base: Option<Range<usize>>,
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
                        base: line_end,
                    }
                } else if marker(line, '=').is_some() {
                    State::Theirs {
                        start,
                        line: first,
                        label,
                        ours: body..line_start,
                        base: None,
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
                base,
            } => {
                if marker(line, '=').is_some() {
                    State::Theirs {
                        start,
                        line: first,
                        label,
                        ours,
                        base: Some(base..line_start),
                        body: line_end,
                    }
                } else {
                    State::Base {
                        start,
                        line: first,
                        label,
                        ours,
                        base,
                    }
                }
            }
            State::Theirs {
                start,
                line: first,
                label,
                ours,
                base,
                body,
            } => {
                if let Some(theirs_label) = marker(line, '>') {
                    blocks.push(ConflictBlock {
                        whole: start..line_end,
                        ours,
                        base,
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
                        base,
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

/// 「並べて見る」の 1 枚（開いている間だけ）。
pub(crate) struct ConflictView {
    focus: FocusHandle,
    /// 開いた時のエディタ（別のタブへ移ったら閉じる＝その衝突のための 1 枚なので）。
    editor: gpui::EntityId,
    /// 見せている衝突の写し。描画ごとに全文を読まないよう、開いた時・操作の後・バッファの版が
    /// 変わった時に取り直す。衝突が無くなれば `None`（その時は閉じる）。
    shown: Option<ShownConflict>,
    /// 結果の欄。見せる衝突が替わるたびに今の側で埋め直す（替わらなければ直した文を残す）。
    result: Entity<EditorView>,
}

/// 並べて見せる 1 つの衝突（写し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShownConflict {
    /// 何番目か（1 始まり）と全部の数。
    pub(crate) index: usize,
    pub(crate) count: usize,
    /// `<<<<<<<` の行（1 始まり）。
    pub(crate) line: usize,
    pub(crate) ours: String,
    pub(crate) base: Option<String>,
    pub(crate) theirs: String,
    pub(crate) ours_label: String,
    pub(crate) theirs_label: String,
}

/// 並べた 1 枚のボタン。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictViewAct {
    Resolve(ConflictSide),
    /// 結果の欄の文で解決する（欄の中の ⌘⏎ も）。
    ResolveWithResult,
    /// 結果の欄をその文で埋め直す。
    Fill(ResultSeed),
    Next,
}

/// 結果の欄を埋める元。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultSeed {
    Side(ConflictSide),
    /// 元（diff3 の印にある時だけ出す）。
    Base,
}

/// 結果の欄に入れる文（改行は `\n` に揃える。欄はエディタなので CRLF の `\r` を見せない）。
pub(crate) fn seed_text(shown: &ShownConflict, seed: ResultSeed) -> String {
    let text = match seed {
        ResultSeed::Side(ConflictSide::Ours) => shown.ours.clone(),
        ResultSeed::Side(ConflictSide::Theirs) => shown.theirs.clone(),
        ResultSeed::Side(ConflictSide::Both) => format!("{}{}", shown.ours, shown.theirs),
        ResultSeed::Base => shown.base.clone().unwrap_or_default(),
    };
    text.replace("\r\n", "\n")
}

/// 結果の欄の文を、衝突（印ごと）と置き換える文にする。空なら衝突ごと消す。そうでなければ最後の
/// 改行を保つ（無ければ足す＝次の行とつながらない）。CRLF のファイルには CRLF で書く。
pub(crate) fn result_for_file(result: &str, crlf: bool) -> String {
    if result.is_empty() {
        return String::new();
    }
    let mut text = result.replace("\r\n", "\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    text
}

/// そのファイルの改行が CRLF か（`<<<<<<<` の行の終わりで見る。git は印の行をファイルの改行で書く。
/// 衝突の中身には入ってくる側の CRLF の行が混ざりうるので、中身では決めない）。
pub(crate) fn marker_line_is_crlf(text: &str, block: &ConflictBlock) -> bool {
    text.get(block.whole.clone())
        .and_then(|whole| whole.split_once('\n'))
        .is_some_and(|(marker, _)| marker.ends_with('\r'))
}

/// 1 つの列に出す行の上限（これを超えた分は数だけ出す）。
const VIEW_MAX_LINES: usize = 400;

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
        self.refresh_conflict_view(cx);
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
        self.replace_conflict(conflict_at, |text, block| resolution(text, block, side), cx);
    }

    /// 並べた 1 枚が見せている衝突を置き換える（キャレットの衝突ではない。欄の中の F8 や ⌃G で
    /// 本体のキャレットが動いても、見ている物を解決する）。写しと食い違えば何もしない。
    fn replace_shown_conflict(
        &mut self,
        make: impl FnOnce(&str, &ConflictBlock) -> String,
        cx: &mut Context<Self>,
    ) {
        let Some(shown) = self
            .chrome
            .conflict_view
            .as_ref()
            .and_then(|view| view.shown.clone())
        else {
            return;
        };
        self.replace_conflict(
            |blocks, _caret| {
                let index = shown.index.checked_sub(1)?;
                blocks
                    .get(index)
                    .filter(|block| block.line + 1 == shown.line)
                    .map(|_| index)
            },
            make,
            cx,
        );
    }

    /// `pick`（衝突の並びとキャレット → 何番目か）で選んだ衝突を、印ごと `make` の文に置き換える
    /// （1 回の編集）。キャレットは置き換えた所へ。
    fn replace_conflict(
        &mut self,
        pick: impl FnOnce(&[ConflictBlock], usize) -> Option<usize>,
        make: impl FnOnce(&str, &ConflictBlock) -> String,
        cx: &mut Context<Self>,
    ) {
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
            let Some(index) = pick(&blocks, caret) else {
                return;
            };
            let block = &blocks[index];
            (block.whole.clone(), make(&view.buffer().text(), block))
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

    /// 並べた 1 枚の「次へ」: 見せている衝突の次（末尾を過ぎたら先頭）へキャレットを移す。キャレットより
    /// 後ろではなく**見せている物の次**（キャレットが最初の衝突より前にあっても、1 回で 2 つ目へ進む）。
    fn show_next_conflict(&mut self, cx: &mut Context<Self>) {
        let Some((editor, blocks)) = self.active_conflicts() else {
            return;
        };
        let Some(shown) = self
            .chrome
            .conflict_view
            .as_ref()
            .and_then(|view| view.shown.as_ref())
        else {
            return;
        };
        let next = shown.index % blocks.len();
        if let Some(start) = blocks.get(next).map(|block| block.whole.start) {
            editor.update(cx, |view, cx| view.select_byte_range(start..start, cx));
        }
    }

    /// 帯の「並べて見る」・パレット「マージ: 衝突を並べて見る」。
    pub(crate) fn show_conflict_side_by_side(
        &mut self,
        _: &ShowConflictSideBySide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shown) = self.conflict_snapshot(cx) else {
            let color = self.accent();
            self.push_toast(i18n::t!("conflict.none").into(), color, cx);
            return;
        };
        let theme = self.theme.clone();
        let accent = self.accent();
        let (font_size, tab_size) = {
            let current = settings::get(cx);
            (current.font_size, current.tab_size)
        };
        let seed = seed_text(&shown, ResultSeed::Side(ConflictSide::Ours));
        let result = cx.new(|cx| {
            let mut editor = EditorView::new(Buffer::new(), theme, accent, cx);
            editor.set_typography(font_size, tab_size, cx);
            editor.set_plain_text(&seed, cx);
            editor
        });
        let Some(editor) = self.active_editor().map(|editor| editor.entity_id()) else {
            return;
        };
        let focus = cx.focus_handle();
        window.focus(&result.read(cx).focus_handle(cx), cx);
        self.chrome.conflict_view = Some(ConflictView {
            focus,
            editor,
            shown: Some(shown),
            result,
        });
        cx.notify();
    }

    /// 開いた時のエディタがもう前に無い（別のタブを開いた・閉じた・プロジェクトを移った）か、見せる
    /// 衝突が無い 1 枚。描かず、次の描画の前に閉じる（[`Self::process_pending_shell_effects`]）。
    /// タブを移る経路はいくつもあり（選ぶ・新しく開く・閉じる・プロジェクトの切替）、全部で写しを
    /// 取り直すとは限らないので、見せる側で確かめる。
    pub(crate) fn conflict_view_is_stale(&self) -> bool {
        self.chrome.conflict_view.as_ref().is_some_and(|view| {
            view.shown.is_none()
                || self.active_editor().map(|editor| editor.entity_id()) != Some(view.editor)
        })
    }

    /// 閉じる（Esc・外側・閉じる・衝突が無くなった時）。フォーカスはエディタへ返す。
    pub(crate) fn close_conflict_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chrome.conflict_view.take().is_none() {
            return;
        }
        if let Some(editor) = self.active_editor() {
            let handle = editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 並べた 1 枚のボタン: 帯と同じ解決（または次へ）をして、次の衝突を見せる。無くなれば閉じる。
    pub(crate) fn conflict_view_act(
        &mut self,
        act: ConflictViewAct,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match act {
            ConflictViewAct::Resolve(side) => {
                self.replace_shown_conflict(|text, block| resolution(text, block, side), cx)
            }
            ConflictViewAct::ResolveWithResult => {
                let Some(result) = self
                    .chrome
                    .conflict_view
                    .as_ref()
                    .map(|view| view.result.read(cx).plain_text())
                else {
                    return;
                };
                self.replace_shown_conflict(
                    |text, block| result_for_file(&result, marker_line_is_crlf(text, block)),
                    cx,
                );
            }
            ConflictViewAct::Fill(seed) => {
                if let Some(view) = self.chrome.conflict_view.as_ref() {
                    if let Some(shown) = &view.shown {
                        let text = seed_text(shown, seed);
                        view.result
                            .update(cx, |editor, cx| editor.set_plain_text(&text, cx));
                    }
                }
            }
            ConflictViewAct::Next => self.show_next_conflict(cx),
        }
        self.refresh_conflict_view(cx);
        match self.chrome.conflict_view.as_ref() {
            // 押した後も打てるように、フォーカスは結果の欄へ戻す（ボタンを押すとカードに移るので）。
            Some(view) => {
                let focus = view.result.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
            // 衝突が無くなって閉じた: 次の描画を待たずにエディタへ。
            None => {
                if let Some(focus) = self.chrome.focus_next_frame.take() {
                    window.focus(&focus, cx);
                }
            }
        }
    }

    /// 開いていれば写しを取り直す（キャレットの衝突が替わった・バッファが変わった）。見せる衝突が
    /// 替わったら結果の欄を今の側で埋め直す（替わらなければ直しかけの文を残す）。
    ///
    /// 別のタブへ移った・衝突が無くなった（外で書き換わった）なら閉じる。ここは窓を持たないので、
    /// フォーカスは次の描画でエディタへ返す（閉じずに残すと、戻った時に欄を埋め直して直しかけを
    /// 失い、フォーカスは下のエディタに居て Esc も ⌘⏎ も届かなかった）。
    fn refresh_conflict_view(&mut self, cx: &mut Context<Self>) {
        let Some(opened_for) = self.chrome.conflict_view.as_ref().map(|view| view.editor) else {
            return;
        };
        let active = self.active_editor();
        let shown = if active.as_ref().map(|editor| editor.entity_id()) == Some(opened_for) {
            self.conflict_snapshot(cx)
        } else {
            None
        };
        if shown.is_none() {
            self.chrome.conflict_view = None;
            if let Some(editor) = active {
                self.chrome.focus_next_frame = Some(editor.read(cx).focus_handle(cx));
            }
            cx.notify();
            return;
        }
        if let Some(view) = self.chrome.conflict_view.as_mut() {
            if view.shown != shown {
                if let Some(shown) = &shown {
                    let seed = seed_text(shown, ResultSeed::Side(ConflictSide::Ours));
                    view.result
                        .update(cx, |editor, cx| editor.set_plain_text(&seed, cx));
                }
                view.shown = shown;
            }
        }
        cx.notify();
    }

    /// キャレットの衝突（無ければ後ろ・先頭）の写し（全文は 1 回だけ読む）。
    fn conflict_snapshot(&self, cx: &App) -> Option<ShownConflict> {
        let (editor, blocks) = self.active_conflicts()?;
        let view = editor.read(cx);
        let caret = view
            .buffer()
            .selections()
            .first()
            .map_or(0, |selection| selection.head);
        let index = conflict_at(&blocks, caret)?;
        let block = &blocks[index];
        let text = view.buffer().text();
        let section = |range: &Range<usize>| text.get(range.clone()).map(str::to_string);
        Some(ShownConflict {
            index: index + 1,
            count: blocks.len(),
            line: block.line + 1,
            ours: section(&block.ours)?,
            base: block.base.as_ref().and_then(section),
            theirs: section(&block.theirs)?,
            ours_label: block.ours_label.clone(),
            theirs_label: block.theirs_label.clone(),
        })
    }

    fn on_conflict_view_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_conflict_view(window, cx);
        }
    }

    /// 並べた 1 枚（画面の中央・幅 86%・高さ 86%）: 今の側 / 元 / 入ってくる側 の列、その下の結果の欄、
    /// 帯と同じボタン。
    pub(crate) fn render_conflict_view(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.conflict_view_is_stale() {
            return None;
        }
        let view = self.chrome.conflict_view.as_ref()?;
        let shown = view.shown.clone()?;
        let theme = self.theme.clone();
        let named = |side: String, name: &str| {
            if name.is_empty() {
                side
            } else {
                format!("{side}（{name}）")
            }
        };
        let column = |id: &'static str, title: String, body: &str| {
            let lines: Vec<&str> = body.lines().collect();
            let hidden = lines.len().saturating_sub(VIEW_MAX_LINES);
            let content = if lines.is_empty() {
                div()
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("conflict.view_empty")))
            } else {
                div()
                    .flex()
                    .flex_col()
                    .children(lines.into_iter().take(VIEW_MAX_LINES).map(|line| {
                        div()
                            .min_h(px(17.))
                            .child(SharedString::from(line.replace('\t', "    ")))
                    }))
                    .when(hidden > 0, |list| {
                        list.child(div().text_color(theme.fg2).child(SharedString::from(
                            i18n::t!("conflict.view_more", "count" => hidden),
                        )))
                    })
            };
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .border_1()
                .border_color(theme.border)
                .rounded(px(6.))
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .px(px(10.))
                        .py(px(5.))
                        .bg(theme.bg1)
                        .border_b_1()
                        .border_color(theme.border)
                        .text_size(px(11.))
                        .text_color(theme.fg1)
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .id(id)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(10.))
                        .py(px(6.))
                        .font_family(ui::code_font(cx))
                        .text_size(px(12.))
                        .text_color(theme.fg0)
                        .child(content),
                )
        };
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.warn.alpha(0.5))
                .text_size(px(11.5))
                .text_color(theme.fg0)
                .cursor_pointer()
                .hover(|style| style.bg(theme.warn.alpha(0.18)))
                .child(SharedString::from(label))
        };
        let act = |act: ConflictViewAct| {
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.conflict_view_act(act, window, cx)
            })
        };
        let ours_title = named(i18n::t!("conflict.ours"), &shown.ours_label);
        let theirs_title = named(i18n::t!("conflict.theirs"), &shown.theirs_label);
        let mut columns = div().flex_1().min_h_0().flex().gap(px(8.)).child(column(
            "conflict-view-ours",
            ours_title.clone(),
            &shown.ours,
        ));
        if let Some(base) = &shown.base {
            columns = columns.child(column(
                "conflict-view-base",
                i18n::t!("conflict.view_base"),
                base,
            ));
        }
        columns = columns.child(column(
            "conflict-view-theirs",
            theirs_title.clone(),
            &shown.theirs,
        ));
        let fill = |id: &'static str, label: String, seed: ResultSeed| {
            div()
                .id(id)
                .flex_none()
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(SharedString::from(label))
                .on_mouse_down(MouseButton::Left, act(ConflictViewAct::Fill(seed)))
        };
        let result = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_1()
            .border_color(theme.border)
            .rounded(px(6.))
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(5.))
                    .bg(theme.bg1)
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(11.))
                    .child(div().flex_1().min_w_0().text_color(theme.fg1).child(
                        SharedString::from(i18n::t!(
                            "conflict.view_result",
                            "key" => keymap_core::keystroke_label("cmd-enter")
                        )),
                    ))
                    .child(fill(
                        "conflict-view-fill-ours",
                        i18n::t!("conflict.view_fill_ours"),
                        ResultSeed::Side(ConflictSide::Ours),
                    ))
                    .when(shown.base.is_some(), |header| {
                        header.child(fill(
                            "conflict-view-fill-base",
                            i18n::t!("conflict.view_fill_base"),
                            ResultSeed::Base,
                        ))
                    })
                    .child(fill(
                        "conflict-view-fill-theirs",
                        i18n::t!("conflict.view_fill_theirs"),
                        ResultSeed::Side(ConflictSide::Theirs),
                    ))
                    .child(fill(
                        "conflict-view-fill-both",
                        i18n::t!("conflict.view_fill_both"),
                        ResultSeed::Side(ConflictSide::Both),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(view.result.clone()),
            );
        let card = div()
            .track_focus(&view.focus)
            .on_key_down(cx.listener(Self::on_conflict_view_key_down))
            // 結果の欄（エディタ）の Esc と ⌘⏎ は、エディタが使わなければアクションのまま上がってくる。
            .on_action(cx.listener(|this, _: &editor_view::Cancel, window, cx| {
                this.close_conflict_view(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &agent_panel::SubmitPrompt, window, cx| {
                    this.conflict_view_act(ConflictViewAct::ResolveWithResult, window, cx)
                }),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(gpui::relative(0.86))
            .h(gpui::relative(0.86))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(12.),
                gpui::hsla(0., 0., 0., 0.5),
            )
            .blur_radius(px(28.))])
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!(
                        "conflict.view_title",
                        "index" => shown.index,
                        "count" => shown.count,
                        "line" => shown.line
                    ))),
            )
            .child(columns)
            .child(result)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .when(shown.base.is_none(), |hint| {
                                hint.child(SharedString::from(i18n::t!("conflict.view_no_base")))
                            }),
                    )
                    .child(
                        button("conflict-view-use-result", i18n::t!("conflict.use_result"))
                            .on_mouse_down(
                                MouseButton::Left,
                                act(ConflictViewAct::ResolveWithResult),
                            ),
                    )
                    .child(button("conflict-view-use-ours", ours_title).on_mouse_down(
                        MouseButton::Left,
                        act(ConflictViewAct::Resolve(ConflictSide::Ours)),
                    ))
                    .child(
                        button("conflict-view-use-theirs", theirs_title).on_mouse_down(
                            MouseButton::Left,
                            act(ConflictViewAct::Resolve(ConflictSide::Theirs)),
                        ),
                    )
                    .child(
                        button("conflict-view-use-both", i18n::t!("conflict.both")).on_mouse_down(
                            MouseButton::Left,
                            act(ConflictViewAct::Resolve(ConflictSide::Both)),
                        ),
                    )
                    .child(
                        button("conflict-view-next", i18n::t!("conflict.next"))
                            .on_mouse_down(MouseButton::Left, act(ConflictViewAct::Next)),
                    )
                    .child(
                        div()
                            .id("conflict-view-close")
                            .flex_none()
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(11.5))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .child(SharedString::from(i18n::t!("conflict.view_close")))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.close_conflict_view(window, cx)
                                }),
                            ),
                    ),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.25))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_conflict_view(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
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
                .child(
                    button("conflict-side-by-side", i18n::t!("conflict.side_by_side"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.show_conflict_side_by_side(&ShowConflictSideBySide, window, cx)
                            }),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// 進行中のマージ / リベース（どちらでもなければ `None`）。worktree でも正しい場所を git に聞く。
/// リベースを先に見る: `git rebase -r` がマージの衝突で止まると MERGE_HEAD もあり、`merge --abort` では
/// 取り消せずリベースが残る（HEAD は切り離されたまま）。
fn in_progress(host: &dyn host::Host, root: &Path) -> Option<InProgress> {
    let git = |args: &[&str]| {
        host.run_command(&host::CommandSpec::new("git", root).args(args.iter().copied()))
            .ok()
            .filter(|output| output.status_code == Some(0))
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    for directory in ["rebase-merge", "rebase-apply"] {
        let Some(path) = git(&["rev-parse", "--git-path", directory]) else {
            continue;
        };
        if exists_from(host, root, &root.join(path)) {
            return Some(InProgress::Rebase);
        }
    }
    if git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_some() {
        return Some(InProgress::Merge);
    }
    None
}

/// `path` があるか。接続先の host は開いた worktree の外（linked worktree の git の置き場は統合先の
/// `.git/worktrees/…`）を metadata で見られないので、worktree の中から `test -e` で聞く。
fn exists_from(host: &dyn host::Host, root: &Path, path: &Path) -> bool {
    if host.is_remote() {
        let spec = host::CommandSpec::new("test", root)
            .args(["-e".to_string(), path.to_string_lossy().into_owned()]);
        host.run_command(&spec)
            .is_ok_and(|output| output.status_code == Some(0))
    } else {
        host.metadata(path).is_ok()
    }
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

    const THREE_WAY: &str = "<<<<<<< HEAD\nlet x = 1;\n||||||| base\nlet x = 0;\n=======\nlet x = 2;\n>>>>>>> feature/x\n";

    /// diff3 の印（`|||||||`）があれば元（共通の祖先）も拾う。2 つだけの印なら無い。
    #[test]
    fn the_base_comes_from_diff3_markers() {
        let block = &parse_conflicts(THREE_WAY)[0];
        assert_eq!(&THREE_WAY[block.ours.clone()], "let x = 1;\n");
        assert_eq!(
            block.base.clone().map(|base| &THREE_WAY[base]),
            Some("let x = 0;\n")
        );
        assert_eq!(&THREE_WAY[block.theirs.clone()], "let x = 2;\n");
        assert_eq!(parse_conflicts(TWO_WAY)[0].base, None);
    }

    /// 並べて見る（O19）: キャレットの衝突の今の側 / 元 / 入ってくる側を出し、ボタンは帯と同じ解決をして
    /// 次の衝突を見せる。無くなれば閉じる。衝突が無ければ開かずに知らせる。
    #[gpui::test]
    fn conflicts_can_be_compared_side_by_side(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_conflicts_view_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        let file = root.join("merge.rs");
        std::fs::write(&file, format!("{THREE_WAY}{TWO_WAY}")).unwrap();
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
            workspace.show_conflict_side_by_side(&ShowConflictSideBySide, window, cx);
            let shown = workspace
                .chrome
                .conflict_view
                .as_ref()
                .and_then(|view| view.shown.clone())
                .expect("開く");
            assert_eq!((shown.index, shown.count, shown.line), (1, 2, 1));
            assert_eq!(shown.ours, "let x = 1;\n");
            assert_eq!(shown.base.as_deref(), Some("let x = 0;\n"));
            assert_eq!(shown.theirs, "let x = 2;\n");
            assert_eq!(shown.theirs_label, "feature/x");
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.conflict_view_act(ConflictViewAct::Resolve(ConflictSide::Theirs), window, cx);
            let shown = workspace
                .chrome
                .conflict_view
                .as_ref()
                .and_then(|view| view.shown.clone())
                .expect("次の衝突を見せる");
            assert_eq!((shown.index, shown.count), (1, 1));
            assert_eq!(shown.base, None, "2 つだけの印");
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.conflict_view_act(ConflictViewAct::Resolve(ConflictSide::Ours), window, cx);
            assert!(workspace.chrome.conflict_view.is_none(), "無くなれば閉じる");
            let editor = workspace.active_editor().expect("エディタ");
            assert_eq!(
                editor.read(cx).plain_text(),
                "let x = 2;\nfn a() {}\nlet x = 1;\nfn b() {}\n"
            );
            workspace.show_conflict_side_by_side(&ShowConflictSideBySide, window, cx);
            assert!(workspace.chrome.conflict_view.is_none());
            assert!(workspace
                .notifications
                .toasts
                .iter()
                .any(|toast| toast.text.as_ref() == i18n::t!("conflict.none")));
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 衝突の印の入ったファイルを 1 つ開いた窓（既定の keymap つき＝⌘⏎ / Esc を実キーで通す）。
    fn open_conflicted<'a>(
        cx: &'a mut gpui::TestAppContext,
        name: &str,
        content: &str,
    ) -> (Entity<Workspace>, &'a mut gpui::VisualTestContext, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("necoder_conflicts_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        let file = root.join("merge.rs");
        std::fs::write(&file, content).unwrap();
        cx.update(|cx| {
            settings::init(Some(settings_path), None, cx);
            let bindings = keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx)
                .expect("既定 keymap がロードできる");
            cx.bind_keys(bindings);
        });
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
            workspace.show_conflict_side_by_side(&ShowConflictSideBySide, window, cx);
        });
        (workspace, cx, root)
    }

    /// 結果の欄とそのフォーカス（開いている時だけ）。
    fn result_of(workspace: &Workspace, window: &Window, cx: &App) -> Option<(String, bool)> {
        let view = workspace.chrome.conflict_view.as_ref()?;
        let result = view.result.read(cx);
        Some((
            result.plain_text(),
            result.focus_handle(cx).is_focused(window),
        ))
    }

    /// 結果の欄（O19）: 今の側で始まり（フォーカスは欄）、元 / 両方で埋め直せ、打って直した文で
    /// 解決できる（欄の中の ⌘⏎・最後の改行は足す）。解決すると次の衝突の今の側で埋め直す。Esc は欄の
    /// 中からでも閉じ、空の結果は衝突ごと消す。
    #[gpui::test]
    fn a_conflict_is_resolved_with_the_edited_result(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = open_conflicted(cx, "result", &format!("{THREE_WAY}{TWO_WAY}"));
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(
                result_of(workspace, window, cx),
                Some(("let x = 1;\n".to_string(), true)),
                "今の側で始まり、フォーカスは欄"
            );
            workspace.conflict_view_act(ConflictViewAct::Fill(ResultSeed::Base), window, cx);
            assert_eq!(
                result_of(workspace, window, cx).map(|(text, _)| text),
                Some("let x = 0;\n".to_string())
            );
            workspace.conflict_view_act(
                ConflictViewAct::Fill(ResultSeed::Side(ConflictSide::Both)),
                window,
                cx,
            );
            assert_eq!(
                result_of(workspace, window, cx),
                Some(("let x = 1;\nlet x = 2;\n".to_string(), true)),
                "埋め直した後も欄で打てる"
            );
            let result = workspace
                .chrome
                .conflict_view
                .as_ref()
                .map(|view| view.result.clone())
                .expect("欄");
            result.update(cx, |editor, cx| editor.set_plain_text("let x = 3;", cx));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_input(" // 手で");
        cx.simulate_keystrokes("cmd-enter");
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            let editor = workspace.active_editor().expect("エディタ");
            assert_eq!(
                editor.read(cx).plain_text(),
                format!("let x = 3; // 手で\n{TWO_WAY}")
            );
            let shown = workspace
                .chrome
                .conflict_view
                .as_ref()
                .and_then(|view| view.shown.clone())
                .expect("次の衝突を見せる");
            assert_eq!((shown.index, shown.count), (1, 1));
            assert_eq!(
                result_of(workspace, window, cx),
                Some(("let x = 1;\n".to_string(), true)),
                "次の衝突の今の側で埋め直す"
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(
                workspace.chrome.conflict_view.is_none(),
                "欄の中の Esc で閉じる"
            );
            workspace.show_conflict_side_by_side(&ShowConflictSideBySide, window, cx);
            let result = workspace
                .chrome
                .conflict_view
                .as_ref()
                .map(|view| view.result.clone())
                .expect("開き直す");
            result.update(cx, |editor, cx| editor.set_plain_text("", cx));
            workspace.conflict_view_act(ConflictViewAct::ResolveWithResult, window, cx);
            assert!(workspace.chrome.conflict_view.is_none(), "無くなれば閉じる");
            let editor = workspace.active_editor().expect("エディタ");
            assert_eq!(
                editor.read(cx).plain_text(),
                "let x = 3; // 手で\nfn a() {}\nfn b() {}\n",
                "空の結果は衝突ごと消す"
            );
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CRLF のファイル: 欄には `\n` で見せ、書く時は CRLF に戻す（1 つのファイルに改行が混ざらない）。
    #[gpui::test]
    fn the_result_keeps_crlf_line_endings(cx: &mut gpui::TestAppContext) {
        let crlf = "fn a() {}\r\n<<<<<<< HEAD\r\nlet x = 1;\r\n=======\r\nlet x = 2;\r\n>>>>>>> b\r\nfn b() {}\r\n";
        let (workspace, cx, root) = open_conflicted(cx, "crlf", crlf);
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(
                result_of(workspace, window, cx).map(|(text, _)| text),
                Some("let x = 1;\n".to_string())
            );
            let result = workspace
                .chrome
                .conflict_view
                .as_ref()
                .map(|view| view.result.clone())
                .expect("欄");
            result.update(cx, |editor, cx| {
                editor.set_plain_text("let x = 3;\nlet y = 4;", cx)
            });
            workspace.conflict_view_act(ConflictViewAct::ResolveWithResult, window, cx);
            let editor = workspace.active_editor().expect("エディタ");
            assert_eq!(
                editor.read(cx).plain_text(),
                "fn a() {}\r\nlet x = 3;\r\nlet y = 4;\r\nfn b() {}\r\n"
            );
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 並べた 1 枚は見せている衝突を解決する（O19・レビュー）: 欄の中の F8 や ⌃G で本体のキャレットが
    /// 別の衝突へ動いても、見ている物を置き換える。「次へ」は見せている物の次へ進む（キャレットが最初の
    /// 衝突より前にあっても 1 回で 2 つ目・末尾の次は先頭）。
    #[gpui::test]
    fn the_card_resolves_the_conflict_it_shows(cx: &mut gpui::TestAppContext) {
        let content = format!("head\n{TWO_WAY}{THREE_WAY}");
        let (workspace, cx, root) = open_conflicted(cx, "shown", &content);
        workspace.update_in(cx, |workspace, window, cx| {
            let shown = |workspace: &Workspace| {
                workspace
                    .chrome
                    .conflict_view
                    .as_ref()
                    .and_then(|view| view.shown.as_ref())
                    .map(|shown| (shown.index, shown.count))
            };
            assert_eq!(
                shown(workspace),
                Some((1, 2)),
                "キャレットは最初の衝突より前"
            );
            workspace.conflict_view_act(ConflictViewAct::Next, window, cx);
            assert_eq!(shown(workspace), Some((2, 2)), "1 回で 2 つ目へ");
            workspace.conflict_view_act(ConflictViewAct::Next, window, cx);
            assert_eq!(shown(workspace), Some((1, 2)), "末尾の次は先頭");
            // 1 つ目を見せたまま、本体のキャレットだけ 2 つ目の衝突の中へ（F8 / ⌃G と同じ）。
            let editor = workspace.active_editor().expect("エディタ");
            let second = content.rfind("<<<<<<<").expect("2 つ目") + 3;
            editor.update(cx, |editor, cx| {
                editor.select_byte_range(second..second, cx)
            });
            let result = workspace
                .chrome
                .conflict_view
                .as_ref()
                .map(|view| view.result.clone())
                .expect("欄");
            result.update(cx, |editor, cx| editor.set_plain_text("let z = 9;", cx));
            workspace.conflict_view_act(ConflictViewAct::ResolveWithResult, window, cx);
            assert_eq!(
                editor.read(cx).plain_text(),
                format!("head\nfn a() {{}}\nlet z = 9;\nfn b() {{}}\n{THREE_WAY}"),
                "見せている 1 つ目を置き換え、2 つ目は残す"
            );
            workspace.conflict_view_act(ConflictViewAct::Resolve(ConflictSide::Theirs), window, cx);
            assert_eq!(
                editor.read(cx).plain_text(),
                "head\nfn a() {}\nlet z = 9;\nfn b() {}\nlet x = 2;\n"
            );
            assert!(workspace.chrome.conflict_view.is_none(), "無くなれば閉じる");
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 別のタブへ移ると 1 枚は閉じる（O19・レビュー）。戻っても出てこない（残すと、戻った時に欄を
    /// 埋め直して直しかけを失い、フォーカスは下のエディタに居て Esc も ⌘⏎ も届かなかった）。
    #[gpui::test]
    fn the_card_closes_when_the_tab_changes(cx: &mut gpui::TestAppContext) {
        let (workspace, cx, root) = open_conflicted(cx, "tabs", THREE_WAY);
        let plain = root.join("plain.rs");
        std::fs::write(&plain, "fn main() {}\n").unwrap();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_file_sync(plain.clone(), window, cx);
            assert!(
                workspace.render_conflict_view(cx).is_none(),
                "新しく開いたタブの上には描かない"
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.chrome.conflict_view.is_none(), "次の描画で閉じる");
            let merge = workspace
                .tabs
                .iter()
                .position(|tab| tab.path.ends_with("merge.rs"))
                .expect("merge.rs のタブ");
            let plain_tab = workspace
                .tabs
                .iter()
                .position(|tab| tab.path.ends_with("plain.rs"))
                .expect("plain.rs のタブ");
            workspace.select_tab(merge, window, cx);
            assert!(
                workspace.chrome.conflict_view.is_none(),
                "戻っても出てこない"
            );
            workspace.show_conflict_side_by_side(&ShowConflictSideBySide, window, cx);
            assert!(workspace.chrome.conflict_view.is_some());
            workspace.select_tab(plain_tab, window, cx);
            assert!(
                workspace.chrome.conflict_view.is_none(),
                "タブを選んで移っても閉じる"
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            let editor = workspace.active_editor().expect("エディタ");
            assert!(editor.read(cx).plain_text().starts_with("fn main"));
            assert!(
                editor.read(cx).focus_handle(cx).is_focused(window),
                "フォーカスは移った先のエディタ"
            );
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// LF のファイルで入ってくる側だけ CRLF の行を持つ衝突は、結果を LF で書く（O19・レビュー）。
    /// 改行は `<<<<<<<` の行で決める（中身の CRLF で決めると、今の側の LF の行まで CRLF になる）。
    #[gpui::test]
    fn the_result_follows_the_marker_line_ending(cx: &mut gpui::TestAppContext) {
        let mixed = "a\n<<<<<<< HEAD\nx\n=======\ny\r\n>>>>>>> b\nz\n";
        let (workspace, cx, root) = open_conflicted(cx, "mixed", mixed);
        workspace.update_in(cx, |workspace, window, cx| {
            let result = workspace
                .chrome
                .conflict_view
                .as_ref()
                .map(|view| view.result.clone())
                .expect("欄");
            result.update(cx, |editor, cx| editor.set_plain_text("p\nq", cx));
            workspace.conflict_view_act(ConflictViewAct::ResolveWithResult, window, cx);
            let editor = workspace.active_editor().expect("エディタ");
            assert_eq!(editor.read(cx).plain_text(), "a\np\nq\nz\n");
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 欄に入れる文と、書き戻す文（空は衝突ごと消す・最後の改行は足す・空行 1 つは残す）。
    #[test]
    fn the_result_is_seeded_and_written_back() {
        let shown = ShownConflict {
            index: 1,
            count: 1,
            line: 1,
            ours: "a\r\nb\r\n".to_string(),
            base: None,
            theirs: "c\r\n".to_string(),
            ours_label: String::new(),
            theirs_label: String::new(),
        };
        assert_eq!(
            seed_text(&shown, ResultSeed::Side(ConflictSide::Ours)),
            "a\nb\n"
        );
        assert_eq!(
            seed_text(&shown, ResultSeed::Side(ConflictSide::Both)),
            "a\nb\nc\n"
        );
        assert_eq!(seed_text(&shown, ResultSeed::Base), "");
        assert_eq!(result_for_file("a\nb", true), "a\r\nb\r\n");
        assert_eq!(result_for_file("a\r\nb\n", false), "a\nb\n");
        assert_eq!(result_for_file("", true), "");
        assert_eq!(result_for_file("\n", false), "\n");
    }

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

    fn scratch_git(
        tag: &str,
    ) -> Option<(PathBuf, impl Fn(&Path, &[&str]) -> std::process::Output)> {
        let base =
            std::env::temp_dir().join(format!("necoder_conflicts_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("repo")).unwrap();
        let base = paths::canonicalize(&base).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=necoder",
                    "-c",
                    "user.email=necoder@example.com",
                ])
                .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
        };
        git(&base.join("repo"), &["init", "-q", "-b", "main"])
            .status
            .success()
            .then_some((base, git))
    }

    /// `git rebase -r` がマージの衝突で止まると MERGE_HEAD もある。中止はリベースの方（本物の git）。
    #[test]
    fn a_rebase_stopped_on_a_merge_is_aborted_as_a_rebase() {
        let Some((base, git)) = scratch_git("rebase_merges") else {
            return;
        };
        let repo = base.join("repo");
        let file = repo.join("a.txt");
        std::fs::write(&file, "base\n").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        // side と feature が同じ行を変え、手で解いたマージ。main は別のファイルだけを変える
        // （1 つずつの pick は通り、マージの作り直しだけが衝突する）。
        git(&repo, &["checkout", "-q", "-b", "side"]);
        std::fs::write(&file, "side\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "side"]);
        git(&repo, &["checkout", "-q", "-b", "feature", "main"]);
        std::fs::write(&file, "feature\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "feature"]);
        git(&repo, &["merge", "-q", "--no-edit", "side"]);
        std::fs::write(&file, "resolved\n").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "merge side"]);
        git(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("m.txt"), "m\n").unwrap();
        git(&repo, &["add", "m.txt"]);
        git(&repo, &["commit", "-q", "-m", "main"]);
        let rebased = git(&repo, &["rebase", "-r", "main", "feature"]);
        assert!(!rebased.status.success(), "マージの作り直しで衝突する");
        assert!(
            git(&repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .status
                .success(),
            "MERGE_HEAD もある"
        );
        let host = host::LocalHost::shared();
        assert_eq!(in_progress(host.as_ref(), &repo), Some(InProgress::Rebase));
        abort(host.as_ref(), &repo, InProgress::Rebase).unwrap();
        assert_eq!(in_progress(host.as_ref(), &repo), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 開いた根の外を metadata で見られない（SSH の接続先と同じ）host でも、linked worktree のリベースを
    /// 見つける（git の置き場が統合先の `.git/worktrees/…` にある）。
    #[cfg(unix)]
    #[test]
    fn a_rebase_in_a_linked_worktree_is_found_through_a_scoped_host() {
        struct Scoped {
            root: PathBuf,
        }
        impl host::Host for Scoped {
            fn id(&self) -> &str {
                "scoped"
            }
            fn display_name(&self) -> &str {
                "scoped"
            }
            fn is_remote(&self) -> bool {
                true
            }
            fn host_for_project(&self, path: &Path) -> anyhow::Result<Arc<dyn host::Host>> {
                Ok(Arc::new(Scoped {
                    root: path.to_path_buf(),
                }))
            }
            fn canonicalize(&self, path: &Path) -> anyhow::Result<PathBuf> {
                host::LocalHost.canonicalize(path)
            }
            fn metadata(&self, path: &Path) -> anyhow::Result<host::HostMetadata> {
                anyhow::ensure!(
                    std::fs::canonicalize(path)?.starts_with(&self.root),
                    "path escapes project root"
                );
                host::LocalHost.metadata(path)
            }
            fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<host::HostEntry>> {
                host::LocalHost.read_dir(path)
            }
            fn read_file(&self, path: &Path) -> anyhow::Result<host::FileContent> {
                host::LocalHost.read_file(path)
            }
            fn write_file(
                &self,
                path: &Path,
                bytes: &[u8],
                condition: host::WriteCondition,
            ) -> anyhow::Result<host::FileRevision> {
                host::LocalHost.write_file(path, bytes, condition)
            }
            fn list_files(&self, root: &Path, limit: usize) -> anyhow::Result<Vec<PathBuf>> {
                host::LocalHost.list_files(root, limit)
            }
            fn search_project(
                &self,
                root: &Path,
                spec: &host::TextSearchSpec,
                file_limit: usize,
            ) -> anyhow::Result<Vec<host::TextSearchHit>> {
                host::LocalHost.search_project(root, spec, file_limit)
            }
            fn run_command(&self, spec: &host::CommandSpec) -> anyhow::Result<host::CommandOutput> {
                anyhow::ensure!(
                    spec.cwd.starts_with(&self.root),
                    "path is outside remote project"
                );
                host::LocalHost.run_command(spec)
            }
            fn spawn_process(&self, spec: &host::CommandSpec) -> anyhow::Result<host::HostProcess> {
                host::LocalHost.spawn_process(spec)
            }
            fn terminal_launch(&self, cwd: &Path) -> anyhow::Result<Option<host::TerminalLaunch>> {
                host::LocalHost.terminal_launch(cwd)
            }
        }

        let Some((base, git)) = scratch_git("linked_rebase") else {
            return;
        };
        let repo = base.join("repo");
        let task = base.join("task");
        std::fs::write(repo.join("a.txt"), "base\n").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "task",
                task.to_str().unwrap(),
            ],
        );
        std::fs::write(task.join("a.txt"), "task\n").unwrap();
        git(&task, &["commit", "-q", "-am", "task"]);
        std::fs::write(repo.join("a.txt"), "main\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "main"]);
        let rebased = git(&task, &["rebase", "main"]);
        assert!(!rebased.status.success(), "衝突する");
        let scoped = Scoped { root: task.clone() };
        assert_eq!(in_progress(&scoped, &task), Some(InProgress::Rebase));
        git(&task, &["rebase", "--abort"]);
        assert_eq!(in_progress(&scoped, &task), None);
        let _ = std::fs::remove_dir_all(&base);
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
