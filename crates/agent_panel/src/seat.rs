//! seat — 決まった道具だけを使う**席**（FLEET-V2 §5.8・Fleet の Captain が座る）。
//!
//! AgentPanel は Fleet を知らない。席は「このスレッドには、この MCP の道具だけを渡し、権限の問いには
//! necoder が決まりで答える」という汎用の仕組みで、中身（どの道具か・断った時の文言）は呼び手が詰める。
//! Chat の裁定（`chat.rs`）と同じ作法で、**人間には何も聞かない**（体感は Bypass と同じ静かさ）。
//!
//! 1. **セッションの作り方** — 権限モードを「聞いてくる」モードに固定し、MCP は席のものだけを渡す
//! 2. **権限** — 読む・探す・考える・fetch と席の道具は許可、編集・削除・移動・shell・他の道具は拒否
//! 3. **見張り** — 編集・削除・移動の道具が**完了した**ら（許可を求めずに実行された＝決まりの漏れ）、
//!    ターンを止めて知らせる。shell は安全な読み取りコマンドを聞かずに通すエージェントがあるので
//!    見張りに入れない（止めすぎる）。shell の危険は権限の問いで断る
//! 4. **付け替え** — 同じ名前の席はパネルに 1 つ。改名したら席を外す
//! 5. **交代** — 会話を捨て、次の送信で新しいセッションを始める（席の持ち主は状態を台帳に置くので失うものが無い）
//!
//! 実験（JOURNAL 2026-09-24）: MCP だけを持たせた Captain は 3 回とも道具だけで采配し、編集も shell も
//! 試さなかった。席の決まりはその振る舞いを**前提にしない**ための関所（長い会話・他のエージェントへの保険）。

use super::*;
use acp_client::ToolCallKind;

/// 席の決まり。
#[derive(Debug, Clone, PartialEq)]
pub struct SeatPolicy {
    /// 席の名前（同じ名前の席はパネルに 1 つ。付け替えると前のスレッドから外れる）。
    pub id: &'static str,
    /// このスレッドのセッションへ渡す MCP サーバ。ユーザー設定の MCP サーバの**代わりに**これだけを渡す。
    pub mcp_servers: Vec<acp_client::mcp::McpServerConfig>,
    /// 許可する道具の名前。種類の分からない呼び出し（MCP の道具）は、タイトルにこの名前を含む時だけ許可する。
    pub allowed_tools: Vec<String>,
    /// 断った時に transcript へ出す 1 行の頭（後ろに道具のタイトルを付ける）。
    pub denied_notice: String,
    /// 決まりの漏れ（許可を求めずに編集された）を見つけた時に transcript へ出す 1 行の頭。
    pub violation_notice: String,
}

/// 席のスレッドで走っている道具 1 本（見張りと、題名の無い許可要求の突き合わせに使う）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SeatToolCall {
    kind: Option<ToolCallKind>,
    title: String,
}

/// 席で使う「聞いてくる」権限モード（エージェントが広告する mode id）。分からないエージェントは `None`
/// ＝エージェントの既定のまま（それでも権限の問いには席の決まりで答え、見張りも効く）。
///
/// Claude Code の `default` は編集・shell・MCP の道具の前に許可を求める。Codex の `read-only` は
/// 書き込みの前に許可を求める（codex-acp の README: `read-only` / `agent` / `agent-full-access`）。
pub fn seat_permission_mode(agent_label: &str) -> Option<&'static str> {
    match acp_client::AgentKind::by_label(agent_label)?.id {
        "claude" => Some("default"),
        "codex" => Some("read-only"),
        _ => None,
    }
}

/// 題名に席の道具の名前を含むか（`mcp__necoder__fleet_list_tasks` / `mcp.necoder.fleet_list_tasks` の両方の綴り）。
fn names_a_seat_tool(seat: &SeatPolicy, title: &str) -> bool {
    seat.allowed_tools.iter().any(|tool| title.contains(tool.as_str()))
}

/// 権限の問いの裁定（純粋関数・テスト用に切り出す）。`true` = 許可。
/// `mcp_tool` = MCP の道具への承認（codex-acp は種別「実行」で送る）。その時は種別でなく題名で決める。
fn seat_allows(seat: &SeatPolicy, kind: Option<ToolCallKind>, title: &str, mcp_tool: bool) -> bool {
    if mcp_tool {
        return names_a_seat_tool(seat, title);
    }
    match kind {
        Some(ToolCallKind::Read | ToolCallKind::Search | ToolCallKind::Think | ToolCallKind::Fetch) => true,
        Some(ToolCallKind::Edit | ToolCallKind::Delete | ToolCallKind::Move | ToolCallKind::Execute) => false,
        Some(ToolCallKind::Other) | None => names_a_seat_tool(seat, title),
    }
}

