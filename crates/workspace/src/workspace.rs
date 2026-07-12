//! workspace — レール（プロジェクト切替）+ 左ドックのエクスプローラ + 中央エディタ + ステータスバー。
//!
//! ARCHITECTURE §5: 1 窓 = アクティブな (project, branch)。レール = 窓内切替。UI-SPEC §1.3 の色の
//! 許可リストに従い、**プロジェクト色**はレール枠/リング・ツリー選択の左バー・キャレットにのみ流す。
//! 状態（開プロジェクト・アクティブ・開ファイル）は `state.json` に保存し、再起動で復元する。

use agent_panel::AgentPanel;
use editor_core::Buffer;
use editor_view::EditorView;
use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, CursorStyle, Entity, FocusHandle,
    Focusable, FontWeight, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Point, SharedString, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowControlArea, WindowOptions, actions, div, point, prelude::*, pulsating_between, px, size,
};
use project::Worktree;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use theme_core::{Theme, project_color};
use ui::Tooltip;
use ui::{Picker, PickerEvent, PickerItem};

actions!(workspace, [FileFinder, ProjectSwitcher, CloseTab, NewThread]);

#[derive(Clone, Copy, PartialEq)]
enum PickerMode {
    Files,
    Projects,
}

const RAIL_WIDTH: f32 = 46.0;
const DOCK_WIDTH: f32 = 218.0;
const DOCK_MIN: f32 = 150.0;
const DOCK_MAX: f32 = 640.0;
const ROW_HEIGHT: f32 = 23.0;
const INDENT: f32 = 14.0;
const AGENT_DOCK_WIDTH: f32 = 440.0; // Agent パネル既定幅（ドラッグで可変）
const AGENT_DOCK_MIN: f32 = 320.0;
const AGENT_DOCK_MAX: f32 = 900.0;
const RESIZE_HANDLE_WIDTH: f32 = 6.0;
const TITLEBAR_HEIGHT: f32 = 38.0;
const TABSTRIP_HEIGHT: f32 = 34.0;
const BREADCRUMB_HEIGHT: f32 = 26.0;
const STATUSBAR_HEIGHT: f32 = 26.0;
/// macOS のネイティブ信号機（appears_transparent 時も残る）を避けるための左余白。
const TRAFFIC_LIGHT_INSET: f32 = 78.0;

/// titlebar / statusbar のドックトグルが指すドック。
#[derive(Clone, Copy, PartialEq)]
enum Dock {
    Left,
    Right,
    Bottom,
}

// ── 状態永続化 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedProject {
    root: PathBuf,
    #[serde(default)]
    open_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedState {
    projects: Vec<PersistedProject>,
    #[serde(default)]
    active: usize,
}

/// `state.json` の標準パス（macOS）。
pub fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/Shirushi/state.json"))
}

/// 前回の (プロジェクト群, アクティブ) を読む。無ければ `None`。
pub fn load_state(path: &Path) -> Option<(Vec<PathBuf>, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: PersistedState = serde_json::from_str(&text).ok()?;
    if state.projects.is_empty() {
        return None;
    }
    let roots = state.projects.into_iter().map(|project| project.root).collect();
    Some((roots, state.active))
}

// ── ツリー ──

struct TreeRow {
    path: PathBuf,
    name: SharedString,
    is_dir: bool,
    depth: usize,
    is_expanded: bool,
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
        });
        if is_expanded {
            build_rows(worktree, &entry.path, depth + 1, expanded, rows);
        }
    }
}

/// エクスプローラの右クリックメニュー（対象パス + 種別 + 出す位置）。
struct ExplorerContextMenu {
    path: PathBuf,
    is_dir: bool,
    position: Point<gpui::Pixels>,
}

/// エクスプローラの表示モード（Finder 風の 3 種。左下で切替）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExplorerView {
    /// 縦ツリー（従来）。
    Tree,
    /// Finder のカラム（Miller columns）。
    Columns,
    /// アイコングリッド。
    Icons,
}

struct ProjectSlot {
    worktree: Rc<Worktree>,
    name: SharedString,
    color: Hsla,
    expanded: HashSet<PathBuf>,
    rows: Vec<TreeRow>,
    selected: Option<PathBuf>,
    open_file: Option<PathBuf>,
    /// カラム/アイコン表示の「現在フォルダ」（ここの直下を見せる）。既定はルート。
    current_dir: Option<PathBuf>,
}

impl ProjectSlot {
    fn refresh(&mut self) {
        let mut rows = Vec::new();
        let root = self.worktree.root().to_path_buf();
        build_rows(&self.worktree, &root, 0, &self.expanded, &mut rows);
        self.rows = rows;
    }
}

// ── Workspace 本体 ──

pub struct Workspace {
    projects: Vec<ProjectSlot>,
    active: usize,
    editor: Option<Entity<EditorView>>,
    agent_panel: Entity<AgentPanel>,
    theme: Theme,
    focus_handle: FocusHandle,
    state_path: Option<PathBuf>,
    // ドックの表示状態（titlebar のアイコンでトグル）。右=Agent パネル・下=ターミナルは M4/M8。
    show_left: bool,
    show_right: bool,
    show_bottom: bool,
    // Agent パネルの可変幅（左縁ドラッグで調整）。
    agent_width: f32,
    resizing_agent: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    // 左ドック（エクスプローラ）の可変幅（右縁ドラッグで調整）。
    explorer_width: f32,
    resizing_explorer: bool,
    // titlebar ドラッグ（down→move で窓移動開始。クリック/ダブルクリックと区別する）。
    should_move_window: bool,
    _editor_observation: Option<Subscription>,
    // オーバーレイ（ファイルファインダ / プロジェクトスイッチャー）
    picker: Option<Entity<Picker>>,
    picker_mode: PickerMode,
    picker_files: Vec<PathBuf>,
    _picker_observation: Option<Subscription>,
    // エクスプローラの表示モード（ツリー/カラム/アイコン）。左下で切替。
    explorer_view: ExplorerView,
    // エクスプローラの右クリックメニュー（開いていれば Some）。
    explorer_context_menu: Option<ExplorerContextMenu>,
}

