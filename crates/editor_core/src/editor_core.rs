//! editor_core — テキスト編集の中核（GPUI 非依存・ロジックのみ）。
//!
//! ARCHITECTURE §1/§3 の鉄則: この crate は **GPUI を知らない**。純データ + ロジックで、
//! ユニットテストが最速で回る層。描画側（editor_view）は [`BufferSnapshot`] だけを読む。
//!
//! 位置は全て **UTF-8 バイトオフセット**。[`Selection`] は byte offset で、編集・スナップショット
//! は常に char 境界にクリップする（ropey 2.0 のバイト索引 API と 1:1）。undo/redo は
//! [`Transaction`] 単位。位置追従アンカーは M2 では offset ベースの簡易版（multibuffer 時に anchor 化）。

use anyhow::{Context as _, Result};
use host::{FileRevision, Host, LocalHost, WriteCondition};
use ropey::{LineType, Rope};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 行の数え方。コードエディタなので LF（と CRLF）のみを改行とみなす（CR 単独は改行にしない）。
const LINE_TYPE: LineType = LineType::LF;

/// 選択（byte offset）。`anchor == head` はキャレット。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }
}

/// 行・列（列は行内の byte offset、改行は含めない）。描画・カーソル計算の座標。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub row: usize,
    pub column: usize,
}

impl Point {
    pub fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// Transaction の識別子（単調増加）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransactionId(u64);

/// 1 箇所の置換。`start` は**変更前バッファ**の byte offset。undo は old、redo は new を使う。
#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    old: String,
    new: String,
}

/// 1 回の編集（複数カーソル分の Edit をまとめる）。undo/redo の単位。
#[derive(Clone, Debug)]
struct Transaction {
    id: TransactionId,
    edits: Vec<Edit>, // start 昇順・非重複（変更前座標）
    before: Vec<Selection>,
    after: Vec<Selection>,
}

#[derive(Default)]
struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
}

impl History {
    fn allocate_id(&mut self) -> TransactionId {
        let id = TransactionId(self.next_id);
        self.next_id += 1;
        id
    }
}

/// 編集対象のテキストバッファ。rope 本体 + 選択 + undo 履歴を持つ。
pub struct Buffer {
    rope: Rope,
    selections: Vec<Selection>,
    history: History,
    version: u64,
    host: Arc<dyn Host>,
    file: Option<PathBuf>,
    file_revision: Option<FileRevision>,
    dirty: bool,
    /// 直近の編集の (start, old, new) 列（**変更前座標**・start 昇順）。増分パース /
    /// didChange 差分化が読む（M11-8）。undo/redo/reload では None（読む側は full へフォールバック）。
    last_change: Option<Vec<(usize, String, String)>>,
    /// 読み取り専用（diff タブ等・M11-9）。true の間は編集系が全て no-op。
    read_only: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    /// 空の無題バッファ。
    pub fn new() -> Buffer {
        Buffer {
            rope: Rope::new(),
            selections: vec![Selection::cursor(0)],
            history: History::default(),
            version: 0,
            host: LocalHost::shared(),
            file: None,
            file_revision: None,
            dirty: false,
            last_change: None,
            read_only: false,
        }
    }

