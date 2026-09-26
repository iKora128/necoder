//! 変更レビューの行の並び（ファイル見出し / お知らせ / 行 / 畳んだ領域）とファイルツリー。GPUI 非依存。
//!
//! 行は model（`project::review::ReviewDiff`）への添字だけを持ち、本文は複製しない。
//! 変更の無い領域（先頭・hunk の間・末尾）は「⋯ N 行」に畳み、押すと前後 [`EXPAND_STEP`] 行ずつ開く。
//! 展開の量はファイルのパスと畳みの番号で覚える（読み直しても開いた所が閉じない）。

use project::review::{FileDiff, ReviewHunk};
use std::collections::{BTreeMap, HashMap, HashSet};

/// 畳みを 1 回押すと開く行数（上下それぞれ）。
pub const EXPAND_STEP: u32 = 20;

/// 畳んだ領域がファイルのどこにあるか（開く向きが違う）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapPosition {
    /// 最初の hunk より前。hunk に近い側（下）から開く。
    Leading,
    /// hunk と hunk の間。上下から同時に開く。
    Between,
    /// 最後の hunk より後ろ。hunk に近い側（上）から開く。
    Trailing,
}

/// 変更の無い領域。行番号は 1 始まり。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    /// 畳みの番号。`i` = hunk `i` の直前、`hunks.len()` = 末尾。
    pub index: usize,
    pub position: GapPosition,
    pub old_start: u32,
    pub new_start: u32,
    pub len: u32,
}

/// 開いた量（上側・下側から開いた行数）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GapExpansion {
    pub top: u32,
    pub bottom: u32,
}

impl GapExpansion {
    /// まだ畳まれている行数。
    pub fn hidden(self, gap: &Gap) -> u32 {
        gap.len.saturating_sub(self.top + self.bottom)
    }
}

/// ファイルの畳める領域を並べる。`new_total` = 新側の全行数（全文を読めた時だけ。末尾の領域に要る）。
pub fn file_gaps(hunks: &[ReviewHunk], new_total: Option<u32>) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let (Some(first), Some(last)) = (hunks.first(), hunks.last()) else {
        return gaps;
    };
    let leading = first.new_span().start.saturating_sub(1);
    if leading > 0 {
        gaps.push(Gap {
            index: 0,
            position: GapPosition::Leading,
            old_start: 1,
            new_start: 1,
            len: leading,
        });
    }
    for index in 1..hunks.len() {
        let previous = &hunks[index - 1];
        let new_start = previous.new_span().end;
        let len = hunks[index].new_span().start.saturating_sub(new_start);
        if len > 0 {
            gaps.push(Gap {
                index,
                position: GapPosition::Between,
                old_start: previous.old_span().end,
                new_start,
                len,
            });
        }
    }
    if let Some(total) = new_total {
        let new_start = last.new_span().end;
        let len = (total + 1).saturating_sub(new_start);
        if len > 0 {
            gaps.push(Gap {
                index: hunks.len(),
                position: GapPosition::Trailing,
                old_start: last.old_span().end,
                new_start,
                len,
            });
        }
    }
    gaps
}

/// 畳みを 1 回押した後の開き方。間の領域は残りが 2 回分以下なら全部開く。
pub fn expand_gap(gap: &Gap, current: GapExpansion) -> GapExpansion {
    let hidden = current.hidden(gap);
    if hidden == 0 {
        return current;
    }
    match gap.position {
        GapPosition::Leading => GapExpansion {
            top: current.top,
            bottom: current.bottom + EXPAND_STEP.min(hidden),
        },
        GapPosition::Trailing => GapExpansion {
            top: current.top + EXPAND_STEP.min(hidden),
            bottom: current.bottom,
        },
        GapPosition::Between if hidden <= EXPAND_STEP * 2 => GapExpansion {
            top: current.top + hidden,
            bottom: current.bottom,
        },
        GapPosition::Between => GapExpansion {
            top: current.top + EXPAND_STEP,
            bottom: current.bottom + EXPAND_STEP,
        },
    }
}

/// 中身の代わりに出す一言。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Binary,
    TooLarge,
    /// 権限（実行ビット等）だけが変わった。
    ModeOnly,
    /// 中身の無いファイル（空の新規・空へ削除・純粋な改名）。
    Empty,
}

