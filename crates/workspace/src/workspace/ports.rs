//! Ports（O5・手元の分）: necoder の中で動いている開発サーバのポートを一覧し、開く / 止める。
//!
//! 読むのは**開いた時と ↻ の時だけ**（ポーリングしない・常駐しない）。手元の `lsof`（LISTEN 中の TCP）と
//! `ps`（親子関係）を 1 回ずつ、necoder の子孫プロセスの作業フォルダを `lsof -d cwd` で 1 回読み、
//! 作業フォルダが入っているプロジェクト / Task に割り当てる（エージェントが立てたサーバも、端末で
//! 立てたサーバも同じ扱い）。**止められるのは necoder の子孫だけ**（止める直前にもう一度確かめる）。
//! ほかのアプリのポートは「ほかのプロセス」に畳んで出す（開けるが止めない）。
//!
//! SSH 先のポート（転送）と Windows は未対応（O5 の続き）。

use crate::workspace::*;
use std::collections::HashMap;

/// LISTEN 中の TCP ポート 1 つ（`lsof -Fpcn` の 1 件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListeningPort {
    pub(crate) pid: u32,
    pub(crate) command: String,
    pub(crate) port: u16,
    /// 待ち受けているアドレス（`127.0.0.1` / `*` / `[::1]` 等）。
    pub(crate) address: String,
}

/// 一覧の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortRow {
    pub(crate) port: ListeningPort,
    /// necoder の子孫か（止められるか）。
    pub(crate) ours: bool,
    /// 作業フォルダが入っているレールの添字（無ければ `None`）。
    pub(crate) project: Option<usize>,
}

/// Ports の画面の状態（開いている間だけ Some）。
pub(crate) struct PortsState {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    /// `None` = 読んでいる途中。
    rows: Option<Result<Vec<PortRow>, SharedString>>,
    /// ほかのプロセスのポートを開いているか。
    show_others: bool,
}

/// `lsof -nP -iTCP -sTCP:LISTEN -Fpcn` を読む（純関数）。`p<pid>` / `c<command>` / `n<addr:port>`。
/// 同じポートを IPv4 と IPv6 の両方で待つプロセスは 1 行にまとめる。
pub(crate) fn parse_lsof_listen(text: &str) -> Vec<ListeningPort> {
    let mut ports: Vec<ListeningPort> = Vec::new();
    let mut pid = None;
    let mut command = String::new();
    for line in text.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => {
                pid = value.parse::<u32>().ok();
                command.clear();
            }
            "c" => command = value.to_string(),
            "n" => {
                let Some(pid) = pid else {
                    continue;
                };
                let Some((address, port)) = value.rsplit_once(':') else {
                    continue;
                };
                let Ok(port) = port.parse::<u16>() else {
                    continue;
                };
                if ports
                    .iter()
                    .any(|known| known.pid == pid && known.port == port)
                {
                    continue;
                }
                ports.push(ListeningPort {
                    pid,
                    command: command.clone(),
                    port,
                    address: address.to_string(),
                });
            }
            _ => {}
        }
    }
    ports.sort_by_key(|port| (port.port, port.pid));
    ports
}

/// `ps -A -o pid=,ppid=` を読む（純関数）。pid → 親の pid。
pub(crate) fn parse_ps_parents(text: &str) -> HashMap<u32, u32> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            Some((pid, parent))
        })
        .collect()
}

/// `lsof -a -d cwd -p <pids> -Fpn` を読む（純関数）。pid → 作業フォルダ。
pub(crate) fn parse_lsof_cwd(text: &str) -> HashMap<u32, PathBuf> {
    let mut cwds = HashMap::new();
    let mut pid = None;
    for line in text.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => pid = value.parse::<u32>().ok(),
            "n" => {
                if let Some(pid) = pid {
                    cwds.insert(pid, PathBuf::from(value));
                }
            }
            _ => {}
        }
    }
    cwds
}

/// `pid` が `ancestor` の子孫か（親を辿る・輪になっていても止まる）。
pub(crate) fn is_descendant(pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut current = pid;
    for _ in 0..256 {
        let Some(&parent) = parents.get(&current) else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        if parent == current || parent == 0 {
            return false;
        }
        current = parent;
    }
    false
}

