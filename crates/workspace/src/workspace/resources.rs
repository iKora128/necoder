//! リソース（O22 の Resource Manager・手元の分）: necoder 本体と子プロセスのメモリを、
//! プロジェクト / Task ごとに見る。
//!
//! エージェントは 1 本で 350〜700 MB を握り、スレッドを開いたままにすると積み上がる（`agent_panel::idle`）。
//! どのプロジェクトが重いかを見て、その場で「使っていないエージェントを止める」（会話は残り、次の送信で
//! 続きから）ための画面。読むのは**開いた時と ↻ の時だけ**（`ps` 1 回 + 作業フォルダの `lsof` 1 回・常駐しない）。
//!
//! - 数えるのは necoder の子孫プロセス。necoder の**直接の子**（エージェントのラッパー・端末のシェル・
//!   LSP 等）ごとに、その子孫を含めた RSS の合計を 1 行にする。
//! - 行の割り当ては直接の子の作業フォルダ（いちばん深いプロジェクトのルート・Ports と同じ）。どこにも
//!   入らなければ「プロジェクトの外」。
//! - 止めるのはエージェントだけで、止め方は idle の回収弁と同じ（静かで会話を引き継げるスレッドだけ・
//!   プロセスへシグナルは送らない）。端末や開発サーバはここでは止めない（開発サーバは Ports で）。
//! - SSH 先のプロセスと Windows は未対応。

use super::ports::{parse_lsof_cwd, project_for_cwd};
use crate::workspace::*;
use std::collections::{BTreeMap, HashMap};

/// `ps` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessInfo {
    pub(crate) pid: u32,
    pub(crate) parent: u32,
    pub(crate) rss_kb: u64,
    pub(crate) command: String,
}

/// necoder の直接の子 1 本分（子孫を含む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessGroup {
    pub(crate) pid: u32,
    /// 直接の子のコマンド（パスの末尾）。
    pub(crate) command: String,
    /// 子孫を含めた RSS の合計（KiB）。
    pub(crate) rss_kb: u64,
    /// 子孫を含めたプロセスの数。
    pub(crate) processes: usize,
    /// 作業フォルダが入っているレールの添字。
    pub(crate) project: Option<usize>,
}

/// 1 回読んだ結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceSnapshot {
    /// necoder 本体の RSS（KiB）。
    pub(crate) own_rss_kb: u64,
    /// 重い順。
    pub(crate) groups: Vec<ProcessGroup>,
}

/// リソースの画面の状態（開いている間だけ Some）。
pub(crate) struct ResourcesState {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    /// `None` = 読んでいる途中。
    snapshot: Option<Result<ResourceSnapshot, SharedString>>,
}

/// 空白で区切られた次の欄と残り。
fn next_field(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    Some(match text.find(char::is_whitespace) {
        Some(end) => (&text[..end], &text[end..]),
        None => (text, ""),
    })
}

/// `ps -A -o pid=,ppid=,rss=,comm=` を読む（純関数）。コマンドは空白を含みうる（mac はフルパス）ので
/// 3 つの数の後ろを丸ごと取る。
pub(crate) fn parse_ps_resources(text: &str) -> Vec<ProcessInfo> {
    text.lines()
        .filter_map(|line| {
            let (pid, rest) = next_field(line)?;
            let (parent, rest) = next_field(rest)?;
            let (rss, rest) = next_field(rest)?;
            Some(ProcessInfo {
                pid: pid.parse().ok()?,
                parent: parent.parse().ok()?,
                rss_kb: rss.parse().ok()?,
                command: rest.trim().to_string(),
            })
        })
        .collect()
}

/// `pid` の祖先のうち `own_pid` の直接の子（自分が直接の子なら自分）。子孫でなければ `None`。
fn top_child(pid: u32, own_pid: u32, parents: &HashMap<u32, u32>) -> Option<u32> {
    let mut current = pid;
    for _ in 0..256 {
        let parent = *parents.get(&current)?;
        if parent == own_pid {
            return Some(current);
        }
        if parent == current || parent == 0 {
            return None;
        }
        current = parent;
    }
    None
}

/// コマンドのパスの末尾（`/usr/bin/node` → `node`）。
fn command_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.to_string())
}

