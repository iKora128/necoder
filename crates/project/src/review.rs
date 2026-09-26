//! 変更レビューのための構造化 diff（GPUI 非依存・テスト可能）。
//!
//! 「worktree の変更を 1 画面でレビューする」ために、比較の基準（Task の base / HEAD / ブランチの
//! 分岐点 / 任意のコミット）から**作業ツリー**までの差分を、ファイル → hunk → 行（種別・旧行番号・
//! 新行番号・本文）へ構造化する。追跡済みの変更は `git diff <base>` の unified diff を解析し、
//! 追跡外のファイルは全行追加として足す。描画・畳み・注記は `review_view` crate が持つ
//! （この crate は UI を知らない）。
//!
//! git の呼び出しは全て [`crate::run_git`] を通す（未信頼 repo の防御をそのまま効かせる）。
//! 出力の形は `--src-prefix` / `--dst-prefix` / `--no-ext-diff` / `--no-textconv` /
//! `core.quotepath=false` で固定し、ユーザーの gitconfig（mnemonicPrefix・外部 diff 等）に左右されない。

use crate::{git_repo_root_on, run_git};
use host::Host;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// hunk の前後に付ける文脈の行数（`git diff -U3` と同じ）。
pub const CONTEXT_LINES: u32 = 3;
/// 1 ファイルの変更行（追加 + 削除）がこれを超えたら中身を出さず「大きすぎる」にする。
pub const MAX_FILE_CHANGED_LINES: usize = 5_000;
/// レビュー全体の変更行の上限。超えた分のファイルは「大きすぎる」扱い（開けるだけ）。
pub const MAX_TOTAL_CHANGED_LINES: usize = 60_000;
/// 全文（ハイライト・畳みの展開用）と追跡外ファイルを読む上限。
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// 追跡外ファイルの中身を読む上限の件数（超えた分は「大きすぎる」で並べるだけ）。
pub const MAX_UNTRACKED_FILES: usize = 1_000;
/// 1 ファイルの差分の行の本文の合計がこれを超えたら、中身を捨てて「大きすぎる」にする（R02）。
/// 行数の上限だけでは、1 行が数 MB あるファイル（圧縮した JS 等）の 1 行の変更を止められない。
pub const MAX_FILE_PATCH_BYTES: usize = 512 * 1024;
/// レビュー全体で持つ差分の行の本文の上限（追跡済み + 追跡外・R02）。超えた分のファイルは「大きすぎる」。
pub const MAX_TOTAL_PATCH_BYTES: usize = 16 * 1024 * 1024;
/// パッチを取る前に、旧側か新側の中身がこれより大きいファイルは除く（R02・git の出力を
/// 丸ごと読む前に止める。ここを抜けても [`MAX_FILE_PATCH_BYTES`] で中身は持たない）。
pub const MAX_DIFF_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// 全文（ハイライト・畳みの展開用）をレビュー全体で持つ上限（R02）。超えた分のファイルは
/// 色を付けず、畳みも開けない（差分の行はそのまま見える）。
pub const MAX_TOTAL_TEXT_BYTES: usize = 24 * 1024 * 1024;

/// 比較の基準（解決前）。UI が選び、[`resolve_review_base_on`] がコミットに解決する。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReviewBase {
    /// HEAD。未コミットの変更（staged + unstaged + 追跡外）だけを見る。
    Head,
    /// 任意のコミット（oid・短縮 sha・`HEAD~3` など rev として解決できるもの）。Task の base もこれ。
    Commit(String),
    /// ブランチ。ブランチと HEAD の**分岐点**（merge-base）からの変更を見る（PR と同じ見え方）。
    Branch(String),
}

/// ファイル単位の変更の種類。表示の記号は [`FileChangeKind::letter`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// 追跡外（`git add` されていない新規ファイル）。全行を追加として扱う。
    Untracked,
}

impl FileChangeKind {
    /// ソース管理パネルと同じ 1 文字（A / M / D / R / U）。
    pub fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
        }
    }
}

/// diff の 1 行の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// diff の 1 行。行番号は 1 始まり。追加行は旧側、削除行は新側の番号を持たない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// 行頭の記号（`+` / `-` / ` `）と行末の改行（CR を含む）を除いた本文。
    pub text: String,
    /// この行の後に `\ No newline at end of file` が付いていた（ファイル末尾に改行が無い）。
    pub no_newline: bool,
}

/// 1 hunk（`@@ -a,b +c,d @@ 見出し` とその行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// `@@ … @@` の後ろの見出し（git が拾った関数名など）。無ければ空。
    pub section: String,
    pub lines: Vec<DiffLine>,
}

impl ReviewHunk {
    /// 旧側で覆う行（1 始まり・半開）。件数 0 の hunk は「start の直後」の空範囲。
    pub fn old_span(&self) -> Range<u32> {
        line_span(self.old_start, self.old_count)
    }

    /// 新側で覆う行（1 始まり・半開）。件数 0 の hunk は「start の直後」の空範囲。
    pub fn new_span(&self) -> Range<u32> {
        line_span(self.new_start, self.new_count)
    }
}

/// hunk 見出しの `start,count` を 1 始まり半開の行範囲へ。git は件数 0 のとき
/// start に「直前の行」を書く（`@@ -5,0 +6,2 @@` = 旧 5 行目の後ろへ挿入）。
fn line_span(start: u32, count: u32) -> Range<u32> {
    if count == 0 {
        start + 1..start + 1
    } else {
        start..start + count
    }
}

/// 1 ファイルの差分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// リポジトリルート相対のパス（`/` 区切り・変更後の名前）。
    pub path: String,
    /// 改名元（改名のときだけ）。
    pub old_path: Option<String>,
    pub kind: FileChangeKind,
    /// バイナリ（中身は出さない）。
    pub binary: bool,
    /// 大きすぎる（中身は出さず、開けるだけにする）。
    pub too_large: bool,
    /// 権限（実行ビット等）の変更を伴う。中身の変更が無ければこれだけが差分。
    pub mode_changed: bool,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<ReviewHunk>,
}

impl FileDiff {
    fn new(path: String) -> Self {
        Self {
            path,
            old_path: None,
            kind: FileChangeKind::Modified,
            binary: false,
            too_large: false,
            mode_changed: false,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
        }
    }

    /// 比較元（旧側）でのパス。改名なら改名元。
    pub fn base_path(&self) -> &str {
        self.old_path.as_deref().unwrap_or(&self.path)
    }

    /// 解析した行から追加・削除の数を数え直す。
    fn recount(&mut self) {
        let lines = self.hunks.iter().flat_map(|hunk| hunk.lines.iter());
        let (mut additions, mut deletions) = (0, 0);
        for line in lines {
            match line.kind {
                DiffLineKind::Added => additions += 1,
                DiffLineKind::Removed => deletions += 1,
                DiffLineKind::Context => {}
            }
        }
        self.additions = additions;
        self.deletions = deletions;
    }
}