/// 作業フォルダが入っているプロジェクト（いちばん深いルート）。`roots` はレールの順。
pub(crate) fn project_for_cwd(cwd: &Path, roots: &[PathBuf]) -> Option<usize> {
    roots
        .iter()
        .enumerate()
        .filter(|(_, root)| cwd.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(index, _)| index)
}

/// 手元のポートを集める（背景で呼ぶ）。`own_pid` = necoder 自身。
fn collect_ports(own_pid: u32, roots: &[PathBuf]) -> Result<Vec<PortRow>, String> {
    let run = |program: &str, args: &[&str]| -> Result<String, String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("{program}: {error}"))?;
        // lsof は該当が無いと 1 で終わる（中身は空）。空の一覧として扱う。
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let listening = parse_lsof_listen(&run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])?);
    let parents = parse_ps_parents(&run("ps", &["-A", "-o", "pid=,ppid="])?);
    let ours: Vec<u32> = listening
        .iter()
        .map(|port| port.pid)
        .filter(|pid| is_descendant(*pid, own_pid, &parents))
        .collect();
    let cwds = if ours.is_empty() {
        HashMap::new()
    } else {
        let pids = ours
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        parse_lsof_cwd(&run("lsof", &["-a", "-d", "cwd", "-p", &pids, "-Fpn"])?)
    };
    Ok(listening
        .into_iter()
        .map(|port| {
            let ours = ours.contains(&port.pid);
            let project = cwds
                .get(&port.pid)
                .and_then(|cwd| project_for_cwd(cwd, roots));
            PortRow {
                port,
                ours,
                project,
            }
        })
        .collect())
}

/// 止める直前の確かめ直し: まだ necoder の子孫か（pid の使い回しで別のプロセスを止めない）。
fn still_ours(pid: u32, own_pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .map(|output| {
            is_descendant(
                pid,
                own_pid,
                &parse_ps_parents(&String::from_utf8_lossy(&output.stdout)),
            )
        })
        .unwrap_or(false)
}

impl Workspace {
    /// パレット「Ports: 開いているポート」。開いた時に 1 回読む。
    pub(crate) fn show_ports(
        &mut self,
        _: &ShowPorts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.ports = Some(PortsState {
            focus,
            previous_focus,
            rows: None,
            show_others: false,
        });
        self.refresh_ports(cx);
        cx.notify();
    }

