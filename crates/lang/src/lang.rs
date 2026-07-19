//! lang — tree-sitter による構文ハイライト（M7）。GPUI 非依存・テスト可能。
//!
//! ROADMAP M7「tree-sitter ハイライト（Rust から。theme の syn-* トークンに接続）」。
//! 出力は**非重複・start 昇順**の [`HighlightSpan`] 列。色は持たず [`HighlightKind`] だけを返し、
//! 描画側（editor_view）が theme_core の syn-* にマップする（= この crate は theme を知らない）。

use anyhow::{Context as _, Result};
use std::ops::Range;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

/// LSP クライアント（rust-analyzer）。M7。
pub mod lsp;

/// syn-* トークンに対応する種別（UI-SPEC §1.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Macro,
    Punctuation,
}

/// byte レンジ + 種別。非重複・start 昇順で返る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// tree-sitter の capture 名。`configure` の順が [`HighlightEvent`] の index になる。
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "operator",
    "property",
    "punctuation.bracket",
    "punctuation.delimiter",
    "string",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

fn kind_for_name(name: &str) -> Option<HighlightKind> {
    let base = name.split('.').next().unwrap_or(name);
    Some(match base {
        "keyword" => HighlightKind::Keyword,
        "function" if name.contains("macro") => HighlightKind::Macro,
        "function" => HighlightKind::Function,
        "type" | "constructor" => HighlightKind::Type,
        "string" | "escape" => HighlightKind::String,
        "constant" => HighlightKind::Number,
        "comment" => HighlightKind::Comment,
        "attribute" => HighlightKind::Macro,
        "operator" | "punctuation" => HighlightKind::Punctuation,
        _ => return None, // variable / property / label 等は既定色
    })
}

/// 1 言語分のハイライタ。クエリのコンパイルは 1 回（`new`）。
pub struct Highlighter {
    config: HighlightConfiguration,
}

impl Highlighter {
    /// 任意言語のハイライタを組む共通コンストラクタ。
    fn with_query(
        language: tree_sitter::Language,
        name: &'static str,
        highlights_query: &str,
        injections_query: &str,
        locals_query: &str,
    ) -> Result<Highlighter> {
        let mut config = HighlightConfiguration::new(
            language,
            name,
            highlights_query,
            injections_query,
            locals_query,
        )
        .with_context(|| format!("{name} ハイライトクエリのコンパイルに失敗"))?;
        config.configure(HIGHLIGHT_NAMES);
        Ok(Highlighter { config })
    }

    /// Rust 用ハイライタ。
    pub fn rust() -> Result<Highlighter> {
        Self::with_query(
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            "",
            "",
        )
    }