    /// 文字列から無題バッファを作る（主にテスト用）。
    pub fn from_str(text: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(text),
            ..Buffer::new()
        }
    }

    /// ファイルを読み込んで開く。
    pub fn from_file(path: impl AsRef<Path>) -> Result<Buffer> {
        Self::from_host(LocalHost::shared(), path)
    }

    /// local/remote 共通の Host からファイルを読み込んで開く。
    /// **UI スレッドから直接呼ばない**（remote は 30s ブロックしうる — ARCHITECTURE §9）。
    /// UI 経路は背景で `host.read_file` → [`Buffer::from_content`] を使う。
    pub fn from_host(host: Arc<dyn Host>, path: impl AsRef<Path>) -> Result<Buffer> {
        let path = path.as_ref();
        let content = host
            .read_file(path)
            .with_context(|| format!("ファイルを開けない: {}", path.display()))?;
        Self::from_content(host, path, content)
    }

    /// 読み込み済み内容からバッファを作る（読み込み自体は呼び出し側が背景スレッドで行う）。
    pub fn from_content(
        host: Arc<dyn Host>,
        path: impl AsRef<Path>,
        content: host::FileContent,
    ) -> Result<Buffer> {
        let path = path.as_ref();
        let rope = Rope::from_reader(io::Cursor::new(&content.bytes))
            .with_context(|| format!("ファイルを読めない: {}", path.display()))?;
        Ok(Buffer {
            rope,
            host,
            file: Some(path.to_path_buf()),
            file_revision: Some(content.revision),
            ..Buffer::new()
        })
    }

    // ── 編集 ──

    /// 複数レンジを同一テキストで置換する（複数カーソルのタイプ相当）。1 Transaction にまとめる。
    pub fn edit(&mut self, ranges: &[Range<usize>], text: &str) -> TransactionId {
        if self.read_only {
            return self.history.allocate_id();
        }
        let normalized = self.normalize_ranges(ranges);
        let edits: Vec<Edit> = normalized
            .iter()
            .map(|range| Edit {
                start: range.start,
                old: self.rope.slice(range.clone()).to_string(),
                new: text.to_string(),
            })
            .collect();

        if edits.is_empty() {
            return self.history.allocate_id();
        }

        let before = self.selections.clone();
        let cursors = Self::apply_forward(&mut self.rope, &edits);
        let after: Vec<Selection> = cursors.into_iter().map(Selection::cursor).collect();

        let id = self.history.allocate_id();
        self.last_change = Some(
            edits
                .iter()
                .map(|edit| (edit.start, edit.old.clone(), edit.new.clone()))
                .collect(),
        );
        self.history.undo.push(Transaction {
            id,
            edits,
            before,
            after: after.clone(),
        });
        self.history.redo.clear();
        self.selections = after;
        self.version += 1;
        self.dirty = true;
        id
    }

    /// レンジ毎に**異なるテキスト**で置換する一括編集（LSP フォーマット/rename の TextEdit 適用）。
    /// 1 Transaction ＝ undo 一発。レンジは正規化（char 境界クリップ・昇順・重複は先勝ちで破棄）。
    pub fn edit_batch(&mut self, edits: &[(Range<usize>, String)]) -> TransactionId {
        if self.read_only {
            return self.history.allocate_id();
        }
        // 正規化: クリップ → start 昇順 → 重なりは先勝ち。
        let len = self.rope.len();
        let mut normalized: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, text)| {
                let start = self.rope.floor_char_boundary(range.start.min(len));
                let end = self.rope.ceil_char_boundary(range.end.min(len));
                (start.min(end)..start.max(end), text.as_str())
            })
            .collect();
        normalized.sort_by_key(|(range, _)| range.start);
        let mut applied: Vec<Edit> = Vec::with_capacity(normalized.len());
        let mut last_end = 0usize;
        for (range, text) in normalized {
            if !applied.is_empty() && range.start < last_end {
                continue; // 重なりは捨てる（LSP は非重複を保証するが防御）
            }
            last_end = range.end;
            applied.push(Edit {
                start: range.start,
                old: self.rope.slice(range.clone()).to_string(),
                new: text.to_string(),
            });
        }
        if applied.is_empty() {
            return self.history.allocate_id();
        }
        let before = self.selections.clone();
        let cursors = Self::apply_forward(&mut self.rope, &applied);
        let after: Vec<Selection> = cursors.into_iter().map(Selection::cursor).collect();
        let id = self.history.allocate_id();
        self.last_change = Some(
            applied
                .iter()
                .map(|edit| (edit.start, edit.old.clone(), edit.new.clone()))
                .collect(),
        );
        self.history.undo.push(Transaction {
            id,
            edits: applied,
            before,
            after: after.clone(),
        });
        self.history.redo.clear();
        self.selections = after;
        self.version += 1;
        self.dirty = true;
        id
    }

    /// 現在の選択をテキストで置換（＝挿入 / 上書き）。
    pub fn insert(&mut self, text: &str) -> TransactionId {
        let ranges: Vec<Range<usize>> = self.selections.iter().map(Selection::range).collect();
        self.edit(&ranges, text)
    }

    /// Backspace: 空選択は前の 1 文字を、範囲選択はその範囲を消す。
    pub fn delete_backward(&mut self) -> TransactionId {
        let ranges: Vec<Range<usize>> = self
            .selections
            .iter()
            .map(|selection| {
                if selection.is_empty() {
                    let head = selection.head;
                    let start = if head == 0 {
                        0
                    } else {
                        self.rope.floor_char_boundary(head - 1)
                    };
                    start..head
                } else {
                    selection.range()
                }
            })
            .collect();
        self.edit(&ranges, "")
    }

    /// Delete: 空選択は後ろの 1 文字を、範囲選択はその範囲を消す。
    pub fn delete_forward(&mut self) -> TransactionId {
        let len = self.rope.len();
        let ranges: Vec<Range<usize>> = self
            .selections
            .iter()
            .map(|selection| {
                if selection.is_empty() {
                    let head = selection.head;
                    let end = if head >= len {
                        len
                    } else {
                        self.rope.ceil_char_boundary(head + 1)
                    };
                    head..end
                } else {
                    selection.range()
                }
            })
            .collect();
        self.edit(&ranges, "")
    }

    /// 直前の Transaction を取り消す。
    pub fn undo(&mut self) -> Option<TransactionId> {
        if self.read_only {
            return None;
        }
        self.last_change = None; // 増分の消費側は full へフォールバック
        let transaction = self.history.undo.pop()?;
        Self::apply_reverse(&mut self.rope, &transaction.edits);
        self.selections = transaction.before.clone();
        self.version += 1;
        self.dirty = true;
        let id = transaction.id;
        self.history.redo.push(transaction);
        Some(id)
    }

    /// 取り消した Transaction をやり直す。
    pub fn redo(&mut self) -> Option<TransactionId> {
        if self.read_only {
            return None;
        }
        self.last_change = None;
        let transaction = self.history.redo.pop()?;
        Self::apply_forward(&mut self.rope, &transaction.edits);
        self.selections = transaction.after.clone();
        self.version += 1;
        self.dirty = true;
        let id = transaction.id;
        self.history.undo.push(transaction);
        Some(id)
    }

    // ── 編集の所作（M10-9: 単語削除・行操作・コメント・インデント） ──

    /// ⌥⌫ 単語削除（各キャレットの直前の単語境界まで消す）。
    pub fn delete_word_backward(&mut self) -> TransactionId {
        let snapshot = self.snapshot();
        let ranges: Vec<Range<usize>> = self
            .selections
            .iter()
            .map(|selection| {
                if selection.is_empty() {
                    snapshot.prev_word_boundary(selection.head)..selection.head
                } else {
                    selection.range()
                }
            })
            .collect();
        self.edit(&ranges, "")
    }

    /// 主選択の行域を返す `(first_row, last_row)`。
    fn primary_rows(&self) -> (usize, usize) {
        let snapshot = self.snapshot();
        let primary = self
            .selections
            .first()
            .copied()
            .unwrap_or(Selection::cursor(0));
        let first = snapshot.byte_to_point(primary.start()).row;
        // 範囲末尾が行頭ちょうどのときはその行を含めない（VSCode 同等）。
        let end_point = snapshot.byte_to_point(primary.end());
        let last = if !primary.is_empty() && end_point.column == 0 && end_point.row > first {
            end_point.row - 1
        } else {
            end_point.row
        };
        (first, last)
    }

    /// ⌥↑↓ 行移動（主選択の行域を上/下の行と入れ替え）。端では何もしない。
    pub fn move_lines(&mut self, down: bool) -> Option<TransactionId> {
        let snapshot = self.snapshot();
        let (first, last) = self.primary_rows();
        let span = snapshot.line_span_bytes(first, last);
        let mut span_text = self.rope.slice(span.clone()).to_string();
        if down {
            if last + 1 >= snapshot.line_count() {
                return None;
            }
            let next = snapshot.line_span_bytes(last + 1, last + 1);
            let mut next_text = self.rope.slice(next.clone()).to_string();
            // 最終行（改行なし）が絡む場合は改行を付け替えて整える。
            if !next_text.ends_with('\n') {
                next_text.push('\n');
                if span_text.ends_with('\n') {
                    span_text.pop();
                }
            }
            let selections = self.selections.clone();
            let delta = next_text.len();
            let id = self.edit(&[span.start..next.end], &format!("{next_text}{span_text}"));
            // set_selections 経由 = 長さ・char 境界へクランプ（移動行の外のカーソルは近似追従）。
            self.set_selections(
                selections
                    .into_iter()
                    .map(|s| Selection::new(s.anchor + delta, s.head + delta))
                    .collect(),
            );
            Some(id)
        } else {
            if first == 0 {
                return None;
            }
            let previous = snapshot.line_span_bytes(first - 1, first - 1);
            let mut previous_text = self.rope.slice(previous.clone()).to_string();
            if !span_text.ends_with('\n') {
                span_text.push('\n');
                if previous_text.ends_with('\n') {
                    previous_text.pop();
                }
            }
            let selections = self.selections.clone();
            let delta = previous_text.len().min(previous.end - previous.start) as isize;
            let id = self.edit(
                &[previous.start..span.end],
                &format!("{span_text}{previous_text}"),
            );
            self.set_selections(
                selections
                    .into_iter()
                    .map(|s| {
                        Selection::new(
                            (s.anchor as isize - delta).max(0) as usize,
                            (s.head as isize - delta).max(0) as usize,
                        )
                    })
                    .collect(),
            );
            Some(id)
        }
    }

    /// ⇧⌥↑↓ 行複製（主選択の行域のコピーを上/下に挿入。キャレットは元のテキスト上に留まる）。
    pub fn duplicate_lines(&mut self, down: bool) -> TransactionId {
        let snapshot = self.snapshot();
        let (first, last) = self.primary_rows();
        let span = snapshot.line_span_bytes(first, last);
        let mut text = self.rope.slice(span.clone()).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let selections = self.selections.clone();
        let id = self.edit(&[span.start..span.start], &text);
        // 挿入は span 先頭 = 元テキストは text.len() 分だけ下へ。
        // down（下に複製）= キャレットを元の位置（上側コピー）へ戻す。up = 下側（移動後）に留まる。
        if down {
            // 挿入点より後ろのカーソルは内容がずれるため位置は近似（クランプだけ保証する）。
            self.set_selections(selections);
        } else {
            self.set_selections(
                selections
                    .into_iter()
                    .map(|s| Selection::new(s.anchor + text.len(), s.head + text.len()))
                    .collect(),
            );
        }
        id
    }

    /// ⌘⇧K 行削除（主選択の行域を丸ごと消す）。
    pub fn delete_lines(&mut self) -> TransactionId {
        let snapshot = self.snapshot();
        let (first, last) = self.primary_rows();
        let span = snapshot.line_span_bytes(first, last);
        let id = self.edit(&[span], "");
        id
    }

    /// ⌘/ コメントトグル。行域の非空行が全て `prefix` で始まるなら外し、そうでなければ付ける。
    pub fn toggle_comment(&mut self, prefix: &str) -> TransactionId {
        let snapshot = self.snapshot();
        let (first, last) = self.primary_rows();
        let mut all_commented = true;
        let mut any_content = false;
        for row in first..=last {
            let line = snapshot.line_text(row);
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            any_content = true;
            if !trimmed.starts_with(prefix) {
                all_commented = false;
            }
        }
        if !any_content {
            return self.history.allocate_id();
        }
        if all_commented {
            // 外す: 各行の `prefix( )?` を削除。
            let mut ranges = Vec::new();
            for row in first..=last {
                let line = snapshot.line_text(row);
                let indent_len = line.len() - line.trim_start().len();
                let trimmed = line.trim_start();
                if !trimmed.starts_with(prefix) {
                    continue;
                }
                let line_start = snapshot.point_to_byte(Point::new(row, 0));
                let mut remove = prefix.len();
                if trimmed[prefix.len()..].starts_with(' ') {
                    remove += 1;
                }
                ranges.push(line_start + indent_len..line_start + indent_len + remove);
            }
            self.edit(&ranges, "")
        } else {
            // 付ける: 非空行のインデント位置に `prefix ` を挿す（零幅レンジ + 同一テキスト = 1 Transaction）。
            let mut ranges = Vec::new();
            for row in first..=last {
                let line = snapshot.line_text(row);
                if line.trim_start().is_empty() {
                    continue;
                }
                let indent_len = line.len() - line.trim_start().len();
                let position = snapshot.point_to_byte(Point::new(row, 0)) + indent_len;
                ranges.push(position..position);
            }
            self.edit(&ranges, &format!("{prefix} "))
        }
    }

    /// 改行 + 自動インデント（前行の字下げ継承 + ブロック開始 `{([:` の直後は 1 段深く）。
    pub fn insert_newline_indented(&mut self, tab_size: usize) -> TransactionId {
        let snapshot = self.snapshot();
        let head = self
            .selections
            .first()
            .copied()
            .unwrap_or(Selection::cursor(0))
            .start();
        let row = snapshot.byte_to_point(head).row;
        let line = snapshot.line_text(row);
        let column = head - snapshot.point_to_byte(Point::new(row, 0));
        let before_caret = &line[..column.min(line.len())];
        let indent: String = before_caret
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let extra = if before_caret.trim_end().ends_with(['{', '(', '[', ':']) {
            " ".repeat(tab_size)
        } else {
            String::new()
        };
        self.insert(&format!("\n{indent}{extra}"))
    }

    /// Tab/⌘] インデント（主選択の行域の各非空行頭に `tab_size` 個の空白を挿す）。
    pub fn indent_lines(&mut self, tab_size: usize) -> TransactionId {
        let snapshot = self.snapshot();
        let (first, last) = self.primary_rows();
        let mut ranges = Vec::new();
        for row in first..=last {
            let position = snapshot.point_to_byte(Point::new(row, 0));
            ranges.push(position..position);
        }
        let selections = self.selections.clone();
        let id = self.edit(&ranges, &" ".repeat(tab_size));
        // キャレットを（行数 × tab_size ぶん）追従させる代わりに、単純に各選択を右へずらす。
        // set_selections 経由 = 長さ・char 境界へクランプ（対象行の外のカーソルは近似追従）。
        let shift = tab_size;
        self.set_selections(
            selections
                .into_iter()
                .map(|s| Selection::new(s.anchor + shift, s.head + shift))
                .collect(),
        );
        id
    }

    /// ⇧Tab/⌘[ アンインデント（各行頭の空白を最大 `tab_size` 個（またはタブ 1 個）外す）。
    pub fn outdent_lines(&mut self, tab_size: usize) -> TransactionId {
        let snapshot = self.snapshot();
        let (first, last) = self.primary_rows();
        let mut ranges = Vec::new();
        for row in first..=last {
            let line = snapshot.line_text(row);
            let line_start = snapshot.point_to_byte(Point::new(row, 0));
            let remove = if line.starts_with('\t') {
                1
            } else {
                line.chars()
                    .take(tab_size)
                    .take_while(|c| *c == ' ')
                    .count()
            };
            if remove > 0 {
                ranges.push(line_start..line_start + remove);
            }
        }
        if ranges.is_empty() {
            return self.history.allocate_id();
        }
        self.edit(&ranges, "")
    }

    // ── multi-cursor（M10-10: ⌘D・⌥⌘↑↓・Esc 単一化） ──

    /// ⌘D。選択が空ならキャレット位置の単語を選択。選択があれば、その内容の**次の一致**を
    /// 追加選択する（末尾で先頭へ回る・既に選択済みの一致はスキップ）。追加できたら true。
    pub fn select_next_occurrence(&mut self) -> bool {
        let snapshot = self.snapshot();
        // 初回: 空選択 → 単語選択に育てる。
        if self.selections.len() == 1 && self.selections[0].is_empty() {
            if let Some(range) = snapshot.word_range_at(self.selections[0].head) {
                self.selections = vec![Selection::new(range.start, range.end)];
                return true;
            }
            return false;
        }
        let primary = self
            .selections
            .first()
            .copied()
            .unwrap_or(Selection::cursor(0));
        if primary.is_empty() {
            return false;
        }
        let needle = self.text_range(primary.range());
        if needle.is_empty() {
            return false;
        }
        let text = self.text();
        let search_from = self
            .selections
            .iter()
            .map(Selection::end)
            .max()
            .unwrap_or(0);
        let already: Vec<Range<usize>> = self.selections.iter().map(Selection::range).collect();
        // search_from 以降 → 先頭から search_from まで、の順で探す（wrap）。
        let find_next = |from: usize| -> Option<usize> {
            text.get(from..)
                .and_then(|tail| tail.find(&needle))
                .map(|position| from + position)
        };
        let mut cursor = search_from;
        for _ in 0..=already.len() + 1 {
            let found = match find_next(cursor) {
                Some(found) => found,
                None => match text.find(&needle) {
                    Some(found) => found,
                    None => return false,
                },
            };
            let range = found..found + needle.len();
            if already.iter().any(|existing| *existing == range) {
                // 既に選択済み → その先から続ける（全部選択済みなら一周して終わる）。
                cursor = range.end;
                if already.len() >= self.selections.len() && found < search_from {
                    return false;
                }
                if cursor >= text.len() {
                    cursor = 0;
                }
                continue;
            }
            let mut selections = self.selections.clone();
            selections.push(Selection::new(range.start, range.end));
            selections.sort_by_key(Selection::start);
            self.selections = selections;
            return true;
        }
        false
    }

    /// ⌥⌘↑↓。端の選択と同じ列で上/下の行にキャレットを追加する。追加できたら true。
    pub fn add_cursor_vertically(&mut self, down: bool) -> bool {
        let snapshot = self.snapshot();
        let anchor_selection = if down {
            self.selections.iter().max_by_key(|s| s.head).copied()
        } else {
            self.selections.iter().min_by_key(|s| s.head).copied()
        };
        let Some(selection) = anchor_selection else {
            return false;
        };
        let point = snapshot.byte_to_point(selection.head);
        let target_row = if down {
            if point.row + 1 >= snapshot.line_count() {
                return false;
            }
            point.row + 1
        } else {
            if point.row == 0 {
                return false;
            }
            point.row - 1
        };
        let offset = snapshot.point_to_byte(Point::new(target_row, point.column));
        if self
            .selections
            .iter()
            .any(|s| s.head == offset && s.is_empty())
        {
            return false;
        }
        let mut selections = self.selections.clone();
        selections.push(Selection::cursor(offset));
        selections.sort_by_key(Selection::start);
        self.selections = selections;
        true
    }

    /// ⌥クリック。任意位置にキャレットを追加する（既にあれば何もしない）。
    pub fn add_cursor_at(&mut self, offset: usize) {
        let snapshot = self.snapshot();
        let offset = snapshot.clip_offset(offset);
        if self
            .selections
            .iter()
            .any(|s| s.is_empty() && s.head == offset)
        {
            return;
        }
        let mut selections = self.selections.clone();
        selections.push(Selection::cursor(offset));
        selections.sort_by_key(Selection::start);
        self.selections = selections;
    }

    /// Esc。複数選択を先頭の 1 個へ畳む。畳んだら true（単一なら何もしない）。
    pub fn collapse_to_primary(&mut self) -> bool {
        if self.selections.len() <= 1 {
            return false;
        }
        self.selections.truncate(1);
        true
    }

    // ── 保存 ──

    /// 開いているファイルへ保存する。無題なら [`Buffer::save_as`] を使う。
    /// **UI スレッドから直接呼ばない**（remote は 30s ブロックしうる）。UI 経路は
    /// [`Buffer::prepare_save`] → 背景で `PendingSave::write` → [`Buffer::complete_save`]。
    pub fn save(&mut self) -> Result<()> {
        let path = self
            .file
            .clone()
            .context("保存先が未設定（無題バッファ）")?;
        self.save_to(&path)
    }

    /// 非同期保存の準備: 書き込みに必要な一式（host・パス・全文・競合条件・現 version）を写す。
    /// 無題バッファは `None`。
    pub fn prepare_save(&self) -> Option<PendingSave> {
        let path = self.file.clone()?;
        Some(PendingSave {
            host: self.host.clone(),
            path,
            text: self.rope.to_string(),
            condition: self
                .file_revision
                .clone()
                .map(WriteCondition::Matches)
                .unwrap_or(WriteCondition::Any),
            version: self.version,
        })
    }

    /// 非同期保存の完了を反映する。保存開始時（`saved_version`）から編集が無ければ dirty を下ろす
    /// （書き込み中に編集された分は dirty のまま＝次の保存対象）。
    pub fn complete_save(&mut self, revision: FileRevision, saved_version: u64) {
        self.file_revision = Some(revision);
        if self.version == saved_version {
            self.dirty = false;
        }
    }

    /// パスを指定して保存し、以後そのファイルに紐づける。
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref().to_path_buf();
        let previous_file = self.file.replace(path.clone());
        let previous_revision = self.file_revision.take();
        if let Err(error) = self.save_to(&path) {
            self.file = previous_file;
            self.file_revision = previous_revision;
            return Err(error);
        }
        Ok(())
    }

    fn save_to(&mut self, path: &Path) -> Result<()> {
        let condition = self
            .file_revision
            .map(WriteCondition::Matches)
            .unwrap_or(WriteCondition::Any);
        let text = self.rope.to_string();
        let revision = self
            .host
            .write_file(path, text.as_bytes(), condition)
            .with_context(|| format!("書き込みに失敗: {}", path.display()))?;
        self.file_revision = Some(revision);
        self.dirty = false;
        Ok(())
    }

    // ── 外部変更の追従（watch 基盤・M10） ──

    /// ディスク上のファイルが読み込み時から（おそらく）変わっていないか。
    /// len + mtime の安価な比較（content_hash は読まないと出ないため使わない）。
    /// `None` = 無題バッファ / メタデータ取得失敗（削除など）。
    pub fn disk_probably_unchanged(&self) -> Option<bool> {
        let path = self.file.as_ref()?;
        let revision = self.file_revision.as_ref()?;
        let metadata = self.host.metadata(path).ok()?;
        Some(metadata.len == revision.len && metadata.modified_ns == revision.modified_ns)
    }

    /// ディスクから読み直す（外部変更の追従）。選択はバッファ長へクランプ、dirty は解除。
    /// undo 履歴はリセットする（外部変更を跨ぐ undo は嘘の状態を作るため）。
    pub fn reload(&mut self) -> Result<()> {
        let path = self
            .file
            .clone()
            .context("再読込先が未設定（無題バッファ）")?;
        let content = self
            .host
            .read_file(&path)
            .with_context(|| format!("再読込に失敗: {}", path.display()))?;
        let rope = Rope::from_reader(io::Cursor::new(&content.bytes))
            .with_context(|| format!("再読込内容を読めない: {}", path.display()))?;
        self.rope = rope;
        self.file_revision = Some(content.revision);
        self.history = History::default();
        self.last_change = None;
        self.version += 1;
        self.dirty = false;
        let selections = self.selections.clone();
        self.set_selections(selections); // 長さ・char 境界へクランプ
        Ok(())
    }

    // ── アクセサ ──

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// 選択を差し替える（char 境界・バッファ長にクリップ）。空なら先頭キャレット。
    pub fn set_selections(&mut self, selections: Vec<Selection>) {
        let len = self.rope.len();
        self.selections = if selections.is_empty() {
            vec![Selection::cursor(0)]
        } else {
            selections
                .into_iter()
                .map(|selection| Selection {
                    anchor: self.rope.floor_char_boundary(selection.anchor.min(len)),
                    head: self.rope.floor_char_boundary(selection.head.min(len)),
                })
                .collect()
        };
    }

    /// 読み取り専用にする（diff タブ・M11-9）。delete_backward/forward と insert は
    /// edit() を通るため、edit/edit_batch/undo/redo のガードで全編集が止まる。
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn path(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// 直近編集の (start, old, new) 列（変更前座標）。undo/redo/reload 直後は None。
    pub fn last_change(&self) -> Option<&[(usize, String, String)]> {
        self.last_change.as_deref()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// byte offset → UTF-16 offset（OS の IME/入力ハンドラは UTF-16 で話すため）。
    pub fn byte_to_utf16(&self, byte: usize) -> usize {
        self.rope.byte_to_utf16_idx(byte.min(self.rope.len()))
    }

    /// UTF-16 offset → byte offset（範囲外は末尾にクリップ）。
    pub fn utf16_to_byte(&self, utf16: usize) -> usize {
        let max = self.rope.byte_to_utf16_idx(self.rope.len());
        self.rope.utf16_to_byte_idx(utf16.min(max))
    }

    /// byte レンジのテキスト（char 境界・バッファ長にクリップ）。入力ハンドラの範囲取得に使う。
    pub fn text_range(&self, range: Range<usize>) -> String {
        let start = self
            .rope
            .floor_char_boundary(range.start.min(self.rope.len()));
        let end = self.rope.ceil_char_boundary(range.end.min(self.rope.len()));
        self.rope.slice(start.min(end)..start.max(end)).to_string()
    }

    /// 描画側が読む不変スナップショット（rope の clone は構造共有で安い）。
    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            rope: self.rope.clone(),
            selections: self.selections.clone(),
            version: self.version,
        }
    }

    // ── 内部 ──

    /// レンジ群を char 境界・バッファ長にクリップし、昇順・非重複に整える。
    fn normalize_ranges(&self, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
        let len = self.rope.len();
        let mut normalized: Vec<Range<usize>> = ranges
            .iter()
            .map(|range| {
                let start = self.rope.floor_char_boundary(range.start.min(len));
                let end = self.rope.ceil_char_boundary(range.end.min(len));
                start.min(end)..start.max(end)
            })
            .collect();
        normalized.sort_by_key(|range| range.start);

        let mut result: Vec<Range<usize>> = Vec::with_capacity(normalized.len());
        for range in normalized {
            match result.last() {
                Some(last) if range.start < last.end => continue, // 前と重なる → 捨てる
                _ => result.push(range),
            }
        }
        result
    }

    /// 前向き適用。edits は昇順・非重複（変更前座標）。挿入後の各キャレット位置を返す。
    fn apply_forward(rope: &mut Rope, edits: &[Edit]) -> Vec<usize> {
        let mut delta: isize = 0;
        let mut cursors = Vec::with_capacity(edits.len());
        for edit in edits {
            let start = (edit.start as isize + delta) as usize;
            rope.remove(start..start + edit.old.len());
            rope.insert(start, &edit.new);
            cursors.push(start + edit.new.len());
            delta += edit.new.len() as isize - edit.old.len() as isize;
        }
        cursors
    }

    /// 逆向き適用（undo）。各 Edit の new を old へ戻す。降順に処理して位置を保つ。
    fn apply_reverse(rope: &mut Rope, edits: &[Edit]) {
        let mut finals = Vec::with_capacity(edits.len());
        let mut delta: isize = 0;
        for edit in edits {
            finals.push((edit.start as isize + delta) as usize);
            delta += edit.new.len() as isize - edit.old.len() as isize;
        }
        for index in (0..edits.len()).rev() {
            let start = finals[index];
            rope.remove(start..start + edits[index].new.len());
            rope.insert(start, &edits[index].old);
        }
    }
}

