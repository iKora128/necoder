//! 変更レビュー: worktree の変更を 1 画面で読むビュー。
//!
//! 比較の基準（Task の base / HEAD / ブランチの分岐点 / 任意のコミット）から作業ツリーまでの
//! 全ファイルの diff を縦に並べる。左にファイルツリー、+/− の行は ok / err の薄い面、旧・新の
//! 行番号の 2 列、`crates/lang` のハイライタで旧・新の全文を解析して行に割り当てる。変更の無い
//! 領域は「⋯ N 行」に畳み、押すと前後 20 行ずつ開く。git と解析は背景で走らせ、UI を止めない。
//!
//! エディタのコア（`editor_view`）は拡張しない。行の間にブロックを差し込む API も任意色の行背景も
//! 無いので、可変高の仮想リスト（`ListState`）に「ファイル見出し / 行 / 畳んだ領域」を並べる
//! 独立したビューにした（ARCHITECTURE §1 の依存方向 = コアは機能を知らない）。
//! Workspace がエディタのタブ・Fleet の Task カードの「変更」タブに載せる。
//!
//! 行コメント（注記）: 行にホバーすると左端に ＋、押すか選択行で `c` → その行の下に Markdown の
//! 入力欄（`EditorView` の平坦モード = IME 安全・⌘⏎ で保存・Esc で取り消し）。⇧クリックかドラッグで
//! 複数行。注記は storage に残し（再起動しても消えない）、下端のトレイから 1 通のプロンプトに束ねて
//! 宛先のスレッドへ送る（宛先の一覧と送信は Workspace が持つ＝このビューは AI を知らない）。
//!
//! 参考: stablyai/orca@646e9a5 の `docs/site/content/docs/review/diff-viewer.mdx`（比較基準の切替・
//! ファイルツリー・畳み・空白の扱い）と `docs/site/content/docs/review/annotate-ai-diff.mdx`
//! （＋ / `c` で行に注記・まとめて送る・Resolve と未解決の再送）。機能の比較だけで、コードは写していない。

mod annotations;
mod notes;
mod rows;
mod syntax;

pub use notes::{
    format_prompt, DiffLinesTarget, ExcerptKind, ExcerptLine, NoteSide, NoteTarget, ReviewNote,
};
pub use rows::{
    build_rows, build_tree, expand_gap, file_gaps, Gap, GapExpansion, GapPosition, Notice, Row,
    RowInputs, RowNote, TreeEntry, EXPAND_STEP,
};
pub use syntax::{highlight_text, split_highlights, FileSyntax, HighlightedText};

use editor_view::EditorView;
use gpui::{
    div, list, prelude::*, px, App, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, FontWeight, HighlightStyle, Hsla, KeyDownEvent, ListAlignment, ListOffset,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, SharedString, StyledText, Window,
};
use host::Host;
use project::review::{
    DiffLineKind, FileChangeKind, FileDiff, ReviewBase, ReviewDiff, ReviewError,
};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage::{ReviewNoteState, Storage};
use theme_core::Theme;
use ui::Tooltip;

/// コードの書体（エディタと同じ等幅）。
const CODE_FONT: &str = "Guguru Sans Code";
const CODE_FONT_SIZE: f32 = 12.5;
const ROW_MIN_HEIGHT: f32 = 20.;
const NUMBER_COLUMN_WIDTH: f32 = 42.;
const MARKER_COLUMN_WIDTH: f32 = 16.;
const TOOLBAR_HEIGHT: f32 = 34.;
const TREE_WIDTH: f32 = 220.;
/// タブは 4 桁の空白に開く（行の折り返しと等幅の揃えを崩さない）。
const TAB_SPACES: &str = "    ";
/// 追加 / 削除の行の面の濃さ（ok / err の低い透明度・UI-SPEC §14）。
const LINE_TINT: f32 = 0.12;
/// 基準のメニューに出すコミットの件数。
const MENU_COMMITS: usize = 30;
/// 全文とハイライトを一度に背景で計算するファイル数（届いた分から色が付く）。
const SYNTAX_CHUNK: usize = 8;
/// 下端のトレイの帯の高さ。
const TRAY_HEIGHT: f32 = 34.;

/// Workspace が渡す「どこを・何と比べるか」。
#[derive(Clone)]
pub struct ReviewContext {
    pub host: Arc<dyn Host>,
    /// プロジェクト（worktree）のルート。差分は含むリポジトリ全体を見る。
    pub root: PathBuf,
    /// Task の base（Fleet の Task だけ）。あればこれが既定の基準、無ければ HEAD。
    pub task_base: Option<String>,
    /// プロジェクト色（選択の左バー・トグルの薄い面に使う・UI-SPEC §1.3）。
    pub accent: Hsla,
    /// 注記の保存先（永続化しない窓は `None`＝その窓の間だけ持つ）。
    pub storage: Option<Storage>,
    /// 注記を束ねる単位（Fleet = Task・Editor = プロジェクト。どちらも TaskSpace の id）。
    pub scope: String,
}

/// 注記を送る宛先（一覧は Workspace が作る。このビューは選ばれたものをそのまま返すだけ）。
///
/// 宛先は**安定した id** で持つ（R07）。メニューを開いてから選ぶまでにスレッドが閉じられたり
/// 並び替わったりしても、添字のずれで別のスレッドへ送らない。id から今の添字を引くのは
/// 送る直前の Workspace で、引けなければ注記を送信済みにせず、メニューを開き直す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTarget {
    /// 宛先のパネル（AgentPanel の Entity id・Workspace が解釈する）。
    pub panel: EntityId,
    /// パネルの中のスレッドの永続 id。`None` = 新しいスレッドを作って送る。
    pub thread: Option<SharedString>,
    pub label: SharedString,
    pub detail: SharedString,
    /// スレッド色（宛先のドット・UI-SPEC §1.3）。
    pub color: Hsla,
    /// 既定の宛先（Fleet = Task のスレッド / Editor = アクティブなスレッド）。
    pub is_default: bool,
}

/// Workspace へ上げる通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewEvent {
    /// ファイルをエディタで開く（`line` は 1 始まり）。
    OpenFile { path: PathBuf, line: Option<u32> },
    /// 注記の宛先を選ぶメニューを開きたい（宛先の一覧を [`ReviewView::show_send_menu`] で渡す）。
    /// `resend` = 未解決（送信済みを含む）をもう一度送る。
    SendMenuRequested { resend: bool },
    /// 注記を 1 通のプロンプトにまとめて送る。届いたら [`ReviewView::mark_notes_sent`] を呼ぶ。
    /// `resend` は選んだメニューの種類（届かなかった時に同じ種類で開き直すため）。
    SendNotes {
        target: SendTarget,
        prompt: String,
        note_ids: Vec<String>,
        resend: bool,
    },
}

/// 行の選択（リストの行の添字・同じファイルの中だけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineSelection {
    anchor: usize,
    head: usize,
}

/// 書きかけの注記（入力欄は行の下に出る）。
struct NoteDraft {
    target: DiffLinesTarget,
    editor: Entity<EditorView>,
    /// 既存の注記を直している（その id）。
    editing: Option<String>,
}

/// 宛先のメニュー（Workspace が一覧を渡した時だけ開く）。
struct SendMenu {
    targets: Vec<SendTarget>,
    resend: bool,
}

/// 読み込みの状態。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Load {
    /// まだ一度も開いていない（表示されるまで git を叩かない）。
    Idle,
    Loading,
    Ready,
    Failed(ReviewError),
}