    /// 拡張子から対応ハイライタを選ぶ（M11 多言語）。クエリのコンパイル失敗は None（無ハイライトで開く）。
    pub fn for_extension(extension: &str) -> Option<Highlighter> {
        let result = match extension {
            "rs" => Highlighter::rust(),
            "js" | "mjs" | "cjs" | "jsx" => Self::with_query(
                tree_sitter_javascript::LANGUAGE.into(),
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            // TS/TSX は JS のクエリ + TS 差分クエリを連結（tree-sitter-typescript の流儀）。
            "ts" => Self::with_query(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
                &format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
                "",
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            "tsx" => Self::with_query(
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                "tsx",
                &format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
                "",
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            "py" => Self::with_query(
                tree_sitter_python::LANGUAGE.into(),
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            "go" => Self::with_query(
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            "json" | "jsonc" => Self::with_query(
                tree_sitter_json::LANGUAGE.into(),
                "json",
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            "yml" | "yaml" => Self::with_query(
                tree_sitter_yaml::LANGUAGE.into(),
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            "toml" => Self::with_query(
                tree_sitter_toml_ng::LANGUAGE.into(),
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            "html" | "htm" => Self::with_query(
                tree_sitter_html::LANGUAGE.into(),
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY,
                tree_sitter_html::INJECTIONS_QUERY,
                "",
            ),
            "css" => Self::with_query(
                tree_sitter_css::LANGUAGE.into(),
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            _ => return None,
        };
        match result {
            Ok(highlighter) => Some(highlighter),
            Err(error) => {
                eprintln!("ハイライタ初期化に失敗（無ハイライトで継続）: {error:#}");
                None
            }
        }
    }

    /// テキストをハイライトし、非重複・start 昇順の span 列を返す。
    pub fn highlight(&self, text: &str) -> Vec<HighlightSpan> {
        let mut highlighter = TsHighlighter::new();
        let events = match highlighter.highlight(&self.config, text.as_bytes(), None, |_| None) {
            Ok(events) => events,
            Err(_) => return Vec::new(),
        };

        let mut spans = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for event in events {
            let Ok(event) = event else { continue };
            match event {
                HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if start >= end {
                        continue;
                    }
                    if let Some(&index) = stack.last() {
                        if let Some(name) = HIGHLIGHT_NAMES.get(index) {
                            if let Some(kind) = kind_for_name(name) {
                                // 直前 span と隣接・同種なら連結
                                match spans.last_mut() {
                                    Some(HighlightSpan { range, kind: last_kind })
                                        if *last_kind == kind && range.end == start =>
                                    {
                                        range.end = end;
                                    }
                                    _ => spans.push(HighlightSpan { range: start..end, kind }),
                                }
                            }
                        }
                    }
                }
            }
        }
        spans
    }
}

/// `highlights`（非重複・start 昇順）のうち `[start, end)` と重なる部分を返す。
pub fn spans_in_range(highlights: &[HighlightSpan], start: usize, end: usize) -> &[HighlightSpan] {
    let first = highlights.partition_point(|span| span.range.end <= start);
    let last = highlights.partition_point(|span| span.range.start < end);
    &highlights[first..last]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_keywords_strings_comments() {
        let highlighter = Highlighter::rust().expect("Rust ハイライタが作れる");
        let source = "fn main() {\n    let x = \"hi\"; // c\n}\n";
        let spans = highlighter.highlight(source);
        assert!(!spans.is_empty(), "span が出る");

        // start 昇順・非重複
        for pair in spans.windows(2) {
            assert!(pair[0].range.start <= pair[1].range.start);
            assert!(pair[0].range.end <= pair[1].range.start, "非重複");
        }

        let kind_of = |needle: &str| {
            let at = source.find(needle).unwrap();
            spans
                .iter()
                .find(|span| span.range.start <= at && at < span.range.end)
                .map(|span| span.kind)
        };
        assert_eq!(kind_of("fn"), Some(HighlightKind::Keyword));
        assert_eq!(kind_of("let"), Some(HighlightKind::Keyword));
        assert_eq!(kind_of("\"hi\""), Some(HighlightKind::String));
        assert_eq!(kind_of("// c"), Some(HighlightKind::Comment));
    }

    #[test]
    fn for_extension_selects_rust_only() {
        assert!(Highlighter::for_extension("rs").is_some());
        assert!(Highlighter::for_extension("txt").is_none());
    }

    #[test]
    fn spans_in_range_filters() {
        let spans = vec![
            HighlightSpan { range: 0..3, kind: HighlightKind::Keyword },
            HighlightSpan { range: 5..8, kind: HighlightKind::String },
            HighlightSpan { range: 10..12, kind: HighlightKind::Comment },
        ];
        let slice = spans_in_range(&spans, 4, 9);
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0].kind, HighlightKind::String);
    }
}

/// 拡張子 → (言語, highlights クエリ)。[`Highlighter::for_extension`] と
/// [`IncrementalHighlighter::for_extension`] が共有する言語表（M11）。
fn language_and_query(extension: &str) -> Option<(tree_sitter::Language, String)> {
    let pair: (tree_sitter::Language, String) = match extension {
        "rs" => (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY.to_string()),
        "js" | "mjs" | "cjs" | "jsx" => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY.to_string(),
        ),
        "ts" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        "tsx" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        "py" => (tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY.to_string()),
        "go" => (tree_sitter_go::LANGUAGE.into(), tree_sitter_go::HIGHLIGHTS_QUERY.to_string()),
        "json" | "jsonc" => {
            (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.to_string())
        }
        "yml" | "yaml" => {
            (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.to_string())
        }
        "toml" => {
            (tree_sitter_toml_ng::LANGUAGE.into(), tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string())
        }
        "html" | "htm" => {
            (tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY.to_string())
        }
        "css" => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.to_string()),
        _ => return None,
    };
    Some(pair)
}

/// 増分ハイライタ（M11-8）。Tree を保持し、編集は `Tree::edit` + 差分パース。
/// span 抽出は**可視 byte 範囲だけ** QueryCursor で走る（巨大ファイルでも描画コスト一定）。
/// 512KB 上限は不要になった（全文パースは開いた時と undo/redo 後だけ）。
pub struct IncrementalHighlighter {
    parser: tree_sitter::Parser,
    query: tree_sitter::Query,
    /// capture index → HighlightKind（対応の無い capture は None = 塗らない）。
    capture_kinds: Vec<Option<HighlightKind>>,
    tree: Option<tree_sitter::Tree>,
}

impl IncrementalHighlighter {
    pub fn for_extension(extension: &str) -> Option<IncrementalHighlighter> {
        let (language, query_source) = language_and_query(extension)?;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).ok()?;
        let query = match tree_sitter::Query::new(&language, &query_source) {
            Ok(query) => query,
            Err(error) => {
                eprintln!("{extension} のハイライトクエリが不正（無ハイライトで継続）: {error}");
                return None;
            }
        };
        let capture_kinds = query.capture_names().iter().map(|name| kind_for_name(name)).collect();
        Some(IncrementalHighlighter { parser, query, capture_kinds, tree: None })
    }

    /// 全文パース（開いた直後・undo/redo・複数編集のフォールバック）。
    pub fn reparse_full(&mut self, text: &str) {
        self.tree = self.parser.parse(text, None);
    }

    /// 単一編集の増分パース。`(start, old, new)` は**変更前座標**の 1 置換。
    /// Tree が無い（初回・直前の失敗）場合は全文へフォールバック。
    pub fn apply_edit(&mut self, text: &str, start: usize, old: &str, new: &str) {
        let Some(tree) = self.tree.as_mut() else {
            self.reparse_full(text);
            return;
        };
        // start までのテキストは編集前後で同一 → 新テキストから start の Point を得る。
        let start_point = point_at(text, start);
        let edit = tree_sitter::InputEdit {
            start_byte: start,
            old_end_byte: start + old.len(),
            new_end_byte: start + new.len(),
            start_position: start_point,
            old_end_position: advance_point(start_point, old),
            new_end_position: advance_point(start_point, new),
        };
        tree.edit(&edit);
        self.tree = self.parser.parse(text, self.tree.as_ref());
    }

    /// 可視 byte 範囲の span（非重複・start 昇順）。範囲外にはみ出す capture はクリップ。
    pub fn spans(&self, text: &str, range: Range<usize>) -> Vec<HighlightSpan> {
        use tree_sitter::StreamingIterator as _;
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        let mut cursor = tree_sitter::QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut collected: Vec<HighlightSpan> = Vec::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), text.as_bytes());
        while let Some(found) = matches.next() {
            for capture in found.captures {
                let Some(kind) = self.capture_kinds[capture.index as usize] else {
                    continue;
                };
                let node_range = capture.node.byte_range();
                let start = node_range.start.max(range.start);
                let end = node_range.end.min(range.end);
                if start < end {
                    collected.push(HighlightSpan { range: start..end, kind });
                }
            }
        }
        // start 昇順・重なりは先勝ち（既存エンジンの「非重複・昇順」不変条件を守る）。
        collected.sort_by_key(|span| (span.range.start, span.range.end));
        let mut result: Vec<HighlightSpan> = Vec::with_capacity(collected.len());
        for span in collected {
            match result.last() {
                Some(last) if span.range.start < last.range.end => continue,
                _ => result.push(span),
            }
        }
        result
    }
}

/// テキスト内 byte offset の (row, column) Point。
fn point_at(text: &str, offset: usize) -> tree_sitter::Point {
    let prefix = &text[..offset.min(text.len())];
    let row = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let column = prefix.rfind('\n').map(|last| offset - last - 1).unwrap_or(offset);
    tree_sitter::Point::new(row, column)
}

/// Point から文字列 `s` の分だけ進めた Point。
fn advance_point(start: tree_sitter::Point, s: &str) -> tree_sitter::Point {
    let newlines = s.bytes().filter(|byte| *byte == b'\n').count();
    if newlines == 0 {
        tree_sitter::Point::new(start.row, start.column + s.len())
    } else {
        let tail = s.rfind('\n').map(|last| s.len() - last - 1).unwrap_or(0);
        tree_sitter::Point::new(start.row + newlines, tail)
    }
}

/// アウトラインの 1 項目（⌘⇧O・M11）。tree-sitter クエリ駆動 = LSP 不要で全対応言語に効く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub name: String,
    /// 種別の短いラベル（fn/struct/enum/trait/impl/mod/const）。
    pub kind: &'static str,
    /// 0 始まりの行番号。
    pub row: usize,
}

