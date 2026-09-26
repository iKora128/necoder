//! history — エージェント側の会話の履歴（O15）。`session/list` で過去の会話を一覧し、`session/load` が
//! 再生する履歴を transcript の項目へ畳む。
//!
//! - **一覧**（[`list_sessions_on`]）: 履歴ビューを開いた時だけ、**一覧のためにエージェントを 1 本**起こし、
//!   `initialize` → `session/list` だけ呼んで畳む。`session/new` はしない（会話を作らない）し、prompt も
//!   送らない（課金しない）。エージェントが `sessionCapabilities.list` を広告していなければ呼ばない。
//!   - claude-agent-acp 0.81.1: SDK の `listSessions({dir})` の結果を一度に返す（nextCursor なし）。
//!     **CLI（`claude`）で作った会話も出る**。同じリポジトリの別 worktree の会話も含む（SDK の既定）。
//!     cwd の無い会話は adapter が落とす（`dist/acp-agent.js` 1201-1217）。
//!   - codex-acp 1.13.1: `codex app-server` の thread 一覧（sourceKinds に `cli` を含む）を 1 ページずつ返す。
//!     cwd の絞り込みは**ページの中だけ**なので、空のページでも nextCursor があれば次を読む
//!     （`dist/index.js` 34207-34250）。
//! - **再生**（[`ReplayLog`]）: `session/load` の間に届く `session/update` を「人の発話・エージェントの本文・
//!   ツールの呼び出し」へ畳む。思考・プラン・使用量は捨てる。サブエージェントの**発話**（claude の
//!   `_meta.claudeCode.parentToolUseId`・codex の `_meta.codex.subagent`）も捨てる（主の会話だけを読む）。
//!   サブエージェントの**ツール**は live と同じく親の相関 id を持たせて残す（UI が親の下に畳む・O17）。
//!   再生は**本文をブロック単位で**送ってくる（claude はメッセージの content ブロックごと・codex は
//!   thread の item ごと）ので、同じ発話の続きのブロックは段落として繋ぐ。

use crate::{
    initialize_request, map_tool_kind, subagent_parent, tool_completed, tool_diffs, tool_locations,
    tool_output, with_handshake_timeout, with_timeout, AgentCommand, ToolCallInfo,
};
use acp::schema::v1;
use agent_client_protocol as acp;
use anyhow::{Context as _, Result};
use host::{CommandSpec, Host};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 再生から残す項目の数（transcript に積む上限）。これより前は捨てて数だけ数える
/// （DB から復元するスレッドの直近 200 turn と揃える）。
pub const REPLAY_KEEP: usize = 200;

/// 1 回の一覧で読むページの上限。codex-acp は cwd の絞り込みがページの中だけなので、空のページが
/// 続いても打ち切れるようにする。
const MAX_LIST_PAGES: usize = 20;

/// `session/list` 1 回を待つ上限。
const LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// エージェント側の過去の会話 1 件（`session/list` の `SessionInfo` を UI 非依存に写した物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionSummary {
    /// エージェント側の会話の鍵（`session/load` に渡す。Claude Code は会話の UUID）。
    pub session_id: String,
    /// 会話を始めた場所（絶対パス）。同じリポジトリの別 worktree のことがある。
    pub cwd: PathBuf,
    /// 会話の題（エージェントが付けた物。無いこともある）。
    pub title: Option<String>,
    /// 最後に動いた時刻（unix ms）。エージェントが言わない・読めない時は `None`。
    pub updated_at_ms: Option<i64>,
}

/// [`list_sessions_on`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionListing {
    /// 一覧（エージェントが返した順）。
    Listed(Vec<AgentSessionSummary>),
    /// エージェントが `session/list` を広告していない（一覧を出せない）。
    Unsupported,
}

