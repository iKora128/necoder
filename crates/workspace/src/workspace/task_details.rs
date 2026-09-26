//! Task の詳細（O21・A12）。レール（Fleet サイドバーの Task 行も同じメニュー）の右クリック「詳細…」で、
//! Task の事実を 1 枚にまとめて見せる: 種類と状態・ブランチ・起点・場所・作った時・スレッド・最後の依頼・
//! トークン。パスとブランチ名はここからもコピーできる。読むだけ（変えるのは今までの所から）。

use crate::workspace::*;

/// 詳細の 1 行（見出し・値）。値が長ければ折り返す。
pub(crate) struct DetailRow {
    pub(crate) label: String,
    pub(crate) value: String,
}

/// ホームの下なら `~/…` にする（詳細は読むためのもの・長いパスを短く）。
fn tilde_path(path: &Path) -> String {
    match paths::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(relative) if !relative.as_os_str().is_empty() => {
            format!("~/{}", relative.display())
        }
        _ => path.display().to_string(),
    }
}

impl Workspace {
    /// 右クリックの「詳細…」。メニューは閉じる。
    pub(crate) fn show_task_details(&mut self, index: usize, cx: &mut Context<Self>) {
        self.overlays.rail_menu = None;
        self.chrome.task_details = Some(index);
        cx.notify();
    }

    pub(crate) fn hide_task_details(&mut self, cx: &mut Context<Self>) {
        if self.chrome.task_details.take().is_some() {
            cx.notify();
        }
    }

    /// 詳細の行（描画とテストで共有）。`index` の project が無ければ `None`。
    pub(crate) fn task_detail_rows(&self, index: usize, cx: &App) -> Option<Vec<DetailRow>> {
        let slot = self.project_sessions.projects.get(index)?;
        let space = &slot.task_space;
        let statuses = self
            .project_sessions
            .sessions
            .get(index)
            .map(|session| session.agent_statuses(cx))
            .unwrap_or_default();
        let dash = || "—".to_string();
        let kind = if space.is_integration() {
            i18n::t!("fleet.details_kind_integration")
        } else {
            i18n::t!("fleet.details_kind_task")
        };
        let branch = slot
            .branch
            .clone()
            .or_else(|| slot.worktree_branch.clone())
            .unwrap_or_else(dash);
        let base = space
            .base_oid
            .as_deref()
            .map(|oid| oid.chars().take(8).collect())
            .unwrap_or_else(dash);
        let threads = if statuses.is_empty() {
            dash()
        } else {
            let agents: Vec<String> = statuses
                .iter()
                .map(|(_, _, status)| status.agent.to_string())
                .collect();
            format!("{}（{}）", statuses.len(), agents.join("・"))
        };
        // 最後の依頼 = いちばん最近に打ったスレッドの原文（1 行目・長ければ切る）。
        let asked = statuses
            .iter()
            .filter_map(|(_, _, status)| {
                Some((status.last_input_at_ms?, status.last_prompt.clone()?))
            })
            .max_by_key(|(at, _)| *at)
            .map(|(_, prompt)| {
                let first = prompt.lines().next().unwrap_or_default();
                let clipped: String = first.chars().take(160).collect();
                if clipped.len() < first.len() {
                    format!("{clipped}…")
                } else {
                    clipped
                }
            })
            .unwrap_or_else(dash);
        let tokens: u32 = statuses
            .iter()
            .map(|(_, _, status)| status.tokens_used)
            .sum();
        let row = |key: &str, value: String| DetailRow {
            label: i18n::t!(key),
            value,
        };
        Some(vec![
            row(
                "fleet.details_state",
                format!("{kind} · {}", super::cleanup::phase_label(space.phase)),
            ),
            row("fleet.details_branch", branch),
            row("fleet.details_base", base),
            row("fleet.details_path", tilde_path(slot.worktree.root())),
            row(
                "fleet.details_created",
                agent_panel::relative_time_label(space.created_at_ms).to_string(),
            ),
            row("fleet.details_threads", threads),
            row("fleet.details_asked", asked),
            row(
                "fleet.details_tokens",
                if tokens == 0 {
                    dash()
                } else {
                    agent_panel::human_tokens(tokens).to_string()
                },
            ),
        ])
    }

    /// 詳細のダイアログ（中央・幅 420）。外側か「閉じる」で閉じる。
    pub(crate) fn render_task_details(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let index = self.chrome.task_details?;
        let slot = self.project_sessions.projects.get(index)?;
        let rows = self.task_detail_rows(index, cx)?;
        let theme = self.theme.clone();
        let color = slot.color;
        let title = slot.task_space.title.clone();
        let path = slot.worktree.root().display().to_string();
        let branch = slot.branch.clone().or_else(|| slot.worktree_branch.clone());
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .px(px(11.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme.border)
                .text_size(px(11.5))
                .text_color(theme.fg1)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(SharedString::from(label))
        };
        let card = div()
            .w(px(420.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(16.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(12.),
                gpui::hsla(0., 0., 0., 0.5),
            )
            .blur_radius(px(28.))])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_none().size(px(9.)).rounded_full().bg(color))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .text_size(px(12.))
                    .children(rows.into_iter().map(|row| {
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(96.))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(row.label)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(theme.fg1)
                                    .child(SharedString::from(row.value)),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("task-details-copy-path", i18n::t!("rail.menu_copy_path"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.copy_from_rail_menu(path.clone(), cx)
                                }),
                            ),
                    )
                    .when_some(branch, |row, branch| {
                        row.child(
                            button(
                                "task-details-copy-branch",
                                i18n::t!("rail.menu_copy_branch"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.copy_from_rail_menu(branch.clone(), cx)
                                }),
                            ),
                        )
                    })
                    .child(
                        button("task-details-close", i18n::t!("fleet.details_close"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| this.hide_task_details(cx)),
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
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.25))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_task_details(cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn details_show_the_facts_of_a_project(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("necoder_task_details_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = paths::canonicalize(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            let rows = workspace.task_detail_rows(0, cx).expect("詳細");
            let value = |key: &str| {
                let label = i18n::t!(key);
                rows.iter()
                    .find(|row| row.label == label)
                    .map(|row| row.value.clone())
                    .unwrap_or_default()
            };
            assert!(
                value("fleet.details_path")
                    .ends_with(root.file_name().unwrap().to_string_lossy().as_ref()),
                "{}",
                value("fleet.details_path")
            );
            assert_eq!(value("fleet.details_base"), "—", "起点を持たない統合先");
            assert_eq!(value("fleet.details_asked"), "—");
            assert!(workspace.task_detail_rows(9, cx).is_none());

            workspace.show_task_details(0, cx);
            assert_eq!(workspace.chrome.task_details, Some(0));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            workspace.hide_task_details(cx);
            assert_eq!(workspace.chrome.task_details, None);
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
