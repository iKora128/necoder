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
        Self { anchor: offset, head: offset }
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
        }
    }

    /// 文字列から無題バッファを作る（主にテスト用）。
    pub fn from_str(text: &str) -> Buffer {
        Buffer { rope: Rope::from_str(text), ..Buffer::new() }
    }

    /// ファイルを読み込んで開く。
    pub fn from_file(path: impl AsRef<Path>) -> Result<Buffer> {
        Self::from_host(LocalHost::shared(), path)
    }

    /// local/remote 共通の Host からファイルを読み込んで開く。
    pub fn from_host(host: Arc<dyn Host>, path: impl AsRef<Path>) -> Result<Buffer> {
        let path = path.as_ref();
        let content = host
            .read_file(path)
            .with_context(|| format!("ファイルを開けない: {}", path.display()))?;
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
        let transaction = self.history.redo.pop()?;
        Self::apply_forward(&mut self.rope, &transaction.edits);
        self.selections = transaction.after.clone();
        self.version += 1;
        self.dirty = true;
        let id = transaction.id;
        self.history.undo.push(transaction);
        Some(id)
    }

    // ── 保存 ──

    /// 開いているファイルへ保存する。無題なら [`Buffer::save_as`] を使う。
    pub fn save(&mut self) -> Result<()> {
        let path = self.file.clone().context("保存先が未設定（無題バッファ）")?;
        self.save_to(&path)
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

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn path(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
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
        let start = self.rope.floor_char_boundary(range.start.min(self.rope.len()));
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
        std::env::temp_dir().join(format!("shirushi_editor_core_{}_{}.txt", tag, std::process::id()))
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
            &[Selection::cursor(2), Selection::cursor(5), Selection::cursor(8)]
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
