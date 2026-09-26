//! history — セッション履歴（O15）のパネル側。
//!
//! 1. **エージェントの過去の会話を開いて続ける** — 履歴ビュー（workspace）が [`AgentPanel::list_agent_sessions`]
//!    で一覧し、選ばれた会話を [`AgentPanel::open_agent_session`] が新しいスレッドとして開く。起こした
//!    セッションは `session/load` で再開し、再生された履歴（`AgentEvent::HistoryReplayed`）を transcript に
//!    積む（[`entries_from_replay`]）。**prompt は送らない**。開いた時点で DB にスレッドと会話 id を書くので、
//!    以後は necoder のスレッドとして履歴に出る（一覧からは重複として消える）
//! 2. **全文検索の一致へ飛ぶ** — [`AgentPanel::open_thread_at_turn`] が、一致した turn まで読み込んで開く
//!    （強調と位置合わせは `search.rs` の [`AgentPanel::reveal_transcript_match`]）
//! 3. **新しいセッションで続ける（handoff）** — 文脈窓が膨らんだ時の逃げ道。今の会話の要点を前置きに
//!    して、同じタブで新しいセッションを始める（[`AgentPanel::continue_in_new_session`]）。前置きは
//!    **要約しない**: 直近のやり取りと人が頼んだことを機械的に切り出す（[`handoff_preamble`]）。
//!    要約に `claude -p` などの子プロセスを使うと課金が起きるため
//! 4. **スレッドの右クリックメニュー**（名前を変更 / 新しいセッションで続ける / 閉じる）

use super::*;
use acp_client::history::{AgentSessionSummary, ReplayItem, SessionListing};

/// 履歴ビューが一覧で読む「エージェントの過去の会話」の上限。
pub const AGENT_SESSION_LIMIT: usize = 100;

/// 前置きに入れる「人が頼んだこと」の数（直近から）と、1 件の長さの上限（文字）。
const HANDOFF_REQUESTS: usize = 8;
const HANDOFF_REQUEST_CHARS: usize = 400;
/// 前置きに入れる「直近のやり取り」の数（人の発話とエージェントの本文）と、1 件の長さの上限（文字）。
const HANDOFF_RECENT_ENTRIES: usize = 6;
const HANDOFF_RECENT_CHARS: usize = 1_500;
/// 前置きに並べる「編集したファイル」の上限。
const HANDOFF_FILES: usize = 20;

/// スレッドタブ（一覧の行）の右クリックメニュー。
pub(crate) struct ThreadMenu {
    /// 対象のスレッドの位置（開いた時点）。
    pub(crate) index: usize,
    /// 押した場所（窓の座標）。メニューの左上になる。
    pub(crate) position: gpui::Point<Pixels>,
}

impl AgentPanel {
    /// 履歴ビューが一覧するエージェント（いま見ているスレッドのエージェント。無ければ既定）。
    pub fn history_agent(&self, cx: &App) -> SharedString {
        self.threads
            .get(self.active)
            .map(|thread| thread.agent.clone())
            .unwrap_or_else(|| default_agent_name(cx))
    }

    /// 一覧と再開が走る host の表示名（SSH のプロジェクトだけ。ローカルは `None`）。エージェントの
    /// 会話はそのマシンのアカウントの物なので、履歴ビューは一覧の見出しに host を添えて区別する。
    pub fn history_host(&self) -> Option<SharedString> {
        self.dest_host
            .is_remote()
            .then(|| SharedString::from(self.dest_host.display_name().to_string()))
    }

    /// 開いているスレッドが握っている会話 id（まだ DB に書けていない物も含む）。
    pub fn open_session_ids(&self) -> Vec<String> {
        self.threads
            .iter()
            .filter_map(|thread| thread.acp_session_id.clone())
            .collect()
    }

