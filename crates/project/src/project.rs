//! project — ファイルシステムの走査。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §2: M3 は**遅延 read_dir**（展開時に直下を読む）で、
//! `.git` と gitignore 対象を除外する（ripgrep の `ignore` crate を使用）。
//! ファイル監視・インクリメンタル更新は後続（M8/性能）で追加する。
//!
//! ⌘P の「無視されたファイルは 2 回目に」（[`ignored_files_local`]）は、stablyai/orca@646e9a5 の
//! `src/shared/quick-open-filter.ts`（ignoredPass）と `docs/site/content/docs/model/quick-open.mdx`
//! の考え方を参考にした（実装は独立。Orca は rg で自動に足し、necoder は一致なしの時に人が選ぶ）。

pub mod review;
pub mod file_operations;
pub mod todos;

use anyhow::{Context as _, Result};
use host::{CommandOutput, CommandSpec, Host, LocalHost};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A project execution source. Host and root must stay paired so identical remote/local
/// paths cannot be confused by the workspace shell.
#[derive(Clone)]
pub struct ProjectSource {
    host: Arc<dyn Host>,
    root: PathBuf,
    restored: bool,
}

impl ProjectSource {
    pub fn local(root: PathBuf) -> Self {
        Self {
            host: LocalHost::shared(),
            root,
            restored: false,
        }
    }

    pub fn new(host: Arc<dyn Host>, root: PathBuf) -> Self {
        Self {
            host,
            root,
            restored: false,
        }
    }

    /// 前回終了時に自分で保存した root から復元する。root は host が返した綴りそのものなので、
    /// 開くときに正式な綴りやディレクトリ判定を聞き直さない（起動を止めないため）。
    pub fn restored(host: Arc<dyn Host>, root: PathBuf) -> Self {
        Self {
            host,
            root,
            restored: true,
        }
    }

    /// 保存済み root からの復元か（＝ host へ問い合わせずに開いてよいか）。
    pub fn is_restored(&self) -> bool {
        self.restored
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

    pub fn into_parts_with_trust(self) -> (Arc<dyn Host>, PathBuf, bool) {
        (self.host, self.root, self.restored)
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
    /// 起動時の遅延復元では**空で始めて後から差し替える**（`.gitignore` の読み込みは
    /// remote だとネット往復になるので、窓を出す前には読まない）。Worktree は `Rc` で
    /// メインスレッド専用なので内部可変で十分。
    ignore: RefCell<Gitignore>,
    /// `host.is_remote()` のキャッシュ。Render は Host を一切呼ばない規律
    /// （workspace の render 監査テストが呼び出しゼロを強制する）ため、
    /// render から参照する判定はここを使う。
    remote: bool,
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

        let remote = host.is_remote();
        Ok(Worktree {
            host,
            root,
            ignore: RefCell::new(ignore),
            remote,
        })
    }

    /// 渡された root を**そのまま信じて**開く（起動時の復元用）。
    ///
    /// [`Self::with_host`] は root の正式な綴りの問い合わせ・ディレクトリ確認・`.gitignore`
    /// 読み込みで host へ 3 回往復する。remote ではこれが窓の表示を止める。復元で渡す root は
    /// 前回 host が返した綴りそのものなので、聞き直す必要がない。
    /// 無視規則は空で始まるので、繋がったら [`Self::set_ignore_source`] で入れ直すこと。
    pub fn trusted(host: Arc<dyn Host>, root: PathBuf) -> Worktree {
        let remote = host.is_remote();
        Worktree {
            host,
            root,
            ignore: RefCell::new(Gitignore::empty()),
            remote,
        }
    }

    /// `.gitignore` の本文から無視規則を組み直す（背景で読んだ内容を後から入れる）。
    pub fn set_ignore_source(&self, contents: &str) {
        let ignore_path = self.root.join(".gitignore");
        let mut builder = GitignoreBuilder::new(&self.root);
        for line in contents.lines() {
            let _invalid_pattern = builder.add_line(Some(ignore_path.clone()), line);
        }
        if let Ok(ignore) = builder.build() {
            *self.ignore.borrow_mut() = ignore;
        }
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// remote host 上の worktree か（構築時キャッシュ・render から呼んでよい）。
    pub fn is_remote(&self) -> bool {
        self.remote
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
        read_any_dir_on(self.host.as_ref(), &self.root, &self.ignore.borrow(), dir)
    }

    /// パスが gitignore 対象か（watch イベントのノイズ除去用。ディレクトリ判定不能なら false 扱いで問い合わせる）。
    pub fn is_ignored(&self, path: &Path) -> bool {
        let ignore = self.ignore.borrow();
        ignore.matched(path, false).is_ignore() || ignore.matched(path, true).is_ignore()
    }

    /// 無視規則のスナップショット。`Worktree` は `Rc` で背景スレッドへ送れないので、
    /// remote のツリー読み取りを背景へ出すときは `host` + `root` + これを持ち出す。
    pub fn ignore_snapshot(&self) -> Gitignore {
        self.ignore.borrow().clone()
    }

    /// `dir` 直下を列挙する（`.git` は除外。gitignore 対象は**除外せず** `ignored=true` で薄字表示。
    /// VSCode 同様「無視ファイルも見えるが淡い」＝ git 管理有無が一目で分かる）。ディレクトリ優先→名前順。
    pub fn read_dir(&self, dir: &Path) -> Result<Vec<Entry>> {
        read_dir_on(self.host.as_ref(), &self.ignore.borrow(), dir)
    }
}

/// [`Worktree::read_dir`] の本体（`Worktree` を持たない背景スレッドから呼べる形）。
pub fn read_dir_on(host: &dyn Host, ignore: &Gitignore, dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dir_entry in host.read_dir(dir)? {
        let name = dir_entry.name;
        if name == ".git" {
            continue;
        }
        let path = dir_entry.path;
        let is_dir = dir_entry.is_dir;
        let ignored = ignore.matched(&path, is_dir).is_ignore();
        entries.push(Entry {
            path,
            name,
            is_dir,
            ignored,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// [`Worktree::read_any_dir`] の本体（`Worktree` を持たない背景スレッドから呼べる形）。
pub fn read_any_dir_on(
    host: &dyn Host,
    root: &Path,
    ignore: &Gitignore,
    dir: &Path,
) -> Result<Vec<Entry>> {
    if dir.starts_with(root) {
        return read_dir_on(host, ignore, dir);
    }
    let mut entries = Vec::new();
    for dir_entry in host.read_dir(dir)? {
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
    sort_entries(&mut entries);
    Ok(entries)
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| natural_name_cmp(&a.name, &b.name))
    });
}

/// 名前の自然順（D16）。数字の並びは数として比べ（`file2` < `file10`・`9` < `99` < `100`）、
/// 文字は大文字小文字を無視して比べる。数が同じなら先頭の 0 が少ない方を先に、最後は元の綴りで
/// 決める（読み直すたびに並びが揺れないよう全順序にする）。
pub fn natural_name_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let (mut left_rest, mut right_rest) = (left, right);
    while let (Some(left_char), Some(right_char)) =
        (left_rest.chars().next(), right_rest.chars().next())
    {
        if left_char.is_ascii_digit() && right_char.is_ascii_digit() {
            let left_end = digit_run_end(left_rest);
            let right_end = digit_run_end(right_rest);
            let ordering = compare_digit_runs(&left_rest[..left_end], &right_rest[..right_end]);
            if ordering.is_ne() {
                return ordering;
            }
            left_rest = &left_rest[left_end..];
            right_rest = &right_rest[right_end..];
            continue;
        }
        let ordering = left_char.to_lowercase().cmp(right_char.to_lowercase());
        if ordering.is_ne() {
            return ordering;
        }
        left_rest = &left_rest[left_char.len_utf8()..];
        right_rest = &right_rest[right_char.len_utf8()..];
    }
    // 片方が尽きた = 短い方が先。両方尽きたら元の綴りで決める。
    left_rest
        .len()
        .cmp(&right_rest.len())
        .then_with(|| left.cmp(right))
}

/// 先頭から続く ASCII 数字の終わり（byte 位置）。
fn digit_run_end(text: &str) -> usize {
    text.find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len())
}

/// 数字の並び同士を数として比べる（桁数に上限なし＝u64 に収まらない並びでも比べられる）。
fn compare_digit_runs(left: &str, right: &str) -> std::cmp::Ordering {
    let left_digits = left.trim_start_matches('0');
    let right_digits = right.trim_start_matches('0');
    left_digits
        .len()
        .cmp(&right_digits.len())
        .then_with(|| left_digits.cmp(right_digits))
        .then_with(|| left.len().cmp(&right.len()))
}

/// ディレクトリ → 直下の一覧。エクスプローラが描画に使うキャッシュの形。
pub type DirListings = std::collections::HashMap<PathBuf, Vec<Entry>>;

/// 複数ディレクトリをまとめて列挙する（エクスプローラのツリー再構築用・背景スレッド向け）。
///
/// 個々のディレクトリの失敗（消えた・権限）は空の一覧にする（従来の表示と同じ）。
/// **root が読めないときだけ `Err`** — それは接続が落ちている印で、その結果でツリーを
/// 空に塗り替えるより手元の表示を残す方がよい。
pub fn read_listings_on(
    host: &dyn Host,
    root: &Path,
    ignore: &Gitignore,
    directories: &[PathBuf],
) -> Result<DirListings> {
    let mut listings = DirListings::with_capacity(directories.len());
    for directory in directories {
        match read_any_dir_on(host, root, ignore, directory) {
            Ok(entries) => {
                listings.insert(directory.clone(), entries);
            }
            Err(error) if directory == root => {
                return Err(error)
                    .with_context(|| format!("root を列挙できない: {}", root.display()));
            }
            Err(_unreadable) => {
                listings.insert(directory.clone(), Vec::new());
            }
        }
    }
    Ok(listings)
}

impl Worktree {
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
// 公開 Git CLI と imara-diff（純 Rust）を使う独立実装。
// 設計根拠は docs/research/git-terminal-lsp-design-notes.md。

/// ファイルの git 状態（ツリー/タブの色分け用）。色は識別に集約（UI-SPEC §1.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Added,
    Modified,
    Deleted,
    Untracked,
    Conflicted,
}

/// git を呼ぶ経路。**フックを走らせてよいか**をここで分ける（呼び出し側は関数名で選ぶ）。
///
/// 未信頼 repo の repo-local `.git/config` 経由のコード実行を封じる（Zed GHSA-fj2r 同型）。
/// フォルダを開くだけで `git status` が自動実行される（レールの git 色）ため、`core.fsmonitor`
/// による drive-by RCE が要点。ext:: リモート transport は**どちらの経路でも**封じる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitTrigger {
    /// necoder が自分で走らせる git（status / diff / log / blame / worktree 作成 / fetch…）。
    /// 開いただけの repo でも走るので、フック（`core.hooksPath`）も含めて全部封じる。[`run_git`]。
    Automatic,
    /// 利用者がボタンで明示したコミット / push。フック（pre-commit / commit-msg / pre-push）は
    /// その repo の持ち主が置いた検査なので、ターミナルの `git commit` と同じく走らせる。
    /// [`run_git_user_action`]。
    UserAction,
}

/// `git` に渡す引数の全体（`-c` の防御 + subcommand 以降）。git レベルのオプションは
/// subcommand より前に置く必要があるので先頭へ差す。
fn git_command_args<I, S>(trigger: GitTrigger, args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut command: Vec<String> = vec!["-c".into(), "core.fsmonitor=false".into()];
    if trigger == GitTrigger::Automatic {
        command.extend(["-c".into(), "core.hooksPath=/dev/null".into()]);
    }
    command.extend(["-c".into(), "protocol.ext.allow=never".into()]);
    command.extend(args.into_iter().map(Into::into));
    command
}

fn run_git_as<I, S>(
    host: &dyn Host,
    dir: &Path,
    trigger: GitTrigger,
    args: I,
    env: &[(&str, &str)],
) -> Result<CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut spec = CommandSpec::new("git", dir).args(git_command_args(trigger, args));
    for (key, value) in env {
        spec.env.insert((*key).to_string(), (*value).to_string());
    }
    host.run_command(&spec)
}

