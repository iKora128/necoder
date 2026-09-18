//! chat_view — **Chat モードの面**（`docs/CHAT.md` §4.3 / `mock/chat.html`）。
//!
//! 新しい部品は一覧だけで、あとは既にある物を並べている:
//!
//! - 左 = チャットの一覧（＋ 新しいチャット・検索・日付グループ）
//! - 中央 = `AgentPanel`（Chat モード）。transcript・composer・承認カードはそのまま
//! - 右 = **既存のエディタ領域**。artifact は「チャットのフォルダに書かれたファイル」を
//!   プレビュー表示のタブで開いたもの。開いている物が無ければ右は閉じていて、会話が中央に広がる
//!
//! Chat はプロジェクトに属さない。だから `ProjectSessions` の `projects` / `sessions`（レールの枠と
//! 同じ添字で並ぶ）には混ぜず、別に 1 本だけ持つ。Chat 中は `Deref` がその session を返すので、
//! 「アクティブな session」を相手にしている既存のコード（タブ・プレビュー・ファイルを開く）は
//! そのまま Chat でも動く。

use crate::workspace::*;
use chat_core::date::{Date, Recency};

/// 一覧の幅。
const CHAT_SIDEBAR_WIDTH: f32 = 248.0;
/// 会話の列の最大幅（右が閉じている時に 1 行が長くなりすぎない）。
const CHAT_COLUMN_MAX_WIDTH: f32 = 860.0;

/// 一覧の行の右クリックメニュー。
pub(crate) struct ChatMenuState {
    pub(crate) id: String,
    pub(crate) position: Point<gpui::Pixels>,
}

impl Workspace {
    pub(crate) fn chat_mode(&self) -> bool {
        self.project_sessions.chat_active
    }

    /// Chat のパネル（まだ一度も Chat を開いていなければ `None`）。
    pub(crate) fn chat_panel(&self) -> Option<Entity<AgentPanel>> {
        self.project_sessions
            .chat
            .as_ref()
            .map(|session| session.agent_panel.clone())
    }

    /// Chat は開くまで何も作らない（エージェントはもちろん、パネルも session も）。
    fn ensure_chat_session(&mut self, cx: &mut Context<Self>) {
        if self.project_sessions.chat.is_some() {
            return;
        }
        let theme = self.theme.clone();
        let storage = self.persistence.storage.clone();
        let session = Self::create_chat_session(theme, storage, cx);
        let search = session.agent_panel.read(cx).chat_search_query().to_string();
        self.chrome.chat_search.update(cx, |editor, cx| {
            if editor.plain_text() != search {
                editor.set_plain_text(&search, cx);
            }
        });
        self.project_sessions.chat = Some(session);
    }

    pub(crate) fn toggle_chat_mode(
        &mut self,
        _: &ToggleChat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_chat_mode(!self.chat_mode(), window, cx);
    }

