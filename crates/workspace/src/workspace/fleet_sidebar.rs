//! Fleet サイドバー（FLEET-V2 §3.2）— **Task を主語にした一覧**。
//!
//! 旧 herd サイドバーは「プロジェクト見出し + スレッド行」だったので、並走している worktree が
//! いくつあるのか・どれに何を頼んだのかが読めなかった。ここでは 1 行 = 1 Task にして、
//! 「頼んだこと → いま何を」の読み順を 3 段で固定する。
//!
//! 上から: Captain バー / リポジトリ見出し / 統合先行 / Task 行 / ＋ Task + 凡例。
//! **要対応（`attention_queue`）は F3 で管制から移設する**（この文書の §3.2-1）。
//!
//! 出すのは**レールで選んでいる 1 リポジトリの編隊だけ**（§3.1）。他プロジェクトの稼働は
//! レールのドットと statusbar のロールアップが担うので、ここに混ぜない。

use crate::workspace::*;

/// Task 行 1 本分の素材（render 前に所有データへ畳む。`cx` を跨いで借用しない）。
struct TaskRow {
    project_index: usize,
    title: SharedString,
    color: Hsla,
    branch: Option<SharedString>,
    activity: agent_panel::ThreadActivity,
    /// 人間が最後に頼んだこと（2 段目・✳ は付けない＝LLM 生成ではない）。
    asked: Option<SharedString>,
    /// エージェントがいま何をしているか / どう終わったか（3 段目・Tier1 digest）。
    digest: Option<SharedString>,
    tokens: u32,
    /// レール上の並び（`⌘N` = `ActivateProjectN` と一致させる。行の並び順ではない＝嘘をつかない）。
    rail_shortcut: Option<usize>,
    /// いちばん最近の依頼の時刻（並べ替え「最近」の鍵・O21）。
    last_input_at_ms: Option<i64>,
}

/// Task 行の並べ方（O21）。既定はレールの順。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FleetSort {
    #[default]
    Rail,
    /// いちばん最近頼んだ Task から。
    Recent,
    /// 要対応（承認・質問待ち）→ 作業中 → 完了・未確認 → 静か の順。
    Attention,
}

impl FleetSort {
    /// 押すたびに次へ（レール → 最近 → 要対応 → レール）。
    fn next(self) -> Self {
        match self {
            FleetSort::Rail => FleetSort::Recent,
            FleetSort::Recent => FleetSort::Attention,
            FleetSort::Attention => FleetSort::Rail,
        }
    }

    fn label(self) -> String {
        match self {
            FleetSort::Rail => i18n::t!("fleet.sort_rail"),
            FleetSort::Recent => i18n::t!("fleet.sort_recent"),
            FleetSort::Attention => i18n::t!("fleet.sort_attention"),
        }
    }
}

/// Task 行を並べ替える（同じ鍵の間はレールの順を保つ）。
fn sort_task_rows(rows: &mut [TaskRow], sort: FleetSort) {
    match sort {
        FleetSort::Rail => {}
        FleetSort::Recent => {
            rows.sort_by_key(|row| std::cmp::Reverse(row.last_input_at_ms.unwrap_or(i64::MIN)))
        }
        FleetSort::Attention => rows.sort_by_key(|row| std::cmp::Reverse(row.activity.urgency())),
    }
}

/// Task がこの数以上ある時に絞り込み欄を出す（少ない時は一覧を見れば足りる・O21）。
const FLEET_FILTER_MIN_TASKS: usize = 6;