/// 基準の選択メニュー。ブランチとコミットは開いた時に背景で読む。
struct BaseMenu {
    branches: Option<Vec<String>>,
    commits: Option<Vec<(String, String)>>,
}

/// スクロール位置を読み直しの前後で保つための「どの行か」。
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowAnchor {
    File(String),
    Line {
        path: String,
        old: Option<u32>,
        new: Option<u32>,
    },
    Fold {
        path: String,
        gap: usize,
    },
    Note(String),
    Draft,
}

pub struct ReviewView {
    theme: Theme,
    accent: Hsla,
    focus_handle: FocusHandle,
    context: Option<ReviewContext>,
    base: ReviewBase,
    /// ユーザーが基準を選んだ（以後 Task の base の更新で上書きしない）。
    base_chosen: bool,
    ignore_whitespace: bool,
    load: Load,
    diff: Option<Arc<ReviewDiff>>,
    /// ファイルごとの全文ハイライト（`diff.files` と同じ添字・届いた分だけ `Some`）。
    syntax: Vec<Option<Arc<FileSyntax>>>,
    rows: Vec<Row>,
    list: ListState,
    expansions: HashMap<(String, usize), GapExpansion>,
    collapsed_files: HashSet<String>,
    collapsed_dirs: HashSet<String>,
    /// 「見た」印（パス）。読み直しても残す。
    reviewed: HashSet<String>,
    show_tree: bool,
    /// ファイルツリーで選んだファイル（押した直後はスクロール位置より優先して光らせる）。
    picked_file: Option<usize>,
    /// 作業ツリーが変わった（「新しい変更があります」を出す）。
    outdated: bool,
    base_menu: Option<BaseMenu>,
    /// 読み込みの世代（古い結果を捨てる）。
    generation: u64,
    /// 注記（作った順）。束ねる単位（`context.scope`）の分だけ。
    notes: Vec<ReviewNote>,
    /// `notes` を読んだ単位（切り替わったら読み直す）。
    notes_scope: Option<String>,
    selection: Option<LineSelection>,
    /// ドラッグで範囲を選んでいる最中。
    selecting: bool,
    draft: Option<NoteDraft>,
    /// 下端のトレイの一覧を開いているか。
    tray_open: bool,
    send_menu: Option<SendMenu>,
}

impl EventEmitter<ReviewEvent> for ReviewView {}

impl Focusable for ReviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ReviewView {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let accent = theme.fg1;
        Self {
            theme,
            accent,
            focus_handle: cx.focus_handle(),
            context: None,
            base: ReviewBase::Head,
            base_chosen: false,
            ignore_whitespace: false,
            load: Load::Idle,
            diff: None,
            syntax: Vec::new(),
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            expansions: HashMap::new(),
            collapsed_files: HashSet::new(),
            collapsed_dirs: HashSet::new(),
            reviewed: HashSet::new(),
            show_tree: true,
            picked_file: None,
            outdated: false,
            base_menu: None,
            generation: 0,
            notes: Vec::new(),
            notes_scope: None,
            selection: None,
            selecting: false,
            draft: None,
            tray_open: false,
            send_menu: None,
        }
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// 比べる場所を渡す（表示する前に毎回呼ぶ）。場所が変わったら中身を捨て、Task の base が
    /// 後から届いた（台帳の復元）だけなら、ユーザーが選んでいない限り既定の基準を差し替える。
    pub fn set_context(&mut self, context: ReviewContext, cx: &mut Context<Self>) {
        let same_place = self.context.as_ref().is_some_and(|current| {
            current.host.id() == context.host.id() && current.root == context.root
        });
        let base_moved = self
            .context
            .as_ref()
            .is_some_and(|current| current.task_base != context.task_base);
        self.accent = context.accent;
        let default_base = default_base(&context);
        self.context = Some(context);
        if !same_place {
            self.generation += 1;
            self.base = default_base;
            self.base_chosen = false;
            self.load = Load::Idle;
            self.diff = None;
            self.syntax.clear();
            self.rows.clear();
            self.list.reset(0);
            self.expansions.clear();
            self.collapsed_files.clear();
            self.reviewed.clear();
            self.outdated = false;
            self.base_menu = None;
            self.selection = None;
            self.draft = None;
            self.send_menu = None;
        } else if base_moved && !self.base_chosen && self.base != default_base {
            self.base = default_base;
            if self.load != Load::Idle {
                self.reload(cx);
            }
        }
        self.load_notes_if_needed(cx);
        cx.notify();
    }

