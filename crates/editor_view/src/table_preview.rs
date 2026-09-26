//! CSV / TSV を表として見る（O29）— ⌘⇧V で本文 ⇄ 表を切り替える（Markdown の整形プレビューと同じ口）。
//!
//! 読み方は RFC 4180（`"` で囲んだ欄の中の区切り・改行・`""` = `"` 1 つ）。TSV も同じ規則で読む。
//! 表は読むだけ（編集は本文で）。行は仮想リストで描くので大きいファイルでも固まらないが、読むのは
//! 先頭 [`MAX_ROWS`] 行まで（それ以上は「先頭 N 行だけ」と出す）。列の幅は先頭の行から文字数で決め、
//! 長い欄は切って出す。色は使わない（見出しの行は太字と下の線・行番号は fg2）。

use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, FontWeight, SharedString,
    UniformListScrollHandle,
};
use std::path::Path;
use std::rc::Rc;
use theme_core::Theme;
use unicode_width::UnicodeWidthStr;

/// 読む行数の上限（見出しの行を含む）。
pub(crate) const MAX_ROWS: usize = 20_000;
/// 列の幅を決めるのに見る行数。
const WIDTH_SAMPLE_ROWS: usize = 500;
/// 列の幅（文字数）の下限と上限。長い欄は切って出す（全文は本文で）。
const MIN_COLUMN_CHARS: usize = 3;
const MAX_COLUMN_CHARS: usize = 40;

/// 表として読めるファイルなら区切り文字（`.csv` = `,` / `.tsv` = タブ）。
pub(crate) fn table_delimiter(path: &Path) -> Option<char> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "csv" => Some(','),
        "tsv" => Some('\t'),
        _ => None,
    }
}

/// 読んだ表。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedTable {
    pub(crate) rows: Vec<Vec<String>>,
    /// [`MAX_ROWS`] で打ち切った（続きがある）。
    pub(crate) truncated: bool,
}

impl ParsedTable {
    /// 列の数（いちばん長い行に合わせる）。
    pub(crate) fn columns(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    /// 列ごとの幅（文字数・先頭 [`WIDTH_SAMPLE_ROWS`] 行の最大を上下限で丸める）。
    pub(crate) fn column_chars(&self) -> Vec<usize> {
        let mut widths = vec![MIN_COLUMN_CHARS; self.columns()];
        for row in self.rows.iter().take(WIDTH_SAMPLE_ROWS) {
            for (column, cell) in row.iter().enumerate() {
                let width = display_width(cell).clamp(MIN_COLUMN_CHARS, MAX_COLUMN_CHARS);
                if width > widths[column] {
                    widths[column] = width;
                }
            }
        }
        widths
    }
}

/// 欄の見た目の幅（全角は 2・欄の中の改行は 1 行目だけ見る）。
fn display_width(cell: &str) -> usize {
    cell.lines().next().unwrap_or("").width()
}

/// 区切り文字で分けて読む（RFC 4180）。`"` は欄の頭でだけ囲みの始まりとして扱い、囲みの中の
/// 区切り・改行はそのまま欄に入る。CR は捨てる（CRLF の行末）。空行は飛ばす。
pub(crate) fn parse_delimited(text: &str, delimiter: char, max_rows: usize) -> ParsedTable {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    // いまの欄が `"` で始まった（閉じた後の `"` を新しい囲みと取り違えない・`""` だけの欄も空行にしない）。
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if in_quotes {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(character);
            }
            continue;
        }
        match character {
            '"' if field.is_empty() && !quoted => {
                in_quotes = true;
                quoted = true;
            }
            '\r' => {}
            '\n' => {
                let blank = row.is_empty() && field.is_empty() && !quoted;
                row.push(std::mem::take(&mut field));
                quoted = false;
                let line = std::mem::take(&mut row);
                if blank {
                    continue; // 空行は飛ばす
                }
                rows.push(line);
                if rows.len() >= max_rows {
                    let truncated = chars.peek().is_some();
                    return ParsedTable { rows, truncated };
                }
            }
            character if character == delimiter => {
                row.push(std::mem::take(&mut field));
                quoted = false;
            }
            character => field.push(character),
        }
    }
    // 最後の改行が無い行。
    if !field.is_empty() || !row.is_empty() || quoted {
        row.push(field);
        rows.push(row);
    }
    ParsedTable {
        rows,
        truncated: false,
    }
}