/// 非同期保存の 1 回分（[`Buffer::prepare_save`] が写す）。`write` は背景スレッドで呼ぶ。
pub struct PendingSave {
    host: Arc<dyn Host>,
    path: PathBuf,
    text: String,
    condition: WriteCondition,
    /// 保存開始時のバッファ version（完了時に dirty を下ろしてよいかの判定に使う）。
    pub version: u64,
}

impl PendingSave {
    /// ディスクへ書き込む（ブロッキング・背景スレッドで呼ぶ）。競合時はエラー（上書きしない）。
    pub fn write(&self) -> Result<FileRevision> {
        self.host
            .write_file(&self.path, self.text.as_bytes(), self.condition.clone())
            .with_context(|| format!("書き込みに失敗: {}", self.path.display()))
    }
}

/// 単語境界のための文字クラス（1=識別子系・2=記号系）。空白は呼び出し側で除外。
fn char_class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// 括弧/クォートの自動ペア入力の分類（M10 所作）。判定はロジックのみ＝テスト可能。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairAction {
    /// 通常入力（ペア処理なし）。
    Insert,
    /// 開き括弧/クォート → `開き+閉じ` を挿入してキャレットを間に置く。
    Pair(char),
    /// 選択を `開き…閉じ` で囲む。
    Wrap(char),
    /// 直後が同じ閉じ文字 → 挿入せずキャレットだけ右へ（打ち抜け）。
    SkipOver,
}

