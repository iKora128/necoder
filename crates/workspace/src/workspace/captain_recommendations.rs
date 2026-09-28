//! Captain の推薦（FLEET-V2 §5.5 の末尾）— 承認待ちの要求 1 件に、Captain が 1 行の見立てを添える。
//!
//! Captain は `fleet_recommend`（Captain 版の MCP にだけある道具・§5.8）で「許可してよい — 理由」を返す。
//! necoder はそれを**どの Task の、どの承認要求への推薦か**（要求の id = `PermissionCard::id`）と一緒に持ち、
//! 要対応の承認待ちのカードに ✳ 付きで添える。**応答はしない**（許可・拒否のボタンの働きは変えない）。
//!
//! - 表示は今の要求の id で引くので、古い要求への推薦が新しい要求に付くことはない。受ける時も、今の要求と
//!   id が違えば断る（Captain が読んだ後に要求が替わった）。
//! - 要求が解決・取り消し・別の要求に替わったら消す（パネルの出来事と、カードのボタンの後に掃除する）。
//! - 画面の上にだけ置く（承認待ち自体が再起動を越えないので、推薦も越えない）。台帳には `captain` の采配として
//!   Task の id で残し、ニュースに丸チップの行を積む（監査・§5.6）。

use crate::workspace::*;

/// 推薦の理由の長さの上限（文字数）。カードの 1 行に収まる見立てに留める（全文はツールチップ）。
pub const MAX_RECOMMENDATION_REASON_CHARS: usize = 200;

/// Captain へ渡す知らせに載せる要求の文の長さの上限（文字数）。
const WAKE_REQUEST_CHARS: usize = 160;

/// Captain の見立て。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendationVerdict {
    /// 許可してよい。
    Allow,
    /// 拒否を勧める。
    Deny,
    /// 人間が中身を見て決めるべき（取り消せない・Task の範囲の外 等）。
    AskHuman,
}

impl RecommendationVerdict {
    pub const ALL: [Self; 3] = [Self::Allow, Self::Deny, Self::AskHuman];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::AskHuman => "ask_human",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|verdict| verdict.as_str() == value)
    }

    fn label(self) -> String {
        match self {
            Self::Allow => i18n::t!("captain.recommend_allow"),
            Self::Deny => i18n::t!("captain.recommend_deny"),
            Self::AskHuman => i18n::t!("captain.recommend_ask_human"),
        }
    }
}

/// 確かめた後の `fleet_recommend` の引数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendationRequest {
    pub task_id: String,
    pub permission_id: String,
    pub verdict: RecommendationVerdict,
    pub reason: String,
}

/// `fleet_recommend` の引数を確かめる（MCP の側で GUI に渡す前と、GUI の IPC の入口の 2 か所）。
/// エラー文はそのまま Captain へ返る（何を直せばよいかが分かる文にする）。
pub fn validate_recommendation(
    arguments: &serde_json::Value,
) -> Result<RecommendationRequest, String> {
    let required = |key: &str| {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| i18n::t!("captain.recommend_err_field", "field" => key))
    };
    let task_id = required("task_id")?;
    let permission_id = required("permission_id")?;
    let verdict_text = required("verdict")?;
    let verdict = RecommendationVerdict::parse(&verdict_text).ok_or_else(|| {
        i18n::t!(
            "captain.recommend_err_verdict",
            "verdict" => &verdict_text,
            "known" => RecommendationVerdict::ALL
                .map(RecommendationVerdict::as_str)
                .join(" / "),
        )
    })?;
    let reason = required("reason")?;
    if reason.contains(['\n', '\r']) {
        return Err(i18n::t!("captain.recommend_err_line"));
    }
    if reason.chars().count() > MAX_RECOMMENDATION_REASON_CHARS {
        return Err(i18n::t!(
            "captain.recommend_err_long",
            "max" => MAX_RECOMMENDATION_REASON_CHARS
        ));
    }
    Ok(RecommendationRequest {
        task_id,
        permission_id,
        verdict,
        reason,
    })
}

