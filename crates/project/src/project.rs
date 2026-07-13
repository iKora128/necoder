//! project — ファイルシステムの走査（worktree の縮小版）。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §2: zed `fs`/`worktree` 相当。M3 は**遅延 read_dir**（展開時に直下を読む）で、
//! `.git` と gitignore 対象を除外する（ripgrep の `ignore` crate を使用）。
//! ファイル監視・インクリメンタル更新は後続（M8/性能）で追加する。

use anyhow::{Context as _, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// ディレクトリ 1 項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// 1 プロジェクトのファイルツリー（ルート + gitignore マッチャ）。
pub struct Worktree {
    root: PathBuf,
    ignore: Gitignore,
}

impl Worktree {
    /// ルートを開く。ルート直下の `.gitignore` を読み込む。
    pub fn new(root: impl AsRef<Path>) -> Result<Worktree> {
        let root = root.as_ref();
        let root = std::fs::canonicalize(root)
            .with_context(|| format!("パスを解決できない: {}", root.display()))?;
        anyhow::ensure!(root.is_dir(), "ディレクトリではない: {}", root.display());

        let mut builder = GitignoreBuilder::new(&root);
        // .gitignore が無い/壊れは正常（gitignore 無しの repo もある）ので続行
        let _ignore_add_error = builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap_or_else(|_| Gitignore::empty());

        Ok(Worktree { root, ignore })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// ルートディレクトリ名（レール・プロジェクト名に使う）。
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    /// ルート直下を列挙する。
    pub fn read_root(&self) -> Result<Vec<Entry>> {
        let root = self.root.clone();
        self.read_dir(&root)
    }

    /// ルート配下の全ファイルを再帰列挙する（gitignore 準拠・`.git` 除外）。ファイルファインダ用。
    /// 各要素は (絶対パス, ルート相対の表示文字列)。上限 `limit` 件で打ち切る。
    pub fn all_files(&self, limit: usize) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .require_git(false) // .git が無くても .gitignore を尊重する
            .filter_entry(|entry| entry.file_name() != ".git")
            .build();
        for result in walker {
            if files.len() >= limit {
                break;
            }
            let Ok(entry) = result else { continue };
            if entry.file_type().map(|kind| kind.is_file()) != Some(true) {
                continue;
            }
            let path = entry.path().to_path_buf();
            let relative = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            files.push((path, relative));
        }
        files.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        files
    }

    /// 任意ディレクトリを列挙する（ルート外＝隣のリポジトリへ辿るブラウズ用）。
    /// ルート配下なら [`Self::read_dir`]（gitignore 準拠）に委譲。ルート外は gitignore を適用せず
    /// （ルートのマッチャは配下専用）、`.git` と隠しファイルを除いてディレクトリ優先→名前順で返す。
    pub fn read_any_dir(&self, dir: &Path) -> Result<Vec<Entry>> {
        if dir.starts_with(&self.root) {
            return self.read_dir(dir);
        }
        let mut entries = Vec::new();
        let read = std::fs::read_dir(dir).with_context(|| format!("読めない: {}", dir.display()))?;
        for dir_entry in read {
            let dir_entry = dir_entry?;
            let name = dir_entry.file_name().to_string_lossy().to_string();
            if name == ".git" || name.starts_with('.') {
                continue;
            }
            let path = dir_entry.path();
            let is_dir = dir_entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            entries.push(Entry { path, name, is_dir });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// `dir` 直下を列挙する（`.git`・gitignore 対象を除外、ディレクトリ優先→名前順）。
    pub fn read_dir(&self, dir: &Path) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let read = std::fs::read_dir(dir).with_context(|| format!("読めない: {}", dir.display()))?;
        for dir_entry in read {
            let dir_entry = dir_entry?;
            let name = dir_entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            let path = dir_entry.path();
            let is_dir = dir_entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if self.ignore.matched(&path, is_dir).is_ignore() {
                continue;
            }
            entries.push(Entry { path, name, is_dir });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }
}

// ── git status / gutter diff（M8） ──
// Zed 準拠: git2/gix を使わず `git` CLI + imara-diff（純 Rust）。
// 詳細な移植根拠は docs/research/porting-git-terminal-lsp.md。

/// ファイルの git 状態（ツリー/タブの色分け用）。色は識別に集約（UI-SPEC §1.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Added,
    Modified,
    Deleted,
    Untracked,
    Conflicted,
}

/// `dir` を含む git リポジトリのルート（`git rev-parse --show-toplevel`）。repo 外なら `None`。
fn git_repo_root(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// `dir` を含む repo の working-tree 状態を読む。返すパスは**絶対**。
/// git が無い / repo でない / 失敗時は空（色を出さないだけ＝安全側）。
pub fn git_status(dir: &Path) -> Vec<(PathBuf, StatusKind)> {
    let Some(repo) = git_repo_root(dir) else {
        return Vec::new();
    };
    let output = Command::new("git")
        .current_dir(&repo)
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    for entry in stdout.split('\0') {
        // レイアウト: XY + ' ' + path（`-z` は path をクォートしない）。
        if entry.len() < 4 || &entry[2..3] != " " {
            continue;
        }
        let path = &entry[3..];
        if path.ends_with('/') {
            continue; // untracked ディレクトリ（配下ファイルで拾う）
        }
        let bytes = entry.as_bytes();
        entries.push((repo.join(path), classify_status(bytes[0], bytes[1])));
    }
    entries
}

/// porcelain の XY（X=index, Y=worktree）を 5 分類に畳む。順序が重要
/// （Untracked / Conflicted を先に判定してから A/D/M）。<https://git-scm.com/docs/git-status>
fn classify_status(x: u8, y: u8) -> StatusKind {
    match (x, y) {
        (b'?', b'?') => StatusKind::Untracked,
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => StatusKind::Conflicted,
        _ if x == b'A' || y == b'A' => StatusKind::Added,
        _ if x == b'D' || y == b'D' => StatusKind::Deleted,
        _ => StatusKind::Modified,
    }
}

// ── branch / worktree（M8: ブランチ横断の完成形） ──

/// 現在のブランチ名（`git rev-parse --abbrev-ref HEAD`）。detached HEAD / repo 外は `None`。
pub fn git_current_branch(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// ローカルブランチ名の一覧（現在ブランチを先頭に）。repo 外は空。
pub fn git_branches(dir: &Path) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["branch", "--format=%(refname:short)"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(current) = git_current_branch(dir) {
        // 現在ブランチを先頭へ（キーが false=0 で先頭）。
        branches.sort_by_key(|branch| *branch != current);
    }
    branches
}

/// worktree の 1 項目（作業ツリーのパス + チェックアウト中のブランチ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// worktree の一覧（`git worktree list --porcelain`）。repo 外は空。
pub fn git_worktrees(dir: &Path) -> Vec<GitWorktree> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["worktree", "list", "--porcelain"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut list = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let flush = |path: &mut Option<PathBuf>, branch: &mut Option<String>, list: &mut Vec<GitWorktree>| {
        if let Some(taken) = path.take() {
            list.push(GitWorktree { path: taken, branch: branch.take() });
        }
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            flush(&mut path, &mut branch, &mut list);
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        }
    }
    flush(&mut path, &mut branch, &mut list);
    list
}

/// ブランチを in-place で切り替える（`git switch`）。作業ツリーが dirty だと失敗し得る。
pub fn switch_branch(dir: &Path, branch: &str) -> Result<()> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["switch", branch])
        .output()
        .context("git switch の実行に失敗")?;
    anyhow::ensure!(
        output.status.success(),
        "ブランチ切替に失敗: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// 既存ブランチの worktree を作る（`git worktree add <path> <branch>`）。
pub fn add_worktree(dir: &Path, path: &Path, branch: &str) -> Result<()> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["worktree", "add"])
        .arg(path)
        .arg(branch)
        .output()
        .context("git worktree add の実行に失敗")?;
    anyhow::ensure!(
        output.status.success(),
        "worktree 作成に失敗: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// gutter diff の 1 ハンク種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Modified,
    Removed,
}

/// gutter diff の 1 ハンク。`new_range` は**現在バッファ側の行範囲**（0 始まり半開）＝ガター描画のキー。
/// `Removed` は `new_range` が空（`n..n`）＝行 n の境界に削除マーカーを出す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_range: std::ops::Range<u32>,
    pub new_range: std::ops::Range<u32>,
    pub kind: HunkKind,
}

/// `file`（絶対パス）の HEAD 版テキスト。`HEAD:./<name>`（cwd 相対）で subdir でも正しく引く。
/// HEAD に無い（新規/未追跡）or repo 外なら `None`。
fn head_blob(dir: &Path, name: &OsStr) -> Option<String> {
    let spec = format!("HEAD:./{}", name.to_string_lossy());
    let output = Command::new("git")
        .current_dir(dir)
        .args(["--no-optional-locks", "show", &spec])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `file`（絶対パス）の HEAD 版 vs 現在テキストを行単位で diff。HEAD が無ければ空
/// （新規/未追跡は tree/tab の色で示すのでガターは静かにする）。
pub fn buffer_diff(file: &Path, current: &str) -> Vec<DiffHunk> {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
        return Vec::new();
    };
    match head_blob(dir, name) {
        Some(head) => diff_hunks(&head, current),
        None => Vec::new(),
    }
}

/// HEAD テキスト vs 現在テキストの行 diff（imara-diff・Histogram）。テスト可能な純関数。
pub fn diff_hunks(head_text: &str, current: &str) -> Vec<DiffHunk> {
    use imara_diff::intern::InternedInput;
    use imara_diff::sources::lines_with_terminator;
    use imara_diff::Algorithm;
    // CRLF を LF へ正規化（さもないと改行差だけで全行 Modified になる）。
    let head = normalize_newlines(head_text);
    let current = normalize_newlines(current);
    let input = InternedInput::new(
        lines_with_terminator(head.as_str()),
        lines_with_terminator(current.as_str()),
    );
    imara_diff::diff(Algorithm::Histogram, &input, HunkCollector::default())
}

fn normalize_newlines(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    }
}

