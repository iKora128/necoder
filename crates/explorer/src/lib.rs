//! Workspace-independent Explorer model and interaction Entity.

use gpui::{
    div, Context, EventEmitter, FocusHandle, IntoElement, Pixels, Point, Render, SharedString,
    Window,
};
use project::Worktree;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: SharedString,
    pub is_dir: bool,
    pub depth: usize,
    pub is_expanded: bool,
    pub ignored: bool,
}

#[derive(Clone)]
pub struct ContextMenu {
    pub path: PathBuf,
    pub is_dir: bool,
    pub position: Point<Pixels>,
}

#[derive(Clone)]
pub struct Naming {
    pub kind: NamingKind,
    pub parent: PathBuf,
    pub target: Option<PathBuf>,
    pub value: String,
    pub focus: FocusHandle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NamingKind {
    NewFile,
    NewDir,
    Rename,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Tree,
    Columns,
    Icons,
}

/// Project ごとの tree/navigation state。FS 読み込みは `refresh` だけで行う。
pub struct ExplorerProject {
    pub expanded: HashSet<PathBuf>,
    pub rows: Vec<TreeRow>,
    pub selected: Option<PathBuf>,
    pub current_dir: Option<PathBuf>,
    dir_listings: RefCell<HashMap<PathBuf, Vec<project::Entry>>>,
}

impl Default for ExplorerProject {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            current_dir: None,
            dir_listings: RefCell::new(HashMap::new()),
        }
    }
}

impl ExplorerProject {
    pub fn refresh(&mut self, worktree: &Worktree) {
        let mut rows = Vec::new();
        build_rows(worktree, worktree.root(), 0, &self.expanded, &mut rows);
        self.rows = rows;

        let mut directories = self.expanded.iter().cloned().collect::<Vec<_>>();
        directories.push(worktree.root().to_path_buf());
        if let Some(current) = &self.current_dir {
            // Finder 風カラム表示は root → current の各ディレクトリを 1 カラムずつ描く。
            // current だけを cache すると、深さ 2 以上では中間カラムが cache miss になり、
            // 選択中の階層だけが透明になったように見える。Render 中に I/O はしない契約を
            // 保ったまま、表示に必要な祖先を refresh 時にまとめて読み込む。
            let root = worktree.root();
            let mut directory = current.as_path();
            loop {
                directories.push(directory.to_path_buf());
                if directory == root {
                    break;
                }
                let Some(parent) = directory.parent() else {
                    break;
                };
                if parent.starts_with(root) || parent == root {
                    directory = parent;
                } else {
                    // root 外ブラウズはカラム連鎖を作らないため current だけでよい。
                    break;
                }
            }
        }
        directories.sort();
        directories.dedup();
        let mut listings = self.dir_listings.borrow_mut();
        listings.clear();
        for directory in directories {
            listings.insert(
                directory.clone(),
                worktree.read_any_dir(&directory).unwrap_or_default(),
            );
        }
    }

    /// Render-safe cache lookup. Missing directories are empty until the controller refreshes.
    pub fn listed_dir(&self, dir: &Path) -> Vec<project::Entry> {
        self.dir_listings
            .borrow()
            .get(dir)
            .cloned()
            .unwrap_or_default()
    }
}

fn build_rows(
    worktree: &Worktree,
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<TreeRow>,
) {
    let Ok(entries) = worktree.read_dir(dir) else {
        return;
    };
    for entry in entries {
        let is_expanded = entry.is_dir && expanded.contains(&entry.path);
        rows.push(TreeRow {
            path: entry.path.clone(),
            name: entry.name.into(),
            is_dir: entry.is_dir,
            depth,
            is_expanded,
            ignored: entry.ignored,
        });
        if is_expanded {
            build_rows(worktree, &entry.path, depth + 1, expanded, rows);
        }
    }
}

pub enum ExplorerEvent {
    OpenPath(PathBuf),
    FilesChanged,
    Focus,
}

/// Explorer interaction state. Rendering can be migrated independently without changing ownership.
pub struct Explorer {
    view: ViewMode,
    context_menu: Option<ContextMenu>,
    naming: Option<Naming>,
}

impl Explorer {
    pub fn new(view: ViewMode) -> Self {
        Self {
            view,
            context_menu: None,
            naming: None,
        }
    }

    pub fn view(&self) -> ViewMode {
        self.view
    }

    pub fn set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    pub fn context_menu(&self) -> Option<ContextMenu> {
        self.context_menu.clone()
    }

    pub fn show_context_menu(&mut self, menu: ContextMenu, cx: &mut Context<Self>) {
        self.context_menu = Some(menu);
        cx.notify();
    }

    pub fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        cx.notify();
    }

    pub fn naming(&self) -> Option<Naming> {
        self.naming.clone()
    }

    pub fn set_naming(&mut self, naming: Naming, cx: &mut Context<Self>) {
        self.naming = Some(naming);
        cx.notify();
    }

    pub fn take_naming(&mut self, cx: &mut Context<Self>) -> Option<Naming> {
        let naming = self.naming.take();
        cx.notify();
        naming
    }

    pub fn update_naming(&mut self, update: impl FnOnce(&mut Naming), cx: &mut Context<Self>) {
        if let Some(naming) = &mut self.naming {
            update(naming);
            cx.notify();
        }
    }
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_cache_is_render_safe() {
        let project = ExplorerProject::default();
        assert!(project.listed_dir(Path::new("/missing")).is_empty());
    }

    #[test]
    fn refresh_caches_every_directory_in_column_chain() {
        let root = std::env::temp_dir().join(format!(
            "necoder_explorer_columns_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("現在時刻")
                .as_nanos()
        ));
        let created_current = root.join("first/second/current");
        std::fs::create_dir_all(&created_current).expect("深いディレクトリを作成");
        std::fs::write(created_current.join("leaf.txt"), "test").expect("末端ファイルを作成");
        let worktree = Worktree::new(&root).expect("worktree を作成");
        // Worktree は /var → /private/var のような実パスへ正規化するため、UI が保持するのと同じ
        // root 基準のパスで current を組み立てる。
        let first = worktree.root().join("first");
        let second = first.join("second");
        let current = second.join("current");
        let mut project = ExplorerProject {
            current_dir: Some(current.clone()),
            ..ExplorerProject::default()
        };
        project.refresh(&worktree);

        assert_eq!(
            project
                .listed_dir(&first)
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["second"]
        );
        assert_eq!(
            project
                .listed_dir(&second)
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["current"]
        );
        assert_eq!(
            project
                .listed_dir(&current)
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["leaf.txt"]
        );

        std::fs::remove_dir_all(root).expect("テストディレクトリを削除");
    }
}
