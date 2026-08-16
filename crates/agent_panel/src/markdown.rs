//! markdown — チャット本文（Agent 発話）の markdown 解析（M4）。
//!
//! パーサは permissive な **pulldown-cmark**（MIT・CommonMark+GFM）を借り、その**イベント列を
//! GPUI 非依存の「ブロックモデル」へ簡約**する。描画（GPUI 要素化）は agent_panel 側の責務で、
//! ブロック毎に `push_selectable` へ載せて選択可能リージョンにする（transcript 選択 M13 を保つ）。
//!
//! Zed にも `markdown` crate があるが GPL のため**移植せず手法のみ参考**（DECISIONS §5・git=CLI /
//! terminal=alacritty と同じ「読んで自作 or permissive で代替」路線）。
//!
//! v1 で扱う範囲: 見出し / 段落 / 箇条書き・番号・タスクリスト（ネスト深さ保持）/ フェンスコード /
//! 水平線 / インライン（**強調**・*斜体*・~~打消し~~・`コード`・リンク）。表・引用装飾は後続。

use std::ops::Range;

/// インライン装飾の種別（具体色は描画側でテーマから当てる＝パーサはテーマ非依存）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpanKind {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    Link,
}

/// ブロックのテキストに掛かるインライン装飾（byte 範囲・ブロックのテキスト基準）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: SpanKind,
}

/// リスト項目のマーカ種別。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListMarker {
    Bullet,
    Ordered(u64),
    /// GFM タスクリスト（true = 済み）。
    Task(bool),
}

/// 解析後のブロック（描画側はこれを上から GPUI 要素へ落とす）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Block {
    Heading {
        level: u8,
        text: String,
        spans: Vec<Span>,
    },
    Paragraph {
        text: String,
        spans: Vec<Span>,
    },
    /// フェンス/インデントのコードブロック（`lang` はフェンスの情報文字列の先頭語）。
    Code {
        lang: Option<String>,
        text: String,
    },
    ListItem {
        depth: usize,
        marker: ListMarker,
        text: String,
        spans: Vec<Span>,
    },
    Rule,
}

/// 現在組み立て中のリーフブロックの種別（`buffer` が何になるか）。
enum Pending {
    Paragraph,
    Heading(u8),
    Item { depth: usize, marker: ListMarker },
}

/// リストのネスト状態（番号リストは次番号を持つ）。
struct ListLevel {
    next_number: Option<u64>,
}