/// necoder の直接の子ごとにまとめる（純関数）。割り当て（`project`）は呼び手が埋める。
pub(crate) fn group_by_child(own_pid: u32, processes: &[ProcessInfo]) -> ResourceSnapshot {
    let parents: HashMap<u32, u32> = processes
        .iter()
        .map(|process| (process.pid, process.parent))
        .collect();
    let own_rss_kb = processes
        .iter()
        .find(|process| process.pid == own_pid)
        .map(|process| process.rss_kb)
        .unwrap_or(0);
    let mut groups: BTreeMap<u32, ProcessGroup> = BTreeMap::new();
    for process in processes {
        let Some(top) = top_child(process.pid, own_pid, &parents) else {
            continue;
        };
        let group = groups.entry(top).or_insert_with(|| ProcessGroup {
            pid: top,
            command: processes
                .iter()
                .find(|candidate| candidate.pid == top)
                .map(|candidate| command_name(&candidate.command))
                .unwrap_or_default(),
            rss_kb: 0,
            processes: 0,
            project: None,
        });
        group.rss_kb += process.rss_kb;
        group.processes += 1;
    }
    let mut groups: Vec<ProcessGroup> = groups.into_values().collect();
    groups.sort_by(|left, right| {
        right
            .rss_kb
            .cmp(&left.rss_kb)
            .then(left.pid.cmp(&right.pid))
    });
    ResourceSnapshot { own_rss_kb, groups }
}

/// KiB を「291 MB」「1.4 GB」へ。
pub(crate) fn format_memory(kib: u64) -> String {
    let mib = kib as f64 / 1024.0;
    if mib < 1024.0 {
        format!("{} MB", mib.round() as u64)
    } else {
        format!("{:.1} GB", mib / 1024.0)
    }
}