/// リストの 1 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    FileHeader {
        file: usize,
    },
    Notice {
        file: usize,
        notice: Notice,
    },
    /// diff の行（hunk の中）。
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    /// 畳みを開いて見せている変更の無い行。
    Context {
        file: usize,
        old_line: u32,
        new_line: u32,
    },
    /// まだ畳んでいる領域。`expandable` = 全文を読めたので中身を出せる。
    Fold {
        file: usize,
        gap: Gap,
        hidden: u32,
        expandable: bool,
        /// 直後の hunk の見出し（git が拾った関数名など）。
        section: String,
    },
}

impl Row {
    pub fn file(&self) -> usize {
        match self {
            Self::FileHeader { file }
            | Self::Notice { file, .. }
            | Self::Line { file, .. }
            | Self::Context { file, .. }
            | Self::Fold { file, .. } => *file,
        }
    }
}

/// 行を並べる材料。
pub struct RowInputs<'a> {
    pub files: &'a [FileDiff],
    /// 新側の全行数（全文を読めたファイルだけ `Some`）。`files` と同じ添字。
    pub new_totals: &'a [Option<u32>],
    /// 開いた量（キー = (パス, 畳みの番号)）。
    pub expansions: &'a HashMap<(String, usize), GapExpansion>,
    /// 見出しで畳んだファイル（パス）。
    pub collapsed: &'a HashSet<String>,
}

/// 全ファイルの行を並べる。
pub fn build_rows(inputs: &RowInputs) -> Vec<Row> {
    let mut rows = Vec::new();
    for (file_index, file) in inputs.files.iter().enumerate() {
        rows.push(Row::FileHeader { file: file_index });
        if inputs.collapsed.contains(&file.path) {
            continue;
        }
        if let Some(notice) = file_notice(file) {
            rows.push(Row::Notice {
                file: file_index,
                notice,
            });
            continue;
        }
        let new_total = inputs.new_totals.get(file_index).copied().flatten();
        let gaps = file_gaps(&file.hunks, new_total);
        let mut gaps = gaps.into_iter().peekable();
        for hunk_index in 0..=file.hunks.len() {
            if let Some(gap) = gaps.next_if(|gap| gap.index == hunk_index) {
                let section = file
                    .hunks
                    .get(hunk_index)
                    .map(|hunk| hunk.section.clone())
                    .unwrap_or_default();
                push_gap(
                    &mut rows, inputs, file_index, &file.path, gap, new_total, section,
                );
            }
            let Some(hunk) = file.hunks.get(hunk_index) else {
                continue;
            };
            for line_index in 0..hunk.lines.len() {
                rows.push(Row::Line {
                    file: file_index,
                    hunk: hunk_index,
                    line: line_index,
                });
            }
        }
    }
    rows
}

fn push_gap(
    rows: &mut Vec<Row>,
    inputs: &RowInputs,
    file: usize,
    path: &str,
    gap: Gap,
    new_total: Option<u32>,
    section: String,
) {
    let expandable = new_total.is_some();
    let expansion = if expandable {
        inputs
            .expansions
            .get(&(path.to_string(), gap.index))
            .copied()
            .unwrap_or_default()
    } else {
        GapExpansion::default()
    };
    let top = expansion.top.min(gap.len);
    let bottom = expansion.bottom.min(gap.len - top);
    let context = |offset: u32| Row::Context {
        file,
        old_line: gap.old_start + offset,
        new_line: gap.new_start + offset,
    };
    rows.extend((0..top).map(context));
    let hidden = gap.len - top - bottom;
    if hidden > 0 {
        rows.push(Row::Fold {
            file,
            gap,
            hidden,
            expandable,
            section,
        });
    }
    rows.extend((gap.len - bottom..gap.len).map(context));
}

/// 中身の代わりに一言だけ出すファイルか。
pub fn file_notice(file: &FileDiff) -> Option<Notice> {
    if file.binary {
        Some(Notice::Binary)
    } else if file.too_large {
        Some(Notice::TooLarge)
    } else if file.hunks.is_empty() && file.mode_changed {
        Some(Notice::ModeOnly)
    } else if file.hunks.is_empty() {
        Some(Notice::Empty)
    } else {
        None
    }
}

/// 左のファイルツリーの 1 行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeEntry {
    /// ディレクトリ（子が 1 つだけのディレクトリは `a/b` のように詰める）。
    Directory {
        path: String,
        label: String,
        depth: usize,
    },
    File {
        file: usize,
        label: String,
        depth: usize,
    },
}

#[derive(Default)]
struct DirectoryNode {
    directories: BTreeMap<String, DirectoryNode>,
    files: Vec<(String, usize)>,
}

