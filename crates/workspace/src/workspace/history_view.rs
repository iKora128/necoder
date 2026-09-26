//! history_view — スレッド履歴（⌘⇧H・🕘・O15）。Picker を置き換えた専用のオーバーレイ。
//!
//! 3 つの区分を 1 つのリストに並べる:
//!
//! 1. **このプロジェクトのスレッド** — necoder の DB のスレッド（閉じた物を含む・新しい順）。入力で名前を
//!    絞る。絞り込みの鍵はパネルの復元と同じ「TaskSpace の stable id + 旧版の表示名」
//! 2. **エージェントの過去の会話** — いまのスレッドのエージェントに `session/list` で訊いた会話（CLI で
//!    作った物を含む）。**開いた時に 1 回だけ**訊く（ポーリングしない・一覧のためにエージェントを 1 本
//!    起こして畳む・prompt は送らない）。necoder が既に持つ会話（DB と開いているタブの会話 id）は出さない。
//!    SSH のプロジェクトではリモートのエージェントに訊くので、見出しに host を添える（会話はその
//!    マシンのアカウントの物）
//! 3. **本文の一致（全スレッド）** — 入力があれば Editor / Fleet / Chat の全スレッドの本文を全文検索する
//!    （件数と抜粋の長さに上限・storage）。選ぶとそのスレッドを開き、一致した発話へ飛んで強調する
//!
//! 入力は IME の正しい `EditorView::plain`（Picker の手書きキー入力では日本語で検索できない）。
//! ↑↓ で選び ⏎ で開く・esc で閉じる・背景クリックで閉じる（UI-SPEC §7 の共通）。色は Picker と同じ
//! （行頭●＝スレッド色・選択面＝プロジェクト色 dim）で、状態には色相を使わない。

use crate::workspace::*;
use agent_panel::{AgentSessionSummary, SessionListing};
use std::collections::HashSet;
use std::time::Duration;