/// markdown を Block 列へ解析する（GPUI 非依存＝unit test 可能）。
pub fn parse(source: &str) -> Vec<Block> {
    use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut blocks: Vec<Block> = Vec::new();
    // 組み立て中のリーフブロック。
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut pending: Option<Pending> = None;
    // 開いているインライン装飾（種別と開始 byte）。閉じで Span を確定。
    let mut inline: Vec<(SpanKind, usize)> = Vec::new();
    // リストのネスト（Start(List) で push・End(List) で pop）。
    let mut lists: Vec<ListLevel> = Vec::new();
    // 現在の項目のマーカ（Start(Item) で決定・TaskListMarker で上書き）。
    let mut item_marker = ListMarker::Bullet;
    // コードブロック中の言語とテキスト（Some = コードブロック内）。
    let mut code: Option<(Option<String>, String)> = None;

    // buffer を pending の種別で確定して blocks へ積む（空テキストは捨てる）。
    let flush = |blocks: &mut Vec<Block>,
                 text: &mut String,
                 spans: &mut Vec<Span>,
                 pending: &mut Option<Pending>| {
        let taken = std::mem::take(text);
        let taken_spans = std::mem::take(spans);
        let kind = pending.take();
        let trimmed = taken.trim_end();
        if trimmed.is_empty() {
            return;
        }
        let taken = trimmed.to_string();
        match kind {
            Some(Pending::Heading(level)) => blocks.push(Block::Heading {
                level,
                text: taken,
                spans: taken_spans,
            }),
            Some(Pending::Item { depth, marker }) => blocks.push(Block::ListItem {
                depth,
                marker,
                text: taken,
                spans: taken_spans,
            }),
            _ => blocks.push(Block::Paragraph {
                text: taken,
                spans: taken_spans,
            }),
        }
    };

    let in_item = |pending: &Option<Pending>| matches!(pending, Some(Pending::Item { .. }));

    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(Tag::Paragraph) => {
                if in_item(&pending) {
                    // ゆるいリストの項目内段落: 改行で繋いで項目テキストを 1 つに保つ。
                    if !text.is_empty() {
                        text.push('\n');
                    }
                } else {
                    flush(&mut blocks, &mut text, &mut spans, &mut pending);
                    pending = Some(Pending::Paragraph);
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if !in_item(&pending) {
                    flush(&mut blocks, &mut text, &mut spans, &mut pending);
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                pending = Some(Pending::Heading(heading_level(level)));
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
            }
            Event::Start(Tag::List(first)) => {
                // 直前の段落を確定してから 1 段深くなる。
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                lists.push(ListLevel { next_number: first });
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                let depth = lists.len().saturating_sub(1);
                item_marker = match lists.last_mut() {
                    Some(ListLevel {
                        next_number: Some(number),
                    }) => {
                        let current = *number;
                        *number += 1;
                        ListMarker::Ordered(current)
                    }
                    _ => ListMarker::Bullet,
                };
                pending = Some(Pending::Item {
                    depth,
                    marker: item_marker,
                });
            }
            Event::End(TagEnd::Item) => {
                // マーカが Task に差し替わっている場合があるので pending を更新してから確定。
                if let Some(Pending::Item { marker, .. }) = &mut pending {
                    *marker = item_marker;
                }
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
            }
            Event::TaskListMarker(checked) => {
                item_marker = ListMarker::Task(checked);
                if let Some(Pending::Item { marker, .. }) = &mut pending {
                    *marker = item_marker;
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                        .filter(|s| !s.is_empty()),
                    CodeBlockKind::Indented => None,
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, body)) = code.take() {
                    let body = body.strip_suffix('\n').map(str::to_string).unwrap_or(body);
                    blocks.push(Block::Code { lang, text: body });
                }
            }
            Event::Start(Tag::Emphasis) => inline.push((SpanKind::Emphasis, text.len())),
            Event::End(TagEnd::Emphasis) => {
                close_span(&mut inline, &mut spans, SpanKind::Emphasis, text.len())
            }
            Event::Start(Tag::Strong) => inline.push((SpanKind::Strong, text.len())),
            Event::End(TagEnd::Strong) => {
                close_span(&mut inline, &mut spans, SpanKind::Strong, text.len())
            }
            Event::Start(Tag::Strikethrough) => inline.push((SpanKind::Strikethrough, text.len())),
            Event::End(TagEnd::Strikethrough) => {
                close_span(&mut inline, &mut spans, SpanKind::Strikethrough, text.len())
            }
            Event::Start(Tag::Link { .. }) => inline.push((SpanKind::Link, text.len())),
            Event::End(TagEnd::Link) => {
                close_span(&mut inline, &mut spans, SpanKind::Link, text.len())
            }
            Event::Text(chunk) => {
                if let Some((_, body)) = &mut code {
                    body.push_str(&chunk);
                } else {
                    text.push_str(&chunk);
                }
            }
            Event::Code(chunk) => {
                // インラインコードは 1 イベント = そのまま範囲を Code スパンに。
                let start = text.len();
                text.push_str(&chunk);
                spans.push(Span {
                    range: start..text.len(),
                    kind: SpanKind::Code,
                });
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, body)) = &mut code {
                    body.push('\n');
                } else {
                    text.push('\n');
                }
            }
            Event::Rule => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                blocks.push(Block::Rule);
            }
            _ => {}
        }
    }
    flush(&mut blocks, &mut text, &mut spans, &mut pending);
    blocks
}

/// 開いているインライン装飾のうち種別一致の最内を閉じて Span を確定する（proper nest 前提）。
fn close_span(
    inline: &mut Vec<(SpanKind, usize)>,
    spans: &mut Vec<Span>,
    kind: SpanKind,
    end: usize,
) {
    if let Some(position) = inline.iter().rposition(|(open_kind, _)| *open_kind == kind) {
        let (_, start) = inline.remove(position);
        if start < end {
            spans.push(Span {
                range: start..end,
                kind,
            });
        }
    }
}