impl Workspace {
    /// プロジェクトのルート群からワークスペースを組み立てる。開けないルートはスキップ。
    pub fn new(
        roots: Vec<PathBuf>,
        theme: Theme,
        state_path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut projects = Vec::new();
        for root in roots {
            match Worktree::new(&root) {
                Ok(worktree) => {
                    let index = projects.len();
                    let mut slot = ProjectSlot {
                        name: worktree.name().into(),
                        color: project_color(index),
                        worktree: Rc::new(worktree),
                        expanded: HashSet::new(),
                        rows: Vec::new(),
                        selected: None,
                        open_file: None,
                        current_dir: None,
                    };
                    slot.refresh();
                    projects.push(slot);
                }
                Err(error) => eprintln!("プロジェクトを開けない（スキップ）: {error:#}"),
            }
        }
        // 開発用フックの値は projects が move される前に計算しておく。
        let explorer_context_menu = std::env::var_os("SHIRUSHI_CONTEXT_MENU").and_then(|_| {
            projects.first().map(|slot| ExplorerContextMenu {
                path: slot.worktree.root().to_path_buf(),
                is_dir: true,
                position: point(px(120.0), px(210.0)),
            })
        });
        let agent_panel = cx.new(|cx| AgentPanel::new(theme.clone(), cx));
        let workspace = Workspace {
            projects,
            active: 0,
            editor: None,
            agent_panel,
            theme,
            focus_handle: cx.focus_handle(),
            state_path,
            show_left: true,
            show_right: true, // Agent パネル（差別化の本丸）は既定で表示
            show_bottom: false,
            agent_width: AGENT_DOCK_WIDTH,
            resizing_agent: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            explorer_width: DOCK_WIDTH,
            resizing_explorer: false,
            should_move_window: false,
            _editor_observation: None,
            picker: None,
            picker_mode: PickerMode::Files,
            picker_files: Vec::new(),
            _picker_observation: None,
            // 開発用: SHIRUSHI_EXPLORER_VIEW=icons|columns で初期表示モードを指定（撮影確認用）。
            explorer_view: match std::env::var("SHIRUSHI_EXPLORER_VIEW").as_deref() {
                Ok("icons") => ExplorerView::Icons,
                Ok("columns") => ExplorerView::Columns,
                _ => ExplorerView::Tree,
            },
            // 開発用: SHIRUSHI_CONTEXT_MENU=1 でルートの右クリックメニューを開いた状態で撮る。
            explorer_context_menu,
        };
        workspace.update_agent_destination(cx); // 宛先チップにプロジェクト/ブランチを反映
        workspace.save_state(); // 起動時点で状態を書く（再起動復元のため）
        workspace
    }