/// Task 行が絞り込みの語に当たるか（O21）: 語を空白で区切り、全部の語が名前・ブランチ・
/// 頼んだこと・いま何を のどれかに含まれる（大文字小文字は無視）。語が無ければ全部当たる。
fn task_row_matches(row: &TaskRow, query: &str) -> bool {
    let haystack = [
        Some(row.title.as_ref()),
        row.branch.as_deref(),
        row.asked.as_deref(),
        row.digest.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
    .to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

impl Workspace {
    /// いま編隊として見ているリポジトリの鍵（レールで選んでいる slot のもの）。
    fn fleet_repository_key(&self) -> Option<String> {
        self.active_slot()
            .map(|slot| slot.repository_key().to_string())
    }

    /// リポジトリの統合先 slot（`⌂`）。同じリポジトリに統合先扱いの slot が複数ある時（`task/` でない
    /// linked worktree を ⌘O で開いた等）は、**メインの作業ツリー**（linked でない）を選ぶ。無ければ
    /// 最初の統合先（O21・以前はサイドバーだけ最後の 1 つを選び、＋Task・Captain と食い違った）。
    pub(crate) fn integration_slot_for(&self, key: &str) -> Option<usize> {
        let mut first = None;
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if slot.repository_key() != key || !slot.task_space.is_integration() {
                continue;
            }
            if !slot.task_space.linked {
                return Some(index);
            }
            first.get_or_insert(index);
        }
        first
    }

    /// 選んでいるリポジトリの worktree 一覧を背景で読み直す（O21・読み終えたら描き直す）。
    /// 読みに行くのは統合先（無ければ選んでいる slot）の場所から。
    pub(crate) fn refresh_fleet_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.fleet_repository_key() else {
            return;
        };
        if matches!(self.chrome.fleet_worktrees.get(&key), Some(None)) {
            return; // 読んでいる途中
        }
        let Some(slot) = self
            .integration_slot_for(&key)
            .or(Some(self.project_sessions.active))
            .and_then(|index| self.project_sessions.projects.get(index))
        else {
            return;
        };
        let host = slot.worktree.host().clone();
        let root = slot.worktree.root().to_path_buf();
        self.chrome.fleet_worktrees.insert(key.clone(), None);
        cx.spawn(async move |workspace, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { project::git_worktrees_on(host.as_ref(), &root) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                workspace.chrome.fleet_worktrees.insert(key, Some(listed));
                cx.notify();
            });
        })
        .detach();
    }

    /// Fleet を出している間、選んでいるリポジトリの一覧がまだ無ければ読みに行く（描画から呼ぶ）。
    pub(crate) fn ensure_fleet_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.fleet_repository_key() else {
            return;
        };
        if !self.chrome.fleet_worktrees.contains_key(&key) {
            self.refresh_fleet_worktrees(cx);
        }
    }

    /// worktree の一覧を古い扱いにする（次に Fleet サイドバーを描く時に読み直す）。
    pub(crate) fn forget_fleet_worktrees(&mut self) {
        self.chrome
            .fleet_worktrees
            .retain(|_, listed| listed.is_none());
    }

    /// 選んでいるリポジトリの worktree のうち、レールに無いもの（necoder の外で作ったもの = Orca・
    /// Claude Code・手で `git worktree add` した等）と、レールにあるが Task でないもの（統合先以外）。
    /// 返すのは `(パス, ブランチ, レールの添字)`。一覧をまだ読んでいなければ空。
    pub(crate) fn external_worktrees(&self) -> Vec<(PathBuf, Option<String>, Option<usize>)> {
        let Some(key) = self.fleet_repository_key() else {
            return Vec::new();
        };
        let integration = self.integration_slot_for(&key);
        let canonical =
            |path: &Path| paths::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let rail: Vec<(usize, PathBuf)> = self
            .project_sessions
            .projects
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.repository_key() == key)
            .map(|(index, slot)| (index, canonical(slot.worktree.root())))
            .collect();
        let mut rows = Vec::new();
        if let Some(Some(listed)) = self.chrome.fleet_worktrees.get(&key) {
            for worktree in listed {
                let path = canonical(&worktree.path);
                match rail.iter().find(|(_, root)| *root == path) {
                    None => rows.push((worktree.path.clone(), worktree.branch.clone(), None)),
                    // レールにあるが Task でも統合先でもない（`task/` でない linked worktree を ⌘O で開いた）。
                    Some((index, _))
                        if Some(*index) != integration
                            && self.project_sessions.projects[*index]
                                .task_space
                                .is_integration() =>
                    {
                        rows.push((worktree.path.clone(), worktree.branch.clone(), Some(*index)))
                    }
                    Some(_) => {}
                }
            }
        }
        rows
    }

    /// レールにある Task のうち、git の一覧から消えた（necoder の外で `git worktree remove` 等された）もの。
    pub(crate) fn vanished_worktree(&self, index: usize) -> bool {
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return false;
        };
        let Some(Some(listed)) = self.chrome.fleet_worktrees.get(slot.repository_key()) else {
            return false;
        };
        let canonical =
            |path: &Path| paths::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let root = canonical(slot.worktree.root());
        !slot.worktree.is_remote()
            && !listed
                .iter()
                .any(|worktree| canonical(&worktree.path) == root)
    }

    /// 外部の worktree を取り込む（O21）: レールに開いて Task にする（ブランチ名に関係なく）。
    /// 既にレールにある（統合先扱いの）ものは、その場で Task にして台帳に残す。
    pub(crate) fn adopt_worktree(
        &mut self,
        path: PathBuf,
        branch: Option<String>,
        rail_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = rail_index {
            self.make_task_space(index, cx);
            cx.notify();
            return;
        }
        let host = self
            .fleet_repository_key()
            .and_then(|key| self.integration_slot_for(&key))
            .and_then(|index| self.project_sessions.projects.get(index))
            .map(|slot| slot.worktree.host().clone())
            .unwrap_or_else(host::LocalHost::shared);
        self.chrome.adopt_as_task.insert(path.clone());
        self.open_folder_in_rail(host, path, branch, cx);
    }

    /// レールの slot を Task にして台帳へ残す（O21・取り込み）。
    pub(crate) fn make_task_space(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(slot) = self.project_sessions.projects.get_mut(index) else {
            return;
        };
        slot.task_space.kind = SpaceKind::Task;
        if slot.task_space.base_oid.is_none() {
            slot.task_space.base_oid = slot.task_space.head_oid.clone();
        }
        let record = slot.task_space.to_record(slot);
        if let Some(storage) = self.persistence.storage.clone() {
            cx.background_executor()
                .spawn(async move {
                    if let Err(error) = storage.upsert_task_space(&record) {
                        eprintln!("取り込んだ worktree を台帳に残せない: {error:#}");
                    }
                })
                .detach();
        }
        cx.notify();
    }

    /// 外で消えた worktree の Task をレールから外す（O21・「片付け」）。
    /// Task 行の ⌘ クリック（O21・Windows / Linux は Ctrl）: 選択に足す / 外す。
    pub(crate) fn toggle_fleet_selection(&mut self, space: SpaceId, cx: &mut Context<Self>) {
        if let Some(position) = self
            .chrome
            .fleet_selection
            .iter()
            .position(|selected| selected == &space)
        {
            self.chrome.fleet_selection.remove(position);
        } else {
            self.chrome.fleet_selection.push(space.clone());
        }
        self.chrome.fleet_selection_anchor = Some(space);
        cx.notify();
    }

    /// Task 行の ⇧ クリック: 起点（最後に ⌘ / ⇧ で押した行）から押した行までを、見えている並びで選ぶ。
    /// 起点が見えていなければ押した行だけ。
    pub(crate) fn select_fleet_range(
        &mut self,
        order: &[SpaceId],
        space: SpaceId,
        cx: &mut Context<Self>,
    ) {
        let anchor = self
            .chrome
            .fleet_selection_anchor
            .clone()
            .unwrap_or_else(|| space.clone());
        let clicked = order.iter().position(|candidate| candidate == &space);
        let from = order.iter().position(|candidate| candidate == &anchor);
        self.chrome.fleet_selection = match (from, clicked) {
            (Some(from), Some(to)) => order[from.min(to)..=from.max(to)].to_vec(),
            _ => vec![space.clone()],
        };
        if self.chrome.fleet_selection_anchor.is_none() {
            self.chrome.fleet_selection_anchor = Some(space);
        }
        cx.notify();
    }

    /// 選択を外す（普通のクリック・まとめての操作の後・× ボタン）。
    pub(crate) fn clear_fleet_selection(&mut self, cx: &mut Context<Self>) {
        if self.chrome.fleet_selection.is_empty() && self.chrome.fleet_selection_anchor.is_none() {
            return;
        }
        self.chrome.fleet_selection.clear();
        self.chrome.fleet_selection_anchor = None;
        cx.notify();
    }

    /// まとめて休ませる: 選んだ Task の静かなエージェントを止める（会話は残り、次に送ると続きから）。
    pub(crate) fn rest_selected_tasks(&mut self, spaces: &[SpaceId], cx: &mut Context<Self>) {
        let sessions: Vec<usize> = spaces
            .iter()
            .filter_map(|space| self.session_index_for_space(space))
            .collect();
        let stopped: usize = sessions
            .into_iter()
            .map(|index| self.stop_quiet_agents_of(index, cx))
            .sum();
        self.report_stopped_agents(stopped, cx);
        self.clear_fleet_selection(cx);
    }

    /// まとめて舞台に並べる（先頭の 3 本まで・舞台の枠は 3 枚）。
    pub(crate) fn stage_selected_tasks(&mut self, spaces: &[SpaceId], cx: &mut Context<Self>) {
        let known: Vec<SpaceId> = spaces
            .iter()
            .filter(|space| self.session_index_for_space(space).is_some())
            .cloned()
            .collect();
        if known.is_empty() {
            return;
        }
        if known.len() > 3 {
            let accent = self.accent();
            self.push_toast(
                SharedString::from(i18n::t!(
                    "fleet.selection_stage_first",
                    "count" => known.len()
                )),
                accent,
                cx,
            );
        }
        self.chrome.stage_pinned = known.into_iter().take(3).collect();
        self.chrome.stage_columns = self.chrome.stage_pinned.len().max(2);
        // ピンは窓セッションに残す（再起動を越える・O21）。
        self.save_state(cx);
        self.clear_fleet_selection(cx);
    }

    /// まとめて片付ける: 片付けの画面（O22）を、選んだ Task に印を付けた状態で開く。消す前に
    /// 失うもの（未コミットの変更・統合していないコミット）をそこで数える。
    pub(crate) fn cleanup_selected_tasks(
        &mut self,
        spaces: &[SpaceId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_cleanup(&ShowCleanup, window, cx);
        self.select_cleanup_rows(spaces);
        self.clear_fleet_selection(cx);
    }

    pub(crate) fn forget_vanished_task(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.project_sessions.projects.get(index) else {
            return;
        };
        let space = slot.task_space.id.clone();
        self.remove_fleet_cells_for(&space);
        self.remove_project_slot(index, window, cx);
        self.forget_fleet_worktrees();
    }

    /// Captain スレッド（IntegrationSpace の pinned thread）へ寄せる。⌘0 / Captain バーのクリック。
    /// Captain カード（舞台の 1 枚）は F6。ここでは「その会話を前面に出す」までを担う。
    pub(crate) fn focus_captain(
        &mut self,
        _: &FocusCaptain,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.fleet_repository_key() else {
            return;
        };
        let Some(index) = self.integration_slot_for(&key) else {
            return;
        };
        let Some(agent) = settings::get(cx).captain_agent.clone() else {
            // 未任命なら設定ホームの「AI エージェント」ページへ（任命ボタンの在処・§5.7）。
            self.open_settings_action(&OpenSettings, window, cx);
            self.chrome
                .settings_view
                .update(cx, |view, cx| view.show_agents_page(cx));
            return;
        };
        let panel = self.project_sessions.sessions[index].fleet_agents[0].clone();
        self.project_sessions.sessions[index].agent_panel = panel.clone();
        let context = self.captain_context(&key, cx);
        let names = captain::captain_thread_names();
        let thread = panel.update(cx, |panel, cx| {
            let thread =
                panel.ensure_named_thread(&captain::captain_thread_name(), &names, &agent, cx);
            panel.set_prompt_context(thread, context);
            panel.focus_thread(thread, cx);
            thread
        });
        self.chrome.fleet_mode = true;
        self.chrome.stage_columns = 1;
        self.chrome.captain_space =
            Some(self.project_sessions.projects[index].task_space.id.clone());
        self.chrome.captain_tab = 0;
        self.reveal_agent_in_fleet(index, thread, window, cx);
    }

    /// Captain バー（§3.2-0・最上段 46px）。**Task 行の並びに混ぜない**（Task ではないので）。
    fn render_captain_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let accent = self.accent();
        let agent = settings::get(cx).captain_agent.clone();
        // 2 行目 = 最後の采配 1 行（✳ = LLM 生成の印）。無ければ役割の一言。
        let last = self
            .notifications
            .news
            .iter()
            .find(|item| item.kind == NewsKind::Captain)
            .map(|item| item.text.clone());
        let shortcut = Self::shortcut_label_for("workspace::FocusCaptain").unwrap_or_default();
        div()
            .id("fleet-captain-bar")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(46.))
            .px(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(if agent.is_some() { accent } else { theme.fg2 })
                    .child("⚑"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(5.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.fg0)
                                    .child(SharedString::from(i18n::t!("captain.title"))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.))
                                    .text_color(theme.fg2)
                                    .child(match &agent {
                                        Some(agent) => SharedString::from(agent.clone()),
                                        None => SharedString::from(i18n::t!("captain.appoint")),
                                    }),
                            ),
                    )
                    // 最後の采配（✳ テラコッタ = LLM 生成の印・規律）。未任命/無采配では出さない。
                    .when_some(last.filter(|_| agent.is_some()), |element, last| {
                        element.child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(9.))
                                        .text_color(theme_core::claude_bullet())
                                        .child("✳"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(9.5))
                                        .text_color(theme.fg2)
                                        .child(last),
                                ),
                        )
                    }),
            )
            .when(!shortcut.is_empty(), |element| {
                element.child(
                    div()
                        .flex_none()
                        .text_size(px(9.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(shortcut)),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.focus_captain(&FocusCaptain, window, cx)
                }),
            )
            .into_any_element()
    }

    /// サイドバーに出す素材を 1 パスで集める（render から切り出して検証できるように）。
    /// 返すのは `(統合先行, Task 行, 今日の統合数)`。**レールで選んでいるリポジトリの slot だけ**。
    fn fleet_sidebar_rows(
        &self,
        cx: &App,
    ) -> (
        Option<(usize, SharedString, Hsla, Option<SharedString>)>,
        Vec<TaskRow>,
        usize,
    ) {
        use agent_panel::ThreadActivity;
        let key = self.fleet_repository_key();
        let integration_index = key
            .as_deref()
            .and_then(|key| self.integration_slot_for(key));
        let mut integration: Option<(usize, SharedString, Hsla, Option<SharedString>)> = None;
        let mut rows: Vec<TaskRow> = Vec::new();
        let mut integrated_today = 0usize;
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if key.as_deref() != Some(slot.repository_key()) {
                continue; // 他リポジトリの編隊は出さない（§3.1）
            }
            let branch = slot
                .branch
                .clone()
                .or_else(|| slot.worktree_branch.clone())
                .map(SharedString::from);
            if slot.task_space.is_integration() {
                // 統合先はメインの作業ツリー（O21）。ほかの統合先扱いの slot は「外部の worktree」に出る。
                if Some(index) == integration_index {
                    integration = Some((index, slot.name.clone(), slot.color, branch));
                }
                continue;
            }
            if slot.task_space.phase == TaskPhase::Archived {
                continue; // アーカイブ済みはサイドバーから消える（既存の規律）
            }
            if slot.task_space.phase == TaskPhase::Integrated {
                integrated_today += 1;
            }
            let Some(session) = self.project_sessions.sessions.get(index) else {
                continue;
            };
            let statuses = session.agent_statuses(cx);
            // Task の状態 = その worktree で**いちばん切迫しているスレッド**（1 Task = 1 行なので畳む）。
            let lead = statuses
                .iter()
                .max_by_key(|(_, _, status)| status.activity.urgency());
            rows.push(TaskRow {
                project_index: index,
                title: slot.task_space.title.clone(),
                color: slot.color,
                branch,
                activity: lead.map_or(ThreadActivity::Idle, |(_, _, status)| status.activity),
                asked: lead.and_then(|(_, _, status)| status.last_prompt.clone()),
                digest: slot
                    .task_space
                    .result_summary
                    .clone()
                    .or_else(|| lead.and_then(|(_, _, status)| status.digest.clone())),
                tokens: statuses
                    .iter()
                    .map(|(_, _, status)| status.tokens_used)
                    .sum(),
                // レールの並び = ⌘1..9（`ActivateProjectN`）。10 本目以降は出さない。
                rail_shortcut: (index < 9).then_some(index + 1),
                last_input_at_ms: statuses
                    .iter()
                    .filter_map(|(_, _, status)| status.last_input_at_ms)
                    .max(),
            });
        }
        (integration, rows, integrated_today)
    }

    /// Task の絞り込み欄（O21）。`⌕` + 1 行入力（placeholder「Task を絞り込む」）。
    /// 複数選択のまとめての操作（O21）: `N 本を選択 · 休ませる · 舞台に並べる · 片付け… · ×`。
    fn render_fleet_selection_bar(
        &self,
        selected: Rc<[SpaceId]>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let chip = |id: &'static str, label: SharedString, tip: String| {
            div()
                .id(id)
                .flex_none()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(3.))
                .border_1()
                .border_color(theme.border)
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg2))
                .child(label)
                .tooltip(Tooltip::text(tip, theme.clone()))
        };
        let rest = selected.clone();
        let stage = selected.clone();
        let cleanup = selected.clone();
        div()
            .id("fleet-selection-bar")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(5.))
            .mx(px(8.))
            .my(px(3.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(theme.bg2)
            .text_size(px(10.5))
            .child(
                div()
                    .flex_none()
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!(
                        "fleet.selection_count",
                        "n" => selected.len()
                    ))),
            )
            .child(div().flex_1())
            .child(
                chip(
                    "fleet-selection-rest",
                    SharedString::from(i18n::t!("fleet.cleanup_sleep")),
                    i18n::t!("fleet.selection_rest_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.rest_selected_tasks(&rest, cx);
                    }),
                ),
            )
            .child(
                chip(
                    "fleet-selection-stage",
                    SharedString::from(i18n::t!("fleet.selection_stage")),
                    i18n::t!("fleet.selection_stage_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.stage_selected_tasks(&stage, cx);
                    }),
                ),
            )
            .child(
                chip(
                    "fleet-selection-cleanup",
                    SharedString::from(i18n::t!("fleet.selection_cleanup")),
                    i18n::t!("fleet.selection_cleanup_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.cleanup_selected_tasks(&cleanup, window, cx);
                    }),
                ),
            )
            .child(
                chip(
                    "fleet-selection-clear",
                    SharedString::from("×"),
                    i18n::t!("fleet.selection_clear_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.clear_fleet_selection(cx);
                    }),
                ),
            )
            .into_any_element()
    }

    fn render_fleet_filter(&self, filtering: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        div()
            .mx(px(8.))
            .my(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(26.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.5))
            .child(div().flex_none().text_color(theme.fg2).child("⌕"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    // 高さを与えないと 1 行ぶんに潰れて文字が出ない（Chat の一覧の検索欄と同じ）。
                    .h(px(18.))
                    .child(self.chrome.fleet_filter.clone())
                    .when(
                        !filtering && self.chrome.fleet_filter_query.is_empty(),
                        |field| {
                            field.child(
                                div()
                                    .absolute()
                                    .top(px(0.))
                                    .left(px(1.))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(i18n::t!(
                                        "fleet.filter_placeholder"
                                    ))),
                            )
                        },
                    ),
            )
            // 並べ方（O21）: 押すたびに レール → 最近 → 要対応。
            .child(
                div()
                    .id("fleet-sort")
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child(SharedString::from(self.chrome.fleet_sort.label()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.chrome.fleet_sort = this.chrome.fleet_sort.next();
                            cx.notify();
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Fleet サイドバー本体。
    pub(crate) fn render_fleet_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use agent_panel::ThreadActivity;
        let theme = self.theme.clone();
        let (integration, rows, integrated_today) = self.fleet_sidebar_rows(cx);
        // 見出しの数は絞り込む前の全部（絞り込みは見せ方だけ・O21）。
        let total_tasks = rows.len();
        let filtering = !self.chrome.fleet_filter_query.trim().is_empty();
        let show_filter = filtering || total_tasks >= FLEET_FILTER_MIN_TASKS;
        let mut rows: Vec<TaskRow> = rows
            .into_iter()
            .filter(|row| task_row_matches(row, &self.chrome.fleet_filter_query))
            .collect();
        sort_task_rows(&mut rows, self.chrome.fleet_sort);
        // 見えている並び（⇧ クリックの範囲）と、その中で選んでいる Task（複数選択・O21）。
        let order: Rc<[SpaceId]> = rows
            .iter()
            .map(|row| {
                self.project_sessions.projects[row.project_index]
                    .task_space
                    .id
                    .clone()
            })
            .collect();
        let selected: Rc<[SpaceId]> = self
            .chrome
            .fleet_selection
            .iter()
            .filter(|space| order.contains(space))
            .cloned()
            .collect();

        let mut list = div()
            .id("fleet-task-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(2.));

        // ① リポジトリ見出し（● 名前 ⎇ 統合先ブランチ · N Tasks）。
        if let Some((_, name, color, branch)) = &integration {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(8.))
                    .pb(px(3.))
                    .child(div().size(px(7.)).rounded_full().bg(*color).flex_none())
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.fg1)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .when_some(branch.clone(), |element, branch| {
                                element.child(SharedString::from(format!("⎇ {branch}")))
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(
                                i18n::t!("fleet.tasks_count", "n" => total_tasks),
                            )),
                    ),
            );
        }

        // ② 統合先行（⌂ main · 統合先 · 保護）。Captain の次・Task 行の前（§3.2-3）。
        if let Some((index, _, color, branch)) = integration.clone() {
            let branch_label = branch.unwrap_or_else(|| SharedString::from("main"));
            list = list.child(
                div()
                    .id("fleet-integration-row")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_h(ui::row_height(cx, 38.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(color)
                            .child("⌂"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(12.))
                                    .text_color(theme.fg0)
                                    .child(branch_label),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(i18n::t!(
                                        "fleet.integration_sub",
                                        "branched" => total_tasks,
                                        "integrated" => integrated_today
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("fleet.integration_row"))),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.chrome.captain_space = None;
                            this.switch_project(index, window, cx);
                        }),
                    ),
            );
        }

        // Task の絞り込み欄（O21）: Task が多い時か、書いてある間だけ。
        if show_filter {
            list = list.child(self.render_fleet_filter(filtering, cx));
            if filtering && rows.is_empty() {
                list = list.child(
                    div()
                        .px(px(12.))
                        .py(px(6.))
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("fleet.filter_none"))),
                );
            }
        }

        // 作成中の Task（O20）: worktree と準備スクリプトが終わるまで。取り消し・やり直しつき。
        let repository_key = self.fleet_repository_key();
        list = list.children(self.render_task_creations(repository_key.as_deref(), cx));

        // 複数選択のまとめての操作（O21）: 選んでいる間だけ Task 行の上に出す。
        if !selected.is_empty() {
            list = list.child(self.render_fleet_selection_bar(selected.clone(), cx));
        }

        // ③ Task 行（3 段固定・§3.2-3）。
        for (seq, row) in rows.iter().enumerate() {
            let project_index = row.project_index;
            let space = order[seq].clone();
            let is_selected = selected.contains(&space);
            let order = order.clone();
            let color = row.color;
            let tokens = if row.tokens == 0 {
                SharedString::from("—")
            } else {
                SharedString::from(agent_panel::human_tokens(row.tokens))
            };
            let renaming_editor = self
                .chrome
                .task_renaming
                .as_ref()
                .filter(|renaming| {
                    renaming.index == project_index && renaming.site == RenameSite::Herd
                })
                .map(|renaming| renaming.editor.clone());
            list = list.child(
                div()
                    .id(("fleet-task-row", seq))
                    .group("fleet-task-row")
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    // 行の詰め具合（O27・`density`）。
                    .min_h(ui::row_height(cx, 54.))
                    .px(px(8.))
                    .py(ui::row_padding(cx, 5.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    // 選んでいる行は地を 1 段上げる（色相は使わない・選択面の色塗りはしない）。
                    .when(is_selected, |row| row.bg(theme.bg3))
                    // 左 2px Task 色バー（帰属＝どの Task か・UI-SPEC §11）。
                    .child(
                        div()
                            .w(px(2.5))
                            .h(px(40.))
                            .rounded_full()
                            .bg(color)
                            .flex_none(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mt(px(3.))
                            .child(agent_panel::activity_dot(
                                ("fleet-task-dot", seq),
                                9.0,
                                color,
                                row.activity,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            // 1 段目: 名前 + ⎇ branch。ダブルクリックで改名（既存）。
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(px(5.))
                                    .child(match renaming_editor {
                                        Some(editor) => div()
                                            .flex_1()
                                            .min_w_0()
                                            .h(px(20.))
                                            .child(editor)
                                            .into_any_element(),
                                        None => div()
                                            .flex_none()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(12.))
                                            .text_color(theme.fg0)
                                            .child(row.title.clone())
                                            .into_any_element(),
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(10.))
                                            .text_color(theme.fg2)
                                            .when_some(row.branch.clone(), |element, branch| {
                                                element.child(SharedString::from(format!(
                                                    "⎇ {branch}"
                                                )))
                                            }),
                                    ),
                            )
                            // 2 段目: 「頼んだこと」。人間の発話なので ✳ を付けず、`›` で引用の形にする。
                            // 長文は**先頭から詰める**（指示は最初に用件が来る）。
                            .when_some(row.asked.clone(), |element, asked| {
                                element.child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(10.))
                                        .text_color(theme.fg1)
                                        .child(SharedString::from(
                                            i18n::t!("fleet.asked", "text" => asked),
                                        )),
                                )
                            })
                            // 3 段目: digest（いま何を / どう終わったか）。
                            .when_some(row.digest.clone(), |element, digest| {
                                element.child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(9.5))
                                        .text_color(theme.fg2)
                                        .child(digest),
                                )
                            })
                            // necoder の外で worktree が消された（O21）。レールから外す導線を出す。
                            .when(self.vanished_worktree(project_index), |element| {
                                element.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(10.))
                                        .child(
                                            div()
                                                .text_color(theme.warn)
                                                .child(SharedString::from(i18n::t!(
                                                    "fleet.worktree_vanished"
                                                ))),
                                        )
                                        .child(
                                            div()
                                                .id(("fleet-task-forget", seq))
                                                .px(px(5.))
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(theme.border)
                                                .text_color(theme.fg1)
                                                .cursor_pointer()
                                                .hover(|style| style.bg(theme.bg2))
                                                .child(SharedString::from(i18n::t!(
                                                    "fleet.worktree_forget"
                                                )))
                                                .tooltip(Tooltip::text(
                                                    i18n::t!("fleet.worktree_forget_tip"),
                                                    theme.clone(),
                                                ))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(
                                                        move |this, _: &MouseDownEvent, window, cx| {
                                                            cx.stop_propagation();
                                                            this.forget_vanished_task(
                                                                project_index,
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ),
                                                ),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(2.))
                            .child(div().text_size(px(10.)).text_color(theme.fg2).child(tokens))
                            // ⌘N は**レールの並び**（`ActivateProjectN`）。行の並びではない＝嘘をつかない。
                            .when_some(row.rail_shortcut, |element, n| {
                                element.child(
                                    div()
                                        .text_size(px(9.))
                                        .text_color(theme.fg2)
                                        .child(SharedString::from(format!("⌘{n}"))),
                                )
                            }),
                    )
                    .child(div().id(("stage-pin", seq)).flex_none().text_size(px(12.)).text_color(color).cursor_pointer().child("◫")
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let space = this.project_sessions.projects[project_index].task_space.id.clone();
                            this.toggle_stage_pin(space, cx);
                        })))
                    // 🗑 worktree ごと削除（ホバーで出現・「失うものを数える」確認へ委譲）。
                    .child(
                        div()
                            .id(("fleet-task-trash", seq))
                            .group("fleet-task-trash")
                            .flex_none()
                            .invisible()
                            .group_hover("fleet-task-row", |style| style.visible())
                            .size(px(17.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(|style| style.bg(theme.bg2))
                            .child(
                                svg()
                                    .path("icons/trash-2.svg")
                                    .size(px(10.))
                                    .text_color(theme.fg2)
                                    .group_hover("fleet-task-trash", |style| {
                                        style.text_color(theme.err)
                                    }),
                            )
                            .tooltip(Tooltip::text(
                                i18n::t!("fleet.delete_worktree_tip"),
                                theme.clone(),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.request_worktree_delete(project_index, false, window, cx);
                                }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            // ⌘（Windows / Linux は Ctrl）= 選択に足す / 外す・⇧ = 範囲（O21）。
                            if event.modifiers.secondary() {
                                this.toggle_fleet_selection(space.clone(), cx);
                                return;
                            }
                            if event.modifiers.shift {
                                this.select_fleet_range(&order, space.clone(), cx);
                                return;
                            }
                            this.clear_fleet_selection(cx);
                            if event.click_count == 2 {
                                this.start_task_rename(project_index, RenameSite::Herd, window, cx);
                                return;
                            }
                            this.switch_project(project_index, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.open_rail_menu(project_index, event.position, cx);
                        }),
                    ),
            );
        }

        // ③' 外部の worktree（O21）: このリポジトリの worktree のうち Task になっていないもの。
        // necoder の外（Orca・Claude Code・手で `git worktree add`）で作ったものも git の一覧から拾う。
        let external = self.external_worktrees();
        if !external.is_empty() {
            let hidden = self.chrome.hide_external_worktrees;
            list = list.child(
                div()
                    .id("fleet-external-header")
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(3.))
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg1))
                    .child(if hidden { "▸" } else { "▾" })
                    .child(SharedString::from(i18n::t!(
                        "fleet.external_header",
                        "count" => external.len()
                    )))
                    .tooltip(Tooltip::text(i18n::t!("fleet.external_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                            this.chrome.hide_external_worktrees =
                                !this.chrome.hide_external_worktrees;
                            cx.notify();
                        }),
                    ),
            );
            if !hidden {
                for (seq, (path, branch, rail_index)) in external.into_iter().enumerate() {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string());
                    let detail = branch
                        .clone()
                        .map_or_else(|| "(detached)".to_string(), |branch| format!("⎇ {branch}"));
                    let adopt_path = path.clone();
                    let adopt_branch = branch.clone();
                    list = list.child(
                        div()
                            .id(("fleet-external-row", seq))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_h(ui::row_height(cx, 30.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .hover(|style| style.bg(theme.bg3))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(theme.fg2)
                                    .child("◌"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(11.5))
                                            .text_color(theme.fg1)
                                            .child(SharedString::from(name)),
                                    )
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_size(px(9.5))
                                            .text_color(theme.fg2)
                                            .child(SharedString::from(detail)),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("fleet-external-adopt", seq))
                                    .flex_none()
                                    .px(px(6.))
                                    .h(px(20.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme.border)
                                    .text_size(px(10.5))
                                    .text_color(theme.fg1)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                                    .child(SharedString::from(i18n::t!("fleet.external_adopt")))
                                    .tooltip(Tooltip::text(
                                        i18n::t!("fleet.external_adopt_tip"),
                                        theme.clone(),
                                    ))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, _: &MouseDownEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.adopt_worktree(
                                                    adopt_path.clone(),
                                                    adopt_branch.clone(),
                                                    rail_index,
                                                    cx,
                                                );
                                            },
                                        ),
                                    ),
                            ),
                    );
                }
            }
        }

        let body = if rows.is_empty() && integration.is_none() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(16.))
                .text_size(px(11.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("herd.empty")))
                .into_any_element()
        } else {
            list.into_any_element()
        };

        // ④ ＋ Task と凡例（**形の説明**・色は識別に使うので中立色で・UI-SPEC §11）。
        let neutral = theme.fg2;
        let legend_states = [
            ThreadActivity::Working,
            ThreadActivity::Blocked,
            ThreadActivity::Done { interrupted: false },
            ThreadActivity::Idle,
        ];
        let mut legend = div()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .border_t_1()
            .border_color(theme.border);
        for (index, activity) in legend_states.into_iter().enumerate() {
            legend = legend.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(agent_panel::activity_dot(
                        ("fleet-legend", index),
                        7.0,
                        neutral,
                        activity,
                    ))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(theme.fg2)
                            .child(activity_label(activity)),
                    ),
            );
        }

        let add_task = div()
            .id("fleet-add-task")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .h(px(30.))
            .mx(px(8.))
            .my(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg2))
            .child(SharedString::from(i18n::t!("fleet.new_task")))
            .debug_selector(|| "fleet-add-task".to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.open_new_task(window, cx);
                    // 親（サイドバー）の mouse-down は `control_focus` へフォーカスを移す。
                    // 止めないと、いま入力欄へ当てたフォーカスを同じクリックの泡立ちで奪い返され、
                    // ダイアログは出ているのに打鍵がサイドバーへ流れる（⌘N だと起きない）。
                    cx.stop_propagation();
                }),
            );

        div()
            .key_context("FleetControl")
            .track_focus(&self.chrome.control_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.chrome.control_focus, cx)),
            )
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_captain_bar(cx))
            .child(self.render_stage_attention(cx))
            .child(body)
            .child(add_task)
            .child(legend)
            .child(self.left_dock_resize_handle(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O21: necoder の外で作った worktree（`task/` でないブランチ）も Fleet に出て、取り込めば Task になる。
    /// 統合先はメインの作業ツリーのまま。外で消された worktree は「消えています」になり、片付けられる。
    #[gpui::test]
    fn external_worktrees_show_up_and_can_be_adopted(cx: &mut gpui::TestAppContext) {
        let base = std::env::temp_dir().join(format!(
            "necoder_fleet_external_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        // 最初から正規化した綴りで扱う。macOS の temp_dir は /var → /private/var のリンクで、
        // 外で消した後の worktree はもう正規化できず元の綴りのまま比べることになる（レールは正規化済み）。
        let base = paths::canonicalize(&base).unwrap();
        let main = base.join("repo");
        let external = base.join("orca-made");
        std::fs::create_dir_all(&main).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git")
                .current_dir(dir)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
        };
        if !git(&main, &["init", "-q", "-b", "main"]).is_ok_and(|output| output.status.success()) {
            return; // git が無い環境
        }
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "-A"]).unwrap();
        git(&main, &["commit", "-qm", "base"]).unwrap();
        // necoder の外（Orca / Claude Code / 手）で作った worktree。ブランチは task/ で始まらない。
        let added = git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature/orca",
                external.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert!(added.status.success(), "{added:?}");
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(vec![main.clone()], Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.refresh_fleet_worktrees(cx);
        });
        cx.run_until_parked();
        let canonical =
            |path: &Path| paths::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let listed = workspace.read_with(cx, |workspace, _| workspace.external_worktrees());
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(canonical(&listed[0].0), canonical(&external));
        assert_eq!(listed[0].1.as_deref(), Some("feature/orca"));
        assert_eq!(listed[0].2, None, "まだレールに無い");

        // 取り込む → レールに開いて Task。統合先はメインの作業ツリーのまま。
        workspace.update_in(cx, |workspace, _window, cx| {
            let (path, branch, rail) = listed[0].clone();
            workspace.adopt_worktree(path, branch, rail, cx);
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            let adopted = workspace
                .project_sessions
                .projects
                .iter()
                .position(|slot| canonical(slot.worktree.root()) == canonical(&external))
                .expect("レールに開いた");
            assert_eq!(
                workspace.project_sessions.projects[adopted].task_space.kind,
                SpaceKind::Task,
                "task/ でないブランチでも Task"
            );
            assert!(
                workspace.project_sessions.projects[adopted]
                    .task_space
                    .linked
            );
            workspace.switch_project(0, window, cx);
            let key = workspace.fleet_repository_key().expect("リポジトリ");
            assert_eq!(
                workspace.integration_slot_for(&key),
                Some(0),
                "統合先はメイン"
            );
            let (integration, rows, _) = workspace.fleet_sidebar_rows(cx);
            assert_eq!(integration.map(|(index, ..)| index), Some(0));
            assert!(rows.iter().any(|row| row.project_index == adopted));
            workspace.refresh_fleet_worktrees(cx);
        });
        cx.run_until_parked();
        assert!(
            workspace.read_with(cx, |workspace, _| workspace.external_worktrees().is_empty()),
            "取り込んだ worktree は外部の一覧から消える"
        );

        // 外で消す → 「消えています」→ 片付け。
        let removed = git(
            &main,
            &["worktree", "remove", "--force", external.to_str().unwrap()],
        )
        .unwrap();
        assert!(removed.status.success(), "{removed:?}");
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.forget_fleet_worktrees();
            workspace.refresh_fleet_worktrees(cx);
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            let adopted = workspace
                .project_sessions
                .projects
                .iter()
                .position(|slot| canonical(slot.worktree.root()) == canonical(&external))
                .expect("まだレールにある");
            assert!(workspace.vanished_worktree(adopted), "外で消えた");
            assert!(!workspace.vanished_worktree(0), "メインは残っている");
            workspace.forget_vanished_task(adopted, window, cx);
            assert_eq!(
                workspace.project_sessions.projects.len(),
                1,
                "片付けでレールから外れる"
            );
        });
        let _ = std::fs::remove_dir_all(&base);
    }

    /// F1 受入: サイドバーは**レールで選んでいる 1 リポジトリの Task だけ**を 3 段で出し、
    fn task_row(title: &str, branch: &str, asked: Option<&str>, digest: Option<&str>) -> TaskRow {
        TaskRow {
            project_index: 0,
            title: SharedString::from(title.to_string()),
            color: gpui::red(),
            branch: Some(SharedString::from(branch.to_string())),
            activity: agent_panel::ThreadActivity::Idle,
            asked: asked.map(|text| SharedString::from(text.to_string())),
            digest: digest.map(|text| SharedString::from(text.to_string())),
            tokens: 0,
            rail_shortcut: None,
            last_input_at_ms: None,
        }
    }

    /// Task の並べ方（O21）: 最近 = 依頼の新しい順（無い物は後ろ）・要対応 = 待ち → 作業中 → 静か。
    /// 同じ鍵の間はレールの順のまま。
    #[test]
    fn task_rows_sort_by_recent_or_attention() {
        use agent_panel::ThreadActivity;
        let row = |title: &str, at: Option<i64>, activity: ThreadActivity| {
            let mut row = task_row(title, "b", None, None);
            row.last_input_at_ms = at;
            row.activity = activity;
            row
        };
        let rows = || {
            vec![
                row("a", Some(10), ThreadActivity::Idle),
                row("b", None, ThreadActivity::Blocked),
                row("c", Some(30), ThreadActivity::Working),
                row("d", Some(20), ThreadActivity::Blocked),
            ]
        };
        let titles = |rows: &[TaskRow]| {
            rows.iter()
                .map(|row| row.title.to_string())
                .collect::<Vec<_>>()
        };
        let mut sorted = rows();
        sort_task_rows(&mut sorted, FleetSort::Rail);
        assert_eq!(titles(&sorted), ["a", "b", "c", "d"]);
        sort_task_rows(&mut sorted, FleetSort::Recent);
        assert_eq!(titles(&sorted), ["c", "d", "a", "b"]);
        let mut sorted = rows();
        sort_task_rows(&mut sorted, FleetSort::Attention);
        assert_eq!(titles(&sorted), ["b", "d", "c", "a"]);
        assert_eq!(FleetSort::Attention.next(), FleetSort::Rail);
    }

    /// Task の絞り込み（O21）: 名前・ブランチ・頼んだこと・いま何を のどれかに全部の語が要る。
    /// O21: ⌘ クリックで Task を選び、⇧ クリックで範囲。まとめて舞台に並べる（3 本まで）・片付けの
    /// 画面へ印を付けて渡す。どれも済んだら選択は外れる。
    #[gpui::test]
    fn several_tasks_can_be_selected_and_handled_together(cx: &mut gpui::TestAppContext) {
        let base =
            std::env::temp_dir().join(format!("necoder_fleet_selection_{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        let folders: Vec<PathBuf> = ["main", "a", "b", "c", "d"]
            .iter()
            .map(|name| base.join(name))
            .collect();
        for folder in &folders {
            std::fs::create_dir_all(folder).expect("作れる");
        }
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(folders.clone(), Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            for slot in &mut workspace.project_sessions.projects {
                slot.task_space.repository_id = "repo".to_string();
            }
            for index in 1..=4 {
                workspace.project_sessions.projects[index].task_space.kind = SpaceKind::Task;
            }
            workspace.switch_project(0, window, cx);
            let ids: Vec<SpaceId> = workspace
                .project_sessions
                .projects
                .iter()
                .map(|slot| slot.task_space.id.clone())
                .collect();
            let order = &ids[1..];

            workspace.toggle_fleet_selection(ids[1].clone(), cx);
            workspace.toggle_fleet_selection(ids[3].clone(), cx);
            assert_eq!(
                workspace.chrome.fleet_selection,
                [ids[1].clone(), ids[3].clone()]
            );
            workspace.toggle_fleet_selection(ids[1].clone(), cx);
            assert_eq!(
                workspace.chrome.fleet_selection,
                [ids[3].clone()],
                "もう一度で外れる"
            );
            // ⇧ は起点（最後に押した行）から押した行まで、見えている並びで。
            workspace.select_fleet_range(order, ids[4].clone(), cx);
            assert_eq!(workspace.chrome.fleet_selection, ids[1..=4].to_vec());
            // 選んでいる間はサイドバーにまとめての操作が出る（描けること）。
            workspace.render_fleet_sidebar(cx);

            workspace.stage_selected_tasks(&ids[1..=4], cx);
            assert_eq!(
                workspace.chrome.stage_pinned,
                ids[1..=3].to_vec(),
                "舞台は 3 枚まで"
            );
            assert_eq!(workspace.chrome.stage_columns, 3);
            assert!(
                workspace.chrome.fleet_selection.is_empty(),
                "済んだら外れる"
            );

            workspace.cleanup_selected_tasks(&[ids[2].clone(), ids[4].clone()], window, cx);
            let marked = workspace.cleanup_selection().expect("片付けの画面が開く");
            assert_eq!(
                marked,
                [ids[2].clone(), ids[4].clone()].into_iter().collect(),
                "選んだ Task に印が付いている"
            );

            // 普通のクリックに当たる操作で外れる。
            workspace.toggle_fleet_selection(ids[2].clone(), cx);
            workspace.clear_fleet_selection(cx);
            assert!(workspace.chrome.fleet_selection.is_empty());
            assert!(workspace.chrome.fleet_selection_anchor.is_none());
        });
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn task_rows_are_filtered_by_every_word() {
        let row = task_row(
            "ログイン修正",
            "task/login-fix",
            Some("OAuth の期限切れを直して"),
            Some("テストを流しています"),
        );
        assert!(task_row_matches(&row, ""), "語が無ければ全部当たる");
        assert!(task_row_matches(&row, "   "));
        assert!(task_row_matches(&row, "ログイン"));
        assert!(task_row_matches(&row, "LOGIN-FIX"), "大文字小文字は無視");
        assert!(
            task_row_matches(&row, "oauth テスト"),
            "語は別の欄に在ってよい"
        );
        assert!(!task_row_matches(&row, "oauth 決済"), "全部の語が要る");
    }

    /// レールを切り替えたら編隊ごと入れ替わる。統合先は Task 行に混ぜず別行にする。
    #[gpui::test]
    fn task_rows_are_scoped_to_the_selected_repository(cx: &mut gpui::TestAppContext) {
        let base = std::env::temp_dir().join(format!(
            "necoder_fleet_sidebar_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // a = 統合先 / a1..a3 = その worktree（Task）/ b = よそのリポジトリ。
        let roots: Vec<PathBuf> = ["a", "a1", "a2", "a3", "b"]
            .iter()
            .map(|name| base.join(name))
            .collect();
        for root in &roots {
            std::fs::create_dir_all(root).unwrap();
        }
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) =
            cx.add_window_view(|_, cx| Workspace::new(roots.clone(), Theme::dark(), None, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            // a1..a3 を a の Task にする（同じ repository_id で束ねるのが編隊の単位）。
            let key = "repo-a".to_string();
            workspace.project_sessions.projects[0]
                .task_space
                .repository_id = key.clone();
            for index in 1..=3 {
                let slot = &mut workspace.project_sessions.projects[index];
                slot.task_space.kind = SpaceKind::Task;
                slot.task_space.repository_id = key.clone();
                slot.task_space.title = SharedString::from(format!("task{index}"));
                slot.branch = Some(format!("task/{index}"));
                slot.task_space.result_summary =
                    Some(SharedString::from(format!("いま何を {index}")));
            }
            // b もそれ自身の統合先（別の鍵）。ここの Task が a の一覧に混ざってはいけない。
            workspace.project_sessions.projects[4]
                .task_space
                .repository_id = "repo-b".to_string();

            workspace.switch_project(0, window, cx); // レール = a
            let (integration, rows, _) = workspace.fleet_sidebar_rows(cx);
            let (integration_index, ..) = integration.expect("統合先行が出る");
            assert_eq!(integration_index, 0, "統合先は Task 行に混ぜない");
            assert_eq!(rows.len(), 3, "このリポジトリの Task だけが 3 行");
            for (offset, row) in rows.iter().enumerate() {
                let index = offset + 1;
                assert_eq!(row.title.as_ref(), format!("task{index}"));
                assert_eq!(
                    row.digest.as_deref(),
                    Some(format!("いま何を {index}").as_str()),
                    "3 段目 = いま何を / どう終わったか"
                );
                // ⌘N は**レールの並び**（`ActivateProjectN`）＝行の並びではない（嘘をつかない）。
                assert_eq!(row.rail_shortcut, Some(index + 1));
            }

            // アーカイブ済みは消える（既存の規律）。
            workspace.project_sessions.projects[3].task_space.phase = TaskPhase::Archived;
            assert_eq!(workspace.fleet_sidebar_rows(cx).1.len(), 2);

            // レール切替 = 編隊ごと切り替わる（b には Task が無いので 0 行 + 統合先だけ）。
            workspace.switch_project(4, window, cx);
            let (integration, rows, _) = workspace.fleet_sidebar_rows(cx);
            assert_eq!(integration.map(|(index, ..)| index), Some(4));
            assert!(rows.is_empty(), "よそのリポジトリの Task は混ざらない");

            // 絞り込み欄に打つと語の写しが変わり、行はそれで絞られる（O21）。
            workspace.switch_project(0, window, cx);
            workspace
                .chrome
                .fleet_filter
                .update(cx, |filter, cx| filter.set_plain_text("task/2", cx));
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            assert_eq!(workspace.chrome.fleet_filter_query, "task/2");
            let (_, rows, _) = workspace.fleet_sidebar_rows(cx);
            let shown: Vec<String> = rows
                .iter()
                .filter(|row| task_row_matches(row, &workspace.chrome.fleet_filter_query))
                .map(|row| row.title.to_string())
                .collect();
            assert_eq!(shown, ["task2"]);
        });
    }
}
