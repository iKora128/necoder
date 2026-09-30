//! markdown — CommonMark(+GFM 一部)を GPUI 非依存の「ブロックモデル」へ簡約する共有パーサ（[model] 層）。
//!
//! 消費側は 2 つ。描画（GPUI 要素化）は各 view の責務で、このパーサはテーマ非依存・GPUI 非依存に保つ
//! （＝最速で unit test が回る層）:
//! - `agent_panel` の transcript（Agent 発話・ストリーミング。ブロック毎に選択リージョン化＝M13）
//! - `editor_view` の `.md` 整形プレビュー（source ⇄ rendered トグル）
//!
//! パーサは permissive な **pulldown-cmark**（MIT・CommonMark+GFM）を借りる。Zed の `markdown`
//! crate のコードは取り込まず、CommonMark 仕様と pulldown-cmark の公開 API から独立に実装する
//! （DECISIONS §5）。
//!
//! v1 で扱う範囲: 見出し / 段落 / 箇条書き・番号・タスクリスト（ネスト深さ保持）/ フェンスコード /
//! 水平線 / 画像（ブロック扱い） / GFM 表 / インライン（**強調**・*斜体*・~~打消し~~・`コード`・リンク）。
//! 引用装飾は後続。

use std::ops::Range;

/// インライン装飾の種別（具体色は描画側でテーマから当てる＝パーサはテーマ非依存）。
///
/// `Link` だけは**行き先を持つ**。`[text](dest)` の dest は本文テキストに現れないため、ここで
/// 運ばないと描画側が「クリックで開く」を作れない（2026-09-16）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SpanKind {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    /// `destination` は生の dest 文字列（URL / パス / アンカーの区別は描画側の責務）。
    Link { destination: String },
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

/// GFM 表の列そろえ（`:---` 記法。Auto = 指定なし＝左そろえで描く）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableAlign {
    Auto,
    Left,
    Center,
    Right,
}

