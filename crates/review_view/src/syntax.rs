//! 全文のハイライトを行ごとに割る（背景スレッドで計算する純関数）。
//!
//! diff の行だけを解析すると文脈が欠けて色が崩れる（複数行コメント・文字列の途中から始まる）ので、
//! 旧側・新側の**全文**を既存の `lang` ハイライタに通し、その結果を行へ配る。

use lang::{HighlightKind, HighlightSpan, IncrementalHighlighter};
use std::ops::Range;
use std::path::Path;

/// 1 つの全文を行に割ったもの（行の本文と、行内 byte 範囲の色種別）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HighlightedText {
    /// 行の本文（改行・CR は含まない）。
    pub lines: Vec<String>,
    /// `lines` と同じ添字の、行内 byte 範囲 + 種別（非重複・昇順）。
    pub spans: Vec<Vec<(Range<usize>, HighlightKind)>>,
}

impl HighlightedText {
    /// 1 始まりの行番号で本文と色を引く。
    pub fn line(&self, number: u32) -> Option<(&str, &[(Range<usize>, HighlightKind)])> {
        let index = (number as usize).checked_sub(1)?;
        let text = self.lines.get(index)?;
        let spans = self.spans.get(index).map(Vec::as_slice).unwrap_or(&[]);
        Some((text.as_str(), spans))
    }
}

/// 旧側・新側の全文のハイライト（読めなかった側は `None`）。
#[derive(Clone, Debug, Default)]
pub struct FileSyntax {
    pub old: Option<HighlightedText>,
    pub new: Option<HighlightedText>,
}

/// `path` の拡張子で言語を決めて全文をハイライトし、行に割る。対応言語が無ければ色無しで行だけ。
pub fn highlight_text(path: &Path, text: &str) -> HighlightedText {
    let spans = IncrementalHighlighter::for_path(path)
        .map(|mut highlighter| {
            highlighter.reparse_full(text);
            highlighter.spans(text, 0..text.len())
        })
        .unwrap_or_default();
    split_highlights(text, &spans)
}

/// 全文の span（非重複・昇順の byte 範囲）を行ごとに切り分ける。行をまたぐ span（複数行コメント等）は
/// 各行の範囲に切り詰める。
pub fn split_highlights(text: &str, spans: &[HighlightSpan]) -> HighlightedText {
    let mut result = HighlightedText::default();
    if text.is_empty() {
        return result;
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut offset = 0;
    let mut first_span = 0;
    for raw in body.split('\n') {
        let start = offset;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let end = start + line.len();
        while first_span < spans.len() && spans[first_span].range.end <= start {
            first_span += 1;
        }
        let mut line_spans = Vec::new();
        for span in spans[first_span..]
            .iter()
            .take_while(|span| span.range.start < end)
        {
            let from = span.range.start.max(start) - start;
            let to = span.range.end.min(end) - start;
            if from < to {
                line_spans.push((from..to, span.kind));
            }
        }
        result.lines.push(line.to_string());
        result.spans.push(line_spans);
        offset = start + raw.len() + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_split_per_line_and_clipped() {
        let text = "a /* x\ny */ b\r\nc\n";
        let spans = vec![
            HighlightSpan {
                range: 2..11,
                kind: HighlightKind::Comment,
            },
            HighlightSpan {
                range: 15..16,
                kind: HighlightKind::Keyword,
            },
        ];
        let split = split_highlights(text, &spans);
        assert_eq!(split.lines, vec!["a /* x", "y */ b", "c"]);
        assert_eq!(split.spans[0], vec![(2..6, HighlightKind::Comment)]);
        assert_eq!(split.spans[1], vec![(0..4, HighlightKind::Comment)]);
        assert_eq!(split.spans[2], vec![(0..1, HighlightKind::Keyword)]);
        assert_eq!(split.line(2).map(|(text, _)| text), Some("y */ b"));
        assert_eq!(split.line(0), None);
    }

    #[test]
    fn rust_text_is_highlighted_per_line() {
        let highlighted =
            highlight_text(Path::new("x.rs"), "fn main() {\n    let s = \"hi\";\n}\n");
        assert_eq!(highlighted.lines.len(), 3);
        assert!(highlighted.spans[0]
            .iter()
            .any(|(range, kind)| *kind == HighlightKind::Keyword && *range == (0..2)));
        assert!(highlighted.spans[1]
            .iter()
            .any(|(_, kind)| *kind == HighlightKind::String));
        let plain = highlight_text(Path::new("notes.unknown"), "a\nb");
        assert_eq!(plain.lines, vec!["a", "b"]);
        assert!(plain.spans.iter().all(Vec::is_empty));
    }
}
