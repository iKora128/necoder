//! markdown — CommonMark(+GFM 一部)を GPUI 非依存の「ブロックモデル」へ簡約する共有パーサ（[model] 層）。
//!
//! 消費側は 2 つ。描画（GPUI 要素化）は各 view の責務で、このパーサはテーマ非依存・GPUI 非依存に保つ
//! （＝最速で unit test が回る層）:
//! - `agent_panel` の transcript（Agent 発話・ストリーミング。ブロック毎に選択リージョン化＝M13）
//! - `editor_view` の `.md` 整形プレビュー（source ⇄ rendered トグル）
//!
//! パーサは permissive な **pulldown-cmark**（MIT・CommonMark+GFM）を借りる。Zed にも `markdown`
//! crate があるが GPL のため**移植せず手法のみ参考**（DECISIONS §5・git=CLI / terminal=alacritty と
//! 同じ「読んで自作 or permissive で代替」路線）。
//!
//! v1 で扱う範囲: 見出し / 段落 / 箇条書き・番号・タスクリスト（ネスト深さ保持）/ フェンスコード /
//! 水平線 / 画像（ブロック扱い） / インライン（**強調**・*斜体*・~~打消し~~・`コード`・リンク）。
//! 表・引用装飾は後続。

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
    /// 画像。段落の途中に現れても**独立ブロックに切り出す**（v1。インライン描画は GPUI の
    /// StyledText がテキスト以外を挟めないため）。`alt` は装飾を落とした素のテキスト。
    /// パスの解決（相対→絶対・URL 判定）は描画側の責務。
    Image {
        source: String,
        alt: String,
    },
}

/// 現在組み立て中のリーフブロックの種別（`buffer` が何になるか）。
#[derive(Clone, Copy)]
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
    // 画像の収集中（Some = `![alt](url)` の内側）。本文へのイベントを alt へ逸らす。
    let mut image: Option<(String, String)> = None;
    // 画像でブロックを分割した直後は、残り本文の先頭空白/改行を捨てる（` after` を `after` に）。
    let mut strip_leading = false;

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
            // 画像の内側では alt の装飾は捨てる（image ガード）。開閉の byte 位置が本文の text を
            // 指してしまい、範囲外 span（= layout_line abort）を作るため。
            Event::Start(Tag::Emphasis) if image.is_none() => {
                inline.push((SpanKind::Emphasis, text.len()))
            }
            Event::End(TagEnd::Emphasis) if image.is_none() => {
                close_span(&mut inline, &mut spans, SpanKind::Emphasis, text.len())
            }
            Event::Start(Tag::Strong) if image.is_none() => {
                inline.push((SpanKind::Strong, text.len()))
            }
            Event::End(TagEnd::Strong) if image.is_none() => {
                close_span(&mut inline, &mut spans, SpanKind::Strong, text.len())
            }
            Event::Start(Tag::Strikethrough) if image.is_none() => {
                inline.push((SpanKind::Strikethrough, text.len()))
            }
            Event::End(TagEnd::Strikethrough) if image.is_none() => {
                close_span(&mut inline, &mut spans, SpanKind::Strikethrough, text.len())
            }
            Event::Start(Tag::Link { .. }) if image.is_none() => {
                inline.push((SpanKind::Link, text.len()))
            }
            Event::End(TagEnd::Link) if image.is_none() => {
                close_span(&mut inline, &mut spans, SpanKind::Link, text.len())
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                image = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((source, alt)) = image.take() {
                    // 段落/項目の途中でも画像は独立ブロックに切り出す（v1）。ここまでの本文を確定し、
                    // 開いている装飾は残り本文の先頭 (=0) から掛け直す（旧 text への stale offset で
                    // 範囲外 span を作らないため）。
                    let restored = pending;
                    flush(&mut blocks, &mut text, &mut spans, &mut pending);
                    for (_, start) in &mut inline {
                        *start = 0;
                    }
                    blocks.push(Block::Image {
                        source,
                        alt: alt.trim().to_string(),
                    });
                    pending = restored;
                    strip_leading = true;
                }
            }
            Event::Text(chunk) => {
                if let Some((_, alt)) = &mut image {
                    alt.push_str(&chunk);
                } else if let Some((_, body)) = &mut code {
                    body.push_str(&chunk);
                } else if strip_leading && text.is_empty() {
                    text.push_str(chunk.trim_start());
                    strip_leading = false;
                } else {
                    text.push_str(&chunk);
                    strip_leading = false;
                }
            }
            Event::Code(chunk) => {
                if let Some((_, alt)) = &mut image {
                    alt.push_str(&chunk);
                    continue;
                }
                // インラインコードは 1 イベント = そのまま範囲を Code スパンに。
                let start = text.len();
                text.push_str(&chunk);
                spans.push(Span {
                    range: start..text.len(),
                    kind: SpanKind::Code,
                });
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, alt)) = &mut image {
                    alt.push(' ');
                } else if let Some((_, body)) = &mut code {
                    body.push('\n');
                } else if !(strip_leading && text.is_empty()) {
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
                Block::Code { .. } | Block::Rule | Block::Image { .. } => continue,
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
    fn image_becomes_block() {
        let blocks = parse("![ねこ](images/cat.png)");
        assert_eq!(
            blocks,
            vec![Block::Image {
                source: "images/cat.png".into(),
                alt: "ねこ".into()
            }]
        );
    }

    #[test]
    fn inline_image_splits_paragraph() {
        let blocks = parse("before ![alt](x.png) after");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph {
                    text: "before".into(),
                    spans: vec![]
                },
                Block::Image {
                    source: "x.png".into(),
                    alt: "alt".into()
                },
                Block::Paragraph {
                    text: "after".into(),
                    spans: vec![]
                },
            ]
        );
    }

    #[test]
    fn open_span_across_image_stays_in_bounds() {
        // 画像で段落を分割した後、開いたままの装飾が残り本文の先頭から掛け直されること
        // （stale offset だと範囲外 span → layout_line abort）。
        assert_spans_valid("**bold ![a](x.png) tail**");
        let blocks = parse("**bold ![a](x.png) tail**");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph {
                    text: "bold".into(),
                    spans: vec![],
                },
                Block::Image {
                    source: "x.png".into(),
                    alt: "a".into()
                },
                Block::Paragraph {
                    text: "tail".into(),
                    spans: vec![Span {
                        range: 0..4,
                        kind: SpanKind::Strong
                    }],
                },
            ]
        );
    }

    #[test]
    fn image_alt_drops_formatting() {
        let blocks = parse("![**強調** と `code`](y.png)");
        assert_eq!(
            blocks,
            vec![Block::Image {
                source: "y.png".into(),
                alt: "強調 と code".into()
            }]
        );
    }

    #[test]
    fn image_inside_list_item_keeps_marker() {
        let blocks = parse("- item ![a](x.png)");
        assert_eq!(
            blocks,
            vec![
                Block::ListItem {
                    depth: 0,
                    marker: ListMarker::Bullet,
                    text: "item".into(),
                    spans: vec![]
                },
                Block::Image {
                    source: "x.png".into(),
                    alt: "a".into()
                },
            ]
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
