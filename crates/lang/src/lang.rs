//! lang — tree-sitter による構文ハイライト（M7）。GPUI 非依存・テスト可能。
//!
//! ROADMAP M7「tree-sitter ハイライト（Rust から。theme の syn-* トークンに接続）」。
//! 出力は**非重複・start 昇順**の [`HighlightSpan`] 列。色は持たず [`HighlightKind`] だけを返し、
//! 描画側（editor_view）が theme_core の syn-* にマップする（= この crate は theme を知らない）。

use anyhow::{Context as _, Result};
use std::ops::Range;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

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
    /// Rust 用ハイライタ。
    pub fn rust() -> Result<Highlighter> {
        let language = tree_sitter_rust::LANGUAGE.into();
        let mut config = HighlightConfiguration::new(
            language,
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .context("Rust ハイライトクエリのコンパイルに失敗")?;
        config.configure(HIGHLIGHT_NAMES);
        Ok(Highlighter { config })
    }

    /// 拡張子から対応ハイライタを選ぶ（今は Rust のみ）。
    pub fn for_extension(extension: &str) -> Option<Highlighter> {
        match extension {
            "rs" => Highlighter::rust().ok(),
            _ => None,
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