/// 履歴ビュー用に、`host` 上でエージェントを起こして `cwd` の過去の会話を一覧する（最大 `limit` 件
/// 読んだら止める）。**prompt は送らない・`session/new` もしない**。終わったらプロセスを畳む。
/// リモート（SSH）のプロジェクトではエージェントがリモートで動くので、リモートの会話が出る。
pub async fn list_sessions_on(
    host: Arc<dyn Host>,
    command: AgentCommand,
    cwd: PathBuf,
    limit: usize,
) -> Result<SessionListing> {
    let spec = CommandSpec::new(command.path.to_string_lossy(), &command.cwd)
        .args(command.args.clone())
        .envs(host::task_environment(host.as_ref(), &command.cwd)?)
        .envs(command.env.clone());
    let mut process = host
        .spawn_process(&spec)
        .with_context(|| format!("ACP agent を起動できない: {}", command.path.display()))?;
    let stdin = process.take_stdin()?;
    let stdout = process.take_stdout()?;
    let transport = acp::ByteStreams::new(
        blocking::Unblock::new(stdin),
        blocking::Unblock::new(stdout),
    );
    let outcome = acp::Client
        .builder()
        .connect_with(transport, async move |connection| {
            let initialized = with_handshake_timeout(
                "ACP initialize",
                connection.send_request(initialize_request()).block_task(),
            )
            .await?;
            if initialized
                .agent_capabilities
                .session_capabilities
                .list
                .is_none()
            {
                return Ok(SessionListing::Unsupported);
            }
            let mut sessions = Vec::new();
            let mut cursor: Option<String> = None;
            for _ in 0..MAX_LIST_PAGES {
                let request = v1::ListSessionsRequest::new()
                    .cwd(cwd.clone())
                    .cursor(cursor.clone());
                let response = with_timeout(
                    LIST_TIMEOUT,
                    "ACP session/list",
                    connection.send_request(request).block_task(),
                )
                .await?;
                sessions.extend(summaries_from(response.sessions));
                cursor = response.next_cursor.filter(|next| !next.is_empty());
                if cursor.is_none() || sessions.len() >= limit {
                    break;
                }
            }
            Ok::<_, acp::Error>(SessionListing::Listed(sessions))
        })
        .await
        .context("ACP session/list に失敗");
    drop(process);
    outcome
}

/// `session/list` の応答を [`AgentSessionSummary`] へ。題は前後の空白を落とし、空なら無しにする。
pub fn summaries_from(sessions: Vec<v1::SessionInfo>) -> Vec<AgentSessionSummary> {
    sessions
        .into_iter()
        .map(|info| AgentSessionSummary {
            session_id: info.session_id.to_string(),
            cwd: info.cwd,
            title: info
                .title
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty()),
            updated_at_ms: info.updated_at.as_deref().and_then(parse_iso8601_ms),
        })
        .collect()
}

/// 履歴ビューに出す「エージェントの過去の会話」: necoder が既に持っている会話（`known` の id）・
/// 題が `hidden_title_prefixes` のどれかで始まる会話（necoder が自分の用事で CLI に頼んだ要約や命名）・
/// 重複を除き、新しい順（時刻の分からない物は後ろ）に `limit` 件まで。
pub fn fresh_sessions(
    listed: Vec<AgentSessionSummary>,
    known: &HashSet<String>,
    hidden_title_prefixes: &[String],
    limit: usize,
) -> Vec<AgentSessionSummary> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut sessions: Vec<AgentSessionSummary> = listed
        .into_iter()
        .filter(|session| !known.contains(&session.session_id))
        .filter(|session| {
            !session.title.as_deref().is_some_and(|title| {
                let title = title.trim_start();
                hidden_title_prefixes
                    .iter()
                    .any(|prefix| !prefix.is_empty() && title.starts_with(prefix.as_str()))
            })
        })
        .filter(|session| seen.insert(session.session_id.clone()))
        .collect();
    // `Option` は None < Some なので、降順にすると時刻の無い物が末尾へ回る。
    sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    sessions.truncate(limit);
    sessions
}

