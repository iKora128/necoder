//! project — ファイルシステムの走査（worktree の縮小版）。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §2: zed `fs`/`worktree` 相当。M3 は**遅延 read_dir**（展開時に直下を読む）で、
//! `.git` と gitignore 対象を除外する（ripgrep の `ignore` crate を使用）。
//! ファイル監視・インクリメンタル更新は後続（M8/性能）で追加する。

use anyhow::{Context as _, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

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
}
