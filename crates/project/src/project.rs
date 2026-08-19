//! project — ファイルシステムの走査（worktree の縮小版）。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §2: zed `fs`/`worktree` 相当。M3 は**遅延 read_dir**（展開時に直下を読む）で、
//! `.git` と gitignore 対象を除外する（ripgrep の `ignore` crate を使用）。
//! ファイル監視・インクリメンタル更新は後続（M8/性能）で追加する。

pub mod todos;

use anyhow::{Context as _, Result};
use host::{CommandOutput, CommandSpec, Host, LocalHost};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A project execution source. Host and root must stay paired so identical remote/local
/// paths cannot be confused by the workspace shell.
#[derive(Clone)]
pub struct ProjectSource {
    host: Arc<dyn Host>,
    root: PathBuf,
}

impl ProjectSource {
    pub fn local(root: PathBuf) -> Self {
        Self {
            host: LocalHost::shared(),
            root,
        }
    }

    pub fn new(host: Arc<dyn Host>, root: PathBuf) -> Self {
        Self { host, root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_remote()
    }

    pub fn into_parts(self) -> (Arc<dyn Host>, PathBuf) {
        (self.host, self.root)
    }
}

/// ディレクトリ 1 項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// gitignore 対象か（ツリーで薄字表示にする。`.git` は列挙しないので常に false）。
    pub ignored: bool,
}

/// 1 プロジェクトのファイルツリー（ルート + gitignore マッチャ）。
pub struct Worktree {
    host: Arc<dyn Host>,
    root: PathBuf,
    ignore: Gitignore,
}

impl Worktree {
    /// ルートを開く。ルート直下の `.gitignore` を読み込む。
    pub fn new(root: impl AsRef<Path>) -> Result<Worktree> {
        Self::with_host(LocalHost::shared(), root)
    }

    /// local/remote 共通の host 上でルートを開く。
    pub fn with_host(host: Arc<dyn Host>, root: impl AsRef<Path>) -> Result<Worktree> {
        let root = host.canonicalize(root.as_ref())?;
        anyhow::ensure!(
            host.metadata(&root)?.is_dir,
            "ディレクトリではない: {}",
            root.display()
        );

        let mut builder = GitignoreBuilder::new(&root);
        let ignore_path = root.join(".gitignore");
        // remote path を local fs へ渡さず、Host から読んだ各行を matcher に積む。
        if let Ok(contents) = host.read_file(&ignore_path) {
            if let Ok(contents) = String::from_utf8(contents.bytes) {
                for line in contents.lines() {
                    let _invalid_pattern = builder.add_line(Some(ignore_path.clone()), line);
                }
            }
        }
        let ignore = builder.build().unwrap_or_else(|_| Gitignore::empty());

        Ok(Worktree { host, root, ignore })
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
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
        all_files_on(self.host.as_ref(), &self.root, limit)
    }