/// GFM 表のセル（テキスト + インライン装飾。ブロック本文と同じ規約）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TableCell {
    pub text: String,
    pub spans: Vec<Span>,
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
    /// GFM 表。`alignments` は列数ぶん（区切り行 `|:---|` 由来）。行のセル数は列数に届かない
    /// ことがある（描画側は不足分を空セル扱いにする）。
    Table {
        alignments: Vec<TableAlign>,
        head: Vec<TableCell>,
        rows: Vec<Vec<TableCell>>,
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

/// 組み立て中の GFM 表（Start(Table)〜End(Table) の間だけ Some）。
struct TableState {
    alignments: Vec<TableAlign>,
    head: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    /// 組み立て中の行（TableHead 内はヘッダ行として使う）。
    row: Vec<TableCell>,
    in_head: bool,
}

/// markdown を Block 列へ解析する（GPUI 非依存＝unit test 可能）。
pub fn parse(source: &str) -> Vec<Block> {
    use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);

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
    // 組み立て中の GFM 表（Some の間、text/spans はセル単位で使い回す）。
    let mut table: Option<TableState> = None;

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
            Event::Start(Tag::Link { dest_url, .. }) if image.is_none() => inline.push((
                SpanKind::Link {
                    destination: dest_url.to_string(),
                },
                text.len(),
            )),
            Event::End(TagEnd::Link) if image.is_none() => close_span(
                &mut inline,
                &mut spans,
                SpanKind::Link {
                    destination: String::new(),
                },
                text.len(),
            ),
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
            Event::Start(Tag::Table(column_alignments)) => {
                flush(&mut blocks, &mut text, &mut spans, &mut pending);
                table = Some(TableState {
                    alignments: column_alignments
                        .iter()
                        .map(|alignment| match alignment {
                            Alignment::None => TableAlign::Auto,
                            Alignment::Left => TableAlign::Left,
                            Alignment::Center => TableAlign::Center,
                            Alignment::Right => TableAlign::Right,
                        })
                        .collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    in_head: false,
                });
            }
            Event::End(TagEnd::Table) => {
                if let Some(state) = table.take() {
                    if !state.head.is_empty() || !state.rows.is_empty() {
                        blocks.push(Block::Table {
                            alignments: state.alignments,
                            head: state.head,
                            rows: state.rows,
                        });
                    }
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(state) = &mut table {
                    state.in_head = true;
                }
            }
            // TableHead は TableRow を挟まず TableCell が直接並ぶ（pulldown-cmark の並び）。
            Event::End(TagEnd::TableHead) => {
                if let Some(state) = &mut table {
                    state.head = std::mem::take(&mut state.row);
                    state.in_head = false;
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(state) = &mut table {
                    let row = std::mem::take(&mut state.row);
                    state.rows.push(row);
                }
            }
            Event::Start(Tag::TableCell) => {
                // セル境界。前セルの取りこぼし（あり得ないが防御）と開き掛けの装飾を捨てる。
                text.clear();
                spans.clear();
                inline.clear();
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(state) = &mut table {
                    state.row.push(finish_table_cell(&mut text, &mut spans));
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

/// セルの text/spans バッファを TableCell へ確定する。末尾空白を落とし、trim で縮んだ分だけ
/// span を新しい長さへクランプする（範囲外 span = 描画側 layout_line abort の芽を残さない）。
fn finish_table_cell(text: &mut String, spans: &mut Vec<Span>) -> TableCell {
    let mut cell_text = std::mem::take(text);
    let taken_spans = std::mem::take(spans);
    let trimmed_length = cell_text.trim_end().len();
    cell_text.truncate(trimmed_length);
    let spans = taken_spans
        .into_iter()
        .filter_map(|mut span| {
            span.range.end = span.range.end.min(trimmed_length);
            (span.range.start < span.range.end).then_some(span)
        })
        .collect();
    TableCell {
        text: cell_text,
        spans,
    }
}

/// 表の 1 列ぶんのレイアウトヒント（テーマ/GPUI 非依存＝ここで unit test する）。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TableColumn {
    /// 列幅の比率（全列合計 1.0）。描画側は `w(relative(fraction))` を当てる。
    pub fraction: f32,
    /// 折り返さず表示したい最短幅（表示幅単位: ASCII=1・CJK=2。上限つき）。描画側は
    /// `min_w` に換算して、ID/DONE のような短い列が比率配分で 1〜2 文字幅へ潰れて
    /// 縦書き状態になるのを防ぐ。
    pub min_units: f32,
}

/// 文書の頭の front matter（Jekyll / Hugo / Obsidian などの `---` YAML・`+++` TOML・O29）。
/// 整形プレビューは表の形で見せ、残りだけを [`parse`] に回す（front matter の閉じの `---` が
/// 直前の行を setext 見出しにしてしまうのを防ぐ）。Agent の発話には使わない（文書の約束なので）。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FrontMatter {
    /// 上の階層のキーと値（書かれた順）。入れ子や一覧は値の 1 行に畳む（`a, b`）。
    pub entries: Vec<(String, String)>,
}

/// 頭の front matter を切り出す。無ければ `(None, source)`。1 行目がちょうど `---`（YAML）か
/// `+++`（TOML）で、閉じの行（YAML は `---` か `...`・TOML は `+++`）がある時だけ。閉じが無ければ
/// front matter とみなさない（ただの水平線と段落として読む）。先頭の BOM と CRLF は許す。
pub fn split_front_matter(source: &str) -> (Option<FrontMatter>, &str) {
    let text = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, source);
    };
    let (closers, separator): (&[&str], char) = match first.trim_end() {
        "---" => (&["---", "..."], ':'),
        "+++" => (&["+++"], '='),
        _ => return (None, source),
    };
    let mut offset = first.len();
    let mut body = Vec::new();
    for line in lines {
        let end = offset + line.len();
        if closers.contains(&line.trim_end()) {
            let entries = front_matter_entries(&body, separator);
            return (Some(FrontMatter { entries }), &text[end..]);
        }
        body.push(line.trim_end_matches(['\r', '\n']));
        offset = end;
    }
    (None, source)
}

/// front matter の行からキーと値を拾う。上の階層の `key: value`（TOML は `key = value`）が 1 項目で、
/// 字下げされた行や `- ` の一覧は直前の項目の値へ `, ` で足す。コメントと空行、TOML の `[表]` は飛ばす。
fn front_matter_entries(lines: &[&str], separator: char) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if separator == '=' && trimmed.starts_with('[') && trimmed.ends_with(']') {
            continue;
        }
        let top_level = !line.starts_with([' ', '\t']) && !trimmed.starts_with("- ");
        if top_level {
            if let Some((key, value)) = line.split_once(separator) {
                entries.push((
                    front_matter_value(key.trim()),
                    front_matter_value(value.trim()),
                ));
                continue;
            }
        }
        let Some((_, value)) = entries.last_mut() else {
            continue;
        };
        let item = front_matter_value(trimmed.strip_prefix("- ").unwrap_or(trimmed).trim());
        if item.is_empty() {
            continue;
        }
        if !value.is_empty() {
            value.push_str(", ");
        }
        value.push_str(&item);
    }
    entries
}