/// 表を描く。1 行目を見出しにして上に固定し、残りを仮想リストで描く。横は列の幅の合計で、はみ出す
/// 分は横にスクロールする。
pub(crate) fn render_table(
    table: Rc<ParsedTable>,
    theme: &Theme,
    code_font: SharedString,
    font_size: f32,
    scroll: &UniformListScrollHandle,
    truncated_label: Option<SharedString>,
) -> AnyElement {
    let cell_width = font_size * 0.62;
    let column_widths: Rc<Vec<f32>> = Rc::new(
        table
            .column_chars()
            .into_iter()
            .map(|chars| chars as f32 * cell_width + 16.)
            .collect(),
    );
    let body_rows = table.rows.len().saturating_sub(1);
    let number_width = (body_rows.max(1).to_string().len() as f32) * cell_width + 16.;
    let total_width: f32 = number_width + column_widths.iter().sum::<f32>();
    let row_height = (font_size * 1.7).round();
    let header = table.rows.first().cloned().unwrap_or_default();
    let header_row = table_row(
        None,
        &header,
        &column_widths,
        number_width,
        row_height,
        theme,
        true,
    );
    let list = {
        let table = table.clone();
        let column_widths = column_widths.clone();
        let theme = theme.clone();
        uniform_list(
            "table-preview-rows",
            body_rows,
            move |range, _window, _cx| {
                range
                    .map(|index| {
                        let cells = table.rows.get(index + 1).cloned().unwrap_or_default();
                        table_row(
                            Some(index + 1),
                            &cells,
                            &column_widths,
                            number_width,
                            row_height,
                            &theme,
                            false,
                        )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(scroll)
        .flex_1()
    };
    div()
        .id("table-preview")
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.bg1)
        .font_family(code_font)
        .text_size(px(font_size))
        .when_some(truncated_label, |element, label| {
            element.child(
                div()
                    .flex_none()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(label),
            )
        })
        .child(
            div()
                .id("table-preview-scroll-x")
                .flex_1()
                .min_h_0()
                .overflow_x_scroll()
                .child(
                    div()
                        .w(px(total_width))
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(header_row)
                        .child(list),
                ),
        )
        .into_any_element()
}

/// 1 行（行番号 + 欄）。見出しの行は太字で下に線。
fn table_row(
    number: Option<usize>,
    cells: &[String],
    column_widths: &[f32],
    number_width: f32,
    row_height: f32,
    theme: &Theme,
    header: bool,
) -> AnyElement {
    let mut row = div()
        .flex()
        .flex_none()
        .h(px(row_height))
        .items_center()
        .when(header, |row| {
            row.font_weight(FontWeight::SEMIBOLD)
                .bg(theme.bg2)
                .border_b_1()
                .border_color(theme.border)
        })
        .child(
            div()
                .flex_none()
                .w(px(number_width))
                .px(px(8.))
                .text_color(theme.fg2)
                .text_right()
                .child(SharedString::from(
                    number.map(|number| number.to_string()).unwrap_or_default(),
                )),
        );
    for (column, width) in column_widths.iter().enumerate() {
        let text = cells
            .get(column)
            .map(|cell| cell.lines().next().unwrap_or("").to_string())
            .unwrap_or_default();
        row = row.child(
            div()
                .flex_none()
                .w(px(*width))
                .px(px(8.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .border_l_1()
                .border_color(theme.border)
                .text_color(if header { theme.fg0 } else { theme.fg1 })
                .child(SharedString::from(text)),
        );
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(table: &ParsedTable) -> Vec<Vec<&str>> {
        table
            .rows
            .iter()
            .map(|row| row.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn csv_and_tsv_files_are_tables() {
        assert_eq!(table_delimiter(Path::new("data/a.csv")), Some(','));
        assert_eq!(table_delimiter(Path::new("B.TSV")), Some('\t'));
        assert_eq!(table_delimiter(Path::new("a.txt")), None);
        assert_eq!(table_delimiter(Path::new("csv")), None);
    }

    #[test]
    fn quoted_fields_keep_delimiters_newlines_and_quotes() {
        let text = "name,note\r\n\"Doe, Jane\",\"said \"\"hi\"\"\"\nplain,\"two\nlines\"\n";
        let table = parse_delimited(text, ',', MAX_ROWS);
        assert_eq!(
            rows(&table),
            vec![
                vec!["name", "note"],
                vec!["Doe, Jane", "said \"hi\""],
                vec!["plain", "two\nlines"],
            ]
        );
        assert!(!table.truncated);
    }

    #[test]
    fn empty_fields_blank_lines_and_a_missing_last_newline() {
        let table = parse_delimited("a,,c\n\n,b,\nx,y,z", ',', MAX_ROWS);
        assert_eq!(
            rows(&table),
            vec![vec!["a", "", "c"], vec!["", "b", ""], vec!["x", "y", "z"]],
            "空行は飛ばし、最後の改行が無くても最後の行を読む"
        );
        assert_eq!(
            rows(&parse_delimited("\"\"\n", ',', MAX_ROWS)),
            vec![vec![""]]
        );
    }

    #[test]
    fn tabs_split_tsv_and_a_quote_inside_a_field_is_kept() {
        let table = parse_delimited("id\tname\n1\t5\" floppy\n", '\t', MAX_ROWS);
        assert_eq!(
            rows(&table),
            vec![vec!["id", "name"], vec!["1", "5\" floppy"]]
        );
    }

    #[test]
    fn reading_stops_at_the_row_limit() {
        let text: String = (0..10).map(|index| format!("{index}\n")).collect();
        let table = parse_delimited(&text, ',', 3);
        assert_eq!(table.rows.len(), 3);
        assert!(table.truncated);
        let exact = parse_delimited("1\n2\n3\n", ',', 3);
        assert!(!exact.truncated, "ちょうど上限なら続きは無い");
    }

    #[test]
    fn column_widths_follow_the_longest_cell_within_bounds() {
        let long = "x".repeat(100);
        let table = parse_delimited(&format!("a,日本語,{long}\nb,c\n"), ',', MAX_ROWS);
        assert_eq!(table.columns(), 3);
        assert_eq!(
            table.column_chars(),
            vec![MIN_COLUMN_CHARS, 6, MAX_COLUMN_CHARS],
            "全角は 2・下限 3・上限 40"
        );
    }
}