/// 1 文字入力 `typed` のペア動作を決める。`selection_empty`・キャレット前後の文字から判定。
pub fn classify_pair_input(
    typed: char,
    selection_empty: bool,
    previous: Option<char>,
    next: Option<char>,
) -> PairAction {
    let closing_of = |open: char| match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    };
    let is_closer = matches!(typed, ')' | ']' | '}' | '"' | '\'' | '`');
    // 打ち抜け: 直後が同じ閉じ文字（クォートは開き=閉じなので同文字）。
    if selection_empty && is_closer && next == Some(typed) {
        return PairAction::SkipOver;
    }
    let Some(close) = closing_of(typed) else {
        return PairAction::Insert;
    };
    if !selection_empty {
        return PairAction::Wrap(close);
    }
    // クォートは単語の途中（前後が識別子）ではペアにしない（アポストロフィ・lifetime 対策）。
    if matches!(typed, '"' | '\'' | '`') {
        let word_adjacent = previous
            .map(|c| char_class(c) == 1 && !c.is_whitespace())
            .unwrap_or(false)
            || next.map(|c| c.is_alphanumeric()).unwrap_or(false);
        if word_adjacent {
            return PairAction::Insert;
        }
    }
    // 開き括弧: 直後が識別子ならペアにしない（既存コードの前に挿す時の邪魔防止）。
    if matches!(typed, '(' | '[' | '{') {
        if next
            .map(|c| c.is_alphanumeric() || c == '_')
            .unwrap_or(false)
        {
            return PairAction::Insert;
        }
    }
    PairAction::Pair(close)
}