    /// 宛先プロジェクトで `agent` の過去の会話を一覧する（履歴ビューを開いた時だけ呼ぶ・ポーリングしない）。
    /// 一覧のためにエージェントを 1 本だけ起こし、`session/list` を読んだら畳む。コマンドの解決も
    /// 背景で行う（リモートは SSH の往復になるため）。Chat・宛先未定・知らないエージェントは `None`。
    pub fn list_agent_sessions(
        &self,
        agent: &str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Task<anyhow::Result<SessionListing>>> {
        if self.chat_mode {
            return None;
        }
        let cwd = self.dest_cwd.clone()?;
        let kind = acp_client::AgentKind::by_label(agent)?;
        let host = self.dest_host.clone();
        let agent_override = agent_server_override(kind.id, cx);
        let registry = acp_client::registry::load_cached();
        let not_installed = i18n::t!("agent.err_no_acp");
        Some(cx.background_executor().spawn(async move {
            let command = kind
                .resolve_command_on(
                    host.as_ref(),
                    cwd.clone(),
                    agent_override.as_ref(),
                    registry.as_ref(),
                )?
                .ok_or_else(|| anyhow::anyhow!(not_installed))?;
            acp_client::history::list_sessions_on(host, command, cwd, AGENT_SESSION_LIMIT).await
        }))
    }

    /// エージェント側の過去の会話（`session/list` の 1 件）を新しいスレッドとして開き、`session/load` で
    /// 再開する。再生された履歴は届き次第 transcript に積む（`AgentEvent::HistoryReplayed`）。
    /// 同じ会話を既に開いていれば切り替えるだけ。開いたスレッドの位置を返す（Chat・知らない agent は None）。
    pub fn open_agent_session(
        &mut self,
        agent: &str,
        session: &AgentSessionSummary,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if self.chat_mode {
            return None;
        }
        if let Some(index) = self.threads.iter().position(|thread| {
            thread.acp_session_id.as_deref() == Some(session.session_id.as_str())
        }) {
            self.switch_thread(index, cx);
            return Some(index);
        }
        acp_client::AgentKind::by_label(agent)?;
        let index = self.threads.len();
        let name = session
            .title
            .as_deref()
            .and_then(agent_title_for_tab)
            .unwrap_or_else(|| SharedString::from(i18n::t!("agent.history_untitled_session")));
        let mut thread = Thread::empty(name, index);
        thread.agent = SharedString::from(agent.to_string());
        apply_agent_sticky(&mut thread, cx);
        thread.acp_session_id = Some(session.session_id.clone());
        thread.replay_pending = true;
        thread.last_input_at_ms = session.updated_at_ms;
        thread.session_note = Some(SharedString::from(i18n::t!("agent.history_loading")));
        self.save_active_draft(cx);
        self.threads.push(thread);
        self.active = index;
        self.renaming = None;
        let color = self.active_color();
        self.composer.update(cx, |composer, cx| {
            composer.set_plain_text("", cx);
            composer.set_accent(color, cx);
        });
        self.reset_transcript_list(true);
        self.refresh_transcript_search(cx);
        // 先に DB へ載せる（会話 id も）＝以後は necoder のスレッドとして履歴に出る。
        self.persist_thread(index);
        self.persist_session_id(index);
        // 送信を待たずに起こす＝再生がすぐ transcript に並ぶ（prompt は送らない）。
        match self.session_cwd(index) {
            Some(cwd) => match self.start_session(index, cwd, cx) {
                Some((command_tx, serial)) => {
                    if let Some(thread) = self.threads.get_mut(index) {
                        thread.command_tx = Some(command_tx);
                        thread.session_serial = serial;
                        thread.session_lost = false;
                    }
                }
                None => self.fail_turn(index, &i18n::t!("agent.err_no_acp"), cx),
            },
            None => self.fail_turn(index, &i18n::t!("agent.err_no_project"), cx),
        }
        self.sync_running_registry(cx);
        cx.notify();
        Some(index)
    }

    /// 全文検索の一致からスレッドを開く。一致した turn（`turn_id`）が直近 [`RESTORED_TURNS`] 件の外に
    /// あっても transcript に入るよう、読み込む数を広げる（前に少し余白を持たせる）。開いていれば切替。
    #[allow(clippy::too_many_arguments)]
    pub fn open_thread_at_turn(
        &mut self,
        id: &str,
        name: &str,
        color_index: usize,
        created_at_ms: i64,
        last_input_at_ms: Option<i64>,
        turn_id: i64,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let turn_limit = match self
            .storage
            .as_ref()
            .map(|storage| storage.count_turns_since(id, turn_id))
        {
            Some(Ok(count)) => (count + 10).max(RESTORED_TURNS),
            Some(Err(error)) => {
                eprintln!("一致した発話の位置を数えられない（直近だけ開く）: {error:#}");
                RESTORED_TURNS
            }
            None => RESTORED_TURNS,
        };
        self.open_stored_thread(
            id,
            name,
            color_index,
            created_at_ms,
            last_input_at_ms,
            turn_limit,
            cx,
        )
    }

    /// 「新しいセッションで続ける」を今使えるか: 会話があり、実行中・承認待ち・回答待ちでない。
    pub fn can_continue_in_new_session(&self, index: usize) -> bool {
        self.threads.get(index).is_some_and(|thread| {
            !thread.running
                && thread.pending_permission.is_none()
                && thread.pending_elicitation.is_none()
                && thread
                    .entries
                    .iter()
                    .any(|entry| matches!(entry, Entry::User(_) | Entry::Agent(_)))
        })
    }

    /// 新しいセッションで続ける（handoff）。今のセッションを畳んで会話 id を忘れ、同じタブで
    /// `session/new` から始め直す。次の通常の送信の頭に、今の会話の要点（[`handoff_preamble`]）を
    /// 1 回だけ付ける。transcript には区切りを 1 行残す（前の会話はそのまま読める）。
    /// 使えない時（[`Self::can_continue_in_new_session`]）は何もしない（`false`）。
    pub fn continue_in_new_session(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        if !self.can_continue_in_new_session(index) {
            return false;
        }
        let Some(thread) = self.threads.get_mut(index) else {
            return false;
        };
        let Some(preamble) = handoff_preamble(&thread.entries, &thread.touched_files) else {
            return false;
        };
        // 自分で畳んだセッションの終わりを「切れました」と報告させない（通し番号を外す）。
        thread.command_tx = None;
        thread.session_serial = 0;
        thread.acp_session_id = None;
        thread.session_used = false;
        thread.session_lost = false;
        thread.session_resumable = false;
        thread.auth_required = false;
        thread.replay_pending = false;
        thread.tokens_used = 0;
        thread.tokens_shown = 0.0;
        thread.plan.clear();
        thread.goal = None;
        thread.handoff_preamble = Some(preamble);
        thread.session_note = Some(SharedString::from(i18n::t!("agent.handoff_note")));
        thread
            .entries
            .push(Entry::Notice(SharedString::from(i18n::t!(
                "agent.handoff_divider"
            ))));
        let thread_id = thread.id.clone();
        self.prewarmed.remove(&thread_id);
        self.prewarm_order.retain(|id| *id != thread_id);
        // 再起動しても前の会話を `session/load` しないよう、DB の会話 id も忘れる。
        if let Some(storage) = self.storage.clone() {
            let forgotten = thread_id.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.clear_thread_session(&forgotten) {
                        eprintln!("新しいセッションで続ける: 前の会話 id を消せない: {error:#}");
                    }
                })
                .detach();
        }
        self.persist_thread(index);
        // 送信を待たずに新しいセッションを起こす（ピルとトークンの表示が新しい会話に揃う）。
        // エージェントが無ければ黙る（送信すれば同じ経路でエラーが出る）。
        if let Some(cwd) = self.session_cwd(index) {
            if let Some((command_tx, serial)) = self.start_session(index, cwd, cx) {
                if let Some(thread) = self.threads.get_mut(index) {
                    thread.command_tx = Some(command_tx);
                    thread.session_serial = serial;
                }
            }
        }
        if index == self.active {
            self.reset_transcript_list(true);
        }
        self.sync_running_registry(cx);
        cx.notify();
        true
    }

    /// id でスレッドを探して [`Self::continue_in_new_session`]（Chat の一覧の行のメニューから）。
    /// Chat で閉じているチャットは先に開く。
    pub fn continue_thread_in_new_session(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if self.thread_index_by_id(id).is_none() && self.chat_mode {
            self.open_chat(id, cx);
        }
        let Some(index) = self.thread_index_by_id(id) else {
            return false;
        };
        self.continue_in_new_session(index, cx)
    }

    /// タブ（一覧の行）の右クリックでメニューを開く。**アクティブなタブは変えない**。
    pub(crate) fn open_thread_menu(
        &mut self,
        index: usize,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if index >= self.threads.len() {
            return;
        }
        self.open_menu = None;
        self.thread_menu = Some(ThreadMenu { index, position });
        cx.notify();
    }

    /// メニューを閉じる。開いていたら `true`。
    pub(crate) fn close_thread_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if self.thread_menu.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// 開発用（offscreen 検証）: スレッドのメニューを開く。
    #[cfg(debug_assertions)]
    pub fn debug_open_thread_menu(
        &mut self,
        index: usize,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_thread_menu(index, position, cx);
    }

    /// スレッドの右クリックメニュー。面は選択ピルのメニューと同じ（bg2・枠・角 8・影）で、色は中立。
    /// 窓の座標で押した場所に出す（パネルの枠に切られないよう最前面へ遅らせて描く）。
    pub(crate) fn render_thread_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.thread_menu.as_ref()?;
        let index = menu.index;
        self.threads.get(index)?;
        let theme = self.theme.clone();
        let can_continue = self.can_continue_in_new_session(index);
        let item = |id: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(if enabled { theme.fg1 } else { theme.fg2 })
                .when(enabled, |element| {
                    element
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                })
                .child(label)
        };
        let menu_box = div()
            .w(px(230.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            // メニューの外を押したら閉じる（別のタブ・transcript・パネルの外を含む）。
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_thread_menu(cx);
            }))
            .child(
                item(
                    "thread-menu-rename",
                    i18n::t!("agent.thread_menu_rename"),
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.thread_menu = None;
                        this.start_rename(index, window, cx);
                    }),
                ),
            )
            .child(
                item(
                    "thread-menu-continue",
                    i18n::t!("agent.thread_menu_continue"),
                    can_continue,
                )
                .tooltip(Tooltip::text(
                    i18n::t!("agent.thread_menu_continue_tip"),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        cx.stop_propagation();
                        if !this.can_continue_in_new_session(index) {
                            return; // 押せない行（色で示している）はメニューも閉じない
                        }
                        this.thread_menu = None;
                        this.continue_in_new_session(index, cx);
                    }),
                ),
            )
            .child(div().my(px(3.)).h(px(1.)).bg(theme.border))
            .child(
                item(
                    "thread-menu-close",
                    i18n::t!("agent.thread_menu_close"),
                    true,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        cx.stop_propagation();
                        this.thread_menu = None;
                        this.remove_thread(index, cx);
                    }),
                ),
            );
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(menu.position)
                    .snap_to_window()
                    .child(menu_box),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// 再生された履歴（[`ReplayItem`]）を transcript のエントリへ。上限で捨てた分があれば先頭に