/// 手元のプロセスを読む（背景で呼ぶ）。`own_pid` = necoder 自身。
fn collect_resources(own_pid: u32, roots: &[PathBuf]) -> Result<ResourceSnapshot, String> {
    let run = |program: &str, args: &[&str]| -> Result<String, String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("{program}: {error}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let processes = parse_ps_resources(&run("ps", &["-A", "-o", "pid=,ppid=,rss=,comm="])?);
    let mut snapshot = group_by_child(own_pid, &processes);
    if !snapshot.groups.is_empty() {
        let pids = snapshot
            .groups
            .iter()
            .map(|group| group.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        // lsof が無い・読めない時は割り当てずに出す（数字は見える）。
        let cwds = run("lsof", &["-a", "-d", "cwd", "-p", &pids, "-Fpn"])
            .map(|text| parse_lsof_cwd(&text))
            .unwrap_or_default();
        for group in &mut snapshot.groups {
            group.project = cwds
                .get(&group.pid)
                .and_then(|cwd| project_for_cwd(cwd, roots));
        }
    }
    Ok(snapshot)
}

impl Workspace {
    /// パレット「リソース: メモリとプロセス」。開いた時に 1 回読む。
    pub(crate) fn show_resources(
        &mut self,
        _: &ShowResources,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.resources = Some(ResourcesState {
            focus,
            previous_focus,
            snapshot: None,
        });
        self.refresh_resources(cx);
        cx.notify();
    }

    /// 読み直す（開いた時・↻・止めた後）。
    pub(crate) fn refresh_resources(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.resources.as_mut() else {
            return;
        };
        if cfg!(target_os = "windows") {
            state.snapshot = Some(Err(i18n::t!("resources.unsupported").into()));
            cx.notify();
            return;
        }
        state.snapshot = None;
        let roots: Vec<PathBuf> = self
            .project_sessions
            .projects
            .iter()
            .map(|slot| {
                if slot.worktree.is_remote() {
                    // SSH 先のプロジェクトは手元のプロセスの作業フォルダと突き合わせない。
                    PathBuf::from("\0remote")
                } else {
                    paths::canonicalize(slot.worktree.root())
                        .unwrap_or_else(|_| slot.worktree.root().to_path_buf())
                }
            })
            .collect();
        let own_pid = std::process::id();
        cx.spawn(async move |workspace, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move { collect_resources(own_pid, &roots) })
                .await;
            // Err = 読んでいる間に窓が閉じた。
            workspace
                .update(cx, |workspace, cx| {
                    if let Some(state) = workspace.overlays.resources.as_mut() {
                        state.snapshot = Some(snapshot.map_err(SharedString::from));
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    fn close_resources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.resources.take() else {
            return;
        };
        if let Some(previous) = state.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    /// その session で今止められるエージェントの数（全パネル）。
    pub(crate) fn stoppable_agents_in(&self, session_index: usize, cx: &App) -> usize {
        self.project_sessions
            .sessions
            .get(session_index)
            .map(|session| {
                session
                    .fleet_agents
                    .iter()
                    .map(|panel| panel.read(cx).stoppable_agent_count())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// 「使っていないエージェントを止める」: その session の全パネルで、静かで会話を引き継げる
    /// スレッドのエージェントを止める（会話は残る）。止まるのを待ってから読み直す。
    pub(crate) fn stop_quiet_agents_in(&mut self, session_index: usize, cx: &mut Context<Self>) {
        let Some(panels) = self
            .project_sessions
            .sessions
            .get(session_index)
            .map(|session| session.fleet_agents.clone())
        else {
            return;
        };
        let stopped: usize = panels
            .iter()
            .map(|panel| panel.update(cx, |panel, cx| panel.stop_quiet_agents(cx)))
            .sum();
        let color = self.accent();
        self.push_toast(
            i18n::t!("resources.stopped", "count" => stopped).into(),
            color,
            cx,
        );
        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(800))
                .await;
            workspace
                .update(cx, |workspace, cx| workspace.refresh_resources(cx))
                .ok();
        })
        .detach();
    }

    /// 中央のモーダル（幅 560）。上に本体と子の合計、プロジェクトごとの子、最後にプロジェクトの外。
    pub(crate) fn render_resources(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.resources.as_ref()?;
        let theme = self.theme.clone();
        let mut body = div().flex().flex_col().gap(px(10.));
        match &state.snapshot {
            None => {
                body = body.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("resources.loading"))),
                )
            }
            Some(Err(error)) => {
                body = body.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.err)
                        .child(error.clone()),
                )
            }
            Some(Ok(snapshot)) => {
                let children_kb: u64 = snapshot.groups.iter().map(|group| group.rss_kb).sum();
                body = body.child(div().text_size(px(11.5)).text_color(theme.fg1).child(
                    SharedString::from(i18n::t!(
                        "resources.summary",
                        "own" => format_memory(snapshot.own_rss_kb),
                        "children" => format_memory(children_kb),
                        "count" => snapshot.groups.len()
                    )),
                ));
                for (index, slot) in self.project_sessions.projects.iter().enumerate() {
                    let groups: Vec<&ProcessGroup> = snapshot
                        .groups
                        .iter()
                        .filter(|group| group.project == Some(index))
                        .collect();
                    let stoppable = self.stoppable_agents_in(index, cx);
                    if groups.is_empty() && stoppable == 0 {
                        continue;
                    }
                    body = body.child(self.render_resource_section(
                        index,
                        Some((slot.color, slot.task_space.title.clone())),
                        &groups,
                        stoppable,
                        cx,
                    ));
                }
                let outside: Vec<&ProcessGroup> = snapshot
                    .groups
                    .iter()
                    .filter(|group| group.project.is_none())
                    .collect();
                if !outside.is_empty() {
                    body =
                        body.child(self.render_resource_section(usize::MAX, None, &outside, 0, cx));
                }
            }
        }
        let card = div()
            .w(px(560.))
            .max_h(px(520.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(16.))
            .rounded(px(10.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .track_focus(&state.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_resources(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(i18n::t!("resources.title"))),
                    )
                    .child(
                        div()
                            .id("resources-refresh")
                            .px(px(6.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3))
                            .child("↻")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.refresh_resources(cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("resources-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("resources.hint"))),
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
                .bg(gpui::hsla(0., 0., 0., 0.35))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_resources(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }

    /// 1 プロジェクト分（`place = None` はプロジェクトの外）。見出しに合計と「止める」。
    fn render_resource_section(
        &self,
        session_index: usize,
        place: Option<(Hsla, SharedString)>,
        groups: &[&ProcessGroup],
        stoppable: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let total: u64 = groups.iter().map(|group| group.rss_kb).sum();
        let (dot, title) =
            place.unwrap_or((theme.fg2, SharedString::from(i18n::t!("resources.outside"))));
        let mut section = div().flex().flex_col().gap(px(2.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_none().size(px(8.)).rounded_full().bg(dot))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .text_color(theme.fg0)
                        .child(title),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .child(SharedString::from(format_memory(total))),
                )
                .when(stoppable > 0, |header| {
                    header.child(
                        div()
                            .id(("resources-stop", session_index))
                            .flex_none()
                            .px(px(8.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .child(SharedString::from(
                                i18n::t!("resources.stop_quiet", "count" => stoppable),
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.stop_quiet_agents_in(session_index, cx);
                                }),
                            ),
                    )
                }),
        );
        for group in groups {
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(16.))
                    .min_h(px(22.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .child(SharedString::from(i18n::t!(
                                "resources.process",
                                "command" => group.command.as_str(),
                                "pid" => group.pid,
                                "count" => group.processes
                            ))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family("Guguru Sans Code")
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .child(SharedString::from(format_memory(group.rss_kb))),
                    ),
            );
        }
        section.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_lines_keep_commands_with_spaces() {
        let processes = parse_ps_resources(
            "  100     1 297000 /Applications/necoder.app/Contents/MacOS/necoder\n  \
             200   100  48000 /opt/homebrew/bin/node\n  \
             300   200 400000 /Users/me/Library/Application Support/claude/claude\n\
             garbage\n",
        );
        assert_eq!(processes.len(), 3);
        assert_eq!(
            processes[2].command,
            "/Users/me/Library/Application Support/claude/claude"
        );
        assert_eq!(processes[1].rss_kb, 48000);
    }

    #[test]
    fn descendants_are_summed_under_each_direct_child() {
        let processes = parse_ps_resources(
            "100 1 300000 necoder\n\
             200 100 48000 /usr/bin/node\n\
             201 200 65000 node\n\
             202 201 400000 claude\n\
             300 100 4000 /bin/zsh\n\
             301 300 90000 python3\n\
             400 1 999999 other-app\n\
             500 400 1000 other-child\n",
        );
        let snapshot = group_by_child(100, &processes);
        assert_eq!(snapshot.own_rss_kb, 300000);
        assert_eq!(
            snapshot
                .groups
                .iter()
                .map(|group| (
                    group.pid,
                    group.command.as_str(),
                    group.rss_kb,
                    group.processes
                ))
                .collect::<Vec<_>>(),
            vec![(200, "node", 513000, 3), (300, "zsh", 94000, 2)],
            "直接の子ごとに子孫を足す・重い順・ほかのアプリは数えない"
        );
    }

    #[test]
    fn memory_reads_in_megabytes_or_gigabytes() {
        assert_eq!(format_memory(297_000), "290 MB");
        assert_eq!(format_memory(1_536 * 1024), "1.5 GB");
        assert_eq!(format_memory(0), "0 MB");
    }

    /// 実プロセス: テストの子は直接の子として数えられ、作業フォルダのプロジェクトに付く。
    #[cfg(unix)]
    #[test]
    fn a_child_process_is_counted_under_its_project() {
        let root = std::env::temp_dir().join(format!("necoder_resources_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).expect("作業フォルダを作れる");
        let root = paths::canonicalize(&root).expect("正規化できる");
        let Ok(mut child) = std::process::Command::new("sleep")
            .current_dir(&root)
            .arg("30")
            .spawn()
        else {
            return; // sleep が無い環境
        };
        let snapshot = collect_resources(
            std::process::id(),
            &[PathBuf::from("/nowhere"), root.clone()],
        )
        .expect("読める");
        let group = snapshot
            .groups
            .iter()
            .find(|group| group.pid == child.id())
            .unwrap_or_else(|| panic!("子が一覧にある: {snapshot:?}"));
        assert_eq!(group.command, "sleep");
        assert!(group.rss_kb > 0);
        if std::process::Command::new("lsof")
            .arg("-v")
            .output()
            .is_ok()
        {
            assert_eq!(group.project, Some(1), "作業フォルダのプロジェクト");
        }
        assert!(snapshot.own_rss_kb > 0, "テストのプロセス自身");
        child.kill().ok();
        child.wait().ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