/// 全文検索で出す件数（1 スレッド 1 件）。storage 側にも上限がある。
const HISTORY_MATCH_LIMIT: usize = 50;
/// 打つたびに DB を引かないよう、入力が止まってから探すまでの間。
const HISTORY_SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// 開いている履歴ビューの状態（`WorkspaceOverlays::thread_history`）。
pub(crate) struct ThreadHistoryState {
    /// 入力欄（IME の正しい `EditorView::plain`）。
    input: Entity<EditorView>,
    /// いまの入力（入力欄の写し）。
    query: String,
    /// 開く前にフォーカスがあった所（何も開かずに閉じたら返す）。
    previous_focus: Option<FocusHandle>,
    /// このプロジェクトのスレッド（DB・新しい順・閉じた物を含む）。
    threads: Vec<HistoryThread>,
    /// 一覧を訊いたエージェント（ラベル）と、それが動く host（SSH の時だけ）。
    agent: SharedString,
    host: Option<SharedString>,
    /// 一覧を訊いたパネル（開いた時点のプロジェクトの物）。会話はこのパネルで開く。
    panel: Entity<AgentPanel>,
    /// このプロジェクトのルート（一覧の会話が別の worktree の物か見分ける）。
    root: PathBuf,
    agent_sessions: AgentSessions,
    matches: HistoryMatches,
    /// 全文検索の世代（古い検索の結果が新しい入力の結果を上書きしないため）。
    search_generation: u64,
    /// 選べる行のうち何番目を選んでいるか。
    selected: usize,
    scroll: gpui::ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

/// このプロジェクトのスレッド 1 行。
#[derive(Clone, Debug)]
struct HistoryThread {
    id: String,
    name: String,
    color_index: i64,
    branch: Option<String>,
    tokens_used: i64,
    archived: bool,
    created_at: i64,
    last_input_at: Option<i64>,
}

/// 「エージェントの過去の会話」の状態。
enum AgentSessions {
    /// 一覧を訊けない（宛先の無い窓・知らないエージェント）。区分ごと出さない。
    Unavailable,
    Loading,
    Loaded(Vec<AgentSessionSummary>),
    /// エージェントが `session/list` を広告していない。
    Unsupported,
    Failed(SharedString),
}

/// 全文検索の状態。
enum HistoryMatches {
    /// 入力が空（区分を出さない）。
    Idle,
    Searching,
    Loaded(Vec<HistoryMatch>),
    Failed(SharedString),
}

/// 全文検索の一致 1 件。
struct HistoryMatch {
    hit: storage::TurnSearchHit,
    /// どこの会話か（プロジェクト名・SSH なら host 付き / Chat）。
    place: SharedString,
    /// 一致の前後を 1 行に畳んだ物。
    snippet: SharedString,
}

/// リストの 1 行。
#[derive(Clone, Debug, PartialEq)]
enum HistoryRow {
    Header(SharedString),
    /// 読み込み中・空・失敗の一言（選べない）。
    Message(SharedString),
    Thread(usize),
    AgentSession(usize),
    Match(usize),
}

impl HistoryRow {
    fn selectable(&self) -> bool {
        matches!(
            self,
            HistoryRow::Thread(_) | HistoryRow::AgentSession(_) | HistoryRow::Match(_)
        )
    }
}

impl ThreadHistoryState {
    /// いまの入力と読み込み状態から、リストの行を組む。
    fn rows(&self) -> Vec<HistoryRow> {
        let query = self.query.trim();
        let mut rows = Vec::new();

        let threads: Vec<usize> = (0..self.threads.len())
            .filter(|index| {
                query.is_empty() || ui::fuzzy_score(query, &self.threads[*index].name).is_some()
            })
            .collect();
        if query.is_empty() || !threads.is_empty() {
            rows.push(HistoryRow::Header(SharedString::from(i18n::t!(
                "agent.history_section_threads"
            ))));
            if threads.is_empty() {
                rows.push(HistoryRow::Message(SharedString::from(i18n::t!(
                    "agent.history_threads_empty"
                ))));
            }
            rows.extend(threads.into_iter().map(HistoryRow::Thread));
        }

        let agent_rows: Vec<HistoryRow> = match &self.agent_sessions {
            AgentSessions::Unavailable => Vec::new(),
            AgentSessions::Loading => vec![HistoryRow::Message(SharedString::from(i18n::t!(
                "agent.history_agent_loading"
            )))],
            AgentSessions::Unsupported => {
                vec![HistoryRow::Message(SharedString::from(i18n::t!(
                    "agent.history_agent_unsupported",
                    "agent" => &self.agent
                )))]
            }
            AgentSessions::Failed(error) => {
                vec![HistoryRow::Message(SharedString::from(i18n::t!(
                    "agent.history_agent_failed",
                    "error" => error
                )))]
            }
            AgentSessions::Loaded(sessions) if sessions.is_empty() => {
                vec![HistoryRow::Message(SharedString::from(i18n::t!(
                    "agent.history_agent_empty"
                )))]
            }
            AgentSessions::Loaded(sessions) => (0..sessions.len())
                .filter(|index| {
                    query.is_empty()
                        || ui::fuzzy_score(query, &agent_session_title(&sessions[*index])).is_some()
                })
                .map(HistoryRow::AgentSession)
                .collect(),
        };
        if !agent_rows.is_empty() {
            let header = match &self.host {
                Some(host) => i18n::t!(
                    "agent.history_section_agent_remote",
                    "agent" => &self.agent,
                    "host" => host
                ),
                None => i18n::t!("agent.history_section_agent", "agent" => &self.agent),
            };
            rows.push(HistoryRow::Header(SharedString::from(header)));
            rows.extend(agent_rows);
        }

        let match_rows: Vec<HistoryRow> = match &self.matches {
            HistoryMatches::Idle => Vec::new(),
            HistoryMatches::Searching => vec![HistoryRow::Message(SharedString::from(i18n::t!(
                "agent.history_matches_searching"
            )))],
            HistoryMatches::Failed(error) => {
                vec![HistoryRow::Message(SharedString::from(i18n::t!(
                    "agent.history_matches_failed",
                    "error" => error
                )))]
            }
            HistoryMatches::Loaded(matches) if matches.is_empty() => {
                vec![HistoryRow::Message(SharedString::from(i18n::t!(
                    "agent.history_matches_empty"
                )))]
            }
            HistoryMatches::Loaded(matches) => (0..matches.len()).map(HistoryRow::Match).collect(),
        };
        if !query.is_empty() && !match_rows.is_empty() {
            rows.push(HistoryRow::Header(SharedString::from(i18n::t!(
                "agent.history_section_matches"
            ))));
            rows.extend(match_rows);
        }
        rows
    }
}

/// 一覧の会話の見出し（題が無ければ「題のない会話」）。
fn agent_session_title(session: &AgentSessionSummary) -> String {
    session
        .title
        .as_deref()
        .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| i18n::t!("agent.history_untitled_session"))
}

/// 選べる行の位置（`rows` の添字）。
fn selectable_positions(rows: &[HistoryRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.selectable())
        .map(|(position, _)| position)
        .collect()
}

impl Workspace {
    /// スレッド履歴を開く（⌘⇧H / 🕘 / パレット「AI: スレッド履歴」・#5 / O15）。**アクティブプロジェクトの**
    /// スレッドを出し、エージェントの過去の会話を 1 回だけ訊きに行き、入力で全スレッドの本文を探す。
    pub(crate) fn open_thread_history(
        &mut self,
        _: &ThreadHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        let Some((scope, legacy, root)) = self.active_slot().map(|slot| {
            (
                slot.task_space.id.as_str().to_string(),
                slot.name.to_string(),
                slot.worktree.root().to_path_buf(),
            )
        }) else {
            return;
        };
        if self.overlays.picker.is_some() {
            self.close_picker(window, cx);
        }
        let previous_focus = match self.overlays.thread_history.take() {
            Some(previous) => previous.previous_focus,
            None => window.focused(cx),
        };
        let threads: Vec<HistoryThread> = storage
            .load_all_threads()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.3 == scope || row.3 == legacy)
            .map(
                |(
                    id,
                    name,
                    color_index,
                    _project,
                    branch,
                    tokens_used,
                    archived,
                    created_at,
                    last_input_at,
                )| HistoryThread {
                    id,
                    name,
                    color_index,
                    branch,
                    tokens_used,
                    archived,
                    created_at,
                    last_input_at,
                },
            )
            .collect();