/// 見張りの対象（完了したら決まりの漏れ）。
fn is_writing_tool(kind: ToolCallKind) -> bool {
    matches!(kind, ToolCallKind::Edit | ToolCallKind::Delete | ToolCallKind::Move)
}

impl AgentPanel {
    /// スレッドを席にする。同じ名前の席を持つ他のスレッドからは外す（前置きも一緒に）。
    /// 権限モードの表示も席のモードへ寄せる（ピルが Bypass のままだと決まりと食い違って見える）。
    pub fn assign_seat(&mut self, index: usize, policy: SeatPolicy, cx: &mut Context<Self>) {
        if self
            .threads
            .get(index)
            .and_then(|thread| thread.seat.as_ref())
            .is_some_and(|seat| *seat == policy)
        {
            return;
        }
        let mut cleared = Vec::new();
        for (other, thread) in self.threads.iter_mut().enumerate() {
            if other != index && thread.seat.as_ref().is_some_and(|seat| seat.id == policy.id) {
                thread.seat = None;
                cleared.push(thread.id.clone());
            }
        }
        for id in cleared {
            self.prompt_context.remove(&id);
        }
        if let Some(thread) = self.threads.get_mut(index) {
            if let Some(mode) = seat_permission_mode(&thread.agent) {
                thread.permission_mode = SharedString::from(mode);
                thread.current_mode_id = SharedString::from(mode);
            }
            thread.seat = Some(policy);
        }
        cx.notify();
    }

    /// 席を外す（普通のスレッドに戻す）。前置きも外す。セッションは次の送信で立て直す。
    pub fn clear_seat(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(thread) = self.threads.get_mut(index) else {
            return;
        };
        if thread.seat.take().is_none() {
            return;
        }
        let id = thread.id.clone();
        self.prompt_context.remove(&id);
        cx.notify();
    }

    /// その名前の席に座っているスレッド。
    pub fn seat_thread(&self, seat_id: &str) -> Option<usize> {
        self.threads
            .iter()
            .position(|thread| thread.seat.as_ref().is_some_and(|seat| seat.id == seat_id))
    }

    /// 表示中のタブを変えずに、名前のスレッドを用意する（Captain の wake・知らせが表示を奪わない）。
    /// 見つからなければ新しく作る。空きタブの使い回しはしない（人間が開いた空のタブを取らない）。
    /// `aliases` は同じスレッドとみなす別名（表示言語を切り替えた後の前の言語の名前）。
    pub fn ensure_named_thread_quietly(
        &mut self,
        name: &str,
        aliases: &[String],
        agent: &str,
        cx: &mut Context<Self>,
    ) -> usize {
        if let Some(index) = self.threads.iter().position(|thread| {
            thread.name.as_ref() == name
                || aliases.iter().any(|alias| alias == thread.name.as_ref())
        }) {
            return index;
        }
        let index = self.threads.len();
        let mut thread = Thread::empty(name.to_string(), index);
        apply_thread_defaults(&mut thread, cx);
        thread.agent = SharedString::from(agent.to_string());
        apply_agent_sticky(&mut thread, cx);
        thread.name_is_custom = true;
        self.threads.push(thread);
        cx.notify();
        index
    }

    /// スレッドが話すエージェントの表示名。
    pub fn thread_agent(&self, index: usize) -> Option<SharedString> {
        self.threads.get(index).map(|thread| thread.agent.clone())
    }

    /// 席のスレッドを普通のスレッドとして退かせる（別の名前を付け、席と前置きを外す）。
    /// Captain のエージェントを替えた時、前の Captain の会話を残したまま新しい席を作るために使う。
    pub fn retire_seat_thread(&mut self, index: usize, name: String, cx: &mut Context<Self>) {
        let Some(thread) = self.threads.get_mut(index) else {
            return;
        };
        thread.name = SharedString::from(name);
        thread.name_is_custom = true;
        let unseated = thread.seat.take().is_some();
        let id = thread.id.clone();
        if unseated {
            self.prompt_context.remove(&id);
        }
        self.persist_thread(index);
        cx.notify();
    }