/// 値の見た目: 囲みの引用符を外し、`[a, "b"]` の一覧は `a, b` にする。
fn front_matter_value(value: &str) -> String {
    let unquote = |text: &str| -> String {
        let text = text.trim();
        let quoted = text.len() >= 2
            && ((text.starts_with('"') && text.ends_with('"'))
                || (text.starts_with('\'') && text.ends_with('\'')));
        if quoted {
            text[1..text.len() - 1].to_string()
        } else {
            text.to_string()
        }
    };
    match value
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        Some(inner) => inner
            .split(',')
            .map(unquote)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        None => unquote(value),
    }
}

/// 表の列レイアウト（内容の最長表示幅ベース）。GPUI にテーブルレイアウトが無いため、
/// 比率 + 最小幅の 2 段構えで近似する: ID のような短い列は内容ぶんを確保して折り返さず、
/// 本文列は残りを比率で分け合い、極端な長文列は頭打ちにして他列を潰さない。
pub fn table_columns(head: &[TableCell], rows: &[Vec<TableCell>]) -> Vec<TableColumn> {
    // 表示幅の目安: ASCII=1・それ以外（CJK 主体）=2。等幅前提の近似で十分（比率と最小幅にしか使わない）。
    fn display_width(text: &str) -> f32 {
        text.lines()
            .map(|line| {
                line.chars()
                    .map(|c| if c.is_ascii() { 1.0 } else { 2.0 })
                    .sum::<f32>()
            })
            .fold(0.0, f32::max)
    }
    let columns = rows
        .iter()
        .map(Vec::len)
        .fold(head.len(), usize::max)
        .max(1);
    let mut widths = vec![0.0f32; columns];
    for row in std::iter::once(head).chain(rows.iter().map(Vec::as_slice)) {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(&cell.text));
        }
    }
    // 下限: 空列でも幅を持つ / 上限: 1 列の長文が他列を 1 文字幅まで潰さない。
    let total: f32 = widths.iter().map(|width| width.clamp(3.0, 40.0)).sum();
    widths
        .into_iter()
        .map(|width| TableColumn {
            fraction: width.clamp(3.0, 40.0) / total,
            // 最小幅は 16 単位（≒ ASCII 16 文字）で頭打ち: 全列の最小幅合計が表幅を超えて
            // あふれる事態を短い列の保護と両立させる。
            min_units: width.min(16.0),
        })
        .collect()
}

