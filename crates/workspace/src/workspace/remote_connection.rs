//! remote host の接続状態を statusbar へ流す配線 + 「今すぐ再接続」（M9・2026-09-08）。
//!
//! host 側の `ReconnectingClient` は切断の検知と張り直しを自分で回すが、その結果は crate の中に
//! 閉じていて UI からは見えなかった（SSH チップは常に緑）。ここでは remote host ごとに 1 本だけ
//! 購読を張り、状態が変わるたびに Workspace を notify する。描画側は
//! `host.connection_state()`（atomic の読み・I/O 無し）を見るだけなので UI スレッドを止めない。
//!
//! 「再接続」チップは heartbeat のバックオフ（到達不能が続くと最大 60s）を短絡するための入口。
//! 生きている接続を捨てて張り直したりはしない（host 側の判定は heartbeat と同じ）。
use crate::workspace::*;

/// 1 remote host ぶんの購読。std の `mpsc::Receiver` はブロッキングなので専用スレッドで受け、
/// futures のチャネルへ転送して前景タスクが待つ（remote watch の pump と同じ形）。
/// drop で転送スレッドを止める（Workspace が消えたら追従して終わる）。
pub(crate) struct ConnectionPump {
    _task: gpui::Task<()>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for ConnectionPump {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl Workspace {
    /// レール上の remote host 全部について接続状態の購読が張られているようにする。同じ host
    /// （同じ id）を複数プロジェクトで使っていても購読は 1 本。レールから消えた host の購読は外す。
    /// 切替・レールへの追加・復元の各入口から呼ぶ（冪等・I/O 無し）。
    pub(crate) fn ensure_connection_pumps(&mut self, cx: &mut gpui::Context<Self>) {
        use futures::StreamExt as _;
        let hosts: Vec<std::sync::Arc<dyn host::Host>> = self
            .project_sessions
            .projects
            .iter()
            .filter(|slot| slot.worktree.is_remote())
            .map(|slot| slot.worktree.host().clone())
            .collect();
        let live_ids: std::collections::HashSet<String> =
            hosts.iter().map(|host| host.id().to_string()).collect();
        self.connection_pumps
            .retain(|host_id, _pump| live_ids.contains(host_id));
        for host in hosts {
            let host_id = host.id().to_string();
            if self.connection_pumps.contains_key(&host_id) {
                continue;
            }
            let Some(receiver) = host.watch_connection() else {
                continue;
            };
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (sender, mut changes) =
                futures::channel::mpsc::unbounded::<host::ConnectionState>();
            let thread_stop = stop.clone();
            let spawned = std::thread::Builder::new()
                .name("necoder-remote-state-pump".to_string())
                .spawn(move || {
                    use std::sync::mpsc::RecvTimeoutError;
                    while !thread_stop.load(std::sync::atomic::Ordering::Acquire) {
                        match receiver.recv_timeout(std::time::Duration::from_millis(500)) {
                            Ok(state) => {
                                if sender.unbounded_send(state).is_err() {
                                    break; // 前景側が終わった
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break, // host が消えた
                        }
                    }
                });
            if let Err(error) = spawned {
                eprintln!("Remote SSH: 接続状態の転送スレッドを起動できない: {error}");
                continue;
            }
            let task = cx.spawn(async move |workspace, cx| {
                while changes.next().await.is_some() {
                    // 値そのものは描画時に host から読む。ここは「変わった」を伝えるだけ。
                    if workspace.update(cx, |_workspace, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            });
            self.connection_pumps
                .insert(host_id, ConnectionPump { _task: task, stop });
        }
    }

    /// statusbar の「再接続」チップ: アクティブプロジェクトの remote host に今すぐ張り直しを頼む。
    /// 接続は host が背景スレッドで張り、結果は購読経由で statusbar に反映される。
    pub(crate) fn reconnect_active_remote(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(slot) = self.active_slot() else {
            return;
        };
        if !slot.worktree.is_remote() {
            return;
        }
        let host = slot.worktree.host().clone();
        let name = slot
            .remote_host
            .clone()
            .unwrap_or_else(|| gpui::SharedString::from(host.display_name().to_string()));
        host.reconnect();
        self.push_toast(
            gpui::SharedString::from(i18n::t!("ssh.reconnecting", "host" => name)),
            self.accent(),
            cx,
        );
        cx.notify();
    }
}
