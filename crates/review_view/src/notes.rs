//! 変更レビューの注記（行コメント）: 対象・状態・保存形式・プロンプトの整形。GPUI 非依存。
//!
//! 注記は「対象 + 本文 + 状態」のデータ。対象は拡張できる形にしてある（今は diff の行だけ。
//! Design Mode の「ページの要素」も同じトレイに入れる予定なので、`NoteTarget` に種類を足すだけで済む）。
//! 保存は storage の `review_notes`（対象は JSON 1 列・種類は `target_kind`）。
//!
//! 参考: stablyai/orca@646e9a5 の `src/shared/diff-comment-types.ts` と
//! `src/shared/diff-comments-format.ts`（注記の持ち方・未送信だけをまとめて 1 通で送る考え方）。
//! プロンプトの形（抜粋のコードフェンス・比較の基準の見出し）は necoder の仕様で独自に組んだ。

use serde::{Deserialize, Serialize};
use storage::{ReviewNoteRecord, ReviewNoteState};

/// 1 通のプロンプトに載せる抜粋の上限（行）。長い範囲は先頭だけ見せて本文で補ってもらう。
pub const EXCERPT_MAX_LINES: usize = 40;

/// 注記を付けた側。追加行・文脈行は新しい側、削除行は古い側の行番号に付ける。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteSide {
    Old,
    New,
}

/// 抜粋の 1 行の種別（diff の記号）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcerptKind {
    Context,
    Added,
    Removed,
}

impl ExcerptKind {
    fn marker(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Added => '+',
            Self::Removed => '-',
        }
    }
}

/// 抜粋の 1 行（注記を付けた時点の diff の行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcerptLine {
    pub kind: ExcerptKind,
    pub text: String,
}

/// diff の行（範囲）への注記の対象。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLinesTarget {
    /// リポジトリルート相対のパス（`/` 区切り）。
    pub path: String,
    pub side: NoteSide,
    /// 1 始まり・両端を含む。
    pub start: u32,
    pub end: u32,
    /// 付けた時の比較の基準（解決済みの oid）。
    pub base: String,
    /// 付けた時点の該当行。
    pub excerpt: Vec<ExcerptLine>,
}

/// 注記の対象。`kind` の文字列が storage の `target_kind` になる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoteTarget {
    DiffLines(DiffLinesTarget),
}

impl NoteTarget {
    /// storage の `target_kind`。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::DiffLines(_) => "diff_lines",
        }
    }

    /// `path:10` / `path:10-14`。transcript のパスリンクでそのまま開ける形。
    pub fn location(&self) -> String {
        match self {
            Self::DiffLines(target) if target.start == target.end => {
                format!("{}:{}", target.path, target.start)
            }
            Self::DiffLines(target) => format!("{}:{}-{}", target.path, target.start, target.end),
        }
    }
}

