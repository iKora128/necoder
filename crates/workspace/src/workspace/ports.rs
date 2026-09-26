//! Ports（O5・手元の分）: necoder の中で動いている開発サーバのポートを一覧し、開く / 止める。
//!
//! 読むのは**開いた時と ↻ の時だけ**（ポーリングしない・常駐しない）。手元の `lsof`（LISTEN 中の TCP）と
//! `ps`（親子関係）を 1 回ずつ、necoder の子孫プロセスの作業フォルダを `lsof -d cwd` で 1 回読み、
//! 作業フォルダが入っているプロジェクト / Task に割り当てる（エージェントが立てたサーバも、端末で
//! 立てたサーバも同じ扱い）。**止められるのは necoder の子孫だけ**（止める直前にもう一度確かめる）。
//! ほかのアプリのポートは「ほかのプロセス」に畳んで出す（開けるが止めない）。
//!
//! **SSH の接続先**（O5 の続き）: 開いている接続先ごとに、そこで LISTEN しているポートを 1 回読み
//! （`ss`・無ければ `lsof`・自分のユーザーのプロセスだけ見える）、作業フォルダでプロジェクトに割り当てる。
//! 「転送」で ControlMaster の `-O forward -L` を張り（手元は 127.0.0.1・同じ番号が塞がっていれば別の番号）、
//! 転送した行は「開く」で手元の localhost を Web タブで開く。再接続したら張り直す（host）。接続先の
//! プロセスは止めない（ほかの人の物かもしれない・端末から止める）。
//!
//! Windows は未対応（lsof / ps が無い・OpenSSH の ControlMaster も無い）。

use crate::workspace::*;
use std::collections::HashMap;

/// 接続先で LISTEN しているポートと、そのプロセスの作業フォルダを 1 回で読むスクリプト（O5・SSH）。
/// Linux は `ss` と `/proc/<pid>/cwd`、無ければ `lsof`（mac の接続先）。どちらも無ければ空。
/// 区切りの行（`necoder-ports …` / `necoder-cwd`）で [`parse_remote_ports`] が読み分ける。
pub(crate) const REMOTE_PORTS_SCRIPT: &str = r#"if command -v ss >/dev/null 2>&1; then
  echo 'necoder-ports ss'
  ss -ltnpH 2>/dev/null
  echo 'necoder-cwd'
  for pid in $(ss -ltnpH 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
    printf '%s %s\n' "$pid" "$(readlink "/proc/$pid/cwd" 2>/dev/null)"
  done
elif command -v lsof >/dev/null 2>&1; then
  echo 'necoder-ports lsof'
  lsof -nP -iTCP -sTCP:LISTEN -Fpcn 2>/dev/null
  echo 'necoder-cwd'
  pids=$(lsof -nP -iTCP -sTCP:LISTEN -Fp 2>/dev/null | sed -n 's/^p//p' | sort -u | paste -sd, -)
  if [ -n "$pids" ]; then
    lsof -a -d cwd -p "$pids" -Fpn 2>/dev/null | awk '/^p/{pid=substr($0,2)} /^n/{print pid, substr($0,2)}'
  fi
fi
"#;

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

/// 接続先のポートの 1 行（O5・SSH）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemotePortRow {
    pub(crate) port: ListeningPort,
    /// 作業フォルダが入っているレールの添字（無ければ `None`）。
    pub(crate) project: Option<usize>,
    /// 手元へ転送している番号（していなければ `None`）。
    pub(crate) forwarded: Option<u16>,
}

/// 接続先 1 つ分のポート。
pub(crate) struct RemotePorts {
    pub(crate) host: Arc<dyn host::Host>,
    pub(crate) label: SharedString,
    /// `None` = 読んでいる途中。
    pub(crate) rows: Option<Result<Vec<RemotePortRow>, SharedString>>,
}

/// Ports の画面の状態（開いている間だけ Some）。
pub(crate) struct PortsState {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    /// `None` = 読んでいる途中。
    rows: Option<Result<Vec<PortRow>, SharedString>>,
    /// ほかのプロセスのポートを開いているか。
    show_others: bool,
    /// 開いている接続先ごとのポート（レールの順・同じ接続先は 1 つ）。
    pub(crate) remote: Vec<RemotePorts>,
}