/// 描画側が読む不変スナップショット。行アクセス・座標変換を提供する。
#[derive(Clone)]
pub struct BufferSnapshot {
    rope: Rope,
    selections: Vec<Selection>,
    version: u64,
}

impl BufferSnapshot {
    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len()
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// 行数（描画のため最低 1 行は保証する。空バッファでも 1 行）。
    pub fn line_count(&self) -> usize {
        self.rope.len_lines(LINE_TYPE).max(1)
    }

    /// 行のテキスト（末尾の改行 `\n` / `\r\n` は含めない）。描画側はこれを 1 行ずつ shape する。
    pub fn line_text(&self, row: usize) -> String {
        let Some(slice) = self.rope.get_line(row, LINE_TYPE) else {
            return String::new();
        };
        let mut text = slice.to_string();
        if text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
        text
    }

    /// 行の byte 長（改行を含めない）。
    pub fn line_len_bytes(&self, row: usize) -> usize {
        self.line_text(row).len()
    }

    /// byte offset → 行・列。
    pub fn byte_to_point(&self, byte: usize) -> Point {
        let byte = byte.min(self.rope.len());
        let row = self.rope.byte_to_line_idx(byte, LINE_TYPE);
        let line_start = self.rope.line_to_byte_idx(row, LINE_TYPE);
        Point::new(row, byte.saturating_sub(line_start))
    }

    /// 行・列 → byte offset（範囲外は行末・バッファ末にクリップ、char 境界へ丸め）。
    pub fn point_to_byte(&self, point: Point) -> usize {
        let row = point.row.min(self.line_count().saturating_sub(1));
        let line_start = self.rope.line_to_byte_idx(row, LINE_TYPE);
        let column = point.column.min(self.line_len_bytes(row));
        self.rope.floor_char_boundary(line_start + column)
    }

    /// byte をバッファ長・char 境界にクリップ。
    pub fn clip_offset(&self, byte: usize) -> usize {
        self.rope.floor_char_boundary(byte.min(self.rope.len()))
    }

    /// `byte` の直前の char 境界（左移動・Backspace 用）。
    pub fn prev_char_boundary(&self, byte: usize) -> usize {
        let byte = byte.min(self.rope.len());
        if byte == 0 {
            0
        } else {
            self.rope.floor_char_boundary(byte - 1)
        }
    }