    /// 表示された: まだ読んでいなければ読む。
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if self.load == Load::Idle {
            self.reload(cx);
        }
    }

    /// 作業ツリーが変わった合図（読み直しはユーザーが押した時だけ＝読んでいる行を動かさない）。
    pub fn mark_outdated(&mut self, cx: &mut Context<Self>) {
        if self.load == Load::Ready && !self.outdated {
            self.outdated = true;
            cx.notify();
        }
    }

    /// 開いたか（Idle でない）。
    pub fn has_loaded(&self) -> bool {
        self.load != Load::Idle
    }

    /// 解決済みの基準（読み込み後）。
    pub fn base_oid(&self) -> Option<&str> {
        self.diff.as_ref().map(|diff| diff.base_oid.as_str())
    }

    /// いまの基準（テスト・プローブ用）。
    pub fn base(&self) -> &ReviewBase {
        &self.base
    }

    /// 並んでいるファイル数（テスト・プローブ用）。
    pub fn file_count(&self) -> usize {
        self.diff.as_ref().map_or(0, |diff| diff.files.len())
    }

    /// 基準を選ぶ（メニュー・プローブ）。
    pub fn select_base(&mut self, base: ReviewBase, cx: &mut Context<Self>) {
        self.base_menu = None;
        self.base_chosen = true;
        if self.base != base || self.load != Load::Ready {
            self.base = base;
            self.reload(cx);
        }
        cx.notify();
    }

    /// 空白を無視しているか。
    pub fn ignores_whitespace(&self) -> bool {
        self.ignore_whitespace
    }

    /// 空白を無視するか（`git diff -w`）。
    pub fn set_ignore_whitespace(&mut self, ignore: bool, cx: &mut Context<Self>) {
        if self.ignore_whitespace != ignore {
            self.ignore_whitespace = ignore;
            self.reload(cx);
        }
    }

    /// 全ての畳みを 1 段開く（プローブ・撮影で「開いた状態」を作る）。
    pub fn expand_first_fold(&mut self, cx: &mut Context<Self>) {
        let target = self.rows.iter().find_map(|row| match row {
            Row::Fold {
                file,
                gap,
                expandable: true,
                ..
            } => Some((*file, *gap)),
            _ => None,
        });
        if let Some((file, gap)) = target {
            self.expand(file, gap, cx);
        }
    }

    /// 読み直す（基準・`-w` はそのまま。開いた畳み・見た印・スクロール位置は保つ）。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(context) = self.context.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Loading;
        self.outdated = false;
        cx.notify();
        let base = self.base.clone();
        let ignore_whitespace = self.ignore_whitespace;
        cx.spawn(async move |this, cx| {
            let host = context.host.clone();
            let root = context.root.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    project::review::review_diff_on(host.as_ref(), &root, &base, ignore_whitespace)
                })
                .await;
            let loaded = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return None;
                }
                match result {
                    Ok(diff) => {
                        let diff = Arc::new(diff);
                        this.apply_diff(diff.clone(), cx);
                        Some(diff)
                    }
                    Err(error) => {
                        this.load = Load::Failed(error);
                        cx.notify();
                        None
                    }
                }
            });
            let Ok(Some(diff)) = loaded else {
                return;
            };
            // 全文とハイライトは届いた分から色を付ける（最初の表示を待たせない）。
            for start in (0..diff.files.len()).step_by(SYNTAX_CHUNK) {
                let end = (start + SYNTAX_CHUNK).min(diff.files.len());
                let chunk_diff = diff.clone();
                let host = context.host.clone();
                let syntax = cx
                    .background_executor()
                    .spawn(async move { load_syntax(host.as_ref(), &chunk_diff, start..end) })
                    .await;
                let current = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    for (offset, file_syntax) in syntax.into_iter().enumerate() {
                        if let Some(slot) = this.syntax.get_mut(start + offset) {
                            *slot = Some(Arc::new(file_syntax));
                        }
                    }
                    this.rebuild_rows();
                    cx.notify();
                    true
                });
                if !matches!(current, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_diff(&mut self, diff: Arc<ReviewDiff>, cx: &mut Context<Self>) {
        let anchor = self.scroll_anchor();
        self.syntax = vec![None; diff.files.len()];
        self.diff = Some(diff);
        self.load = Load::Ready;
        self.picked_file = None;
        self.rebuild_rows_at(anchor);
        cx.notify();
    }

    /// 行を並べ直す（スクロール位置は同じ行に保つ）。
    fn rebuild_rows(&mut self) {
        let anchor = self.scroll_anchor();
        self.rebuild_rows_at(anchor);
    }

    fn rebuild_rows_at(&mut self, anchor: Option<(RowAnchor, gpui::Pixels)>) {
        let Some(diff) = self.diff.clone() else {
            self.rows.clear();
            self.list.reset(0);
            return;
        };
        let totals: Vec<Option<u32>> = self
            .syntax
            .iter()
            .map(|syntax| {
                syntax
                    .as_ref()
                    .and_then(|syntax| syntax.new.as_ref())
                    .map(|text| text.lines.len() as u32)
            })
            .collect();
        let editing = self.draft.as_ref().and_then(|draft| draft.editing.clone());
        let mut row_notes: Vec<RowNote> = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, note)| Some(&note.id) != editing.as_ref())
            .map(|(index, note)| {
                let NoteTarget::DiffLines(target) = &note.target;
                RowNote {
                    path: target.path.clone(),
                    side: target.side,
                    line: target.end,
                    note: Some(index),
                }
            })
            .collect();
        if let Some(draft) = &self.draft {
            row_notes.push(RowNote {
                path: draft.target.path.clone(),
                side: draft.target.side,
                line: draft.target.end,
                note: None,
            });
        }
        // 選択は「どの行か」で持ち越す（行の並びが変わっても同じ行を指す）。
        let selection = self.selection.and_then(|selection| {
            Some((
                self.row_anchor(selection.anchor)?,
                self.row_anchor(selection.head)?,
            ))
        });
        self.rows = build_rows(&RowInputs {
            files: &diff.files,
            new_totals: &totals,
            expansions: &self.expansions,
            collapsed: &self.collapsed_files,
            notes: &row_notes,
        });
        self.list.reset(self.rows.len());
        self.selection = selection.and_then(|(anchor, head)| {
            Some(LineSelection {
                anchor: self.find_exact_anchor(&anchor)?,
                head: self.find_exact_anchor(&head)?,
            })
        });
        if let Some((anchor, offset)) = anchor {
            if let Some(item_ix) = self.find_anchor(&anchor) {
                self.list.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item: offset,
                });
            }
        }
    }

    fn scroll_anchor(&self) -> Option<(RowAnchor, gpui::Pixels)> {
        let top = self.list.logical_scroll_top();
        let anchor = self.row_anchor(top.item_ix)?;
        Some((anchor, top.offset_in_item))
    }

    fn row_anchor(&self, index: usize) -> Option<RowAnchor> {
        let diff = self.diff.as_ref()?;
        let row = self.rows.get(index)?;
        let path = diff.files.get(row.file())?.path.clone();
        Some(match row {
            Row::FileHeader { .. } | Row::Notice { .. } => RowAnchor::File(path),
            Row::Line { file, hunk, line } => {
                let line = diff.files[*file].hunks.get(*hunk)?.lines.get(*line)?;
                RowAnchor::Line {
                    path,
                    old: line.old_line,
                    new: line.new_line,
                }
            }
            Row::Context {
                old_line, new_line, ..
            } => RowAnchor::Line {
                path,
                old: Some(*old_line),
                new: Some(*new_line),
            },
            Row::Fold { gap, .. } => RowAnchor::Fold {
                path,
                gap: gap.index,
            },
            Row::Note { note, .. } => RowAnchor::Note(self.notes.get(*note)?.id.clone()),
            Row::Draft { .. } => RowAnchor::Draft,
        })
    }

    fn find_exact_anchor(&self, anchor: &RowAnchor) -> Option<usize> {
        (0..self.rows.len()).find(|index| self.row_anchor(*index).as_ref() == Some(anchor))
    }

    fn find_anchor(&self, anchor: &RowAnchor) -> Option<usize> {
        self.find_exact_anchor(anchor).or_else(|| match anchor {
            RowAnchor::File(path) | RowAnchor::Line { path, .. } | RowAnchor::Fold { path, .. } => {
                self.file_header_row_by_path(path)
            }
            RowAnchor::Note(_) | RowAnchor::Draft => None,
        })
    }

    fn file_header_row_by_path(&self, path: &str) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        let file = diff.files.iter().position(|file| file.path == path)?;
        self.file_header_row(file)
    }

    fn file_header_row(&self, file: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::FileHeader { file: header } if *header == file))
    }

    /// ツリーから押したファイルの見出しへ飛ぶ。
    pub fn scroll_to_file(&mut self, file: usize, cx: &mut Context<Self>) {
        if let Some(item_ix) = self.file_header_row(file) {
            self.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(0.),
            });
            self.picked_file = Some(file);
            cx.notify();
        }
    }

    fn expand(&mut self, file: usize, gap: Gap, cx: &mut Context<Self>) {
        let Some(path) = self
            .diff
            .as_ref()
            .and_then(|diff| diff.files.get(file))
            .map(|file| file.path.clone())
        else {
            return;
        };
        let key = (path, gap.index);
        let current = self.expansions.get(&key).copied().unwrap_or_default();
        self.expansions.insert(key, expand_gap(&gap, current));
        self.rebuild_rows();
        cx.notify();
    }

    fn toggle_file_collapsed(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(path) = self
            .diff
            .as_ref()
            .and_then(|diff| diff.files.get(file))
            .map(|file| file.path.clone())
        else {
            return;
        };
        if !self.collapsed_files.remove(&path) {
            self.collapsed_files.insert(path);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn toggle_reviewed(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(path) = self
            .diff
            .as_ref()
            .and_then(|diff| diff.files.get(file))
            .map(|file| file.path.clone())
        else {
            return;
        };
        // 見た = 畳んでおく（GitHub の Viewed と同じ手触り）。外したら開き直す。
        if self.reviewed.remove(&path) {
            self.collapsed_files.remove(&path);
        } else {
            self.reviewed.insert(path.clone());
            self.collapsed_files.insert(path);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn open_file(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(diff) = self.diff.as_ref() else {
            return;
        };
        let Some(entry) = diff.files.get(file) else {
            return;
        };
        if entry.kind == FileChangeKind::Deleted {
            return;
        }
        let line = entry
            .hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter())
            .find(|line| line.kind == DiffLineKind::Added)
            .and_then(|line| line.new_line)
            .or_else(|| entry.hunks.first().map(|hunk| hunk.new_span().start));
        cx.emit(ReviewEvent::OpenFile {
            path: diff.repo_root.join(&entry.path),
            line,
        });
    }

    /// 基準のメニューを開く / 閉じる（ツールバーのチップ・プローブ）。
    pub fn toggle_base_menu(&mut self, cx: &mut Context<Self>) {
        if self.base_menu.take().is_some() {
            cx.notify();
            return;
        }
        let Some(context) = self.context.clone() else {
            return;
        };
        self.base_menu = Some(BaseMenu {
            branches: None,
            commits: None,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let host = context.host.clone();
            let root = context.root.clone();
            let (branches, commits) = cx
                .background_executor()
                .spawn(async move {
                    let branches = project::git_branches_on(host.as_ref(), &root);
                    let commits = project::git_log_graph_on(host.as_ref(), &root, MENU_COMMITS)
                        .into_iter()
                        .map(|commit| (commit.short_hash, commit.summary))
                        .collect::<Vec<_>>();
                    (branches, commits)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(menu) = this.base_menu.as_mut() {
                    menu.branches = Some(branches);
                    menu.commits = Some(commits);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" {
            if self.send_menu.take().is_some() || self.base_menu.take().is_some() {
                cx.stop_propagation();
                cx.notify();
            } else if self.selection.take().is_some() {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        // 以降は一覧そのものにフォーカスがある時だけ（注記の入力欄の打鍵を奪わない）。
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt || modifiers.function {
            return;
        }
        match keystroke.key.as_str() {
            "up" => self.move_selection(-1, modifiers.shift, cx),
            "down" => self.move_selection(1, modifiers.shift, cx),
            "c" if !modifiers.shift => self.open_draft(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// 基準の表示（ツールバーのチップ・空の案内で共用）。
    fn base_label(&self) -> String {
        let resolved = self.base_oid().map(short_sha).unwrap_or_default();
        match &self.base {
            ReviewBase::Head => i18n::t!("review.base_head"),
            ReviewBase::Commit(rev) => {
                let task = self
                    .context
                    .as_ref()
                    .and_then(|context| context.task_base.as_deref());
                let sha = if resolved.is_empty() {
                    short_sha(rev)
                } else {
                    resolved
                };
                if task == Some(rev.as_str()) {
                    i18n::t!("review.base_task", "sha" => sha)
                } else {
                    i18n::t!("review.base_commit", "sha" => sha)
                }
            }
            ReviewBase::Branch(name) if resolved.is_empty() => {
                i18n::t!("review.base_branch_pending", "branch" => name)
            }
            ReviewBase::Branch(name) => {
                i18n::t!("review.base_branch", "branch" => name, "sha" => resolved)
            }
        }
    }
}

/// 1 行の描画に要るもの。
struct CodeLine<'a> {
    index: usize,
    kind: DiffLineKind,
    old_line: Option<u32>,
    new_line: Option<u32>,
    text: &'a str,
    spans: &'a [(Range<usize>, lang::HighlightKind)],
    no_newline: bool,
}

/// 選択の (上, 下)。
fn ordered(selection: LineSelection) -> (usize, usize) {
    if selection.anchor <= selection.head {
        (selection.anchor, selection.head)
    } else {
        (selection.head, selection.anchor)
    }
}

/// Task の base があればそれ、無ければ HEAD。
fn default_base(context: &ReviewContext) -> ReviewBase {
    match &context.task_base {
        Some(oid) if !oid.is_empty() => ReviewBase::Commit(oid.clone()),
        _ => ReviewBase::Head,
    }
}

/// 表示用の短い SHA（7 桁）。
pub fn short_sha(oid: &str) -> String {
    oid.chars().take(7).collect()
}

/// 背景スレッドで全文を読んでハイライトする（`range` のファイルだけ）。
fn load_syntax(host: &dyn Host, diff: &ReviewDiff, range: Range<usize>) -> Vec<FileSyntax> {
    diff.files[range]
        .iter()
        .map(|file| {
            let texts =
                project::review::review_file_texts_on(host, &diff.repo_root, &diff.base_oid, file);
            FileSyntax {
                old: texts
                    .old
                    .map(|text| highlight_text(Path::new(file.base_path()), &text)),
                new: texts
                    .new
                    .map(|text| highlight_text(Path::new(&file.path), &text)),
            }
        })
        .collect()
}

/// 行の本文を色付きの StyledText にする（タブは空白に開き、色の範囲も合わせてずらす）。
fn styled_code(
    text: &str,
    spans: &[(Range<usize>, lang::HighlightKind)],
    theme: &Theme,
) -> StyledText {
    if !text.contains('\t') {
        let highlights = spans
            .iter()
            .filter(|(range, _)| range.end <= text.len())
            .map(|(range, kind)| {
                (
                    range.clone(),
                    HighlightStyle {
                        color: Some(editor_view::syntax_color(*kind, &theme.syntax)),
                        ..Default::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        let styled = StyledText::new(SharedString::from(text.to_string()));
        return if highlights.is_empty() {
            styled
        } else {
            styled.with_highlights(highlights)
        };
    }
    // タブを開いた文字列と、元の byte 位置 → 開いた後の位置の対応。
    let mut expanded = String::with_capacity(text.len() + 8);
    let mut map = vec![0usize; text.len() + 1];
    for (index, character) in text.char_indices() {
        map[index] = expanded.len();
        if character == '\t' {
            expanded.push_str(TAB_SPACES);
        } else {
            expanded.push(character);
        }
    }
    map[text.len()] = expanded.len();
    let highlights = spans
        .iter()
        .filter(|(range, _)| range.end <= text.len())
        .map(|(range, kind)| {
            (
                map[range.start]..map[range.end],
                HighlightStyle {
                    color: Some(editor_view::syntax_color(*kind, &theme.syntax)),
                    ..Default::default()
                },
            )
        })
        .filter(|(range, _)| !range.is_empty())
        .collect::<Vec<_>>();
    let styled = StyledText::new(SharedString::from(expanded));
    if highlights.is_empty() {
        styled
    } else {
        styled.with_highlights(highlights)
    }
}

// ── 描画 ──

impl ReviewView {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let (files, additions, deletions) = self
            .diff
            .as_ref()
            .map(|diff| {
                diff.files
                    .iter()
                    .fold((0, 0, 0), |(files, additions, deletions), file| {
                        (
                            files + 1,
                            additions + file.additions,
                            deletions + file.deletions,
                        )
                    })
            })
            .unwrap_or((0, 0, 0));
        let chip = |id: &'static str, label: SharedString, active: bool| {
            div()
                .id(id)
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .h(px(22.))
                .px(px(8.))
                .rounded(px(5.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.5))
                .text_color(if active { theme.fg0 } else { theme.fg1 })
                .when(active, |chip| chip.bg(self.accent.alpha(0.16)))
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(label)
        };
        div()
            .flex_none()
            .h(px(TOOLBAR_HEIGHT))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .bg(theme.bg0)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("review-tree-toggle")
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(if self.show_tree { theme.fg0 } else { theme.fg2 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child("☰")
                    .tooltip(Tooltip::text(i18n::t!("review.tree_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.show_tree = !this.show_tree;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("review.compare"))),
            )
            .child(
                chip(
                    "review-base",
                    SharedString::from(format!("{} ▾", self.base_label())),
                    self.base_menu.is_some(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_base_menu(cx);
                    }),
                ),
            )
            .child(
                chip(
                    "review-whitespace",
                    SharedString::from(i18n::t!("review.ignore_whitespace")),
                    self.ignore_whitespace,
                )
                .tooltip(Tooltip::text(
                    i18n::t!("review.ignore_whitespace_tip"),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        let ignore = !this.ignore_whitespace;
                        this.set_ignore_whitespace(ignore, cx);
                    }),
                ),
            )
            .child(div().flex_1())
            .when(self.load == Load::Loading, |bar| {
                bar.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("review.loading"))),
                )
            })
            .when(self.diff.is_some(), |bar| {
                bar.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.5))
                        .child(div().text_color(theme.fg1).child(SharedString::from(
                            i18n::t!("review.files", "count" => files),
                        )))
                        .child(
                            div()
                                .text_color(theme.ok)
                                .child(SharedString::from(format!("+{additions}"))),
                        )
                        .child(
                            div()
                                .text_color(theme.err)
                                .child(SharedString::from(format!("−{deletions}"))),
                        ),
                )
            })
            .child(
                div()
                    .id("review-refresh")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .h(px(22.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .text_color(if self.outdated { theme.fg0 } else { theme.fg2 })
                    .when(self.outdated, |button| {
                        button.border_1().border_color(theme.border)
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child("↻")
                    .when(self.outdated, |button| {
                        button.child(SharedString::from(i18n::t!("review.outdated")))
                    })
                    .tooltip(Tooltip::text(i18n::t!("review.refresh_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.reload(cx)),
                    ),
            )
    }

    fn render_base_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.base_menu.as_ref()?;
        let theme = self.theme.clone();
        let heading = |label: String| {
            div()
                .px(px(10.))
                .pt(px(8.))
                .pb(px(3.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg2)
                .child(SharedString::from(label))
        };
        let item =
            |id: (&'static str, usize), label: String, detail: Option<String>, selected: bool| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(if selected { theme.fg0 } else { theme.fg1 })
                    .when(selected, |item| item.bg(self.accent.alpha(0.16)))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(SharedString::from(label)),
                    )
                    .when_some(detail, |item, detail| {
                        item.child(
                            div()
                                .ml_auto()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(detail)),
                        )
                    })
            };
        let mut body = div().flex().flex_col().p(px(4.));
        if let Some(task_base) = self
            .context
            .as_ref()
            .and_then(|context| context.task_base.clone())
        {
            let selected = self.base == ReviewBase::Commit(task_base.clone());
            let target = ReviewBase::Commit(task_base.clone());
            body = body.child(
                item(
                    ("review-base-task", 0),
                    i18n::t!("review.menu_task"),
                    Some(short_sha(&task_base)),
                    selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.select_base(target.clone(), cx);
                    }),
                ),
            );
        }
        body = body.child(
            item(
                ("review-base-head", 0),
                i18n::t!("review.menu_head"),
                None,
                self.base == ReviewBase::Head,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.select_base(ReviewBase::Head, cx);
                }),
            ),
        );
        body = body.child(heading(i18n::t!("review.menu_branches")));
        match &menu.branches {
            None => body = body.child(heading(i18n::t!("review.menu_loading"))),
            Some(branches) => {
                for (index, branch) in branches.iter().enumerate() {
                    let target = ReviewBase::Branch(branch.clone());
                    let selected = self.base == target;
                    body = body.child(
                        item(
                            ("review-base-branch", index),
                            format!("⎇ {branch}"),
                            None,
                            selected,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.select_base(target.clone(), cx);
                            }),
                        ),
                    );
                }
            }
        }
        body = body.child(heading(i18n::t!("review.menu_commits")));
        if let Some(commits) = &menu.commits {
            for (index, (sha, summary)) in commits.iter().enumerate() {
                let target = ReviewBase::Commit(sha.clone());
                let selected = matches!(&self.base, ReviewBase::Commit(rev) if rev.starts_with(sha.as_str()) || sha.starts_with(rev.as_str()));
                body = body.child(
                    item(
                        ("review-base-commit", index),
                        summary.clone(),
                        Some(sha.clone()),
                        selected,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_base(target.clone(), cx);
                        }),
                    ),
                );
            }
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.base_menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("review-base-menu")
                        .absolute()
                        .top(px(TOOLBAR_HEIGHT - 2.))
                        .left(px(64.))
                        .w(px(380.))
                        .max_h(px(380.))
                        .overflow_y_scroll()
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(6.),
                            gpui::hsla(0., 0., 0., 0.35),
                        )
                        .blur_radius(px(16.))])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let Some(diff) = self.diff.clone() else {
            return div()
                .w(px(TREE_WIDTH))
                .h_full()
                .flex_none()
                .bg(theme.bg0)
                .border_r_1()
                .border_color(theme.border)
                .into_any_element();
        };
        let current = self.picked_file.or_else(|| {
            self.rows
                .get(self.list.logical_scroll_top().item_ix)
                .map(Row::file)
        });
        let mut tree = div()
            .id("review-tree")
            .w(px(TREE_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .py(px(4.))
            .overflow_y_scroll()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border);
        for (index, entry) in build_tree(&diff.files, &self.collapsed_dirs)
            .into_iter()
            .enumerate()
        {
            tree = tree.child(match entry {
                TreeEntry::Directory { path, label, depth } => {
                    let open = !self.collapsed_dirs.contains(&path);
                    div()
                        .id(("review-tree-dir", index))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h(px(22.))
                        .pl(px(8. + depth as f32 * 12.))
                        .pr(px(8.))
                        .text_size(px(11.5))
                        .text_color(theme.fg2)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3))
                        .child(
                            div()
                                .flex_none()
                                .w(px(10.))
                                .child(if open { "▾" } else { "▸" }),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(SharedString::from(label)),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if !this.collapsed_dirs.remove(&path) {
                                    this.collapsed_dirs.insert(path.clone());
                                }
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                }
                TreeEntry::File { file, label, depth } => {
                    let entry = &diff.files[file];
                    let selected = current == Some(file);
                    let reviewed = self.reviewed.contains(&entry.path);
                    div()
                        .id(("review-tree-file", index))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .h(px(22.))
                        .pl(px(8. + depth as f32 * 12.))
                        .pr(px(8.))
                        .border_l_2()
                        .border_color(if selected {
                            self.accent
                        } else {
                            gpui::transparent_black()
                        })
                        .when(selected, |row| row.bg(theme.bg3))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3))
                        .child(
                            div()
                                .flex_none()
                                .w(px(10.))
                                .text_size(px(10.5))
                                .text_color(status_tint(&theme, entry.kind))
                                .child(entry.kind.letter()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(if reviewed { theme.fg2 } else { theme.fg0 })
                                .child(SharedString::from(label)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(if reviewed {
                                    "✓".to_string()
                                } else {
                                    counts_label(entry)
                                })),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.scroll_to_file(file, cx)),
                        )
                        .into_any_element()
                }
            });
        }
        tree.into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let centered = |text: String| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(20.))
                .text_size(px(12.5))
                .text_color(theme.fg2)
                .child(SharedString::from(text))
                .into_any_element()
        };
        if let Load::Failed(error) = &self.load {
            return centered(error_label(error));
        }
        let Some(diff) = self.diff.as_ref() else {
            return centered(i18n::t!("review.loading"));
        };
        if diff.files.is_empty() {
            return centered(i18n::t!("review.empty", "base" => self.base_label()));
        }
        list(self.list.clone(), cx.processor(Self::render_row))
            .size_full()
            .into_any_element()
    }

    fn render_row(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (Some(diff), Some(row)) = (self.diff.clone(), self.rows.get(index).cloned()) else {
            return div().into_any_element();
        };
        match row {
            Row::FileHeader { file } => self.render_file_header(&diff, file, cx),
            Row::Notice { file, notice } => self.render_notice(&diff.files[file], notice),
            Row::Line { file, hunk, line } => {
                let entry = &diff.files[file];
                let Some(line) = entry.hunks.get(hunk).and_then(|hunk| hunk.lines.get(line)) else {
                    return div().into_any_element();
                };
                let syntax = self.syntax.get(file).cloned().flatten();
                let highlighted = syntax.as_ref().and_then(|syntax| match line.kind {
                    DiffLineKind::Removed => syntax
                        .old
                        .as_ref()
                        .zip(line.old_line)
                        .and_then(|(text, number)| text.line(number)),
                    _ => syntax
                        .new
                        .as_ref()
                        .zip(line.new_line)
                        .and_then(|(text, number)| text.line(number)),
                });
                // 全文の行と diff の行が一致した時だけ色を付ける（`-w` などで食い違ったら素の文字）。
                let spans = highlighted
                    .filter(|(text, _)| *text == line.text)
                    .map(|(_, spans)| spans.to_vec())
                    .unwrap_or_default();
                self.render_code_line(
                    CodeLine {
                        index,
                        kind: line.kind,
                        old_line: line.old_line,
                        new_line: line.new_line,
                        text: &line.text,
                        spans: &spans,
                        no_newline: line.no_newline,
                    },
                    cx,
                )
            }
            Row::Context {
                file,
                old_line,
                new_line,
            } => {
                let syntax = self.syntax.get(file).cloned().flatten();
                let (text, spans) = syntax
                    .as_ref()
                    .and_then(|syntax| syntax.new.as_ref())
                    .and_then(|text| text.line(new_line))
                    .map(|(text, spans)| (text.to_string(), spans.to_vec()))
                    .unwrap_or_default();
                self.render_code_line(
                    CodeLine {
                        index,
                        kind: DiffLineKind::Context,
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                        text: &text,
                        spans: &spans,
                        no_newline: false,
                    },
                    cx,
                )
            }
            Row::Fold {
                file,
                gap,
                hidden,
                expandable,
                section,
            } => self.render_fold(index, file, gap, hidden, expandable, section, cx),
            Row::Note { note, .. } => self.render_note(index, note, cx),
            Row::Draft { .. } => self.render_draft(cx),
        }
    }

    fn render_file_header(
        &self,
        diff: &ReviewDiff,
        file: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let entry = &diff.files[file];
        let collapsed = self.collapsed_files.contains(&entry.path);
        let reviewed = self.reviewed.contains(&entry.path);
        let (directory, name) = match entry.path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_string()),
            None => (String::new(), entry.path.clone()),
        };
        div()
            .id(("review-file", file))
            .flex()
            .items_center()
            .gap(px(7.))
            .h(px(30.))
            .mt(px(if file == 0 { 0. } else { 6. }))
            .px(px(10.))
            .bg(theme.bg0)
            .border_t_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id(("review-file-chevron", file))
                    .flex_none()
                    .w(px(12.))
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .child(if collapsed { "▸" } else { "▾" })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_file_collapsed(file, cx)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(12.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(status_tint(&theme, entry.kind))
                    .child(entry.kind.letter()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .child(
                                div()
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(directory)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if reviewed { theme.fg2 } else { theme.fg0 })
                                    .child(SharedString::from(name)),
                            ),
                    )
                    .when_some(entry.old_path.clone(), |header, old| {
                        header.child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(
                                    i18n::t!("review.renamed_from", "path" => old),
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(5.))
                    .text_size(px(11.))
                    .child(
                        div()
                            .text_color(theme.ok)
                            .child(SharedString::from(format!("+{}", entry.additions))),
                    )
                    .child(
                        div()
                            .text_color(theme.err)
                            .child(SharedString::from(format!("−{}", entry.deletions))),
                    ),
            )
            .child(
                div()
                    .id(("review-file-reviewed", file))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .h(px(20.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(11.))
                    .text_color(if reviewed { theme.fg0 } else { theme.fg2 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(if reviewed { "☑" } else { "☐" })
                    .child(SharedString::from(i18n::t!("review.reviewed")))
                    .tooltip(Tooltip::text(
                        i18n::t!("review.reviewed_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_reviewed(file, cx)),
                    ),
            )
            .when(entry.kind != FileChangeKind::Deleted, |header| {
                header.child(
                    div()
                        .id(("review-file-open", file))
                        .flex_none()
                        .px(px(6.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child(SharedString::from(format!(
                            "{} ↗",
                            i18n::t!("review.open_file")
                        )))
                        .tooltip(Tooltip::text(
                            i18n::t!("review.open_file_tip"),
                            theme.clone(),
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| this.open_file(file, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_notice(&self, file: &FileDiff, notice: Notice) -> gpui::AnyElement {
        let text = match notice {
            Notice::Binary => i18n::t!("review.notice_binary"),
            Notice::TooLarge => i18n::t!("review.notice_too_large", "counts" => counts_label(file)),
            Notice::ModeOnly => i18n::t!("review.notice_mode"),
            Notice::Empty => i18n::t!("review.notice_empty"),
        };
        div()
            .flex()
            .items_center()
            .h(px(28.))
            .pl(px(NUMBER_COLUMN_WIDTH * 2. + MARKER_COLUMN_WIDTH))
            .text_size(px(11.5))
            .text_color(self.theme.fg2)
            .child(SharedString::from(text))
            .into_any_element()
    }

    fn render_code_line(&self, line: CodeLine, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = &self.theme;
        let index = line.index;
        let (tint, marker, marker_color) = match line.kind {
            DiffLineKind::Added => (Some(theme.ok.alpha(LINE_TINT)), "+", theme.ok),
            DiffLineKind::Removed => (Some(theme.err.alpha(LINE_TINT)), "−", theme.err),
            DiffLineKind::Context => (None, " ", theme.fg2),
        };
        let selected = self.selection.is_some_and(|selection| {
            let (low, high) = ordered(selection);
            (low..=high).contains(&index)
        });
        let number = |value: Option<u32>| {
            div()
                .flex_none()
                .w(px(NUMBER_COLUMN_WIDTH))
                .pr(px(8.))
                .flex()
                .justify_end()
                .text_color(theme.fg2)
                .child(SharedString::from(
                    value.map(|value| value.to_string()).unwrap_or_default(),
                ))
        };
        div()
            .id(("review-line", index))
            .group("review-line")
            .relative()
            .flex()
            .items_start()
            .min_h(px(ROW_MIN_HEIGHT))
            .w_full()
            // 選択 = 左バー 2px（プロジェクト色・UI-SPEC §1.3 の選択の左バー）。非選択も透明で幅を揃える。
            .border_l_2()
            .border_color(if selected {
                self.accent
            } else {
                gpui::transparent_black()
            })
            .when_some(tint, |row, tint| row.bg(tint))
            .font_family(CODE_FONT)
            .text_size(px(CODE_FONT_SIZE))
            .line_height(px(ROW_MIN_HEIGHT))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.select_row(index, event.modifiers.shift, cx);
                    this.selecting = !event.modifiers.shift;
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let dragging = this.selecting && event.pressed_button == Some(MouseButton::Left);
                if dragging
                    && this
                        .selection
                        .is_some_and(|selection| selection.head != index)
                {
                    this.select_row(index, true, cx);
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_start()
                    // 選択の面は追加 / 削除の面の上に重ねる（色相を塗り潰さない）。
                    .when(selected, |content| content.bg(theme.bg3.alpha(0.7)))
                    .child(number(line.old_line))
                    .child(number(line.new_line))
                    .child(
                        div()
                            .flex_none()
                            .w(px(MARKER_COLUMN_WIDTH))
                            .text_color(marker_color)
                            .child(marker),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr(px(12.))
                            .text_color(theme.fg0)
                            .child(styled_code(line.text, line.spans, theme))
                            .when(line.no_newline, |text| {
                                text.child(div().text_size(px(10.5)).text_color(theme.fg2).child(
                                    SharedString::from(format!(
                                        "⏎̸ {}",
                                        i18n::t!("review.no_newline")
                                    )),
                                ))
                            }),
                    ),
            )
            .child(
                // ＋（ホバーで出る・押すとこの行 / 選んでいる範囲に注記を付ける）。
                div()
                    .id(("review-line-add", index))
                    .absolute()
                    .left(px(2.))
                    .top(px(2.))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(12.))
                    .line_height(px(14.))
                    .text_color(theme.fg0)
                    .cursor_pointer()
                    .invisible()
                    .group_hover("review-line", |style| style.visible())
                    .hover(|style| style.bg(theme.bg3))
                    .child("+")
                    .tooltip(Tooltip::text(
                        i18n::t!("review.add_note_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let inside = this.selection.is_some_and(|selection| {
                                let (low, high) = ordered(selection);
                                (low..=high).contains(&index)
                            });
                            if event.modifiers.shift || !inside {
                                this.select_row(index, event.modifiers.shift, cx);
                            }
                            this.open_draft(window, cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_fold(
        &self,
        index: usize,
        file: usize,
        gap: Gap,
        hidden: u32,
        expandable: bool,
        section: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        div()
            .id(("review-fold", index))
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(24.))
            .pl(px(NUMBER_COLUMN_WIDTH * 2. - 20.))
            .bg(theme.bg0)
            .text_size(px(11.5))
            .text_color(theme.fg2)
            .when(expandable, |fold| {
                fold.cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .tooltip(Tooltip::text(i18n::t!("review.fold_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.expand(file, gap, cx)),
                    )
            })
            .child(SharedString::from(
                i18n::t!("review.fold", "count" => hidden),
            ))
            .when(!section.is_empty(), |fold| {
                fold.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(CODE_FONT)
                        .child(SharedString::from(section)),
                )
            })
            .into_any_element()
    }
}

/// ソース管理パネルと同じ色の割り当て（追加 = ok・変更 = warn・削除 = err）。
fn status_tint(theme: &Theme, kind: FileChangeKind) -> Hsla {
    match kind {
        FileChangeKind::Added | FileChangeKind::Untracked => theme.ok,
        FileChangeKind::Modified | FileChangeKind::Renamed => theme.warn,
        FileChangeKind::Deleted => theme.err,
    }
}

fn counts_label(file: &FileDiff) -> String {
    format!("+{} −{}", file.additions, file.deletions)
}

fn error_label(error: &ReviewError) -> String {
    match error {
        ReviewError::NotRepository => i18n::t!("review.error_not_repo"),
        ReviewError::UnknownBase(base) => i18n::t!("review.error_unknown_base", "base" => base),
        ReviewError::Git(message) => i18n::t!("review.error_git", "message" => message),
    }
}

impl Render for ReviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("review-view")
            .key_context("ReviewView")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(self.show_tree, |body| body.child(self.render_tree(cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_body(cx)),
                    ),
            )
            .children(self.render_tray(cx))
            .children(self.render_base_menu(cx))
            .children(self.render_send_menu(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_expanded_and_highlights_follow() {
        let theme = Theme::dark();
        let spans = vec![(1..3, lang::HighlightKind::Keyword)];
        // 描画要素そのものは比較できないので、写像の関数だけを確かめる（落ちないこと）。
        let _ = styled_code("\tfn x", &spans, &theme);
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
    }

    #[test]
    fn default_base_prefers_the_task_base() {
        let context = ReviewContext {
            host: host::LocalHost::shared(),
            root: PathBuf::from("/tmp"),
            task_base: Some("abc".into()),
            accent: Theme::dark().fg1,
            storage: None,
            scope: "test".into(),
        };
        assert_eq!(default_base(&context), ReviewBase::Commit("abc".into()));
        let editor = ReviewContext {
            task_base: None,
            ..context
        };
        assert_eq!(default_base(&editor), ReviewBase::Head);
    }

    /// 一時 repo（80 行・5 行目と 60 行目を書き換え）を作る。git が無ければ `None`。
    fn temp_repo(tag: &str) -> Option<PathBuf> {
        let dir =
            std::env::temp_dir().join(format!("necoder_review_view_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&dir)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
        };
        if !git(&["init", "-q", "-b", "main"]).is_ok_and(|output| output.status.success()) {
            return None;
        }
        let original: String = (1..=80).map(|line| format!("line {line}\n")).collect();
        std::fs::write(dir.join("a.rs"), &original).ok()?;
        git(&["add", "-A"]).ok()?;
        git(&["commit", "-qm", "base"]).ok()?;
        let changed = original
            .replace("line 5\n", "LINE 5\n")
            .replace("line 60\n", "LINE 60\n");
        std::fs::write(dir.join("a.rs"), changed).ok()?;
        Some(dir)
    }

    fn open_review(view: &Entity<ReviewView>, dir: &Path, cx: &mut gpui::VisualTestContext) {
        view.update(cx, |view, cx| {
            view.set_context(
                ReviewContext {
                    host: host::LocalHost::shared(),
                    root: dir.to_path_buf(),
                    task_base: None,
                    accent: Theme::dark().fg1,
                    storage: None,
                    scope: "test".into(),
                },
                cx,
            );
            view.activate(cx);
        });
        for _ in 0..200 {
            cx.run_until_parked();
            let ready = view.read_with(cx, |view, _| {
                view.load == Load::Ready && view.syntax.iter().all(Option::is_some)
            });
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// 行を選んで注記を書き、保存 → 送る → 送信済み → 解決 → 未解決に戻す、まで。
    /// 送る先はこのビューは知らない（宛先の一覧を受け取り、選ばれた宛先とプロンプトを上げるだけ）。
    #[gpui::test]
    fn notes_are_drafted_saved_sent_and_resolved(cx: &mut gpui::TestAppContext) {
        let Some(dir) = temp_repo("notes") else {
            return;
        };
        i18n::set_locale("ja");
        let (view, cx) = cx.add_window_view(|_window, cx| ReviewView::new(Theme::dark(), cx));
        open_review(&view, &dir, cx);
        let events: std::rc::Rc<std::cell::RefCell<Vec<ReviewEvent>>> = Default::default();
        let sink = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &ReviewEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach()
        });
        view.update_in(cx, |view, window, cx| {
            // 削除行（旧 5）から追加行（新 5）までを選ぶ = 新しい側の 5 行目に付く。
            assert!(view.select_line("a.rs", NoteSide::Old, 5, false, cx));
            assert!(view.select_line("a.rs", NoteSide::New, 5, true, cx));
            view.open_draft(window, cx);
            let draft = view.draft.as_ref().expect("入力欄が開く");
            assert_eq!(draft.target.side, NoteSide::New);
            assert_eq!((draft.target.start, draft.target.end), (5, 5));
            assert_eq!(
                draft.target.excerpt.len(),
                2,
                "削除行と追加行の両方が抜粋に入る"
            );
            view.set_draft_text("  大文字にしない  ", cx);
            view.save_draft(window, cx);
            assert!(view.draft.is_none());
            assert_eq!(view.note_counts(), (1, 1));
            assert_eq!(view.notes[0].body, "大文字にしない");
            assert!(view
                .rows
                .iter()
                .any(|row| matches!(row, Row::Note { note: 0, .. })));
            // 空の本文は残さない。
            assert!(view.select_line("a.rs", NoteSide::New, 60, false, cx));
            view.open_draft(window, cx);
            view.save_draft(window, cx);
            assert_eq!(view.note_counts(), (1, 1));
        });
        view.update(cx, |view, cx| view.request_send(false, cx));
        cx.run_until_parked();
        assert_eq!(
            events.borrow().last(),
            Some(&ReviewEvent::SendMenuRequested { resend: false })
        );
        let target = SendTarget {
            panel: EntityId::from(7u64),
            thread: Some("thread-2".into()),
            label: "スレッド2".into(),
            detail: "待機中".into(),
            color: Theme::dark().fg1,
            is_default: true,
        };
        view.update(cx, |view, cx| {
            view.show_send_menu(vec![target.clone()], false, cx);
            view.send_to(target.clone(), cx);
            assert!(view.send_menu.is_none(), "選んだら閉じる");
        });
        cx.run_until_parked();
        let sent = events.borrow().last().cloned();
        let Some(ReviewEvent::SendNotes {
            target: chosen,
            prompt,
            note_ids,
            resend,
        }) = sent
        else {
            panic!("注記が送られていない: {sent:?}");
        };
        assert_eq!(chosen, target, "宛先は選んだものがそのまま返る");
        assert!(!resend, "未送信だけを送るメニューから選んだ");
        assert!(prompt.starts_with("レビューコメント（1 件）— 比較: "));
        assert!(prompt.contains("1. a.rs:5\n```diff\n-line 5\n+LINE 5\n```\n大文字にしない"));
        view.update(cx, |view, cx| {
            view.mark_notes_sent(&note_ids, cx);
            assert_eq!(view.note_counts(), (1, 0));
            assert_eq!(view.notes[0].state, ReviewNoteState::Sent);
            // 未送信が無ければ「送る」は何もしない。未解決の再送はできる。
            let before = view.notes_to_send(false).len();
            assert_eq!(before, 0);
            assert_eq!(view.notes_to_send(true).len(), 1);
            view.toggle_resolved(0, cx);
            assert_eq!(view.notes[0].state, ReviewNoteState::Resolved);
            assert!(view.notes_to_send(true).is_empty(), "解決は再送しない");
            view.toggle_resolved(0, cx);
            assert_eq!(
                view.notes[0].state,
                ReviewNoteState::Sent,
                "送っていれば送信済みに戻る"
            );
            view.delete_note(0, cx);
            assert_eq!(view.note_counts(), (0, 0));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 本物と同じ名前の action（既定 keymap の Editor 文脈で ⌘⏎ が指す先）。受け手は居ない＝
    // アプリと同じく打鍵のまま入力欄の親へ上がってくることを確かめるために登録だけする。
    gpui::actions!(agent, [SubmitPrompt]);

    /// 入力欄のキー: ⌘⏎（非 mac は Ctrl+Enter）で保存・Esc で取り消し。一覧にフォーカスがある時の
    /// `c` は入力欄を開き、入力欄の中の `c` は文字として入る（一覧のキーが打鍵を奪わない）。
    #[gpui::test]
    fn draft_keys_save_cancel_and_do_not_steal_typing(cx: &mut gpui::TestAppContext) {
        let Some(dir) = temp_repo("keys") else {
            return;
        };
        cx.update(|cx| {
            let bindings = keymap_core::load_bindings(keymap_core::DEFAULT_KEYMAP_JSON, cx)
                .expect("既定 keymap がロードできる");
            cx.bind_keys(bindings);
        });
        let (view, cx) = cx.add_window_view(|_window, cx| ReviewView::new(Theme::dark(), cx));
        open_review(&view, &dir, cx);
        let redraw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            cx.run_until_parked();
        };

        view.update_in(cx, |view, window, cx| {
            assert!(view.select_line("a.rs", NoteSide::New, 5, false, cx));
            view.open_draft(window, cx);
            view.set_draft_text("キーで保存", cx);
        });
        redraw(cx);
        cx.simulate_keystrokes("secondary-enter");
        assert_eq!(
            view.read_with(cx, |view, _| view.note_counts()),
            (1, 1),
            "⌘⏎ で保存"
        );

        view.update_in(cx, |view, window, cx| {
            assert!(view.select_line("a.rs", NoteSide::New, 60, false, cx));
            view.open_draft(window, cx);
            view.set_draft_text("捨てる", cx);
        });
        redraw(cx);
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert!(view.draft.is_none(), "Esc で閉じる");
            assert_eq!(view.note_counts(), (1, 1), "取り消しは残さない");
        });

        view.update_in(cx, |view, window, cx| {
            assert!(view.select_line("a.rs", NoteSide::New, 60, false, cx));
            window.focus(&view.focus_handle, cx);
        });
        redraw(cx);
        cx.simulate_keystrokes("c");
        assert!(
            view.read_with(cx, |view, _| view.draft.is_some()),
            "一覧で c = 入力欄を開く"
        );
        redraw(cx);
        cx.simulate_keystrokes("c");
        view.read_with(cx, |view, cx| {
            let draft = view.draft.as_ref().expect("入力欄は開いたまま");
            assert_eq!(draft.editor.read(cx).plain_text(), "c", "入力欄の c は文字");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 一時 repo を実際に読み込み、畳みの展開でスクロール位置の行が保たれることを確かめる。
    #[gpui::test]
    fn loads_a_temp_repo_and_expands_folds(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("necoder_review_view_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(&dir)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
        };
        if !git(&["init", "-q", "-b", "main"]).is_ok_and(|output| output.status.success()) {
            return;
        }
        let original: String = (1..=80).map(|line| format!("line {line}\n")).collect();
        std::fs::write(dir.join("a.rs"), &original).unwrap();
        git(&["add", "-A"]).ok();
        git(&["commit", "-qm", "base"]).ok();
        let changed = original
            .replace("line 5\n", "LINE 5\n")
            .replace("line 60\n", "LINE 60\n");
        std::fs::write(dir.join("a.rs"), changed).unwrap();

        let view = cx.new(|cx| ReviewView::new(Theme::dark(), cx));
        view.update(cx, |view, cx| {
            view.set_context(
                ReviewContext {
                    host: host::LocalHost::shared(),
                    root: dir.clone(),
                    task_base: None,
                    accent: Theme::dark().fg1,
                    storage: None,
                    scope: "test".into(),
                },
                cx,
            );
            view.activate(cx);
        });
        // git と全文の読み込みは背景。終わるまで回す。
        for _ in 0..200 {
            cx.run_until_parked();
            let ready = view.read_with(cx, |view, _| {
                view.load == Load::Ready && view.syntax.iter().all(Option::is_some)
            });
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        view.update(cx, |view, cx| {
            assert_eq!(view.file_count(), 1);
            let folds = |view: &ReviewView| {
                view.rows
                    .iter()
                    .filter_map(|row| match row {
                        Row::Fold { hidden, .. } => Some(*hidden),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            // 1 行目〜: 5 行目の変更は 2〜8 行目の hunk（先頭 1 行）/ 間 48 行 / 末尾 17 行。
            assert_eq!(folds(view), vec![1, 48, 17]);
            view.expand_first_fold(cx);
            assert_eq!(folds(view), vec![48, 17], "先頭の 1 行を開いた");
            view.expand_first_fold(cx);
            assert_eq!(folds(view), vec![8, 17], "間は上下 20 行ずつ開く");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