/// necoder が自分で走らせる git（[`GitTrigger::Automatic`]・フックは走らない）。
fn run_git<I, S>(host: &dyn Host, dir: &Path, args: I) -> Result<CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    run_git_as(host, dir, GitTrigger::Automatic, args, &[])
}

/// 利用者が明示したコミット / push（[`GitTrigger::UserAction`]・フックを走らせる）。
fn run_git_user_action<I, S>(host: &dyn Host, dir: &Path, args: I) -> Result<CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    run_git_as(host, dir, GitTrigger::UserAction, args, &[])
}

/// [`run_git`] の env 付き版。ネットワークを触る git（fetch 等）で
/// `GIT_TERMINAL_PROMPT=0` を差すために使う（credential 対話で永久に固まるのを防ぐ）。
fn run_git_with_env<I, S>(
    host: &dyn Host,
    dir: &Path,
    args: I,
    env: &[(&str, &str)],
) -> Result<CommandOutput>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    run_git_as(host, dir, GitTrigger::Automatic, args, env)
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

/// `root` が linked worktree（`git worktree add` で作った作業ツリー）か。メインの作業ツリーは
/// git dir と共通の git dir が同じ、linked は別（`.git/worktrees/<name>`）。repo 外は `false`。
pub fn is_linked_worktree_on(host: &dyn Host, root: &Path) -> bool {
    let Some(common) = git_common_dir_on(host, root) else {
        return false;
    };
    let Ok(output) = run_git(
        host,
        root,
        ["rev-parse", "--path-format=absolute", "--git-dir"],
    ) else {
        return false;
    };
    if !output.success() {
        return false;
    }
    let git_dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let canonical = |path: &Path| paths::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    !git_dir.as_os_str().is_empty() && canonical(&git_dir) != canonical(&common)
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

/// [`sync_current_branch_on`] の結果。呼び出し側（+ Task）は FastForwarded の時だけ toast する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchSyncOutcome {
    /// upstream へ早送りした（取り込んだコミット数付き）。
    FastForwarded { branch: String, commits: u64 },
    /// fetch はしたが既に最新だった。
    UpToDate,
    /// 同期の対象外または安全に進められない状態（理由はログ/デバッグ用の固定文字列）:
    /// detached / upstream 無し / fetch 失敗（オフライン含む）/ dirty / diverged。
    Skipped(&'static str),
}

/// worktree を切る前の「土台の鮮度」対策（2026-08-30・Orca の default branch 自動同期を参考）:
/// checked-out ブランチを upstream へ fast-forward する。①fetch ②clean 確認 ③ff-only の順で、
/// **どの段階で無理でも黙って諦める**（オフラインや fork 状態で worktree 作成を止めない）。
/// merge/rebase を勝手にしない＝早送りだけ。dirty なら fetch 止まり（作業ツリーに触らない）。
pub fn sync_current_branch_on(host: &dyn Host, dir: &Path) -> BranchSyncOutcome {
    // detached HEAD（rebase 中・タグ checkout 等）は対象外。
    let branch = match run_git(host, dir, ["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(output) if output.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => return BranchSyncOutcome::Skipped("head"),
    };
    if branch.is_empty() || branch == "HEAD" {
        return BranchSyncOutcome::Skipped("detached");
    }
    // fetch（credential 対話で固まらないよう GIT_TERMINAL_PROMPT=0）。remote 未設定もここで落ちる。
    let fetched = run_git_with_env(
        host,
        dir,
        ["fetch", "--quiet"],
        &[("GIT_TERMINAL_PROMPT", "0")],
    );
    match fetched {
        Ok(output) if output.success() => {}
        _ => return BranchSyncOutcome::Skipped("fetch"),
    }
    // upstream 有無 + ahead/behind + dirty を 1 呼び出しで（`## main...origin/main [behind 2]`）。
    let status_output = match run_git(host, dir, ["status", "--porcelain", "--branch"]) {
        Ok(output) if output.success() => String::from_utf8_lossy(&output.stdout).to_string(),
        _ => return BranchSyncOutcome::Skipped("status"),
    };
    if !status_output
        .lines()
        .next()
        .is_some_and(|first| first.contains("..."))
    {
        return BranchSyncOutcome::Skipped("no-upstream");
    }
    let status = parse_status_branch(&status_output);
    if status.dirty {
        return BranchSyncOutcome::Skipped("dirty");
    }
    if status.behind == 0 {
        return BranchSyncOutcome::UpToDate;
    }
    if status.ahead > 0 {
        return BranchSyncOutcome::Skipped("diverged");
    }
    let merged = run_git(host, dir, ["merge", "--ff-only", "--quiet", "@{upstream}"]);
    match merged {
        Ok(output) if output.success() => BranchSyncOutcome::FastForwarded {
            branch,
            commits: status.behind as u64,
        },
        _ => BranchSyncOutcome::Skipped("ff"),
    }
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

/// 失敗した git / gh の出力（[`git_fail_message`]）を運ぶエラー。見出し（「コミットに失敗」等）は
/// `context` で重ねるので、`{error:#}` は従来どおり「見出し: 出力」になる。
/// UI は [`failure_output`] で出力だけを取り出し、自分の言葉の見出しに添える
/// （この crate は i18n を知らない）。フックで止まったコミットでは、ここにフックの出力が入る。
#[derive(Debug)]
pub struct CommandFailed {
    pub output: String,
}

impl std::fmt::Display for CommandFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.output)
    }
}

impl std::error::Error for CommandFailed {}

/// 成功でなければ、出力を [`CommandFailed`] に包んで見出しを重ねたエラーにする。
fn ensure_command_success(output: &CommandOutput, headline: &str) -> Result<()> {
    if output.success() {
        return Ok(());
    }
    Err(anyhow::Error::new(CommandFailed {
        output: git_fail_message(output),
    })
    .context(headline.to_string()))
}

/// エラーの「理由」（UI の見出しに添える部分）。git / gh が失敗したならその出力、
/// 起動できなかった等ならエラーの連鎖全体（`{error:#}`）。
pub fn failure_output(error: &anyhow::Error) -> String {
    match error.downcast_ref::<CommandFailed>() {
        Some(failed) => failed.output.clone(),
        None => format!("{error:#}"),
    }
}

/// 変更を index に上げる（`git add -- <path>`）。
pub fn stage_path(dir: &Path, path: &Path) -> Result<()> {
    stage_path_on(&LocalHost, dir, path)
}

pub fn stage_path_on(host: &dyn Host, dir: &Path, path: &Path) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let output =
        run_git(host, dir, ["add", "--", path.as_str()]).context("git add の実行に失敗")?;
    ensure_command_success(&output, "stage に失敗")
}

/// 全変更を index に上げる（`git add -A`）。
pub fn stage_all(dir: &Path) -> Result<()> {
    stage_all_on(&LocalHost, dir)
}

pub fn stage_all_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_git(host, dir, ["add", "-A"]).context("git add -A の実行に失敗")?;
    ensure_command_success(&output, "stage に失敗")
}

/// エクスプローラの「変更を破棄」（D16）の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardOutcome {
    /// HEAD の内容へ戻した（index も作業ツリーも）。
    Restored,
    /// git の管理外になった（未追跡・HEAD に無いまま add しただけのファイル）。git では中身を
    /// 戻す先が無いので、**ファイルを片付けるのは呼び出し側**（ゴミ箱へ入れる＝取り消せる形で）。
    Untracked,
}