/// パスの一覧からツリーを組む（ディレクトリが先・名前順）。`collapsed` のディレクトリは中を出さない。
pub fn build_tree(files: &[FileDiff], collapsed: &HashSet<String>) -> Vec<TreeEntry> {
    let mut root = DirectoryNode::default();
    for (index, file) in files.iter().enumerate() {
        let mut node = &mut root;
        let mut parts: Vec<&str> = file.path.split('/').collect();
        let name = parts.pop().unwrap_or_default().to_string();
        for part in parts {
            node = node.directories.entry(part.to_string()).or_default();
        }
        node.files.push((name, index));
    }
    let mut entries = Vec::new();
    flatten_tree(&root, "", 0, collapsed, &mut entries);
    entries
}

fn flatten_tree(
    node: &DirectoryNode,
    prefix: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    entries: &mut Vec<TreeEntry>,
) {
    for (name, child) in &node.directories {
        let mut label = name.clone();
        let mut path = join_path(prefix, name);
        let mut current = child;
        while current.files.is_empty() && current.directories.len() == 1 {
            let Some((next_name, next)) = current.directories.iter().next() else {
                break;
            };
            label = format!("{label}/{next_name}");
            path = join_path(&path, next_name);
            current = next;
        }
        let open = !collapsed.contains(&path);
        entries.push(TreeEntry::Directory {
            path: path.clone(),
            label,
            depth,
        });
        if open {
            flatten_tree(current, &path, depth + 1, collapsed, entries);
        }
    }
    let mut files = node.files.clone();
    files.sort();
    for (label, file) in files {
        entries.push(TreeEntry::File { file, label, depth });
    }
}

fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project::review::{parse_unified_diff, untracked_file_diff};

    fn patch(lines: &[&str]) -> String {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    /// 2 hunk（2〜4 行目と 40〜43 行目の近く）を持つ 60 行のファイル。
    fn two_hunk_file() -> FileDiff {
        let text = patch(&[
            "diff --git a/a.rs b/a.rs",
            "--- a/a.rs",
            "+++ b/a.rs",
            "@@ -2,3 +2,3 @@",
            " l2",
            "-l3",
            "+L3",
            " l4",
            "@@ -40,3 +40,4 @@ fn tail()",
            " l40",
            "+new",
            " l41",
            " l42",
        ]);
        parse_unified_diff(&text).remove(0)
    }

    #[test]
    fn gaps_cover_leading_between_and_trailing() {
        let file = two_hunk_file();
        let gaps = file_gaps(&file.hunks, Some(61));
        assert_eq!(
            gaps,
            vec![
                Gap {
                    index: 0,
                    position: GapPosition::Leading,
                    old_start: 1,
                    new_start: 1,
                    len: 1
                },
                Gap {
                    index: 1,
                    position: GapPosition::Between,
                    old_start: 5,
                    new_start: 5,
                    len: 35
                },
                Gap {
                    index: 2,
                    position: GapPosition::Trailing,
                    old_start: 43,
                    new_start: 44,
                    len: 18
                },
            ]
        );
        // 全文が無ければ末尾は数えられない（出さない）。
        assert_eq!(file_gaps(&file.hunks, None).len(), 2);
    }

    #[test]
    fn folding_and_expanding_keeps_line_numbers_aligned() {
        let file = two_hunk_file();
        let files = vec![file];
        let totals = vec![Some(61)];
        let mut expansions = HashMap::new();
        let collapsed = HashSet::new();
        let rows = build_rows(&RowInputs {
            files: &files,
            new_totals: &totals,
            expansions: &expansions,
            collapsed: &collapsed,
        });
        let folds: Vec<u32> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Fold { hidden, .. } => Some(*hidden),
                _ => None,
            })
            .collect();
        assert_eq!(folds, vec![1, 35, 18], "3 つの領域が畳まれる");
        let section = rows.iter().find_map(|row| match row {
            Row::Fold { gap, section, .. } if gap.index == 1 => Some(section.clone()),
            _ => None,
        });
        assert_eq!(
            section.as_deref(),
            Some("fn tail()"),
            "直後の hunk の見出し"
        );

        // 間の 35 行を 1 回押す = 上下 20 行ずつ…ではなく残り 35 ≤ 40 なので全部開く。
        let between = file_gaps(&files[0].hunks, Some(61))[1];
        let opened = expand_gap(&between, GapExpansion::default());
        assert_eq!(opened.hidden(&between), 0);
        expansions.insert(("a.rs".to_string(), 1), opened);
        let rows = build_rows(&RowInputs {
            files: &files,
            new_totals: &totals,
            expansions: &expansions,
            collapsed: &collapsed,
        });
        let context: Vec<(u32, u32)> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Context {
                    old_line, new_line, ..
                } => Some((*old_line, *new_line)),
                _ => None,
            })
            .collect();
        assert_eq!(context.len(), 35);
        assert_eq!(context.first(), Some(&(5, 5)));
        assert_eq!(context.last(), Some(&(39, 39)));

        // 末尾（18 行）は hunk 側から 20 行 = 全部。新側 44 行目 = 旧側 43 行目（1 行追加の後）。
        let trailing = file_gaps(&files[0].hunks, Some(61))[2];
        let opened = expand_gap(&trailing, GapExpansion::default());
        assert_eq!((opened.top, opened.bottom), (18, 0));
        expansions.insert(("a.rs".to_string(), 2), opened);
        let rows = build_rows(&RowInputs {
            files: &files,
            new_totals: &totals,
            expansions: &expansions,
            collapsed: &collapsed,
        });
        assert!(rows.contains(&Row::Context {
            file: 0,
            old_line: 43,
            new_line: 44
        }));
    }

    #[test]
    fn large_gaps_open_twenty_lines_from_each_side() {
        let gap = Gap {
            index: 1,
            position: GapPosition::Between,
            old_start: 10,
            new_start: 12,
            len: 100,
        };
        let once = expand_gap(&gap, GapExpansion::default());
        assert_eq!((once.top, once.bottom, once.hidden(&gap)), (20, 20, 60));
        let twice = expand_gap(&gap, once);
        assert_eq!(twice.hidden(&gap), 20);
        let thrice = expand_gap(&gap, twice);
        assert_eq!(thrice.hidden(&gap), 0, "残りが 40 行以下なら全部開く");
        let leading = Gap {
            position: GapPosition::Leading,
            index: 0,
            ..gap
        };
        let opened = expand_gap(&leading, GapExpansion::default());
        assert_eq!(
            (opened.top, opened.bottom),
            (0, 20),
            "先頭は hunk 側（下）から開く"
        );
    }

    #[test]
    fn notices_and_collapsed_files_skip_lines() {
        let mut binary = untracked_file_diff("img.png", b"\x00\x01");
        binary.kind = project::review::FileChangeKind::Modified;
        let text = untracked_file_diff("b.txt", b"one\ntwo\n");
        let files = vec![binary, text];
        let totals = vec![None, Some(2)];
        let expansions = HashMap::new();
        let mut collapsed = HashSet::new();
        let rows = build_rows(&RowInputs {
            files: &files,
            new_totals: &totals,
            expansions: &expansions,
            collapsed: &collapsed,
        });
        assert_eq!(
            rows[..2],
            [
                Row::FileHeader { file: 0 },
                Row::Notice {
                    file: 0,
                    notice: Notice::Binary
                }
            ]
        );
        assert_eq!(rows.len(), 2 + 1 + 2, "追跡外は全行が出る（畳む所が無い）");
        collapsed.insert("b.txt".to_string());
        let rows = build_rows(&RowInputs {
            files: &files,
            new_totals: &totals,
            expansions: &expansions,
            collapsed: &collapsed,
        });
        assert_eq!(rows.last(), Some(&Row::FileHeader { file: 1 }));
    }

    #[test]
    fn tree_compresses_single_child_directories() {
        let files: Vec<FileDiff> = ["crates/a/src/lib.rs", "crates/a/src/main.rs", "README.md"]
            .iter()
            .map(|path| untracked_file_diff(path, b"x\n"))
            .collect();
        let tree = build_tree(&files, &HashSet::new());
        assert_eq!(
            tree,
            vec![
                TreeEntry::Directory {
                    path: "crates/a/src".into(),
                    label: "crates/a/src".into(),
                    depth: 0
                },
                TreeEntry::File {
                    file: 0,
                    label: "lib.rs".into(),
                    depth: 1
                },
                TreeEntry::File {
                    file: 1,
                    label: "main.rs".into(),
                    depth: 1
                },
                TreeEntry::File {
                    file: 2,
                    label: "README.md".into(),
                    depth: 0
                },
            ]
        );
        let collapsed: HashSet<String> = ["crates/a/src".to_string()].into_iter().collect();
        assert_eq!(build_tree(&files, &collapsed).len(), 2);
    }
}