        let panel = self.agent_panel.clone();
        let (agent, host) = {
            let panel = panel.read(cx);
            (panel.history_agent(cx), panel.history_host())
        };
        let listing = panel.update(cx, |panel, cx| panel.list_agent_sessions(&agent, cx));
        let agent_sessions = if listing.is_some() {
            AgentSessions::Loading
        } else {
            AgentSessions::Unavailable
        };

        let accent = self
            .active_slot()
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0));
        let input = cx.new(|cx| EditorView::plain(self.theme.clone(), accent, true, cx));
        let subscriptions = vec![
            cx.observe(&input, |workspace, _, cx| {
                workspace.on_history_query_changed(cx)
            }),
            cx.subscribe_in(&input, window, |workspace, _, event, window, cx| {
                if matches!(event, ComposerEvent::Submit) {
                    workspace.confirm_history_selection(window, cx);
                }
            }),
        ];
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.overlays.thread_history = Some(ThreadHistoryState {
            input,
            query: String::new(),
            previous_focus,
            threads,
            agent,
            host,
            panel: panel.clone(),
            root,
            agent_sessions,
            matches: HistoryMatches::Idle,
            search_generation: 0,
            selected: 0,
            scroll: gpui::ScrollHandle::new(),
            _subscriptions: subscriptions,
        });
        cx.notify();

        let Some(listing) = listing else {
            return;
        };
        // 一覧が届いたら、necoder が既に持つ会話（DB のスレッドと、開いているタブ）を除いて並べる。
        let open_sessions: HashSet<String> =
            panel.read(cx).open_session_ids().into_iter().collect();
        cx.spawn(async move |workspace, cx| {
            let listed = listing.await;
            let known = cx
                .background_executor()
                .spawn(async move { storage.load_known_sessions() })
                .await;
            let updated = workspace.update(cx, |workspace, cx| {
                let Some(state) = workspace.overlays.thread_history.as_mut() else {
                    return; // 閉じた
                };
                if state.panel != panel {
                    return; // 別のプロジェクトで開き直した
                }
                state.agent_sessions = match listed {
                    Ok(SessionListing::Listed(sessions)) => {
                        let mut known: HashSet<String> = match known {
                            Ok(known) => known.into_iter().collect(),
                            Err(error) => {
                                eprintln!("履歴: necoder の会話 id を読めない（重複を除けない）: {error:#}");
                                HashSet::new()
                            }
                        };
                        known.extend(open_sessions);
                        AgentSessions::Loaded(agent_panel::fresh_agent_sessions(sessions, &known))
                    }
                    Ok(SessionListing::Unsupported) => AgentSessions::Unsupported,
                    Err(error) => {
                        eprintln!("履歴: エージェントの会話を一覧できない: {error:#}");
                        AgentSessions::Failed(SharedString::from(format!("{error:#}")))
                    }
                };
                workspace.clamp_history_selection();
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("履歴: 一覧を出せない（窓が閉じた）: {error:#}");
            }
        })
        .detach();
    }

    /// パレット「AI: 新しいセッションで続ける」: いま見ているスレッド（Chat ならいまのチャット）を、
    /// 要点を前置きにして新しいセッションで続ける（O15）。使えない時は理由を一言出す。
    pub(crate) fn continue_in_new_session(
        &mut self,
        _: &ContinueInNewSession,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = if self.chat_mode() {
            self.chat_panel()
        } else {
            Some(self.agent_panel.clone())
        };
        let continued = panel.is_some_and(|panel| {
            panel.update(cx, |panel, cx| {
                let index = panel.active_thread();
                panel.continue_in_new_session(index, cx)
            })
        });
        if !continued {
            self.push_toast(
                SharedString::from(i18n::t!("agent.handoff_unavailable")),
                self.theme.fg2,
                cx,
            );
        }
    }

    /// 何も開かずに閉じる（esc・背景クリック）。開く前のフォーカスへ返す。
    pub(crate) fn close_thread_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.thread_history.take() else {
            return;
        };
        if let Some(previous) = state.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    /// 入力が変わった: 選択を先頭へ戻し、空でなければ（少し待ってから）全スレッドの本文を探す。
    fn on_history_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.persistence.storage.clone() else {
            return;
        };
        let Some(state) = self.overlays.thread_history.as_mut() else {
            return;
        };
        let query = state.input.read(cx).plain_text();
        if query == state.query {
            return;
        }
        state.query = query.clone();
        state.selected = 0;
        state.scroll.scroll_to_item(0);
        state.search_generation = state.search_generation.wrapping_add(1);
        let generation = state.search_generation;
        let query = query.trim().to_string();
        if query.is_empty() {
            state.matches = HistoryMatches::Idle;
            cx.notify();
            return;
        }
        state.matches = HistoryMatches::Searching;
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(HISTORY_SEARCH_DEBOUNCE)
                .await;
            let still_current = workspace
                .update(cx, |workspace, _| {
                    workspace
                        .overlays
                        .thread_history
                        .as_ref()
                        .is_some_and(|state| state.search_generation == generation)
                })
                .unwrap_or(false);
            if !still_current {
                return;
            }
            let search_query = query.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let hits = storage.search_thread_turns(
                        None,
                        true,
                        &search_query,
                        HISTORY_MATCH_LIMIT,
                    )?;
                    // 開いていないプロジェクトの名前（TaskSpace の題）。読めなければ id のまま出す。
                    let titles: HashMap<String, String> = match storage.load_task_spaces() {
                        Ok(records) => records
                            .into_iter()
                            .map(|record| (record.id, record.title))
                            .collect(),
                        Err(error) => {
                            eprintln!("履歴: TaskSpace の名前を読めない: {error:#}");
                            HashMap::new()
                        }
                    };
                    anyhow::Ok((hits, titles))
                })
                .await;
            let updated = workspace.update(cx, |workspace, cx| {
                let matches = match result {
                    Ok((hits, titles)) => {
                        let lowercase = query.to_lowercase();
                        HistoryMatches::Loaded(
                            hits.into_iter()
                                .map(|hit| HistoryMatch {
                                    place: workspace.history_place(&hit.project, &titles),
                                    snippet: agent_panel::snippet_around(&hit.excerpt, &lowercase),
                                    hit,
                                })
                                .collect(),
                        )
                    }
                    Err(error) => {
                        eprintln!("履歴: 全文検索に失敗: {error:#}");
                        HistoryMatches::Failed(SharedString::from(format!("{error:#}")))
                    }
                };
                let Some(state) = workspace.overlays.thread_history.as_mut() else {
                    return;
                };
                if state.search_generation != generation {
                    return; // もう次の入力の検索が走っている
                }
                state.matches = matches;
                workspace.clamp_history_selection();
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("履歴: 検索結果を出せない（窓が閉じた）: {error:#}");
            }
        })
        .detach();
    }

    /// 一致したスレッドがどこの会話か（Chat / このウィンドウのプロジェクト名（SSH は host 付き）/
    /// 開いていないプロジェクトの題）。
    fn history_place(&self, project: &str, titles: &HashMap<String, String>) -> SharedString {
        if project == chat_core::STORAGE_SCOPE {
            return SharedString::from(i18n::t!("agent.history_scope_chat"));
        }
        if let Some(slot) =
            self.project_sessions.projects.iter().find(|slot| {
                slot.task_space.id.as_str() == project || slot.name.as_ref() == project
            })
        {
            return match &slot.remote_host {
                Some(host) => SharedString::from(format!("{}（{host}）", slot.name)),
                None => slot.name.clone(),
            };
        }
        titles
            .get(project)
            .map(|title| SharedString::from(title.clone()))
            .unwrap_or_else(|| SharedString::from(project.to_string()))
    }

    /// 行が増減したら選択を範囲に収める。
    fn clamp_history_selection(&mut self) {
        let Some(state) = self.overlays.thread_history.as_mut() else {
            return;
        };
        let count = selectable_positions(&state.rows()).len();
        state.selected = state.selected.min(count.saturating_sub(1));
    }

    /// ↑↓（`delta` = -1 / +1）。端で折り返し、選んだ行が見えるようスクロールする。IME 変換中は入力へ流す。
    fn move_history_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.thread_history.as_mut() else {
            return;
        };
        if state.input.read(cx).has_marked_text() {
            return;
        }
        cx.stop_propagation();
        let positions = selectable_positions(&state.rows());
        if positions.is_empty() {
            return;
        }
        let count = positions.len() as isize;
        state.selected = (state.selected as isize + delta).rem_euclid(count) as usize;
        state.scroll.scroll_to_item(positions[state.selected]);
        cx.notify();
    }

    /// ⏎: 選んでいる行を開く。
    fn confirm_history_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.thread_history.as_ref() else {
            return;
        };
        let rows = state.rows();
        let Some(position) = selectable_positions(&rows).get(state.selected).copied() else {
            return;
        };
        self.confirm_history_row(rows[position].clone(), window, cx);
    }

    /// 行を開く（⏎ / クリック）。ビューを閉じてから、行の種類ごとの先へ移る。
    fn confirm_history_row(
        &mut self,
        row: HistoryRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !row.selectable() {
            return;
        }
        let Some(state) = self.overlays.thread_history.take() else {
            return;
        };
        cx.notify();
        match row {
            HistoryRow::Thread(index) => {
                let Some(thread) = state.threads.get(index).cloned() else {
                    return;
                };
                let panel = state.panel.clone();
                let thread_index = panel.update(cx, |panel, cx| {
                    panel.open_thread_from_history(
                        &thread.id,
                        &thread.name,
                        thread.color_index.max(0) as usize,
                        thread.created_at,
                        thread.last_input_at,
                        cx,
                    )
                });
                self.reveal_history_thread(panel, thread_index, None, window, cx);
            }
            HistoryRow::AgentSession(index) => {
                let AgentSessions::Loaded(sessions) = &state.agent_sessions else {
                    return;
                };
                let Some(session) = sessions.get(index).cloned() else {
                    return;
                };
                let panel = state.panel.clone();
                let agent = state.agent.clone();
                let thread_index = panel.update(cx, |panel, cx| {
                    panel.open_agent_session(&agent, &session, cx)
                });
                self.reveal_history_thread(panel, thread_index, None, window, cx);
            }
            HistoryRow::Match(index) => {
                let HistoryMatches::Loaded(matches) = &state.matches else {
                    return;
                };
                let Some(found) = matches.get(index) else {
                    return;
                };
                let query = state.query.trim().to_string();
                self.open_history_match(found.hit.clone(), query, window, cx);
            }
            HistoryRow::Header(_) | HistoryRow::Message(_) => {}
        }
    }

    /// 開いたスレッドを見せる: Fleet ならセルとして前面へ（M14）、それ以外は Agent ドックを開く。
    /// `query` があれば一致した発話へ飛んで強調する（検索バーに入力が入った状態）。無ければ composer へ。
    fn reveal_history_thread(
        &mut self,
        panel: Entity<AgentPanel>,
        thread_index: Option<usize>,
        query: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chrome.fleet_mode {
            if let Some(thread_index) = thread_index {
                if let Some(space) = self
                    .project_sessions
                    .sessions
                    .iter()
                    .position(|session| session.agent_panel == panel)
                {
                    self.reveal_agent_in_fleet(space, thread_index, window, cx);
                }
            }
        } else if !self.chrome.show_right && !self.chat_mode() {
            self.chrome.show_right = true;
        }
        // 切替・ドックの開閉が決めたフォーカスの後で、パネルの中へ移す。
        window.defer(cx, move |window, cx| {
            panel.update(cx, |panel, cx| match &query {
                Some(query) => panel.reveal_transcript_match(query, window, cx),
                None => panel.focus_composer(window, cx),
            });
        });
        cx.notify();
    }

    /// 全文検索の一致を開く: Chat の会話は Chat で、このウィンドウのプロジェクトの会話はそのプロジェクトへ
    /// 切り替えて開く。開いていないローカルのプロジェクトはレールに開いてから開く。SSH など開けない物は
    /// 知らせるだけ（勝手に別のマシンへ繋がない）。
    fn open_history_match(
        &mut self,
        hit: storage::TurnSearchHit,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hit.project == chat_core::STORAGE_SCOPE {
            self.set_chat_mode(true, window, cx);
            if let Some(panel) = self.chat_panel() {
                panel.update(cx, |panel, cx| panel.open_chat(&hit.thread_id, cx));
                window.defer(cx, move |window, cx| {
                    panel.update(cx, |panel, cx| {
                        panel.reveal_transcript_match(&query, window, cx)
                    });
                });
            }
            return;
        }
        let Some(index) = self
            .history_slot_for(&hit.project)
            .or_else(|| self.open_history_project(&hit.project, cx))
        else {
            let titles = match self
                .persistence
                .storage
                .as_ref()
                .map(|storage| storage.load_task_spaces())
            {
                Some(Ok(records)) => records
                    .into_iter()
                    .map(|record| (record.id, record.title))
                    .collect(),
                _ => HashMap::new(),
            };
            let place = self.history_place(&hit.project, &titles);
            self.push_toast(
                SharedString::from(i18n::t!("agent.history_project_closed", "project" => &place)),
                self.theme.fg2,
                cx,
            );
            return;
        };
        if self.chat_mode() || index != self.project_sessions.active {
            self.switch_project(index, window, cx);
        }
        let Some(panel) = self
            .project_sessions
            .sessions
            .get(index)
            .map(|session| session.agent_panel.clone())
        else {
            return;
        };
        let thread_index = panel.update(cx, |panel, cx| {
            panel.open_thread_at_turn(
                &hit.thread_id,
                &hit.thread_name,
                hit.color_index.max(0) as usize,
                hit.thread_created_at,
                hit.thread_last_input_at,
                hit.turn_id,
                cx,
            )
        });
        self.reveal_history_thread(panel, thread_index, Some(query), window, cx);
    }

    /// スレッドの持ち主（`threads.project`）に当たる、このウィンドウのプロジェクトの位置。
    fn history_slot_for(&self, project: &str) -> Option<usize> {
        self.project_sessions
            .projects
            .iter()
            .position(|slot| slot.task_space.id.as_str() == project)
            .or_else(|| {
                // 旧版はプロジェクトの表示名で保存していた。
                self.project_sessions
                    .projects
                    .iter()
                    .position(|slot| slot.name.as_ref() == project)
            })
    }

    /// 開いていないプロジェクトの会話: TaskSpace の記録がこのマシン（ローカル）の物なら、そのフォルダを
    /// レールに開いて位置を返す。SSH の物・消えたフォルダは開かない（`None`）。
    fn open_history_project(&mut self, project: &str, cx: &mut Context<Self>) -> Option<usize> {
        let storage = self.persistence.storage.clone()?;
        let record = match storage.load_task_spaces() {
            Ok(records) => records.into_iter().find(|record| record.id == project)?,
            Err(error) => {
                eprintln!("履歴: TaskSpace を読めない: {error:#}");
                return None;
            }
        };
        let local: Arc<dyn Host> = host::LocalHost::shared();
        // stable id は host と root から作る＝ローカルで作り直して一致すれば、このマシンの物。
        if project::stable_worktree_id_on(local.as_ref(), &record.root) != project
            || !record.root.is_dir()
        {
            return None;
        }
        self.open_folder_in_rail(local, record.root.clone(), None, cx);
        self.history_slot_for(project)
    }

    /// 開発用（offscreen 検証）: 履歴ビューの入力に語を入れる。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_history_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if let Some(state) = self.overlays.thread_history.as_ref() {
            state
                .input
                .update(cx, |input, cx| input.set_plain_text(query, cx));
        }
    }

    /// 開発用（offscreen 検証）: 選択を `n` 番目（選べる行の中で）へ動かす。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_history_select(&mut self, target: usize, cx: &mut Context<Self>) {
        if let Some(state) = self.overlays.thread_history.as_mut() {
            let positions = selectable_positions(&state.rows());
            if let Some(position) = positions.get(target) {
                state.selected = target;
                state.scroll.scroll_to_item(*position);
            }
            cx.notify();
        }
    }

    /// 開発用（offscreen 検証）: 選択中の行を開く（⏎ と同じ）。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_history_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_history_selection(window, cx);
    }

    /// スレッド履歴のオーバーレイ（Picker と同じ位置・同じ面）。
    pub(crate) fn render_thread_history(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.thread_history.as_ref()?;
        let theme = self.theme.clone();
        let accent = self
            .active_slot()
            .map(|slot| slot.color)
            .unwrap_or_else(|| project_color(0));
        let rows = state.rows();
        let selected_position = selectable_positions(&rows).get(state.selected).copied();
        let empty = state.query.is_empty();

        let input_row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(12.5))
            .child(div().flex_none().text_color(theme.fg2).child("⌕"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(px(20.))
                    // ↑↓ は入力欄の行移動より先に受けて、リストの選択を動かす（IME 変換中は入力へ流す）。
                    .capture_action(cx.listener(|this, _: &editor_view::MoveUp, _, cx| {
                        this.move_history_selection(-1, cx)
                    }))
                    .capture_action(cx.listener(|this, _: &editor_view::MoveDown, _, cx| {
                        this.move_history_selection(1, cx)
                    }))
                    // Esc = 閉じる（入力欄は複数選択を畳む時以外 `editor::Cancel` を親へ流す）。
                    .on_action(cx.listener(|this, _: &editor_view::Cancel, window, cx| {
                        this.close_thread_history(window, cx)
                    }))
                    .child(state.input.clone())
                    .when(empty, |field| {
                        field.child(
                            div()
                                .absolute()
                                .top(px(2.))
                                .left(px(2.))
                                .text_color(theme.fg2)
                                .child(SharedString::from(i18n::t!("agent.history_placeholder"))),
                        )
                    }),
            );

        let list = div()
            .id("thread-history-list")
            .flex()
            .flex_col()
            .p_1()
            .max_h(px(460.))
            .overflow_y_scroll()
            .track_scroll(&state.scroll)
            .children(rows.iter().enumerate().map(|(position, row)| {
                self.render_history_row(
                    state,
                    row,
                    position,
                    Some(position) == selected_position,
                    accent,
                    cx,
                )
            }));

        let footer = div()
            .px_3()
            .py(px(6.))
            .border_t_1()
            .border_color(theme.border)
            .text_size(px(10.5))
            .text_color(theme.fg2)
            .child(SharedString::from(i18n::t!("agent.history_footer")));

        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(96.))
                // 背景クリックで閉じる（Picker と同じ）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_thread_history(window, cx)),
                )
                .child(
                    div()
                        .w(px(640.))
                        .flex()
                        .flex_col()
                        .bg(theme.bg2)
                        .rounded(px(12.))
                        .border_1()
                        .border_color(theme.border)
                        .overflow_hidden()
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(10.),
                            gpui::hsla(0., 0., 0., 0.45),
                        )
                        .blur_radius(px(28.))])
                        // 箱の中のクリックは背景の「閉じる」に伝播させない。入力欄へフォーカスを戻す。
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                if let Some(state) = this.overlays.thread_history.as_ref() {
                                    let handle = state.input.read(cx).focus_handle(cx);
                                    window.focus(&handle, cx);
                                }
                            }),
                        )
                        .child(input_row)
                        .child(list)
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    fn render_history_row(
        &self,
        state: &ThreadHistoryState,
        row: &HistoryRow,
        position: usize,
        selected: bool,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = &self.theme;
        let base = |id: (&'static str, usize)| {
            let row_theme = theme.clone();
            div()
                .id(id)
                .flex()
                .flex_col()
                .gap(px(2.))
                .px_2()
                .py(px(5.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(move |style| style.bg(row_theme.bg1))
                .when(selected, |element| element.bg(accent.alpha(0.16)))
        };
        let line = |label: SharedString, detail: Option<SharedString>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .text_size(px(12.5))
                .text_color(if selected { theme.fg0 } else { theme.fg1 })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .when_some(detail, |element, detail| {
                    element.child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(detail),
                    )
                })
        };
        let dot = |color: Hsla| div().size(px(7.)).rounded(px(3.5)).flex_none().bg(color);
        let confirm = cx.listener(move |this, _: &MouseDownEvent, window, cx| {
            cx.stop_propagation();
            let row = this
                .overlays
                .thread_history
                .as_ref()
                .and_then(|state| state.rows().get(position).cloned());
            if let Some(row) = row {
                this.confirm_history_row(row, window, cx);
            }
        });
        match row {
            HistoryRow::Header(label) => div()
                .px_2()
                .pt(px(if position == 0 { 4. } else { 10. }))
                .pb(px(3.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.fg2)
                .child(label.clone())
                .into_any_element(),
            HistoryRow::Message(text) => div()
                .px_2()
                .py(px(5.))
                .text_size(px(11.5))
                .text_color(theme.fg2)
                .child(text.clone())
                .into_any_element(),
            HistoryRow::Thread(index) => {
                let Some(thread) = state.threads.get(*index) else {
                    return div().into_any_element();
                };
                let mut detail = Vec::new();
                if let Some(branch) = &thread.branch {
                    detail.push(format!("⎇ {branch}"));
                }
                if thread.tokens_used > 0 {
                    detail.push(format!("Σ {:.1}k", thread.tokens_used as f32 / 1000.0));
                }
                let mut when = i18n::t!(
                    "time.started",
                    "when" => agent_panel::relative_time_label(thread.created_at)
                );
                if let Some(last_input_at) = thread.last_input_at {
                    when.push_str(" · ");
                    when.push_str(&i18n::t!(
                        "time.last_input",
                        "when" => agent_panel::relative_time_label(last_input_at)
                    ));
                }
                detail.push(when);
                if thread.archived {
                    detail.push(i18n::t!("agent.history_archived_mark"));
                }
                base(("history-thread", position))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(dot(theme_core::thread_color(
                                thread.color_index.max(0) as usize
                            )))
                            .child(line(
                                SharedString::from(thread.name.clone()),
                                Some(SharedString::from(detail.join("  "))),
                            )),
                    )
                    .on_mouse_down(MouseButton::Left, confirm)
                    .into_any_element()
            }
            HistoryRow::AgentSession(index) => {
                let AgentSessions::Loaded(sessions) = &state.agent_sessions else {
                    return div().into_any_element();
                };
                let Some(session) = sessions.get(*index) else {
                    return div().into_any_element();
                };
                let mut detail = Vec::new();
                // 同じリポジトリの別の worktree で始めた会話（Claude の一覧は worktree を含む）。
                if session.cwd != state.root {
                    if let Some(name) = session.cwd.file_name() {
                        detail.push(format!("⎇ {}", name.to_string_lossy()));
                    }
                }
                if let Some(updated_at) = session.updated_at_ms {
                    detail.push(agent_panel::relative_time_label(updated_at).to_string());
                }
                base(("history-agent-session", position))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(agent_panel::agent_badge(&state.agent, 14.))
                            .child(line(
                                SharedString::from(agent_session_title(session)),
                                (!detail.is_empty()).then(|| SharedString::from(detail.join("  "))),
                            )),
                    )
                    .on_mouse_down(MouseButton::Left, confirm)
                    .into_any_element()
            }
            HistoryRow::Match(index) => {
                let HistoryMatches::Loaded(matches) = &state.matches else {
                    return div().into_any_element();
                };
                let Some(found) = matches.get(*index) else {
                    return div().into_any_element();
                };
                let detail = format!(
                    "{}  {}",
                    found.place,
                    agent_panel::relative_time_label(found.hit.created_at)
                );
                let speaker = if found.hit.role == "user" {
                    "▸"
                } else {
                    "⏺"
                };
                base(("history-match", position))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(dot(theme_core::thread_color(
                                found.hit.color_index.max(0) as usize
                            )))
                            .child(line(
                                SharedString::from(found.hit.thread_name.clone()),
                                Some(SharedString::from(detail)),
                            )),
                    )
                    .child(
                        div()
                            .pl(px(15.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(format!("{speaker} {}", found.snippet))),
                    )
                    .on_mouse_down(MouseButton::Left, confirm)
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, title: Option<&str>) -> AgentSessionSummary {
        AgentSessionSummary {
            session_id: id.into(),
            cwd: PathBuf::from("/repo"),
            title: title.map(str::to_string),
            updated_at_ms: Some(1),
        }
    }

    /// 行の組み立て: 入力が空ならスレッドとエージェントの会話（読み込み中は一言）。入力があれば名前で
    /// 絞り、本文の一致の区分を足す。当たらない区分は出さない（見出しだけ残さない）。
    #[gpui::test]
    fn rows_follow_the_query_and_the_loading_state(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "necoder_workspace_history_rows_{}_{}",
            std::process::id(),
            agent_panel::now_unix_ms()
        ));
        std::fs::create_dir_all(&root).expect("作業ディレクトリを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let panel = cx.new(|cx| AgentPanel::new(Theme::dark(), cx));
        let input = cx.new(|cx| EditorView::plain(Theme::dark(), project_color(0), true, cx));
        let thread = |id: &str, name: &str| HistoryThread {
            id: id.into(),
            name: name.into(),
            color_index: 0,
            branch: None,
            tokens_used: 0,
            archived: false,
            created_at: 1,
            last_input_at: None,
        };
        let mut state = ThreadHistoryState {
            input,
            query: String::new(),
            previous_focus: None,
            threads: vec![thread("a", "rope の修正"), thread("b", "README")],
            agent: "Claude Code".into(),
            host: Some("dev-box".into()),
            panel,
            root: PathBuf::from("/repo"),
            agent_sessions: AgentSessions::Loading,
            matches: HistoryMatches::Idle,
            search_generation: 0,
            selected: 0,
            scroll: gpui::ScrollHandle::new(),
            _subscriptions: Vec::new(),
        };
        let rows = state.rows();
        assert!(matches!(rows[0], HistoryRow::Header(_)));
        assert_eq!(rows[1], HistoryRow::Thread(0));
        assert_eq!(rows[2], HistoryRow::Thread(1));
        match &rows[3] {
            HistoryRow::Header(label) => assert!(label.contains("dev-box"), "{label}"),
            other => panic!("エージェントの区分の見出し（host 付き）: {other:?}"),
        }
        assert!(matches!(rows[4], HistoryRow::Message(_)), "読み込み中");
        assert_eq!(selectable_positions(&rows), vec![1, 2]);

        state.agent_sessions = AgentSessions::Loaded(vec![
            summary("s1", Some("rope の索引を足す")),
            summary("s2", None),
        ]);
        state.query = "rope".into();
        state.matches = HistoryMatches::Searching;
        let rows = state.rows();
        let kinds: Vec<&HistoryRow> = rows.iter().filter(|row| row.selectable()).collect();
        assert_eq!(
            kinds,
            vec![&HistoryRow::Thread(0), &HistoryRow::AgentSession(0)],
            "名前・題で絞る（README と題の無い会話は外れる）"
        );
        assert!(
            matches!(rows.last(), Some(HistoryRow::Message(_))),
            "本文の一致は検索中の一言"
        );

        state.query = "zzz".into();
        state.matches = HistoryMatches::Loaded(Vec::new());
        let rows = state.rows();
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row, HistoryRow::Header(_)))
                .count(),
            1,
            "当たらない区分は見出しごと出さない（本文の一致の区分だけ・0 件の一言付き）"
        );
        std::fs::remove_dir_all(&root).expect("片付けられる");
    }

    /// 全文検索の一致を開く: 入力が止まってから DB を探し、選ぶとそのプロジェクトのスレッドを開いて
    /// Agent ドックを出す。一覧のエージェントは起動できないコマンドにして、本物を起こさない。
    #[gpui::test]
    fn a_full_text_match_opens_its_thread(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "necoder_workspace_history_match_{}_{}",
            std::process::id(),
            agent_panel::now_unix_ms()
        ));
        let project_dir = root.join("project");
        std::fs::create_dir_all(&project_dir).expect("作業ディレクトリを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false,
                "agent_servers":{"claude":{"type":"custom","command":"/nonexistent/necoder-test-agent"}}}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let storage = storage::Storage::open(&root.join("necoder.db")).expect("DB を開ける");
        let sources = vec![ProjectSource::new(
            host::LocalHost::shared(),
            project_dir.clone(),
        )];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.persistence.storage = Some(storage.clone());
            let (scope, legacy) = workspace
                .active_slot()
                .map(|slot| {
                    (
                        slot.task_space.id.as_str().to_string(),
                        slot.name.to_string(),
                    )
                })
                .expect("プロジェクトがある");
            storage
                .upsert_thread("found", "境界のバグ", 3, &scope, None, None, None, 0, 0)
                .expect("書ける");
            storage
                .insert_turn("found", "user", "境界値で落ちる")
                .expect("書ける");
            storage.archive_thread("found").expect("閉じた印を書ける");
            let panel = workspace.agent_panel.clone();
            panel.update(cx, |panel, cx| {
                panel.set_storage_for_scope(storage.clone(), scope, &legacy, cx)
            });
            workspace.chrome.show_right = false;
            workspace.open_thread_history(&ThreadHistory, window, cx);
            workspace.debug_history_query("境界値", cx);
        });
        cx.executor()
            .advance_clock(HISTORY_SEARCH_DEBOUNCE + Duration::from_millis(50));
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            let state = workspace
                .overlays
                .thread_history
                .as_ref()
                .expect("開いている");
            assert!(
                matches!(state.agent_sessions, AgentSessions::Failed(_)),
                "起こせないエージェントは失敗の一言（本物は起こさない）"
            );
            let rows = state.rows();
            let selectable = selectable_positions(&rows).len();
            let target = selectable_positions(&rows)
                .iter()
                .position(|position| rows[*position] == HistoryRow::Match(0))
                .expect("本文の一致の行がある");
            // ↑↓ は端で折り返す。
            workspace.move_history_selection(-1, cx);
            assert_eq!(
                workspace
                    .overlays
                    .thread_history
                    .as_ref()
                    .map(|state| state.selected),
                Some(selectable - 1)
            );
            workspace.move_history_selection(1, cx);
            assert_eq!(
                workspace
                    .overlays
                    .thread_history
                    .as_ref()
                    .map(|state| state.selected),
                Some(0)
            );
            workspace.debug_history_select(target, cx);
            workspace.confirm_history_selection(window, cx);
            assert!(
                workspace.overlays.thread_history.is_none(),
                "開いたら閉じる"
            );
            assert!(workspace.chrome.show_right, "Agent ドックを出す");
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(
                workspace
                    .agent_panel
                    .read(cx)
                    .active_thread_name()
                    .as_deref(),
                Some("境界のバグ"),
                "一致したスレッドを開いた"
            );
            // もう一度開いて esc（入力欄の `editor::Cancel`）で閉じる。
            workspace.open_thread_history(&ThreadHistory, window, cx);
        });
        cx.run_until_parked();
        cx.dispatch_action(editor_view::Cancel);
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(workspace.overlays.thread_history.is_none(), "esc で閉じる");
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        std::fs::remove_dir_all(&root).expect("片付けられる");
    }
}