/// 1 ファイルの変更を破棄して HEAD の状態へ戻す（`git restore --source=HEAD --staged --worktree`）。
/// HEAD に無いファイル（未追跡・add しただけ）は index から外すだけで [`DiscardOutcome::Untracked`]
/// を返す。競合中のファイルは破棄しない（どちらの内容を残すかは人が決める）。
pub fn discard_path_on(host: &dyn Host, dir: &Path, path: &Path) -> Result<DiscardOutcome> {
    let path_arg = path.to_string_lossy().into_owned();
    let output = run_git(
        host,
        dir,
        [
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
            "--",
            path_arg.as_str(),
        ],
    )
    .context("git status の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "変更を読めない: {}",
        git_fail_message(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(entry) = stdout
        .split('\0')
        .find(|entry| entry.len() >= 4 && &entry[2..3] == " ")
    else {
        anyhow::bail!("変更が無い: {}", path.display());
    };
    let (index, worktree) = (entry.as_bytes()[0], entry.as_bytes()[1]);
    match classify_status(index, worktree) {
        StatusKind::Untracked => return Ok(DiscardOutcome::Untracked),
        StatusKind::Conflicted => {
            anyhow::bail!("競合中のファイルは破棄しない: {}", path.display())
        }
        _ => {}
    }
    if index == b'A' {
        let output = run_git(
            host,
            dir,
            ["rm", "--cached", "--quiet", "--", path_arg.as_str()],
        )
        .context("git rm --cached の実行に失敗")?;
        anyhow::ensure!(
            output.success(),
            "index から外せない: {}",
            git_fail_message(&output)
        );
        return Ok(DiscardOutcome::Untracked);
    }
    let output = run_git(
        host,
        dir,
        [
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            path_arg.as_str(),
        ],
    )
    .context("git restore の実行に失敗")?;
    anyhow::ensure!(
        output.success(),
        "変更を破棄できない: {}",
        git_fail_message(&output)
    );
    Ok(DiscardOutcome::Restored)
}

/// index から下ろす（`git restore --staged -- <path>`）。
pub fn unstage_path(dir: &Path, path: &Path) -> Result<()> {
    unstage_path_on(&LocalHost, dir, path)
}

pub fn unstage_path_on(host: &dyn Host, dir: &Path, path: &Path) -> Result<()> {
    let path = path.to_string_lossy().into_owned();
    let output = run_git(host, dir, ["restore", "--staged", "--", path.as_str()])
        .context("git restore --staged の実行に失敗")?;
    ensure_command_success(&output, "unstage に失敗")
}

/// staged 変更をコミット（`git commit -m <message>`）。message 空・staged 無しは失敗。
/// 利用者の明示操作なので**フックを走らせる**（pre-commit / commit-msg が止めたらその出力が
/// [`CommandFailed`] に入る）。
pub fn commit(dir: &Path, message: &str) -> Result<()> {
    commit_on(&LocalHost, dir, message)
}

pub fn commit_on(host: &dyn Host, dir: &Path, message: &str) -> Result<()> {
    anyhow::ensure!(!message.trim().is_empty(), "コミットメッセージが空");
    let output = run_git_user_action(host, dir, ["commit", "-m", message])
        .context("git commit の実行に失敗")?;
    ensure_command_success(&output, "コミットに失敗")
}

/// 新しいブランチを作って切り替え（`git switch -c <name>`）。既存名なら失敗。
pub fn create_branch(dir: &Path, name: &str) -> Result<()> {
    create_branch_on(&LocalHost, dir, name)
}

pub fn create_branch_on(host: &dyn Host, dir: &Path, name: &str) -> Result<()> {
    anyhow::ensure!(!name.trim().is_empty(), "ブランチ名が空");
    let output =
        run_git(host, dir, ["switch", "-c", name]).context("git switch -c の実行に失敗")?;
    ensure_command_success(&output, "ブランチ作成に失敗")
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
/// 利用者の明示操作なので**フックを走らせる**（pre-push が止めたらその出力が [`CommandFailed`] に入る）。
pub fn push(dir: &Path) -> Result<()> {
    push_on(&LocalHost, dir)
}

pub fn push_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_git_user_action(host, dir, ["push"]).context("git push の実行に失敗")?;
    if output.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("has no upstream") || stderr.contains("--set-upstream") {
        let branch = git_current_branch_on(host, dir)
            .context("push: 現在ブランチが取得できない（detached HEAD?）")?;
        let retry = run_git_user_action(
            host,
            dir,
            ["push", "--set-upstream", "origin", branch.as_str()],
        )
        .context("git push --set-upstream の実行に失敗")?;
        return ensure_command_success(&retry, "push に失敗");
    }
    ensure_command_success(&output, "push に失敗")
}

/// upstream から pull（`git pull --ff-only`）。fast-forward できなければ失敗（安全側・merge しない）。
pub fn pull(dir: &Path) -> Result<()> {
    pull_on(&LocalHost, dir)
}

pub fn pull_on(host: &dyn Host, dir: &Path) -> Result<()> {
    let output = run_git(host, dir, ["pull", "--ff-only"]).context("git pull の実行に失敗")?;
    ensure_command_success(&output, "pull に失敗")
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
    ensure_command_success(&output, "PR 作成に失敗")
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
    ensure_command_success(&output, "リポジトリを開けません")
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
    // スクリプト本体が POSIX 構文（パイプ・`$?`・`rm -f`）なので、シェルを差し替えるだけでは動かない。
    // Windows ローカルでは明示的に断る（WINDOWS-PORT.md §D3）。
    anyhow::ensure!(
        host.has_posix_shell(),
        "この機能は POSIX シェル前提のため Windows ではまだ使えません"
    );
    let output = host
        .run_command(&host.shell_script(script.as_str(), dir))
        .context("コミットメッセージ生成の実行に失敗（claude CLI 未導入？）")?;
    ensure_command_success(&output, "生成に失敗")?;
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
    let temp = PathBuf::from(format!("/tmp/necoder-inline-{unix_ms}.txt"));
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
    // スクリプト本体が POSIX 構文（パイプ・`$?`・`rm -f`）なので、シェルを差し替えるだけでは動かない。
    // Windows ローカルでは明示的に断る（WINDOWS-PORT.md §D3）。
    anyhow::ensure!(
        host.has_posix_shell(),
        "この機能は POSIX シェル前提のため Windows ではまだ使えません"
    );
    let output = host
        .run_command(&host.shell_script(script.as_str(), dir))
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
    let temp = PathBuf::from(format!("/tmp/necoder-inline-cmd-{unix_ms}.txt"));
    host.write_file(&temp, payload.as_bytes(), host::WriteCondition::Any)
        .context("コマンド生成の一時ファイル作成に失敗")?;
    // 引用符 / $ / バッククォートを含めない（sh -c の二重引用符に素で埋めるため）。
    let prompt = "入力のやりたいことを実現するシェルコマンドを1行だけ出力して。\
        対象は macOS の zsh。説明・前置き・コードフェンスは出力しない。";
    let script = format!(
        "claude -p \"{prompt}\" < {temp}; status=$?; rm -f {temp}; exit $status",
        temp = temp.display()
    );
    // スクリプト本体が POSIX 構文（パイプ・`$?`・`rm -f`）なので、シェルを差し替えるだけでは動かない。
    // Windows ローカルでは明示的に断る（WINDOWS-PORT.md §D3）。
    anyhow::ensure!(
        host.has_posix_shell(),
        "この機能は POSIX シェル前提のため Windows ではまだ使えません"
    );
    let output = host
        .run_command(&host.shell_script(script.as_str(), dir))
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
    let temp = PathBuf::from(format!("/tmp/necoder-oneshot-{unix_ms}.txt"));
    let out = PathBuf::from(format!("/tmp/necoder-oneshot-{unix_ms}.out"));
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
    // スクリプト本体が POSIX 構文（パイプ・`$?`・`rm -f`）なので、シェルを差し替えるだけでは動かない。
    // Windows ローカルでは明示的に断る（WINDOWS-PORT.md §D3）。
    anyhow::ensure!(
        host.has_posix_shell(),
        "この機能は POSIX シェル前提のため Windows ではまだ使えません"
    );
    let output = host
        .run_command(&host.shell_script(script.as_str(), dir))
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
    blob_at_on(host, dir, "HEAD", Path::new(name))
}

/// `rev` 時点の `dir/<relative>` のテキスト（`git show <rev>:./<relative>`・cwd 相対）。
/// その rev に無い・repo 外・`rev` がオプションに見える（`-` 始まり）なら `None`。
fn blob_at_on(host: &dyn Host, dir: &Path, rev: &str, relative: &Path) -> Option<String> {
    if rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let spec = format!("{rev}:./{}", relative_display(relative));
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
/// 表示・絞り込み用の相対パス文字列。**Windows でも `/` 区切りに揃える**。
///
/// この文字列は ⌘P の絞り込み・タブの表示・検索結果の見出しに出る。ユーザーは `/` で打つし、
/// mac / Linux と同じ文字列になるので設定やスナップショットも揃う（VSCode と同じ流儀）。
/// `PathBuf::from("src/main.rs")` は Windows でも正しく解決されるため、戻す側の心配も要らない。
fn relative_display(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

pub fn all_files_on(host: &dyn Host, root: &Path, limit: usize) -> Vec<(PathBuf, String)> {
    let mut files = host
        .list_files(root, limit)
        .unwrap_or_default()
        .into_iter()
        .map(|path| {
            let relative = relative_display(path.strip_prefix(root).unwrap_or(&path));
            (path, relative)
        })
        .collect::<Vec<_>>();
    // ツリーと同じ自然順（`item2` < `item10`・D16）。⌘P の空クエリと同点の並びがこれになる。
    files.sort_by(|a, b| natural_name_cmp(&a.1, &b.1));
    files
}

/// ⌘P の 2 回目（D19）: [`all_files_on`] が返さなかったファイル（主に gitignore で隠れたもの）を、
/// **浅いものから順に**（幅優先）最大 `limit` 件集める。`listed` = 1 回目で出したパス（除く）。
///
/// `node_modules` や `target` のような巨大な無視フォルダの奥まで同じ重さで辿ると、`.env` や
/// `build/` 直下のような「たまに開きたいもの」へ届く前に上限を使い切るので、深さ順にする。
/// `.git` とシンボリックリンクは辿らない（循環を避ける）。local のみ。
pub fn ignored_files_local(
    root: &Path,
    listed: &std::collections::HashSet<PathBuf>,
    limit: usize,
) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(directory) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue; // 読めないフォルダ（権限など）は飛ばす
        };
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            natural_name_cmp(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for entry in entries {
            if entry.file_name() == ".git" {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                queue.push_back(path);
            } else if kind.is_file() && !listed.contains(&path) {
                let relative = relative_display(path.strip_prefix(root).unwrap_or(&path));
                found.push((path, relative));
                if found.len() >= limit {
                    return found;
                }
            }
        }
    }
    found
}

/// HEAD のファイル内容（テキスト）。無ければ None（新規ファイル等）。M11 diff タブ/hunk 操作用。
pub fn head_text_on(host: &dyn Host, file: &Path) -> Option<String> {
    let (dir, name) = (file.parent()?, file.file_name()?);
    head_blob_on(host, dir, name)
}

/// `file`（絶対パス）の `rev` 時点のテキスト（`git show <rev>:<path>` 相当）。その rev に無い・
/// repo 外・読めないなら `None`。親フォルダごと消えていても、残っている祖先から引く
/// （削除されたファイルの diff を出すため）。
pub fn rev_text_on(host: &dyn Host, file: &Path, rev: &str) -> Option<String> {
    let mut anchor = file.parent()?;
    while !host.metadata(anchor).is_ok_and(|metadata| metadata.is_dir) {
        anchor = anchor.parent()?;
    }
    let relative = file.strip_prefix(anchor).ok()?;
    blob_at_on(host, anchor, rev, relative)
}

/// diff タブの比較相手（左側）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffBase {
    /// HEAD（ソース管理パネルの行・開いているファイルの diff）。
    Head,
    /// 任意の commit。Fleet の Task は worktree を切った時点の commit（`TaskSpace.base_oid`）と比べる
    /// ＝エージェントがコミット済みの変更も出る。
    Commit(String),
}

impl DiffBase {
    /// git に渡す rev。
    pub fn rev(&self) -> &str {
        match self {
            DiffBase::Head => "HEAD",
            DiffBase::Commit(oid) => oid,
        }
    }

    /// 見出し・タブ題名に出す比較相手（`HEAD` / 短い SHA）。
    pub fn label(&self) -> String {
        match self {
            DiffBase::Head => "HEAD".to_string(),
            DiffBase::Commit(oid) => oid.chars().take(7).collect(),
        }
    }
}

/// 1 ファイルを base と作業ツリー（またはバッファ）で比べた結果（diff タブの中身の元）。
/// 本文は `@@` 行から始まる unified diff。見出し行（`---` / `+++`）は UI が自分の言葉で付ける
/// （この crate は i18n を知らない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDiff {
    /// 両方に在って中身が違う。
    Modified(String),
    /// base に無く、今は在る（全行が追加）。
    Added(String),
    /// base に在って、作業ツリーから消えた（全行が削除）。
    Deleted(String),
    /// base と同じ（改行コードの違いだけも同じ扱い）。
    Unchanged,
    /// base にも作業ツリーにも無い。
    Missing,
}

/// `file` を `base` 時点と比べる。`current` は今の中身で、`None` は「作業ツリーから消えている」。
pub fn diff_file_on(
    host: &dyn Host,
    file: &Path,
    base: &DiffBase,
    current: Option<&str>,
) -> FileDiff {
    match (rev_text_on(host, file, base.rev()), current) {
        (None, None) => FileDiff::Missing,
        (Some(old), None) => FileDiff::Deleted(unified_diff_body(&old, "").unwrap_or_default()),
        (None, Some(new)) => FileDiff::Added(unified_diff_body("", new).unwrap_or_default()),
        (Some(old), Some(new)) => match unified_diff_body(&old, new) {
            Some(body) => FileDiff::Modified(body),
            None => FileDiff::Unchanged,
        },
    }
}

/// テキスト同士の unified diff の本文（`@@` hunk 列・見出し行なし）。差分なしは None。
/// 改行コード（CRLF / LF）の違いだけでは差分にしない。
pub fn unified_diff_body(old_text: &str, new_text: &str) -> Option<String> {
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
    // imara-diff 0.1.8 の `UnifiedDiffBuilder` は各行の後ろに `\n` を足すが、`lines_with_terminator`
    // の行は改行を含むので、diff タブで 1 行おきに空行が挟まっていた。末尾の改行だけの変更も
    // 差分として拾いたいので入力は改行込みのまま、出力の二重改行だけを畳む（行は途中に改行を含まない）。
    let body = body.replace("\n\n", "\n");
    (!body.is_empty()).then_some(body)
}

/// 任意テキスト同士の unified diff（エージェント承認カードの「エディタで開く」・M12-6）。
/// 差分なしは None。
pub fn unified_diff_texts(old_text: &str, new_text: &str, name: &str) -> Option<String> {
    let body = unified_diff_body(old_text, new_text)?;
    Some(format!(
        "--- a/{name}（現在）\n+++ b/{name}（提案）\n{body}"
    ))
}