    /// 読み直す（開いた時・↻・止めた後）。
    pub(crate) fn refresh_ports(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.ports.as_mut() else {
            return;
        };
        if cfg!(target_os = "windows") {
            state.rows = Some(Err(i18n::t!("ports.unsupported").into()));
            cx.notify();
            return;
        }
        state.rows = None;
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
            let rows = cx
                .background_executor()
                .spawn(async move { collect_ports(own_pid, &roots) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if let Some(state) = workspace.overlays.ports.as_mut() {
                    state.rows = Some(rows.map_err(SharedString::from));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_ports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.overlays.ports.take() else {
            return;
        };
        if let Some(previous) = state.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    /// 開く: そのポートの localhost を Web タブで（プロジェクトが分かればそのプロジェクトから）。
    fn open_port(&mut self, row: &PortRow, window: &mut Window, cx: &mut Context<Self>) {
        let url = format!("http://localhost:{}/", row.port.port);
        let session = row.project.unwrap_or(self.project_sessions.active);
        self.close_ports(window, cx);
        self.open_url_from_session(session, &url, cx);
    }

    /// 止める: necoder の子孫であることを確かめ直してから SIGTERM。読み直す。
    fn stop_port(&mut self, row: &PortRow, cx: &mut Context<Self>) {
        if !row.ours {
            return;
        }
        let pid = row.port.pid;
        let own_pid = std::process::id();
        cx.spawn(async move |workspace, cx| {
            let stopped: Result<(), ()> = cx
                .background_executor()
                .spawn(async move {
                    if !still_ours(pid, own_pid) {
                        return Err(());
                    }
                    #[cfg(unix)]
                    {
                        let Ok(target) = libc::pid_t::try_from(pid) else {
                            return Err(());
                        };
                        // SAFETY: 確かめ直した necoder の子孫の pid へ SIGTERM を送るだけ。
                        let result = unsafe { libc::kill(target, libc::SIGTERM) };
                        if result == 0 {
                            Ok(())
                        } else {
                            Err(())
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        Err(())
                    }
                })
                .await;
            // 止まるまで少し待ってから読み直す（止まる前に読むと残って見える）。
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                if stopped.is_err() {
                    let color = workspace.accent();
                    workspace.push_toast(
                        i18n::t!("ports.stop_failed", "pid" => pid).into(),
                        color,
                        cx,
                    );
                }
                workspace.refresh_ports(cx);
            });
        })
        .detach();
    }

    /// 中央のモーダル（幅 520）。necoder の中のポートを行に、ほかのプロセスは畳んで出す。
    pub(crate) fn render_ports(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.ports.as_ref()?;
        let theme = self.theme.clone();
        let mut body = div().flex().flex_col().gap(px(2.));
        match &state.rows {
            None => {
                body = body.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("ports.loading"))),
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
            Some(Ok(rows)) => {
                let (ours, others): (Vec<&PortRow>, Vec<&PortRow>) =
                    rows.iter().partition(|row| row.ours);
                if ours.is_empty() {
                    body = body.child(
                        div()
                            .py(px(6.))
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("ports.empty"))),
                    );
                }
                for (index, row) in ours.into_iter().enumerate() {
                    body = body.child(self.render_port_row(("port-ours", index), row, true, cx));
                }
                if !others.is_empty() {
                    let open = state.show_others;
                    body = body.child(
                        div()
                            .id("ports-others")
                            .mt(px(8.))
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.fg1))
                            .child(SharedString::from(format!(
                                "{} {}",
                                if open { "▾" } else { "▸" },
                                i18n::t!("ports.others", "count" => others.len())
                            )))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(state) = this.overlays.ports.as_mut() {
                                        state.show_others = !state.show_others;
                                    }
                                    cx.notify();
                                }),
                            ),
                    );
                    if open {
                        for (index, row) in others.into_iter().enumerate() {
                            body = body.child(self.render_port_row(
                                ("port-other", index),
                                row,
                                false,
                                cx,
                            ));
                        }
                    }
                }
            }
        }
        let card = div()
            .w(px(520.))
            .max_h(px(480.))
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
                    this.close_ports(window, cx);
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
                            .child(SharedString::from(i18n::t!("ports.title"))),
                    )
                    .child(
                        div()
                            .id("ports-refresh")
                            .px(px(6.))
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(theme.fg1)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3))
                            .child("↻")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.refresh_ports(cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("ports-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("ports.hint"))),
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
                    cx.listener(|this, _, window, cx| this.close_ports(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }

    fn render_port_row(
        &self,
        id: (&'static str, usize),
        row: &PortRow,
        ours: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let project = row
            .project
            .and_then(|index| self.project_sessions.projects.get(index));
        let (dot, place) = match project {
            Some(slot) => (slot.color, slot.task_space.title.clone()),
            None => (theme.fg2, SharedString::from(i18n::t!("ports.no_project"))),
        };
        let open_row = row.clone();
        let stop_row = row.clone();
        let button = |id: (&'static str, usize), label: String| {
            div()
                .id(id)
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
                .child(SharedString::from(label))
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(32.))
            .px(px(6.))
            .rounded(px(6.))
            .hover(|style| style.bg(theme.bg3))
            .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
            .child(
                div()
                    .flex_none()
                    .w(px(64.))
                    .font_family(ui::code_font(cx))
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .child(SharedString::from(format!(":{}", row.port.port))),
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
                            .child(SharedString::from(format!(
                                "{} · {} ({})",
                                place, row.port.command, row.port.pid
                            ))),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(row.port.address.clone())),
                    ),
            )
            .child(
                button((id.0, id.1 * 2), i18n::t!("ports.open")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_port(&open_row, window, cx);
                    }),
                ),
            )
            .when(ours, |element| {
                element.child(
                    button((id.0, id.1 * 2 + 1), i18n::t!("ports.stop")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.stop_port(&stop_row, cx);
                        }),
                    ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_listen_output_becomes_one_row_per_port() {
        let text = "p4242\ncnode\nn127.0.0.1:5173\nn[::1]:5173\np5151\ncpython3\nn*:8000\np99\ncpostgres\nnlocalhost:5432\n";
        let ports = parse_lsof_listen(text);
        assert_eq!(
            ports,
            vec![
                ListeningPort {
                    pid: 4242,
                    command: "node".into(),
                    port: 5173,
                    address: "127.0.0.1".into(),
                },
                ListeningPort {
                    pid: 99,
                    command: "postgres".into(),
                    port: 5432,
                    address: "localhost".into(),
                },
                ListeningPort {
                    pid: 5151,
                    command: "python3".into(),
                    port: 8000,
                    address: "*".into(),
                },
            ],
            "IPv4 と IPv6 の同じポートは 1 行・ポート順"
        );
    }

    #[test]
    fn only_descendants_of_necoder_are_ours() {
        let parents =
            parse_ps_parents("  100     1\n  200   100\n  300   200\n  400     1\n  500   500\n");
        assert!(is_descendant(300, 100, &parents), "孫も子孫");
        assert!(is_descendant(200, 100, &parents));
        assert!(!is_descendant(400, 100, &parents), "ほかのアプリ");
        assert!(!is_descendant(500, 100, &parents), "輪になっていても止まる");
        assert!(!is_descendant(999, 100, &parents), "知らない pid");
    }

    /// 実プロセス: テストの子が待ち受けたポートは「necoder の中」で、作業フォルダのプロジェクトに付く。
    #[cfg(unix)]
    #[test]
    fn a_child_dev_server_is_found_with_its_project() {
        if std::process::Command::new("lsof")
            .arg("-v")
            .output()
            .is_err()
        {
            return; // lsof が無い環境
        }
        let root = std::env::temp_dir().join(format!("necoder_ports_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = paths::canonicalize(&root).unwrap();
        let child = std::process::Command::new("python3")
            .current_dir(&root)
            .args([
                "-c",
                "import socket,sys,time\ns=socket.socket()\ns.bind(('127.0.0.1',0))\ns.listen()\nprint(s.getsockname()[1],flush=True)\ntime.sleep(30)",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            return; // python3 が無い環境
        };
        let mut line = String::new();
        {
            use std::io::BufRead as _;
            let stdout = child.stdout.take().expect("stdout");
            std::io::BufReader::new(stdout)
                .read_line(&mut line)
                .unwrap();
        }
        let port: u16 = line.trim().parse().expect("ポート番号");
        let rows = collect_ports(
            std::process::id(),
            &[PathBuf::from("/nowhere"), root.clone()],
        )
        .expect("読める");
        let row = rows
            .iter()
            .find(|row| row.port.port == port)
            .unwrap_or_else(|| panic!("{port} が一覧にある: {rows:?}"));
        assert!(row.ours, "テストの子 = necoder の子孫");
        assert_eq!(row.project, Some(1), "作業フォルダのプロジェクト");
        assert!(still_ours(row.port.pid, std::process::id()));
        child.kill().ok();
        child.wait().ok();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ports_belong_to_the_deepest_project_folder() {
        let cwds =
            parse_lsof_cwd("p4242\nfcwd\nn/work/necoder-worktrees/rope/web\np5151\nfcwd\nn/tmp\n");
        let roots = vec![
            PathBuf::from("/work/necoder"),
            PathBuf::from("/work/necoder-worktrees/rope"),
        ];
        assert_eq!(
            project_for_cwd(&cwds[&4242], &roots),
            Some(1),
            "Task の worktree の中"
        );
        assert_eq!(project_for_cwd(&cwds[&5151], &roots), None);
        assert_eq!(
            project_for_cwd(Path::new("/work/necoder/app"), &roots),
            Some(0)
        );
    }
}
