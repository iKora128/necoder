//! Workspace-independent Explorer model and interaction Entity.

use gpui::{
    div, Context, EventEmitter, FocusHandle, IntoElement, Pixels, Point, Render, SharedString,
    Window,
};
use project::{DirListings, Worktree};
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

/// Project ごとの tree/navigation state。FS 読み込みは `refresh`（同期）か、
/// `directories_to_read` → 背景で [`project::read_listings_on`] → `apply_listings`（非同期）
/// の形でだけ行う。行の組み立て自体は I/O をしない。
pub struct ExplorerProject {
    pub expanded: HashSet<PathBuf>,
    pub rows: Vec<TreeRow>,
    pub selected: Option<PathBuf>,
    pub current_dir: Option<PathBuf>,
    dir_listings: RefCell<DirListings>,
    /// 背景再構築の世代。古い読み取り結果が新しい状態を上書きしないための番号。
    refresh_generation: u64,
}

impl Default for ExplorerProject {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            current_dir: None,
            dir_listings: RefCell::new(HashMap::new()),
            refresh_generation: 0,
        }
    }
}

impl ExplorerProject {
    /// 同期版: 読み取りと反映を一度に行う。local 用（一瞬で終わる）。remote はディレクトリ
    /// 1 つにつき SSH の往復になるので、controller が読み取りだけ背景へ出す。
    pub fn refresh(&mut self, worktree: &Worktree) {
        let root = worktree.root().to_path_buf();
        let mut listings = DirListings::new();
        for directory in self.directories_to_read(&root) {
            listings.insert(
                directory.clone(),
                worktree.read_any_dir(&directory).unwrap_or_default(),
            );
        }
        self.apply_listings(&root, listings);
    }

    /// 表示に必要なディレクトリ（root・展開中・カラム連鎖の祖先）。I/O はしない。
    pub fn directories_to_read(&self, root: &Path) -> Vec<PathBuf> {
        let mut directories = self.expanded.iter().cloned().collect::<Vec<_>>();
        directories.push(root.to_path_buf());
        if let Some(current) = &self.current_dir {
            // Finder 風カラム表示は root → current の各ディレクトリを 1 カラムずつ描く。
            // current だけを cache すると、深さ 2 以上では中間カラムが cache miss になり、
            // 選択中の階層だけが透明になったように見える。Render 中に I/O はしない契約を
            // 保ったまま、表示に必要な祖先を refresh 時にまとめて読み込む。
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
        directories
    }

    /// 読み取り結果を反映して行を組み直す。I/O はしない。
    pub fn apply_listings(&mut self, root: &Path, listings: DirListings) {
        *self.dir_listings.borrow_mut() = listings;
        self.rebuild_rows(root);
    }

    /// 手元のキャッシュだけで行を組み直す（展開/折り畳みの即時反映用）。
    /// まだ読んでいないディレクトリは、読み取りが届くまで空のまま。
    pub fn rebuild_rows(&mut self, root: &Path) {
        let mut rows = Vec::new();
        {
            let listings = self.dir_listings.borrow();
            build_rows(&listings, root, 0, &self.expanded, &mut rows);
        }
        self.rows = rows;
    }

    /// 背景再構築を 1 回始める。返った世代を [`Self::is_latest_refresh`] で照合してから反映する
    /// （最後に始めたものだけを採用する。並び順が入れ替わって古い展開状態が勝つのを防ぐ）。
    pub fn begin_refresh(&mut self) -> u64 {
        self.refresh_generation += 1;
        self.refresh_generation
    }

    pub fn is_latest_refresh(&self, generation: u64) -> bool {
        self.refresh_generation == generation
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
    listings: &DirListings,
    dir: &Path,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    rows: &mut Vec<TreeRow>,
) {
    let Some(entries) = listings.get(dir) else {
        return;
    };
    for entry in entries {
        let is_expanded = entry.is_dir && expanded.contains(&entry.path);
        rows.push(TreeRow {
            path: entry.path.clone(),
            name: entry.name.clone().into(),
            is_dir: entry.is_dir,
            depth,
            is_expanded,
            ignored: entry.ignored,
        });
        if is_expanded {
            build_rows(listings, &entry.path, depth + 1, expanded, rows);
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