/// 任意テキスト同士の unified diff を、見出し（`---` / `+++` の行）を指定して作る
/// （`ne --diff <a> <b>` の 2 ファイル比較）。差分なしは None。
pub fn unified_diff_labeled(
    old_text: &str,
    new_text: &str,
    old_label: &str,
    new_label: &str,
) -> Option<String> {
    let body = unified_diff_body(old_text, new_text)?;
    Some(format!("--- {old_label}\n+++ {new_label}\n{body}"))
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
    let temp = repo_root.join(".git/necoder-hunk.patch");
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

/// [`blame_line_on`] の 1 行分。「未コミット」の文言は UI 側で i18n する
/// （この crate は依存方向の規律で i18n を知らない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlameLine {
    /// まだどのコミットにも属さない行。
    Uncommitted,
    /// "作者, YYYY-MM-DD • 要旨" の合成済みテキスト。
    Commit(String),
}

/// 1 行の blame（作者・日付・要旨・M11-11）。`line` は 1 始まり。
/// dirty バッファでは行ずれの近似になる（HEAD 基準）。失敗は None（表示しない）。
pub fn blame_line_on(host: &dyn Host, file: &Path, line: u32) -> Option<BlameLine> {
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
        return Some(BlameLine::Uncommitted);
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
    Some(BlameLine::Commit(format!("{author}, {date} • {summary}")))
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

/// 外部（Finder D&D 等）から `target_dir` の中へコピーする（ファイル/フォルダ再帰・
/// 同名が既に居ればエラー＝上書きしない）。元は動かさない（移動ではなくコピー＝安全側）。
pub fn copy_into_local(source: &Path, target_dir: &Path) -> Result<PathBuf> {
    let name = source
        .file_name()
        .with_context(|| format!("名前が取れない: {}", source.display()))?;
    let destination = target_dir.join(name);
    anyhow::ensure!(
        !destination.exists(),
        "既に存在する: {}",
        destination.display()
    );
    if source.is_dir() {
        copy_dir_recursive(source, &destination)?;
    } else {
        std::fs::copy(source, &destination)
            .with_context(|| format!("コピーに失敗: {}", source.display()))?;
    }
    Ok(destination)
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
    move_to_trash_local(path).map(|_location| ())
}

/// [`trash_local`] と同じくゴミ箱へ入れ、**ゴミ箱の中での場所**を返す（⌘Z で戻すため・H30）。
///
/// macOS 15 からの `/usr/bin/trash` は `NSFileManager trashItemAtURL:resultingItemURL:` を呼び、
/// `-v` を付けると標準出力へ `# Moved "<元>" to "<ゴミ箱の中>"` を書く（名前が重なると
/// ゴミ箱側で別名になるので、場所は出力から読むしかない）。Finder 経由（それより古い macOS）は
/// `delete` が返す項目から読む。どちらも読めなければ `None`（ゴミ箱には入ったが戻せない）。
pub fn move_to_trash_local(path: &Path) -> Result<Option<PathBuf>> {
    anyhow::ensure!(
        path.symlink_metadata().is_ok(),
        "存在しない: {}",
        path.display()
    );
    if Path::new("/usr/bin/trash").exists() {
        let output = std::process::Command::new("/usr/bin/trash")
            .arg("-v")
            .arg(path)
            .output()
            .context("trash コマンドの起動に失敗")?;
        anyhow::ensure!(output.status.success(), "trash が失敗: {}", path.display());
        let printed = String::from_utf8_lossy(&output.stdout);
        return Ok(parse_trash_destination(&printed, path)
            .filter(|location| location.symlink_metadata().is_ok()));
    }
    // フォールバック: Finder に頼む（AppleScript）。戻り値（ゴミ箱の中の項目）のパスを出力させる。
    // パスの取り出しに失敗しても、ゴミ箱へ入れること自体は成功として扱う（`try` で包む）。
    let quoted = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let output = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            &format!(
                "tell application \"Finder\" to set trashedItem to delete POSIX file \"{quoted}\""
            ),
            "-e",
            "try",
            "-e",
            "return POSIX path of (trashedItem as alias)",
            "-e",
            "on error",
            "-e",
            "return \"\"",
            "-e",
            "end try",
        ])
        .output()
        .context("osascript の起動に失敗")?;
    anyhow::ensure!(
        output.status.success(),
        "Finder でのゴミ箱移動が失敗: {}",
        path.display()
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    let location = printed.trim().trim_end_matches('/');
    Ok((!location.is_empty())
        .then(|| PathBuf::from(location))
        .filter(|location| location.symlink_metadata().is_ok()))
}

