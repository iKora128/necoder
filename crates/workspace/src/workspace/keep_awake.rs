//! エージェントが作業している間はコンピュータを寝かせない（O13・B17 Keep computer awake）。
//!
//! 放っておいた時のスリープ（idle sleep）が来ると、作業中のエージェントもその場で止まり、API への
//! 接続が切れてターンが途切れる。夜通し並べて走らせる使い方ではこれが一番の事故なので、設定
//! `keep_awake`（既定 `"working"`）の間は、**作業中（Working）のスレッドが全窓で 1 本でもある間だけ**
//! OS の「寝かせない」を掴み、0 本になったら放す。
//!
//! - 止めるのは放っておいた時のスリープだけ。画面は普通に消える。自分で選んだスリープ
//!   （メニューの「スリープ」）と、ノートの蓋を閉じた時のスリープは止めない（止められない）。
//! - 承認待ち・質問待ち（Blocked）は数えない。人の返事を待っている間まで起こし続けない。
//! - 電池の時も掴む。作業が終われば放すので、掴む長さはターンの長さで頭打ちになる（スマホ連携の
//!   ホストが電池の間は掴まないのは、あちらが**終わりの無い**掴み方だから・JOURNAL 2026-09-13）。
//! - macOS は `caffeinate -i -w <necoder の pid>`（necoder が落ちても一緒に抜ける）。Windows は
//!   `SetThreadExecutionState`（前景スレッドに付く・プロセスが終われば消える）。ほかの OS は
//!   何もしない（設定の行も出さない）。
//!
//! 数え直すのはスレッドの状態台帳（`RunningRegistry`）と設定が変わるたび（Dock バッジと同じ経路）。

use crate::workspace::*;
use agent_panel::{RunningRegistry, ThreadActivity};
use settings_core::KeepAwake;

/// この OS で寝かせない手段を持っているか（持たない OS では何もしない）。
pub(crate) const KEEP_AWAKE_SUPPORTED: bool = cfg!(any(target_os = "macos", windows));

/// 寝かせないでおくか: 設定が「作業中の間」で、作業中のスレッドが 1 本でもある。
pub(crate) fn wants_wake_lock(mode: KeepAwake, working_threads: usize) -> bool {
    mode == KeepAwake::WhileWorking && working_threads > 0
}

/// 全窓で作業中（Working）のスレッドの数。承認待ち・質問待ち・完了・待機は数えない。
fn working_threads(cx: &App) -> usize {
    cx.try_global::<RunningRegistry>()
        .map(|registry| {
            registry
                .0
                .values()
                .flatten()
                .filter(|row| row.2 == ThreadActivity::Working)
                .count()
        })
        .unwrap_or(0)
}

/// OS の「寝かせない」の握り。落とす（drop）と放す。
trait WakeLock {
    /// まだ効いているか（外から殺された子プロセスなら false）。
    fn is_held(&mut self) -> bool;
}

/// いま掴んでいる「寝かせない」。
#[derive(Default)]
struct KeepAwakeState {
    lock: Option<Box<dyn WakeLock>>,
    /// 掴もうとして失敗した。作業中が続く間は掴み直さない（台帳が変わるたびに同じ失敗を
    /// ログへ積まない）。作業中が 0 本になったら忘れて、次の作業でまた試す。
    failed: bool,
}

impl gpui::Global for KeepAwakeState {}

impl KeepAwakeState {
    /// 欲しい状態に合わせて掴む / 放す。掴み方は `acquire`（テストでは偽物を渡す）。
    fn apply(&mut self, wanted: bool, acquire: impl FnOnce() -> anyhow::Result<Box<dyn WakeLock>>) {
        // 外から止められていた（`caffeinate` を手で殺した等）なら、掴んでいない扱いに戻す。
        if self.lock.as_mut().is_some_and(|lock| !lock.is_held()) {
            self.lock = None;
        }
        if !wanted {
            // drop で放す。
            self.lock = None;
            self.failed = false;
            return;
        }
        if self.lock.is_some() || self.failed {
            return;
        }
        match acquire() {
            Ok(lock) => self.lock = Some(lock),
            Err(error) => {
                eprintln!("スリープを止められない（この作業の間は諦める）: {error:#}");
                self.failed = true;
            }
        }
    }
}

/// 作業中のスレッドの数と設定を見直して、寝かせない / 放すを切り替える。
pub(crate) fn refresh_keep_awake(cx: &mut App) {
    if !KEEP_AWAKE_SUPPORTED {
        return;
    }
    // 設定は丸ごと複製せずに 1 項目だけ読む（台帳はターンの出来事のたびに変わる）。
    let mode = cx
        .try_global::<settings::SettingsGlobal>()
        .map(|global| global.settings().keep_awake_mode())
        .unwrap_or(KeepAwake::WhileWorking);
    let wanted = wants_wake_lock(mode, working_threads(cx));
    cx.default_global::<KeepAwakeState>()
        .apply(wanted, acquire_wake_lock);
}

/// 起動時に 1 回だけ繋ぐ（main から呼ぶ）: スレッドの状態台帳と設定が変わるたびに数え直す。
pub fn install_keep_awake(cx: &mut App) {
    cx.observe_global::<RunningRegistry>(refresh_keep_awake)
        .detach();
    cx.observe_global::<settings::SettingsGlobal>(refresh_keep_awake)
        .detach();
}

