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
}

impl Workspace {
    /// いま編隊として見ているリポジトリの鍵（レールで選んでいる slot のもの）。
    fn fleet_repository_key(&self) -> Option<String> {
        self.active_slot()
            .map(|slot| slot.repository_key().to_string())
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
        let Some(index) = self
            .project_sessions
            .projects
            .iter()
            .position(|slot| slot.repository_key() == key && slot.task_space.is_integration())
        else {
            return;
        };
        let Some(agent) = settings::get(cx).captain_agent.clone() else {
            // 未任命なら設定ホームへ（「任命する」の行き先は設定・§5.7）。
            self.open_settings_action(&OpenSettings, window, cx);
            return;
        };
        let panel = self.project_sessions.sessions[index].agent_panel.clone();
        let names = captain::captain_thread_names();
        let thread = panel.update(cx, |panel, cx| {
            let thread =
                panel.ensure_named_thread(&captain::captain_thread_name(), &names, &agent, cx);
            panel.focus_thread(thread, cx);
            thread
        });
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
                integration = Some((index, slot.name.clone(), slot.color, branch));
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
            });
        }
        (integration, rows, integrated_today)
    }

    /// Fleet サイドバー本体。
    pub(crate) fn render_fleet_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use agent_panel::ThreadActivity;
        let theme = self.theme.clone();
        let (integration, rows, integrated_today) = self.fleet_sidebar_rows(cx);

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
                                i18n::t!("fleet.tasks_count", "n" => rows.len()),
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
                    .min_h(px(38.))
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
                                        "branched" => rows.len(),
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
                            this.switch_project(index, window, cx);
                        }),
                    ),
            );
        }

        // ③ Task 行（3 段固定・§3.2-3）。
        for (seq, row) in rows.iter().enumerate() {
            let project_index = row.project_index;
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
                    .min_h(px(54.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    this.add_worktree_agent(cx);
                }),
            );

        div()
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

    /// F1 受入: サイドバーは**レールで選んでいる 1 リポジトリの Task だけ**を 3 段で出し、
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
        });
    }
}