fn heading_level(level: pulldown_cmark::HeadingLevel) -> u8 {
    use pulldown_cmark::HeadingLevel::*;
    match level {
        H1 => 1,
        H2 => 2,
        H3 => 3,
        H4 => 4,
        H5 => 5,
        H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全ブロックについて span の byte 範囲が block.text の文字境界に乗り、範囲外でないこと。
    /// 乗らないと StyledText → gpui layout_line が `split_at` で abort する（クラッシュ再現）。
    fn assert_spans_valid(source: &str) {
        for block in parse(source) {
            let (text, spans) = match &block {
                Block::Heading { text, spans, .. }
                | Block::Paragraph { text, spans }
                | Block::ListItem { text, spans, .. } => (text.as_str(), spans),
                Block::Code { .. } | Block::Rule => continue,
            };
            for span in spans {
                assert!(
                    span.range.end <= text.len(),
                    "span {:?} が範囲外（text len {}, source={source:?})",
                    span.range,
                    text.len(),
                );
                assert!(
                    text.is_char_boundary(span.range.start)
                        && text.is_char_boundary(span.range.end),
                    "span {:?} が文字境界に乗らない（text={text:?}, source={source:?})",
                    span.range,
                );
            }
        }
    }

    #[test]
    fn span_ranges_stay_on_char_boundaries() {
        // 末尾装飾 + 末尾に全角/空白 → flush の trim_end で span が範囲外/非境界化しないか。
        assert_spans_valid("これは **強調（＝末尾）** \n");
        assert_spans_valid("末尾コード `TextRun（=フォントラン）`   \n");
        assert_spans_valid("- 項目 **太字（＝）**　\n- 次の項目");
        assert_spans_valid("# 見出し **（末尾）**\u{3000}\n");
        assert_spans_valid("name,color_index,project,branch,model のまま） `key` \n");
    }

    #[test]
    fn heading_and_inline_strong() {
        let blocks = parse("# タイトル\n\nHello **world**");
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    text: "タイトル".into(),
                    spans: vec![]
                },
                Block::Paragraph {
                    text: "Hello world".into(),
                    spans: vec![Span {
                        range: 6..11,
                        kind: SpanKind::Strong
                    }],
                },
            ]
        );
    }

    #[test]
    fn inline_code_span() {
        let blocks = parse("use `foo` now");
        assert_eq!(
            blocks,
            vec![Block::Paragraph {
                text: "use foo now".into(),
                spans: vec![Span {
                    range: 4..7,
                    kind: SpanKind::Code
                }],
            }]
        );
    }

    #[test]
    fn bullets_and_task_and_ordered() {
        let blocks = parse("- a\n- b");
        assert_eq!(
            blocks,
            vec![
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Bullet,
                    text: "a".into(),
                    spans: vec![]
                },
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Bullet,
                    text: "b".into(),
                    spans: vec![]
                },
            ]
        );

        let ordered = parse("1. first\n2. second");
        assert_eq!(
            ordered,
            vec![
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Ordered(1),
                    text: "first".into(),
                    spans: vec![]
                },
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Ordered(2),
                    text: "second".into(),
                    spans: vec![]
                },
            ]
        );

        let task = parse("- [x] done\n- [ ] todo");
        assert_eq!(
            task,
            vec![
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Task(true),
                    text: "done".into(),
                    spans: vec![]
                },
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Task(false),
                    text: "todo".into(),
                    spans: vec![]
                },
            ]
        );
    }

    #[test]
    fn fenced_code_keeps_body() {
        let blocks = parse("```rust\nlet x = 1;\n```");
        assert_eq!(
            blocks,
            vec![Block::Code {
                lang: Some("rust".into()),
                text: "let x = 1;".into()
            }]
        );
    }

    #[test]
    fn nested_bullets_track_depth() {
        let blocks = parse("- outer\n  - inner");
        assert_eq!(
            blocks,
            vec![
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Bullet,
                    text: "outer".into(),
                    spans: vec![]
                },
                Block::ListItem {
                    depth: 1,
                    marker: ListMarker::Bullet,
                    text: "inner".into(),
                    spans: vec![]
                },
            ]
        );
    }
}