/// 比較の基準から作業ツリーまでの差分一式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDiff {
    /// リポジトリのルート（ファイルを開くときは `repo_root.join(path)`）。
    pub repo_root: PathBuf,
    /// 解決済みの基準（コミットの oid。コミットの無い repo の HEAD は空のツリー）。
    pub base_oid: String,
    /// パス順。
    pub files: Vec<FileDiff>,
}

/// 変更レビューを作れなかった理由。UI はこれを i18n の文言へ写す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewError {
    /// git のリポジトリではない（git が無い場合も含む）。
    NotRepository,
    /// 比較の基準（ブランチ / コミット）が見つからない。
    UnknownBase(String),
    /// git の実行に失敗した（stderr の要約）。
    Git(String),
}

impl std::fmt::Display for ReviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRepository => write!(formatter, "git リポジトリではない"),
            Self::UnknownBase(rev) => write!(formatter, "比較の基準が見つからない: {rev}"),
            Self::Git(message) => write!(formatter, "git に失敗: {message}"),
        }
    }
}

impl std::error::Error for ReviewError {}

/// 1 ファイルの旧側・新側の全文（ハイライトと畳みの展開に使う）。読めなければ `None`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTexts {
    pub old: Option<String>,
    pub new: Option<String>,
}

// ── git の読み取り API（全て run_git 経由） ──

/// rev をコミットの oid に解決する（`git rev-parse --verify <rev>^{commit}`）。無ければ `None`。
/// `-` で始まる値はオプション注入を避けるため解決しない。
pub fn git_resolve_commit_on(host: &dyn Host, dir: &Path, rev: &str) -> Option<String> {
    if rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let spec = format!("{rev}^{{commit}}");
    let output = run_git(
        host,
        dir,
        ["rev-parse", "--verify", "--quiet", spec.as_str()],
    )
    .ok()?;
    if !output.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!oid.is_empty()).then_some(oid)
}

/// 2 つの rev の分岐点（`git merge-base a b`）。無関係な履歴・失敗は `None`。
pub fn git_merge_base_on(host: &dyn Host, dir: &Path, left: &str, right: &str) -> Option<String> {
    if left.starts_with('-') || right.starts_with('-') {
        return None;
    }
    let output = run_git(host, dir, ["merge-base", left, right]).ok()?;
    if !output.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    (!oid.is_empty()).then_some(oid)
}

/// 任意の rev のファイル内容（`git show <rev>:<path>`・`path` はリポジトリルート相対）。
/// そのコミットに無いファイル・失敗は `None`。textconv は通さない（生のバイト列）。
pub fn git_show_file_on(
    host: &dyn Host,
    repo_root: &Path,
    rev: &str,
    path: &str,
) -> Option<Vec<u8>> {
    if rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let spec = format!("{rev}:{path}");
    let output = run_git(
        host,
        repo_root,
        [
            "--no-optional-locks",
            "show",
            "--no-textconv",
            spec.as_str(),
        ],
    )
    .ok()?;
    output.success().then_some(output.stdout)
}

/// 空のツリーの oid（コミットの無い repo で「HEAD と比較」する時の基準）。
/// オブジェクト形式（sha1 / sha256）に合わせて git に計算させる（`-w` を付けないので書き込まない）。
fn empty_tree_oid_on(host: &dyn Host, repo_root: &Path) -> Option<String> {
    let output = run_git(host, repo_root, ["hash-object", "-t", "tree", "--stdin"]).ok()?;
    if !output.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!oid.is_empty()).then_some(oid)
}

/// 比較の基準をコミット（または空のツリー）の oid へ解決する。ブランチは HEAD との分岐点。
pub fn resolve_review_base_on(
    host: &dyn Host,
    repo_root: &Path,
    base: &ReviewBase,
) -> Result<String, ReviewError> {
    match base {
        ReviewBase::Head => git_resolve_commit_on(host, repo_root, "HEAD")
            .or_else(|| empty_tree_oid_on(host, repo_root))
            .ok_or_else(|| ReviewError::Git("HEAD を解決できない".to_string())),
        ReviewBase::Commit(rev) => git_resolve_commit_on(host, repo_root, rev)
            .ok_or_else(|| ReviewError::UnknownBase(rev.clone())),
        ReviewBase::Branch(name) => {
            let tip = git_resolve_commit_on(host, repo_root, name)
                .ok_or_else(|| ReviewError::UnknownBase(name.clone()))?;
            Ok(git_merge_base_on(host, repo_root, &tip, "HEAD").unwrap_or(tip))
        }
    }
}

/// `git diff` の共通の頭（出力の形を gitconfig に左右させない）。
fn diff_args(ignore_whitespace: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-c",
        "core.quotepath=false",
        "--no-optional-locks",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    if ignore_whitespace {
        args.push("-w".to_string());
    }
    args
}

fn git_failure(output: &host::CommandOutput) -> ReviewError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    ReviewError::Git(
        stderr
            .lines()
            .next()
            .unwrap_or("git diff")
            .trim()
            .to_string(),
    )
}