/// テキストのアウトラインを返す（対応言語のみ・現状 Rust）。定義順。
pub fn outline(text: &str, extension: &str) -> Vec<OutlineItem> {
    match extension {
        "rs" => outline_rust(text),
        _ => Vec::new(),
    }
}

fn outline_rust(text: &str) -> Vec<OutlineItem> {
    use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator as _};
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    // name capture の順に (クエリ, 種別) を並べる。impl は型名を名前として使う。
    const QUERY: &str = r#"
        (function_item name: (identifier) @fn)
        (struct_item name: (type_identifier) @struct)
        (enum_item name: (type_identifier) @enum)
        (trait_item name: (type_identifier) @trait)
        (impl_item type: (type_identifier) @impl)
        (mod_item name: (identifier) @mod)
        (const_item name: (identifier) @const)
        (static_item name: (identifier) @const)
    "#;
    let Ok(query) = Query::new(&language, QUERY) else {
        return Vec::new();
    };
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut items = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    while let Some(found) = matches.next() {
        for capture in found.captures {
            let node = capture.node;
            let Ok(name) = node.utf8_text(text.as_bytes()) else {
                continue;
            };
            let kind: &'static str = match names[capture.index as usize] {
                "fn" => "fn",
                "struct" => "struct",
                "enum" => "enum",
                "trait" => "trait",
                "impl" => "impl",
                "mod" => "mod",
                "const" => "const",
                _ => "•",
            };
            items.push(OutlineItem {
                name: name.to_string(),
                kind,
                row: node.start_position().row,
            });
        }
    }
    items.sort_by_key(|item| item.row);
    items
}