/// 承認要求の文を知らせの 1 行に畳む（1 行目だけ・長ければ切る）。
fn request_line(title: &str) -> String {
    let trimmed = title.trim();
    let first_line = trimmed.lines().next().unwrap_or_default().trim_end();
    let mut line: String = first_line.chars().take(WAKE_REQUEST_CHARS).collect();
    if line.len() < trimmed.len() {
        line.push_str(" …");
    }
    line
}

/// 画面に出している推薦 1 件。
pub(crate) struct CaptainRecommendation {
    /// どの Task の。
    pub(crate) task: SpaceId,
    /// どの承認要求への（`PermissionCard::id`）。
    pub(crate) permission_id: SharedString,
    pub(crate) verdict: RecommendationVerdict,
    pub(crate) reason: SharedString,
}

impl CaptainRecommendation {
    /// `Captain: 許可してよい`（カードで太さを立てる部分・ツールチップの頭）。
    fn headline(&self) -> String {
        i18n::t!("captain.recommend_card", "verdict" => self.verdict.label())
    }
}

impl Workspace {
    /// IPC `recommend`（Captain 版の MCP の `fleet_recommend`）。今の承認待ちの要求にだけ付け、応答はしない。
    /// エラー文はそのまま Captain へ返る。
    pub(crate) fn accept_captain_recommendation(
        &mut self,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let request = validate_recommendation(params)?;
        if settings::get(cx).captain_agent.is_none() {
            return Err(i18n::t!("captain.recommend_err_no_captain"));
        }
        let Some(index) = self
            .project_sessions
            .projects
            .iter()
            .position(|slot| slot.task_space.id.as_str() == request.task_id)
        else {
            return Err(i18n::t!("captain.recommend_err_task", "id" => &request.task_id));
        };
        if self.project_sessions.projects[index]
            .task_space
            .is_integration()
        {
            return Err(i18n::t!("captain.recommend_err_integration"));
        }
        let pending = self.pending_permissions(index, cx);
        let Some((thread, card)) = pending
            .iter()
            .find(|(_, card)| card.id.as_ref() == request.permission_id)
        else {
            // Captain が読んだ後に要求が解決・取り消し・別の要求に替わった。古い要求への推薦は付けない。
            let current = match pending.first() {
                Some((_, card)) => i18n::t!(
                    "captain.recommend_err_stale_current",
                    "permission" => card.id.as_ref(),
                    "request" => request_line(&card.title),
                ),
                None => i18n::t!("captain.recommend_err_stale_none"),
            };
            return Err(i18n::t!(
                "captain.recommend_err_stale",
                "permission" => &request.permission_id,
                "current" => current,
            ));
        };
        let slot = &self.project_sessions.projects[index];
        let task = slot.task_space.id.clone();
        let news = i18n::t!(
            "captain.recommend_news",
            "task" => slot.task_space.title.as_ref(),
            "verdict" => request.verdict.label(),
            "reason" => &request.reason,
        );
        if let Some(storage) = self.persistence.storage.clone() {
            // 采配の監査（§5.6）: 既存の Captain の行と同じ kind `captain` で、どの Task の要求かが分かるよう
            // Task の id に積む（Captain へ渡す知らせからは kind で外れる）。
            let payload = serde_json::json!({
                "text": news,
                "recommendation": {
                    "verdict": request.verdict.as_str(),
                    "reason": request.reason,
                    "permission_id": request.permission_id,
                    "permission": card.title.as_ref(),
                    "thread": thread.as_ref(),
                },
            })
            .to_string();
            let task_id = request.task_id.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.append_task_event(&task_id, "captain", &payload) {
                        eprintln!("Captain の推薦を記録できない: {error:#}");
                    }
                })
                .detach();
        }
        self.remember_captain_recommendation(
            CaptainRecommendation {
                task,
                permission_id: card.id.clone(),
                verdict: request.verdict,
                reason: SharedString::from(request.reason.clone()),
            },
            news,
            cx,
        );
        Ok(serde_json::json!({
            "task_id": request.task_id,
            "permission_id": request.permission_id,
            "verdict": request.verdict.as_str(),
            "status": "shown",
            "message": i18n::t!("captain.recommend_reply"),
        }))
    }

    /// 推薦を画面に置き（同じ要求への前の推薦は差し替える）、ニュースに Captain の行を積む。
    /// ニュースの行はその Task へ飛ぶ（承認待ちがある場所）。
    fn remember_captain_recommendation(
        &mut self,
        recommendation: CaptainRecommendation,
        news: String,
        cx: &mut Context<Self>,
    ) {
        let color = self.captain_color_for(&recommendation.task, cx);
        self.push_news(
            NewsKind::Captain,
            color,
            SharedString::from(i18n::t!("captain.title")),
            SharedString::from(news),
            Some(recommendation.task.clone()),
        );
        self.prune_captain_recommendations(cx);
        self.chrome.captain_recommendations.retain(|existing| {
            existing.task != recommendation.task
                || existing.permission_id != recommendation.permission_id
        });
        self.chrome.captain_recommendations.push(recommendation);
        cx.notify();
    }

    /// ニュースの Captain 行の色 = その Task のリポジトリの Captain スレッドの色（席が無ければ accent）。
    fn captain_color_for(&self, task: &SpaceId, cx: &App) -> Hsla {
        self.project_sessions
            .projects
            .iter()
            .find(|slot| &slot.task_space.id == task)
            .and_then(|slot| self.captain_integration_index(slot.repository_key()))
            .and_then(|integration| self.project_sessions.sessions.get(integration))
            .and_then(|session| session.fleet_agents.first())
            .and_then(|panel| {
                let panel = panel.read(cx);
                let seat = panel.seat_thread(captain::CAPTAIN_SEAT)?;
                panel.beacons().get(seat).map(|(_, color, _)| *color)
            })
            .unwrap_or_else(|| self.accent())
    }

    /// その Task の承認待ちの要求（スレッド名と要求・スレッドの並び順）。
    fn pending_permissions(
        &self,
        index: usize,
        cx: &App,
    ) -> Vec<(SharedString, agent_panel::PermissionCard)> {
        let Some(session) = self.project_sessions.sessions.get(index) else {
            return Vec::new();
        };
        session
            .agent_statuses(cx)
            .into_iter()
            .filter_map(|(panel, thread, status)| {
                panel
                    .read(cx)
                    .permission_card(thread)
                    .map(|card| (status.name, card))
            })
            .collect()
    }

    /// その Task のその承認要求への推薦。表示は今の要求の id で引く（古い要求への推薦は出ない）。
    pub(crate) fn captain_recommendation_for(
        &self,
        task: &SpaceId,
        permission_id: &str,
    ) -> Option<&CaptainRecommendation> {
        self.chrome
            .captain_recommendations
            .iter()
            .find(|recommendation| {
                &recommendation.task == task
                    && recommendation.permission_id.as_ref() == permission_id
            })
    }

    /// 解決・取り消し・別の要求に替わった承認への推薦を捨てる。表示は今の要求の id で引くので、これは
    /// 残骸の掃除（パネルの出来事と、要対応カードのボタンを押した後に呼ぶ）。
    pub(crate) fn prune_captain_recommendations(&mut self, cx: &App) {
        if self.chrome.captain_recommendations.is_empty() {
            return;
        }
        let mut live: Vec<(SpaceId, SharedString)> = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if !self
                .chrome
                .captain_recommendations
                .iter()
                .any(|recommendation| recommendation.task == slot.task_space.id)
            {
                continue;
            }
            for (_, card) in self.pending_permissions(index, cx) {
                live.push((slot.task_space.id.clone(), card.id));
            }
        }
        self.chrome
            .captain_recommendations
            .retain(|recommendation| {
                live.iter().any(|(task, permission_id)| {
                    *task == recommendation.task && *permission_id == recommendation.permission_id
                })
            });
    }

    /// Captain を起こす 1 通に添える「今の承認待ち」（§5.5）。まだ推薦を付けていない要求だけを、Task の id・
    /// 要求の文・permission_id で 1 行ずつ。無ければ空。
    pub(crate) fn captain_permission_lines(&self, repository: &str, cx: &App) -> Vec<String> {
        let mut lines = Vec::new();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.repository_key() != repository || slot.task_space.is_integration() {
                continue;
            }
            for (_, card) in self.pending_permissions(index, cx) {
                if self
                    .captain_recommendation_for(&slot.task_space.id, &card.id)
                    .is_some()
                {
                    continue;
                }
                lines.push(i18n::t!(
                    "captain.recommend_wake_line",
                    "task" => slot.task_space.id.as_str(),
                    "title" => slot.task_space.title.as_ref(),
                    "request" => request_line(&card.title),
                    "permission" => card.id.as_ref(),
                ));
            }
        }
        lines
    }

    /// 要対応の承認待ちのカードに添える 1 行: `✳ Captain: 許可してよい — 理由`。✳ だけテラコッタ（LLM 生成の
    /// 印・UI-SPEC §1.3 / §11）で、見立ては太さで立てて色相は使わない。理由が長ければ省略し、全文はツールチップ。
    pub(super) fn render_captain_recommendation(
        &self,
        position: usize,
        recommendation: &CaptainRecommendation,
    ) -> gpui::AnyElement {
        let theme = &self.theme;
        let headline = recommendation.headline();
        let tip = format!("{headline} — {}", recommendation.reason);
        // 見立ての下に理由を 2 行まで（サイドバー 256px で見立てと同じ行に置くと、理由が 10 字ほどで切れて
        // 読めなかった）。全文はツールチップとニュース。
        div()
            .id(("control-recommend", position))
            .flex()
            .gap(px(5.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(theme_core::claude_bullet())
                    .child("✳"),
            )
            .child(
                div()
                    .flex_1() // min_w_0 は flex_1 とセット（単独だと幅 0 に潰れて消える）
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.fg0)
                            .child(SharedString::from(headline)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.fg1)
                            .line_clamp(2)
                            .text_ellipsis()
                            .child(SharedString::from(recommendation.reason.clone())),
                    ),
            )
            .tooltip(Tooltip::text(tip, theme.clone()))
            .into_any_element()
    }

    /// 開発用（`NECODER_FLEET_PROBE=recommend[:allow|deny|ask_human]`）: 承認待ちの最初の Task に Captain の推薦を
    /// 仕込む（`NECODER_CONTROL_PROBE=1` の「バグ #412」の `cargo publish`・既定は deny）。台帳には書かない。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_seed_captain_recommendation(
        &mut self,
        verdict: &str,
        cx: &mut Context<Self>,
    ) {
        let verdict = RecommendationVerdict::parse(verdict).unwrap_or(RecommendationVerdict::Deny);
        let target = self
            .project_sessions
            .projects
            .iter()
            .enumerate()
            .filter(|(_, slot)| !slot.task_space.is_integration())
            .find_map(|(index, slot)| {
                self.pending_permissions(index, cx)
                    .into_iter()
                    .next()
                    .map(|(_, card)| {
                        (
                            slot.task_space.id.clone(),
                            slot.task_space.title.clone(),
                            card.id,
                        )
                    })
            });
        let Some((task, title, permission_id)) = target else {
            eprintln!(
                "FLEET_PROBE: 承認待ちの Task が無い（NECODER_CONTROL_PROBE=1 と一緒に使う）"
            );
            return;
        };
        let reason = match verdict {
            RecommendationVerdict::Allow => "worktree の中で走らせるだけ（書き込みなし）",
            RecommendationVerdict::Deny => "crates.io への公開は取り消せない。Task の範囲の外",
            RecommendationVerdict::AskHuman => "公開するかは Task の範囲の外。中身を見て決めて",
        };
        let news = i18n::t!(
            "captain.recommend_news",
            "task" => title.as_ref(),
            "verdict" => verdict.label(),
            "reason" => reason,
        );
        self.remember_captain_recommendation(
            CaptainRecommendation {
                task,
                permission_id,
                verdict,
                reason: SharedString::from(reason),
            },
            news,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(value: serde_json::Value) -> Result<RecommendationRequest, String> {
        validate_recommendation(&value)
    }

    /// 引数は GUI や画面に触れる前に確かめる。見立ては 3 つのどれか、理由は 1 行で上限まで。
    #[test]
    fn a_recommendation_needs_a_known_verdict_and_a_one_line_reason() {
        let request = arguments(serde_json::json!({
            "task_id": " space-rope ",
            "permission_id": "1727-3",
            "verdict": "allow",
            "reason": " worktree の中で cargo test を走らせるだけ ",
        }))
        .expect("正しい引数は通る");
        assert_eq!(request.task_id, "space-rope", "前後の空白は落とす");
        assert_eq!(request.verdict, RecommendationVerdict::Allow);
        assert_eq!(request.reason, "worktree の中で cargo test を走らせるだけ");

        for field in ["task_id", "permission_id", "verdict", "reason"] {
            let mut value = serde_json::json!({
                "task_id": "space-rope", "permission_id": "1727-3", "verdict": "deny", "reason": "r",
            });
            value[field] = serde_json::json!("  ");
            let missing = arguments(value).expect_err("空の必須は断る");
            assert!(
                missing.starts_with(field),
                "何が足りないかを返す: {missing}"
            );
        }
        let unknown = arguments(serde_json::json!({
            "task_id": "t", "permission_id": "p", "verdict": "approve", "reason": "r",
        }))
        .expect_err("知らない見立ては断る");
        assert!(unknown.contains("ask_human"), "使える値を添える: {unknown}");
        assert!(arguments(serde_json::json!({
            "task_id": "t", "permission_id": "p", "verdict": "allow", "reason": "1 行目\n2 行目",
        }))
        .is_err());
        let long = "あ".repeat(MAX_RECOMMENDATION_REASON_CHARS + 1);
        assert!(arguments(serde_json::json!({
            "task_id": "t", "permission_id": "p", "verdict": "allow", "reason": long,
        }))
        .is_err());
        let longest = "あ".repeat(MAX_RECOMMENDATION_REASON_CHARS);
        assert!(
            arguments(serde_json::json!({
                "task_id": "t", "permission_id": "p", "verdict": "ask_human", "reason": longest,
            }))
            .is_ok(),
            "上限ちょうどは通す（数えるのは文字）"
        );
        for verdict in RecommendationVerdict::ALL {
            assert_eq!(
                RecommendationVerdict::parse(verdict.as_str()),
                Some(verdict)
            );
        }
    }

    #[test]
    fn the_request_line_keeps_only_the_first_line() {
        assert_eq!(request_line("Bash: cargo test"), "Bash: cargo test");
        assert_eq!(
            request_line("cat > a.rs <<EOF\nfn main() {}\nEOF"),
            "cat > a.rs <<EOF …"
        );
        let long = "x".repeat(WAKE_REQUEST_CHARS + 10);
        assert_eq!(request_line(&long).chars().count(), WAKE_REQUEST_CHARS + 2);
    }

    /// テスト用の一時フォルダと設定（Captain 任命済み・エージェントの起動コマンドは存在しないパス＝本物の
    /// エージェントを起こさない・✳ 要約と自動命名も止める）。本物の設定と台帳には触れない。
    fn scratch(label: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "necoder_recommend_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(root.join("repo")).expect("一時フォルダを作れる");
        std::fs::create_dir_all(root.join("task")).expect("一時フォルダを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false,"captain_agent":"Claude Code",
                "tier2_summaries":false,"agent_auto_name":false,
                "agent_servers":{"claude":{"type":"custom","command":"/nonexistent/necoder-test-agent"}}}"#,
        )
        .expect("設定を書ける");
        (root, settings_path)
    }

    fn clean_up(root: &Path) {
        if let Err(error) = std::fs::remove_dir_all(root) {
            eprintln!("一時フォルダを消せない（{}）: {error}", root.display());
        }
    }

    /// 統合先（repo）と、同じリポジトリの Task（task）を開く。返すのは (窓, 台帳, リポジトリの鍵)。
    fn open_fleet<'a>(
        root: &Path,
        settings_path: &Path,
        cx: &'a mut gpui::TestAppContext,
    ) -> (
        Entity<Workspace>,
        storage::Storage,
        String,
        &'a mut gpui::VisualTestContext,
    ) {
        let settings_path = settings_path.to_path_buf();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let storage = storage::Storage::open(&root.join("necoder.db")).expect("台帳を開ける");
        let persistence = WindowPersistence {
            storage: Some(storage.clone()),
            window_id: Some("w1".to_string()),
        };
        let projects = vec![root.join("repo"), root.join("task")];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(projects, Theme::dark(), Some(persistence), cx)
        });
        cx.run_until_parked();
        let repository = workspace.update(cx, |workspace, _| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            let repository = workspace.project_sessions.projects[0]
                .repository_key()
                .to_string();
            let task = &mut workspace.project_sessions.projects[1].task_space;
            task.kind = SpaceKind::Task;
            task.repository_id = repository.clone();
            task.title = SharedString::from("rope 設計");
            repository
        });
        (workspace, storage, repository, cx)
    }

    /// Task の担当に承認要求を 1 つ届ける（本物の `PermissionRequest` の経路）。返すのは (要求の id, 答えの受け口)。
    fn request_permission(
        workspace: &Entity<Workspace>,
        title: &str,
        cx: &mut gpui::VisualTestContext,
    ) -> (
        SharedString,
        futures::channel::mpsc::UnboundedReceiver<usize>,
    ) {
        let panel = workspace.update(cx, |workspace, _| {
            workspace.project_sessions.sessions[1].fleet_agents[0].clone()
        });
        let answers = panel.update(cx, |panel, cx| panel.debug_request_permission(title, cx));
        cx.run_until_parked();
        let id = panel.read_with(cx, |panel, _| {
            panel
                .permission_card(panel.active_thread())
                .map(|card| card.id)
                .expect("承認待ちになる")
        });
        (id, answers)
    }

    /// 推薦は「どの Task の、どの承認要求か」に付く: 今の要求にだけ受け、カードの素材（要対応のキュー）に
    /// 出る。解決したら消え、次の要求には付かない。古い要求の id では受けない。台帳とニュースに残る。
    #[gpui::test]
    fn a_recommendation_belongs_to_the_request_it_was_made_for(cx: &mut gpui::TestAppContext) {
        let (root, settings_path) = scratch("belongs");
        let (workspace, storage, repository, cx) = open_fleet(&root, &settings_path, cx);
        let task_id = workspace.read_with(cx, |workspace, _| {
            workspace.project_sessions.projects[1].task_space.id.clone()
        });
        let (first, mut answers) = request_permission(&workspace, "Bash: cargo test", cx);

        workspace.update(cx, |workspace, cx| {
            // 知らせに今の承認待ちが載る（推薦を付ける前）。
            let lines = workspace.captain_permission_lines(&repository, cx);
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(lines[0].contains(task_id.as_str()) && lines[0].contains(first.as_ref()));
            assert!(lines[0].contains("Bash: cargo test"));

            // 統合先には推薦しない・知らない Task は断る。
            let integration = workspace.project_sessions.projects[0].task_space.id.clone();
            let refused = workspace.accept_captain_recommendation(
                &serde_json::json!({
                    "task_id": integration.as_str(), "permission_id": first.as_ref(),
                    "verdict": "allow", "reason": "r",
                }),
                cx,
            );
            assert!(refused.is_err());
            assert!(workspace
                .accept_captain_recommendation(
                    &serde_json::json!({
                        "task_id": "space-missing", "permission_id": first.as_ref(),
                        "verdict": "allow", "reason": "r",
                    }),
                    cx,
                )
                .is_err());

            let reply = workspace
                .accept_captain_recommendation(
                    &serde_json::json!({
                        "task_id": task_id.as_str(), "permission_id": first.as_ref(),
                        "verdict": "allow", "reason": "worktree の中で走らせるだけ",
                    }),
                    cx,
                )
                .expect("今の要求には付く");
            assert_eq!(reply["status"], "shown");

            // 要対応のキューのカード（承認待ち）が、その要求の id で推薦を引ける＝ ✳ の行が出る条件。
            let queue = workspace.control_attention_queue(cx);
            let card = queue
                .iter()
                .find_map(|item| match &item.kind {
                    control_view::AttentionKind::Permission(card) => Some(card.id.clone()),
                    _ => None,
                })
                .expect("承認待ちのカードがある");
            let shown = workspace
                .captain_recommendation_for(&task_id, &card)
                .expect("カードに推薦が出る");
            assert_eq!(shown.verdict, RecommendationVerdict::Allow);
            assert!(
                workspace
                    .captain_permission_lines(&repository, cx)
                    .is_empty(),
                "推薦を付けた要求は知らせに重ねて載せない"
            );
            // ニュースは Captain の丸チップの行で、押すとその Task へ。
            let news = &workspace.notifications.news[0];
            assert!(news.kind == NewsKind::Captain);
            assert_eq!(news.space.as_ref(), Some(&task_id));
            assert!(
                news.text.contains("worktree の中で走らせるだけ"),
                "{}",
                news.text
            );
        });
        cx.run_until_parked();
        let events = storage
            .load_task_events(task_id.as_str())
            .expect("台帳を読める");
        let recorded = events
            .iter()
            .find(|event| event.kind == "captain")
            .expect("推薦は captain の采配として台帳に残る");
        let payload: serde_json::Value =
            serde_json::from_str(&recorded.payload).expect("payload は JSON");
        assert_eq!(payload["recommendation"]["verdict"], "allow");
        assert_eq!(payload["recommendation"]["permission_id"], first.as_ref());

        // 人間が許可する（推薦は応答しない＝答えは人間が押した添字だけ）→ 推薦は消える。
        let panel = workspace.update(cx, |workspace, _| {
            workspace.project_sessions.sessions[1].fleet_agents[0].clone()
        });
        panel.update(cx, |panel, cx| {
            let thread = panel.active_thread();
            panel.respond_permission(thread, 0, cx)
        });
        assert_eq!(
            answers.try_recv().ok(),
            Some(0),
            "答えは人間の選んだ許可だけ"
        );
        workspace.update(cx, |workspace, cx| {
            workspace.prune_captain_recommendations(cx);
            assert!(
                workspace.chrome.captain_recommendations.is_empty(),
                "解決した要求の推薦は消える"
            );
        });

        // 次の要求には前の推薦が付かない。古い要求の id では受けず、今の要求を知らせる。
        let (second, _answers) = request_permission(&workspace, "Bash: cargo publish", cx);
        assert_ne!(first, second);
        workspace.update(cx, |workspace, cx| {
            assert!(workspace
                .captain_recommendation_for(&task_id, &second)
                .is_none());
            let stale = workspace
                .accept_captain_recommendation(
                    &serde_json::json!({
                        "task_id": task_id.as_str(), "permission_id": first.as_ref(),
                        "verdict": "allow", "reason": "r",
                    }),
                    cx,
                )
                .expect_err("古い要求への推薦は受けない");
            assert!(
                stale.contains(second.as_ref()),
                "今の要求を知らせる: {stale}"
            );
            assert!(workspace.chrome.captain_recommendations.is_empty());
        });
        clean_up(&root);
    }

    /// 替わった要求への推薦は、パネルの出来事（次の承認待ち・ターンの終わり）で掃除される。
    /// Captain が居ない（未任命）時は受けない。
    #[gpui::test]
    fn recommendations_are_swept_when_the_request_changes_and_need_a_captain(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, settings_path) = scratch("sweep");
        let (workspace, _storage, _repository, cx) = open_fleet(&root, &settings_path, cx);
        let task_id = workspace.read_with(cx, |workspace, _| {
            workspace.project_sessions.projects[1].task_space.id.clone()
        });
        let (first, _answers) = request_permission(&workspace, "Bash: cargo test", cx);
        workspace.update(cx, |workspace, cx| {
            workspace
                .accept_captain_recommendation(
                    &serde_json::json!({
                        "task_id": task_id.as_str(), "permission_id": first.as_ref(),
                        "verdict": "deny", "reason": "範囲の外",
                    }),
                    cx,
                )
                .expect("今の要求には付く");
        });
        // 同じスレッドに次の要求が届く（前の要求は取り消された形）→ PermissionWaiting で掃除される。
        let (second, _second_answers) = request_permission(&workspace, "Bash: rm -rf target", cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace
                .captain_recommendation_for(&task_id, &first)
                .is_none());
            assert!(workspace
                .captain_recommendation_for(&task_id, &second)
                .is_none());
            assert!(
                workspace.chrome.captain_recommendations.is_empty(),
                "替わった要求への推薦は残さない"
            );
        });
        workspace.update(cx, |workspace, cx| {
            settings::set_user_value(cx, "captain_agent", serde_json::Value::Null)
                .expect("解任を書ける");
            let refused = workspace.accept_captain_recommendation(
                &serde_json::json!({
                    "task_id": task_id.as_str(), "permission_id": second.as_ref(),
                    "verdict": "allow", "reason": "r",
                }),
                cx,
            );
            assert!(refused.is_err(), "Captain が居なければ推薦を受けない");
            assert!(workspace.chrome.captain_recommendations.is_empty());
        });
        clean_up(&root);
    }

    /// Blocked で起こす 1 通には、今の承認要求と「fleet_recommend で 1 行返してよい・応答はしない」が添わる。
    #[gpui::test]
    fn the_captain_is_told_how_to_recommend_when_a_task_waits_for_approval(
        cx: &mut gpui::TestAppContext,
    ) {
        let (root, settings_path) = scratch("wake");
        let (workspace, _storage, repository, cx) = open_fleet(&root, &settings_path, cx);
        let settle = |cx: &mut gpui::VisualTestContext| {
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(3));
            cx.run_until_parked();
        };
        // 初めての任命: 読んだ位置をいまの末尾に置く。
        workspace.update(cx, |workspace, cx| {
            workspace.request_captain_wake(&repository, cx)
        });
        settle(cx);
        let (permission, _answers) = request_permission(&workspace, "Bash: cargo test", cx);
        workspace.update(cx, |workspace, cx| {
            workspace.request_captain_wake(&repository, cx)
        });
        settle(cx);
        let panel = workspace.update(cx, |workspace, _| {
            workspace.project_sessions.sessions[0].fleet_agents[0].clone()
        });
        panel.read_with(cx, |panel, _| {
            let seat = panel
                .seat_thread(captain::CAPTAIN_SEAT)
                .expect("Captain が起きる");
            let delivered = panel.statuses()[seat]
                .last_prompt
                .clone()
                .unwrap_or_default();
            assert!(delivered.contains("→ blocked"), "台帳の未読: {delivered}");
            assert!(
                delivered.contains("fleet_recommend"),
                "推薦の道具の案内: {delivered}"
            );
            assert!(
                delivered.contains(&format!("permission_id={permission}")),
                "今の要求の id を渡す: {delivered}"
            );
        });
        clean_up(&root);
    }
}