/// `ss -ltnpH` を読む（純関数）。`LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:(("node",pid=12,fd=21))`。
/// プロセスが見えない行（ほかのユーザーの物）は pid 0。同じ pid とポートの IPv4 / IPv6 は 1 行。
pub(crate) fn parse_ss_listen(text: &str) -> Vec<ListeningPort> {
    let mut ports: Vec<ListeningPort> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(local) = fields.get(3) else {
            continue;
        };
        let Some((address, port)) = local.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.parse::<u16>() else {
            continue;
        };
        let process = fields
            .get(5..)
            .map(|rest| rest.join(" "))
            .unwrap_or_default();
        let command = process
            .split_once("((\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(command, _)| command.to_string())
            .unwrap_or_default();
        let pid = process
            .split_once("pid=")
            .map(|(_, rest)| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|digits| digits.parse::<u32>().ok())
            .unwrap_or(0);
        if ports
            .iter()
            .any(|known| known.pid == pid && known.port == port)
        {
            continue;
        }
        ports.push(ListeningPort {
            pid,
            command,
            port,
            address: address.to_string(),
        });
    }
    ports.sort_by_key(|port| (port.port, port.pid));
    ports
}

/// [`REMOTE_PORTS_SCRIPT`] の出力を読む（純関数）。ポートと、pid → 作業フォルダ。
pub(crate) fn parse_remote_ports(text: &str) -> (Vec<ListeningPort>, HashMap<u32, PathBuf>) {
    let (head, cwd_part) = text.split_once("necoder-cwd\n").unwrap_or((text, ""));
    let ports = if let Some(listing) = head.split_once("necoder-ports ss\n") {
        parse_ss_listen(listing.1)
    } else if let Some(listing) = head.split_once("necoder-ports lsof\n") {
        parse_lsof_listen(listing.1)
    } else {
        Vec::new()
    };
    let cwds = cwd_part
        .lines()
        .filter_map(|line| {
            let (pid, cwd) = line.split_once(' ')?;
            let cwd = cwd.trim();
            (!cwd.is_empty()).then(|| (pid.parse::<u32>().ok(), PathBuf::from(cwd)))
        })
        .filter_map(|(pid, cwd)| Some((pid?, cwd)))
        .collect();
    (ports, cwds)
}