/// 拡張子 → 行コメント接頭辞（⌘/ トグル用の最小表。M11 の言語表に統合予定）。
/// ブロックコメントのみの言語（html/css/md）は v1 非対応 = None。
pub fn comment_prefix(extension: &str) -> Option<&'static str> {
    match extension {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "c" | "h" | "cpp" | "cc" | "cxx"
        | "hpp" | "go" | "java" | "swift" | "kt" | "json" | "jsonc" => Some("//"),
        "py" | "sh" | "zsh" | "bash" | "rb" | "yml" | "yaml" | "toml" => Some("#"),
        "sql" | "lua" => Some("--"),
        _ => None,
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;

    #[test]
    fn incremental_matches_full_reparse_after_edits() {
        let mut incremental = IncrementalHighlighter::for_extension("rs").unwrap();
        let v1 = "fn main() { let x = 1; }";
        incremental.reparse_full(v1);
        let full_spans = |text: &str| {
            let mut fresh = IncrementalHighlighter::for_extension("rs").unwrap();
            fresh.reparse_full(text);
            fresh.spans(text, 0..text.len())
        };
        assert_eq!(incremental.spans(v1, 0..v1.len()), full_spans(v1));

        // "1" → "\"hello\"" の単一編集（変更前座標 start=22, old="1"）
        let v2 = "fn main() { let x = \"hello\"; }";
        incremental.apply_edit(v2, 20, "1", "\"hello\"");
        assert_eq!(incremental.spans(v2, 0..v2.len()), full_spans(v2));

        // 改行を跨ぐ編集
        let v3 = "fn main() { let x = \"hello\";\n    let y = 2; }";
        incremental.apply_edit(v3, 28, "", "\n    let y = 2;");
        assert_eq!(incremental.spans(v3, 0..v3.len()), full_spans(v3));

        // 可視範囲クエリ = 範囲内だけ返る
        let ranged = incremental.spans(v3, 0..10);
        assert!(ranged.iter().all(|span| span.range.end <= 10));
        assert!(!ranged.is_empty());
    }

    #[test]
    fn point_helpers_count_rows_and_columns() {
        let text = "ab\ncde\nf";
        assert_eq!(point_at(text, 0), tree_sitter::Point::new(0, 0));
        assert_eq!(point_at(text, 4), tree_sitter::Point::new(1, 1));
        assert_eq!(point_at(text, 8), tree_sitter::Point::new(2, 1)); // "f" の後
        assert_eq!(advance_point(tree_sitter::Point::new(1, 2), "xy"), tree_sitter::Point::new(1, 4));
        assert_eq!(advance_point(tree_sitter::Point::new(1, 2), "x\nyz"), tree_sitter::Point::new(2, 2));
    }
}