/// ISO 8601 の日時（`2026-09-26T01:02:03.456Z` / `+09:00` 付き / 時差なし＝UTC）を unix ms へ。
/// 読めなければ `None`。ACP の `updatedAt` は JavaScript の `toISOString()` の形が普通。
pub fn parse_iso8601_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let (time, offset_seconds) = match rest.strip_suffix(['Z', 'z']) {
        Some(time) => (time, 0),
        None => match rest.rfind(['+', '-']) {
            Some(index) => {
                let (time, zone) = rest.split_at(index);
                let sign = if zone.starts_with('-') { -1 } else { 1 };
                let zone = &zone[1..];
                if !zone.is_ascii() {
                    return None;
                }
                let (hours, minutes) = zone
                    .split_once(':')
                    .unwrap_or_else(|| zone.split_at(zone.len().min(2)));
                let hours: i64 = hours.parse().ok()?;
                let minutes: i64 = if minutes.is_empty() {
                    0
                } else {
                    minutes.parse().ok()?
                };
                (time, sign * (hours * 3600 + minutes * 60))
            }
            None => (rest, 0),
        },
    };
    let mut time_parts = time.splitn(3, ':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let (second, fraction) = match time_parts.next() {
        Some(seconds) => match seconds.split_once(['.', ',']) {
            Some((whole, fraction)) => (whole.parse::<i64>().ok()?, fraction),
            None => (seconds.parse::<i64>().ok()?, ""),
        },
        None => (0, ""),
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
        || !fraction.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }
    // 小数部はミリ秒の桁まで（足りなければ 0 で埋める）。
    let millis = fraction
        .chars()
        .chain(std::iter::repeat('0'))
        .take(3)
        .collect::<String>()
        .parse::<i64>()
        .ok()?;
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    Some(seconds * 1_000 + millis)
}

/// 暦日 → 1970-01-01 からの日数（先発グレゴリオ暦。Howard Hinnant の days_from_civil の算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// 再生された履歴の 1 項目（UI 非依存）。
#[derive(Debug, Clone)]
pub enum ReplayItem {
    /// 人の発話。
    User(String),
    /// エージェントの本文。
    Agent(String),
    /// ツールの呼び出し（開始と後追いの更新を畳んだ物）。
    Tool(ToolCallInfo),
}

/// 伸ばしている発話の役割。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speaker {
    User,
    Agent,
}

/// `session/load` の再生を [`ReplayItem`] の列へ畳む。上限（`keep`）を超えたら古い方から捨て、
/// 捨てた数を数える（transcript には「以前の N 件は省略」とだけ出す）。
#[derive(Debug)]
pub struct ReplayLog {
    items: VecDeque<ReplayItem>,
    omitted: usize,
    keep: usize,
    /// いま伸ばしている発話（役割とメッセージ id）。役割か id が変わったら新しい項目にする。
    open_text: Option<(Speaker, Option<String>)>,
}

impl ReplayLog {
    pub fn new(keep: usize) -> ReplayLog {
        ReplayLog {
            items: VecDeque::new(),
            omitted: 0,
            keep: keep.max(1),
            open_text: None,
        }
    }