/// 接続先のポートを集める（背景で呼ぶ）。`roots` はレールの順で、この接続先でないプロジェクトは
/// 何にも一致しないパス。
fn collect_remote_ports(
    host: &dyn host::Host,
    cwd: &Path,
    roots: &[PathBuf],
) -> Result<Vec<RemotePortRow>, String> {
    let output = host
        .run_command(&host.shell_script(REMOTE_PORTS_SCRIPT, cwd))
        .map_err(|error| format!("{error:#}"))?;
    let (ports, cwds) = parse_remote_ports(&String::from_utf8_lossy(&output.stdout));
    let forwarded = host.forwarded_ports();
    Ok(ports
        .into_iter()
        .map(|port| RemotePortRow {
            project: cwds
                .get(&port.pid)
                .and_then(|cwd| project_for_cwd(cwd, roots)),
            forwarded: forwarded
                .iter()
                .find(|(remote, _)| *remote == port.port)
                .map(|(_, local)| *local),
            port,
        })
        .collect())
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
            remote: Vec::new(),
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
        self.refresh_remote_ports(cx);
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

    /// 開いている接続先ごとにポートを読み直す（同じ接続先は 1 回・背景で）。
    fn refresh_remote_ports(&mut self, cx: &mut Context<Self>) {
        let mut hosts: Vec<(Arc<dyn host::Host>, PathBuf)> = Vec::new();
        for slot in &self.project_sessions.projects {
            let host = slot.worktree.host();
            if host.is_remote() && !hosts.iter().any(|(known, _)| known.id() == host.id()) {
                hosts.push((host.clone(), slot.worktree.root().to_path_buf()));
            }
        }
        let Some(state) = self.overlays.ports.as_mut() else {
            return;
        };
        state.remote = hosts
            .iter()
            .map(|(host, _)| RemotePorts {
                host: host.clone(),
                label: SharedString::from(host.display_name().to_string()),
                rows: None,
            })
            .collect();
        for (host, cwd) in hosts {
            // この接続先のプロジェクトのルートだけを突き合わせる（ほかは何にも一致しない）。
            let roots: Vec<PathBuf> = self
                .project_sessions
                .projects
                .iter()
                .map(|slot| {
                    if slot.worktree.host().id() == host.id() {
                        slot.worktree.root().to_path_buf()
                    } else {
                        PathBuf::from("\0other")
                    }
                })
                .collect();
            let id = host.id().to_string();
            cx.spawn(async move |workspace, cx| {
                let rows = cx
                    .background_executor()
                    .spawn(async move { collect_remote_ports(host.as_ref(), &cwd, &roots) })
                    .await;
                // Err = 待っている間に窓が閉じた。
                workspace
                    .update(cx, |workspace, cx| {
                        let section = workspace.overlays.ports.as_mut().and_then(|state| {
                            state
                                .remote
                                .iter_mut()
                                .find(|section| section.host.id() == id)
                        });
                        if let Some(section) = section {
                            section.rows = Some(rows.map_err(SharedString::from));
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .detach();
        }
    }

    /// 接続先のポートを手元へ転送する（背景で・張れたら知らせて読み直す）。
    fn forward_remote_port(
        &mut self,
        host: Arc<dyn host::Host>,
        remote_port: u16,
        cx: &mut Context<Self>,
    ) {
        let label = host.display_name().to_string();
        cx.spawn(async move |workspace, cx| {
            let forwarded = cx
                .background_executor()
                .spawn(async move { host.forward_port(remote_port) })
                .await;
            workspace
                .update(cx, |workspace, cx| {
                    match forwarded {
                        Ok(local) => {
                            let color = workspace.accent();
                            workspace.push_toast(
                                i18n::t!(
                                    "ports.forwarded",
                                    "local" => local,
                                    "host" => label,
                                    "remote" => remote_port
                                )
                                .into(),
                                color,
                                cx,
                            );
                        }
                        Err(error) => workspace.push_failure_toast(
                            SharedString::from(format!("{error:#}")),
                            None,
                            cx,
                        ),
                    }
                    workspace.refresh_ports(cx);
                })
                .ok();
        })
        .detach();
    }

    /// 転送をやめる（背景で・読み直す）。
    fn cancel_remote_forward(
        &mut self,
        host: Arc<dyn host::Host>,
        remote_port: u16,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |workspace, cx| {
            let cancelled = cx
                .background_executor()
                .spawn(async move { host.cancel_forward(remote_port) })
                .await;
            workspace
                .update(cx, |workspace, cx| {
                    if let Err(error) = cancelled {
                        workspace.push_failure_toast(
                            SharedString::from(format!("{error:#}")),
                            None,
                            cx,
                        );
                    }
                    workspace.refresh_ports(cx);
                })
                .ok();
        })
        .detach();
    }

    /// 転送した接続先のポートを、手元の localhost で開く（Web タブ）。
    fn open_forwarded_port(
        &mut self,
        row: &RemotePortRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(local) = row.forwarded else {
            return;
        };
        let url = format!("http://localhost:{local}/");
        let session = row.project.unwrap_or(self.project_sessions.active);
        self.close_ports(window, cx);
        self.open_url_from_session(session, &url, cx);
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
        // 接続先ごとのポート（O5・SSH）。見出し = 接続先の名前。
        for (section_index, section) in state.remote.iter().enumerate() {
            body = body.child(
                div()
                    .mt(px(10.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(
                        i18n::t!("ports.remote_heading", "host" => section.label.clone()),
                    )),
            );
            match &section.rows {
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
                Some(Ok(rows)) if rows.is_empty() => {
                    body = body.child(
                        div()
                            .py(px(6.))
                            .text_size(px(11.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("ports.remote_empty"))),
                    )
                }
                Some(Ok(rows)) => {
                    for (index, row) in rows.iter().enumerate() {
                        body = body.child(self.render_remote_port_row(
                            section_index * 1000 + index,
                            section.host.clone(),
                            row,
                            cx,
                        ));
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

    /// 接続先のポートの 1 行: 転送していなければ「転送」、していれば「開く」と「やめる」。
    fn render_remote_port_row(
        &self,
        id: usize,
        host: Arc<dyn host::Host>,
        row: &RemotePortRow,
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
        let process = if row.port.pid == 0 {
            i18n::t!("ports.remote_other_user")
        } else {
            format!("{} ({})", row.port.command, row.port.pid)
        };
        let detail = match row.forwarded {
            Some(local) => i18n::t!("ports.forwarded_to", "local" => local),
            None => row.port.address.clone(),
        };
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
        let remote_port = row.port.port;
        let mut line = div()
            .id(("remote-port", id))
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
                    .child(SharedString::from(format!(":{remote_port}"))),
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
                            .child(SharedString::from(format!("{place} · {process}"))),
                    )
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(if row.forwarded.is_some() {
                                theme.fg1
                            } else {
                                theme.fg2
                            })
                            .child(SharedString::from(detail)),
                    ),
            );
        if row.forwarded.is_some() {
            let open_row = row.clone();
            let cancel_host = host.clone();
            line = line
                .child(
                    button(("remote-port-open", id), i18n::t!("ports.open")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_forwarded_port(&open_row, window, cx);
                        }),
                    ),
                )
                .child(
                    button(("remote-port-cancel", id), i18n::t!("ports.stop_forward"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.cancel_remote_forward(cancel_host.clone(), remote_port, cx);
                            }),
                        ),
                );
        } else {
            line = line.child(
                button(("remote-port-forward", id), i18n::t!("ports.forward")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.forward_remote_port(host.clone(), remote_port, cx);
                    }),
                ),
            );
        }
        line.into_any_element()
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

    /// 接続先の `ss -ltnpH`: プロセスが見える行は pid と名前、見えない行（ほかのユーザー）は pid 0。
    #[test]
    fn ss_listen_output_becomes_one_row_per_port() {
        let text = "LISTEN 0      511        127.0.0.1:5173      0.0.0.0:*    users:((\"node\",pid=4242,fd=21))
LISTEN 0      511            [::1]:5173         [::]:*    users:((\"node\",pid=4242,fd=22))
LISTEN 0      4096   127.0.0.53%lo:53        0.0.0.0:*
LISTEN 0      128          0.0.0.0:8000      0.0.0.0:*    users:((\"python3\",pid=5151,fd=3))
";
        assert_eq!(
            parse_ss_listen(text),
            vec![
                ListeningPort {
                    pid: 0,
                    command: String::new(),
                    port: 53,
                    address: "127.0.0.53%lo".into(),
                },
                ListeningPort {
                    pid: 4242,
                    command: "node".into(),
                    port: 5173,
                    address: "127.0.0.1".into(),
                },
                ListeningPort {
                    pid: 5151,
                    command: "python3".into(),
                    port: 8000,
                    address: "0.0.0.0".into(),
                },
            ]
        );
    }

    /// スクリプトの出力は区切りの行で `ss` / `lsof` と作業フォルダに読み分ける。
    #[test]
    fn remote_listings_are_read_by_their_markers() {
        let (ports, cwds) = parse_remote_ports(
            "necoder-ports ss\nLISTEN 0 511 127.0.0.1:3000 0.0.0.0:* users:((\"node\",pid=7,fd=1))\nnecoder-cwd\n7 /srv/app/web\n8 \n",
        );
        assert_eq!(ports.len(), 1);
        assert_eq!(cwds.get(&7), Some(&PathBuf::from("/srv/app/web")));
        assert!(!cwds.contains_key(&8), "読めない作業フォルダは持たない");
        let (ports, _) = parse_remote_ports(
            "necoder-ports lsof\np9\ncruby\nn*:4567\nnecoder-cwd\n9 /home/me/site\n",
        );
        assert_eq!(ports[0].port, 4567);
        assert_eq!(ports[0].command, "ruby");
        assert!(
            parse_remote_ports("").0.is_empty(),
            "ss も lsof も無い接続先"
        );
    }

    /// 接続先のポート: 一覧に出し（作業フォルダのプロジェクトに付く）、「転送」で手元へ張り、「やめる」で外す。
    /// 接続先は偽物（決まった出力を返し、転送を控えるだけ）。
    #[cfg(not(target_os = "windows"))]
    #[gpui::test]
    fn remote_ports_are_listed_and_forwarded(cx: &mut gpui::TestAppContext) {
        use std::sync::Mutex;

        struct FakeRemote {
            inner: Arc<dyn host::Host>,
            listing: String,
            forwards: Mutex<Vec<(u16, u16)>>,
        }

        impl host::Host for FakeRemote {
            fn id(&self) -> &str {
                "fake-remote"
            }
            fn display_name(&self) -> &str {
                "devbox"
            }
            fn is_remote(&self) -> bool {
                true
            }
            fn host_for_project(&self, path: &Path) -> anyhow::Result<Arc<dyn host::Host>> {
                self.inner.host_for_project(path)
            }
            fn canonicalize(&self, path: &Path) -> anyhow::Result<PathBuf> {
                self.inner.canonicalize(path)
            }
            fn metadata(&self, path: &Path) -> anyhow::Result<host::HostMetadata> {
                self.inner.metadata(path)
            }
            fn read_dir(&self, path: &Path) -> anyhow::Result<Vec<host::HostEntry>> {
                self.inner.read_dir(path)
            }
            fn read_file(&self, path: &Path) -> anyhow::Result<host::FileContent> {
                self.inner.read_file(path)
            }
            fn write_file(
                &self,
                path: &Path,
                bytes: &[u8],
                condition: host::WriteCondition,
            ) -> anyhow::Result<host::FileRevision> {
                self.inner.write_file(path, bytes, condition)
            }
            fn list_files(&self, root: &Path, limit: usize) -> anyhow::Result<Vec<PathBuf>> {
                self.inner.list_files(root, limit)
            }
            fn search_project(
                &self,
                root: &Path,
                spec: &host::TextSearchSpec,
                file_limit: usize,
            ) -> anyhow::Result<Vec<host::TextSearchHit>> {
                self.inner.search_project(root, spec, file_limit)
            }
            fn run_command(&self, spec: &host::CommandSpec) -> anyhow::Result<host::CommandOutput> {
                if spec.args.iter().any(|arg| arg.contains("necoder-ports")) {
                    return Ok(host::CommandOutput {
                        status_code: Some(0),
                        stdout: self.listing.clone().into_bytes(),
                        stderr: Vec::new(),
                    });
                }
                self.inner.run_command(spec)
            }
            fn spawn_process(&self, spec: &host::CommandSpec) -> anyhow::Result<host::HostProcess> {
                self.inner.spawn_process(spec)
            }
            fn terminal_launch(&self, cwd: &Path) -> anyhow::Result<Option<host::TerminalLaunch>> {
                self.inner.terminal_launch(cwd)
            }
            fn forward_port(&self, remote_port: u16) -> anyhow::Result<u16> {
                let local = remote_port + 10_000;
                self.forwards.lock().unwrap().push((remote_port, local));
                Ok(local)
            }
            fn cancel_forward(&self, remote_port: u16) -> anyhow::Result<()> {
                self.forwards
                    .lock()
                    .unwrap()
                    .retain(|(remote, _)| *remote != remote_port);
                Ok(())
            }
            fn forwarded_ports(&self) -> Vec<(u16, u16)> {
                self.forwards.lock().unwrap().clone()
            }
        }

        let root =
            std::env::temp_dir().join(format!("necoder_remote_ports_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("web")).unwrap();
        let root = paths::canonicalize(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let listing = format!(
            "necoder-ports ss\nLISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:((\"node\",pid=4242,fd=21))\nnecoder-cwd\n4242 {}\n",
            root.join("web").display()
        );
        let fake: Arc<dyn host::Host> = Arc::new(FakeRemote {
            inner: host::LocalHost::shared(),
            listing,
            forwards: Mutex::new(Vec::new()),
        });
        let sources = vec![ProjectSource::new(fake.clone(), root.clone())];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.show_ports(&ShowPorts, window, cx)
        });
        cx.run_until_parked();
        let row = |workspace: &Workspace| {
            let state = workspace.overlays.ports.as_ref().expect("開いている");
            assert_eq!(state.remote.len(), 1, "接続先ごとに 1 つ");
            match &state.remote[0].rows {
                Some(Ok(rows)) => rows[0].clone(),
                other => panic!(
                    "読めていない: {:?}",
                    other.as_ref().map(|rows| rows.is_ok())
                ),
            }
        };
        workspace.update_in(cx, |workspace, _window, cx| {
            let first = row(workspace);
            assert_eq!(first.port.port, 5173);
            assert_eq!(first.project, Some(0), "作業フォルダのプロジェクト");
            assert_eq!(first.forwarded, None);
            workspace.forward_remote_port(fake.clone(), 5173, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, cx| {
            assert_eq!(row(workspace).forwarded, Some(15173));
            assert!(workspace
                .notifications
                .toasts
                .iter()
                .any(|toast| toast.text.contains("15173")));
            workspace.cancel_remote_forward(fake.clone(), 5173, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert_eq!(row(workspace).forwarded, None);
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