/// 注記 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewNote {
    pub id: String,
    pub target: NoteTarget,
    pub body: String,
    pub state: ReviewNoteState,
    pub sent_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ReviewNote {
    /// まだ解決していない（未送信 + 送信済み）。
    pub fn is_unresolved(&self) -> bool {
        self.state != ReviewNoteState::Resolved
    }

    /// 解決を戻す先（一度でも送っていれば送信済み、なければ未送信）。
    pub fn reopened_state(&self) -> ReviewNoteState {
        if self.sent_at.is_some() {
            ReviewNoteState::Sent
        } else {
            ReviewNoteState::Unsent
        }
    }

    pub fn to_record(&self, scope: &str) -> Result<ReviewNoteRecord, serde_json::Error> {
        Ok(ReviewNoteRecord {
            id: self.id.clone(),
            scope: scope.to_string(),
            target_kind: self.target.kind().to_string(),
            target: serde_json::to_string(&self.target)?,
            body: self.body.clone(),
            state: self.state,
            sent_at: self.sent_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }

    /// 保存形式から戻す。知らない種類の対象（新しい版で足したもの）は読めないので `Err`。
    pub fn from_record(record: &ReviewNoteRecord) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id: record.id.clone(),
            target: serde_json::from_str(&record.target)?,
            body: record.body.clone(),
            state: record.state,
            sent_at: record.sent_at,
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

/// 注記をまとめて 1 通のプロンプトにする。並びはパス → 行の順（読む順と同じ）。
///
/// ````text
/// レビューコメント（2 件）— 比較: 1a2b3c4..作業ツリー
/// 1. src/foo.rs:10-14
/// ```rust
/// （該当行の抜粋）
/// ```
/// （コメント本文）
/// ````
pub fn format_prompt(notes: &[&ReviewNote]) -> String {
    let mut notes: Vec<&ReviewNote> = notes.to_vec();
    notes.sort_by(|left, right| sort_key(left).cmp(&sort_key(right)));
    let mut bases: Vec<String> = Vec::new();
    for note in &notes {
        let NoteTarget::DiffLines(target) = &note.target;
        let short: String = target.base.chars().take(7).collect();
        if !bases.contains(&short) {
            bases.push(short);
        }
    }
    let mut out = i18n::t!(
        "review.prompt_header",
        "count" => notes.len(),
        "base" => bases.join(", ")
    );
    for (index, note) in notes.iter().enumerate() {
        out.push('\n');
        let NoteTarget::DiffLines(target) = &note.target;
        let location = note.target.location();
        let location = match target.side {
            NoteSide::New => location,
            NoteSide::Old => i18n::t!("review.prompt_old_side", "location" => location),
        };
        out.push_str(&format!("{}. {location}\n", index + 1));
        out.push_str(&excerpt_block(target));
        out.push_str(note.body.trim_end());
        out.push('\n');
    }
    out
}

fn sort_key(note: &ReviewNote) -> (String, u32, u8) {
    let NoteTarget::DiffLines(target) = &note.target;
    let side = match target.side {
        NoteSide::Old => 0,
        NoteSide::New => 1,
    };
    (target.path.clone(), target.start, side)
}

/// 抜粋のコードフェンス。削除行を含むなら `diff`（記号付き）、それ以外はファイルの言語。
/// 本文に ``` があればフェンスを伸ばして崩さない。
fn excerpt_block(target: &DiffLinesTarget) -> String {
    let with_markers = target
        .excerpt
        .iter()
        .any(|line| line.kind == ExcerptKind::Removed);
    let language = if with_markers {
        "diff".to_string()
    } else {
        lang::language_for_path(std::path::Path::new(&target.path))
            .map(|language| language.canonical_id().to_string())
            .unwrap_or_default()
    };
    let mut fence = "```".to_string();
    while target
        .excerpt
        .iter()
        .any(|line| line.text.contains(fence.as_str()))
    {
        fence.push('`');
    }
    let mut block = format!("{fence}{language}\n");
    for line in target.excerpt.iter().take(EXCERPT_MAX_LINES) {
        if with_markers {
            block.push(line.kind.marker());
        }
        block.push_str(&line.text);
        block.push('\n');
    }
    if target.excerpt.len() > EXCERPT_MAX_LINES {
        block.push_str("…\n");
    }
    block.push_str(&fence);
    block.push('\n');
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(
        id: &str,
        path: &str,
        side: NoteSide,
        range: (u32, u32),
        excerpt: &[(ExcerptKind, &str)],
        body: &str,
    ) -> ReviewNote {
        ReviewNote {
            id: id.to_string(),
            target: NoteTarget::DiffLines(DiffLinesTarget {
                path: path.to_string(),
                side,
                start: range.0,
                end: range.1,
                base: "1a2b3c4d5e6f".to_string(),
                excerpt: excerpt
                    .iter()
                    .map(|(kind, text)| ExcerptLine {
                        kind: *kind,
                        text: text.to_string(),
                    })
                    .collect(),
            }),
            body: body.to_string(),
            state: ReviewNoteState::Unsent,
            sent_at: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn prompt_bundles_notes_in_reading_order() {
        i18n::set_locale("ja");
        let second = note(
            "b",
            "src/lib.rs",
            NoteSide::Old,
            (7, 7),
            &[(ExcerptKind::Removed, "fn old() {}")],
            "消さないで",
        );
        let first = note(
            "a",
            "src/foo.rs",
            NoteSide::New,
            (10, 11),
            &[
                (ExcerptKind::Added, "let x = 1;"),
                (ExcerptKind::Context, "run(x);"),
            ],
            "名前を value に\n",
        );
        let prompt = format_prompt(&[&second, &first]);
        assert_eq!(
            prompt,
            "レビューコメント（2 件）— 比較: 1a2b3c4..作業ツリー\n\
             1. src/foo.rs:10-11\n\
             ```rust\n\
             let x = 1;\n\
             run(x);\n\
             ```\n\
             名前を value に\n\
             \n\
             2. src/lib.rs:7（削除された行・比較元の行番号）\n\
             ```diff\n\
             -fn old() {}\n\
             ```\n\
             消さないで\n"
        );
    }

    /// 送った本文の `path:行` は transcript の既存のリンク検出（`ui::links`）でそのまま開ける
    /// （範囲 `10-14` は先頭の 10 行目へ）。形を変えたらここが落ちる。
    #[test]
    fn prompt_locations_are_transcript_links() {
        let range = note(
            "a",
            "src/foo.rs",
            NoteSide::New,
            (10, 14),
            &[(ExcerptKind::Added, "x")],
            "本文",
        );
        let single = note(
            "b",
            "crates/b.rs",
            NoteSide::New,
            (3, 3),
            &[(ExcerptKind::Added, "y")],
            "本文",
        );
        let prompt = format_prompt(&[&range, &single]);
        let paths: Vec<(String, Option<u32>)> = ui::links::find_links(&prompt)
            .into_iter()
            .filter_map(|link| match link.target {
                ui::links::LinkTarget::Path { path, line, .. } => Some((path, line)),
                ui::links::LinkTarget::Url(_) => None,
            })
            .collect();
        assert!(
            paths.contains(&("src/foo.rs".to_string(), Some(10))),
            "{paths:?}"
        );
        assert!(
            paths.contains(&("crates/b.rs".to_string(), Some(3))),
            "{paths:?}"
        );
    }

    #[test]
    fn fences_grow_past_backticks_and_long_excerpts_are_cut() {
        let lines: Vec<(ExcerptKind, &str)> = (0..EXCERPT_MAX_LINES + 5)
            .map(|_| (ExcerptKind::Added, "```"))
            .collect();
        let long = note("a", "README.md", NoteSide::New, (1, 45), &lines, "長い");
        let NoteTarget::DiffLines(target) = &long.target;
        let block = excerpt_block(target);
        assert!(block.starts_with("````markdown\n"), "{block}");
        assert!(block.ends_with("…\n````\n"));
        assert_eq!(block.lines().count(), EXCERPT_MAX_LINES + 3);
    }

    #[test]
    fn records_round_trip_and_keep_the_target_kind() {
        let original = note(
            "a",
            "src/foo.rs",
            NoteSide::New,
            (3, 3),
            &[(ExcerptKind::Added, "x")],
            "本文",
        );
        let record = original.to_record("task-1").expect("JSON にできる");
        assert_eq!(record.target_kind, "diff_lines");
        assert!(record.target.contains("\"kind\":\"diff_lines\""));
        assert_eq!(ReviewNote::from_record(&record).expect("戻せる"), original);
        let mut unknown = record.clone();
        unknown.target = "{\"kind\":\"page_element\"}".to_string();
        assert!(
            ReviewNote::from_record(&unknown).is_err(),
            "知らない種類は読まない"
        );
        let mut sent = original.clone();
        sent.sent_at = Some(5);
        assert_eq!(sent.reopened_state(), ReviewNoteState::Sent);
        assert_eq!(original.reopened_state(), ReviewNoteState::Unsent);
    }
}