#[cfg(target_os = "macos")]
fn acquire_wake_lock() -> anyhow::Result<Box<dyn WakeLock>> {
    use anyhow::Context as _;
    use std::process::{Command, Stdio};

    /// 抱えている間だけ効く `caffeinate`。落とすと殺して回収する。
    struct Caffeinate(std::process::Child);

    impl WakeLock for Caffeinate {
        fn is_held(&mut self) -> bool {
            matches!(self.0.try_wait(), Ok(None))
        }
    }

    impl Drop for Caffeinate {
        fn drop(&mut self) {
            if let Err(error) = self.0.kill() {
                eprintln!("caffeinate を止められない: {error:#}");
            }
            // 回収しないとゾンビが残る（kill の直後なのですぐ返る）。
            if let Err(error) = self.0.wait() {
                eprintln!("caffeinate を回収できない: {error:#}");
            }
        }
    }

    // -i = 放っておいた時のスリープを止める（電池でも効く）。-w <pid> = necoder が落ちたら
    // caffeinate も抜ける（放し忘れて永久に起こし続けない保険）。画面（-d）は止めない。
    // -s（AC の時のシステムスリープ）も付けない: 人が選んだスリープまで止めたくない。
    let child = Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("caffeinate を起動できない")?;
    Ok(Box::new(Caffeinate(child)))
}

#[cfg(windows)]
fn acquire_wake_lock() -> anyhow::Result<Box<dyn WakeLock>> {
    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };

    /// 前景スレッドに付けた「システムを起こしておく」実行状態。落とすと元に戻す。
    struct ThreadExecutionState;

    impl WakeLock for ThreadExecutionState {
        fn is_held(&mut self) -> bool {
            true
        }
    }

    impl Drop for ThreadExecutionState {
        fn drop(&mut self) {
            // SAFETY: 引数はフラグだけ。付けた時と同じ前景スレッドから呼ぶ（GPUI の observer は
            // 前景で走る）。ES_CONTINUOUS だけ = 付けていた要求を外す。
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            if previous == 0 {
                eprintln!("スリープの抑止を解けない（SetThreadExecutionState）");
            }
        }
    }

    // SAFETY: 同上。ES_CONTINUOUS = 外すまで効き続ける。ES_SYSTEM_REQUIRED = 放っておいた時の
    // スリープを止める。画面（ES_DISPLAY_REQUIRED）は付けない＝画面は普通に消える。
    let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
    if previous == 0 {
        anyhow::bail!("SetThreadExecutionState が失敗した");
    }
    Ok(Box::new(ThreadExecutionState))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn acquire_wake_lock() -> anyhow::Result<Box<dyn WakeLock>> {
    anyhow::bail!("この OS ではスリープを止める手段を持っていない")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// 掴んだ回数・放した回数を数える偽物。`alive` を false にすると「外から殺された」。
    #[derive(Default, Clone)]
    struct Counts {
        acquired: Rc<Cell<usize>>,
        released: Rc<Cell<usize>>,
        alive: Rc<Cell<bool>>,
    }

    struct FakeLock(Counts);

    impl WakeLock for FakeLock {
        fn is_held(&mut self) -> bool {
            self.0.alive.get()
        }
    }

    impl Drop for FakeLock {
        fn drop(&mut self) {
            self.0.released.set(self.0.released.get() + 1);
        }
    }

    fn acquire(counts: &Counts) -> impl FnOnce() -> anyhow::Result<Box<dyn WakeLock>> + '_ {
        move || {
            counts.acquired.set(counts.acquired.get() + 1);
            counts.alive.set(true);
            Ok(Box::new(FakeLock(counts.clone())) as Box<dyn WakeLock>)
        }
    }

    #[test]
    fn only_working_threads_keep_the_computer_awake() {
        assert!(wants_wake_lock(KeepAwake::WhileWorking, 1));
        assert!(!wants_wake_lock(KeepAwake::WhileWorking, 0));
        assert!(
            !wants_wake_lock(KeepAwake::Off, 3),
            "off なら作業中でも止めない"
        );
    }

    #[test]
    fn the_lock_is_taken_once_and_released_when_work_stops() {
        let counts = Counts::default();
        let mut state = KeepAwakeState::default();
        state.apply(true, acquire(&counts));
        state.apply(true, acquire(&counts));
        state.apply(true, acquire(&counts));
        assert_eq!(counts.acquired.get(), 1, "作業中が続く間は掴み直さない");
        assert_eq!(counts.released.get(), 0);

        state.apply(false, acquire(&counts));
        assert_eq!(counts.released.get(), 1, "作業中が 0 本になったら放す");
        state.apply(false, acquire(&counts));
        assert_eq!(counts.released.get(), 1, "放した後は何もしない");

        state.apply(true, acquire(&counts));
        assert_eq!(counts.acquired.get(), 2, "次の作業でまた掴む");
    }

    #[test]
    fn a_lock_killed_from_outside_is_taken_again() {
        let counts = Counts::default();
        let mut state = KeepAwakeState::default();
        state.apply(true, acquire(&counts));
        counts.alive.set(false);
        state.apply(true, acquire(&counts));
        assert_eq!(counts.acquired.get(), 2, "外から止められていたら掴み直す");
        assert_eq!(counts.released.get(), 1, "死んだ握りは片付ける");
    }

    #[test]
    fn a_failure_is_not_retried_until_the_next_piece_of_work() {
        let attempts = Cell::new(0);
        let failing = || {
            attempts.set(attempts.get() + 1);
            Err(anyhow::anyhow!("caffeinate が無い"))
        };
        let mut state = KeepAwakeState::default();
        state.apply(true, failing);
        state.apply(true, || {
            attempts.set(attempts.get() + 1);
            Err(anyhow::anyhow!("caffeinate が無い"))
        });
        assert_eq!(attempts.get(), 1, "同じ作業中の間は同じ失敗を繰り返さない");
        state.apply(false, || unreachable!("放す時は掴まない"));
        let counts = Counts::default();
        state.apply(true, acquire(&counts));
        assert_eq!(counts.acquired.get(), 1, "作業が一度終わったらまた試す");
    }
}