    /// Editor / Fleet ⇄ Chat。3 つのモードは排他で、入口はここと `toggle_fleet_mode` だけ。
    pub(crate) fn set_chat_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on == self.chat_mode() {
            return;
        }
        if on {
            // プロジェクト側のタブの並びを枠へ書き戻してから切り替える（切り替えた後は
            // 「アクティブな session」が Chat を指すので、書き戻し先を取り違える）。
            self.sync_active_slot();
            self.ensure_chat_session(cx);
            self.chrome.fleet_mode = false;
            self.chrome.agent_full_screen = false;
            self.chrome.show_settings = false;
            self.project_sessions.chat_active = true;
            self.agent_active = true; // ⌘W / スレッド移動の宛先を会話に
            self.focus_chat_composer(window, cx);
        } else {
            self.project_sessions.chat_active = false;
            self.agent_panel
                .update(cx, |panel, cx| panel.parent_width_changed(cx));
            self.focus_session_surface(false, false, window, cx);
        }
        self.save_state(cx);
        cx.notify();
    }

    /// パネルを別の親へ載せ替えた直後は、幅の再計算とフォーカスの付け直しを同じ transaction で行う
    /// （`toggle_agent_full_screen_state` と同じ規律。怠ると Workspace のショートカットが死ぬ）。
    fn focus_chat_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.chat_panel() else {
            return;
        };
        panel.update(cx, |panel, cx| panel.parent_width_changed(cx));
        window.defer(cx, move |window, cx| {
            panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
        });
    }

    /// ⌘N（Chat 中）/ パレット / 一覧の ＋。Chat でなければ先に Chat へ移る。
    pub(crate) fn new_chat(&mut self, _: &NewChat, window: &mut Window, cx: &mut Context<Self>) {
        self.set_chat_mode(true, window, cx);
        if let Some(panel) = self.chat_panel() {
            panel.update(cx, |panel, cx| panel.open_new_chat(cx));
        }
        self.focus_chat_composer(window, cx);
    }

    fn open_chat_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = self.chat_panel() {
            panel.update(cx, |panel, cx| panel.open_chat(id, cx));
        }
        self.focus_chat_composer(window, cx);
    }

    /// 検索欄の文字をパネルへ流す（絞り込みと全文検索はパネルと storage の仕事）。
    pub(crate) fn sync_chat_search(&mut self, cx: &mut Context<Self>) {
        let query = self.chrome.chat_search.read(cx).plain_text();
        if let Some(panel) = self.chat_panel() {
            panel.update(cx, |panel, cx| panel.set_chat_search(query, cx));
        }
    }

    // ------------------------------------------------------------------
    // パネルからのイベント
    // ------------------------------------------------------------------

    /// Chat のパネルのイベント。プロジェクトの session と違って TaskSpace も Captain も無いので、
    /// 拾うのは「ファイルを開く」「成果物を見せる」「知らせる」だけ。
    pub(crate) fn on_chat_panel_event(
        &mut self,
        event: &agent_panel::PanelEvent,
        cx: &mut Context<Self>,
    ) {
        let neutral = self.theme.fg2;
        match event {
            agent_panel::PanelEvent::ChatRowsChanged => {
                self.chrome.chat_accent = self
                    .chat_panel()
                    .and_then(|panel| panel.read(cx).active_chat_color());
                self.sync_chat_artifact_tabs(cx);
                cx.notify();
            }
            agent_panel::PanelEvent::OpenPreviewRequest { path } => {
                self.queue_chat_preview(path.clone(), cx);
            }
            agent_panel::PanelEvent::OpenPathRequest { path, line, column } => {
                let Some(chat) = self.project_sessions.chat.as_mut() else {
                    return;
                };
                if line.is_none() && chat_core::folder::is_previewable(path) {
                    chat.pending_preview = Some(path.clone());
                } else if path.is_dir() {
                    if let Err(error) = project::open_with_default_app_local(path) {
                        eprintln!("既定アプリで開けない: {error:#}");
                    }
                } else {
                    chat.pending_navigation = Some((
                        path.clone(),
                        line.unwrap_or(1).saturating_sub(1) as usize,
                        column.unwrap_or(1).saturating_sub(1) as usize,
                    ));
                }
                cx.notify();
            }
            agent_panel::PanelEvent::OpenUrlRequest { url } => {
                if let Err(error) = crate::crash::open_url(url) {
                    eprintln!("URL を開けない: {error:#}");
                    self.push_toast(
                        i18n::t!("link.open_failed", "target" => url.as_ref()).into(),
                        neutral,
                        cx,
                    );
                }
            }
            agent_panel::PanelEvent::OpenDiffRequest {
                title,
                old_text,
                new_text,
            } => {
                if let Some(diff_text) = project::unified_diff_texts(old_text, new_text, title) {
                    let mut buffer = Buffer::from_str(&diff_text);
                    buffer.set_read_only(true);
                    if let Some(chat) = self.project_sessions.chat.as_mut() {
                        chat.pending_transient_tab = Some((
                            PathBuf::from(i18n::t!("difftab.proposal_title", "title" => title)),
                            buffer,
                        ));
                    }
                    cx.notify();
                }
            }
            agent_panel::PanelEvent::FilesTouched { files, color } => {
                self.on_chat_files_touched(files, *color, cx);
            }
            agent_panel::PanelEvent::TurnEnded {
                thread,
                color,
                summary,
                muted,
                ..
            } => {
                // 見えている会話が終わったことは画面で分かる。別のモードに居る時だけ知らせる。
                if !self.chat_mode() && !muted {
                    self.push_toast(
                        SharedString::from(format!("● {thread} — {summary}")),
                        *color,
                        cx,
                    );
                }
                self.on_chat_turn_ended(cx);
                if let Some(panel) = self.chat_panel() {
                    panel.update(cx, |panel, cx| panel.stop_idle_chat_agents(cx));
                }
                cx.notify();
            }
            agent_panel::PanelEvent::TurnFailed {
                thread,
                color,
                message,
                muted,
            } => {
                if !self.chat_mode() && !muted {
                    self.push_toast(
                        SharedString::from(format!("● {thread} — {message}")),
                        *color,
                        cx,
                    );
                }
            }
            agent_panel::PanelEvent::PermissionWaiting {
                thread,
                color,
                title,
                muted,
                ..
            } => {
                if !self.chat_mode() && !muted {
                    self.push_toast(
                        SharedString::from(format!("◐ {thread} — {title}")),
                        *color,
                        cx,
                    );
                }
            }
            _ => {}
        }
    }

    /// エージェントが触るファイルの予告。**このイベントは書かれる前に届く**（許可リクエストの時点）ので、
    /// ここでは覚えるだけにして、読み直しと表示はターン終了時に行う（[`Self::on_chat_turn_ended`]）。
    fn on_chat_files_touched(&mut self, files: &[PathBuf], color: Hsla, cx: &mut Context<Self>) {
        let Some(chat) = self.project_sessions.chat.as_mut() else {
            return;
        };
        for file in files {
            chat.agent_touched.insert(file.clone(), color);
            if !self.chrome.chat_touched.contains(file) {
                self.chrome.chat_touched.push(file.clone());
            }
        }
        cx.notify();
    }

    /// ターンが終わった。Chat の session にはファイル監視が無いので、このターンで書かれた物を
    /// ここで取り込む: 開いているタブは読み直し（＝プレビューの再読込も**ターンにつき 1 回**。
    /// ツール呼び出しのたびに読み直すとタイマーなどのページ状態が消える）、まだ開いていない
    /// 成果物は右に出す。
    fn on_chat_turn_ended(&mut self, cx: &mut Context<Self>) {
        let touched = std::mem::take(&mut self.chrome.chat_touched);
        let Some(chat) = self.project_sessions.chat.as_mut() else {
            return;
        };
        let mut newest_artifact = None;
        for file in &touched {
            let color = chat.agent_touched.get(file).copied();
            match chat.tabs.iter().find(|tab| &tab.path == file) {
                Some(tab) => {
                    if let Some(editor) = tab.editor().cloned() {
                        editor.update(cx, |view, cx| {
                            view.set_agent_mark_color(color, cx);
                            view.handle_external_change(cx);
                        });
                    }
                }
                None if chat_core::folder::is_previewable(file) && file.is_file() => {
                    newest_artifact = Some(file.clone());
                }
                None => {}
            }
        }
        if let Some(path) = newest_artifact {
            self.queue_chat_preview(path, cx);
        }
    }

    /// 右のエディタ領域にプレビュー表示で開く（実際に開くのは window のある次の描画）。
    fn queue_chat_preview(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(chat) = self.project_sessions.chat.as_mut() {
            chat.pending_preview = Some(path);
            chat.pending_preview_keeps_focus = true;
        }
        cx.notify();
    }

    /// 見ているチャットが替わったら、右のタブを付け替える: 前のチャットの成果物は閉じ
    /// （未保存の編集があるタブは残す）、替わった先に成果物があれば最新の 1 つを出す。
    fn sync_chat_artifact_tabs(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.chat_panel() else {
            return;
        };
        let (active_id, artifacts) = {
            let panel = panel.read(cx);
            (panel.active_chat_id(), panel.active_chat_artifacts())
        };
        if self.chrome.chat_shown == active_id {
            return;
        }
        self.chrome.chat_shown = active_id;
        if let Some(chat) = self.project_sessions.chat.as_mut() {
            chat.pending_close_clean_tabs = true;
            chat.pending_preview = artifacts
                .into_iter()
                .find(|path| chat_core::folder::is_previewable(path));
            chat.pending_preview_keeps_focus = true;
        }
    }

    // ------------------------------------------------------------------
    // 行のメニューと削除
    // ------------------------------------------------------------------

    fn open_chat_menu(
        &mut self,
        id: String,
        position: Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.chrome.chat_menu = Some(ChatMenuState { id, position });
        cx.notify();
    }

    fn close_chat_menu(&mut self, cx: &mut Context<Self>) {
        if self.chrome.chat_menu.take().is_some() {
            cx.notify();
        }
    }

    /// 消す。フォルダにファイルが残っていれば先に聞く（既定は「ファイルは残す」）。
    fn request_delete_chat(&mut self, id: String, cx: &mut Context<Self>) {
        let has_files = self
            .chat_panel()
            .is_some_and(|panel| panel.read(cx).chat_has_files(&id));
        if has_files {
            self.chrome.chat_delete_confirm = Some(id);
        } else if let Some(panel) = self.chat_panel() {
            panel.update(cx, |panel, cx| panel.delete_chat(&id, false, cx));
        }
        cx.notify();
    }

    fn confirm_delete_chat(&mut self, trash_files: bool, cx: &mut Context<Self>) {
        let Some(id) = self.chrome.chat_delete_confirm.take() else {
            return;
        };
        if let Some(panel) = self.chat_panel() {
            panel.update(cx, |panel, cx| panel.delete_chat(&id, trash_files, cx));
        }
        cx.notify();
    }

    /// 会話を Markdown に書き出して、そのチャットのフォルダ直下へ置く（`artifacts/` には入れない＝
    /// 成果物として数えない）。書けたらタブで開く。
    fn export_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(panel) = self.chat_panel() else {
            return;
        };
        let neutral = self.theme.fg2;
        match panel.update(cx, |panel, cx| panel.export_chat_markdown(id, cx)) {
            Ok(path) => {
                self.push_toast(
                    i18n::t!("chat.exported", "path" => &path.display().to_string()).into(),
                    neutral,
                    cx,
                );
                if let Some(chat) = self.project_sessions.chat.as_mut() {
                    chat.pending_preview = Some(path);
                }
            }
            Err(message) => self.push_toast(message.into(), self.theme.err, cx),
        }
        cx.notify();
    }

    /// Chat から成果物を持っていける先: Chat を抜けた時に戻るプロジェクト（ローカルのものだけ）。
    pub(crate) fn chat_copy_target(&self) -> Option<(SharedString, PathBuf)> {
        if !self.chat_mode() {
            return None;
        }
        let slot = self
            .project_sessions
            .projects
            .get(self.project_sessions.active)?;
        slot.remote_host
            .is_none()
            .then(|| (slot.name.clone(), slot.worktree.root().to_path_buf()))
    }

    /// `source` をプロジェクトのルートへ複製する。**上書きしない**（同名があれば ` 2`・` 3` …）。
    pub(crate) fn copy_to_project(
        &mut self,
        source: &Path,
        project_root: &Path,
        cx: &mut Context<Self>,
    ) {
        let neutral = self.theme.fg2;
        match copy_without_overwrite(source, project_root) {
            Ok(copied) => self.push_toast(
                i18n::t!("chat.copied_to_project", "path" => &copied.display().to_string()).into(),
                neutral,
                cx,
            ),
            Err(error) => {
                eprintln!("プロジェクトへコピーできない: {error:#}");
                self.push_toast(
                    i18n::t!("chat.copy_failed", "message" => &error.to_string()).into(),
                    self.theme.err,
                    cx,
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // 描画
    // ------------------------------------------------------------------

    /// Chat モードの本体（titlebar と statusbar の間）。
    pub(crate) fn render_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let artifact_open = !self.tabs.is_empty();
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_rail(cx))
            .when(self.chrome.show_left, |row| {
                row.child(self.render_chat_sidebar(cx))
            })
            .child(self.render_chat_center(artifact_open, cx))
            .when(artifact_open, |row| {
                row.child(self.render_chat_artifacts(cx))
            })
    }

    fn render_chat_center(&self, artifact_open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let panel = self.agent_panel.clone();
        div()
            .flex_1()
            .min_w(px(380.))
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .bg(theme.bg1)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.agent_active = true),
            )
            .children(self.render_chat_artifact_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    // 右が閉じている時だけ列幅を絞る（開いている時は既に十分狭い）。
                    .when(!artifact_open, |column| {
                        column.max_w(px(CHAT_COLUMN_MAX_WIDTH))
                    })
                    .child(panel.cached(StyleRefinement::default().flex().flex_col().size_full())),
            )
    }

    /// このチャットの成果物の帯（会話の上）。ツールカードの `▣ プレビュー` は再起動すると消える
    /// （永続化するのは会話の本文だけ）ので、**成果物へ戻る道**はここに常設する。無ければ出さない。
    fn render_chat_artifact_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let theme = self.theme.clone();
        let artifacts: Vec<PathBuf> = self.chat_panel()?.read(cx).chat_artifacts().to_vec();
        if artifacts.is_empty() {
            return None;
        }
        let accent = self.accent();
        let open_path = self.active_tab_path();
        let mut bar = div()
            .id("chat-artifacts")
            .flex_none()
            .w_full()
            .h(px(30.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .overflow_x_scroll()
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.));
        for (index, path) in artifacts.into_iter().enumerate() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let shown = open_path.as_ref() == Some(&path);
            let target = path.clone();
            bar = bar.child(
                div()
                    .id(("chat-artifact", index))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(theme.border)
                    .text_color(if shown { theme.fg0 } else { theme.fg1 })
                    // 開いている物だけスレッド色の下線（色は識別。面は塗らない）。
                    .when(shown, |chip| chip.border_b_2().border_color(accent))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child("▣")
                    .child(SharedString::from(name))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            if let Some(chat) = this.project_sessions.chat.as_mut() {
                                chat.pending_preview = Some(target.clone());
                            }
                            cx.notify();
                        }),
                    ),
            );
        }
        Some(bar.into_any_element())
    }

    /// 右 = 既存のエディタ領域（タブ列 + パンくず + エディタ / プレビュー）。artifact 専用のペインは無い。
    fn render_chat_artifacts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex_none()
            .w(gpui::relative(0.42))
            .min_w(px(420.))
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .border_l_1()
            .border_color(theme.border)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.agent_active = false),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .child(self.render_main_pane(cx)),
            )
    }

    fn render_chat_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let rows: Vec<agent_panel::ChatRow> = self
            .chat_panel()
            .map(|panel| panel.read(cx).chat_rows().to_vec())
            .unwrap_or_default();
        let searching = !self
            .chrome
            .chat_search
            .read(cx)
            .plain_text()
            .trim()
            .is_empty();
        let running = rows.iter().filter(|row| row.activity.is_some()).count();

        let new_button = div()
            .id("chat-new")
            .flex_none()
            .mx(px(10.))
            .mt(px(10.))
            .mb(px(6.))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(7.))
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .text_color(theme.fg0)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .child("＋")
            .child(SharedString::from(i18n::t!("chat.new_chat")))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(keymap_core::keystroke_label("cmd-n"))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.new_chat(&NewChat, window, cx);
                }),
            );

        let search = div()
            .flex_none()
            .mx(px(10.))
            .mb(px(4.))
            .h(px(26.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.5))
            .child(div().flex_none().text_color(theme.fg2).child("⌕"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.chrome.chat_search.clone())
                    .when(!searching, |field| {
                        field.child(
                            div()
                                .absolute()
                                .top(px(4.))
                                .left(px(2.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("chat.search_placeholder"))),
                        )
                    }),
            );

        let mut list = div()
            .id("chat-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb(px(8.));
        if rows.is_empty() && searching {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("chat.no_results"))),
            );
        }
        let today = Date::today();
        let offset = chat_core::date::local_offset_seconds();
        let mut last_group: Option<ChatGroup> = None;
        for (index, row) in rows.iter().enumerate() {
            let group = if row.pinned {
                ChatGroup::Pinned
            } else {
                ChatGroup::Recency(Date::from_unix_ms_local(row.sort_at_ms, offset).recency(today))
            };
            if last_group != Some(group) && !searching {
                list = list.child(
                    div()
                        .px(px(12.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .text_size(px(10.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(theme.fg2)
                        .child(SharedString::from(group.label())),
                );
                last_group = Some(group);
            }
            list = list.child(self.render_chat_row(index, row, cx));
        }

        div()
            .flex_none()
            .w(px(CHAT_SIDEBAR_WIDTH))
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(new_button)
            .child(search)
            .child(list)
            .child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!(
                        "chat.agents_running",
                        "n" => running,
                        "total" => rows.len()
                    ))),
            )
    }

    fn render_chat_row(
        &self,
        index: usize,
        row: &agent_panel::ChatRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = row.color;
        let id = row.id.clone();
        let menu_id = row.id.clone();
        // 状態は形と動きで示す（色は識別）: 応答中 = スピナー / 承認待ち・完了 = 既存の点 /
        // **輪郭だけ = エージェントが起きていない**（一覧に並んでいるだけではプロセスを持たない）。
        let dot = match row.activity {
            Some(activity) => agent_panel::activity_dot(("chat-dot", index), 8.0, color, activity),
            None => div()
                .w(px(8.))
                .h(px(8.))
                .rounded_full()
                .border_1()
                .border_color(color)
                .into_any_element(),
        };
        div()
            .id(("chat-row", index))
            .flex()
            .items_start()
            .gap(px(8.))
            .pl(px(9.))
            .pr(px(10.))
            .py(px(6.))
            .border_l_2()
            .border_color(gpui::transparent_black())
            .cursor_pointer()
            .when(row.active, |element| {
                element.bg(theme.bg2).border_color(color)
            })
            .when(!row.active, |element| {
                element.hover(|style| style.bg(theme.bg1))
            })
            .child(
                div()
                    .flex_none()
                    .w(px(12.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(dot),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(if row.active { theme.fg0 } else { theme.fg1 })
                            .when(row.active, |name| {
                                name.font_weight(gpui::FontWeight::SEMIBOLD)
                            })
                            .child(row.name.clone()),
                    )
                    .when_some(row.snippet.clone(), |column, snippet| {
                        column.child(
                            div()
                                .truncate()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(snippet),
                        )
                    }),
            )
            .when(row.artifacts > 0, |element| {
                element.child(
                    div()
                        .flex_none()
                        .pt(px(2.))
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(format!("▣ {}", row.artifacts))),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_chat_row(&id, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.open_chat_menu(menu_id.clone(), event.position, cx);
                }),
            )
    }

    pub(crate) fn render_chat_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.chrome.chat_menu.as_ref()?;
        let panel = self.chat_panel()?;
        let id = menu.id.clone();
        let position = menu.position;
        let (pinned, dir) = {
            let panel = panel.read(cx);
            (
                panel
                    .chat_rows()
                    .iter()
                    .any(|row| row.id == id && row.pinned),
                panel.chat_dir(&id).filter(|dir| dir.is_dir()),
            )
        };
        let (bg2, bg3, border, fg0, fg1, err) = (
            self.theme.bg2,
            self.theme.bg3,
            self.theme.border,
            self.theme.fg0,
            self.theme.fg1,
            self.theme.err,
        );
        let item = move |element_id: &'static str, label: String| {
            div()
                .id(element_id)
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
            .w(px(220.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))]);

        let pin_id = id.clone();
        menu_box = menu_box.child(
            item(
                "chat-ctx-pin",
                if pinned {
                    i18n::t!("chat.unpin")
                } else {
                    i18n::t!("chat.pin")
                },
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if let Some(panel) = this.chat_panel() {
                        panel.update(cx, |panel, cx| panel.toggle_chat_pinned(&pin_id, cx));
                    }
                    this.close_chat_menu(cx);
                }),
            ),
        );
        if let Some(dir) = dir {
            menu_box = menu_box.child(
                item("chat-ctx-reveal", i18n::t!("chat.reveal_folder")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        if let Err(error) = project::reveal_in_finder_local(&dir) {
                            eprintln!("フォルダを表示できない: {error:#}");
                        }
                        this.close_chat_menu(cx);
                    }),
                ),
            );
        }
        let export_id = id.clone();
        menu_box = menu_box.child(
            item("chat-ctx-export", i18n::t!("chat.export_markdown")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.close_chat_menu(cx);
                    this.export_chat(&export_id, cx);
                }),
            ),
        );
        menu_box = menu_box.child(div().h(px(1.)).bg(border).my(px(3.)));
        let delete_id = id.clone();
        menu_box = menu_box.child(
            item("chat-ctx-delete", i18n::t!("chat.delete"))
                .text_color(err)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.close_chat_menu(cx);
                        this.request_delete_chat(delete_id.clone(), cx);
                    }),
                ),
        );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.close_chat_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.close_chat_menu(cx)),
                )
                .child(menu_box)
                .into_any_element(),
        )
    }

    /// 「ファイルもゴミ箱へ移す / ファイルは残す」。何が失われるかを選ばせる（完全削除はしない）。
    pub(crate) fn render_chat_delete_confirm(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let id = self.chrome.chat_delete_confirm.clone()?;
        let panel = self.chat_panel()?;
        let (name, dir) = {
            let panel = panel.read(cx);
            (
                panel
                    .chat_rows()
                    .iter()
                    .find(|row| row.id == id)
                    .map(|row| row.name.clone())
                    .unwrap_or_default(),
                panel.chat_dir(&id),
            )
        };
        let theme = self.theme.clone();
        let button = |element_id: &'static str, label: String, primary: bool| {
            div()
                .id(element_id)
                .px(px(12.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(if primary { theme.fg2 } else { theme.border })
                .when(primary, |element| element.bg(theme.bg3))
                .text_size(px(12.))
                .text_color(if primary { theme.fg0 } else { theme.fg1 })
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(label)
        };
        let dialog = div()
            .w(px(440.))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(12.),
                gpui::hsla(0., 0., 0., 0.5),
            )
            .blur_radius(px(32.))])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(
                        i18n::t!("chat.delete_confirm_title", "name" => name.as_ref()),
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!("chat.delete_confirm_body"))),
            )
            .when_some(dir, |dialog, dir| {
                dialog.child(
                    div()
                        .text_size(px(11.))
                        .font_family("Guguru Sans Code")
                        .text_color(theme.fg2)
                        .child(SharedString::from(dir.display().to_string())),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .pt(px(4.))
                    .child(
                        button("chat-delete-cancel", i18n::t!("chat.cancel"), false).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                this.chrome.chat_delete_confirm = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        button(
                            "chat-delete-trash",
                            i18n::t!("chat.delete_trash_files"),
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.confirm_delete_chat(true, cx)),
                        ),
                    )
                    .child(
                        button("chat-delete-keep", i18n::t!("chat.delete_keep_files"), true)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| {
                                    this.confirm_delete_chat(false, cx)
                                }),
                            ),
                    ),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(140.))
                .bg(gpui::hsla(0., 0., 0., 0.4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        this.chrome.chat_delete_confirm = None;
                        cx.notify();
                    }),
                )
                .child(dialog)
                .into_any_element(),
        )
    }

    /// titlebar のピル。Chat はプロジェクトに属さないので、ブランチも色も出さない。
    pub(crate) fn render_chat_pill(&self) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(10.))
            .rounded(px(6.))
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.5))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("titlebar.chat"))),
            )
            .child(
                div()
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("chat.no_project"))),
            )
    }

    /// statusbar の左: いま見ているチャットのフォルダ（押すと Finder で開く）。
    pub(crate) fn render_chat_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let dir = self
            .chat_panel()
            .and_then(|panel| panel.read(cx).active_chat_dir())
            .filter(|dir| dir.is_dir());
        let label = match &dir {
            Some(dir) => display_home_relative(dir),
            None => i18n::t!("chat.no_folder"),
        };
        div()
            .id("chat-status-folder")
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(10.5))
            .child(
                div()
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!("titlebar.chat"))),
            )
            .child(
                div()
                    .font_family("Guguru Sans Code")
                    .text_color(theme.fg2)
                    .child(SharedString::from(label)),
            )
            .when_some(dir, |element, dir| {
                element.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, _window, _cx| {
                        if let Err(error) = project::reveal_in_finder_local(&dir) {
                            eprintln!("フォルダを表示できない: {error:#}");
                        }
                    }),
                )
            })
    }
}