#[derive(Default)]
struct HunkCollector {
    hunks: Vec<DiffHunk>,
}

impl imara_diff::Sink for HunkCollector {
    type Out = Vec<DiffHunk>;
    fn process_change(&mut self, before: std::ops::Range<u32>, after: std::ops::Range<u32>) {
        let kind = if after.is_empty() {
            HunkKind::Removed
        } else if before.is_empty() {
            HunkKind::Added
        } else {
            HunkKind::Modified
        };
        self.hunks.push(DiffHunk { old_range: before, new_range: after, kind });
    }
    fn finish(self) -> Self::Out {
        self.hunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shirushi_project_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn scans_sorted_and_filters_git_and_gitignore() {
        let root = scratch("scan");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("main.rs"), "").unwrap();
        std::fs::write(root.join("build.log"), "").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();

        let worktree = Worktree::new(&root).unwrap();
        let names: Vec<String> = worktree.read_root().unwrap().into_iter().map(|e| e.name).collect();

        assert!(names.contains(&"src".to_string()));
        assert!(names.contains(&"main.rs".to_string()));
        assert!(!names.contains(&"target".to_string()), "gitignore の dir を除外");
        assert!(!names.contains(&"build.log".to_string()), "gitignore の glob を除外");
        assert!(!names.contains(&".git".to_string()), ".git を除外");
        // ディレクトリが先頭
        assert_eq!(names.first().map(String::as_str), Some("src"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn name_is_root_basename() {
        let root = scratch("name");
        std::fs::create_dir_all(&root).unwrap();
        let worktree = Worktree::new(&root).unwrap();
        assert!(worktree.name().starts_with("shirushi_project_name_"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn all_files_walks_recursively_respecting_gitignore() {
        let root = scratch("allfiles");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        std::fs::write(root.join("target/out.o"), "").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();

        let worktree = Worktree::new(&root).unwrap();
        let relatives: Vec<String> = worktree.all_files(1000).into_iter().map(|(_, r)| r).collect();
        assert!(relatives.contains(&"src/main.rs".to_string()));
        assert!(relatives.contains(&"README.md".to_string()));
        assert!(!relatives.iter().any(|r| r.contains("target")), "gitignore の target を除外");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_directory_errors() {
        assert!(Worktree::new("/no/such/dir/shirushi-xyz").is_err());
    }

    #[test]
    fn diff_hunks_classifies_add_modify_remove() {
        // 同一 → ハンク無し
        assert!(diff_hunks("a\nb\n", "a\nb\n").is_empty());
        // 変更（b→B）→ Modified・現在行 0..1
        let modified = diff_hunks("a\n", "b\n");
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].kind, HunkKind::Modified);
        assert_eq!(modified[0].new_range, 0..1);
        // 追加（b を挿入）→ Added・現在行 1..2
        let added = diff_hunks("a\n", "a\nb\n");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].kind, HunkKind::Added);
        assert_eq!(added[0].new_range, 1..2);
        // 削除（b を除去）→ Removed・現在行 1..1（境界）
        let removed = diff_hunks("a\nb\n", "a\n");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].kind, HunkKind::Removed);
        assert_eq!(removed[0].new_range, 1..1);
    }

    #[test]
    fn git_status_and_buffer_diff_on_temp_repo() {
        let root = scratch("gitstatus");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            Command::new("git").current_dir(&root).args(args).output().expect("git 実行")
        };
        // git が無い環境ではスキップ（CI 等）。
        if !git(&["init", "-q"]).status.success() {
            return;
        }
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "tester"]);
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        // 追跡ファイルを変更 + 未追跡ファイルを追加
        std::fs::write(root.join("tracked.txt"), "two\n").unwrap();
        std::fs::write(root.join("new.txt"), "x\n").unwrap();

        let status: std::collections::HashMap<PathBuf, StatusKind> =
            git_status(&root).into_iter().collect();
        // git は toplevel を realpath で返すので比較側も canonicalize（macOS の /var→/private/var）。
        let tracked = std::fs::canonicalize(root.join("tracked.txt")).unwrap();
        let new = std::fs::canonicalize(root.join("new.txt")).unwrap();
        assert_eq!(status.get(&tracked), Some(&StatusKind::Modified));
        assert_eq!(status.get(&new), Some(&StatusKind::Untracked));

        // buffer_diff: HEAD="one\n" vs 現在 "two\n" → 1 行 Modified
        let hunks = buffer_diff(&tracked, "two\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Modified);

        let _ = std::fs::remove_dir_all(&root);
    }
}
