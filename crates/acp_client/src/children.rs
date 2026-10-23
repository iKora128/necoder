//! ACP 子プロセスの後始末。
//!
//! `std::process::Child` は drop では死なない。GUI を閉じてもエージェントが残ると、閉じた後に
//! ターンが終わって通知音が鳴る・メモリが残る・同じスレッドが二重に走る、が起きる（2026-10-23 の実害）。
//!
//! 起動時に PID を登録し、[`ChildGuard`] の drop（＝セッション終了）で SIGTERM を送る。
//! アプリ終了時は [`shutdown_all`] が全員へ SIGTERM → 猶予 → SIGKILL。
//!
//! **PID だけを持つ**（`Child` はセッションが持つ）。PID の再利用は理論上あり得るが、番人は
//! セッションと同じ寿命で、終了時に即座に外れる。アプリ終了の猶予は数百 ms なので、実害より
//! 「閉じたのに残る」を優先する。
//!
//! 登録先は [`Registry`]。アプリは global の 1 つを使うが、テストは自分専用の registry を作る
//! （並走するテストの子を巻き込まない）。

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// 生きている ACP 子プロセスの PID の集合。
#[derive(Debug, Default)]
pub struct Registry {
    children: Arc<Mutex<BTreeSet<u32>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 子プロセスを登録する。返った番人はセッションの間だけ持つ（drop で SIGTERM）。
    pub fn register(&self, pid: u32) -> ChildGuard {
        if let Ok(mut children) = self.children.lock() {
            children.insert(pid);
        }
        ChildGuard {
            children: Arc::clone(&self.children),
            pid,
        }
    }

    /// 登録されている子を全部止める。SIGTERM を送り、`grace` 待っても生きていれば SIGKILL。
    /// 止めた本数を返す。
    ///
    /// **待つのは猶予であって完了の保証ではない**。子は親が `wait` するまで zombie として残り、
    /// `kill(pid, 0)` では生きて見えるため、猶予を丸ごと使うことがある（終了時だけなので許容）。
    pub fn shutdown_all(&self, grace: Duration) -> usize {
        let pids: Vec<u32> = match self.children.lock() {
            Ok(children) => children.iter().copied().collect(),
            Err(_) => return 0,
        };
        if pids.is_empty() {
            return 0;
        }
        for &pid in &pids {
            signal(pid, Signal::Terminate);
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline && pids.iter().any(|&pid| is_alive(pid)) {
            std::thread::sleep(Duration::from_millis(20));
        }
        for &pid in &pids {
            if is_alive(pid) {
                signal(pid, Signal::Kill);
            }
        }
        // 二重に呼ばれても（quit_now → app.run の後）猶予を繰り返さない。
        if let Ok(mut children) = self.children.lock() {
            children.clear();
        }
        pids.len()
    }
}

/// アプリ全体の registry（GUI 1 つ = 1 プロセス）。
static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

/// 登録した子の番人。落ちるときに SIGTERM を送り、登録から外す。
#[derive(Debug)]
pub struct ChildGuard {
    children: Arc<Mutex<BTreeSet<u32>>>,
    pid: u32,
}

impl ChildGuard {
    /// 登録した子の PID。
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(mut children) = self.children.lock() {
            children.remove(&self.pid);
        }
        signal(self.pid, Signal::Terminate);
    }
}

/// アプリの registry へ子プロセスを登録する。返った番人はセッションの間だけ持つ。
pub fn register(pid: u32) -> ChildGuard {
    REGISTRY.register(pid)
}

/// アプリの registry の子を全部止める（アプリ終了時に呼ぶ）。
pub fn shutdown_all(grace: Duration) -> usize {
    REGISTRY.shutdown_all(grace)
}

#[derive(Clone, Copy, Debug)]
enum Signal {
    Terminate,
    Kill,
}

#[cfg(unix)]
fn signal(pid: u32, signal: Signal) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    let number = match signal {
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // ACP の子は新しいグループの長（`CommandSpec::in_own_process_group`）。長なら負の PID で
    // **グループ全体**（app-server / MCP などの孫も）へ送る。長でなければその PID だけ。
    // SAFETY: getpgid は問い合わせだけ。負の PID はプロセスグループ宛て。
    let group_leader = unsafe { libc::getpgid(pid) } == pid;
    let target = if group_leader { -pid } else { pid };
    // SAFETY: 自分が起動した子（またはそのグループ）へシグナルを送るだけ。
    unsafe {
        libc::kill(target, number);
    }
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: 0 番のシグナルは存在確認だけで、相手を止めない。
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(windows)]
fn signal(pid: u32, signal: Signal) {
    let mut command = std::process::Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T"]);
    if matches!(signal, Signal::Kill) {
        command.arg("/F");
    }
    let _status = command.status();
}

#[cfg(windows)]
fn is_alive(_pid: u32) -> bool {
    // 生死は見ず、猶予を使ってから taskkill /F /T で強制終了する（SIGTERM 相当に当たる
    // taskkill 単体はコンソールの子に効かないことがある）。
    true
}

#[cfg(not(any(unix, windows)))]
fn signal(_pid: u32, _signal: Signal) {}

#[cfg(not(any(unix, windows)))]
fn is_alive(_pid: u32) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::Command;

    fn sleeping_child() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep を起動できる")
    }

    fn sleeping_group_leader() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .expect("sleep を起動できる")
    }

    #[test]
    fn dropping_the_guard_terminates_the_child() {
        let registry = Registry::new();
        let mut child = sleeping_child();
        {
            let _guard = registry.register(child.id());
            assert!(is_alive(child.id()));
        }
        let status = child.wait().expect("子の終了を待てる");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn dropping_the_guard_terminates_a_process_group_leader() {
        let registry = Registry::new();
        let mut child = sleeping_group_leader();
        {
            let _guard = registry.register(child.id());
            assert!(is_alive(child.id()));
        }
        let status = child.wait().expect("子の終了を待てる");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn a_spec_can_start_its_own_process_group() {
        use host::{CommandSpec, LocalHost};

        let spec = CommandSpec::new("sleep", std::env::temp_dir())
            .args(["30"])
            .in_own_process_group();
        let mut process = LocalHost::shared()
            .spawn_process(&spec)
            .expect("sleep を起動できる");
        let pid = process.id();
        // 新しいグループの長になっている（`signal` が負の PID へ送る前提）。
        assert_eq!(
            unsafe { libc::getpgid(pid as libc::pid_t) },
            pid as libc::pid_t
        );
        let killed = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        assert_eq!(killed, 0, "グループを止められる");
        process.wait().expect("子の終了を待てる");
    }

    #[test]
    fn shutdown_all_terminates_every_registered_child() {
        let registry = Registry::new();
        let mut child = sleeping_child();
        let _guard = registry.register(child.id());
        let stopped = registry.shutdown_all(Duration::from_millis(200));
        assert!(stopped >= 1, "登録した子を止めた（止めた本数 {stopped}）");
        let status = child.wait().expect("子の終了を待てる");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}