/// 基準コミットから作業ツリーまでの unified diff（`git diff <base> -- <pathspec>`）。
/// `paths` が空ならリポジトリ全体、`exclude` はパッチに含めないパス（大きすぎる・バイナリ）。
pub fn git_diff_patch_on(
    host: &dyn Host,
    repo_root: &Path,
    base_oid: &str,
    paths: &[String],
    exclude: &[String],
    ignore_whitespace: bool,
) -> Result<String, ReviewError> {
    if base_oid.starts_with('-') {
        return Err(ReviewError::UnknownBase(base_oid.to_string()));
    }
    let mut args = diff_args(ignore_whitespace);
    args.push(format!("-U{CONTEXT_LINES}"));
    args.push("--src-prefix=a/".to_string());
    args.push("--dst-prefix=b/".to_string());
    args.push(base_oid.to_string());
    args.push("--".to_string());
    if paths.is_empty() {
        args.push(".".to_string());
    } else {
        args.extend(paths.iter().map(|path| format!(":(literal){path}")));
    }
    args.extend(
        exclude
            .iter()
            .map(|path| format!(":(exclude,literal){path}")),
    );
    let output =
        run_git(host, repo_root, args).map_err(|error| ReviewError::Git(error.to_string()))?;
    if !output.success() {
        return Err(git_failure(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 追跡外のファイル（`git ls-files --others --exclude-standard`・ルート相対）。
pub fn git_untracked_files_on(host: &dyn Host, repo_root: &Path) -> Vec<String> {
    let output = run_git(
        host,
        repo_root,
        [
            "-c",
            "core.quotepath=false",
            "--no-optional-locks",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    );
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty() && !path.ends_with('/'))
        .map(str::to_string)
        .collect()
}

/// `--name-status -z` の 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
struct NameStatus {
    kind: FileChangeKind,
    path: String,
    old_path: Option<String>,
}

/// `git diff --name-status -z` を読む（`M\0path\0` / `R086\0old\0new\0`）。
fn parse_name_status_z(text: &str) -> Vec<NameStatus> {
    let mut fields = text.split('\0').filter(|field| !field.is_empty());
    let mut entries = Vec::new();
    while let Some(status) = fields.next() {
        let letter = status.chars().next().unwrap_or('M');
        let two_paths = matches!(letter, 'R' | 'C');
        let Some(first) = fields.next() else { break };
        let (old_path, path) = if two_paths {
            let Some(second) = fields.next() else { break };
            (Some(first.to_string()), second.to_string())
        } else {
            (None, first.to_string())
        };
        let kind = match letter {
            'A' => FileChangeKind::Added,
            'D' => FileChangeKind::Deleted,
            'R' => FileChangeKind::Renamed,
            // C（複写）・T（種別の変更）・M は「変更」として見せる。
            _ => FileChangeKind::Modified,
        };
        // 複写は複写元が残っているので改名元として扱わない。
        let old_path = old_path.filter(|_| letter == 'R');
        entries.push(NameStatus {
            kind,
            path,
            old_path,
        });
    }
    entries
}

/// `git diff --numstat -z` を読む。値 = (追加, 削除)・バイナリは `None`。キーは変更後のパス。
fn parse_numstat_z(text: &str) -> HashMap<String, Option<(usize, usize)>> {
    let mut counts = HashMap::new();
    let mut fields = text.split('\0');
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut columns = field.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        let value = match (added.parse::<usize>(), deleted.parse::<usize>()) {
            (Ok(added), Ok(deleted)) => Some((added, deleted)),
            _ => None,
        };
        // 改名は `A\tD\t\0old\0new\0`（パス欄が空で、続く 2 欄が旧・新）。
        let path = if path.is_empty() {
            let _old = fields.next();
            match fields.next() {
                Some(new_path) => new_path.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        counts.insert(path, value);
    }
    counts
}

/// 基準 `base` から作業ツリーまでの差分を構造化して返す（`dir` を含むリポジトリ全体）。
///
/// 追跡済みは `git diff <base>`（staged / unstaged をまとめて）、追跡外は全行追加。
/// 大きすぎるファイル（[`MAX_FILE_CHANGED_LINES`] / [`MAX_TOTAL_CHANGED_LINES`]）とバイナリは
/// パッチを取らず、`too_large` / `binary` の印だけで並べる。`ignore_whitespace` は `git diff -w`。
pub fn review_diff_on(
    host: &dyn Host,
    dir: &Path,
    base: &ReviewBase,
    ignore_whitespace: bool,
) -> Result<ReviewDiff, ReviewError> {
    review_diff_with_budgets_on(
        host,
        dir,
        base,
        ignore_whitespace,
        &ReviewBudgets::default(),
    )
}

/// レビューが持つ中身の上限（R02）。既定は各定数。テストは小さい値で上限の働きを確かめる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewBudgets {
    /// 変更行（追跡済み + 追跡外）の合計。
    pub total_lines: usize,
    /// 1 ファイルの差分の行の本文。
    pub file_patch_bytes: usize,
    /// 差分の行の本文の合計（追跡済み + 追跡外）。
    pub total_patch_bytes: usize,
    /// パッチを取る前に除くファイルの中身の大きさ。
    pub diff_file_bytes: u64,
}

impl Default for ReviewBudgets {
    fn default() -> Self {
        Self {
            total_lines: MAX_TOTAL_CHANGED_LINES,
            file_patch_bytes: MAX_FILE_PATCH_BYTES,
            total_patch_bytes: MAX_TOTAL_PATCH_BYTES,
            diff_file_bytes: MAX_DIFF_FILE_BYTES,
        }
    }
}

/// [`review_diff_on`] の本体（上限を渡せる）。
pub fn review_diff_with_budgets_on(
    host: &dyn Host,
    dir: &Path,
    base: &ReviewBase,
    ignore_whitespace: bool,
    budgets: &ReviewBudgets,
) -> Result<ReviewDiff, ReviewError> {
    let repo_root = git_repo_root_on(host, dir).ok_or(ReviewError::NotRepository)?;
    let base_oid = resolve_review_base_on(host, &repo_root, base)?;
    let listing = |extra: &str| -> Result<String, ReviewError> {
        let mut args = diff_args(ignore_whitespace);
        args.push(extra.to_string());
        args.push("-z".to_string());
        args.push(base_oid.clone());
        args.push("--".to_string());
        let output =
            run_git(host, &repo_root, args).map_err(|error| ReviewError::Git(error.to_string()))?;
        if !output.success() {
            return Err(git_failure(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let statuses = parse_name_status_z(&listing("--name-status")?);
    let counts = parse_numstat_z(&listing("--numstat")?);

    // パッチに入れるものを決める（パス順に積み、上限を超えた分は「大きすぎる」）。
    let mut statuses = statuses;
    statuses.sort_by(|left, right| tree_order(&left.path, &right.path));
    let mut files = Vec::new();
    let mut exclude = Vec::new();
    let mut budget = budgets.total_lines;
    for status in &statuses {
        let mut file = FileDiff::new(status.path.clone());
        file.kind = status.kind;
        file.old_path = status.old_path.clone();
        match counts.get(&status.path).copied().flatten() {
            Some((additions, deletions)) => {
                file.additions = additions;
                file.deletions = deletions;
                let changed = additions + deletions;
                if changed > MAX_FILE_CHANGED_LINES || changed > budget {
                    file.too_large = true;
                } else {
                    budget -= changed;
                }
            }
            // numstat の `-\t-` = バイナリ。
            None if counts.contains_key(&status.path) => file.binary = true,
            None => {}
        }
        if file.too_large || file.binary {
            exclude.push(file.path.clone());
            exclude.extend(file.old_path.clone());
        }
        files.push(file);
    }

    // 中身が大きすぎるファイルは、パッチを取る前に除く（R02）。旧側は `git ls-tree -l` 1 回、
    // 新側は作業ツリーの大きさ。どちらも読めなければ素通し（取った後の予算で止まる）。
    let sized: Vec<&FileDiff> = files
        .iter()
        .filter(|file| !file.too_large && !file.binary)
        .collect();
    if !sized.is_empty() {
        let old_paths: Vec<String> = sized
            .iter()
            .filter(|file| file.kind != FileChangeKind::Added)
            .map(|file| file.base_path().to_string())
            .collect();
        let old_sizes = git_blob_sizes_on(host, &repo_root, &base_oid, &old_paths);
        let oversized: Vec<String> = sized
            .iter()
            .filter(|file| {
                let old = old_sizes.get(file.base_path()).copied().unwrap_or(0);
                let new = if file.kind == FileChangeKind::Deleted {
                    0
                } else {
                    host.metadata(&repo_root.join(&file.path))
                        .map_or(0, |metadata| metadata.len)
                };
                old.max(new) > budgets.diff_file_bytes
            })
            .map(|file| file.path.clone())
            .collect();
        for file in files
            .iter_mut()
            .filter(|file| oversized.contains(&file.path))
        {
            file.too_large = true;
            exclude.push(file.path.clone());
            exclude.extend(file.old_path.clone());
        }
    }

    let mut byte_budget = budgets.total_patch_bytes;
    if files.iter().any(|file| !file.too_large && !file.binary) {
        let patch = git_diff_patch_on(
            host,
            &repo_root,
            &base_oid,
            &[],
            &exclude,
            ignore_whitespace,
        )?;
        let mut parsed: HashMap<String, FileDiff> = parse_unified_diff(&patch)
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect();
        for file in files
            .iter_mut()
            .filter(|file| !file.too_large && !file.binary)
        {
            if let Some(parsed) = parsed.remove(&file.path) {
                file.hunks = parsed.hunks;
                file.binary = parsed.binary;
                file.mode_changed = parsed.mode_changed;
                file.recount();
            }
        }
        // パッチ全体の文字列はここで手放す。持つ中身は予算の分だけ（R02）。
        drop(patch);
        apply_patch_byte_budget(&mut files, budgets.file_patch_bytes, &mut byte_budget);
    }
    if ignore_whitespace {
        // `-w` で空白の差しか無かったファイルは消す（それを隠すためのトグル）。
        files.retain(|file| {
            !(file.kind == FileChangeKind::Modified
                && file.hunks.is_empty()
                && !file.binary
                && !file.too_large
                && !file.mode_changed)
        });
    }

    // 並べるだけ（中身を読まない）の追跡外ファイル: 件数の上限を超えた分・大きすぎる・読めない。
    let listed_only = |path: String| {
        let mut file = FileDiff::new(path);
        file.kind = FileChangeKind::Untracked;
        file.too_large = true;
        file
    };
    // 追跡外も、追跡済みの残りの行数・バイト数の予算の中で読む（R02・以前は全体の予算を減らさず、
    // 1MB × 1,000 件まで読めた）。予算を超えた分は並べるだけ。
    let mut line_budget = budget;
    let untracked = git_untracked_files_on(host, &repo_root);
    for (index, path) in untracked.into_iter().enumerate() {
        let absolute = repo_root.join(&path);
        let size = host.metadata(&absolute).map_or(0, |metadata| metadata.len);
        let oversized = index >= MAX_UNTRACKED_FILES
            || size > MAX_TEXT_BYTES as u64
            || size > byte_budget as u64;
        let file = if oversized {
            listed_only(path)
        } else {
            match host.read_file(&absolute) {
                Ok(content) => {
                    let file = untracked_file_diff(&path, &content.bytes);
                    let bytes = patch_bytes(&file);
                    if file.additions > line_budget || bytes > byte_budget {
                        listed_only(path)
                    } else {
                        line_budget -= file.additions;
                        byte_budget -= bytes;
                        file
                    }
                }
                Err(_) => listed_only(path),
            }
        };
        files.push(file);
    }
    files.sort_by(|left, right| tree_order(&left.path, &right.path));
    Ok(ReviewDiff {
        repo_root,
        base_oid,
        files,
    })
}

/// 差分の行の本文の合計（バイト）。レビューが持つ中身の量の目安（R02）。
pub fn patch_bytes(file: &FileDiff) -> usize {
    file.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .map(|line| line.text.len())
        .sum()
}

/// 解析した差分に、1 ファイル（`per_file`）と全体（`remaining`）のバイトの上限を掛ける（R02・
/// パス順に積む）。超えたファイルは中身を捨てて「大きすぎる」にする（開けるだけ）。追加・削除の数は
/// 解析した時のまま残す。
pub fn apply_patch_byte_budget(files: &mut [FileDiff], per_file: usize, remaining: &mut usize) {
    for file in files.iter_mut().filter(|file| !file.hunks.is_empty()) {
        let bytes = patch_bytes(file);
        if bytes > per_file || bytes > *remaining {
            file.hunks = Vec::new();
            file.too_large = true;
        } else {
            *remaining -= bytes;
        }
    }
}

/// 基準のツリーでのファイルの大きさ（`git ls-tree -l -z <base> -- <paths>`・R02 の事前確認）。
/// 読めなかったものは入らない（呼び出し側は 0 とみなす）。
fn git_blob_sizes_on(
    host: &dyn Host,
    repo_root: &Path,
    base_oid: &str,
    paths: &[String],
) -> HashMap<String, u64> {
    let mut sizes = HashMap::new();
    if paths.is_empty() || base_oid.is_empty() || base_oid.starts_with('-') {
        return sizes;
    }
    let mut args: Vec<String> = [
        "-c",
        "core.quotepath=false",
        "--no-optional-locks",
        "ls-tree",
        "-l",
        "-z",
        base_oid,
        "--",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    args.extend(paths.iter().cloned());
    let Ok(output) = run_git(host, repo_root, args) else {
        return sizes;
    };
    if !output.success() {
        return sizes;
    }
    // `<mode> SP <type> SP <oid> SP+ <size> TAB <path>`（size は右寄せの空白つき。tree は `-`）。
    for entry in String::from_utf8_lossy(&output.stdout).split('\0') {
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        if let Some(size) = meta
            .split_whitespace()
            .nth(3)
            .and_then(|size| size.parse::<u64>().ok())
        {
            sizes.insert(path.to_string(), size);
        }
    }
    sizes
}

/// ファイルツリーと同じ並び（各階層でディレクトリが先・名前順）。レビューの縦の並びとツリーを揃える。
pub fn tree_order(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_parts = left.split('/').peekable();
    let mut right_parts = right.split('/').peekable();
    loop {
        match (left_parts.next(), right_parts.next()) {
            (Some(left_part), Some(right_part)) => {
                // この階層で片方がディレクトリ（まだ続きがある）なら、ディレクトリが先。
                let left_is_directory = left_parts.peek().is_some();
                let right_is_directory = right_parts.peek().is_some();
                if left_is_directory != right_is_directory {
                    return right_is_directory.cmp(&left_is_directory);
                }
                match left_part.cmp(right_part) {
                    std::cmp::Ordering::Equal => continue,
                    other => return other,
                }
            }
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
        }
    }
}

/// git と同じ判定（先頭 8000 バイトに NUL があればバイナリ）。
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|byte| *byte == 0)
}

/// テキストを行へ割る。末尾の改行は行に含めず、最後の行が改行で終わらなければ `true` を返す。
fn split_text_lines(text: &str) -> (Vec<&str>, bool) {
    if text.is_empty() {
        return (Vec::new(), false);
    }
    let missing_newline = !text.ends_with('\n');
    let body = text.strip_suffix('\n').unwrap_or(text);
    let lines = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    (lines, missing_newline)
}

/// 追跡外ファイルを「全行追加」の差分にする。バイナリ・大きすぎるものは印だけ。
pub fn untracked_file_diff(path: &str, bytes: &[u8]) -> FileDiff {
    let mut file = FileDiff::new(path.to_string());
    file.kind = FileChangeKind::Untracked;
    if looks_binary(bytes) {
        file.binary = true;
        return file;
    }
    if bytes.len() > MAX_TEXT_BYTES {
        file.too_large = true;
        return file;
    }
    let text = String::from_utf8_lossy(bytes);
    let (lines, missing_newline) = split_text_lines(&text);
    if lines.len() > MAX_FILE_CHANGED_LINES {
        file.too_large = true;
        file.additions = lines.len();
        return file;
    }
    if lines.is_empty() {
        return file;
    }
    let count = lines.len() as u32;
    let last = lines.len() - 1;
    let lines = lines
        .into_iter()
        .enumerate()
        .map(|(index, text)| DiffLine {
            kind: DiffLineKind::Added,
            old_line: None,
            new_line: Some(index as u32 + 1),
            text: text.to_string(),
            no_newline: missing_newline && index == last,
        })
        .collect();
    file.hunks.push(ReviewHunk {
        old_start: 0,
        old_count: 0,
        new_start: 1,
        new_count: count,
        section: String::new(),
        lines,
    });
    file.recount();
    file
}

/// 旧側・新側の全文を読む（ハイライトと畳みの展開用）。バイナリ・大きすぎるもの・
/// UTF-8 でないもの・[`MAX_TEXT_BYTES`] を超えるものは `None`。
pub fn review_file_texts_on(
    host: &dyn Host,
    repo_root: &Path,
    base_oid: &str,
    file: &FileDiff,
) -> FileTexts {
    if file.binary || file.too_large {
        return FileTexts::default();
    }
    // 新側が大きすぎるなら両側とも読まない（旧側もほぼ同じ大きさ・全文を持つとメモリを食う）。
    let new_path = repo_root.join(&file.path);
    if file.kind != FileChangeKind::Deleted
        && host
            .metadata(&new_path)
            .is_ok_and(|metadata| metadata.len > MAX_TEXT_BYTES as u64)
    {
        return FileTexts::default();
    }
    let decode = |bytes: Vec<u8>| -> Option<String> {
        if bytes.len() > MAX_TEXT_BYTES || looks_binary(&bytes) {
            return None;
        }
        String::from_utf8(bytes).ok()
    };
    let old = match file.kind {
        FileChangeKind::Added | FileChangeKind::Untracked => None,
        _ => git_show_file_on(host, repo_root, base_oid, file.base_path()).and_then(decode),
    };
    let new = match file.kind {
        FileChangeKind::Deleted => None,
        _ => host
            .read_file(&new_path)
            .ok()
            .and_then(|content| decode(content.bytes)),
    };
    FileTexts { old, new }
}

// ── unified diff の解析 ──

/// 解析中の hunk と、次に来る行の番号・残りの行数。
struct HunkCursor {
    hunk: ReviewHunk,
    old_next: u32,
    new_next: u32,
    old_left: u32,
    new_left: u32,
}

/// `git diff` の unified diff を構造化する（純関数）。改名・バイナリ・新規 / 削除・権限の変更・
/// `\ No newline at end of file`・引用符付きのパス（`"a/q\"x"`）を扱う。
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut cursor: Option<HunkCursor> = None;

    let close_hunk = |file: &mut Option<FileDiff>, cursor: &mut Option<HunkCursor>| {
        if let (Some(file), Some(done)) = (file.as_mut(), cursor.take()) {
            file.hunks.push(done.hunk);
        }
    };

    for line in text.split('\n') {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            close_hunk(&mut current, &mut cursor);
            if let Some(mut done) = current.take() {
                done.recount();
                files.push(done);
            }
            let (old_path, path) = parse_git_header_paths(rest);
            let mut file = FileDiff::new(path.clone());
            if old_path != path {
                file.old_path = Some(old_path);
            }
            current = Some(file);
            continue;
        }
        if current.is_none() {
            continue;
        }
        if let Some(active) = cursor.as_mut() {
            if line.starts_with('\\') {
                // `\ No newline at end of file` は直前の行に付く（文脈行の後ろにも来る）。
                if let Some(last) = active.hunk.lines.last_mut() {
                    last.no_newline = true;
                }
                continue;
            }
            if active.old_left > 0 || active.new_left > 0 {
                let (kind, body) = match line.chars().next() {
                    Some('+') => (DiffLineKind::Added, &line[1..]),
                    Some('-') => (DiffLineKind::Removed, &line[1..]),
                    Some(' ') => (DiffLineKind::Context, &line[1..]),
                    // 末尾空白を削るエディタを通った diff では空の文脈行が "" になる。
                    None => (DiffLineKind::Context, ""),
                    Some(_) => {
                        close_hunk(&mut current, &mut cursor);
                        continue;
                    }
                };
                let text = body.strip_suffix('\r').unwrap_or(body).to_string();
                let (old_line, new_line) = match kind {
                    DiffLineKind::Added => {
                        let number = active.new_next;
                        active.new_next += 1;
                        active.new_left = active.new_left.saturating_sub(1);
                        (None, Some(number))
                    }
                    DiffLineKind::Removed => {
                        let number = active.old_next;
                        active.old_next += 1;
                        active.old_left = active.old_left.saturating_sub(1);
                        (Some(number), None)
                    }
                    DiffLineKind::Context => {
                        let numbers = (active.old_next, active.new_next);
                        active.old_next += 1;
                        active.new_next += 1;
                        active.old_left = active.old_left.saturating_sub(1);
                        active.new_left = active.new_left.saturating_sub(1);
                        (Some(numbers.0), Some(numbers.1))
                    }
                };
                active.hunk.lines.push(DiffLine {
                    kind,
                    old_line,
                    new_line,
                    text,
                    no_newline: false,
                });
                continue;
            }
            close_hunk(&mut current, &mut cursor);
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            if let Some((old_start, old_count, new_start, new_count, section)) =
                parse_hunk_header(line)
            {
                cursor = Some(HunkCursor {
                    old_next: old_start.max(1),
                    new_next: new_start.max(1),
                    old_left: old_count,
                    new_left: new_count,
                    hunk: ReviewHunk {
                        old_start,
                        old_count,
                        new_start,
                        new_count,
                        section,
                        lines: Vec::new(),
                    },
                });
            }
        } else if line.starts_with("new file mode") {
            file.kind = FileChangeKind::Added;
        } else if line.starts_with("deleted file mode") {
            file.kind = FileChangeKind::Deleted;
        } else if let Some(from) = line.strip_prefix("rename from ") {
            file.old_path = Some(unquote_path(from));
            file.kind = FileChangeKind::Renamed;
        } else if let Some(to) = line.strip_prefix("rename to ") {
            file.path = unquote_path(to);
            file.kind = FileChangeKind::Renamed;
        } else if line.starts_with("old mode") || line.starts_with("new mode") {
            file.mode_changed = true;
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            file.binary = true;
        } else if let Some(old) = line.strip_prefix("--- ") {
            if let Some(path) = side_path(old, "a/") {
                if file.kind != FileChangeKind::Renamed && path != file.path {
                    file.old_path = Some(path);
                }
            }
        } else if let Some(new) = line.strip_prefix("+++ ") {
            if let Some(path) = side_path(new, "b/") {
                file.path = path;
            }
        }
    }
    close_hunk(&mut current, &mut cursor);
    if let Some(mut done) = current.take() {
        done.recount();
        files.push(done);
    }
    files
}

/// `@@ -a[,b] +c[,d] @@ 見出し` を読む。
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let parse_range = |range: &str| -> Option<(u32, u32)> {
        match range.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((range.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Some((
        old_start,
        old_count,
        new_start,
        new_count,
        section.trim().to_string(),
    ))
}

/// `--- a/path` / `+++ b/path` の path 部分。`/dev/null` は `None`。空白を含むパスに git が
/// 付ける末尾のタブと、引用符（C 風エスケープ）を外す。
fn side_path(raw: &str, prefix: &str) -> Option<String> {
    let raw = raw.strip_suffix('\t').unwrap_or(raw);
    if raw == "/dev/null" {
        return None;
    }
    let path = unquote_path(raw);
    Some(
        path.strip_prefix(prefix)
            .map(str::to_string)
            .unwrap_or(path),
    )
}

/// `diff --git a/X b/Y` の (X, Y)。引用符付きは字句で、無引用は「同じパス 2 つ」と見て真ん中で割る
/// （空白を含むパスでも改名でなければ一意に割れる。改名は `rename from/to` が上書きする）。
fn parse_git_header_paths(rest: &str) -> (String, String) {
    let rest = rest.trim_end_matches('\r');
    if rest.starts_with('"') {
        let (first, remainder) = take_quoted(rest);
        let second = remainder.trim_start();
        let second = if second.starts_with('"') {
            take_quoted(second).0
        } else {
            second.to_string()
        };
        return (strip_side(&first, "a/"), strip_side(&second, "b/"));
    }
    // 同じパス 2 つ（`a/P b/P`）なら長さから真ん中が決まる。
    if rest.len() >= 5 && (rest.len() - 5) % 2 == 0 {
        let half = (rest.len() - 5) / 2;
        if rest.is_char_boundary(2 + half) && rest.is_char_boundary(rest.len() - half) {
            let first = &rest[2..2 + half];
            let second = &rest[rest.len() - half..];
            if rest.starts_with("a/") && first == second && rest[2 + half..].starts_with(" b/") {
                return (first.to_string(), second.to_string());
            }
        }
    }
    match rest.split_once(" b/") {
        Some((first, second)) => (strip_side(first, "a/"), second.to_string()),
        None => (strip_side(rest, "a/"), strip_side(rest, "a/")),
    }
}

fn strip_side(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

/// 先頭の `"…"` を C 風エスケープを解いて取り出し、残りを返す。
fn take_quoted(text: &str) -> (String, &str) {
    let bytes = text.as_bytes();
    let mut index = 1;
    let mut out: Vec<u8> = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                return (
                    String::from_utf8_lossy(&out).into_owned(),
                    &text[index + 1..],
                );
            }
            b'\\' if index + 1 < bytes.len() => {
                let next = bytes[index + 1];
                match next {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'v' => out.push(0x0b),
                    // `\346` のような 8 進 3 桁（git は非 ASCII のバイトをこう書く）。
                    b'0'..=b'7' => {
                        let digits: Vec<u8> = bytes[index + 1..]
                            .iter()
                            .take(3)
                            .take_while(|digit| (b'0'..=b'7').contains(digit))
                            .copied()
                            .collect();
                        let value = digits
                            .iter()
                            .fold(0u32, |value, digit| value * 8 + u32::from(digit - b'0'));
                        out.push(value as u8);
                        index += 1 + digits.len();
                        continue;
                    }
                    other => out.push(other),
                }
                index += 2;
                continue;
            }
            other => out.push(other),
        }
        index += 1;
    }
    (String::from_utf8_lossy(&out).into_owned(), "")
}

/// 引用符付きなら外す（無引用はそのまま）。
fn unquote_path(raw: &str) -> String {
    let raw = raw.trim_end_matches(['\t', '\r']);
    if raw.starts_with('"') {
        take_quoted(raw).0
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// 行の配列から patch を組む（`\` 継続行だと文脈行の先頭空白が消えるため）。
    fn patch_text(lines: &[&str]) -> String {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }

    fn line(kind: DiffLineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_line: old,
            new_line: new,
            text: text.to_string(),
            no_newline: false,
        }
    }

    #[test]
    fn parses_modified_file_with_line_numbers_and_section() {
        let patch = patch_text(&[
            "diff --git a/src/lib.rs b/src/lib.rs",
            "index 1111111..2222222 100644",
            "--- a/src/lib.rs",
            "+++ b/src/lib.rs",
            "@@ -10,4 +10,5 @@ fn main() {",
            " keep",
            "-old",
            "+new",
            "+added",
            " tail",
            " end",
        ]);
        let files = parse_unified_diff(&patch);
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "src/lib.rs");
        assert_eq!(file.old_path, None);
        assert_eq!(file.kind, FileChangeKind::Modified);
        assert_eq!((file.additions, file.deletions), (2, 1));
        let hunk = &file.hunks[0];
        assert_eq!(hunk.section, "fn main() {");
        assert_eq!(hunk.old_span(), 10..14);
        assert_eq!(hunk.new_span(), 10..15);
        assert_eq!(
            hunk.lines,
            vec![
                line(DiffLineKind::Context, Some(10), Some(10), "keep"),
                line(DiffLineKind::Removed, Some(11), None, "old"),
                line(DiffLineKind::Added, None, Some(11), "new"),
                line(DiffLineKind::Added, None, Some(12), "added"),
                line(DiffLineKind::Context, Some(12), Some(13), "tail"),
                line(DiffLineKind::Context, Some(13), Some(14), "end"),
            ]
        );
    }

    #[test]
    fn parses_rename_binary_new_deleted_mode_and_no_newline() {
        let patch = patch_text(&[
            "diff --git a/bin.dat b/bin.dat",
            "index 8352675..c5793f9 100644",
            "Binary files a/bin.dat and b/bin.dat differ",
            "diff --git a/empty.txt b/empty.txt",
            "new file mode 100644",
            "index 0000000..e69de29",
            "diff --git a/gone.txt b/gone.txt",
            "deleted file mode 100644",
            "index 286c5f5..0000000",
            "--- a/gone.txt",
            "+++ /dev/null",
            "@@ -1 +0,0 @@",
            "-gone",
            "diff --git a/keep.txt b/keep.txt",
            "index 4cb29ea..a9beb14 100644",
            "--- a/keep.txt",
            "+++ b/keep.txt",
            "@@ -1,3 +1,3 @@",
            " one",
            "-two",
            "-three",
            "+TWO",
            "+three",
            "\\ No newline at end of file",
            "diff --git a/mode.sh b/mode.sh",
            "old mode 100644",
            "new mode 100755",
            "diff --git a/moved.txt b/renamed.txt",
            "similarity index 71%",
            "rename from moved.txt",
            "rename to renamed.txt",
            "index 600d48a..2573efa 100644",
            "--- a/moved.txt",
            "+++ b/renamed.txt",
            "@@ -2,4 +2,4 @@ alpha",
            " beta",
            " gamma",
            " delta",
            "-epsilon",
            "+epsilon2",
        ]);
        let files = parse_unified_diff(&patch);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "bin.dat",
                "empty.txt",
                "gone.txt",
                "keep.txt",
                "mode.sh",
                "renamed.txt"
            ]
        );
        assert!(files[0].binary && files[0].hunks.is_empty());
        assert_eq!(files[1].kind, FileChangeKind::Added);
        assert!(files[1].hunks.is_empty());
        assert_eq!(files[2].kind, FileChangeKind::Deleted);
        assert_eq!(files[2].deletions, 1);
        assert_eq!(
            files[2].hunks[0].new_span(),
            1..1,
            "削除だけの hunk は新側が空"
        );
        let keep = &files[3];
        let last = keep.hunks[0].lines.last().expect("最後の行");
        assert_eq!(last.text, "three");
        assert!(last.no_newline, "`\\ No newline` は直前の行に付く");
        assert_eq!(
            keep.hunks[0]
                .lines
                .iter()
                .filter(|line| line.no_newline)
                .count(),
            1
        );
        assert!(files[4].mode_changed && files[4].hunks.is_empty());
        let renamed = &files[5];
        assert_eq!(renamed.kind, FileChangeKind::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some("moved.txt"));
        assert_eq!(renamed.base_path(), "moved.txt");
        assert_eq!(renamed.hunks[0].lines[3].old_line, Some(5));
        assert_eq!(renamed.hunks[0].lines[4].new_line, Some(5));
    }

    #[test]
    fn parses_quoted_and_spaced_paths_and_crlf_lines() {
        let patch = patch_text(&[
            "diff --git \"a/q\\\"uote.txt\" \"b/q\\\"uote.txt\"",
            "new file mode 100644",
            "--- /dev/null",
            "+++ \"b/q\\\"uote.txt\"",
            "@@ -0,0 +1 @@",
            "+c\r",
            "diff --git a/sp ace.txt b/sp ace.txt",
            "new file mode 100644",
            "index 0000000..7898192",
            "--- /dev/null",
            "+++ b/sp ace.txt\t",
            "@@ -0,0 +1 @@",
            "+a",
            "diff --git \"a/\\346\\227\\245.txt\" \"b/\\346\\227\\245.txt\"",
            "new file mode 100644",
        ]);
        let files = parse_unified_diff(&patch);
        assert_eq!(files[0].path, "q\"uote.txt");
        assert_eq!(files[0].hunks[0].lines[0].text, "c", "CR は本文に残さない");
        assert_eq!(files[0].hunks[0].old_span(), 1..1);
        assert_eq!(
            files[1].path, "sp ace.txt",
            "空白を含むパスの末尾タブを外す"
        );
        assert_eq!(files[2].path, "日.txt", "8 進エスケープを UTF-8 として戻す");
    }

    #[test]
    fn files_follow_the_tree_order() {
        let mut paths = vec![
            "notes.md",
            "src/main.rs",
            "src/a/b.rs",
            "README.md",
            "src/z.rs",
        ];
        paths.sort_by(|left, right| tree_order(left, right));
        assert_eq!(
            paths,
            vec![
                "src/a/b.rs",
                "src/main.rs",
                "src/z.rs",
                "README.md",
                "notes.md"
            ],
            "ディレクトリが先・同じ階層は名前順"
        );
    }

    /// R02: 1 行が数 MB あるファイルの 1 行の変更は、パッチの上限で中身を持たない。中身が
    /// 大きすぎるファイルはパッチを取る前に除く。追跡外も全体の予算の中で読む。
    #[test]
    fn review_budgets_bound_what_the_diff_holds() {
        let root = scratch("review_budgets");
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
        };
        if !git(&["init", "-q", "-b", "main"]).is_ok_and(|output| output.status.success()) {
            return; // git が無い環境
        }
        let long_line = |fill: char, bytes: usize| format!("{}\n", fill.to_string().repeat(bytes));
        std::fs::write(root.join("bundle.js"), long_line('x', 64 * 1024)).unwrap();
        std::fs::write(root.join("huge.js"), long_line('x', 300 * 1024)).unwrap();
        std::fs::write(root.join("small.txt"), "a\n").unwrap();
        git(&["add", "-A"]).unwrap();
        git(&["commit", "-qm", "base"]).unwrap();
        std::fs::write(root.join("bundle.js"), long_line('y', 64 * 1024)).unwrap();
        std::fs::write(root.join("huge.js"), long_line('y', 300 * 1024)).unwrap();
        std::fs::write(root.join("small.txt"), "b\n").unwrap();
        for index in 0..3 {
            std::fs::write(root.join(format!("new{index}.txt")), "1\n2\n3\n").unwrap();
        }
        let budgets = ReviewBudgets {
            total_lines: 1_000,
            // 64KB の 1 行の変更（旧 + 新で 128KB）は上限を超える。
            file_patch_bytes: 100 * 1024,
            total_patch_bytes: 1024,
            // 300KB のファイルはパッチを取る前に除く。
            diff_file_bytes: 200 * 1024,
        };
        let diff = review_diff_with_budgets_on(
            &host::LocalHost,
            &root,
            &ReviewBase::Head,
            false,
            &budgets,
        )
        .expect("差分を読める");
        let file = |path: &str| {
            diff.files
                .iter()
                .find(|file| file.path == path)
                .unwrap_or_else(|| panic!("{path} が並ぶ"))
        };
        assert!(file("bundle.js").too_large, "1 ファイルのバイト上限");
        assert!(file("bundle.js").hunks.is_empty(), "中身は持たない");
        assert!(file("huge.js").too_large, "パッチを取る前に除く");
        assert!(file("huge.js").hunks.is_empty());
        assert!(!file("small.txt").too_large, "小さい変更はそのまま");
        assert_eq!(patch_bytes(file("small.txt")), 2);
        let held: usize = diff.files.iter().map(patch_bytes).sum();
        assert!(held <= budgets.total_patch_bytes, "全体の上限: {held}");
        let untracked: Vec<&FileDiff> = diff
            .files
            .iter()
            .filter(|file| file.kind == FileChangeKind::Untracked)
            .collect();
        assert_eq!(untracked.len(), 3, "追跡外は全部並ぶ");
        assert!(
            untracked.iter().all(|file| !file.too_large),
            "予算内なら中身を読む"
        );

        // 全体の予算が尽きたら、追跡外は並べるだけ。
        let tight = ReviewBudgets {
            total_patch_bytes: 4,
            ..budgets
        };
        let diff =
            review_diff_with_budgets_on(&host::LocalHost, &root, &ReviewBase::Head, false, &tight)
                .expect("差分を読める");
        let read = diff
            .files
            .iter()
            .filter(|file| file.kind == FileChangeKind::Untracked && !file.too_large)
            .count();
        assert_eq!(read, 0, "予算を超えた追跡外は中身を読まない");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untracked_files_become_all_added() {
        let file = untracked_file_diff("notes.md", b"a\nb\nc");
        assert_eq!(file.kind, FileChangeKind::Untracked);
        assert_eq!(file.additions, 3);
        let hunk = &file.hunks[0];
        assert_eq!(hunk.new_span(), 1..4);
        assert_eq!(hunk.lines[2].new_line, Some(3));
        assert!(hunk.lines[2].no_newline, "改行で終わらない最後の行");
        assert!(!hunk.lines[1].no_newline);
        assert!(untracked_file_diff("x.bin", b"\x00\x01").binary);
        assert!(untracked_file_diff("empty", b"").hunks.is_empty());
        let big = vec![b'a'; MAX_TEXT_BYTES + 1];
        assert!(untracked_file_diff("big.txt", &big).too_large);
    }

    #[test]
    fn listings_parse_renames_and_binary_counts() {
        let statuses =
            parse_name_status_z("M\0bin.dat\0A\0new.txt\0R071\0moved.txt\0renamed.txt\0");
        assert_eq!(statuses.len(), 3);
        assert_eq!(statuses[2].kind, FileChangeKind::Renamed);
        assert_eq!(statuses[2].old_path.as_deref(), Some("moved.txt"));
        assert_eq!(statuses[2].path, "renamed.txt");
        let counts =
            parse_numstat_z("-\t-\tbin.dat\x002\t1\tnew.txt\x001\t1\t\0moved.txt\0renamed.txt\0");
        assert_eq!(counts.get("bin.dat"), Some(&None));
        assert_eq!(counts.get("new.txt"), Some(&Some((2, 1))));
        assert_eq!(counts.get("renamed.txt"), Some(&Some((1, 1))));
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("necoder_review_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリ");
        dir
    }

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .output()
            .expect("git 実行")
    }

    /// 一時 repo で「base コミット → 作業ツリー」の全経路（追跡済みの変更・改名・削除・追跡外・
    /// バイナリ・-w・ブランチの分岐点・git show）を確かめる。
    #[test]
    fn review_diff_on_temp_repo_covers_base_kinds() {
        let dir = scratch("temp_repo");
        if !git(&dir, &["init", "-q", "-b", "main"]).status.success() {
            return; // git の無い環境
        }
        std::fs::write(dir.join("keep.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(
            dir.join("moved.txt"),
            "alpha\nbeta\ngamma\ndelta\nepsilon\n",
        )
        .unwrap();
        std::fs::write(dir.join("gone.txt"), "gone\n").unwrap();
        std::fs::write(dir.join("space.txt"), "a  b\n").unwrap();
        std::fs::write(dir.join("bin.dat"), b"\x00\x01").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "base"]);
        let base = String::from_utf8(git(&dir, &["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        // Task の作業: コミット 1 つ + 未コミットの変更。
        git(&dir, &["switch", "-q", "-c", "task/x"]);
        std::fs::write(dir.join("keep.txt"), "one\nTWO\nthree\n").unwrap();
        git(&dir, &["commit", "-qam", "work"]);
        git(&dir, &["mv", "moved.txt", "renamed.txt"]);
        std::fs::write(
            dir.join("renamed.txt"),
            "alpha\nbeta\ngamma\ndelta\nepsilon2\n",
        )
        .unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("space.txt"), "a b\n").unwrap();
        std::fs::write(dir.join("bin.dat"), b"\x00\x02").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\nfile").unwrap();
        let host = host::LocalHost;

        let diff = review_diff_on(&host, &dir, &ReviewBase::Commit(base.clone()), false)
            .expect("base との差分");
        assert_eq!(diff.base_oid, base);
        let summary: Vec<(&str, FileChangeKind)> = diff
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.kind))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("bin.dat", FileChangeKind::Modified),
                ("gone.txt", FileChangeKind::Deleted),
                ("keep.txt", FileChangeKind::Modified),
                ("new.txt", FileChangeKind::Untracked),
                ("renamed.txt", FileChangeKind::Renamed),
                ("space.txt", FileChangeKind::Modified),
            ]
        );
        let file = |path: &str| diff.files.iter().find(|file| file.path == path).unwrap();
        assert!(file("bin.dat").binary);
        assert_eq!(
            (file("keep.txt").additions, file("keep.txt").deletions),
            (1, 1),
            "コミット済みの変更も base から数える"
        );
        assert_eq!(file("renamed.txt").old_path.as_deref(), Some("moved.txt"));
        let new = file("new.txt");
        assert_eq!(new.additions, 2);
        assert!(new.hunks[0].lines[1].no_newline);

        // HEAD 基準 = 未コミットの分だけ（keep.txt のコミット済みの変更は出ない）。
        let head = review_diff_on(&host, &dir, &ReviewBase::Head, false).expect("HEAD との差分");
        assert!(head.files.iter().all(|file| file.path != "keep.txt"));
        // -w で空白だけの差を消す。
        let ignoring = review_diff_on(&host, &dir, &ReviewBase::Head, true).expect("-w");
        assert!(ignoring.files.iter().all(|file| file.path != "space.txt"));
        // ブランチ = 分岐点（main の先へ進んでも task の変更だけを見る）。
        git(&dir, &["stash", "-q", "-u"]);
        git(&dir, &["switch", "-q", "main"]);
        std::fs::write(dir.join("main_only.txt"), "main\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "main moves on"]);
        git(&dir, &["switch", "-q", "task/x"]);
        let branch = review_diff_on(&host, &dir, &ReviewBase::Branch("main".into()), false)
            .expect("ブランチとの差分");
        assert_eq!(branch.base_oid, base, "分岐点に解決する");
        assert!(branch.files.iter().all(|file| file.path != "main_only.txt"));
        assert!(branch.files.iter().any(|file| file.path == "keep.txt"));
        // git show と全文。
        let texts = review_file_texts_on(&host, &dir, &base, file("keep.txt"));
        assert_eq!(texts.old.as_deref(), Some("one\ntwo\nthree\n"));
        assert_eq!(texts.new.as_deref(), Some("one\nTWO\nthree\n"));
        assert_eq!(
            git_show_file_on(&host, &dir, &base, "gone.txt").as_deref(),
            Some(b"gone\n".as_slice())
        );
        assert!(matches!(
            review_diff_on(&host, &dir, &ReviewBase::Branch("nope".into()), false),
            Err(ReviewError::UnknownBase(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// コミットの無い repo でも HEAD 基準で開ける（空のツリーと比べる）。
    #[test]
    fn head_review_works_before_the_first_commit() {
        let dir = scratch("unborn");
        if !git(&dir, &["init", "-q", "-b", "main"]).status.success() {
            return;
        }
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let diff = review_diff_on(&host::LocalHost, &dir, &ReviewBase::Head, false)
            .expect("コミット前でも開ける");
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].kind, FileChangeKind::Untracked);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