    /// 再生された `session/update` を 1 件取り込む。
    pub fn push(&mut self, update: v1::SessionUpdate) {
        match update {
            v1::SessionUpdate::UserMessageChunk(chunk) => self.push_text(Speaker::User, chunk),
            v1::SessionUpdate::AgentMessageChunk(chunk) => self.push_text(Speaker::Agent, chunk),
            v1::SessionUpdate::ToolCall(tool_call) => {
                // サブエージェントのツールも残す（親の相関 id 付き＝UI が親の下に畳む・live と同じ）。
                self.open_text = None;
                self.push_item(ReplayItem::Tool(ToolCallInfo {
                    id: tool_call.tool_call_id.0.to_string(),
                    title: Some(tool_call.title),
                    kind: Some(map_tool_kind(tool_call.kind)),
                    locations: tool_locations(&tool_call.locations),
                    diffs: tool_diffs(&tool_call.content),
                    output: tool_output(&tool_call.content),
                    completed: tool_completed(tool_call.status),
                    parent: subagent_parent(tool_call.meta.as_ref()),
                }));
            }
            v1::SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                let Some(info) = self.items.iter_mut().rev().find_map(|item| match item {
                    ReplayItem::Tool(info) if info.id == id => Some(info),
                    _ => None,
                }) else {
                    return; // 開始が再生に無い（上限で捨てた・サブエージェントの物）
                };
                if info.parent.is_none() {
                    info.parent = subagent_parent(update.meta.as_ref());
                }
                let fields = update.fields;
                if let Some(title) = fields.title.filter(|title| !title.is_empty()) {
                    info.title = Some(title);
                }
                if let Some(kind) = fields.kind {
                    info.kind = Some(map_tool_kind(kind));
                }
                if let Some(locations) = fields.locations.as_deref() {
                    let locations = tool_locations(locations);
                    if !locations.is_empty() {
                        info.locations = locations;
                    }
                }
                if let Some(content) = fields.content.as_deref() {
                    let diffs = tool_diffs(content);
                    if !diffs.is_empty() {
                        info.diffs = diffs;
                    }
                    if let Some(output) = tool_output(content) {
                        info.output = Some(output);
                    }
                }
                if let Some(completed) = fields.status.and_then(tool_completed) {
                    info.completed = Some(completed);
                }
            }
            // 思考・プラン・使用量・状態は transcript の本文ではない（状態は別の道で流れる）。
            _ => {}
        }
    }

    /// 畳んだ項目（古い順）と、上限で捨てた項目の数。
    pub fn finish(self) -> (Vec<ReplayItem>, usize) {
        (self.items.into_iter().collect(), self.omitted)
    }

    fn push_text(&mut self, speaker: Speaker, chunk: v1::ContentChunk) {
        if is_subagent(chunk.meta.as_ref()) {
            return;
        }
        // 画像・リソースは読み物に向かないので落とす（本文のテキストだけ）。
        let v1::ContentBlock::Text(text) = chunk.content else {
            return;
        };
        let message_id = chunk.message_id.map(|id| id.to_string());
        let continues = match &self.open_text {
            Some((open_speaker, open_id)) => {
                *open_speaker == speaker
                    && (open_id.is_none() || message_id.is_none() || *open_id == message_id)
            }
            None => false,
        };
        if continues {
            if let Some(ReplayItem::User(existing) | ReplayItem::Agent(existing)) =
                self.items.back_mut()
            {
                // 同じ発話の次のブロック＝段落として繋ぐ（ブロック単位で届くため）。
                if !existing.is_empty() && !existing.ends_with('\n') && !text.text.starts_with('\n')
                {
                    existing.push_str("\n\n");
                }
                existing.push_str(&text.text);
                if message_id.is_some() {
                    self.open_text = Some((speaker, message_id));
                }
                return;
            }
        }
        if text.text.trim().is_empty() {
            return;
        }
        self.open_text = Some((speaker, message_id));
        self.push_item(match speaker {
            Speaker::User => ReplayItem::User(text.text),
            Speaker::Agent => ReplayItem::Agent(text.text),
        });
    }

    fn push_item(&mut self, item: ReplayItem) {
        self.items.push_back(item);
        while self.items.len() > self.keep {
            self.items.pop_front();
            self.omitted += 1;
        }
    }
}