/// `directory` へ `source` を複製する。同名があれば ` 2`・` 3` … を付ける。存在確認ではなく
/// **`create_new` の成否**で決める（確認してから書くと、間に出来たファイルを上書きする）。
fn copy_without_overwrite(source: &Path, directory: &Path) -> std::io::Result<PathBuf> {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = source
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let bytes = std::fs::read(source)?;
    for attempt in 1..=999 {
        let name = if attempt == 1 {
            format!("{stem}{extension}")
        } else {
            format!("{stem} {attempt}{extension}")
        };
        let target = directory.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut file) => {
                use std::io::Write as _;
                file.write_all(&bytes)?;
                return Ok(target);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "同名のファイルが多すぎます",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_never_overwrites_what_the_project_already_has() {
        let base = std::env::temp_dir().join(format!(
            "necoder_chat_copy_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let artifact = base.join("timer.html");
        std::fs::write(&artifact, "new").unwrap();
        std::fs::write(project.join("timer.html"), "mine").unwrap();

        let copied = copy_without_overwrite(&artifact, &project).unwrap();
        assert_eq!(copied, project.join("timer 2.html"));
        assert_eq!(
            std::fs::read_to_string(project.join("timer.html")).unwrap(),
            "mine"
        );
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "new");
        let _ = std::fs::remove_dir_all(base);
    }
}

/// `~/Documents/necoder/…` の形（ホームを `~` に畳む）。
fn display_home_relative(path: &Path) -> String {
    if let Some(home) = paths::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatGroup {
    Pinned,
    Recency(Recency),
}

impl ChatGroup {
    fn label(self) -> String {
        match self {
            ChatGroup::Pinned => i18n::t!("chat.group_pinned"),
            ChatGroup::Recency(Recency::Today) => i18n::t!("chat.group_today"),
            ChatGroup::Recency(Recency::Yesterday) => i18n::t!("chat.group_yesterday"),
            ChatGroup::Recency(Recency::ThisWeek) => i18n::t!("chat.group_week"),
            ChatGroup::Recency(Recency::Older) => i18n::t!("chat.group_older"),
        }
    }
}