    /// 起動後に、アクティブプロジェクトの前回開ファイルがあれば開く。
    pub fn restore_open_file(&mut self, open_files: &[Option<PathBuf>], window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Some(path)) = open_files.get(self.active) {
            let path = path.clone();
            self.open_file(path, window, cx);
        }
    }

    fn active_slot(&self) -> Option<&ProjectSlot> {
        self.projects.get(self.active)
    }

    fn switch_project(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.projects.len() || index == self.active {
            return;
        }
        self.active = index;
        // 切替先プロジェクトの開ファイルを復元（無ければエディタを閉じる）
        let open_file = self.projects[index].open_file.clone();
        match open_file {
            Some(path) => self.open_file(path, window, cx),
            None => {
                self.editor = None;
                self._editor_observation = None;
            }
        }
        self.update_agent_destination(cx);
        self.save_state();
        cx.notify();
    }

    /// Agent パネルの宛先チップにアクティブプロジェクト名・ブランチを反映する。
    fn update_agent_destination(&self, cx: &mut Context<Self>) {
        let (name, branch, cwd, files) = match self.active_slot() {
            Some(slot) => {
                // Add context の候補（プロジェクト先頭 60 ファイルの相対パス）。
                let files = slot
                    .worktree
                    .all_files(60)
                    .into_iter()
                    .map(|(_, relative)| SharedString::from(relative))
                    .collect();
                (
                    slot.name.clone(),
                    git_branch(slot.worktree.root()).map(SharedString::from),
                    Some(slot.worktree.root().to_path_buf()),
                    files,
                )
            }
            None => (SharedString::from("—"), None, None, Vec::new()),
        };
        self.agent_panel
            .update(cx, |panel, cx| panel.set_destination(name, branch, cwd, files, cx));
    }

    // ── タブ/スレッドのショートカット（⌘W / ⌘⇧A） ──

    fn close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        self.close_active_editor(cx);
    }

    fn new_agent_thread(&mut self, _: &NewThread, _: &mut Window, cx: &mut Context<Self>) {
        if !self.show_right {
            self.show_right = true;
        }
        self.agent_panel.update(cx, |panel, cx| panel.new_thread(cx));
        cx.notify();
    }

    // ── ドックの可変幅（縁ドラッグ）。Agent=左縁 / エクスプローラ=右縁 ──

    fn on_resize_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dx = f32::from(event.position.x) - self.resize_start_x;
        if self.resizing_agent {
            // 左縁を左へ動かすと広がる（dx 負 → 幅増）。
            self.agent_width = (self.resize_start_width - dx).clamp(AGENT_DOCK_MIN, AGENT_DOCK_MAX);
            cx.notify();
        } else if self.resizing_explorer {
            // 右縁を右へ動かすと広がる（dx 正 → 幅増）。
            self.explorer_width = (self.resize_start_width + dx).clamp(DOCK_MIN, DOCK_MAX);
            cx.notify();
        }
    }

    fn on_resize_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.resizing_agent || self.resizing_explorer {
            self.resizing_agent = false;
            self.resizing_explorer = false;
            cx.notify();
        }
    }

    /// Agent パネルを可変幅コンテナに入れて描く（左縁にリサイズハンドル）。
    fn render_agent_dock(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .flex_none()
            .w(px(self.agent_width))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("agent-resize")
                    .w(px(RESIZE_HANDLE_WIDTH))
                    .h_full()
                    .flex_none()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme.border))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.resizing_agent = true;
                            this.resize_start_x = f32::from(event.position.x);
                            this.resize_start_width = this.agent_width;
                            cx.notify();
                        }),
                    ),
            )
            .child(div().flex_1().min_w_0().h_full().child(self.agent_panel.clone()))
    }

    fn toggle_dock(&mut self, dock: Dock, cx: &mut Context<Self>) {
        match dock {
            Dock::Left => self.show_left = !self.show_left,
            Dock::Right => self.show_right = !self.show_right,
            Dock::Bottom => self.show_bottom = !self.show_bottom,
        }
        cx.notify();
    }

    /// アクティブタブ（＝現在のエディタ）を閉じる。
    fn close_active_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self._editor_observation = None;
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.open_file = None;
            slot.selected = None;
        }
        self.save_state();
        cx.notify();
    }

    fn toggle_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(slot) = self.projects.get_mut(self.active) {
            if slot.expanded.contains(&path) {
                slot.expanded.remove(&path);
            } else {
                slot.expanded.insert(path);
            }
            slot.refresh();
            cx.notify();
        }
    }

    /// エクスプローラの表示モードを切り替える（左下スイッチャー）。
    /// カラム表示は幅が要るので、狭ければ広げる（以後ユーザーがドラッグで調整）。
    fn set_explorer_view(&mut self, view: ExplorerView, cx: &mut Context<Self>) {
        self.explorer_view = view;
        if view == ExplorerView::Columns && self.explorer_width < 440.0 {
            self.explorer_width = 440.0;
        }
        cx.notify();
    }

    /// カラム/アイコン表示で `dir` に入る（現在フォルダを更新）。
    fn enter_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.current_dir = Some(dir.clone());
            slot.selected = Some(dir);
        }
        cx.notify();
    }

    /// エクスプローラの右クリックメニューを出す（対象と位置を記録）。
    fn show_context_menu(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.explorer_context_menu = Some(ExplorerContextMenu { path, is_dir, position });
        cx.notify();
    }

    /// 右クリックメニューを閉じる（外側クリック・アクション実行後）。
    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.explorer_context_menu.take().is_some() {
            cx.notify();
        }
    }

    /// フォルダを**新規ウィンドウ**でプロジェクトとして開く（ウィンドウモデルの核）。
    fn open_folder_as_window(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Shirushi".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(13.0), px(13.0))),
                }),
                is_movable: false,
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|cx| Workspace::new(vec![path.clone()], theme.clone(), state_path(), cx))
            },
        );
        if let Err(error) = opened {
            eprintln!("新規ウィンドウを開けない: {error}");
        }
        self.explorer_context_menu = None;
        cx.notify();
    }

    /// パスをクリップボードへコピー。
    fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        self.explorer_context_menu = None;
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let buffer = match Buffer::from_file(&path) {
            Ok(buffer) => buffer,
            Err(error) => {
                eprintln!("ファイルを開けない: {error:#}");
                return;
            }
        };
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let editor = cx.new(|cx| EditorView::new(buffer, theme, accent, cx));

        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self._editor_observation = Some(cx.observe(&editor, |_, _, cx| cx.notify()));
        self.editor = Some(editor);

        if let Some(slot) = self.projects.get_mut(self.active) {
            slot.selected = Some(path.clone());
            slot.open_file = Some(path);
        }
        self.save_state();
        cx.notify();
    }

    fn save_state(&self) {
        let Some(path) = self.state_path.as_ref() else {
            return;
        };
        let state = PersistedState {
            projects: self
                .projects
                .iter()
                .map(|slot| PersistedProject {
                    root: slot.worktree.root().to_path_buf(),
                    open_file: slot.open_file.clone(),
                })
                .collect(),
            active: self.active,
        };
        let Ok(text) = serde_json::to_string_pretty(&state) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(path, text) {
            eprintln!("状態の保存に失敗: {error}");
        }
    }

    /// 復元用: 各プロジェクトの前回開ファイル。
    pub fn persisted_open_files(state_path: &Path) -> Vec<Option<PathBuf>> {
        let Ok(text) = std::fs::read_to_string(state_path) else {
            return Vec::new();
        };
        let Ok(state) = serde_json::from_str::<PersistedState>(&text) else {
            return Vec::new();
        };
        state.projects.into_iter().map(|project| project.open_file).collect()
    }

    // ── オーバーレイ（Picker） ──

    fn open_file_finder(&mut self, _: &FileFinder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(slot) = self.active_slot() else {
            return;
        };
        let files = slot.worktree.all_files(5000);
        let items = files
            .iter()
            .enumerate()
            .map(|(id, (_, relative))| PickerItem::new(id, relative.clone()))
            .collect();
        self.picker_files = files.into_iter().map(|(path, _)| path).collect();
        self.open_picker(PickerMode::Files, i18n::t!("finder.files"), items, window, cx);
    }

    fn open_project_switcher(&mut self, _: &ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        let items = self
            .projects
            .iter()
            .enumerate()
            .map(|(id, slot)| {
                PickerItem::new(id, slot.name.clone())
                    .with_detail(slot.worktree.root().display().to_string())
            })
            .collect();
        self.open_picker(PickerMode::Projects, i18n::t!("finder.projects"), items, window, cx);
    }

    fn open_picker(
        &mut self,
        mode: PickerMode,
        placeholder: String,
        items: Vec<PickerItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme.clone();
        let accent = self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0));
        let picker = cx.new(|cx| Picker::new(placeholder, items, theme, accent, cx));
        window.focus(&picker.read(cx).focus_handle(), cx);
        self._picker_observation = Some(cx.subscribe_in(&picker, window, Self::on_picker_event));
        self.picker_mode = mode;
        self.picker = Some(picker);
        cx.notify();
    }

    fn on_picker_event(
        &mut self,
        _picker: &Entity<Picker>,
        event: &PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PickerEvent::Confirmed(id) => {
                let id = *id;
                let mode = self.picker_mode;
                self.close_picker(window, cx);
                match mode {
                    PickerMode::Files => {
                        if let Some(path) = self.picker_files.get(id).cloned() {
                            self.open_file(path, window, cx);
                        }
                    }
                    PickerMode::Projects => self.switch_project(id, window, cx),
                }
            }
            PickerEvent::Dismissed => self.close_picker(window, cx),
        }
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self._picker_observation = None;
        match &self.editor {
            Some(editor) => {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => window.focus(&self.focus_handle, cx),
        }
        cx.notify();
    }

    // ── 描画 ──

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.active;
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_none()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .pt_2()
            .children(self.projects.iter().enumerate().map(|(index, slot)| {
                let color = slot.color;
                let is_active = index == active;
                let monogram = slot.name.chars().next().unwrap_or('•').to_string();
                let name = slot.name.clone();
                div()
                    .id(("rail-project", index))
                    .size(px(30.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg0)
                    .bg(color.alpha(0.14))
                    .border_2()
                    .border_color(if is_active { color } else { color.alpha(0.35) })
                    .cursor_pointer()
                    // 非アクティブは hover で色が濃くなる＝クリックできる合図（Zed の気持ちよさ）
                    .hover(|style| style.bg(color.alpha(0.24)).border_color(color))
                    .child(monogram)
                    .tooltip(Tooltip::text(name, theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.switch_project(index, window, cx)),
                    )
            }))
            // ＋ = プロジェクト/ブランチスイッチャー（⌘O）
            .child(
                div()
                    .id("rail-add")
                    .size(px(30.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.fg2)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .child("＋")
                    .hover(|style| style.text_color(theme.fg0).border_color(theme.fg2).bg(theme.bg2))
                    .tooltip(Tooltip::text("プロジェクトを開く  ⌘O", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.open_project_switcher(&ProjectSwitcher, window, cx)
                        }),
                    ),
            )
            .child(div().flex_1())
            .child(div().pb_1().text_size(px(9.)).text_color(theme.fg2).child("⌘O"))
    }

    fn render_explorer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let Some(slot) = self.active_slot() else {
            return div()
                .w(px(DOCK_WIDTH))
                .h_full()
                .flex_none()
                .bg(theme.bg0)
                .border_r_1()
                .border_color(theme.border);
        };
        // 表示モードで本体を切り替える。カラム表示は複数カラム分だけ横幅を広げる（Finder 風）。
        let body = match self.explorer_view {
            ExplorerView::Tree => self.render_tree(slot, cx),
            ExplorerView::Columns => self.render_columns(slot, cx),
            ExplorerView::Icons => self.render_icons(slot, cx),
        };
        div()
            .w(px(self.explorer_width))
            .h_full()
            .flex_none()
            .relative() // リサイズハンドルの絶対配置基準
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_explorer_header(slot, cx))
            .child(body)
            .child(self.render_explorer_footer(cx))
            // 右縁のリサイズハンドル（ドラッグで幅変更）。
            .child(
                div()
                    .id("explorer-resize")
                    .absolute()
                    .top_0()
                    .right(px(0.))
                    .w(px(RESIZE_HANDLE_WIDTH))
                    .h_full()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(|style| style.bg(theme.border))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.resizing_explorer = true;
                            this.resize_start_x = f32::from(event.position.x);
                            this.resize_start_width = this.explorer_width;
                            cx.notify();
                        }),
                    ),
            )
    }

    /// ツリー表示（縦。従来）。行 = chevron + アイコン + 名前。
    fn render_tree(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let color = slot.color;
        let selected = slot.selected.clone();
        div()
            .flex_1()
            .overflow_hidden()
            .children(slot.rows.iter().enumerate().map(|(index, row)| {
                let path = row.path.clone();
                let is_dir = row.is_dir;
                let is_selected = selected.as_ref() == Some(&row.path);
                let chevron = if row.is_dir {
                    if row.is_expanded { "▾" } else { "▸" }
                } else {
                    ""
                };
                div()
                    .id(("tree-row", index))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(ROW_HEIGHT))
                    .pr_2()
                    .pl(px(6.0 + row.depth as f32 * INDENT))
                    .text_size(px(12.5))
                    .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .when(is_selected, |element| {
                        element.bg(theme.bg3).border_l_2().border_color(color)
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(9.))
                            .text_size(px(9.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(chevron.to_string())),
                    )
                    .child(file_icon(&row.name, is_dir, &theme))
                    .child(row.name.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.toggle_dir(path.clone(), cx);
                            } else {
                                this.open_file(path.clone(), window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let path = row.path.clone();
                            move |this, event: &MouseDownEvent, _window, cx| {
                                this.show_context_menu(path.clone(), is_dir, event.position, cx)
                            }
                        }),
                    )
            }))
            .into_any_element()
    }

    /// アイコングリッド表示（現在フォルダの直下。フォルダはクリックで中に入る・ファイルは開く）。
    fn render_icons(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let dir = slot.current_dir.clone().unwrap_or_else(|| slot.worktree.root().to_path_buf());
        let entries = slot.worktree.read_dir(&dir).unwrap_or_default();
        let selected = slot.selected.clone();
        div()
            .flex_1()
            .overflow_hidden()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(2.))
            .p(px(6.))
            .children(entries.into_iter().enumerate().map(|(index, entry)| {
                let is_dir = entry.is_dir;
                let path = entry.path.clone();
                let is_selected = selected.as_ref() == Some(&entry.path);
                div()
                    .id(("icon-cell", index))
                    .w(px(84.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .py(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .when(is_selected, |element| element.bg(theme.bg3))
                    // 大きめアイコン（グリッド用に 2 倍スケール）
                    .child(div().flex().items_center().justify_center().h(px(30.)).child(icon_large(&entry.name, is_dir, &theme)))
                    .child(
                        div()
                            .max_w_full()
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .text_center()
                            .overflow_hidden()
                            .child(SharedString::from(entry.name.clone())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.enter_dir(path.clone(), cx);
                            } else {
                                this.open_file(path.clone(), window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let path = entry.path.clone();
                            move |this, event: &MouseDownEvent, _window, cx| {
                                this.show_context_menu(path.clone(), is_dir, event.position, cx)
                            }
                        }),
                    )
            }))
            .into_any_element()
    }

    /// Finder のカラム表示（Miller columns）。ルート→現在フォルダの各段をカラムで並べる。
    fn render_columns(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let root = slot.worktree.root().to_path_buf();
        let current = slot.current_dir.clone().unwrap_or_else(|| root.clone());
        // ルート → current の連鎖（各段がカラムになる）。
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut walk = current.as_path();
        loop {
            chain.push(walk.to_path_buf());
            if walk == root {
                break;
            }
            match walk.parent() {
                Some(parent) if parent.starts_with(&root) || parent == root => walk = parent,
                _ => break,
            }
        }
        chain.reverse();
        // 460px に収まるよう末尾 3 段（＝現在フォルダ + 親 2 つ）だけ見せる。
        let visible_start = chain.len().saturating_sub(3);

        div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .children(chain.iter().enumerate().skip(visible_start).map(|(column_index, dir)| {
                let entries = slot.worktree.read_dir(dir).unwrap_or_default();
                // このカラムで選択中（＝連鎖の次の段）のパス。
                let selected_child = chain.get(column_index + 1).cloned();
                div()
                    .w(px(150.))
                    .flex_none()
                    .h_full()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(theme.border)
                    .children(entries.into_iter().enumerate().map(|(row_index, entry)| {
                        let is_dir = entry.is_dir;
                        let path = entry.path.clone();
                        let on_path = selected_child.as_ref() == Some(&entry.path);
                        div()
                            .id(("col", column_index * 1000 + row_index))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .h(px(ROW_HEIGHT))
                            .px(px(7.))
                            .text_size(px(12.))
                            .text_color(if on_path { theme.fg0 } else { theme.fg1 })
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .when(on_path, |element| element.bg(theme.bg3))
                            .child(file_icon(&entry.name, is_dir, &theme))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(SharedString::from(entry.name.clone())),
                            )
                            // フォルダは中に入る合図の ›
                            .when(is_dir, |element| {
                                element.child(div().flex_none().text_size(px(9.)).text_color(theme.fg2).child("›"))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if is_dir {
                                        this.enter_dir(path.clone(), cx);
                                    } else {
                                        this.open_file(path.clone(), window, cx);
                                    }
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener({
                                    let path = entry.path.clone();
                                    move |this, event: &MouseDownEvent, _window, cx| {
                                        this.show_context_menu(path.clone(), is_dir, event.position, cx)
                                    }
                                }),
                            )
                    }))
            }))
            .into_any_element()
    }

    /// エクスプローラ上部のブレッドクラム。プロジェクト名 → 現在フォルダまでの各段を**クリックで上へ**
    /// たどれる（up-nav）。カラム/アイコン表示で「戻る」手段になる。
    fn render_explorer_header(&self, slot: &ProjectSlot, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let root = slot.worktree.root().to_path_buf();
        let current = slot.current_dir.clone().unwrap_or_else(|| root.clone());
        let mut crumbs: Vec<(SharedString, PathBuf)> = vec![(slot.name.clone(), root.clone())];
        if let Ok(relative) = current.strip_prefix(&root) {
            let mut accumulated = root.clone();
            for segment in relative.components() {
                accumulated = accumulated.join(segment.as_os_str());
                crumbs.push((
                    SharedString::from(segment.as_os_str().to_string_lossy().to_string()),
                    accumulated.clone(),
                ));
            }
        }
        let last = crumbs.len().saturating_sub(1);
        let mut header = div()
            .flex()
            .items_center()
            .h(px(28.))
            .px(px(6.))
            .flex_none()
            .text_size(px(11.))
            .text_color(theme.fg2)
            .overflow_hidden()
            .border_b_1()
            .border_color(theme.border);
        for (index, (label, path)) in crumbs.into_iter().enumerate() {
            if index > 0 {
                header = header.child(div().px(px(1.)).text_color(theme.fg2).child("›"));
            }
            let is_current = index == last;
            header = header.child(
                div()
                    .id(("crumb", index))
                    .px(px(3.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(is_current, |element| {
                        element.font_weight(FontWeight::SEMIBOLD).text_color(theme.fg0)
                    })
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.enter_dir(path.clone(), cx)),
                    ),
            );
        }
        header
    }

    /// エクスプローラ下部の表示モード切替（ツリー / カラム / アイコン）。
    fn render_explorer_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let current = self.explorer_view;
        let button = |view: ExplorerView, id: &'static str, glyph: &'static str, tip: &'static str| {
            let active = current == view;
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(if active { theme.fg0 } else { theme.fg2 })
                .cursor_pointer()
                .when(active, |element| element.bg(theme.bg3))
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(glyph)
                .tooltip(Tooltip::text(tip, theme.clone()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| this.set_explorer_view(view, cx)),
                )
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(30.))
            .px(px(6.))
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .child(button(ExplorerView::Tree, "view-tree", "☰", "ツリー表示"))
            .child(button(ExplorerView::Columns, "view-columns", "▥", "カラム表示（Finder 風）"))
            .child(button(ExplorerView::Icons, "view-icons", "▦", "アイコン表示"))
    }

    /// エクスプローラの右クリックメニュー（開いていれば）。フォルダ=新規ウィンドウで開く 等。
    /// 背後に透明バックドロップを敷き、外側クリックで閉じる。
    fn render_explorer_context_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.explorer_context_menu.as_ref()?;
        let (bg2, bg3, border, fg1, fg0) =
            (self.theme.bg2, self.theme.bg3, self.theme.border, self.theme.fg1, self.theme.fg0);
        let path = menu.path.clone();
        let is_dir = menu.is_dir;
        let position = menu.position;

        let item = move |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(fg1)
                .cursor_pointer()
                .hover(move |style| style.bg(bg3).text_color(fg0))
                .child(label)
        };

        let mut menu_box = div()
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(210.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
            ]);

        if is_dir {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open-window", "新規ウィンドウで開く").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.open_folder_as_window(open_path.clone(), cx)),
            ));
            let enter_path = path.clone();
            menu_box = menu_box.child(item("ctx-enter", "このフォルダを開く").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.enter_dir(enter_path.clone(), cx);
                    this.hide_context_menu(cx);
                }),
            ));
        } else {
            let open_path = path.clone();
            menu_box = menu_box.child(item("ctx-open", "開く").on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_file(open_path.clone(), window, cx);
                    this.hide_context_menu(cx);
                }),
            ));
        }
        let copy_path = path.clone();
        menu_box = menu_box.child(item("ctx-copy", "パスをコピー").on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| this.copy_path(&copy_path, cx)),
        ));

        // 透明バックドロップ（外側クリックで閉じる）。メニューはその子（最前面）。
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_context_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.hide_context_menu(cx)),
                )
                .child(menu_box)
                .into_any_element(),
        )
    }

    /// アクティブプロジェクト色（無ければパレット先頭）。titlebar ピル左縁・タブ上線に流す。
    fn accent(&self) -> Hsla {
        self.active_slot().map(|slot| slot.color).unwrap_or_else(|| project_color(0))
    }

    // ── titlebar（UI-SPEC §3） ──

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("titlebar")
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .gap_2()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .pl(px(TRAFFIC_LIGHT_INSET)) // ネイティブ信号機を避ける
            .pr_2()
            .border_b_1()
            .border_color(theme.border)
            // 窓ドラッグ: down→move で開始（クリックと区別）。ダブルクリックで zoom（Zed 準拠）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.should_move_window = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.should_move_window = false),
            )
            .on_mouse_down_out(cx.listener(|this, _, _window, _cx| this.should_move_window = false))
            .on_mouse_move(cx.listener(|this, _, window, _cx| {
                if this.should_move_window {
                    this.should_move_window = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _cx| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(self.render_project_pill(cx))
            .child(div().flex_1().h_full()) // 空き＝ドラッグ領域（titlebar 全体で処理）
            // 実行中スレッドの beacon（窓上部から常に見える＝方向感覚の核・UI-SPEC §3）
            .child(self.render_beacons(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(self.dock_button(Dock::Left, self.show_left, cx))
                    .child(self.dock_button(Dock::Bottom, self.show_bottom, cx))
                    .child(self.dock_button(Dock::Right, self.show_right, cx)),
            )
    }

    /// titlebar の beacon 列（アクティブプロジェクトのスレッド。実行中は色濃く・停止中は淡く）。
    fn render_beacons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let beacons = self.agent_panel.read(cx).beacons();
        div()
            .flex()
            .items_center()
            .gap_3()
            .mr_2()
            .children(beacons.into_iter().enumerate().map(|(index, (name, color, running))| {
                let status = if running { "実行中" } else { "待機中" };
                let label = format!("{name} — {status}");
                div()
                    .id(("beacon-item", index))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(px(11.))
                    .text_color(if running { theme.fg1 } else { theme.fg2 })
                    .child(beacon_dot(("beacon", index), color, running))
                    .child(name)
                    .tooltip(Tooltip::text(label, theme.clone()))
            }))
    }

    /// プロジェクトピル: 枠 + 左縁 3px プロジェクト色 + 「名前 ▾」+「⎇ branch」。名前クリックで ⌘O。
    fn render_project_pill(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let name = self
            .active_slot()
            .map(|slot| slot.name.clone())
            .unwrap_or_else(|| SharedString::from("—"));
        let branch = self.active_slot().and_then(|slot| git_branch(slot.worktree.root()));

        let mut inner = div().flex().items_center().gap(px(7.)).py(px(3.)).px(px(9.)).child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg0)
                .child(format!("{name} ▾")),
        );
        if let Some(branch) = branch {
            inner = inner.child(
                div()
                    .text_color(theme.fg1)
                    .text_size(px(11.5))
                    .child(format!("⎇ {branch}")),
            );
        }

        div()
            .id("project-pill")
            .flex()
            .items_stretch()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|style| style.border_color(theme.fg2).bg(theme.bg1))
            .overflow_hidden()
            .text_size(px(12.))
            .child(div().w(px(3.)).bg(accent)) // 左縁＝プロジェクト色
            .child(inner)
            .tooltip(Tooltip::text("プロジェクト / ブランチを切替  ⌘O", theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation(); // titlebar ドラッグを起こさない
                    this.open_project_switcher(&ProjectSwitcher, window, cx)
                }),
            )
    }

    /// ドックトグルの小アイコン（枠 + 位置ストリップで左/右/下を示す）。
    fn dock_button(&self, dock: Dock, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let strip = || div().bg(theme.fg1);
        let frame = div()
            .w(px(14.))
            .h(px(11.))
            .flex()
            .border_1()
            .border_color(theme.fg2)
            .rounded(px(2.))
            .overflow_hidden();
        let icon = match dock {
            Dock::Left => frame.child(strip().w(px(4.)).h_full()),
            Dock::Right => frame.justify_end().child(strip().w(px(4.)).h_full()),
            Dock::Bottom => frame.flex_col().justify_end().child(strip().h(px(3.5)).w_full()),
        };
        let (id, label): (&'static str, &'static str) = match dock {
            Dock::Left => ("dock-left", "左パネル（エクスプローラ）を表示/隠す"),
            Dock::Bottom => ("dock-bottom", "下パネル（ターミナル等）を表示/隠す"),
            Dock::Right => ("dock-right", "右パネル（エージェント）を表示/隠す"),
        };
        div()
            .id(id)
            .w(px(26.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .when(active, |element| element.bg(theme.bg3))
            .hover(|style| style.bg(theme.bg3))
            .child(icon)
            .tooltip(Tooltip::text(label, theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    cx.stop_propagation(); // titlebar ドラッグを起こさない
                    this.toggle_dock(dock, cx)
                }),
            )
    }

    // ── タブ列・パンくず（UI-SPEC §5） ──

    fn render_tabstrip(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let view = editor.read(cx);
        let name = view
            .buffer()
            .path()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "無題".to_string());
        let dirty = view.buffer().is_dirty();
        // ファイルが変わったら（キーが変わる）タブが一度だけ fade-in する。
        let tab_key = SharedString::from(format!("editor-tab-appear-{name}"));

        let tab = div()
            .id("editor-tab")
            .flex()
            .flex_col()
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.bg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            // アクティブタブ上線 = プロジェクト色（UI-SPEC §5）
            .child(div().h(px(2.)).w_full().bg(accent))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(14.))
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .when(dirty, |element| {
                        element.child(div().size(px(7.)).rounded(px(3.5)).bg(theme.warn))
                    })
                    .child(SharedString::from(name))
                    .child(
                        div()
                            .id("close-tab")
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.fg0))
                            .child("×")
                            .tooltip(Tooltip::text("閉じる  ⌘W", theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| this.close_active_editor(cx)),
                            ),
                    ),
            );

        div()
            .flex()
            .items_stretch()
            .h(px(TABSTRIP_HEIGHT))
            .flex_none()
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            // 新しいファイルを開くとタブがすっと現れる（key=ファイル名。oneshot＝idle 0% 維持）。
            .child(tab.with_animation(
                tab_key,
                Animation::new(std::time::Duration::from_millis(200))
                    .with_easing(gpui::ease_out_quint()),
                |element, delta| element.opacity(delta),
            ))
            .child(div().flex_1())
    }

    fn render_breadcrumb(&self, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let path = editor.read(cx).buffer().path().map(Path::to_path_buf);
        let root = self.active_slot().map(|slot| slot.worktree.root().to_path_buf());
        let crumbs = breadcrumb_text(root.as_deref(), path.as_deref());
        div()
            .flex()
            .items_center()
            .h(px(BREADCRUMB_HEIGHT))
            .px(px(14.))
            .flex_none()
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg2)
            .child(SharedString::from(crumbs))
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let center = div().flex_1().flex().flex_col().min_w_0().bg(theme.bg1);
        match self.editor.clone() {
            Some(editor) => center
                .child(self.render_tabstrip(&editor, cx))
                .child(self.render_breadcrumb(&editor, cx))
                .child(div().flex_1().overflow_hidden().child(editor)),
            None => center
                .items_center()
                .justify_center()
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("editor.empty_hint"))),
        }
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let branch = self.active_slot().and_then(|slot| git_branch(slot.worktree.root()));
        let (cursor, language) = match &self.editor {
            Some(editor) => {
                let view = editor.read(cx);
                let (row, column) = view.cursor_display();
                (Some(format!("{row}:{column}")), view.language_label())
            }
            None => (None, None),
        };

        let left = div()
            .flex()
            .items_center()
            .gap_3()
            .when_some(branch, |element, branch| {
                element.child(SharedString::from(format!("⎇ {branch}")))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(theme.err).child("✗ 0"))
                    .child(div().text_color(theme.warn).child("▲ 0")),
            );

        let right = div()
            .flex()
            .items_center()
            .gap_3()
            .when_some(cursor, |element, cursor| element.child(SharedString::from(cursor)))
            .child(SharedString::from("UTF-8"))
            .when_some(language, |element, language| element.child(language));

        div()
            .flex()
            .items_center()
            .h(px(STATUSBAR_HEIGHT))
            .px_3()
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .child(left)
            .child(div().flex_1())
            .child(right)
    }
}