/// `/usr/bin/trash -v` の出力からゴミ箱の中での場所を読む。書式は `# Moved "<元>" to "<先>"`
/// （引用符はエスケープされない）。元のパスが分かっているので、その後ろを切り出す。
fn parse_trash_destination(printed: &str, original: &Path) -> Option<PathBuf> {
    let prefix = format!("# Moved \"{}\" to \"", original.display());
    printed.lines().find_map(|line| {
        let destination = match line.strip_prefix(prefix.as_str()) {
            Some(rest) => rest.strip_suffix('"')?,
            // 元の綴りが変わって出た場合（正規化など）は最後の `" to "` で切る。
            None => line
                .strip_prefix("# Moved \"")?
                .rsplit_once("\" to \"")?
                .1
                .strip_suffix('"')?,
        };
        (!destination.is_empty()).then(|| PathBuf::from(destination))
    })
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
            .name("necoder-remote-watch-pump".to_string())
            .spawn(move || {
                use std::sync::atomic::Ordering::Acquire;
                while !pump_stop.load(Acquire) {
                    match host_watch.recv_timeout(std::time::Duration::from_millis(500)) {
                        Ok(relative) => {
                            let absolute: Vec<PathBuf> =
                                relative.into_iter().map(|path| root.join(path)).collect();
                            if !absolute.is_empty() {
                                on_paths(absolute);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        // sender が drop された後は recv_timeout が待機せず即時に返る。
                        // 切断を無視するとここが 1 core を使い切る busy loop になる。
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
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

    /// diff タブの本文は 1 行 = 1 行（UnifiedDiffBuilder は行ごとに改行を足すので、改行つきの行を
    /// 渡すと全行の後ろに空行が挟まっていた）。承認カードの diff と `ne --diff` が同じ関数を通る。
    #[test]
    fn unified_diff_has_one_line_per_diff_line() {
        let diff = unified_diff_labeled("same\nold\n", "same\nnew\n", "a/left", "b/right")
            .expect("差分がある");
        assert_eq!(
            diff,
            "--- a/left\n+++ b/right\n@@ -1,2 +1,2 @@\n same\n-old\n+new\n"
        );
        // CRLF も同じ形になる（行末の \r を本文へ持ち込まない）。
        assert_eq!(
            unified_diff_labeled("same\r\nold\r\n", "same\r\nnew\r\n", "a/left", "b/right"),
            Some(diff)
        );
        assert_eq!(unified_diff_labeled("x\n", "x\n", "a", "b"), None);
    }

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
        let Some(line) = result else {
            return; // shallow clone 等では blame が引けないことがある
        };
        // "作者, YYYY-MM-DD • 要旨" か Uncommitted のどちらか。
        assert!(
            match &line {
                BlameLine::Uncommitted => true,
                BlameLine::Commit(text) => text.contains(" • ") && text.contains("-"),
            },
            "{line:?}"
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
        let dir = std::env::temp_dir().join(format!("necoder_hunk_{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("necoder_fileops_{}", std::process::id()));
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

        // 外部からのコピー（Finder D&D）: ファイル・フォルダ再帰・同名は拒否。
        let inbox = dir.join("inbox");
        create_dir_local(&inbox).unwrap();
        let copied = copy_into_local(&renamed, &inbox).unwrap();
        assert_eq!(copied, inbox.join("b.rs"));
        assert!(copied.is_file() && renamed.is_file()); // 元は残る（移動ではなくコピー）
        assert!(copy_into_local(&renamed, &inbox).is_err()); // 同名は上書きしない
        let copied_dir = copy_into_local(&folder, &inbox).unwrap();
        assert!(copied_dir.join("b.rs").is_file()); // フォルダは中身ごと

        let _ = std::fs::remove_dir_all(&dir);
    }
    use std::process::Command;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("necoder_project_{}_{}", tag, std::process::id()));
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
        assert!(worktree.name().starts_with("necoder_project_name_"));
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
    fn natural_order_compares_numbers_by_value() {
        let mut names = vec![
            "file10.rs",
            "file2.rs",
            "File1.rs",
            "100",
            "99",
            "9",
            "a",
            "a01",
            "a1",
            "b",
        ];
        names.sort_by(|left, right| natural_name_cmp(left, right));
        assert_eq!(
            names,
            vec![
                "9",
                "99",
                "100",
                "a",
                "a1",
                "a01",
                "b",
                "File1.rs",
                "file2.rs",
                "file10.rs"
            ]
        );
        // 大文字小文字だけが違う名前も順序が決まる（読み直しで揺れない）。
        assert_eq!(
            natural_name_cmp("README", "readme"),
            std::cmp::Ordering::Less
        );
        // u64 に収まらない桁でも比べられる。
        assert_eq!(
            natural_name_cmp("v99999999999999999999", "v100000000000000000000"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn tree_lists_folders_first_in_natural_order() {
        let root = scratch("natural");
        std::fs::create_dir_all(root.join("dir10")).unwrap();
        std::fs::create_dir_all(root.join("dir9")).unwrap();
        for name in ["file10.rs", "file2.rs", "file1.rs"] {
            std::fs::write(root.join(name), "").unwrap();
        }
        let worktree = Worktree::new(&root).unwrap();
        let names: Vec<String> = worktree
            .read_root()
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(
            names,
            vec!["dir9", "dir10", "file1.rs", "file2.rs", "file10.rs"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trash_output_parser_reads_the_destination() {
        let original = Path::new("/tmp/a \"quoted\" name.txt");
        let printed = "# Moved \"/tmp/a \"quoted\" name.txt\" to \"/Users/me/.Trash/a \"quoted\" name 2.txt\"\n";
        assert_eq!(
            parse_trash_destination(printed, original),
            Some(PathBuf::from("/Users/me/.Trash/a \"quoted\" name 2.txt"))
        );
        // 元の綴りが違って出ても最後の ` to ` で拾う。
        assert_eq!(
            parse_trash_destination(
                "# Moved \"/private/tmp/b.txt\" to \"/Users/me/.Trash/b.txt\"",
                Path::new("/tmp/b.txt")
            ),
            Some(PathBuf::from("/Users/me/.Trash/b.txt"))
        );
        assert_eq!(parse_trash_destination("", original), None);
    }

    #[test]
    fn discard_restores_tracked_files_and_hands_back_untracked_ones() {
        let root = scratch("discard");
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
        git(&["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-q", "-m", "init"]);

        // 変更（staged + unstaged の両方）→ HEAD へ戻る。
        std::fs::write(root.join("tracked.txt"), "two\n").unwrap();
        git(&["add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "three\n").unwrap();
        let tracked = root.join("tracked.txt");
        assert_eq!(
            discard_path_on(&LocalHost, &root, &tracked).unwrap(),
            DiscardOutcome::Restored
        );
        assert_eq!(std::fs::read_to_string(&tracked).unwrap(), "one\n");
        assert!(
            git_status(&root).is_empty(),
            "index も作業ツリーも HEAD と同じ"
        );

        // 未追跡 → 片付けは呼び出し側（ファイルはまだある）。
        let untracked = root.join("new.txt");
        std::fs::write(&untracked, "x\n").unwrap();
        assert_eq!(
            discard_path_on(&LocalHost, &root, &untracked).unwrap(),
            DiscardOutcome::Untracked
        );
        assert!(untracked.exists());

        // add しただけ（HEAD に無い）→ index から外れて未追跡に戻る。
        git(&["add", "new.txt"]);
        assert_eq!(
            discard_path_on(&LocalHost, &root, &untracked).unwrap(),
            DiscardOutcome::Untracked
        );
        let status: std::collections::HashMap<PathBuf, StatusKind> =
            git_status(&root).into_iter().collect();
        let canonical = paths::canonicalize(&untracked).unwrap();
        assert_eq!(status.get(&canonical), Some(&StatusKind::Untracked));

        // 変更の無いファイルは断る。
        assert!(discard_path_on(&LocalHost, &root, &tracked).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⌘P の 2 回目: 1 回目（gitignore 準拠）に出なかったファイルだけが、浅い順に出る。
    #[test]
    fn ignored_files_appear_only_in_the_second_pass() {
        let root = scratch("ignored-pass");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("build/deep/deeper")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join(".env"), "").unwrap();
        std::fs::write(root.join("app.log"), "").unwrap();
        std::fs::write(root.join("build/deep/deeper/out.js"), "").unwrap();
        std::fs::write(root.join(".git/HEAD"), "").unwrap();
        std::fs::write(root.join(".gitignore"), ".env\n*.log\nbuild/\n").unwrap();

        let first = all_files_on(&LocalHost, &root, 1000);
        let first_relatives: Vec<&str> = first
            .iter()
            .map(|(_, relative)| relative.as_str())
            .collect();
        assert!(first_relatives.contains(&"src/main.rs"));
        for ignored in [".env", "app.log", "build/deep/deeper/out.js"] {
            assert!(
                !first_relatives.contains(&ignored),
                "1 回目に無視ファイル {ignored} を出さない"
            );
        }

        let listed: std::collections::HashSet<PathBuf> =
            first.into_iter().map(|(path, _)| path).collect();
        let second: Vec<String> = ignored_files_local(&root, &listed, 1000)
            .into_iter()
            .map(|(_, relative)| relative)
            .collect();
        assert_eq!(
            second,
            vec![".env", "app.log", "build/deep/deeper/out.js"],
            "無視ファイルだけ・浅い順（.git は辿らない）"
        );
        assert_eq!(ignored_files_local(&root, &listed, 1).len(), 1, "上限");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_directory_errors() {
        assert!(Worktree::new("/no/such/dir/necoder-xyz").is_err());
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

    /// [`sync_current_branch_on`] の実挙動: ローカル bare を upstream に見立て、
    /// ①behind → 早送り ②最新 → UpToDate ③dirty → スキップ（fetch 止まり）を固定する。
    /// ネットワーク不要（file:// 相当のローカル remote）。
    #[test]
    fn sync_current_branch_fast_forwards_and_skips_safely() {
        let base = scratch("branch-sync");
        let origin = base.join("origin.git");
        let upstream_work = base.join("upstream-work");
        let clone = base.join("clone");
        std::fs::create_dir_all(&base).unwrap();
        let git_in = |dir: &Path, args: &[&str]| {
            Command::new("git")
                .current_dir(dir)
                .args(args)
                .output()
                .expect("git 実行")
        };
        // git が無い環境ではスキップ（CI 等）。bare にも `-b main` — 既定ブランチが master の
        // 環境（CI は init.defaultBranch 未設定）だと HEAD が存在しない master を指し、
        // clone が unborn HEAD になって Skipped("head") に化ける。
        if !git_in(&base, &["init", "-q", "-b", "main", "--bare", "origin.git"])
            .status
            .success()
        {
            return;
        }
        // upstream 側の作業 repo → origin へ 1 コミット push。
        git_in(&base, &["init", "-q", "-b", "main", "upstream-work"]);
        let config = |dir: &Path| {
            git_in(dir, &["config", "user.email", "t@example.com"]);
            git_in(dir, &["config", "user.name", "tester"]);
        };
        config(&upstream_work);
        std::fs::write(upstream_work.join("a.txt"), "one\n").unwrap();
        git_in(&upstream_work, &["add", "a.txt"]);
        git_in(&upstream_work, &["commit", "-q", "-m", "c1"]);
        git_in(
            &upstream_work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&upstream_work, &["push", "-q", "-u", "origin", "main"]);
        // clone（同期対象）。
        git_in(
            &base,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        config(&clone);
        let host = LocalHost;
        // ①最新なら UpToDate。
        assert_eq!(
            sync_current_branch_on(&host, &clone),
            BranchSyncOutcome::UpToDate
        );
        // upstream 側で 2 コミット進める → clone は behind 2。
        std::fs::write(upstream_work.join("a.txt"), "two\n").unwrap();
        git_in(&upstream_work, &["commit", "-q", "-am", "c2"]);
        std::fs::write(upstream_work.join("a.txt"), "three\n").unwrap();
        git_in(&upstream_work, &["commit", "-q", "-am", "c3"]);
        git_in(&upstream_work, &["push", "-q"]);
        // ②behind → 早送り 2 コミット。作業ツリーも追従している。
        assert_eq!(
            sync_current_branch_on(&host, &clone),
            BranchSyncOutcome::FastForwarded {
                branch: "main".to_string(),
                commits: 2
            }
        );
        assert_eq!(
            std::fs::read_to_string(clone.join("a.txt")).unwrap(),
            "three\n"
        );
        // ③dirty なら作業ツリーに触らない（スキップ）。
        std::fs::write(upstream_work.join("a.txt"), "four\n").unwrap();
        git_in(&upstream_work, &["commit", "-q", "-am", "c4"]);
        git_in(&upstream_work, &["push", "-q"]);
        std::fs::write(clone.join("a.txt"), "local edit\n").unwrap();
        assert_eq!(
            sync_current_branch_on(&host, &clone),
            BranchSyncOutcome::Skipped("dirty")
        );
        assert_eq!(
            std::fs::read_to_string(clone.join("a.txt")).unwrap(),
            "local edit\n"
        );
        let _ = std::fs::remove_dir_all(&base);
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
        // 改行変換を切る。Windows の Git は既定で core.autocrlf=true（インストーラ既定）なので、
        // 切らないと checkout したファイルが CRLF になり「書いた内容と読んだ内容が違う」で落ちる。
        // このテストが見たいのは merge/status であって改行ではない（WINDOWS-PORT.md §4）。
        git(&["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        // 追跡ファイルを変更 + 未追跡ファイルを追加
        std::fs::write(root.join("tracked.txt"), "two\n").unwrap();
        std::fs::write(root.join("new.txt"), "x\n").unwrap();

        let status: std::collections::HashMap<PathBuf, StatusKind> =
            git_status(&root).into_iter().collect();
        // git は toplevel を realpath で返すので比較側も canonicalize（macOS の /var→/private/var）。
        let tracked = paths::canonicalize(root.join("tracked.txt")).unwrap();
        let new = paths::canonicalize(root.join("new.txt")).unwrap();
        assert_eq!(status.get(&tracked), Some(&StatusKind::Modified));
        assert_eq!(status.get(&new), Some(&StatusKind::Untracked));

        // buffer_diff: HEAD="one\n" vs 現在 "two\n" → 1 行 Modified
        let hunks = buffer_diff(&tracked, "two\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kind, HunkKind::Modified);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **gutter が「全行 Modified」にならないこと**（WINDOWS-PORT.md §W4 の受入条件）。
    ///
    /// Windows の Git はインストーラ既定が `core.autocrlf=true` なので、
    /// **index は LF・作業ツリーは CRLF** という状態が普通に起きる。素朴に突き合わせると
    /// 全行が違って見え、**変更していないファイルの gutter が真っ赤になる**。
    ///
    /// `.gitattributes` での回避は「necoder が開くのは他人のリポジトリ」なので採れない
    /// （§4）。だから **necoder 側が比較前に LF 正規化する**のが唯一の道であり、
    /// それをこのテストで固定する。
    ///
    /// mac / Linux でも同じ状況は作れる（autocrlf を明示的に立てる）ので、
    /// **プラットフォームを問わず走る**ようにしてある。
    #[test]
    fn gutter_ignores_line_ending_differences_but_not_real_edits() {
        let root = scratch("crlf-gutter");
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
        // **この罠を再現するために、あえて Windows の既定と同じ true にする。**
        git(&["config", "core.autocrlf", "true"]);

        // index には LF で入る（autocrlf=true が commit 時に CRLF→LF する）。
        let file = root.join("note.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
        git(&["add", "note.txt"]);
        git(&["commit", "-q", "-m", "init"]);

        // 作業ツリーを CRLF にする（checkout が autocrlf=true でやることと同じ）。
        std::fs::write(&file, "alpha\r\nbeta\r\ngamma\r\n").unwrap();
        let working = std::fs::read_to_string(&file).unwrap();
        assert!(working.contains("\r\n"), "作業ツリーが CRLF になっていない");

        let tracked = paths::canonicalize(&file).unwrap();

        // ① 改行しか違わない ＝ gutter は 1 本も出ない（これが本命）。
        assert!(
            buffer_diff(&tracked, &working).is_empty(),
            "改行の違いだけで gutter が出た（全行 Modified の再現）"
        );

        // ② 正規化が**本物の変更まで隠していない**こと。CRLF のまま 1 行だけ変える。
        let edited = "alpha\r\nBETA\r\ngamma\r\n";
        let hunks = buffer_diff(&tracked, edited);
        assert_eq!(hunks.len(), 1, "本物の変更が 1 hunk として出ない");
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
        // 改行変換を切る。Windows の Git は既定で core.autocrlf=true（インストーラ既定）なので、
        // 切らないと checkout したファイルが CRLF になり「書いた内容と読んだ内容が違う」で落ちる。
        // このテストが見たいのは merge/status であって改行ではない（WINDOWS-PORT.md §4）。
        git(&["config", "core.autocrlf", "false"]);

        // 新規ファイル → stage 前は unstaged=Untracked
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let a = paths::canonicalize(root.join("a.txt")).unwrap();
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
        // 改行変換を切る。Windows の Git は既定で core.autocrlf=true（インストーラ既定）なので、
        // 切らないと checkout したファイルが CRLF になり「書いた内容と読んだ内容が違う」で落ちる。
        // このテストが見たいのは merge/status であって改行ではない（WINDOWS-PORT.md §4）。
        git(&["config", "core.autocrlf", "false"]);
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
        // 改行変換を切る（Windows の Git 既定 core.autocrlf=true 対策・WINDOWS-PORT.md §4）。
        git(&root, &["config", "core.autocrlf", "false"]);
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
        // 改行変換を切る（Windows の Git 既定 core.autocrlf=true 対策・WINDOWS-PORT.md §4）。
        git(&root, &["config", "core.autocrlf", "false"]);
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
        // 改行変換を切る。Windows の Git は既定で core.autocrlf=true（インストーラ既定）なので、
        // 切らないと checkout したファイルが CRLF になり「書いた内容と読んだ内容が違う」で落ちる。
        // このテストが見たいのは merge/status であって改行ではない（WINDOWS-PORT.md §4）。
        git(&["config", "core.autocrlf", "false"]);
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
            parse_github_slug("https://github.com/iKora128/necoder.git").as_deref(),
            Some("iKora128/necoder")
        );
        assert_eq!(
            parse_github_slug("git@github.com:iKora128/necoder.git").as_deref(),
            Some("iKora128/necoder")
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

    #[test]
    fn task_slug_is_ascii_first_line_and_never_empty() {
        // 1 行目だけ・小文字 ASCII・区切りは '-' に畳む（FLEET-V2 §4.1）。
        assert_eq!(task_slug("Rope 設計 (undo)\n2 行目は無視"), "rope-undo");
        assert_eq!(task_slug("  --Fix:  Login!!  "), "fix-login");
        // 40 字で切り、末尾の '-' は残さない。
        let long = task_slug(&"abcdefghij-".repeat(6));
        assert!(long.len() <= 40 && !long.ends_with('-'), "{long}");
        // ASCII が 1 文字も無ければ既定名（branch 名が空にならない）。
        assert_eq!(task_slug("設計"), "task");
        assert_eq!(task_slug(""), "task");
    }

    /// O20: ＋ Task の「詳細」— 名前を決める / 起点を決める / 既にあるブランチの worktree を作る。
    #[test]
    fn tasks_can_start_from_a_chosen_branch_or_base() {
        let base = scratch("task_start");
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .unwrap()
        };
        if !git(&["init", "-q", "-b", "main"]).status.success() {
            return;
        }
        std::fs::write(root.join("a.txt"), "1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let first = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        std::fs::write(root.join("a.txt"), "2\n").unwrap();
        git(&["commit", "-qam", "second"]);
        git(&["branch", "feature/existing"]);
        let head_of = |dir: &Path| {
            let output = std::process::Command::new("git")
                .current_dir(dir)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        let start = |branch: &str, base: &str| TaskStart {
            branch: (!branch.is_empty()).then(|| branch.to_string()),
            base: (!base.is_empty()).then(|| base.to_string()),
            skip_setup: false,
        };

        // 名前も起点も空 = 従来どおり task/<slug> を HEAD から。
        let (target, branch, _) =
            create_task_with_on(&LocalHost, &root, "Fix the parser", &start("", "")).unwrap();
        assert_eq!(branch, "task/fix-the-parser");
        assert_eq!(head_of(&target), head_of(&root));

        // 名前を決め、1 つ前のコミットから切る。
        let (target, branch, _) =
            create_task_with_on(&LocalHost, &root, "x", &start("feature/new", &first)).unwrap();
        assert_eq!(branch, "feature/new");
        assert!(target.ends_with("feature-new"));
        assert_eq!(head_of(&target), first, "起点から切れる");

        // 既にあるブランチ = その worktree を作る（新しいブランチは切らない）。
        let (target, branch, _) =
            create_task_with_on(&LocalHost, &root, "x", &start("feature/existing", "")).unwrap();
        assert_eq!(branch, "feature/existing");
        assert_eq!(head_of(&target), head_of(&root));
        assert_eq!(
            git_branches_on(&LocalHost, &root)
                .iter()
                .filter(|known| known.as_str() == "feature/existing")
                .count(),
            1
        );

        // 起点だけ = 名前は自動。
        let (target, branch, _) =
            create_task_with_on(&LocalHost, &root, "Old base", &start("", &first)).unwrap();
        assert_eq!(branch, "task/old-base");
        assert_eq!(head_of(&target), first);

        // 使えない名前は断る。
        assert!(create_task_with_on(&LocalHost, &root, "x", &start("bad..name", "")).is_err());

        // repo ごとの既定の起点（`.necoder/settings.json` の task_base）: 起点を指定しない新しい
        // ブランチはそこから切る。指定した起点・既にあるブランチはそちらが勝つ。
        std::fs::create_dir_all(root.join(".necoder")).unwrap();
        std::fs::write(
            root.join(".necoder/settings.json"),
            format!(r#"{{ "task_base": " {first} " }}"#),
        )
        .unwrap();
        assert_eq!(
            repository_task_base_on(&LocalHost, &root).as_deref(),
            Some(first.as_str())
        );
        let (target, branch, _) =
            create_task_with_on(&LocalHost, &root, "From default", &start("", "")).unwrap();
        assert_eq!(branch, "task/from-default");
        assert_eq!(head_of(&target), first, "名前も起点も空 = repo の既定から");
        let (target, _, _) =
            create_task_with_on(&LocalHost, &root, "x", &start("feature/named", "")).unwrap();
        assert_eq!(head_of(&target), first, "名前だけ = repo の既定から");
        let (target, _, _) =
            create_task_with_on(&LocalHost, &root, "Explicit", &start("", "HEAD")).unwrap();
        assert_eq!(head_of(&target), head_of(&root), "指定した起点が勝つ");
        let (target, _, _) = create_named_task_on(&LocalHost, &root, "CLI", false).unwrap();
        assert_eq!(head_of(&target), first, "ne fleet create も同じ既定");
        std::fs::write(root.join(".necoder/settings.json"), r#"{ "task_base": "" }"#).unwrap();
        assert_eq!(repository_task_base_on(&LocalHost, &root), None, "空は無いのと同じ");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// O20: `.worktreeinclude` に書かれ、かつ git が無視しているファイルだけを新しい Task へ持ち込む。
    #[test]
    fn worktree_include_brings_ignored_files_into_new_tasks() {
        let base = scratch("worktree_include");
        let root = base.join("repo");
        std::fs::create_dir_all(root.join("secrets")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .unwrap()
        };
        if !git(&["init", "-q", "-b", "main"]).status.success() {
            return;
        }
        std::fs::write(root.join(".gitignore"), ".env\nsecrets/\nbuild/\n*.log\n").unwrap();
        std::fs::write(
            root.join(".worktreeinclude"),
            // `.gitignore` と同じ約束: 否定で戻すなら親はディレクトリごとでなく `secrets/*` と書く。
            "# Task へ持ち込む\n.env\nsecrets/*\nnotes.txt\n!secrets/skip.json\n",
        )
        .unwrap();
        std::fs::write(root.join("a.txt"), "1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        std::fs::write(root.join(".env"), "TOKEN=local\n").unwrap();
        std::fs::write(root.join("secrets/key.json"), "{}\n").unwrap();
        std::fs::write(root.join("secrets/skip.json"), "{}\n").unwrap();
        std::fs::write(root.join("build/out.bin"), "bin").unwrap();
        std::fs::write(root.join("debug.log"), "log").unwrap();
        // 無視されていない追跡外（書きかけ）は、`.worktreeinclude` に書いてあっても持ち込まない。
        std::fs::write(root.join("notes.txt"), "draft").unwrap();

        let (target, _branch, failure) =
            create_task_with_on(&LocalHost, &root, "Use env", &TaskStart::default()).unwrap();
        assert_eq!(failure, None);
        assert_eq!(
            std::fs::read_to_string(target.join(".env")).unwrap(),
            "TOKEN=local\n"
        );
        assert!(target.join("secrets/key.json").exists());
        assert!(
            !target.join("secrets/skip.json").exists(),
            "否定（!）も効く"
        );
        assert!(
            !target.join("build/out.bin").exists(),
            "書いていない物は写さない"
        );
        assert!(!target.join("debug.log").exists());
        assert!(
            !target.join("notes.txt").exists(),
            "無視されていない物は写さない"
        );

        // 既にある物は上書きしない・上限を超えた分は数えて写さない。
        let second = base.join("second");
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join(".env"), "TOKEN=mine\n").unwrap();
        let result =
            copy_worktree_includes_within_on(&LocalHost, &root, &second, 1, u64::MAX).unwrap();
        assert_eq!(result.already_there, 1);
        assert_eq!(result.copied, vec![PathBuf::from("secrets/key.json")]);
        assert_eq!(result.over_limit, 0);
        assert_eq!(
            std::fs::read_to_string(second.join(".env")).unwrap(),
            "TOKEN=mine\n"
        );
        let third = base.join("third");
        std::fs::create_dir_all(&third).unwrap();
        let result =
            copy_worktree_includes_within_on(&LocalHost, &root, &third, 1, u64::MAX).unwrap();
        assert_eq!(result.copied.len(), 1);
        assert_eq!(result.over_limit, 1, "件数の上限");

        // `.worktreeinclude` が無ければ何もしない。
        std::fs::remove_file(root.join(".worktreeinclude")).unwrap();
        let fourth = base.join("fourth");
        std::fs::create_dir_all(&fourth).unwrap();
        assert_eq!(
            copy_worktree_includes_on(&LocalHost, &root, &fourth).unwrap(),
            WorktreeIncludes::default()
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// O20: 準備スクリプトは今回だけ飛ばせる。失敗した準備は直してからやり直せる。
    #[test]
    fn setup_can_be_skipped_and_retried() {
        let base = scratch("setup_retry");
        let root = base.join("repo");
        std::fs::create_dir_all(root.join(".necoder")).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&root)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .unwrap()
        };
        if !git(&["init", "-q", "-b", "main"]).status.success() {
            return;
        }
        std::fs::write(root.join("a.txt"), "1\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let script = worktree_setup_script(&root);
        std::fs::write(&script, "touch \"$NECODER_TASK_ROOT/prepared\"\n").unwrap();

        let skip = TaskStart {
            skip_setup: true,
            ..TaskStart::default()
        };
        let (target, _, failure) = create_task_with_on(&LocalHost, &root, "Quick", &skip).unwrap();
        assert_eq!(failure, None);
        assert!(!target.join("prepared").exists(), "飛ばした");

        let (target, branch, failure) =
            create_task_with_on(&LocalHost, &root, "Normal", &TaskStart::default()).unwrap();
        assert_eq!(failure, None);
        assert!(target.join("prepared").exists(), "既定は流す");

        std::fs::write(&script, "echo broken >&2\nexit 3\n").unwrap();
        let (target, branch_failed, failure) =
            create_task_with_on(&LocalHost, &root, "Broken", &TaskStart::default()).unwrap();
        let failure = failure.expect("準備の失敗を返す");
        assert!(failure.contains("broken"), "{failure}");
        // 直してからやり直す。
        std::fs::write(&script, "touch \"$NECODER_TASK_ROOT/prepared\"\n").unwrap();
        prepare_task_worktree_on(&LocalHost, &root, &target, &branch_failed, true).unwrap();
        assert!(target.join("prepared").exists());
        assert!(!branch.is_empty());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn task_worktrees_sit_beside_the_repository() {
        let dir = task_worktree_dir(Path::new("/work/necoder")).unwrap();
        assert_eq!(dir, PathBuf::from("/work/necoder-worktrees"));
        assert_eq!(worktree_setup_script(Path::new("/work/necoder")), PathBuf::from("/work/necoder/.necoder/worktree-setup.sh"));
        assert!(task_worktree_dir(Path::new("/")).is_none(), "親が無ければ作れない");
    }

    /// 検査用の一時 repo（git が無い環境では None＝スキップ）。フックは `hooks/` に置く
    /// （repo-local の `core.hooksPath` で指す＝利用者のグローバル設定に左右されない）。
    fn hook_repo(tag: &str) -> Option<PathBuf> {
        let root = scratch(tag);
        std::fs::create_dir_all(root.join("hooks")).ok()?;
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .ok()
                .filter(|output| output.status.success())
        };
        git(&["init", "-q"])?;
        for (key, value) in [
            ("user.email", "t@example.com"),
            ("user.name", "tester"),
            ("core.autocrlf", "false"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "hooks"),
        ] {
            git(&["config", key, value])?;
        }
        Some(root)
    }

    /// 実行可能なフックを書く（unix のみ・Windows の Git は sh で走らせるが権限の付け方が違う）。
    #[cfg(unix)]
    fn write_hook(root: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        let path = root.join("hooks").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn commit_count(root: &Path) -> usize {
        let output = Command::new("git")
            .current_dir(root)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    }

    #[test]
    fn unified_diff_body_has_one_line_per_diff_line() {
        let body = unified_diff_body("a\n\nb\n", "a\n\nc\n").unwrap();
        assert_eq!(
            body.lines().collect::<Vec<_>>(),
            ["@@ -1,3 +1,3 @@", " a", " ", "-b", "+c"]
        );
        // 末尾の改行だけの違いも差分として出る（1 行の入れ替え）。
        let body = unified_diff_body("a\n", "a").unwrap();
        assert_eq!(
            body.lines().collect::<Vec<_>>(),
            ["@@ -1,1 +1,1 @@", "-a", "+a"]
        );
        assert_eq!(
            unified_diff_body("a\r\nb\r\n", "a\nb\n"),
            None,
            "改行コードだけは差分にしない"
        );
    }

    #[test]
    fn automatic_git_disables_hooks_but_user_actions_keep_them() {
        // 自動で走る git（status 等）は hooksPath も封じる。
        assert_eq!(
            git_command_args(GitTrigger::Automatic, ["status", "--porcelain"]),
            [
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "protocol.ext.allow=never",
                "status",
                "--porcelain",
            ]
        );
        // 明示のコミット / push はフックを残す。fsmonitor と ext:: は両方の経路で封じる。
        let user = git_command_args(GitTrigger::UserAction, ["commit", "-m", "x"]);
        assert!(
            !user.iter().any(|arg| arg.starts_with("core.hooksPath")),
            "{user:?}"
        );
        assert!(user.contains(&"core.fsmonitor=false".to_string()));
        assert!(user.contains(&"protocol.ext.allow=never".to_string()));
        assert_eq!(user[user.len() - 3..], ["commit", "-m", "x"]);
    }

    #[test]
    fn failure_output_keeps_the_command_output_apart_from_the_headline() {
        let output = CommandOutput {
            status_code: Some(1),
            stdout: b"ignored".to_vec(),
            stderr: b"pre-commit: trailing whitespace\n".to_vec(),
        };
        let error = ensure_command_success(&output, "コミットに失敗").unwrap_err();
        assert_eq!(failure_output(&error), "pre-commit: trailing whitespace");
        // 従来のログ表記（見出し: 出力）は変わらない。
        assert_eq!(
            format!("{error:#}"),
            "コミットに失敗: pre-commit: trailing whitespace"
        );
        // git を起動できなかった等は、連鎖全体を理由にする。
        let spawn = anyhow::anyhow!("No such file or directory").context("git の実行に失敗");
        assert_eq!(
            failure_output(&spawn),
            "git の実行に失敗: No such file or directory"
        );
    }

    /// パネルからのコミットはフックを走らせ、止められたらフックの出力を返す。
    /// 自動で走る git（worktree 作成）はフックを走らせない。
    #[cfg(unix)]
    #[test]
    fn commit_runs_hooks_and_reports_their_output() {
        let Some(root) = hook_repo("hooks_commit") else {
            return; // git 無し環境はスキップ
        };
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        stage_all(&root).unwrap();
        commit(&root, "init").unwrap();

        write_hook(
            &root,
            "pre-commit",
            "echo 'pre-commit: 末尾に空白があります' >&2\nexit 1",
        );
        std::fs::write(root.join("a.txt"), "two \n").unwrap();
        stage_all(&root).unwrap();
        let error = commit(&root, "blocked").unwrap_err();
        assert!(
            failure_output(&error).contains("pre-commit: 末尾に空白があります"),
            "{error:#}"
        );
        assert_eq!(commit_count(&root), 1, "フックが止めたらコミットされない");

        write_hook(&root, "pre-commit", "touch pre-commit-ran\nexit 0");
        write_hook(
            &root,
            "commit-msg",
            "grep -q '^fix' \"$1\" || { echo 'commit-msg: fix で始めてください' >&2; exit 1; }",
        );
        let error = commit(&root, "tidy").unwrap_err();
        assert!(
            failure_output(&error).contains("commit-msg: fix で始めてください"),
            "{error:#}"
        );
        commit(&root, "fix: trailing space").unwrap();
        assert!(root.join("pre-commit-ran").exists(), "pre-commit が走った");
        assert_eq!(commit_count(&root), 2);

        // 自動の git は hooksPath を /dev/null に向けるので post-checkout は走らない。
        write_hook(&root, "post-checkout", "touch post-checkout-ran");
        let worktree = scratch("hooks_commit_worktree");
        create_task_worktree_on(&LocalHost, &root, &worktree, "task/hooks").unwrap();
        assert!(
            !root.join("post-checkout-ran").exists()
                && !worktree.join("post-checkout-ran").exists(),
            "自動の git でフックが走った"
        );
        remove_worktree(&root, &worktree, true).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// パネルからの push は pre-push を走らせ、止められたらその出力を返す。
    #[cfg(unix)]
    #[test]
    fn push_runs_the_pre_push_hook() {
        let Some(root) = hook_repo("hooks_push") else {
            return;
        };
        let bare = scratch("hooks_push_remote");
        std::fs::create_dir_all(&bare).unwrap();
        let bare_init = Command::new("git")
            .current_dir(&bare)
            .args(["init", "-q", "--bare"])
            .output()
            .unwrap();
        assert!(bare_init.status.success());
        Command::new("git")
            .current_dir(&root)
            .args(["remote", "add", "origin", &bare.to_string_lossy()])
            .output()
            .unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        stage_all(&root).unwrap();
        commit(&root, "init").unwrap();
        push(&root).unwrap(); // upstream 未設定 → set-upstream で通る（フック無し）

        write_hook(
            &root,
            "pre-push",
            "echo 'pre-push: テストが落ちています' >&2\nexit 1",
        );
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        stage_all(&root).unwrap();
        commit(&root, "second").unwrap();
        let error = push(&root).unwrap_err();
        assert!(
            failure_output(&error).contains("pre-push: テストが落ちています"),
            "{error:#}"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&bare);
    }

    /// Fleet の「変更」から開く diff は Task の base と比べる。エージェントがコミット済みの変更は
    /// HEAD 比較では消えるが、base 比較なら出る。削除・新規・フォルダごとの削除も区別する。
    #[test]
    fn diff_against_a_base_commit_shows_committed_deleted_and_new_files() {
        let Some(root) = hook_repo("diff_base") else {
            return;
        };
        let host = LocalHost::shared();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(root.join("sub").join("deep.txt"), "deep\n").unwrap();
        stage_all(&root).unwrap();
        commit(&root, "base").unwrap();
        let base = git_head_oid_on(host.as_ref(), &root).unwrap();

        // エージェントのコミット（base より後）＋作業ツリーの削除・新規。
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        stage_all(&root).unwrap();
        commit(&root, "agent").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::remove_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();

        let a = root.join("a.txt");
        let task_base = DiffBase::Commit(base.clone());
        assert_eq!(
            diff_file_on(host.as_ref(), &a, &DiffBase::Head, Some("two\n")),
            FileDiff::Unchanged,
            "HEAD 比較ではコミット済みの変更が消える（直した不具合の再現）"
        );
        match diff_file_on(host.as_ref(), &a, &task_base, Some("two\n")) {
            FileDiff::Modified(body) => {
                assert!(body.contains("-one") && body.contains("+two"), "{body}")
            }
            other => panic!("base 比較なら中身が出る: {other:?}"),
        }
        match diff_file_on(host.as_ref(), &root.join("gone.txt"), &task_base, None) {
            FileDiff::Deleted(body) => assert!(body.contains("-bye"), "{body}"),
            other => panic!("削除は削除として出る: {other:?}"),
        }
        match diff_file_on(
            host.as_ref(),
            &root.join("sub").join("deep.txt"),
            &task_base,
            None,
        ) {
            FileDiff::Deleted(body) => assert!(body.contains("-deep"), "{body}"),
            other => panic!("フォルダごと消えても削除として出る: {other:?}"),
        }
        match diff_file_on(
            host.as_ref(),
            &root.join("new.txt"),
            &task_base,
            Some("new\n"),
        ) {
            FileDiff::Added(body) => assert!(body.contains("+new"), "{body}"),
            other => panic!("base に無いファイルは新規: {other:?}"),
        }
        assert_eq!(
            diff_file_on(host.as_ref(), &root.join("never.txt"), &task_base, None),
            FileDiff::Missing
        );

        assert_eq!(
            rev_text_on(host.as_ref(), &a, &base).as_deref(),
            Some("one\n")
        );
        assert_eq!(
            rev_text_on(host.as_ref(), &root.join("new.txt"), &base),
            None
        );
        assert_eq!(
            rev_text_on(host.as_ref(), &a, "--output=/tmp/necoder-probe"),
            None,
            "オプションに見える rev は git に渡さない"
        );
        assert_eq!(task_base.label(), base[..7]);
        assert_eq!(DiffBase::Head.label(), "HEAD");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// GUI / CLI 共通の Task 作成先。タイトルは表示用、branch は安全な ASCII slug に分ける。
pub fn task_slug(title: &str) -> String {
    let text: String = title.lines().next().unwrap_or("").chars().map(|c| {
        if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }
    }).collect();
    let slug = text.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    let slug = slug.chars().take(40).collect::<String>().trim_end_matches('-').to_string();
    if slug.is_empty() { "task".to_string() } else { slug }
}

/// Task の worktree を置く親フォルダ `<repo の親>/<repo 名>-worktrees`（FLEET-V2 §4.1・§6.1）。
/// ダイアログの表示と実際の作成が同じ場所を指すよう、計算はここ 1 か所に置く。
pub fn task_worktree_dir(root: &Path) -> Option<PathBuf> {
    let parent = root.parent()?;
    let repo = root.file_name()?.to_string_lossy();
    Some(parent.join(format!("{repo}-worktrees")))
}

/// 準備スクリプトの置き場所（統合先の worktree 基準・§6.2）。
pub fn worktree_setup_script(root: &Path) -> PathBuf {
    root.join(".necoder/worktree-setup.sh")
}

/// 「作る」ボタンが書く準備スクリプトのテンプレ（§6.2 の 3 用途そのまま）。
/// necoder に言語の知識を持たせない — Rust 固有の `CARGO_TARGET_DIR` 共有はスクリプト側の判断。
pub const WORKTREE_SETUP_TEMPLATE: &str = r#"#!/bin/sh
# necoder の Task worktree 準備スクリプト。worktree 作成直後に 1 回、その worktree を cwd にして走る。
# 環境: NECODER_MAIN_ROOT（統合先）/ NECODER_TASK_ROOT（新 worktree）/ NECODER_TASK_BRANCH
set -e
# 1) 追跡外ファイルを持ち込む（worktree には commit 済みしか入らない）
for f in .env .env.local .necoder/settings.local.json; do
  if [ -f "$NECODER_MAIN_ROOT/$f" ]; then
    mkdir -p "$(dirname "$NECODER_TASK_ROOT/$f")"
    cp "$NECODER_MAIN_ROOT/$f" "$NECODER_TASK_ROOT/$f"
  fi
done
# 2) Task のプロセス（ACP・ターミナル）に渡す環境変数（KEY=VALUE を 1 行ずつ・.gitignore 推奨）
mkdir -p "$NECODER_TASK_ROOT/.necoder"
cat > "$NECODER_TASK_ROOT/.necoder/task.env" <<ENV
CARGO_TARGET_DIR=$NECODER_MAIN_ROOT/target
ENV
# 3) 依存の準備（言語ごと・任意）
# pnpm install --prefer-offline
"#;

/// リポジトリの既定の起点（O20）: 統合先の `.necoder/settings.json` の `task_base`
/// （例 `"origin/develop"`）。＋ Task・fan-out・`ne fleet create` が起点を指定せずに**新しいブランチを
/// 切る**時に使う。無い・読めない・空 = 統合先の HEAD。Host 越しに読む（SSH の先の repo でも同じ）。
pub fn repository_task_base_on(host: &dyn Host, root: &Path) -> Option<String> {
    let content = host
        .read_file(&root.join(".necoder").join("settings.json"))
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&content.bytes).ok()?;
    value
        .get("task_base")?
        .as_str()
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .map(str::to_string)
}

/// worktree を作って準備を一度だけ実行する。準備失敗でも作成済み worktree を返し、台帳に failed を残せる。
/// 起点はリポジトリの既定（[`repository_task_base_on`]）、無ければ統合先の HEAD。
pub fn create_named_task_on(host: &dyn Host, root: &Path, title: &str, run_setup: bool) -> Result<(PathBuf, String, Option<String>)> {
    let worktrees = task_worktree_dir(root).context("worktree の作成先がありません")?;
    let stem = task_slug(title);
    let used = git_branches_on(host, root);
    let mut number = 1;
    let (branch, target) = loop {
        let suffix = if number == 1 { stem.clone() } else { format!("{stem}-{number}") };
        let branch = format!("task/{suffix}");
        let target = worktrees.join(&suffix);
        if !used.contains(&branch) && host.metadata(&target).is_err() { break (branch, target); }
        number += 1;
    };
    match repository_task_base_on(host, root) {
        Some(base) => create_task_worktree_from_on(host, root, &target, &branch, &base)?,
        None => create_task_worktree_on(host, root, &target, &branch)?,
    }
    let failure = prepare_task_worktree_on(host, root, &target, &branch, run_setup).err().map(|error| format!("{error:#}"));
    Ok((target, branch, failure))
}

/// ＋ Task の作り方（O20・ダイアログの「詳細」）。どちらも空なら [`create_named_task_on`] と同じ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskStart {
    /// ブランチ名。空 = 1 行目から `task/<slug>`。**既にあるローカルブランチ名なら、そのブランチの
    /// worktree を作る**（新しいブランチは切らない・起点は使わない）。
    pub branch: Option<String>,
    /// 新しいブランチを切る起点（ブランチ / タグ / コミット）。空 = リポジトリの既定
    /// （[`repository_task_base_on`]）、それも無ければ統合先の HEAD。
    pub base: Option<String>,
    /// 準備スクリプトを今回は流さない（`.worktreeinclude` の持ち込みはする・O20）。
    pub skip_setup: bool,
}

/// [`TaskStart`] に従って Task の worktree を作り、準備スクリプトを流す（O20）。
/// 返すのは `(worktree, ブランチ, 準備の失敗)`。
pub fn create_task_with_on(
    host: &dyn Host,
    root: &Path,
    title: &str,
    start: &TaskStart,
) -> Result<(PathBuf, String, Option<String>)> {
    let branch = start
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty());
    let base = start
        .base
        .as_deref()
        .map(str::trim)
        .filter(|base| !base.is_empty());
    let run_setup = !start.skip_setup;
    // 起点の指定が無ければリポジトリの既定（O20）。名前も起点も空なら下の create_named_task_on が読む。
    let repository_base = if base.is_none() && branch.is_some() {
        repository_task_base_on(host, root)
    } else {
        None
    };
    let base = base.or(repository_base.as_deref());
    let Some(branch) = branch else {
        if base.is_none() {
            return create_named_task_on(host, root, title, run_setup);
        }
        // 名前は自動・起点だけ指定。
        let (target, branch) = free_task_target(host, root, &task_slug(title))?;
        create_task_worktree_from_on(host, root, &target, &branch, base.unwrap_or("HEAD"))?;
        let failure = prepare_task_worktree_on(host, root, &target, &branch, run_setup)
            .err()
            .map(|error| format!("{error:#}"));
        return Ok((target, branch, failure));
    };
    let worktrees = task_worktree_dir(root).context("worktree の作成先がありません")?;
    let folder = branch.replace('/', "-");
    let mut target = worktrees.join(&folder);
    let mut number = 2;
    while host.metadata(&target).is_ok() {
        target = worktrees.join(format!("{folder}-{number}"));
        number += 1;
    }
    if git_branches_on(host, root).iter().any(|known| known == branch) {
        // 既にあるブランチ = その worktree を作る（別の worktree で使っていれば git が断る）。
        add_worktree_on(host, root, &target, branch)?;
    } else {
        let valid = run_git(host, root, ["check-ref-format", "--branch", branch])
            .is_ok_and(|output| output.success());
        anyhow::ensure!(valid, "ブランチ名に使えない文字があります: {branch}");
        create_task_worktree_from_on(host, root, &target, branch, base.unwrap_or("HEAD"))?;
    }
    let failure = prepare_task_worktree_on(host, root, &target, branch, run_setup)
        .err()
        .map(|error| format!("{error:#}"));
    Ok((target, branch.to_string(), failure))
}

/// 空いている `task/<slug>`（と worktree の置き場）を探す（[`create_named_task_on`] と同じ決め方）。
fn free_task_target(host: &dyn Host, root: &Path, stem: &str) -> Result<(PathBuf, String)> {
    let worktrees = task_worktree_dir(root).context("worktree の作成先がありません")?;
    let used = git_branches_on(host, root);
    let mut number = 1;
    loop {
        let suffix = if number == 1 {
            stem.to_string()
        } else {
            format!("{stem}-{number}")
        };
        let branch = format!("task/{suffix}");
        let target = worktrees.join(&suffix);
        if !used.contains(&branch) && host.metadata(&target).is_err() {
            return Ok((target, branch));
        }
        number += 1;
    }
}

/// 起点 `base` から新しいブランチと worktree を作る（`git worktree add -b <branch> <path> <base>`）。
pub fn create_task_worktree_from_on(
    host: &dyn Host,
    dir: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<()> {
    anyhow::ensure!(!base.starts_with('-'), "起点に使えない名前です: {base}");
    let path = path.to_string_lossy().into_owned();
    let output = run_git(
        host,
        dir,
        ["worktree", "add", "-b", branch, path.as_str(), base],
    )
    .context("Task worktree の作成に失敗")?;
    anyhow::ensure!(
        output.success(),
        "Task worktree の作成に失敗: {}",
        git_fail_message(&output)
    );
    Ok(())
}

/// `.worktreeinclude`（統合先のルート・`.gitignore` と同じ書き方）の置き場所（O20・A05）。
pub fn worktree_include_file(root: &Path) -> PathBuf {
    root.join(".worktreeinclude")
}

/// `.worktreeinclude` で持ち込む量の上限（件数・合計バイト）。`node_modules` のような大物を
/// 1 ファイルずつ写さない（それは準備スクリプトの `pnpm install` や symlink の仕事）。
pub const WORKTREE_INCLUDE_MAX_FILES: usize = 500;
pub const WORKTREE_INCLUDE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// `.worktreeinclude` で持ち込んだ結果。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorktreeIncludes {
    /// 写したファイル（統合先からの相対パス）。
    pub copied: Vec<PathBuf>,
    /// 新しい worktree に既にあったので写さなかった数（上書きしない）。
    pub already_there: usize,
    /// 上限を超えたので写さなかった数。
    pub over_limit: usize,
}

/// `.worktreeinclude` に書かれ、**かつ git が無視している**追跡外のファイルを、統合先 `main` から
/// 新しい worktree `target` へ写す（Claude Code・Orca と同じ約束。`.env` のように commit しない
/// 物を Task へ持ち込む）。追跡中のファイルは checkout で既に入り、無視されていない追跡外の
/// ファイル（書きかけのソース）は持ち込まない。`.worktreeinclude` が無ければ何もしない。
///
/// 一致の判定は git 自身（`git ls-files --others --ignored`）に任せる＝否定（`!`）やディレクトリの
/// 書き方も `.gitignore` とまったく同じに効く。書き込みは `target` を開き直した host で行う
/// （SSH の host は project の外へ書けない）。
pub fn copy_worktree_includes_on(
    host: &dyn Host,
    main: &Path,
    target: &Path,
) -> Result<WorktreeIncludes> {
    copy_worktree_includes_within_on(
        host,
        main,
        target,
        WORKTREE_INCLUDE_MAX_FILES,
        WORKTREE_INCLUDE_MAX_BYTES,
    )
}

fn copy_worktree_includes_within_on(
    host: &dyn Host,
    main: &Path,
    target: &Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<WorktreeIncludes> {
    let include = worktree_include_file(main);
    if host.metadata(&include).is_err() {
        return Ok(WorktreeIncludes::default());
    }
    let list = |extra: &str| -> Result<Vec<PathBuf>> {
        let output = run_git(
            host,
            main,
            ["ls-files", "-z", "--others", "--ignored", extra],
        )?;
        anyhow::ensure!(
            output.success(),
            "git ls-files に失敗: {}",
            git_fail_message(&output)
        );
        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| PathBuf::from(String::from_utf8_lossy(entry).into_owned()))
            .collect())
    };
    let wanted = list(&format!("--exclude-from={}", include.to_string_lossy()))?;
    if wanted.is_empty() {
        return Ok(WorktreeIncludes::default());
    }
    let ignored: std::collections::HashSet<PathBuf> =
        list("--exclude-standard")?.into_iter().collect();
    let mut files: Vec<PathBuf> = wanted
        .into_iter()
        .filter(|relative| ignored.contains(relative))
        .collect();
    files.sort();
    let target_host = host.host_for_project(target)?;
    let mut result = WorktreeIncludes::default();
    let mut total = 0u64;
    for relative in files {
        let source = main.join(&relative);
        let destination = target.join(&relative);
        if target_host.metadata(&destination).is_ok() {
            result.already_there += 1;
            continue;
        }
        let size = host.metadata(&source).map(|metadata| metadata.len)?;
        if result.copied.len() >= max_files || total + size > max_bytes {
            result.over_limit += 1;
            continue;
        }
        let content = host
            .read_file(&source)
            .with_context(|| format!("読めない: {}", source.display()))?;
        target_host
            .write_file(
                &destination,
                &content.bytes,
                host::WriteCondition::NotExists,
            )
            .with_context(|| format!("書けない: {}", destination.display()))?;
        total += size;
        result.copied.push(relative);
    }
    Ok(result)
}

/// 作ったばかりの Task worktree を使える状態にする: `.worktreeinclude` の持ち込み → 準備スクリプト
/// （スクリプトは持ち込んだファイルを前提にできる・`run_setup = false` なら流さない）。持ち込みに
/// 失敗しても準備は走らせ、両方の失敗をまとめて返す（台帳の failed に残る）。上限で写さなかった分は
/// 失敗にしない（標準エラーに残す）。失敗した準備をやり直す時も同じ関数を呼ぶ（既にある物は写さない）。
pub fn prepare_task_worktree_on(
    host: &dyn Host,
    main: &Path,
    target: &Path,
    branch: &str,
    run_setup: bool,
) -> Result<()> {
    let includes = copy_worktree_includes_on(host, main, target);
    if let Ok(includes) = &includes {
        if includes.over_limit > 0 {
            eprintln!(
                ".worktreeinclude: 上限（{WORKTREE_INCLUDE_MAX_FILES} 件・{} MiB）を超えた {} 件は写していません: {}",
                WORKTREE_INCLUDE_MAX_BYTES / (1024 * 1024),
                includes.over_limit,
                target.display()
            );
        }
    }
    let setup = if run_setup {
        run_task_setup_on(host, main, target, branch)
    } else {
        Ok(())
    };
    match (includes, setup) {
        (Ok(_), Ok(())) => Ok(()),
        (Err(error), Ok(())) => {
            Err(error.context(".worktreeinclude のファイルを持ち込めませんでした"))
        }
        (Ok(_), Err(error)) => Err(error),
        (Err(include_error), Err(setup_error)) => Err(anyhow::anyhow!(
            ".worktreeinclude のファイルを持ち込めませんでした: {include_error:#}\n{setup_error:#}"
        )),
    }
}

/// 準備スクリプト（§6.2）を新 worktree を cwd に 1 回流す。無ければ何もしない。
pub fn run_task_setup_on(host: &dyn Host, main: &Path, target: &Path, branch: &str) -> Result<()> {
    let script = worktree_setup_script(main);
    if host.metadata(&script).is_err() { return Ok(()); }
    anyhow::ensure!(host.has_posix_shell(), "worktree-setup.sh には POSIX shell が必要です");
    let spec = host::CommandSpec::new("sh", target).args([script.to_string_lossy().into_owned()])
        .envs([
            ("NECODER_MAIN_ROOT", main.to_string_lossy().into_owned()),
            ("NECODER_TASK_ROOT", target.to_string_lossy().into_owned()),
            ("NECODER_TASK_BRANCH", branch.to_string()),
        ]);
    let output = host.run_command(&spec)?;
    anyhow::ensure!(output.success(), "準備スクリプトに失敗しました: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    Ok(())
}