    /// スレッドの文脈の大きさ `(使っているトークン, 上限)`。会話の交代の判断に使う。
    pub fn thread_tokens(&self, index: usize) -> (u32, u32) {
        self.threads
            .get(index)
            .map(|thread| (thread.tokens_used, thread.tokens_max))
            .unwrap_or((0, 0))
    }

    /// 会話を交代する（FLEET-V2 §5.6）。送信路と会話 id を捨て、次の送信で `session/new` から始める。
    /// transcript には区切りを 1 行残す。実行中・承認待ち・回答待ちなら何もしない（`false`）。
    pub fn rotate_thread_session(
        &mut self,
        index: usize,
        divider: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(thread) = self.threads.get_mut(index) else {
            return false;
        };
        if thread.running
            || thread.pending_permission.is_some()
            || thread.pending_elicitation.is_some()
        {
            return false;
        }
        thread.command_tx = None;
        // 自分で畳んだセッションの終了を「切れました」と報告させない（idle の弁と同じ作法）。
        thread.session_serial = 0;
        thread.acp_session_id = None;
        thread.session_seated = false;
        thread.session_used = false;
        thread.session_note = None;
        thread.seat_tools.clear();
        thread.tokens_used = 0;
        thread.entries.push(Entry::LedgerEvent(SharedString::from(divider)));
        let thread_id = thread.id.clone();
        // DB の会話 id も忘れる（再起動で古い会話を `session/load` しない）。
        if let Some(storage) = self.storage.clone() {
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.clear_thread_session(&thread_id) {
                        eprintln!("会話の交代で古いセッション id を消せない: {error:#}");
                    }
                })
                .detach();
        }
        self.sync_running_registry(cx);
        cx.notify();
        true
    }

    /// セッションの作り方を席の決まりで上書きする（`start_session` から）。
    pub(crate) fn seat_session_preferences(
        &self,
        thread_index: usize,
        mut preferences: acp_client::SessionPreferences,
        cx: &App,
    ) -> acp_client::SessionPreferences {
        let Some(thread) = self.threads.get(thread_index) else {
            return preferences;
        };
        let Some(seat) = &thread.seat else {
            return preferences;
        };
        preferences.mode = seat_permission_mode(&thread.agent).map(str::to_string);
        // 他の MCP を持ち込ませない（Codex は自分の設定の MCP サーバも立ち上げる・Claude は `~/.claude` の
        // 設定と claude.ai のコネクタを持ち込む）。やり方の違いは acp_client に閉じ込めてある。
        let agent_id = acp_client::AgentKind::by_label(&thread.agent)
            .map(|kind| kind.id)
            .unwrap_or_default();
        // Codex のアカウントを設定で切り替えていれば（`agent_servers.codex.env.CODEX_HOME`）、止めるのはその
        // 置き場の設定のサーバ（そのセッションの codex-acp が読むのはそちら。necoder 自身の `~/.codex` ではない）。
        let codex_home = crate::agent_server_override("codex", cx)
            .and_then(|codex| codex.env.get("CODEX_HOME").map(std::path::PathBuf::from));
        acp_client::preset::restrict_mcp_to(
            &mut preferences,
            agent_id,
            seat.mcp_servers.clone(),
            &acp_client::mcp::codex_configured_server_names_for(codex_home.as_deref()),
        );
        preferences
    }

    /// 権限の問いの裁定（席のスレッドでなければ `None`＝従来どおり）。許可は**今回だけ**で答える
    /// （「常に許可」を返すとエージェント側が権限モードを切り替え、以後の問いが来なくなる）。
    ///
    /// 題名の無い要求（codex-acp の MCP の承認は id と印だけ）は、先に届いた同じ id の呼び出しの題名と種別で裁く。
    pub(crate) fn seat_permission(
        thread: &Thread,
        kind: Option<ToolCallKind>,
        title: &str,
        tool_call_id: &str,
        mcp_tool: bool,
    ) -> Option<chat::ChatPermission> {
        let seat = thread.seat.as_ref()?;
        let known = thread.seat_tools.get(tool_call_id);
        let title = match known {
            Some(call) if !call.title.is_empty() => format!("{title} {}", call.title),
            _ => title.to_string(),
        };
        let kind = kind.or_else(|| known.and_then(|call| call.kind));
        Some(if seat_allows(seat, kind, &title, mcp_tool) {
            chat::ChatPermission::Allow
        } else {
            chat::ChatPermission::Deny {
                notice: format!("{} {title}", seat.denied_notice),
            }
        })
    }

    /// 見張り（道具の開始）: 席のスレッドで走り出した道具の種類と題名を控える。
    pub(crate) fn seat_note_tool_started(thread: &mut Thread, info: &acp_client::ToolCallInfo) {
        if thread.seat.is_none() {
            return;
        }
        let call = thread.seat_tools.entry(info.id.clone()).or_default();
        if info.kind.is_some() {
            call.kind = info.kind;
        }
        if let Some(title) = info.title.as_ref().filter(|title| !title.is_empty()) {
            call.title = title.clone();
        }
    }

    /// 見張り（道具の更新）: 編集・削除・移動が**成功して**終わったら決まりの漏れ。transcript に 1 行を積み、
    /// 道具のタイトルを返す（呼び手がターンを止めて workspace へ知らせる）。
    pub(crate) fn seat_check_tool_updated(
        thread: &mut Thread,
        info: &acp_client::ToolCallInfo,
    ) -> Option<String> {
        let seat = thread.seat.as_ref()?;
        let call = thread.seat_tools.entry(info.id.clone()).or_default();
        if info.kind.is_some() {
            call.kind = info.kind;
        }
        if let Some(title) = info.title.as_ref().filter(|title| !title.is_empty()) {
            call.title = title.clone();
        }
        if info.completed.is_none() {
            return None;
        }
        let kind = thread.seat_tools.remove(&info.id)?.kind?;
        if info.completed != Some(true) || !is_writing_tool(kind) {
            return None;
        }
        let title = info.title.clone().unwrap_or_else(|| format!("{kind:?}"));
        let notice = format!("{} {title}", seat.violation_notice);
        thread.entries.push(Entry::Agent(SharedString::from(notice)));
        Some(title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captain_seat() -> SeatPolicy {
        SeatPolicy {
            id: "captain",
            mcp_servers: Vec::new(),
            allowed_tools: vec!["fleet_propose_tasks".into(), "fleet_list_tasks".into()],
            denied_notice: "断った:".into(),
            violation_notice: "漏れた:".into(),
        }
    }

    #[test]
    fn a_seat_allows_reading_and_its_own_tools_and_refuses_writing_and_the_shell() {
        let seat = captain_seat();
        assert!(seat_allows(&seat, Some(ToolCallKind::Read), "Read src/main.rs", false));
        assert!(seat_allows(&seat, Some(ToolCallKind::Search), "Grep TODO", false));
        assert!(seat_allows(&seat, Some(ToolCallKind::Fetch), "WebFetch", false));
        assert!(seat_allows(&seat, Some(ToolCallKind::Other), "mcp__necoder__fleet_propose_tasks", false));
        assert!(seat_allows(&seat, None, "necoder.fleet_list_tasks", false), "エージェントごとの綴り違いも名前で拾う");
        assert!(!seat_allows(&seat, Some(ToolCallKind::Edit), "Edit src/main.rs", false));
        assert!(!seat_allows(&seat, Some(ToolCallKind::Delete), "rm", false));
        assert!(!seat_allows(&seat, Some(ToolCallKind::Move), "mv", false));
        assert!(
            !seat_allows(&seat, Some(ToolCallKind::Execute), "necoder fleet create . x", false),
            "shell は道具の名前を含んでいても断る（CLI の抜け道）"
        );
        assert!(!seat_allows(&seat, Some(ToolCallKind::Other), "mcp__higgsfield__generate_image", false));
        assert!(!seat_allows(&seat, None, "write_file", false));
        // codex-acp の MCP の承認は種別「実行」で来る。印があれば題名で決める（shell とは分ける）。
        assert!(seat_allows(&seat, Some(ToolCallKind::Execute), "mcp.necoder.fleet_propose_tasks", true));
        assert!(!seat_allows(&seat, Some(ToolCallKind::Execute), "mcp.computer-use.click", true));
        assert!(
            !seat_allows(&seat, Some(ToolCallKind::Execute), "fleet_list_tasks", false),
            "印の無い実行は shell として断る"
        );
    }

    #[test]
    fn seat_modes_are_the_asking_modes_of_each_agent() {
        assert_eq!(seat_permission_mode("Claude Code"), Some("default"));
        assert_eq!(seat_permission_mode("Codex"), Some("read-only"));
        assert_eq!(seat_permission_mode("Qwen Code"), None);
        assert_eq!(seat_permission_mode("unknown"), None);
    }
}