/// サブエージェントの発話か（主の会話の transcript には載せない）。
fn is_subagent(meta: Option<&v1::Meta>) -> bool {
    let Some(meta) = meta else {
        return false;
    };
    let claude = meta
        .get("claudeCode")
        .and_then(|claude| claude.get("parentToolUseId"))
        .is_some_and(serde_json::Value::is_string);
    let codex = meta
        .get("codex")
        .and_then(|codex| codex.get("subagent"))
        .is_some_and(|subagent| !subagent.is_null());
    claude || codex
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(value: serde_json::Value) -> v1::SessionUpdate {
        serde_json::from_value(value).expect("SessionUpdate として読める")
    }

    fn text_chunk(kind: &str, text: &str, message_id: Option<&str>) -> v1::SessionUpdate {
        let mut value = json!({
            "sessionUpdate": kind,
            "content": {"type": "text", "text": text},
        });
        if let Some(message_id) = message_id {
            value["messageId"] = json!(message_id);
        }
        update(value)
    }

    fn describe(items: &[ReplayItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                ReplayItem::User(text) => format!("user:{text}"),
                ReplayItem::Agent(text) => format!("agent:{text}"),
                ReplayItem::Tool(info) => format!(
                    "tool:{}:{}:{}:{:?}:{}",
                    info.title.as_deref().unwrap_or(""),
                    info.locations.join(","),
                    info.output.as_deref().unwrap_or(""),
                    info.completed,
                    info.parent.as_deref().unwrap_or("-")
                ),
            })
            .collect()
    }

    /// 再生された update → transcript の項目: 発話は役割とメッセージ id で区切り、同じ発話のブロックは
    /// 段落で繋ぐ。ツールは開始に後追いの更新を畳む。思考・サブエージェントの内側は捨てる。
    #[test]
    fn replayed_updates_fold_into_turns_and_tool_calls() {
        let mut log = ReplayLog::new(REPLAY_KEEP);
        log.push(text_chunk(
            "user_message_chunk",
            "rope のバグを直して",
            Some("u1"),
        ));
        log.push(text_chunk("agent_thought_chunk", "考え中", Some("a1")));
        log.push(text_chunk(
            "agent_message_chunk",
            "見てみます。",
            Some("a1"),
        ));
        log.push(text_chunk(
            "agent_message_chunk",
            "原因は境界です。",
            Some("a1"),
        ));
        log.push(update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "title": "Read rope.rs",
            "kind": "read",
            "status": "pending",
            "locations": [{"path": "/repo/rope.rs"}],
        })));
        // サブエージェントの発話（claude）: 主の transcript には出さない。ツールは親付きで残す。
        log.push(update(json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "サブエージェントへの指示"},
            "_meta": {"claudeCode": {"parentToolUseId": "t1"}},
        })));
        log.push(update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t2",
            "title": "Grep split",
            "kind": "search",
            "status": "completed",
            "_meta": {"claudeCode": {"parentToolUseId": "t1"}},
        })));
        log.push(update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "t1",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "fn split()"}}],
        })));
        log.push(text_chunk(
            "agent_message_chunk",
            "直しました。",
            Some("a2"),
        ));
        // id の無いエージェントの続き（同じ役割）は同じ発話として繋ぐ。
        log.push(text_chunk(
            "agent_message_chunk",
            "テストも通ります。",
            None,
        ));
        log.push(text_chunk("user_message_chunk", "ありがとう", Some("u2")));
        let (items, omitted) = log.finish();
        assert_eq!(omitted, 0);
        assert_eq!(
            describe(&items),
            vec![
                "user:rope のバグを直して".to_string(),
                "agent:見てみます。\n\n原因は境界です。".to_string(),
                "tool:Read rope.rs:/repo/rope.rs:fn split():Some(true):-".to_string(),
                "tool:Grep split:::Some(true):t1".to_string(),
                "agent:直しました。\n\nテストも通ります。".to_string(),
                "user:ありがとう".to_string(),
            ]
        );
    }

    /// 同じ役割でもメッセージ id が変われば別の発話（ユーザーが続けて 2 回送った・中断した後の再送）。
    #[test]
    fn a_new_message_id_starts_a_new_turn() {
        let mut log = ReplayLog::new(REPLAY_KEEP);
        log.push(text_chunk("user_message_chunk", "1 回目", Some("u1")));
        log.push(text_chunk("user_message_chunk", "2 回目", Some("u2")));
        log.push(update(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "codex の子"},
            "_meta": {"codex": {"subagent": {"threadId": "child"}}},
        })));
        let (items, _) = log.finish();
        assert_eq!(
            describe(&items),
            vec!["user:1 回目".to_string(), "user:2 回目".to_string()]
        );
    }

    /// 上限を超えた分は古い方から捨てて数える。捨てたツールへの更新は黙って捨てる。
    #[test]
    fn replay_keeps_the_latest_items_and_counts_the_rest() {
        let mut log = ReplayLog::new(2);
        log.push(update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "old",
            "title": "Read a.rs",
            "kind": "read",
            "status": "pending",
        })));
        log.push(text_chunk("user_message_chunk", "次", Some("u1")));
        log.push(text_chunk("agent_message_chunk", "はい", Some("a1")));
        log.push(update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "old",
            "status": "completed",
        })));
        let (items, omitted) = log.finish();
        assert_eq!(omitted, 1);
        assert_eq!(
            describe(&items),
            vec!["user:次".to_string(), "agent:はい".to_string()]
        );
    }

    /// `session/list` の応答 → 履歴の行: necoder が既に持っている会話・necoder が自分の用事で CLI に
    /// 頼んだ会話（題の頭で見分ける）・重複を除き、新しい順に上限まで。
    #[test]
    fn listed_sessions_drop_known_ones_and_sort_newest_first() {
        let response: v1::ListSessionsResponse = serde_json::from_value(json!({
            "sessions": [
                {"sessionId": "old", "cwd": "/repo", "title": "古い会話",
                 "updatedAt": "2026-09-01T00:00:00.000Z"},
                {"sessionId": "known", "cwd": "/repo", "title": "necoder のスレッド",
                 "updatedAt": "2026-09-25T00:00:00.000Z"},
                {"sessionId": "new", "cwd": "/repo-wt", "title": "  CLI の会話  ",
                 "updatedAt": "2026-09-20T12:00:00Z"},
                {"sessionId": "untimed", "cwd": "/repo", "title": "   "},
                {"sessionId": "new", "cwd": "/repo", "title": "重複"},
                {"sessionId": "helper", "cwd": "/repo",
                 "title": "入力はエージェントターンの指示と結果です。何をして…",
                 "updatedAt": "2026-09-26T00:00:00.000Z"},
            ],
        }))
        .expect("応答として読める");
        let listed = summaries_from(response.sessions);
        let known: HashSet<String> = ["known".to_string()].into_iter().collect();
        let hidden = vec!["入力はエージェントターンの".to_string(), String::new()];
        let rows = fresh_sessions(listed.clone(), &known, &hidden, 10);
        let ids: Vec<&str> = rows.iter().map(|row| row.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "old", "untimed"]);
        assert_eq!(rows[0].title.as_deref(), Some("CLI の会話"));
        assert_eq!(rows[0].cwd, PathBuf::from("/repo-wt"));
        assert_eq!(rows[2].title, None, "空白だけの題は無し");
        assert_eq!(rows[2].updated_at_ms, None);
        assert_eq!(
            fresh_sessions(listed, &known, &hidden, 1).len(),
            1,
            "上限が効く"
        );
    }

    #[test]
    fn iso_timestamps_become_unix_milliseconds() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso8601_ms("2026-09-26T01:02:03.456Z"),
            Some(1_790_384_523_456)
        );
        // 時差付き: +09:00 の 10:02:03 は UTC の 01:02:03。
        assert_eq!(
            parse_iso8601_ms("2026-09-26T10:02:03+09:00"),
            Some(1_790_384_523_000)
        );
        assert_eq!(
            parse_iso8601_ms("2026-09-25T20:02:03.5-05:00"),
            Some(1_790_384_523_500)
        );
        // 閏年の 2 月末。
        assert_eq!(
            parse_iso8601_ms("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
        assert_eq!(parse_iso8601_ms("yesterday"), None);
        assert_eq!(parse_iso8601_ms("2026-13-01T00:00:00Z"), None);
    }
}