#[cfg(test)]
mod multilang_tests {
    use super::*;

    #[test]
    fn highlighters_compile_and_produce_spans_for_all_languages() {
        let samples: &[(&str, &str)] = &[
            ("rs", "fn main() { let x = 1; }"),
            ("js", "function f(a) { return a + 1; }"),
            ("ts", "function f(a: number): number { return a; }"),
            ("tsx", "const x = <div className=\"a\">hi</div>;"),
            ("py", "def f(a):\n    return a + 1\n"),
            ("go", "package main\nfunc main() { x := 1 }\n"),
            ("json", "{ \"key\": [1, 2, true] }"),
            ("yml", "key: value\nlist:\n  - 1\n"),
            ("toml", "[table]\nkey = \"value\"\n"),
            ("html", "<html><body class=\"x\">hi</body></html>"),
            ("css", ".cls { color: red; }"),
        ];
        for (extension, sample) in samples {
            let highlighter = Highlighter::for_extension(extension)
                .unwrap_or_else(|| panic!("{extension} のハイライタが作れない"));
            let spans = highlighter.highlight(sample);
            assert!(!spans.is_empty(), "{extension} の span が空");
            // 非重複・昇順の不変条件
            for pair in spans.windows(2) {
                assert!(pair[0].range.end <= pair[1].range.start, "{extension} で重複 span");
            }
        }
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    #[test]
    fn rust_outline_lists_definitions_in_order() {
        let text = "mod helpers;\n\npub struct Thing { x: u32 }\n\nimpl Thing {\n    pub fn new() -> Self { Self { x: 0 } }\n}\n\nfn main() {}\n";
        let items = outline(text, "rs");
        let summary: Vec<(&str, &str, usize)> =
            items.iter().map(|item| (item.kind, item.name.as_str(), item.row)).collect();
        assert_eq!(
            summary,
            vec![
                ("mod", "helpers", 0),
                ("struct", "Thing", 2),
                ("impl", "Thing", 4),
                ("fn", "new", 5),
                ("fn", "main", 8),
            ]
        );
        // 非対応言語は空
        assert!(outline(text, "md").is_empty());
    }
}