/// 開いているインライン装飾のうち種別一致の最内を閉じて Span を確定する（proper nest 前提）。
/// 開いているインライン装飾を閉じる。`kind` は**種別の照合にだけ**使い、実際に積むのは
/// 開いた時の値（`Link` が持つ行き先を落とさないため）。
fn close_span(
    inline: &mut Vec<(SpanKind, usize)>,
    spans: &mut Vec<Span>,
    kind: SpanKind,
    end: usize,
) {
    let wanted = std::mem::discriminant(&kind);
    if let Some(position) = inline
        .iter()
        .rposition(|(open_kind, _)| std::mem::discriminant(open_kind) == wanted)
    {
        let (open_kind, start) = inline.remove(position);
        if start < end {
            spans.push(Span {
                range: start..end,
                kind: open_kind,
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

    #[test]
    fn front_matter_is_cut_off_and_read_as_properties() {
        let source = "---\ntitle: \"Release notes\"\ndate: 2026-09-26\ntags:\n  - rust\n  - gpui\naliases: [notes, 'changelog']\n# comment\nurl: https://example.com/a\n---\n# Heading\n";
        let (front, body) = split_front_matter(source);
        let entries = front.expect("front matter").entries;
        assert_eq!(
            entries,
            vec![
                ("title".to_string(), "Release notes".to_string()),
                ("date".to_string(), "2026-09-26".to_string()),
                ("tags".to_string(), "rust, gpui".to_string()),
                ("aliases".to_string(), "notes, changelog".to_string()),
                ("url".to_string(), "https://example.com/a".to_string()),
            ]
        );
        assert_eq!(body, "# Heading\n");
        assert!(matches!(
            parse(body).first(),
            Some(Block::Heading { level: 1, .. })
        ));

        let (front, body) = split_front_matter(
            "\u{feff}+++\r\ntitle = \"TOML\"\r\n[extra]\r\ndraft = false\r\n+++\r\nText\r\n",
        );
        assert_eq!(
            front.expect("TOML").entries,
            vec![
                ("title".to_string(), "TOML".to_string()),
                ("draft".to_string(), "false".to_string()),
            ]
        );
        assert_eq!(body, "Text\r\n");
    }

    #[test]
    fn a_rule_without_a_closing_line_is_not_front_matter() {
        let source = "---\ntitle: no end\n\nParagraph\n";
        assert_eq!(split_front_matter(source), (None, source));
        let source = "# Title\n---\nkey: value\n---\n";
        assert_eq!(
            split_front_matter(source),
            (None, source),
            "頭でなければ読まない"
        );
        let (front, body) = split_front_matter("---\n---\nbody");
        assert_eq!(front, Some(FrontMatter::default()), "空でも front matter");
        assert_eq!(body, "body");
    }

    #[test]
    fn link_spans_carry_their_destination() {
        // 実測（2026-09-16）で Claude が最も使う引用形式。dest を捨てるとクリックで開けない。
        let blocks = parse("[acp_client.rs](/tmp/crates/acp_client/src/acp_client.rs:361) を直した");
        let Some(Block::Paragraph { text, spans }) = blocks.first() else {
            panic!("段落が来ていない: {blocks:?}");
        };
        assert_eq!(text, "acp_client.rs を直した");
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].range.clone()], "acp_client.rs");
        assert_eq!(
            spans[0].kind,
            SpanKind::Link {
                destination: "/tmp/crates/acp_client/src/acp_client.rs:361".into()
            }
        );
    }

    #[test]
    fn nested_decorations_close_without_stealing_the_link_destination() {
        // 強調の中のリンク: 種別だけで閉じるので、内側の行き先が外側に混ざらないこと。
        let blocks = parse("**強調と [リンク](https://necoder.com) の混在**");
        let Some(Block::Paragraph { spans, .. }) = blocks.first() else {
            panic!("段落が来ていない: {blocks:?}");
        };
        assert!(spans.iter().any(|span| span.kind
            == SpanKind::Link {
                destination: "https://necoder.com".into()
            }));
        assert!(spans.iter().any(|span| span.kind == SpanKind::Strong));
    }

    /// 全ブロックについて span の byte 範囲が block.text の文字境界に乗り、範囲外でないこと。
    /// 乗らないと StyledText → gpui layout_line が `split_at` で abort する（クラッシュ再現）。
    fn assert_spans_valid(source: &str) {
        fn assert_text_spans(text: &str, spans: &[Span], source: &str) {
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
        for block in parse(source) {
            match &block {
                Block::Heading { text, spans, .. }
                | Block::Paragraph { text, spans }
                | Block::ListItem { text, spans, .. } => assert_text_spans(text, spans, source),
                Block::Table { head, rows, .. } => {
                    for cell in head.iter().chain(rows.iter().flatten()) {
                        assert_text_spans(&cell.text, &cell.spans, source);
                    }
                }
                Block::Code { .. } | Block::Rule | Block::Image { .. } => {}
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

    #[test]
    fn gfm_table_with_inline_code_and_alignment() {
        let blocks = parse(
            "| ID | 状態 | タスク |\n|:---|:---:|---|\n| P0-1 | DONE | `git init` と README |\n| P0-2 | DONE | uv 環境 |",
        );
        let [Block::Table {
            alignments,
            head,
            rows,
        }] = blocks.as_slice()
        else {
            panic!("表 1 ブロックになるはず: {blocks:?}");
        };
        assert_eq!(
            alignments,
            &vec![TableAlign::Left, TableAlign::Center, TableAlign::Auto]
        );
        assert_eq!(
            head.iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            vec!["ID", "状態", "タスク"]
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].text, "P0-1");
        // セル内インラインコードは Code スパンとして残る。
        let code_cell = &rows[0][2];
        assert_eq!(code_cell.text, "git init と README");
        assert_eq!(
            code_cell.spans,
            vec![Span {
                range: 0.."git init".len(),
                kind: SpanKind::Code
            }]
        );
        assert_spans_valid("| a | b |\n|---|---|\n| **強調（＝末尾）** | `末尾コード` |");
    }

    #[test]
    fn table_between_paragraphs_and_ragged_rows() {
        // 前後の段落が表に飲まれない・セル数が列数に満たない行もそのまま保持する。
        let blocks = parse("before\n\n| a | b |\n|---|---|\n| 1 |\n\nafter");
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert!(matches!(&blocks[0], Block::Paragraph { text, .. } if text == "before"));
        let Block::Table { head, rows, .. } = &blocks[1] else {
            panic!("中央は表: {blocks:?}");
        };
        assert_eq!(head.len(), 2);
        assert_eq!(rows.len(), 1);
        assert!(matches!(&blocks[2], Block::Paragraph { text, .. } if text == "after"));
    }

    #[test]
    fn table_columns_favor_long_columns_but_protect_short_ones() {
        let blocks = parse("| ID | 長い説明列 |\n|---|---|\n| P0-1 | これはとても長い説明のセルでほかの列より太くなるはず |");
        let Some(Block::Table { head, rows, .. }) = blocks.first() else {
            panic!("表になるはず: {blocks:?}");
        };
        let columns = table_columns(head, rows);
        assert_eq!(columns.len(), 2);
        assert!(
            columns[1].fraction > columns[0].fraction,
            "本文列が太い: {columns:?}"
        );
        let total: f32 = columns.iter().map(|column| column.fraction).sum();
        assert!((total - 1.0).abs() < 1e-4, "正規化: {columns:?}");
        // 短い列は内容ぶんの最小幅を持つ（P0-1 = 4 単位）・長い列は 16 で頭打ち。
        assert_eq!(columns[0].min_units, 4.0);
        assert_eq!(columns[1].min_units, 16.0);
    }
}