/// プロジェクトルート（またはその祖先）の `.git/HEAD` から現在ブランチ名を読む。無ければ `None`。
/// M8 の本格 VCS crate までの暫定（`ref: refs/heads/<branch>` の 1 行を解釈するだけ）。
fn git_branch(root: &Path) -> Option<String> {
    let mut dir = Some(root);
    while let Some(current) = dir {
        let head = current.join(".git/HEAD");
        if let Ok(content) = std::fs::read_to_string(&head) {
            let branch = content.trim().strip_prefix("ref: refs/heads/")?;
            return Some(branch.to_string());
        }
        dir = current.parent();
    }
    None
}

/// パンくず文字列。ファイルがプロジェクト配下ならルート相対の各要素を ` › ` で連結。
/// 配下でなければファイル名のみ。
fn breadcrumb_text(root: Option<&Path>, path: Option<&Path>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let relative = match root.and_then(|root| path.strip_prefix(root).ok()) {
        Some(relative) => relative,
        None => {
            return path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
        }
    };
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" › ")
}

/// パスの末尾 `max` 階層のフォルダ名（ルート `/` は除く）。エクスプローラの上位階層ブレッドクラム用。
/// titlebar beacon のドット。実行中は breathing で pulse（停止中は淡色・静止）。
fn beacon_dot(id: impl Into<gpui::ElementId>, color: Hsla, running: bool) -> gpui::AnyElement {
    let base = if running { color } else { color.alpha(0.5) };
    let dot = div().size(px(8.)).rounded(px(4.)).flex_none().bg(base);
    if running {
        dot.with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(1600))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            |element, delta| element.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

/// 拡張子別のアイコン色（識別用。theme の syntax パレットを流用＝色による方向感覚）。
fn file_type_color(name: &str, theme: &Theme) -> Hsla {
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let syntax = &theme.syntax;
    match extension.as_str() {
        "rs" => syntax.number,                                          // オレンジ
        "toml" | "yaml" | "yml" | "json" | "lock" | "ini" => syntax.function, // 青
        "md" | "markdown" | "txt" | "log" => theme.fg2,                 // ミュート
        "ts" | "tsx" | "js" | "mjs" | "cjs" | "jsx" => syntax.type_,    // 黄
        "py" | "rb" | "go" | "sh" | "zsh" | "bash" => syntax.string,    // 緑
        "html" | "htm" | "css" | "scss" | "vue" => syntax.keyword,      // 紫
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => syntax.macro_, // シアン
        _ => theme.fg1,
    }
}

/// エクスプローラのファイル/フォルダアイコン。フォルダ＝横長（folder 色）・ファイル＝縦長（型色）の
/// シルエットで一目で見分く。固定幅スロットに入れて名前を揃える。
fn file_icon(name: &str, is_dir: bool, theme: &Theme) -> impl IntoElement {
    let shape = if is_dir {
        let color = theme.folder_icon();
        div().w(px(13.)).h(px(10.)).rounded(px(2.5)).bg(color)
    } else {
        let color = file_type_color(name, theme);
        div().w(px(10.)).h(px(13.)).rounded(px(2.)).bg(color.alpha(0.9))
    };
    div().flex_none().w(px(16.)).flex().items_center().justify_center().child(shape)
}

/// アイコングリッド用の大きめアイコン（[`file_icon`] の 2 倍強）。
fn icon_large(name: &str, is_dir: bool, theme: &Theme) -> impl IntoElement {
    let shape = if is_dir {
        div().w(px(30.)).h(px(23.)).rounded(px(4.)).bg(theme.folder_icon())
    } else {
        let color = file_type_color(name, theme);
        div().w(px(22.)).h(px(28.)).rounded(px(3.5)).bg(color.alpha(0.9))
    };
    div().flex().items_center().justify_center().child(shape)
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.editor {
            Some(editor) => editor.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_file_finder))
            .on_action(cx.listener(Self::open_project_switcher))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::new_agent_thread))
            .on_mouse_move(cx.listener(Self::on_resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_resize_end))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg0)
            .text_color(theme.fg0)
            .font_family("IBM Plex Sans JP") // UI = IBM Plex Sans JP（bin で bundle 済み）
            .text_size(px(12.5))
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .when(self.show_left, |element| element.child(self.render_explorer(cx)))
                    .child(self.render_center(cx))
                    .when(self.show_right, |element| element.child(self.render_agent_dock(cx))),
            )
            .child(self.render_statusbar(cx))
            // オーバーレイ（最前面）
            .when_some(self.picker.clone(), |this, picker| this.child(picker))
            .children(self.render_explorer_context_menu(cx))
    }
}