/// 「以前の N 件は省略」の区切りを置く。ツールは後から更新が来ないので相関 id を持たせない。
pub(crate) fn entries_from_replay(items: Vec<ReplayItem>, omitted: usize) -> Vec<Entry> {
    let mut entries = Vec::with_capacity(items.len() + 1);
    if omitted > 0 {
        entries.push(Entry::Notice(SharedString::from(i18n::t!(
            "agent.history_omitted",
            "count" => omitted
        ))));
    }
    for item in items {
        entries.push(match item {
            ReplayItem::User(text) => Entry::User(SharedString::from(text)),
            ReplayItem::Agent(text) => Entry::Agent(SharedString::from(text)),
            ReplayItem::Tool(info) => {
                let mut entry = build_step_entry(info);
                if let Entry::Step { id, .. } = &mut entry {
                    *id = None;
                }
                entry
            }
        });
    }
    entries
}

/// 「新しいセッションで続ける」の前置き。**要約しない** — 人が頼んだこと（直近 8 件）・直近のやり取り
/// （人の発話とエージェントの本文を 6 件）・このスレッドで編集したファイルを、それぞれ長さを切って
/// 並べる。最後に「今回の依頼」の見出しを置き、その後ろに人の新しい発話が続く。会話が無ければ `None`。
pub(crate) fn handoff_preamble(entries: &[Entry], touched_files: &[PathBuf]) -> Option<String> {
    let requests: Vec<&SharedString> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::User(text) => Some(text),
            _ => None,
        })
        .collect();
    let mut recent: Vec<&Entry> = entries
        .iter()
        .rev()
        .filter(|entry| matches!(entry, Entry::User(_) | Entry::Agent(_)))
        .take(HANDOFF_RECENT_ENTRIES)
        .collect();
    recent.reverse();
    if requests.is_empty() && recent.is_empty() {
        return None;
    }
    let mut text = i18n::t!("agent.handoff_preamble_intro");
    text.push_str("\n\n");
    if !requests.is_empty() {
        text.push_str(&i18n::t!("agent.handoff_preamble_requests"));
        text.push('\n');
        let skip = requests.len().saturating_sub(HANDOFF_REQUESTS);
        for (number, request) in requests.iter().skip(skip).enumerate() {
            let request = clip_middle(request, HANDOFF_REQUEST_CHARS).replace('\n', "\n   ");
            text.push_str(&format!("{}. {request}\n", number + 1));
        }
        text.push('\n');
    }
    if !recent.is_empty() {
        text.push_str(&i18n::t!("agent.handoff_preamble_recent"));
        text.push('\n');
        for entry in recent {
            let (speaker, body) = match entry {
                Entry::User(body) => (i18n::t!("agent.handoff_preamble_you"), body),
                Entry::Agent(body) => (i18n::t!("agent.handoff_preamble_agent"), body),
                _ => continue,
            };
            text.push_str(&format!(
                "{speaker}\n{}\n\n",
                clip_middle(body, HANDOFF_RECENT_CHARS)
            ));
        }
    }
    if !touched_files.is_empty() {
        text.push_str(&i18n::t!("agent.handoff_preamble_files"));
        text.push('\n');
        for file in touched_files.iter().take(HANDOFF_FILES) {
            text.push_str(&format!("- {}\n", file.display()));
        }
        text.push('\n');
    }
    text.push_str(&i18n::t!("agent.handoff_preamble_request_now"));
    Some(text)
}