    /// 直前の単語境界（⌥←）。空白を飛ばし、同クラス（識別子 or 記号）の連なりの先頭へ。
    /// 改行は空白扱い（行を跨いで前の単語末尾へ行ける）。
    pub fn prev_word_boundary(&self, offset: usize) -> usize {
        let offset = self.clip_offset(offset);
        // 直前 256 バイトの窓で十分（超長トークンは複数回押しで届く）。
        let window_start = self.clip_offset(offset.saturating_sub(256));
        let window = self.rope.slice(window_start..offset).to_string();
        let mut boundary = window.len();
        let mut chars = window.chars().rev().peekable();
        // 1) 空白を飛ばす
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                boundary -= c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        // 2) 最初の非空白のクラスの連なりを飛ばす
        let class = chars.peek().map(|&c| char_class(c));
        if let Some(class) = class {
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() && char_class(c) == class {
                    boundary -= c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
        window_start + boundary
    }

    /// 直後の単語境界（⌥→）。空白を飛ばし、次の単語（同クラスの連なり）の末尾へ。
    pub fn next_word_boundary(&self, offset: usize) -> usize {
        let offset = self.clip_offset(offset);
        let window_end = self.clip_offset((offset + 256).min(self.rope.len()));
        let window = self.rope.slice(offset..window_end).to_string();
        let mut boundary = 0usize;
        let mut chars = window.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                boundary += c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let class = chars.peek().map(|&c| char_class(c));
        if let Some(class) = class {
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() && char_class(c) == class {
                    boundary += c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
        offset + boundary
    }

    /// `offset` が乗っている単語（識別子クラスの連なり）のレンジ。単語上でなければ None。
    /// ⌘D の初回（キャレット位置の単語選択）に使う。
    pub fn word_range_at(&self, offset: usize) -> Option<Range<usize>> {
        let offset = self.clip_offset(offset);
        let window_start = self.clip_offset(offset.saturating_sub(256));
        let window_end = self.clip_offset((offset + 256).min(self.rope.len()));
        let window = self.rope.slice(window_start..window_end).to_string();
        let relative = offset - window_start;
        let is_word = |c: char| char_class(c) == 1 && !c.is_whitespace();
        // キャレットの直前 or 直後が単語文字であること。
        let after = window[relative..].chars().next();
        let before = window[..relative].chars().next_back();
        if !(after.map(&is_word).unwrap_or(false) || before.map(&is_word).unwrap_or(false)) {
            return None;
        }
        let mut start = relative;
        for c in window[..relative].chars().rev() {
            if is_word(c) {
                start -= c.len_utf8();
            } else {
                break;
            }
        }
        let mut end = relative;
        for c in window[relative..].chars() {
            if is_word(c) {
                end += c.len_utf8();
            } else {
                break;
            }
        }
        (start < end).then(|| window_start + start..window_start + end)
    }

    /// 行域 `[first_row, last_row]` の byte 範囲（末尾の改行を含む。最終行は改行なしのまま）。
    fn line_span_bytes(&self, first_row: usize, last_row: usize) -> Range<usize> {
        let start = self.point_to_byte(Point::new(first_row, 0));
        let end = if last_row + 1 < self.line_count() {
            self.point_to_byte(Point::new(last_row + 1, 0))
        } else {
            self.rope.len()
        };
        start..end
    }

    /// `byte` の直後の char 境界（右移動・Delete 用）。
    pub fn next_char_boundary(&self, byte: usize) -> usize {
        let len = self.rope.len();
        if byte >= len {
            len
        } else {
            self.rope.ceil_char_boundary(byte + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shirushi_editor_core_{}_{}.txt",
            tag,
            std::process::id()
        ))
    }

    #[test]
    fn insert_puts_text_and_moves_cursor() {
        let mut buffer = Buffer::new();
        buffer.insert("hello");
        assert_eq!(buffer.text(), "hello");
        assert_eq!(buffer.selections(), &[Selection::cursor(5)]);
        assert!(buffer.is_dirty());
    }

    #[test]
    fn insert_replaces_selection() {
        let mut buffer = Buffer::from_str("hello world");
        buffer.set_selections(vec![Selection::new(0, 5)]);
        buffer.insert("goodbye");
        assert_eq!(buffer.text(), "goodbye world");
        assert_eq!(buffer.selections(), &[Selection::cursor(7)]);
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut buffer = Buffer::new();
        buffer.insert("abc");
        buffer.insert("def");
        assert_eq!(buffer.text(), "abcdef");

        buffer.undo();
        assert_eq!(buffer.text(), "abc");
        buffer.undo();
        assert_eq!(buffer.text(), "");

        buffer.redo();
        assert_eq!(buffer.text(), "abc");
        buffer.redo();
        assert_eq!(buffer.text(), "abcdef");
    }

    #[test]
    fn undo_restores_selection() {
        let mut buffer = Buffer::from_str("hello");
        buffer.set_selections(vec![Selection::cursor(5)]);
        buffer.insert("!");
        assert_eq!(buffer.selections(), &[Selection::cursor(6)]);
        buffer.undo();
        assert_eq!(buffer.text(), "hello");
        assert_eq!(buffer.selections(), &[Selection::cursor(5)]);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut buffer = Buffer::new();
        buffer.insert("a");
        buffer.undo();
        buffer.insert("b");
        // redo スタックはクリアされている
        assert!(buffer.redo().is_none());
        assert_eq!(buffer.text(), "b");
    }

    #[test]
    fn multi_cursor_insert() {
        let mut buffer = Buffer::from_str("a.b.c");
        // 3 箇所のキャレット（0, 2, 4 の後ろ）に同時挿入
        buffer.set_selections(vec![
            Selection::cursor(1),
            Selection::cursor(3),
            Selection::cursor(5),
        ]);
        buffer.insert("!");
        assert_eq!(buffer.text(), "a!.b!.c!");
        // 各キャレットが挿入分ぶんずれる
        assert_eq!(
            buffer.selections(),
            &[
                Selection::cursor(2),
                Selection::cursor(5),
                Selection::cursor(8)
            ]
        );
        buffer.undo();
        assert_eq!(buffer.text(), "a.b.c");
    }

    #[test]
    fn backspace_deletes_previous_char() {
        let mut buffer = Buffer::from_str("abc");
        buffer.set_selections(vec![Selection::cursor(3)]);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "ab");
        assert_eq!(buffer.selections(), &[Selection::cursor(2)]);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut buffer = Buffer::from_str("abc");
        buffer.set_selections(vec![Selection::cursor(0)]);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "abc");
    }

    #[test]
    fn delete_forward_removes_next_char() {
        let mut buffer = Buffer::from_str("abc");
        buffer.set_selections(vec![Selection::cursor(0)]);
        buffer.delete_forward();
        assert_eq!(buffer.text(), "bc");
        assert_eq!(buffer.selections(), &[Selection::cursor(0)]);
    }

    #[test]
    fn backspace_respects_multibyte_boundary() {
        // "café" — é は 2 バイト（[3,5)）。末尾キャレットからの Backspace は é 全体を消す。
        let mut buffer = Buffer::from_str("café");
        assert_eq!(buffer.len_bytes(), 5);
        buffer.set_selections(vec![Selection::cursor(5)]);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "caf");
        assert_eq!(buffer.selections(), &[Selection::cursor(3)]);
    }

    #[test]
    fn japanese_insert_and_backspace() {
        let mut buffer = Buffer::new();
        buffer.insert("日本語");
        assert_eq!(buffer.text(), "日本語");
        // 各文字 3 バイト → 末尾は 9
        assert_eq!(buffer.selections(), &[Selection::cursor(9)]);
        buffer.delete_backward();
        assert_eq!(buffer.text(), "日本");
        assert_eq!(buffer.selections(), &[Selection::cursor(6)]);
    }

    #[test]
    fn snapshot_is_independent_of_later_edits() {
        let mut buffer = Buffer::from_str("first");
        buffer.set_selections(vec![Selection::cursor(5)]); // from_str はキャレットを先頭に置く
        let snapshot = buffer.snapshot();
        buffer.insert("!");
        assert_eq!(snapshot.text(), "first");
        assert_eq!(buffer.text(), "first!");
        assert_ne!(snapshot.version(), buffer.version());
    }

    #[test]
    fn line_access_strips_newlines_and_counts() {
        let snapshot = Buffer::from_str("one\ntwo\nthree").snapshot();
        assert_eq!(snapshot.line_count(), 3);
        assert_eq!(snapshot.line_text(0), "one");
        assert_eq!(snapshot.line_text(1), "two");
        assert_eq!(snapshot.line_text(2), "three");

        // 末尾改行つき → 末尾に空行が増える
        let trailing = Buffer::from_str("a\n").snapshot();
        assert_eq!(trailing.line_count(), 2);
        assert_eq!(trailing.line_text(1), "");
    }

    #[test]
    fn empty_buffer_has_one_line() {
        let snapshot = Buffer::new().snapshot();
        assert_eq!(snapshot.line_count(), 1);
        assert_eq!(snapshot.line_text(0), "");
    }

    #[test]
    fn point_and_byte_convert_both_ways() {
        let snapshot = Buffer::from_str("abc\nあい\nxyz").snapshot();
        // row 1 は "あい"（各 3 バイト）。列 3 = "い" の手前
        let point = Point::new(1, 3);
        let byte = snapshot.point_to_byte(point);
        assert_eq!(snapshot.byte_to_point(byte), point);
        // 行頭 "abc\n" = 4 バイト → row1 開始は 4
        assert_eq!(snapshot.point_to_byte(Point::new(1, 0)), 4);
        // 列が行末を超えたら行末にクリップ
        assert_eq!(snapshot.point_to_byte(Point::new(0, 999)), 3);
    }

    #[test]
    fn char_boundary_navigation() {
        let snapshot = Buffer::from_str("aあb").snapshot(); // a[0] あ[1..4] b[4]
        assert_eq!(snapshot.next_char_boundary(0), 1);
        assert_eq!(snapshot.next_char_boundary(1), 4); // あ を跨ぐ
        assert_eq!(snapshot.prev_char_boundary(4), 1); // あ の頭へ
        assert_eq!(snapshot.prev_char_boundary(0), 0);
        assert_eq!(snapshot.next_char_boundary(5), 5); // 末尾
    }

    #[test]
    fn save_and_reload_round_trip() {
        let path = temp_path("save");
        let mut buffer = Buffer::new();
        buffer.insert("保存テスト\nsecond line\n");
        assert!(buffer.is_dirty());
        buffer.save_as(&path).expect("保存できる");
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.path(), Some(path.as_path()));

        let reloaded = Buffer::from_file(&path).expect("再読み込みできる");
        assert_eq!(reloaded.text(), "保存テスト\nsecond line\n");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reload_follows_external_change_and_clamps_selection() {
        let path = temp_path("reload");
        std::fs::write(&path, "line one\nline two\n").unwrap();
        let mut buffer = Buffer::from_file(&path).unwrap();
        buffer.set_selections(vec![Selection::cursor(15)]);
        assert_eq!(buffer.disk_probably_unchanged(), Some(true));

        // 外部変更（短くなる）→ unchanged=false → reload で追従・選択はクランプ・dirty 解除
        std::thread::sleep(std::time::Duration::from_millis(5)); // mtime 粒度対策
        std::fs::write(&path, "short\n").unwrap();
        assert_eq!(buffer.disk_probably_unchanged(), Some(false));
        buffer.reload().expect("再読込できる");
        assert_eq!(buffer.text(), "short\n");
        assert!(!buffer.is_dirty());
        assert!(buffer.selections()[0].head <= buffer.len_bytes());
        // 履歴はリセット（外部変更前へは戻れない）
        assert!(buffer.undo().is_none());
        // 再読込後はディスクと同期している
        assert_eq!(buffer.disk_probably_unchanged(), Some(true));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_probably_unchanged_is_none_for_untitled_or_missing() {
        assert_eq!(Buffer::new().disk_probably_unchanged(), None);
        let path = temp_path("gone");
        std::fs::write(&path, "x").unwrap();
        let buffer = Buffer::from_file(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(buffer.disk_probably_unchanged(), None); // 削除済み → メタデータ取得失敗
    }

    #[test]
    fn prepare_and_complete_save_round_trip() {
        let path = temp_path("async-save");
        std::fs::write(&path, "before").unwrap();
        let mut buffer = Buffer::from_file(&path).unwrap();
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.insert("after");
        let pending = buffer.prepare_save().expect("パスがあるので保存できる");
        let revision = pending.write().expect("背景の書き込み相当");
        buffer.complete_save(revision, pending.version);
        assert!(!buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after");
        // 自分の保存 = 外部変更ではない（watch の「自分の保存イベントは無視」判定が成立する）。
        assert_eq!(buffer.disk_probably_unchanged(), Some(true));
        // 保存開始後に編集が入った場合は dirty が残る
        buffer.insert("!");
        let pending = buffer.prepare_save().unwrap();
        let revision = pending.write().unwrap();
        buffer.insert("?"); // 書き込み中の編集を模す
        buffer.complete_save(revision, pending.version);
        assert!(buffer.is_dirty());
        // 2 周目の保存で書き込み中の編集も載ってクリーンになる（自分の revision = 競合しない）。
        let pending = buffer.prepare_save().unwrap();
        let revision = pending.write().expect("自分の保存の続き = 競合ではない");
        buffer.complete_save(revision, pending.version);
        assert!(!buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after!?");
        // 無題は None
        assert!(Buffer::new().prepare_save().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn word_boundaries_move_over_words_and_punctuation() {
        let snapshot = Buffer::from_str("let foo_bar = baz();\n").snapshot();
        // "baz();" 末尾(20…改行手前) から ⌥← → "baz" の頭ではなく "();" の頭（記号クラス）
        assert_eq!(snapshot.prev_word_boundary(20), 17); // "();" の頭
        assert_eq!(snapshot.prev_word_boundary(17), 14); // "baz" の頭
        assert_eq!(snapshot.prev_word_boundary(14), 12); // "= " → "=" の頭
        assert_eq!(snapshot.prev_word_boundary(4), 0); // "let" の頭
                                                       // ⌥→
        assert_eq!(snapshot.next_word_boundary(0), 3); // "let" の後
        assert_eq!(snapshot.next_word_boundary(3), 11); // " foo_bar" の後
        assert_eq!(snapshot.next_word_boundary(11), 13); // " =" の後
                                                         // 日本語（識別子クラス）も 1 語で跨ぐ
        let jp = Buffer::from_str("こんにちは world").snapshot();
        assert_eq!(jp.next_word_boundary(0), 15); // こんにちは = 3byte×5
        assert_eq!(jp.prev_word_boundary(15), 0);
    }

    #[test]
    fn delete_word_backward_removes_previous_word() {
        let mut buffer = Buffer::from_str("hello world");
        buffer.set_selections(vec![Selection::cursor(11)]);
        buffer.delete_word_backward();
        assert_eq!(buffer.text(), "hello ");
        buffer.delete_word_backward();
        assert_eq!(buffer.text(), "");
    }

    #[test]
    fn move_lines_swaps_with_neighbors_and_survives_last_line() {
        let mut buffer = Buffer::from_str("aaa\nbbb\nccc");
        // bbb 行にキャレット → 下へ（最終行 ccc は改行なし）
        buffer.set_selections(vec![Selection::cursor(4)]);
        buffer.move_lines(true).expect("動く");
        assert_eq!(buffer.text(), "aaa\nccc\nbbb");
        // 端では no-op
        assert!(buffer.move_lines(true).is_none());
        // 上へ 2 回で先頭へ
        buffer.move_lines(false).unwrap();
        assert_eq!(buffer.text(), "aaa\nbbb\nccc");
        buffer.move_lines(false).unwrap();
        assert_eq!(buffer.text(), "bbb\naaa\nccc");
        assert!(buffer.move_lines(false).is_none());
        // undo 一発で戻る
        buffer.undo();
        assert_eq!(buffer.text(), "aaa\nbbb\nccc");
    }

    #[test]
    fn duplicate_and_delete_lines() {
        let mut buffer = Buffer::from_str("one\ntwo\nthree");
        buffer.set_selections(vec![Selection::cursor(5)]); // two 行
        buffer.duplicate_lines(true); // 下に複製・キャレットは上側（元位置）
        assert_eq!(buffer.text(), "one\ntwo\ntwo\nthree");
        assert_eq!(buffer.selections()[0].head, 5);
        buffer.undo();
        buffer.duplicate_lines(false); // 上に複製・キャレットは下側（元テキスト上）
        assert_eq!(buffer.text(), "one\ntwo\ntwo\nthree");
        assert_eq!(buffer.selections()[0].head, 9);
        buffer.undo();
        buffer.set_selections(vec![Selection::cursor(5)]);
        buffer.delete_lines();
        assert_eq!(buffer.text(), "one\nthree");
        // 最終行（改行なし）の削除
        let mut tail = Buffer::from_str("a\nb");
        tail.set_selections(vec![Selection::cursor(2)]);
        tail.delete_lines();
        assert_eq!(tail.text(), "a\n");
    }

    #[test]
    fn toggle_comment_adds_and_removes_prefix() {
        let mut buffer = Buffer::from_str("    let x = 1;\n\n    let y = 2;");
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.toggle_comment("//");
        assert_eq!(buffer.text(), "    // let x = 1;\n\n    // let y = 2;");
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.toggle_comment("//");
        assert_eq!(buffer.text(), "    let x = 1;\n\n    let y = 2;");
        // undo 一発（全行 = 1 Transaction）
        buffer.undo();
        assert_eq!(buffer.text(), "    // let x = 1;\n\n    // let y = 2;");
    }

    #[test]
    fn newline_inherits_indent_and_deepens_after_block_open() {
        let mut buffer = Buffer::from_str("    fn main() {");
        buffer.set_selections(vec![Selection::cursor(buffer.len_bytes())]);
        buffer.insert_newline_indented(4);
        assert_eq!(buffer.text(), "    fn main() {\n        ");
        // ブロック開始でない行は継承のみ
        let mut plain = Buffer::from_str("    let x = 1;");
        plain.set_selections(vec![Selection::cursor(plain.len_bytes())]);
        plain.insert_newline_indented(4);
        assert_eq!(plain.text(), "    let x = 1;\n    ");
    }

    #[test]
    fn indent_and_outdent_lines() {
        let mut buffer = Buffer::from_str("a\n    b\nc");
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.indent_lines(4);
        assert_eq!(buffer.text(), "    a\n        b\n    c");
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.outdent_lines(4);
        assert_eq!(buffer.text(), "a\n    b\nc");
        // これ以上外せない行は据え置き・undo 一発
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.outdent_lines(4);
        assert_eq!(buffer.text(), "a\nb\nc");
        buffer.undo();
        assert_eq!(buffer.text(), "a\n    b\nc");
    }

    #[test]
    fn pair_classification_covers_brackets_and_quotes() {
        use PairAction::*;
        // 開き括弧 → ペア（直後が識別子なら素通し）
        assert_eq!(classify_pair_input('(', true, None, None), Pair(')'));
        assert_eq!(
            classify_pair_input('(', true, Some('a'), Some(' ')),
            Pair(')')
        );
        assert_eq!(classify_pair_input('(', true, None, Some('x')), Insert);
        // 打ち抜け
        assert_eq!(
            classify_pair_input(')', true, Some('('), Some(')')),
            SkipOver
        );
        assert_eq!(
            classify_pair_input('"', true, Some('"'), Some('"')),
            SkipOver
        );
        // 選択があれば囲む
        assert_eq!(classify_pair_input('(', false, None, None), Wrap(')'));
        assert_eq!(classify_pair_input('"', false, None, None), Wrap('"'));
        // クォートは単語の途中でペアにしない（don't → don''t 事故防止）
        assert_eq!(
            classify_pair_input('\'', true, Some('n'), Some('t')),
            Insert
        );
        assert_eq!(classify_pair_input('"', true, Some(' '), None), Pair('"'));
        // 非ペア文字
        assert_eq!(classify_pair_input('a', true, None, None), Insert);
    }

    #[test]
    fn select_next_occurrence_grows_word_then_adds_matches() {
        let mut buffer = Buffer::from_str("foo bar foo baz foo");
        buffer.set_selections(vec![Selection::cursor(1)]); // "foo" の中
        assert!(buffer.select_next_occurrence()); // 単語選択
        assert_eq!(buffer.selections(), &[Selection::new(0, 3)]);
        assert!(buffer.select_next_occurrence()); // 2 個目
        assert_eq!(buffer.selections().len(), 2);
        assert!(buffer.select_next_occurrence()); // 3 個目
        assert_eq!(buffer.selections().len(), 3);
        assert!(!buffer.select_next_occurrence()); // 全部選択済み → false
                                                   // 3 箇所同時書き換え（受入: ⌘D×3 で同名 3 箇所を書き換えられる）
        buffer.insert("qux");
        assert_eq!(buffer.text(), "qux bar qux baz qux");
        // undo 一発で戻る
        buffer.undo();
        assert_eq!(buffer.text(), "foo bar foo baz foo");
    }

    #[test]
    fn add_cursor_vertically_clips_column_and_stops_at_edges() {
        let mut buffer = Buffer::from_str("long line here\nshort\nlong line again");
        buffer.set_selections(vec![Selection::cursor(10)]); // 1 行目 col10
        assert!(buffer.add_cursor_vertically(true)); // 2 行目 = short(5byte) → 行末クリップ
        assert_eq!(buffer.selections().len(), 2);
        assert_eq!(buffer.selections()[1].head, 15 + 5); // "short" 行末
        assert!(buffer.add_cursor_vertically(true)); // 3 行目 col10... 最下から
        assert_eq!(buffer.selections().len(), 3);
        assert!(!buffer.add_cursor_vertically(true)); // 最終行 → false
                                                      // 同時タイプが 3 箇所に入る
        buffer.insert("X");
        assert_eq!(buffer.text().matches('X').count(), 3);
    }

    #[test]
    fn collapse_and_add_cursor_at() {
        let mut buffer = Buffer::from_str("aaa\nbbb");
        buffer.set_selections(vec![Selection::cursor(0)]);
        buffer.add_cursor_at(4);
        assert_eq!(buffer.selections().len(), 2);
        buffer.add_cursor_at(4); // 重複は増えない
        assert_eq!(buffer.selections().len(), 2);
        assert!(buffer.collapse_to_primary());
        assert_eq!(buffer.selections().len(), 1);
        assert!(!buffer.collapse_to_primary());
    }

    #[test]
    fn edit_batch_applies_different_texts_in_one_transaction() {
        let mut buffer = Buffer::from_str("fn main(){let x=1;}");
        // フォーマット風: 2 箇所に異なるテキスト
        buffer.edit_batch(&[
            (9..10, " {\n    ".to_string()), // "{" → " {\n    "
            (18..19, "\n}".to_string()),     // 末尾 "}" 置換
        ]);
        assert_eq!(buffer.text(), "fn main() {\n    let x=1;\n}");
        // undo 一発で全部戻る
        buffer.undo();
        assert_eq!(buffer.text(), "fn main(){let x=1;}");
        // 重なりは先勝ち・空は no-op
        let mut buffer = Buffer::from_str("abcdef");
        buffer.edit_batch(&[(0..3, "X".to_string()), (2..5, "Y".to_string())]);
        assert_eq!(buffer.text(), "Xdef");
        buffer.edit_batch(&[]);
        assert_eq!(buffer.text(), "Xdef");
    }

    #[test]
    fn save_rejects_external_change_without_overwriting_it() {
        let path = temp_path("conflict");
        std::fs::write(&path, "original").unwrap();
        let mut buffer = Buffer::from_file(&path).unwrap();
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.insert("mine");

        std::fs::write(&path, "external").unwrap();
        let error = buffer.save().unwrap_err();
        assert!(format!("{error:#}").contains("保存競合"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.path(), Some(path.as_path()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn failed_save_as_keeps_original_file_binding() {
        let original = temp_path("save-as-original");
        std::fs::write(&original, "one").unwrap();
        let mut buffer = Buffer::from_file(&original).unwrap();
        buffer.insert("two");
        let invalid = original.join("child.txt");

        assert!(buffer.save_as(&invalid).is_err());
        assert_eq!(buffer.path(), Some(original.as_path()));
        assert!(buffer.is_dirty());
        let _ = std::fs::remove_file(&original);
    }

    #[test]
    fn utf16_conversions_handle_astral_and_bmp() {
        // "a" = 1 UTF-16, "あ" = 1 UTF-16 (3 バイト), "𝔸" = 2 UTF-16 (サロゲートペア・4 バイト)
        let buffer = Buffer::from_str("aあ𝔸");
        assert_eq!(buffer.len_bytes(), 1 + 3 + 4);
        assert_eq!(buffer.byte_to_utf16(0), 0);
        assert_eq!(buffer.byte_to_utf16(1), 1); // "a" の後
        assert_eq!(buffer.byte_to_utf16(4), 2); // "あ" の後
        assert_eq!(buffer.byte_to_utf16(8), 4); // "𝔸"(2 UTF-16) の後
                                                // 逆変換
        assert_eq!(buffer.utf16_to_byte(2), 4);
        assert_eq!(buffer.utf16_to_byte(4), 8);
        // 範囲外はクリップ
        assert_eq!(buffer.utf16_to_byte(999), 8);
    }

    #[test]
    fn save_without_path_errors() {
        let mut buffer = Buffer::new();
        buffer.insert("x");
        assert!(buffer.save().is_err());
    }

    // ── データ完全性（M13 公開準備: 「作業が消える」系だけはベータでも許されない） ──

    #[test]
    fn async_save_rejects_external_change_between_prepare_and_write() {
        let path = temp_path("async-conflict");
        std::fs::write(&path, "original").unwrap();
        let mut buffer = Buffer::from_file(&path).unwrap();
        buffer.set_selections(vec![Selection::new(0, buffer.len_bytes())]);
        buffer.insert("mine");

        let pending = buffer.prepare_save().unwrap();
        // prepare と write の間に外部変更（長さも変える = revision 確実に不一致）。
        std::fs::write(&path, "external change!").unwrap();

        let error = pending.write().unwrap_err();
        assert!(
            format!("{error:#}").contains("保存競合"),
            "競合エラーであること: {error:#}"
        );
        // 外部の内容は上書きされず、バッファは dirty のまま = どちらの作業も消えていない。
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external change!");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.text(), "mine");
        // 警告バーの「再読込」相当 → 以後は普通に保存できる。
        buffer.reload().unwrap();
        assert_eq!(buffer.text(), "external change!");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reload_on_deleted_file_errors_and_keeps_buffer() {
        let path = temp_path("reload-deleted");
        std::fs::write(&path, "keep me").unwrap();
        let mut buffer = Buffer::from_file(&path).unwrap();
        buffer.set_selections(vec![Selection::cursor(buffer.len_bytes())]);
        buffer.insert("!");
        std::fs::remove_file(&path).unwrap();

        // 再読込は失敗するが、未保存の編集は失われない（dirty のまま保持）。
        assert!(buffer.reload().is_err());
        assert_eq!(buffer.text(), "keep me!");
        assert!(buffer.is_dirty());
        // save は消えたパスへ書き戻せる（新規作成扱い）= 作業を救出できる。
        buffer.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me!");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_as_binds_conflict_detection_to_new_file() {
        let original = temp_path("save-as-bind-a");
        let target = temp_path("save-as-bind-b");
        std::fs::write(&original, "one").unwrap();
        let mut buffer = Buffer::from_file(&original).unwrap();
        buffer.set_selections(vec![Selection::cursor(3)]);
        buffer.insert(" two");
        buffer.save_as(&target).unwrap();
        assert!(!buffer.is_dirty());

        // 以後の競合検知は新しいファイル（target）に対して働く。
        buffer.insert(" three");
        std::fs::write(&target, "external on target!!").unwrap();
        let error = buffer.save().unwrap_err();
        assert!(format!("{error:#}").contains("保存競合"));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "external on target!!"
        );
        // 元ファイルは save_as 以降ノータッチ。
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "one");
        let _ = std::fs::remove_file(&original);
        let _ = std::fs::remove_file(&target);
    }

    // ── 性能計測（通常テストでは ignore。`cargo test -p editor_core --release -- --ignored --nocapture`）──

    #[test]
    #[ignore = "perf 計測用"]
    fn bench_insert_and_undo_throughput() {
        use std::time::Instant;
        let operations = 100_000;
        let mut buffer = Buffer::new();

        let start = Instant::now();
        for _ in 0..operations {
            buffer.insert("x");
        }
        let insert_elapsed = start.elapsed();

        let start = Instant::now();
        for _ in 0..operations {
            buffer.undo();
        }
        let undo_elapsed = start.elapsed();

        eprintln!(
            "insert: {:.0} ops/s ({:.1}ms/{}回) / undo: {:.0} ops/s ({:.1}ms)",
            operations as f64 / insert_elapsed.as_secs_f64(),
            insert_elapsed.as_secs_f64() * 1000.0,
            operations,
            operations as f64 / undo_elapsed.as_secs_f64(),
            undo_elapsed.as_secs_f64() * 1000.0,
        );
        assert_eq!(buffer.text(), "");
    }

    #[test]
    #[ignore = "perf 計測用"]
    fn bench_edit_on_large_buffer() {
        use std::time::Instant;
        let line = "The quick brown fox jumps over the lazy dog 日本語混在。\n";
        let text = line.repeat(15_000); // ~1MB+
        let mut buffer = Buffer::from_str(&text);
        let len = buffer.len_bytes();
        buffer.set_selections(vec![Selection::cursor(len / 2)]);

        let cycles = 10_000;
        let start = Instant::now();
        for _ in 0..cycles {
            buffer.insert("あ");
            buffer.delete_backward();
        }
        let elapsed = start.elapsed();

        eprintln!(
            "large buffer ({} bytes): {:.0} edit-cycles/s ({:.1}ms/{}回)",
            len,
            cycles as f64 / elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0,
            cycles,
        );
    }
}