    /// 任意ディレクトリを列挙する（ルート外＝隣のリポジトリへ辿るブラウズ用）。
    /// ルート配下なら [`Self::read_dir`]（gitignore 準拠）に委譲。ルート外は gitignore を適用せず
    /// （ルートのマッチャは配下専用）、`.git` と隠しファイルを除いてディレクトリ優先→名前順で返す。
    pub fn read_any_dir(&self, dir: &Path) -> Result<Vec<Entry>> {
        if dir.starts_with(&self.root) {
            return self.read_dir(dir);
        }
        let mut entries = Vec::new();
        for dir_entry in self.host.read_dir(dir)? {
            let name = dir_entry.name;
            if name == ".git" || name.starts_with('.') {
                continue;
            }
            let path = dir_entry.path;
            let is_dir = dir_entry.is_dir;
            entries.push(Entry {
                path,
                name,
                is_dir,
                ignored: false,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// パスが gitignore 対象か（watch イベントのノイズ除去用。ディレクトリ判定不能なら false 扱いで問い合わせる）。
    pub fn is_ignored(&self, path: &Path) -> bool {
        self.ignore.matched(path, false).is_ignore() || self.ignore.matched(path, true).is_ignore()
    }

    /// `dir` 直下を列挙する（`.git` は除外。gitignore 対象は**除外せず** `ignored=true` で薄字表示。
    /// VSCode 同様「無視ファイルも見えるが淡い」＝ git 管理有無が一目で分かる）。ディレクトリ優先→名前順。
    pub fn read_dir(&self, dir: &Path) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for dir_entry in self.host.read_dir(dir)? {
            let name = dir_entry.name;
            if name == ".git" {
                continue;
            }
            let path = dir_entry.path;
            let is_dir = dir_entry.is_dir;
            let ignored = self.ignore.matched(&path, is_dir).is_ignore();
            entries.push(Entry {
                path,
                name,
                is_dir,
                ignored,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    pub fn read_file(&self, path: &Path) -> Result<host::FileContent> {
        self.host.read_file(path)
    }

    pub fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        condition: host::WriteCondition,
    ) -> Result<host::FileRevision> {
        self.host.write_file(path, bytes, condition)
    }

    pub fn git_status(&self) -> Vec<(PathBuf, StatusKind)> {
        git_status_on(self.host.as_ref(), &self.root)
    }

    pub fn git_current_branch(&self) -> Option<String> {
        git_current_branch_on(self.host.as_ref(), &self.root)
    }

    pub fn git_branches(&self) -> Vec<String> {
        git_branches_on(self.host.as_ref(), &self.root)
    }

    pub fn git_worktrees(&self) -> Vec<GitWorktree> {
        git_worktrees_on(self.host.as_ref(), &self.root)
    }

    pub fn switch_branch(&self, branch: &str) -> Result<()> {
        switch_branch_on(self.host.as_ref(), &self.root, branch)
    }

    pub fn add_worktree(&self, path: &Path, branch: &str) -> Result<()> {
        add_worktree_on(self.host.as_ref(), &self.root, path, branch)
    }

    pub fn stage_path(&self, path: &Path) -> Result<()> {
        stage_path_on(self.host.as_ref(), &self.root, path)
    }

    pub fn stage_all(&self) -> Result<()> {
        stage_all_on(self.host.as_ref(), &self.root)
    }

    pub fn unstage_path(&self, path: &Path) -> Result<()> {
        unstage_path_on(self.host.as_ref(), &self.root, path)
    }

    pub fn commit(&self, message: &str) -> Result<()> {
        commit_on(self.host.as_ref(), &self.root, message)
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        create_branch_on(self.host.as_ref(), &self.root, name)
    }

    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        delete_branch_on(self.host.as_ref(), &self.root, name, force)
    }

    pub fn push(&self) -> Result<()> {
        push_on(self.host.as_ref(), &self.root)
    }

    pub fn pull(&self) -> Result<()> {
        pull_on(self.host.as_ref(), &self.root)
    }

    pub fn git_changes(&self) -> Vec<WorkingChange> {
        git_changes_on(self.host.as_ref(), &self.root)
    }

    pub fn git_log_graph(&self, limit: usize) -> Vec<GraphCommit> {
        git_log_graph_on(self.host.as_ref(), &self.root, limit)
    }

    pub fn buffer_diff(&self, file: &Path, current: &str) -> Vec<DiffHunk> {
        buffer_diff_on(self.host.as_ref(), file, current)
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

fn run_git<I, S>(host: &dyn Host, dir: &Path, args: I) -> Result<CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    // 未信頼 repo の repo-local `.git/config` 経由のコード実行を封じる（Zed GHSA-fj2r 同型）。
    // フォルダを開くだけで `git status` が自動実行される（レールの git 色）ため、`core.fsmonitor`
    // による drive-by RCE が要点。hooks / ext:: リモート transport も常時無効化して防御を集約する。
    // git レベルのオプションは subcommand より前に置く必要があるので先頭へ差す。
    let mut hardened: Vec<String> = vec![
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "protocol.ext.allow=never".into(),
    ];
    hardened.extend(args.into_iter().map(Into::into));
    host.run_command(&CommandSpec::new("git", dir).args(hardened))
}

/// `dir` を含む git リポジトリのルート（`git rev-parse --show-toplevel`）。repo 外なら `None`。
fn git_repo_root_on(host: &dyn Host, dir: &Path) -> Option<PathBuf> {
    let output = run_git(host, dir, ["rev-parse", "--show-toplevel"]).ok()?;
    if !output.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// `dir` を含む repo の working-tree 状態を読む。返すパスは**絶対**。
/// git が無い / repo でない / 失敗時は空（色を出さないだけ＝安全側）。
pub fn git_status(dir: &Path) -> Vec<(PathBuf, StatusKind)> {
    git_status_on(&LocalHost, dir)
}

pub fn git_status_on(host: &dyn Host, dir: &Path) -> Vec<(PathBuf, StatusKind)> {
    let Some(repo) = git_repo_root_on(host, dir) else {
        return Vec::new();
    };
    let output = run_git(
        host,
        &repo,
        [
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    );
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.success() {
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
    git_current_branch_on(&LocalHost, dir)
}

pub fn git_current_branch_on(host: &dyn Host, dir: &Path) -> Option<String> {
    let output = run_git(host, dir, ["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    if !output.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// linked worktree 間で共通な Git directory。Fleet の Repository ID は worktree root
/// ではなくこれを使い、同じ repository から切った TaskSpace を確実に束ねる。
pub fn git_common_dir_on(host: &dyn Host, dir: &Path) -> Option<PathBuf> {
    let output = run_git(
        host,
        dir,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .or_else(|| run_git(host, dir, ["rev-parse", "--git-common-dir"]).ok())?;
    if !output.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    Some(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

/// UI / CLI / MCP が同じ TaskSpace ID を生成するための共有実装。
pub fn stable_worktree_id_on(host: &dyn Host, root: &Path) -> String {
    let identity = format!("{}\0{}", host.id(), root.display());
    let hash = identity.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("space-{hash:016x}")
}

pub fn repository_id_on(host: &dyn Host, root: &Path) -> String {
    let repository_root = git_common_dir_on(host, root).unwrap_or_else(|| root.to_path_buf());
    format!("{}:{}", host.id(), repository_root.display())
}

/// 現在の HEAD commit。Task 作成時の base と review 時の head を別に保持するために使う。
pub fn git_head_oid_on(host: &dyn Host, dir: &Path) -> Option<String> {
    let output = run_git(host, dir, ["rev-parse", "HEAD"]).ok()?;
    if !output.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!oid.is_empty()).then_some(oid)
}

/// ローカルブランチ名の一覧（現在ブランチを先頭に）。repo 外は空。
pub fn git_branches(dir: &Path) -> Vec<String> {
    git_branches_on(&LocalHost, dir)
}

pub fn git_branches_on(host: &dyn Host, dir: &Path) -> Vec<String> {
    let output = run_git(host, dir, ["branch", "--format=%(refname:short)"]);
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.success() {
        return Vec::new();
    }
    let mut branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(current) = git_current_branch_on(host, dir) {
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
    git_worktrees_on(&LocalHost, dir)
}

pub fn git_worktrees_on(host: &dyn Host, dir: &Path) -> Vec<GitWorktree> {
    let output = run_git(host, dir, ["worktree", "list", "--porcelain"]);
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut list = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let flush =
        |path: &mut Option<PathBuf>, branch: &mut Option<String>, list: &mut Vec<GitWorktree>| {
            if let Some(taken) = path.take() {
                list.push(GitWorktree {
                    path: taken,
                    branch: branch.take(),
                });
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
    switch_branch_on(&LocalHost, dir, branch)
}

pub fn switch_branch_on(host: &dyn Host, dir: &Path, branch: &str) -> Result<()> {
    let output = run_git(host, dir, ["switch", branch]).context("git switch の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "ブランチ切替に失敗: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// 既存ブランチの worktree を作る（`git worktree add <path> <branch>`）。
pub fn add_worktree(dir: &Path, path: &Path, branch: &str) -> Result<()> {
    add_worktree_on(&LocalHost, dir, path, branch)
}

pub fn add_worktree_on(host: &dyn Host, dir: &Path, path: &Path, branch: &str) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let output = run_git(host, dir, ["worktree", "add", path.as_str(), branch])
        .context("git worktree add の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "worktree 作成に失敗: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Fleet の `+ Task` 用: 現在の HEAD から新規 branch と linked worktree を一度に作る。
/// 通常の「既存 branch を開く」[`add_worktree_on`] と混ぜず、既定操作が必ず隔離されるようにする。
pub fn create_task_worktree_on(
    host: &dyn Host,
    dir: &Path,
    path: &Path,
    branch: &str,
) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let output = run_git(
        host,
        dir,
        ["worktree", "add", "-b", branch, path.as_str(), "HEAD"],
    )
    .context("Task worktree の作成に失敗")?;
    anyhow::ensure!(
        output.success(),
        "Task worktree の作成に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// IntegrationSpace から見た Task branch の merge 可否。`git merge-tree` なので index / worktree を
/// 一切変更せず、Conflict Radar から安全に呼べる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePreview {
    pub clean: bool,
    pub detail: String,
}

pub fn preview_merge_on(
    host: &dyn Host,
    integration_dir: &Path,
    branch: &str,
) -> Result<MergePreview> {
    let output = run_git(
        host,
        integration_dir,
        ["merge-tree", "--write-tree", "HEAD", branch],
    )
    .context("merge preview の実行に失敗")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok(MergePreview {
        clean: output.success(),
        detail: if stderr.is_empty() { stdout } else { stderr },
    })
}

/// `base` に取り込まれていない HEAD 側のコミット数（`git rev-list --count base..HEAD`）。
/// worktree 削除の確認で「消したら**どこにも残らない**コミットが何件あるか」を示すために使う。
/// base が解決できない（未 push の孤立ブランチ等）場合は 0 を返さず None ＝「数えられない」を区別する。
pub fn git_unmerged_count_on(host: &dyn Host, dir: &Path, base: &str) -> Option<usize> {
    let output = run_git(host, dir, ["rev-list", "--count", &format!("{base}..HEAD")]).ok()?;
    if !output.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// 明示的に merge-ready となった Task を IntegrationSpace へ統合する。
/// dirty integration / preview conflict は拒否し、merge 自体が失敗した場合も自動 abort して戻す。
pub fn integrate_branch_on(
    host: &dyn Host,
    integration_dir: &Path,
    branch: &str,
) -> Result<String> {
    anyhow::ensure!(
        git_status_on(host, integration_dir).is_empty(),
        "IntegrationSpace に未コミット変更があります。統合前に clean にしてください"
    );
    let preview = preview_merge_on(host, integration_dir, branch)?;
    anyhow::ensure!(
        preview.clean,
        "競合のため統合できません: {}",
        preview.detail
    );
    let message = format!("Integrate {branch}");
    let output = run_git(
        host,
        integration_dir,
        ["merge", "--no-ff", branch, "-m", message.as_str()],
    )
    .context("git merge の実行に失敗")?;
    if !output.success() {
        let detail = git_fail_message(&output);
        let _ = run_git(host, integration_dir, ["merge", "--abort"]);
        anyhow::bail!("統合に失敗したため merge を中止しました: {detail}");
    }
    git_head_oid_on(host, integration_dir).context("統合後の HEAD を取得できない")
}

/// worktree を削除（`git worktree remove [--force] <path>`）。dirty だと非 force で git が拒否＝安全側。
/// メインの作業ツリーは git が拒否する（呼び手はレールから外すへ倒す）。
pub fn remove_worktree(dir: &Path, path: &Path, force: bool) -> Result<()> {
    remove_worktree_on(&LocalHost, dir, path, force)
}

pub fn remove_worktree_on(host: &dyn Host, dir: &Path, path: &Path, force: bool) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let args: &[&str] = if force {
        &["worktree", "remove", "--force", path.as_str()]
    } else {
        &["worktree", "remove", path.as_str()]
    };
    let output =
        run_git(host, dir, args.iter().copied()).context("git worktree remove の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "worktree 削除に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

// ── git 基礎操作（stage / commit / push / pull / branch 作成・削除） ──
// すべて `git` CLI ラッパ。失敗は stderr（空なら stdout）を人間向けに返す。

/// 失敗した git コマンドの人間向けメッセージ（stderr 優先・空なら stdout。commit の
/// 「nothing to commit」等は stdout に出るため両対応）。
fn git_fail_message(output: &CommandOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// 変更を index に上げる（`git add -- <path>`）。
pub fn stage_path(dir: &Path, path: &Path) -> Result<()> {
    stage_path_on(&LocalHost, dir, path)
}

pub fn stage_path_on(host: &dyn Host, dir: &Path, path: &Path) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let output =
        run_git(host, dir, ["add", "--", path.as_str()]).context("git add の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "stage に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// 全変更を index に上げる（`git add -A`）。
pub fn stage_all(dir: &Path) -> Result<()> {
    stage_all_on(&LocalHost, dir)
}

pub fn stage_all_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_git(host, dir, ["add", "-A"]).context("git add -A の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "stage に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// index から下ろす（`git restore --staged -- <path>`）。
pub fn unstage_path(dir: &Path, path: &Path) -> Result<()> {
    unstage_path_on(&LocalHost, dir, path)
}

pub fn unstage_path_on(host: &dyn Host, dir: &Path, path: &Path) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let output = run_git(host, dir, ["restore", "--staged", "--", path.as_str()])
        .context("git restore --staged の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "unstage に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// staged 変更をコミット（`git commit -m <message>`）。message 空・staged 無しは失敗。
pub fn commit(dir: &Path, message: &str) -> Result<()> {
    commit_on(&LocalHost, dir, message)
}

pub fn commit_on(host: &dyn Host, dir: &Path, message: &str) -> Result<()> {
    anyhow::ensure!(!message.trim().is_empty(), "コミットメッセージが空");
    let output =
        run_git(host, dir, ["commit", "-m", message]).context("git commit の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "コミットに失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// 新しいブランチを作って切り替え（`git switch -c <name>`）。既存名なら失敗。
pub fn create_branch(dir: &Path, name: &str) -> Result<()> {
    create_branch_on(&LocalHost, dir, name)
}

pub fn create_branch_on(host: &dyn Host, dir: &Path, name: &str) -> Result<()> {
    anyhow::ensure!(!name.trim().is_empty(), "ブランチ名が空");
    let output =
        run_git(host, dir, ["switch", "-c", name]).context("git switch -c の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "ブランチ作成に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// ブランチを削除（`git branch -d`; `force` で `-D`）。現在ブランチは git が拒否する。
pub fn delete_branch(dir: &Path, name: &str, force: bool) -> Result<()> {
    delete_branch_on(&LocalHost, dir, name, force)
}

pub fn delete_branch_on(host: &dyn Host, dir: &Path, name: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };
    let output =
        run_git(host, dir, ["branch", flag, name]).context("git branch -d の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "ブランチ削除に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// 現在ブランチを push（`git push`）。upstream 未設定なら `-u origin <branch>` で再試行
/// （初回 push の定番動線）。remote が無ければ失敗を返す。
pub fn push(dir: &Path) -> Result<()> {
    push_on(&LocalHost, dir)
}

pub fn push_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_git(host, dir, ["push"]).context("git push の実行に失敗")?;
    if output.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("has no upstream") || stderr.contains("--set-upstream") {
        let branch = git_current_branch_on(host, dir)
            .context("push: 現在ブランチが取得できない（detached HEAD?）")?;
        let retry = run_git(
            host,
            dir,
            ["push", "--set-upstream", "origin", branch.as_str()],
        )
        .context("git push --set-upstream の実行に失敗")?;
        anyhow::ensure!(retry.success(), "push に失敗: {}", git_fail_message(&retry));
        return Ok(());
    }
    anyhow::bail!("push に失敗: {}", stderr.trim());
}

/// upstream から pull（`git pull --ff-only`）。fast-forward できなければ失敗（安全側・merge しない）。
pub fn pull(dir: &Path) -> Result<()> {
    pull_on(&LocalHost, dir)
}

pub fn pull_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_git(host, dir, ["pull", "--ff-only"]).context("git pull の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "pull に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

// ── GitHub 連携（M8: `gh` CLI 経由。git と同じ host 上で動かす＝remote repo でも同じ動線） ──

/// `gh` を `dir` で実行する（git と同じく host 経由）。
fn run_gh<I, S>(host: &dyn Host, dir: &Path, args: I) -> Result<CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    host.run_command(&CommandSpec::new("gh", dir).args(args))
}

/// origin リモートが GitHub なら `owner/repo` を返す（PR ボタンの表示判定に使う）。GitHub 以外は `None`。
pub fn github_slug(dir: &Path) -> Option<String> {
    github_slug_on(&LocalHost, dir)
}

pub fn github_slug_on(host: &dyn Host, dir: &Path) -> Option<String> {
    let output = run_git(host, dir, ["remote", "get-url", "origin"]).ok()?;
    if !output.success() {
        return None;
    }
    parse_github_slug(String::from_utf8_lossy(&output.stdout).trim())
}

/// GitHub の remote URL から `owner/repo` を取り出す（https / ssh 両形式）。テスト可能な純関数。
fn parse_github_slug(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let slug = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = slug.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let repo = parts.next().filter(|part| !part.is_empty())?;
    // owner/repo の 2 段ちょうどだけ受ける（余分な段は弾く）。
    parts.next().is_none().then(|| format!("{owner}/{repo}"))
}

/// 現在ブランチから PR 作成ページをブラウザで開く（`gh pr create --web`）。
/// ブラウザ側で内容確認して作成＝安全側（in-app 入力は不要）。
pub fn create_pr(dir: &Path) -> Result<()> {
    create_pr_on(&LocalHost, dir)
}

pub fn create_pr_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_gh(host, dir, ["pr", "create", "--web"])
        .context("gh pr create の実行に失敗（gh 未導入？）")?;
    anyhow::ensure!(
        output.success(),
        "PR 作成に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// 現在ブランチの PR をブラウザで開く（無ければリポジトリのトップを開く）。
pub fn open_pr_web(dir: &Path) -> Result<()> {
    open_pr_web_on(&LocalHost, dir)
}

pub fn open_pr_web_on(host: &dyn Host, dir: &Path) -> Result<()> {
    // まず現在ブランチの PR。無ければ repo トップ。
    if run_gh(host, dir, ["pr", "view", "--web"])
        .map(|output| output.success())
        .unwrap_or(false)
    {
        return Ok(());
    }
    let output = run_gh(host, dir, ["repo", "view", "--web"])
        .context("gh repo view の実行に失敗（gh 未導入？）")?;
    anyhow::ensure!(
        output.success(),
        "リポジトリを開けません: {}",
        git_fail_message(&output)
    );
    Ok(())
}

// ── AI コミットメッセージ生成（M8: AI-agent-native。Claude Code CLI に diff を渡す） ──

/// tracked 変更（`git diff HEAD`）を Claude Code CLI に渡してコミットメッセージを 1 本生成する。
/// `claude` 未導入 / 差分なし / 失敗時は Err。host 経由なので remote でも（claude があれば）動く。
pub fn ai_commit_message(dir: &Path) -> Result<String> {
    ai_commit_message_on(&LocalHost, dir)
}

pub fn ai_commit_message_on(host: &dyn Host, dir: &Path) -> Result<String> {
    // 引用符 / $ / バッククォートを含めない（sh -c の二重引用符に素で埋めるため）。
    let instruction = "この git diff を読んで簡潔なコミットメッセージを日本語で1本だけ出力して。\
        1行目に要約、変更が複数なら空行のあと箇条書きで本文。\
        前置き・説明・引用符・コードブロックは付けず、メッセージ本文だけを出力して。";
    let script = format!("git --no-pager diff HEAD | claude -p \"{instruction}\"");
    let output = host
        .run_command(&CommandSpec::new("sh", dir).args(["-c", script.as_str()]))
        .context("コミットメッセージ生成の実行に失敗（claude CLI 未導入？）")?;
    anyhow::ensure!(
        output.success(),
        "生成に失敗: {}",
        git_fail_message(&output)
    );
    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();
    anyhow::ensure!(
        !message.is_empty(),
        "生成結果が空（差分が無い？先に stage/編集を）"
    );
    Ok(message)
}

/// worktree の一望情報（⌘O ダッシュボード・M12-12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorktreeStatus {
    /// upstream より進んでいるコミット数。
    pub ahead: usize,
    /// upstream より遅れているコミット数。
    pub behind: usize,
    /// 未コミットの変更があるか。
    pub dirty: bool,
}

/// worktree の ahead/behind と dirty を 1 コマンドで取る（`git status --short --branch`）。
pub fn worktree_status_on(host: &dyn Host, dir: &Path) -> Result<WorktreeStatus> {
    // run_git 経由で `.git/config` ハードニング（fsmonitor/hooks/ext 無効化）を通す。
    let output = run_git(host, dir, ["status", "--short", "--branch"])
        .context("git status を実行できません")?;
    anyhow::ensure!(
        output.success(),
        "git status に失敗: {}",
        git_fail_message(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(parse_status_branch(&stdout))
}

/// `git status --short --branch` の出力をパースする（pure・テスト用に分離）。
/// 先頭行 `## main...origin/main [ahead 1, behind 2]` + 残り行数 = dirty。
fn parse_status_branch(stdout: &str) -> WorktreeStatus {
    let mut status = WorktreeStatus::default();
    let mut lines = stdout.lines();
    if let Some(first) = lines.next() {
        if let Some(bracket) = first.find('[') {
            let inside = first[bracket + 1..].trim_end_matches(']');
            for part in inside.split(',') {
                let part = part.trim();
                if let Some(count) = part.strip_prefix("ahead ") {
                    status.ahead = count.trim().parse().unwrap_or(0);
                } else if let Some(count) = part.strip_prefix("behind ") {
                    status.behind = count.trim().parse().unwrap_or(0);
                }
            }
        }
    }
    status.dirty = lines.any(|line| !line.trim().is_empty());
    status
}

/// 選択コードを自然言語の指示で書き換える（⌘I インライン編集・M12-8）。
/// `claude -p` に「指示 + 対象コード」を**一時ファイル経由**で渡し、書き換え後の
/// コードだけを受け取る。指示・コードはファイル経由なので shell 引用の心配が無い。
/// host 経由なので remote でも（claude があれば）動く。
pub fn inline_rewrite_on(
    host: &dyn Host,
    dir: &Path,
    instruction: &str,
    code: &str,
) -> Result<String> {
    // 指示は 1 行に正規化（ペイロードの構造を単純に保つ）。
    let instruction = instruction.replace(['\n', '\r'], " ");
    let payload = format!("指示: {instruction}\n--- 対象コード ---\n{code}");
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let temp = PathBuf::from(format!("/tmp/shirushi-inline-{unix_ms}.txt"));
    host.write_file(&temp, payload.as_bytes(), host::WriteCondition::Any)
        .context("インライン編集の一時ファイル作成に失敗")?;
    // 引用符 / $ / バッククォートを含めない（sh -c の二重引用符に素で埋めるため）。
    let prompt =
        "入力の最初の行にある指示に従って、対象コードの区切り行より後のコードを書き換えて。\
        出力は書き換え後のコード全体だけ。前置き・説明・コードフェンスは出力しない。\
        インデントと空行は元のスタイルを保つ。";
    let script = format!(
        "claude -p \"{prompt}\" < {temp}; status=$?; rm -f {temp}; exit $status",
        temp = temp.display()
    );
    let output = host
        .run_command(&CommandSpec::new("sh", dir).args(["-c", script.as_str()]))
        .context("インライン編集の実行に失敗（claude CLI 未導入？）")?;
    anyhow::ensure!(
        output.success(),
        "生成に失敗: {}",
        git_fail_message(&output)
    );
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let text = strip_code_fence(&raw);
    // 末尾改行は元コードに合わせる（LLM は末尾改行を付けがち → 差分ノイズを消す）。
    let text = if code.ends_with('\n') {
        format!("{}\n", text.trim_end_matches('\n'))
    } else {
        text.trim_end_matches('\n').to_string()
    };
    anyhow::ensure!(!text.trim().is_empty(), "生成結果が空");
    Ok(text)
}

/// 自然言語からシェルコマンドを 1 行生成する（⌘I ターミナル同型・M12-8）。
/// 生成コマンドは**挿入のみ**（実行は呼び出し側でユーザーの Enter に委ねる）。
pub fn inline_command_on(host: &dyn Host, dir: &Path, instruction: &str) -> Result<String> {
    let instruction = instruction.replace(['\n', '\r'], " ");
    let payload = format!("やりたいこと: {instruction}");
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let temp = PathBuf::from(format!("/tmp/shirushi-inline-cmd-{unix_ms}.txt"));
    host.write_file(&temp, payload.as_bytes(), host::WriteCondition::Any)
        .context("コマンド生成の一時ファイル作成に失敗")?;
    // 引用符 / $ / バッククォートを含めない（sh -c の二重引用符に素で埋めるため）。
    let prompt = "入力のやりたいことを実現するシェルコマンドを1行だけ出力して。\
        対象は macOS の zsh。説明・前置き・コードフェンスは出力しない。";
    let script = format!(
        "claude -p \"{prompt}\" < {temp}; status=$?; rm -f {temp}; exit $status",
        temp = temp.display()
    );
    let output = host
        .run_command(&CommandSpec::new("sh", dir).args(["-c", script.as_str()]))
        .context("コマンド生成の実行に失敗（claude CLI 未導入？）")?;
    anyhow::ensure!(
        output.success(),
        "生成に失敗: {}",
        git_fail_message(&output)
    );
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let text = strip_code_fence(&raw);
    // 最初の非空行だけ（複数行で返ってきても 1 コマンドに絞る）。
    let command = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string();
    anyhow::ensure!(!command.is_empty(), "生成結果が空");
    Ok(command)
}

/// 会話の冒頭から簡潔なスレッドタイトルを1行もらう（AI 自動命名・#6）。
/// `inline_command_on` と同型（一時ファイル経由で shell 引用を回避・host 経由なので remote でも動く）。
/// 失敗（claude 未導入・空応答）は `Err`。呼び出し側は静かに既定名のままにする。
pub fn name_thread_on(
    host: &dyn Host,
    dir: &Path,
    excerpt: &str,
    template: &str,
) -> Result<String> {
    // 引用符 / $ / バッククォートを含めない（sh -c の二重引用符に素で埋めるため）。
    let prompt = "入力はエージェントとの会話の冒頭です。この会話に短いタイトルを付けて。\
        日本語・18文字以内・体言止め・記号や引用符や句読点や番号は付けない・タイトルだけを1行で出力して。";
    oneshot_line_on(host, dir, excerpt, template, prompt, 24)
}

/// 汎用の 1 行生成（スレッド命名・Tier 2 遷移スナップショット要約が共用・FLEET-CONTROL-PLAN P4）。
/// `template` = 既定 Agent ごとの shell テンプレート（{prompt}=指示・{excerpt}=入力ファイル・
/// {out}=最終メッセージ出力先）。stdout にクリーンな 1 行が載るよう各 CLI 差を吸収する
/// （claude -p は素で stdout・codex exec は agent 実行で stdout が汚いため --output-last-message + cat）。
/// `prompt` に引用符 / $ / バッククォートを含めないこと（sh -c の二重引用符に素で埋める）。
pub fn oneshot_line_on(
    host: &dyn Host,
    dir: &Path,
    input: &str,
    template: &str,
    prompt: &str,
    max_chars: usize,
) -> Result<String> {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let temp = PathBuf::from(format!("/tmp/shirushi-oneshot-{unix_ms}.txt"));
    let out = PathBuf::from(format!("/tmp/shirushi-oneshot-{unix_ms}.out"));
    host.write_file(&temp, input.as_bytes(), host::WriteCondition::Any)
        .context("oneshot の一時ファイル作成に失敗")?;
    let body = template
        .replace("{prompt}", prompt)
        .replace("{excerpt}", &temp.display().to_string())
        .replace("{out}", &out.display().to_string());
    let script = format!(
        "{body}; status=$?; rm -f {temp} {out}; exit $status",
        temp = temp.display(),
        out = out.display()
    );
    let output = host
        .run_command(&CommandSpec::new("sh", dir).args(["-c", script.as_str()]))
        .context("oneshot の実行に失敗（既定 Agent の CLI 未導入？）")?;
    anyhow::ensure!(
        output.success(),
        "oneshot に失敗: {}",
        git_fail_message(&output)
    );
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let text = strip_code_fence(&raw);
    // 最初の非空行・前後の引用符/括弧/空白を除去・max_chars で clamp（LLM の饒舌さ対策）。
    let line: String = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '「' | '」' | '『' | '』' | '　' | ' '))
        .chars()
        .take(max_chars)
        .collect();
    anyhow::ensure!(!line.trim().is_empty(), "oneshot の結果が空");
    Ok(line.trim().to_string())
}

/// 出力が ``` フェンスで包まれていたら中身だけ取り出す（そのままなら素通し）。
fn strip_code_fence(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() >= 2 && lines.last().is_some_and(|line| line.trim() == "```") {
        lines.pop();
        lines.remove(0);
        return lines.join("\n");
    }
    trimmed.to_string()
}

/// コミットパネル用の 1 変更（staged / unstaged を分離して持つ。同一ファイルが両方に出得る）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingChange {
    pub path: PathBuf,
    /// index 側（staged）の状態。変更なしは `None`。
    pub staged: Option<StatusKind>,
    /// worktree 側（unstaged）の状態。変更なしは `None`。
    pub unstaged: Option<StatusKind>,
}

/// working-tree の変更を staged / unstaged 別に読む（コミットパネル用）。返すパスは**絶対**。
/// `git_status` は色分け用に XY を 1 状態へ畳むが、こちらは index / worktree を分けて持つ。
pub fn git_changes(dir: &Path) -> Vec<WorkingChange> {
    git_changes_on(&LocalHost, dir)
}

pub fn git_changes_on(host: &dyn Host, dir: &Path) -> Vec<WorkingChange> {
    let Some(repo) = git_repo_root_on(host, dir) else {
        return Vec::new();
    };
    let output = run_git(
        host,
        &repo,
        [
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    );
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut changes = Vec::new();
    for entry in stdout.split('\0') {
        if entry.len() < 4 || &entry[2..3] != " " {
            continue;
        }
        let path = &entry[3..];
        if path.ends_with('/') {
            continue; // untracked ディレクトリは配下ファイルで拾う
        }
        let bytes = entry.as_bytes();
        let staged = index_status(bytes[0]);
        let unstaged = worktree_status(bytes[1]);
        if staged.is_none() && unstaged.is_none() {
            continue;
        }
        changes.push(WorkingChange {
            path: repo.join(path),
            staged,
            unstaged,
        });
    }
    changes
}

/// index 側 1 文字を `StatusKind` へ（' ' と '?' は「index に変更なし」→ `None`）。
fn index_status(x: u8) -> Option<StatusKind> {
    match x {
        b' ' | b'?' => None,
        b'A' => Some(StatusKind::Added),
        b'D' => Some(StatusKind::Deleted),
        b'U' => Some(StatusKind::Conflicted),
        _ => Some(StatusKind::Modified),
    }
}

/// worktree 側 1 文字を `StatusKind` へ（' ' は `None`、'?' は Untracked）。
fn worktree_status(y: u8) -> Option<StatusKind> {
    match y {
        b' ' => None,
        b'?' => Some(StatusKind::Untracked),
        b'A' => Some(StatusKind::Added),
        b'D' => Some(StatusKind::Deleted),
        b'U' => Some(StatusKind::Conflicted),
        _ => Some(StatusKind::Modified),
    }
}

// ── git graph（M8: コミットグラフ。色による方向感覚＝レーン色をパレットに乗せる） ──

/// git graph の 1 行（ログ + レーン割当済み）。描画は右角（railway）方式で線＝矩形に落とす。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCommit {
    pub short_hash: String,
    pub summary: String,
    /// ref ラベル（`HEAD -> main`, `origin/main`, タグ等）。
    pub refs: Vec<String>,
    /// コミットの点が乗るレーン。
    pub dot_lane: usize,
    /// この行の上半分に伸びる縦線のレーン（上の行から降りてくる線）。
    pub lanes_in: Vec<usize>,
    /// この行の下半分に伸びる縦線のレーン（下の行へ降りる線）。
    pub lanes_out: Vec<usize>,
    /// 点（dot_lane）と横で結ぶ相手レーン（分岐＝第2親 / 合流＝子の収束）。
    pub connectors: Vec<usize>,
}

/// ログ解析前の生コミット（親・要約・ref）。レーン割当の入力。
struct RawCommit {
    hash: String,
    parents: Vec<String>,
    summary: String,
    refs: Vec<String>,
}

/// 直近 `limit` 件のコミットグラフを返す（`git log` → レーン割当）。repo 外は空。
pub fn git_log_graph(dir: &Path, limit: usize) -> Vec<GraphCommit> {
    git_log_graph_on(&LocalHost, dir, limit)
}

pub fn git_log_graph_on(host: &dyn Host, dir: &Path, limit: usize) -> Vec<GraphCommit> {
    let count = format!("-n{limit}");
    let output = run_git(
        host,
        dir,
        [
            "--no-optional-locks",
            "log",
            count.as_str(),
            // topo 順で「子は必ず親より前」を保証（レーン割当の前提）＝ git log --graph と同じ並び。
            "--topo-order",
            // %h=短縮hash %p=短縮親 %s=要約 %D=ref名。0x1f 区切り・行=コミット。
            "--pretty=format:%h%x1f%p%x1f%s%x1f%D",
        ],
    );
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut raws = Vec::new();
    for line in text.lines() {
        let mut parts = line.split('\u{1f}');
        let hash = match parts.next() {
            Some(hash) if !hash.is_empty() => hash.to_string(),
            _ => continue,
        };
        let parents = parts
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let summary = parts.next().unwrap_or("").to_string();
        let refs = parts
            .next()
            .unwrap_or("")
            .split(',')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect();
        raws.push(RawCommit {
            hash,
            parents,
            summary,
            refs,
        });
    }
    layout_graph(&raws)
}

/// レーン割当（git log --graph 相当の縦レーン + 分岐/合流コネクタ）。テスト可能な純関数。
fn layout_graph(raws: &[RawCommit]) -> Vec<GraphCommit> {
    // lanes[l] = そのレーンが次に描くのを待っている commit hash（無ければ空）。
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut result = Vec::with_capacity(raws.len());

    for raw in raws {
        let lanes_in: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, lane)| lane.as_ref().map(|_| i))
            .collect();

        // 点のレーン: このコミットを待っているレーン。無ければ空きレーン（＝ブランチ先端）。
        let dot_lane = match lanes
            .iter()
            .position(|lane| lane.as_deref() == Some(raw.hash.as_str()))
        {
            Some(lane) => lane,
            None => match lanes.iter().position(Option::is_none) {
                Some(lane) => lane,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };

        let mut connectors = Vec::new();

        // 同じ hash を待つ他レーンを dot_lane へ畳む（複数の子が合流）。
        for (index, lane) in lanes.iter_mut().enumerate() {
            if index != dot_lane && lane.as_deref() == Some(raw.hash.as_str()) {
                *lane = None;
                connectors.push(index);
            }
        }

        // 第1親は同じレーンを継続。親が無ければ根（レーンを空ける）。
        match raw.parents.first() {
            Some(first) => lanes[dot_lane] = Some(first.clone()),
            None => lanes[dot_lane] = None,
        }
        // 第2親以降は別レーンへ（既に待っていれば再利用・無ければ空き・無ければ新設）。
        for parent in raw.parents.iter().skip(1) {
            let target = lanes
                .iter()
                .position(|lane| lane.as_deref() == Some(parent.as_str()))
                .or_else(|| lanes.iter().position(Option::is_none))
                .unwrap_or_else(|| {
                    lanes.push(None);
                    lanes.len() - 1
                });
            lanes[target] = Some(parent.clone());
            connectors.push(target);
        }

        // 末尾の空きレーンを畳む（幅を詰める。中間の空きは位置維持のため残す）。
        while lanes.last() == Some(&None) {
            lanes.pop();
        }

        let lanes_out: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, lane)| lane.as_ref().map(|_| i))
            .collect();

        result.push(GraphCommit {
            short_hash: raw.hash.clone(),
            summary: raw.summary.clone(),
            refs: raw.refs.clone(),
            dot_lane,
            lanes_in,
            lanes_out,
            connectors,
        });
    }
    result
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
fn head_blob_on(host: &dyn Host, dir: &Path, name: &OsStr) -> Option<String> {
    let spec = format!("HEAD:./{}", name.to_string_lossy());
    let output = run_git(host, dir, ["--no-optional-locks", "show", spec.as_str()]).ok()?;
    output
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `file`（絶対パス）の HEAD 版 vs 現在テキストを行単位で diff。HEAD が無ければ空
/// （新規/未追跡は tree/tab の色で示すのでガターは静かにする）。
pub fn buffer_diff(file: &Path, current: &str) -> Vec<DiffHunk> {
    buffer_diff_on(&LocalHost, file, current)
}

pub fn buffer_diff_on(host: &dyn Host, file: &Path, current: &str) -> Vec<DiffHunk> {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
        return Vec::new();
    };
    match head_blob_on(host, dir, name) {
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

/// [`Worktree::all_files`] の host 直呼び版（背景スレッド用・Worktree は Rc なので Send できない）。
pub fn all_files_on(host: &dyn Host, root: &Path, limit: usize) -> Vec<(PathBuf, String)> {
    let mut files = host
        .list_files(root, limit)
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            (path, relative)
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    files
}

/// HEAD のファイル内容（テキスト）。無ければ None（新規ファイル等）。M11 diff タブ/hunk 操作用。
pub fn head_text_on(host: &dyn Host, file: &Path) -> Option<String> {
    let (dir, name) = (file.parent()?, file.file_name()?);
    head_blob_on(host, dir, name)
}

/// HEAD vs 現在テキストの **unified diff 文字列**（M11-9 diff タブ）。差分なしは None。
pub fn unified_diff_on(host: &dyn Host, file: &Path, current: &str) -> Option<String> {
    use imara_diff::intern::InternedInput;
    use imara_diff::sources::lines_with_terminator;
    use imara_diff::{Algorithm, UnifiedDiffBuilder};
    let head = head_text_on(host, file).unwrap_or_default();
    if head == current {
        return None;
    }
    let head_normalized = normalize_newlines(&head);
    let current_normalized = normalize_newlines(current);
    let input = InternedInput::new(
        lines_with_terminator(head_normalized.as_str()),
        lines_with_terminator(current_normalized.as_str()),
    );
    let body = imara_diff::diff(
        Algorithm::Histogram,
        &input,
        UnifiedDiffBuilder::new(&input),
    );
    if body.is_empty() {
        return None;
    }
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Some(format!(
        "--- a/{name}（HEAD）\n+++ b/{name}（バッファ）\n{body}"
    ))
}

/// 任意テキスト同士の unified diff（エージェント承認カードの「エディタで開く」・M12-6）。
/// 差分なしは None。
pub fn unified_diff_texts(old_text: &str, new_text: &str, name: &str) -> Option<String> {
    use imara_diff::intern::InternedInput;
    use imara_diff::sources::lines_with_terminator;
    use imara_diff::{Algorithm, UnifiedDiffBuilder};
    if old_text == new_text {
        return None;
    }
    let old_normalized = normalize_newlines(old_text);
    let new_normalized = normalize_newlines(new_text);
    let input = InternedInput::new(
        lines_with_terminator(old_normalized.as_str()),
        lines_with_terminator(new_normalized.as_str()),
    );
    let body = imara_diff::diff(
        Algorithm::Histogram,
        &input,
        UnifiedDiffBuilder::new(&input),
    );
    if body.is_empty() {
        return None;
    }
    Some(format!(
        "--- a/{name}（現在）\n+++ b/{name}（提案）\n{body}"
    ))
}

/// 1 hunk 分の unified diff（`git apply --cached` に食わせる形・M11-10 hunk stage）。
/// パスはリポジトリルート相対で書く。
pub fn hunk_patch_text(
    relative_path: &str,
    head_lines: &[&str],
    current_lines: &[&str],
    hunk: &DiffHunk,
) -> String {
    let old_start = hunk.old_range.start as usize;
    let old_len = hunk.old_range.len();
    let new_start = hunk.new_range.start as usize;
    let new_len = hunk.new_range.len();
    let mut body = String::new();
    for line in head_lines.iter().skip(old_start).take(old_len) {
        body.push('-');
        body.push_str(line);
        body.push('\n');
    }
    for line in current_lines.iter().skip(new_start).take(new_len) {
        body.push('+');
        body.push_str(line);
        body.push('\n');
    }
    format!(
        "--- a/{relative_path}\n+++ b/{relative_path}\n@@ -{},{} +{},{} @@\n{body}",
        old_start + 1,
        old_len,
        new_start + 1,
        new_len,
    )
}

/// パッチを index へ適用する（hunk 単位 stage・M11-10）。パッチは一時ファイル経由（host 汎用）。
pub fn apply_patch_to_index_on(host: &dyn Host, repo_root: &Path, patch: &str) -> Result<()> {
    let temp = repo_root.join(".git/shirushi-hunk.patch");
    host.write_file(&temp, patch.as_bytes(), host::WriteCondition::Any)
        .context("パッチの書き込みに失敗")?;
    let temp_arg = temp.to_string_lossy().to_string();
    let result = run_git(
        host,
        repo_root,
        ["apply", "--cached", "--unidiff-zero", temp_arg.as_str()],
    );
    // 一時ファイルは成否に関わらず消す（remove_file は local のみ有効・失敗は無視）。
    let _ = std::fs::remove_file(&temp);
    result.map(|_| ()).context("git apply --cached に失敗")
}

/// 1 行の blame（作者・日付・要旨・M11-11）。`line` は 1 始まり。未コミット行は「未コミット」。
/// dirty バッファでは行ずれの近似になる（HEAD 基準）。失敗は None（表示しない）。
pub fn blame_line_on(host: &dyn Host, file: &Path, line: u32) -> Option<String> {
    let dir = file.parent()?;
    let repo = git_repo_root_on(host, dir)?;
    let file_arg = file.to_string_lossy().to_string();
    let range = format!("{line},{line}");
    let output = run_git(
        host,
        &repo,
        [
            "blame",
            "--porcelain",
            "-L",
            range.as_str(),
            "--",
            file_arg.as_str(),
        ],
    )
    .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    if text.starts_with("0000000") {
        return Some("未コミット".to_string());
    }
    let mut author = None;
    let mut time = None;
    let mut summary = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("author ") {
            author = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("author-time ") {
            time = value.parse::<i64>().ok();
        } else if let Some(value) = line.strip_prefix("summary ") {
            summary = Some(value.to_string());
        }
    }
    let (author, summary) = (author?, summary?);
    let date = time.map(format_unix_date).unwrap_or_default();
    Some(format!("{author}, {date} • {summary}"))
}

/// unix 秒 → "YYYY-MM-DD"（依存なしの civil 変換・Howard Hinnant のアルゴリズム）。
fn format_unix_date(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

// ── ツリーのファイル操作（M10・local のみ。remote 版は M13 の Host 拡張と一緒に） ──

/// 空ファイルを作る（既存ならエラー＝上書きしない）。
pub fn create_file_local(path: &Path) -> Result<()> {
    anyhow::ensure!(!path.exists(), "既に存在する: {}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("親フォルダを作れない: {}", parent.display()))?;
    }
    std::fs::write(path, b"").with_context(|| format!("作成に失敗: {}", path.display()))
}

/// フォルダを作る（既存ならエラー）。
pub fn create_dir_local(path: &Path) -> Result<()> {
    anyhow::ensure!(!path.exists(), "既に存在する: {}", path.display());
    std::fs::create_dir_all(path).with_context(|| format!("作成に失敗: {}", path.display()))
}

/// リネーム/移動（移動先が既存ならエラー＝上書きしない）。
pub fn rename_local(from: &Path, to: &Path) -> Result<()> {
    anyhow::ensure!(!to.exists(), "移動先が既に存在する: {}", to.display());
    std::fs::rename(from, to)
        .with_context(|| format!("リネームに失敗: {} → {}", from.display(), to.display()))
}

/// 複製する。`name.ext` → `name copy.ext`（衝突したら `name copy 2.ext`…）。フォルダは再帰コピー。
pub fn duplicate_local(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().context("親フォルダが無い")?;
    let stem = path.file_stem().and_then(OsStr::to_str).unwrap_or("copy");
    let extension = path.extension().and_then(OsStr::to_str);
    let mut candidate = None;
    for index in 0..100 {
        let name = match (index, extension) {
            (0, Some(ext)) => format!("{stem} copy.{ext}"),
            (0, None) => format!("{stem} copy"),
            (n, Some(ext)) => format!("{stem} copy {}.{ext}", n + 1),
            (n, None) => format!("{stem} copy {}", n + 1),
        };
        let target = parent.join(name);
        if !target.exists() {
            candidate = Some(target);
            break;
        }
    }
    let target = candidate.context("複製先の名前を決められない（copy が多すぎる）")?;
    if path.is_dir() {
        copy_dir_recursive(path, &target)?;
    } else {
        std::fs::copy(path, &target).with_context(|| format!("複製に失敗: {}", path.display()))?;
    }
    Ok(target)
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// OS のゴミ箱へ入れる（macOS: `/usr/bin/trash`、無ければ Finder 経由）。完全削除はしない。
pub fn trash_local(path: &Path) -> Result<()> {
    anyhow::ensure!(path.exists(), "存在しない: {}", path.display());
    if Path::new("/usr/bin/trash").exists() {
        let status = std::process::Command::new("/usr/bin/trash")
            .arg(path)
            .status()
            .context("trash コマンドの起動に失敗")?;
        anyhow::ensure!(status.success(), "trash が失敗: {}", path.display());
        return Ok(());
    }
    // フォールバック: Finder に頼む（AppleScript）。
    let script = format!(
        "tell application \"Finder\" to delete POSIX file \"{}\"",
        path.display()
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .status()
        .context("osascript の起動に失敗")?;
    anyhow::ensure!(
        status.success(),
        "Finder でのゴミ箱移動が失敗: {}",
        path.display()
    );
    Ok(())
}

/// Finder で対象を表示（親フォルダを開いて選択状態にする）。macOS の `open -R`。
pub fn reveal_in_finder_local(path: &Path) -> Result<()> {
    anyhow::ensure!(path.exists(), "存在しない: {}", path.display());
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .status()
        .context("open の起動に失敗")?;
    anyhow::ensure!(status.success(), "Finder 表示が失敗: {}", path.display());
    Ok(())
}

/// OS の既定アプリで開く（ファイル=関連付けアプリ / フォルダ=Finder）。macOS の `open`。
pub fn open_with_default_app_local(path: &Path) -> Result<()> {
    anyhow::ensure!(path.exists(), "存在しない: {}", path.display());
    let status = std::process::Command::new("/usr/bin/open")
        .arg(path)
        .status()
        .context("open の起動に失敗")?;
    anyhow::ensure!(
        status.success(),
        "既定アプリでの起動が失敗: {}",
        path.display()
    );
    Ok(())
}

// ── ファイル監視（watch 基盤・M10） ──

/// worktree の再帰監視ハンドル。drop すると監視停止。
/// local は notify（FSEvents）、remote は Host 経由の poll（daemon push）。型は外に漏らさない。
pub struct Watch {
    _inner: WatchInner,
}

enum WatchInner {
    /// notify watcher は drop で監視停止するため保持のみ（read しない）。
    #[allow(dead_code)]
    Local(notify::RecommendedWatcher),
    /// remote: pump スレッドが HostWatch を所有。drop で stop を立てるとスレッドが抜け、
    /// HostWatch も drop されて daemon 側の監視も止まる。
    Remote {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let WatchInner::Remote { stop } = &self._inner {
            stop.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

/// `root` 以下を再帰監視し、変化したパス群（**絶対パス**）をコールバックへ渡す。
/// - local: notify（macOS は FSEvents）。呼び出しは watcher スレッド。
/// - remote: Host の watch（M13・daemon poll → Event push）。相対パスを `root` 基準で絶対化して
///   渡す＝local と同じく開バッファのパスと突き合わせられる。
/// イベント種別は使わない（「そのパスで何かが起きた」の粒度）。UI 側は channel で受けて executor へ。
pub fn watch_root(
    host: &std::sync::Arc<dyn Host>,
    root: &Path,
    on_paths: impl Fn(Vec<PathBuf>) + Send + 'static,
) -> Result<Watch> {
    if host.is_remote() {
        let host_watch = host
            .clone()
            .watch()
            .context("remote watch の開始に失敗")?
            .context("remote host が watch を返さない")?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pump_stop = stop.clone();
        let root = root.to_path_buf();
        std::thread::Builder::new()
            .name("shirushi-remote-watch-pump".to_string())
            .spawn(move || {
                use std::sync::atomic::Ordering::Acquire;
                while !pump_stop.load(Acquire) {
                    if let Some(relative) =
                        host_watch.recv_timeout(std::time::Duration::from_millis(500))
                    {
                        let absolute: Vec<PathBuf> =
                            relative.into_iter().map(|path| root.join(path)).collect();
                        if !absolute.is_empty() {
                            on_paths(absolute);
                        }
                    }
                }
            })
            .context("remote watch pump thread spawn")?;
        return Ok(Watch {
            _inner: WatchInner::Remote { stop },
        });
    }

    use notify::Watcher as _;
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            if !event.paths.is_empty() {
                on_paths(event.paths);
            }
        }
    })
    .context("ファイル監視の初期化に失敗")?;
    watcher
        .watch(root, notify::RecursiveMode::Recursive)
        .with_context(|| format!("ファイル監視を開始できない: {}", root.display()))?;
    Ok(Watch {
        _inner: WatchInner::Local(watcher),
    })
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
        self.hunks.push(DiffHunk {
            old_range: before,
            new_range: after,
            kind,
        });
    }
    fn finish(self) -> Self::Out {
        self.hunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_branch_parses_ahead_behind_dirty() {
        // ahead/behind + dirty。
        let both = "## main...origin/main [ahead 3, behind 1]\n M src/a.rs\n?? new.txt\n";
        assert_eq!(
            parse_status_branch(both),
            WorktreeStatus {
                ahead: 3,
                behind: 1,
                dirty: true
            }
        );
        // upstream 無し・クリーン。
        assert_eq!(
            parse_status_branch("## feature\n"),
            WorktreeStatus::default()
        );
        // ahead のみ。
        let ahead = "## main...origin/main [ahead 2]\n";
        assert_eq!(
            parse_status_branch(ahead),
            WorktreeStatus {
                ahead: 2,
                behind: 0,
                dirty: false
            }
        );
    }

    #[test]
    fn strip_code_fence_unwraps_and_passes_through() {
        // フェンス付き（言語タグあり）→ 中身だけ。
        assert_eq!(strip_code_fence("```rust\nfn a() {}\n```"), "fn a() {}");
        // フェンス無し → trim だけして素通し。
        assert_eq!(strip_code_fence("  fn a() {}\n"), "fn a() {}");
        // 閉じフェンスが無い壊れた出力 → 素通し（安全側）。
        assert_eq!(strip_code_fence("```\nfn a() {}"), "```\nfn a() {}");
    }

    #[test]
    fn blame_line_reads_real_history() {
        // この repo 自身の committed ファイルで実 blame（作者・日付・要旨の合成を検証）。
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let readme = repo.join("README.md");
        if !readme.exists() {
            return; // 環境依存の保険
        }
        let result = blame_line_on(host::LocalHost::shared().as_ref(), &readme, 1);
        let Some(text) = result else {
            return; // shallow clone 等では blame が引けないことがある
        };
        // "作者, YYYY-MM-DD • 要旨" か「未コミット」のどちらか。
        assert!(
            text == "未コミット" || (text.contains(" • ") && text.contains("-")),
            "{text}"
        );
    }

    #[test]
    fn unix_date_formats_known_values() {
        assert_eq!(format_unix_date(0), "1970-01-01");
        assert_eq!(format_unix_date(86_400), "1970-01-02");
        assert_eq!(format_unix_date(1_752_710_400), "2025-07-17"); // 2025-07-17T00:00:00Z
    }

    #[test]
    fn hunk_stage_round_trip_on_temp_repo() {
        let dir = std::env::temp_dir().join(format!("shirushi_hunk_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init", "-q"]);
        run(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "base",
        ]);
        // 2 hunk 作る: 先頭に追記 + 末尾を変更
        let current = "zero\none\ntwo\nTHREE\n";
        std::fs::write(&file, current).unwrap();
        let host = host::LocalHost::shared();
        let hunks = buffer_diff_on(host.as_ref(), &file, current);
        assert_eq!(hunks.len(), 2, "hunks: {hunks:?}");
        // 1 個目（zero 追加）だけ stage
        let head = head_text_on(host.as_ref(), &file).unwrap();
        let head_lines: Vec<&str> = head.lines().collect();
        let current_lines: Vec<&str> = current.lines().collect();
        let patch = hunk_patch_text("a.txt", &head_lines, &current_lines, &hunks[0]);
        apply_patch_to_index_on(host.as_ref(), &dir, &patch).expect("stage できる");
        // index には zero 追加のみ・THREE は未 stage のはず
        let staged = String::from_utf8(run(&["diff", "--cached"]).stdout).unwrap();
        assert!(staged.contains("+zero"), "staged: {staged}");
        assert!(!staged.contains("+THREE"), "staged: {staged}");
        let unstaged = String::from_utf8(run(&["diff"]).stdout).unwrap();
        assert!(unstaged.contains("+THREE"), "unstaged: {unstaged}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_operations_create_rename_duplicate() {
        let dir = std::env::temp_dir().join(format!("shirushi_fileops_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let folder = dir.join("sub");
        create_dir_local(&folder).unwrap();
        assert!(folder.is_dir());
        assert!(create_dir_local(&folder).is_err()); // 既存はエラー

        let file = folder.join("a.rs");
        create_file_local(&file).unwrap();
        assert!(file.is_file());
        assert!(create_file_local(&file).is_err());

        let renamed = folder.join("b.rs");
        rename_local(&file, &renamed).unwrap();
        assert!(renamed.exists() && !file.exists());
        std::fs::write(&renamed, "x").unwrap();
        let copy1 = duplicate_local(&renamed).unwrap();
        assert_eq!(copy1.file_name().unwrap().to_str().unwrap(), "b copy.rs");
        let copy2 = duplicate_local(&renamed).unwrap();
        assert_eq!(copy2.file_name().unwrap().to_str().unwrap(), "b copy 2.rs");
        // 上書き拒否
        assert!(rename_local(&copy1, &copy2).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
    use std::process::Command;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("shirushi_project_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn scans_sorted_and_marks_gitignore_dimmed() {
        let root = scratch("scan");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("main.rs"), "").unwrap();
        std::fs::write(root.join("build.log"), "").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();

        let worktree = Worktree::new(&root).unwrap();
        let entries = worktree.read_root().unwrap();
        let find = |name: &str| entries.iter().find(|entry| entry.name == name);

        // 追跡対象は ignored=false
        assert_eq!(find("src").map(|entry| entry.ignored), Some(false));
        assert_eq!(find("main.rs").map(|entry| entry.ignored), Some(false));
        // gitignore 対象は除外せず ignored=true（薄字で見える）
        assert_eq!(
            find("target").map(|entry| entry.ignored),
            Some(true),
            "無視 dir も表示・薄字"
        );
        assert_eq!(
            find("build.log").map(|entry| entry.ignored),
            Some(true),
            "無視 glob も表示・薄字"
        );
        // .git は常に除外
        assert!(find(".git").is_none(), ".git を除外");
        // ディレクトリが先頭（src と target の 2 つ、名前順で src が先）
        assert_eq!(
            entries.first().map(|entry| entry.name.as_str()),
            Some("src")
        );

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
        let relatives: Vec<String> = worktree
            .all_files(1000)
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        assert!(relatives.contains(&"src/main.rs".to_string()));
        assert!(relatives.contains(&"README.md".to_string()));
        assert!(
            !relatives.iter().any(|r| r.contains("target")),
            "gitignore の target を除外"
        );
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
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git 実行")
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

    #[test]
    fn git_ops_stage_commit_branch_on_temp_repo() {
        let root = scratch("gitops");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git 実行")
        };
        if !git(&["init", "-q"]).status.success() {
            return; // git 無し環境はスキップ
        }
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "tester"]);

        // 新規ファイル → stage 前は unstaged=Untracked
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let a = std::fs::canonicalize(root.join("a.txt")).unwrap();
        let before = git_changes(&root);
        let entry = before
            .iter()
            .find(|c| c.path == a)
            .expect("a.txt が変更に出る");
        assert_eq!(entry.unstaged, Some(StatusKind::Untracked));
        assert_eq!(entry.staged, None);

        // stage → staged=Added
        stage_all(&root).unwrap();
        let staged = git_changes(&root);
        let entry = staged
            .iter()
            .find(|c| c.path == a)
            .expect("a.txt が staged に出る");
        assert_eq!(entry.staged, Some(StatusKind::Added));

        // commit → working-tree クリーン
        commit(&root, "add a").unwrap();
        assert!(git_changes(&root).is_empty(), "commit 後は変更なし");

        // 新規ブランチ作成 → 現在ブランチが feature
        create_branch(&root, "feature").unwrap();
        assert_eq!(git_current_branch(&root).as_deref(), Some("feature"));

        // feature 上で編集 → stage_path → commit
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        stage_path(&root, &root.join("a.txt")).unwrap();
        commit(&root, "edit a").unwrap();

        // 既定ブランチへ戻る（名前は環境依存 main/master なので feature 以外を拾う）
        let base = git_branches(&root)
            .into_iter()
            .find(|branch| branch != "feature")
            .expect("base ブランチ");
        switch_branch(&root, &base).unwrap();

        // feature を削除（未マージなので force）
        delete_branch(&root, "feature", true).unwrap();
        assert!(!git_branches(&root).contains(&"feature".to_string()));

        // 空メッセージ commit は失敗
        assert!(commit(&root, "   ").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worktree_add_remove_and_branch_delete_guard() {
        let root = scratch("worktree");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git 実行")
        };
        if !git(&["init", "-q"]).status.success() {
            return; // git 無し環境はスキップ
        }
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "tester"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        stage_all(&root).unwrap();
        commit(&root, "init").unwrap();

        // feature ブランチを作って base に戻る（feature は未チェックアウト状態にする）。
        create_branch(&root, "feature").unwrap();
        let base = git_branches(&root)
            .into_iter()
            .find(|branch| branch != "feature")
            .expect("base ブランチ");
        switch_branch(&root, &base).unwrap();

        // feature を worktree として隣に開く。
        let wt = root
            .parent()
            .unwrap()
            .join(format!("wt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wt);
        add_worktree(&root, &wt, "feature").unwrap();
        assert!(
            git_worktrees(&root)
                .iter()
                .any(|worktree| worktree.branch.as_deref() == Some("feature")),
            "worktree 一覧に feature が出る"
        );

        // worktree に checkout 中のブランチは削除できない（git が拒否＝バグ報告の状況）。
        assert!(
            delete_branch(&root, "feature", true).is_err(),
            "worktree 使用中のブランチ削除は失敗する"
        );

        // worktree を削除 → 一覧から消える。
        remove_worktree(&root, &wt, true).unwrap();
        assert!(
            !git_worktrees(&root)
                .iter()
                .any(|worktree| worktree.branch.as_deref() == Some("feature")),
            "remove 後は feature worktree が消える"
        );

        // worktree が無くなればブランチ削除は通る。
        delete_branch(&root, "feature", true).unwrap();
        assert!(!git_branches(&root).contains(&"feature".to_string()));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn task_worktree_preview_and_explicit_integration() {
        let root = scratch("task_integration");
        let wt = scratch("task_integration_wt");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&wt);
        std::fs::create_dir_all(&root).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .expect("git 実行")
        };
        if !git(&root, &["init", "-q"]).status.success() {
            return;
        }
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "tester"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "base"]);

        create_task_worktree_on(&LocalHost, &root, &wt, "task/preview").unwrap();
        std::fs::write(wt.join("task.txt"), "done\n").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-q", "-m", "task result"]);

        let preview = preview_merge_on(&LocalHost, &root, "task/preview").unwrap();
        assert!(
            preview.clean,
            "独立変更は conflict radar を通る: {}",
            preview.detail
        );
        let before = git_head_oid_on(&LocalHost, &root).unwrap();
        let after = integrate_branch_on(&LocalHost, &root, "task/preview").unwrap();
        assert_ne!(before, after);
        assert_eq!(
            std::fs::read_to_string(root.join("task.txt")).unwrap(),
            "done\n"
        );

        remove_worktree(&root, &wt, true).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn push_sets_upstream_to_local_bare_remote() {
        let root = scratch("gitpush");
        let bare = scratch("gitpush_remote");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&bare).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .expect("git 実行")
        };
        if !git(&root, &["init", "-q"]).status.success() {
            return; // git 無し環境はスキップ
        }
        git(&bare, &["init", "-q", "--bare"]);
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "tester"]);
        git(&root, &["remote", "add", "origin", &bare.to_string_lossy()]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "init"]);

        // upstream 未設定 → push が set-upstream 経由で通る
        push(&root).unwrap();
        // 2 回目（upstream 設定済み・up-to-date）も成功
        push(&root).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn layout_graph_diamond_merge() {
        let raw = |hash: &str, parents: &[&str]| RawCommit {
            hash: hash.to_string(),
            parents: parents.iter().map(|parent| parent.to_string()).collect(),
            summary: String::new(),
            refs: Vec::new(),
        };
        // C(merge B,D) / B(A) / D(A) / A(root) → ダイヤモンド
        let rows = layout_graph(&[
            raw("C", &["B", "D"]),
            raw("B", &["A"]),
            raw("D", &["A"]),
            raw("A", &[]),
        ]);
        assert_eq!(rows.len(), 4);
        // C はレーン0、D 用にレーン1へ分岐
        assert_eq!(rows[0].dot_lane, 0);
        assert!(rows[0].connectors.contains(&1), "C→D の分岐コネクタ");
        // D はレーン1
        assert_eq!(rows[2].dot_lane, 1);
        // A で 2 レーンが合流（レーン1→0）
        assert_eq!(rows[3].dot_lane, 0);
        assert!(rows[3].connectors.contains(&1), "D 側レーンが A で合流");
        // A は根（下へ伸びる線は無い）
        assert!(rows[3].lanes_out.is_empty());
    }

    #[test]
    fn git_log_graph_linear_on_temp_repo() {
        let root = scratch("gitgraph");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .expect("git 実行")
        };
        if !git(&["init", "-q"]).status.success() {
            return;
        }
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "tester"]);
        for name in ["one", "two", "three"] {
            std::fs::write(root.join("a.txt"), format!("{name}\n")).unwrap();
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", name]);
        }
        let graph = git_log_graph(&root, 10);
        assert_eq!(graph.len(), 3);
        // 直線履歴 → 全部レーン0・要約は新しい順（three → two → one）
        assert!(graph.iter().all(|commit| commit.dot_lane == 0));
        assert_eq!(graph[0].summary, "three");
        assert_eq!(graph[2].summary, "one");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_github_slug_handles_https_and_ssh() {
        assert_eq!(
            parse_github_slug("https://github.com/iKora128/shirushi.git").as_deref(),
            Some("iKora128/shirushi")
        );
        assert_eq!(
            parse_github_slug("git@github.com:iKora128/shirushi.git").as_deref(),
            Some("iKora128/shirushi")
        );
        assert_eq!(
            parse_github_slug("https://github.com/owner/repo").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            parse_github_slug("ssh://git@github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        // GitHub 以外・段数不一致は None
        assert_eq!(parse_github_slug("https://gitlab.com/owner/repo.git"), None);
        assert_eq!(
            parse_github_slug("git@github.com:owner/repo/extra.git"),
            None
        );
        assert_eq!(parse_github_slug("git@github.com:owner"), None);
    }
}