/// 長い文を頭（2/3）と尻（1/3）だけ残して切る。エージェントの本文は結論が末尾に来やすく、人の依頼は
/// 頭に要点が来やすいので、両端を残す。
fn clip_middle(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head = max_chars * 2 / 3;
    let tail = max_chars - head;
    let head_text: String = text.chars().take(head).collect();
    let tail_text: String = text.chars().skip(count - tail).collect();
    format!("{head_text}\n…\n{tail_text}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client::ToolCallInfo;

    fn plain(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::User(text) => format!("user:{text}"),
                Entry::Agent(text) => format!("agent:{text}"),
                Entry::Notice(text) => format!("notice:{text}"),
                Entry::Step {
                    id, tool, result, ..
                } => format!(
                    "step:{tool}:{}:{}",
                    id.is_some(),
                    result.as_deref().unwrap_or("")
                ),
                other => format!("other:{}", entry_plain_text(other)),
            })
            .collect()
    }

    /// 再生された update（を畳んだ項目）→ transcript のエントリ。省略があれば先頭に区切りを置く。
    #[test]
    fn replayed_items_become_transcript_entries() {
        let items = vec![
            ReplayItem::User("README を直して".into()),
            ReplayItem::Agent("読みます。".into()),
            ReplayItem::Tool(ToolCallInfo {
                id: "toolu_1".into(),
                title: Some("Read README.md".into()),
                kind: Some(acp_client::ToolCallKind::Read),
                locations: vec!["/repo/README.md".into()],
                diffs: Vec::new(),
                output: Some("# necoder".into()),
                completed: Some(true),
            }),
            ReplayItem::Agent("直しました。".into()),
        ];
        let entries = entries_from_replay(items.clone(), 0);
        assert_eq!(
            plain(&entries),
            vec![
                "user:README を直して".to_string(),
                "agent:読みます。".to_string(),
                "step:Read README.md:false:# necoder".to_string(),
                "agent:直しました。".to_string(),
            ],
            "ツールは後から更新が来ないので相関 id を持たない"
        );
        let entries = entries_from_replay(items, 34);
        assert_eq!(entries.len(), 5);
        match &entries[0] {
            Entry::Notice(text) => assert!(text.contains("34"), "{text}"),
            _ => panic!("先頭は省略の区切り"),
        }
    }

    /// 引き継ぎの前置き: 人が頼んだこと（古い順・直近 8 件）・直近のやり取り（6 件・区切りやツールは
    /// 入れない）・編集したファイル。長い本文は両端を残して切る。会話が無ければ作らない。
    #[test]
    fn handoff_preamble_quotes_requests_and_the_latest_exchange() {
        assert!(handoff_preamble(&[], &[]).is_none());
        assert!(handoff_preamble(&[Entry::Notice("区切り".into())], &[]).is_none());

        let mut entries = Vec::new();
        for number in 1..=10 {
            entries.push(Entry::User(format!("依頼{number}").into()));
            entries.push(Entry::Step {
                id: None,
                tool: format!("Read file{number}.rs").into(),
                args: SharedString::default(),
                result: None,
                result_lines: 0,
                diffs: Vec::new(),
            });
            entries.push(Entry::Agent(format!("回答{number}").into()));
        }
        entries.push(Entry::Notice("区切りの知らせ".into()));
        let long_answer = format!("結論の前置き{}最後の結論", "あ".repeat(3_000));
        entries.push(Entry::Agent(long_answer.into()));
        let touched = vec![PathBuf::from("/repo/src/rope.rs")];
        let preamble = handoff_preamble(&entries, &touched).expect("会話がある");

        // 人が頼んだことは直近 8 件だけ・古い順。
        assert!(!preamble.contains("1. 依頼1\n"));
        assert!(!preamble.contains("依頼2\n"));
        let third = preamble.find("1. 依頼3").expect("依頼3 から");
        let tenth = preamble.find("8. 依頼10").expect("依頼10 まで");
        assert!(third < tenth);
        // 直近のやり取りは 6 件（人・エージェントの本文だけ）。区切り・ツールは入らない。
        let recent = &preamble[tenth..];
        assert!(recent.contains("回答8"));
        assert!(!recent.contains("回答7"));
        assert!(!preamble.contains("Read file"));
        assert!(!preamble.contains("区切りの知らせ"));
        // 長い本文は頭と尻を残して切る。
        assert!(preamble.contains("結論の前置き"));
        assert!(preamble.contains("最後の結論"));
        assert!(preamble.chars().count() < 8_000, "{}", preamble.len());
        assert!(preamble.contains("/repo/src/rope.rs"));
        // 最後は「今回の依頼」の見出し（この後ろに人の新しい発話が続く）。
        assert!(preamble
            .trim_end()
            .ends_with(i18n::t!("agent.handoff_preamble_request_now").trim_end()));
    }

    /// 一時の設定（先張りしない）でパネルを作る。実エージェントを起こさないため宛先は決めない。
    fn panel_with_temp_settings<'a>(
        cx: &'a mut gpui::TestAppContext,
        label: &str,
    ) -> (Entity<AgentPanel>, &'a mut gpui::VisualTestContext, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "necoder_agent_history_{label}_{}_{}",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&root).expect("作業ディレクトリを作れる");
        let settings = root.join("settings.json");
        std::fs::write(&settings, r#"{"onboarded":true,"agent_prewarm":false}"#)
            .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings), None, cx));
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        (panel, cx, root)
    }

    /// 再生は 1 つのスレッドに 1 回だけ、開いた時点の transcript の前に積む。load を待つ間に送った
    /// 発話も、後から来る live の本文も、再生と二重にならない。
    #[gpui::test]
    fn a_replayed_conversation_lands_once_before_live_entries(cx: &mut gpui::TestAppContext) {
        let (panel, cx, root) = panel_with_temp_settings(cx, "replay");
        panel.update_in(cx, |panel, _window, cx| {
            let index = panel.threads.len();
            let mut thread = Thread::empty("CLI の会話", index);
            thread.acp_session_id = Some("cli-session".into());
            thread.replay_pending = true;
            // load を待つ間に人が送った発話（コマンドは load の後に処理される）。
            thread.entries.push(Entry::User("続きをお願い".into()));
            panel.threads.push(thread);
            panel.active = index;

            let replay = || AgentEvent::HistoryReplayed {
                items: vec![
                    ReplayItem::User("README を直して".into()),
                    ReplayItem::Agent("直しました。".into()),
                ],
                omitted: 0,
            };
            panel.on_event(index, replay(), cx);
            panel.on_event(index, replay(), cx); // 2 回目は捨てる
            panel.on_event(
                index,
                AgentEvent::SessionStarted {
                    session_id: "cli-session".into(),
                    resumed: true,
                    resumable: true,
                },
                cx,
            );
            panel.on_event(index, AgentEvent::TurnStarted, cx);
            panel.on_event(index, AgentEvent::AgentChunk("続けます。".into()), cx);
            let thread = &panel.threads[index];
            assert_eq!(
                plain(&thread.entries),
                vec![
                    "user:README を直して".to_string(),
                    "agent:直しました。".to_string(),
                    "user:続きをお願い".to_string(),
                    "agent:続けます。".to_string(),
                ]
            );
            assert!(!thread.replay_pending);
            assert_eq!(thread.last_prompt.as_deref(), Some("続きをお願い"));
            // 頼んでいないスレッドに届いた再生は捨てる。
            panel.on_event(index, replay(), cx);
            assert_eq!(panel.threads[index].entries.len(), 4);
        });
        std::fs::remove_dir_all(&root).expect("片付けられる");
    }

    /// 新しいセッションで続ける: 会話 id とトークンを捨て、区切りを 1 行残し、次の通常の送信にだけ
    /// 前置きを付ける（表示は人の本文のまま）。実行中は使えない。
    #[gpui::test]
    fn continuing_in_a_new_session_carries_the_preamble_once(cx: &mut gpui::TestAppContext) {
        let (panel, cx, root) = panel_with_temp_settings(cx, "handoff");
        let (command_tx, mut command_rx) = mpsc::unbounded::<SessionCommand>();
        panel.update_in(cx, |panel, _window, cx| {
            let index = panel.active;
            {
                let thread = &mut panel.threads[index];
                thread.entries = vec![
                    Entry::User("rope のバグを直して".into()),
                    Entry::Agent("境界を直しました。".into()),
                ];
                thread.acp_session_id = Some("old-session".into());
                thread.tokens_used = 150_000;
            }
            assert!(panel.continue_in_new_session(index, cx));
            {
                let thread = &panel.threads[index];
                assert_eq!(thread.acp_session_id, None);
                assert_eq!(thread.tokens_used, 0);
                assert!(thread.handoff_preamble.is_some());
                assert!(matches!(thread.entries.last(), Some(Entry::Notice(_))));
                assert!(thread.session_note.is_some());
            }

            // 宛先と送信路を用意して送る（実エージェントは起こさない）。
            panel.dest_cwd = Some(root.clone());
            panel.threads[index].command_tx = Some(command_tx.clone());
            panel.send_prompt_text("続きをお願い".into(), cx);
            assert!(
                panel.threads[index].handoff_preamble.is_none(),
                "1 回で捨てる"
            );
            assert!(matches!(
                panel.threads[index].entries.last(),
                Some(Entry::User(text)) if text.as_ref() == "続きをお願い"
            ));
            assert!(
                !panel.continue_in_new_session(index, cx),
                "実行中は使えない"
            );
            panel.threads[index].running = false;
            panel.send_prompt_text("もう一つ".into(), cx);
        });
        let first = match command_rx.try_recv() {
            Ok(SessionCommand::Prompt(prompt)) => prompt,
            other => panic!("前置き付きの prompt を期待: {other:?}"),
        };
        assert!(first.starts_with(&i18n::t!("agent.handoff_preamble_intro")));
        assert!(first.contains("rope のバグを直して"));
        assert!(first.contains("境界を直しました。"));
        assert!(first.trim_end().ends_with("続きをお願い"));
        match command_rx.try_recv() {
            Ok(SessionCommand::Prompt(prompt)) => assert_eq!(prompt, "もう一つ"),
            other => panic!("前置きの無い prompt を期待: {other:?}"),
        }
        std::fs::remove_dir_all(&root).expect("片付けられる");
    }

    /// 全文検索の一致へ飛ぶ: 一致が直近 200 turn の外でも読み込み、検索バーでその発話を見せる。
    #[gpui::test]
    fn a_search_hit_beyond_the_restore_window_is_loaded_and_revealed(
        cx: &mut gpui::TestAppContext,
    ) {
        let (panel, cx, root) = panel_with_temp_settings(cx, "search_hit");
        let storage = storage::Storage::open(&root.join("necoder.db")).expect("DB を開ける");
        storage
            .upsert_thread("long", "長いスレッド", 2, "scope", None, None, None, 0, 0)
            .expect("書ける");
        storage
            .insert_turn("long", "user", "境界値のバグを探して")
            .expect("書ける");
        for number in 0..260 {
            storage
                .insert_turn("long", "agent", &format!("作業 {number}"))
                .expect("書ける");
        }
        let hit = storage
            .search_thread_turns(None, true, "境界値", 10)
            .expect("探せる")
            .pop()
            .expect("一致がある");
        panel.update_in(cx, |panel, window, cx| {
            panel.storage = Some(storage.clone());
            let index = panel
                .open_thread_at_turn(&hit.thread_id, "長いスレッド", 2, 0, None, hit.turn_id, cx)
                .expect("開ける");
            assert_eq!(panel.active, index);
            assert_eq!(
                panel.threads[index].entries.len(),
                261,
                "一致した発話まで読み込む"
            );
            panel.reveal_transcript_match("境界値", window, cx);
            let search = panel.transcript_search.as_ref().expect("検索バーが開く");
            assert_eq!(search.entries, vec![0]);
            assert_eq!(search.current, 0);
        });
        std::fs::remove_dir_all(&root).expect("片付けられる");
    }

    #[test]
    fn clip_middle_keeps_both_ends() {
        assert_eq!(clip_middle("  短い  ", 10), "短い");
        let clipped = clip_middle(&format!("頭{}尻", "x".repeat(100)), 30);
        assert!(clipped.starts_with("頭"));
        assert!(clipped.ends_with("尻"));
        assert!(clipped.contains("\n…\n"));
    }
}
