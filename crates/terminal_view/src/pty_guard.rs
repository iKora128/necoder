//! PTY の読み口を包み、kitty keyboard のモードスタックが溢れる前に push を止める。
//!
//! ## なぜ要るか（alacritty_terminal 0.26.0 の不具合）
//!
//! `Term::push_keyboard_mode`（`term/mod.rs` 1295-1296）はスタックが上限 4096 に達すると、
//! **キーボードのスタックではなくタイトルのスタック**を `remove(0)` する。タイトルのスタックが
//! 空なら `Vec::remove` が panic し、PTY の読み取りスレッドが落ちる（端末は固まり、クラッシュ
//! ログも残る）。`CSI > 1 u` を 4097 回出すだけで起きるので、細工したファイルを `cat` するだけで
//! 再現する（`tests::the_library_panics_without_the_guard` が裏付け）。kitty keyboard を有効に
//! するまでは push 自体を無視していたので、この経路は開いていなかった。
//!
//! ## 直し方
//!
//! 本物の解析器（EventLoop 内・触れない）より先に、**同じ vte の解析器を影で走らせて**スタックの
//! 深さを数える。上限（[`STACK_LIMIT`]）を超える push は、終端文字 `u` を `~` に書き換えて
//! 未対応の無害な列（`CSI > … ~`）にする。push が確定するのは終端の `u` を読んだ瞬間なので、
//! バイト列を `u` ごとに区切って影へ渡せば、書き換える位置は必ず今読んだ塊の中にある
//! （前の塊を取り消す必要が無い）。
//!
//! 影の解析器は同期更新（`CSI ? 2026 h`）でも溜めない（[`NoSyncTimeout`]）。本物は溜めても最後は
//! 同じ順に処理するので、push / pop / 画面切替の呼ばれる順は一致し、深さの列も一致する。

use std::io;
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use alacritty_terminal::vte::ansi::{
    Handler, KeyboardModes, NamedPrivateMode, PrivateMode, Processor, Timeout,
};
use polling::{Event, PollMode, Poller};

/// 1 画面あたりの push の上限。実際のアプリの入れ子は数段なので十分に大きく、ライブラリの
/// 4096 には十分に遠い。これを超えた push は捨てる（モードはその時の一番上のまま）。
pub(crate) const STACK_LIMIT: usize = 64;

/// 影の解析器では同期更新を溜めない（来た順にすぐ処理する）。
#[derive(Default)]
struct NoSyncTimeout;

impl Timeout for NoSyncTimeout {
    fn set_timeout(&mut self, _duration: Duration) {}

    fn clear_timeout(&mut self) {}

    fn pending_timeout(&self) -> bool {
        false
    }
}

/// 影の解析器が呼ぶ Handler。`Term` のうちスタックの深さに関わる所だけを同じ規則でなぞる
/// （push / pop・主画面と代替画面の切替でスタックが入れ替わる・RIS で空になる）。
#[derive(Default)]
struct StackShadow {
    depth: usize,
    inactive_depth: usize,
    alternate_screen: bool,
    /// 直前の push が上限を超えていた（呼び元が終端文字を書き換える）。
    rejected_push: bool,
}

impl StackShadow {
    fn swap_screens(&mut self) {
        std::mem::swap(&mut self.depth, &mut self.inactive_depth);
        self.alternate_screen = !self.alternate_screen;
    }
}

impl Handler for StackShadow {
    fn push_keyboard_mode(&mut self, _mode: KeyboardModes) {
        if self.depth >= STACK_LIMIT {
            self.rejected_push = true;
        } else {
            self.depth += 1;
        }
    }

    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        self.depth = self.depth.saturating_sub(usize::from(to_pop));
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        let swap = matches!(
            mode,
            PrivateMode::Named(NamedPrivateMode::SwapScreenAndSetRestoreCursor)
        );
        if swap && !self.alternate_screen {
            self.swap_screens();
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        let swap = matches!(
            mode,
            PrivateMode::Named(NamedPrivateMode::SwapScreenAndSetRestoreCursor)
        );
        if swap && self.alternate_screen {
            self.swap_screens();
        }
    }

    fn reset_state(&mut self) {
        *self = Self::default();
    }
}

/// PTY の出力を本物の解析器より先に見て、溢れる push だけを無害化する。
#[derive(Default)]
pub(crate) struct KeyboardStackGuard {
    parser: Processor<NoSyncTimeout>,
    shadow: StackShadow,
}

impl KeyboardStackGuard {
    /// 読んだ塊をその場で書き換える（長さは変えない）。
    pub(crate) fn filter(&mut self, bytes: &mut [u8]) {
        let mut start = 0;
        while start < bytes.len() {
            let end = bytes[start..]
                .iter()
                .position(|&byte| byte == b'u')
                .map_or(bytes.len(), |offset| start + offset + 1);
            self.parser.advance(&mut self.shadow, &bytes[start..end]);
            if std::mem::take(&mut self.shadow.rejected_push) {
                bytes[end - 1] = b'~';
            }
            start = end;
        }
    }
}

/// [`KeyboardStackGuard`] を挟んだ PTY。読み以外（書き・登録・リサイズ・子の終了）は素通し。
pub(crate) struct GuardedPty<P> {
    pty: P,
    guard: KeyboardStackGuard,
}

impl<P> GuardedPty<P> {
    pub(crate) fn new(pty: P) -> Self {
        Self {
            pty,
            guard: KeyboardStackGuard::default(),
        }
    }
}

impl<P: EventedReadWrite> io::Read for GuardedPty<P> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.pty.reader().read(buffer)?;
        self.guard.filter(&mut buffer[..count]);
        Ok(count)
    }
}

impl<P: EventedReadWrite> EventedReadWrite for GuardedPty<P> {
    type Reader = Self;
    type Writer = P::Writer;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        // SAFETY: 包んだ PTY の登録をそのまま委ねる。登録したソースは包んだ PTY が持ち、
        // この値と同じ寿命なので、呼び元（EventLoop）の約束がそのまま守られる。
        unsafe { self.pty.register(poll, interest, mode) }
    }

    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        self.pty.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.pty.writer()
    }
}

impl<P: EventedPty> EventedPty for GuardedPty<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl<P: OnResize> OnResize for GuardedPty<P> {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::{Config, Term, TermMode};

    struct Size;

    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            24
        }
        fn screen_lines(&self) -> usize {
            24
        }
        fn columns(&self) -> usize {
            80
        }
    }

    fn kitty_term() -> Term<VoidListener> {
        let config = Config {
            kitty_keyboard: true,
            ..Config::default()
        };
        Term::new(config, &Size, VoidListener)
    }

    fn pushes(count: usize) -> Vec<u8> {
        b"\x1b[>1u".repeat(count)
    }

    /// ライブラリの不具合の裏付け。**これが落ちたら（＝ライブラリ側で直ったら）ガードは外してよい。**
    #[test]
    #[should_panic(expected = "removal index")]
    fn the_library_panics_without_the_guard() {
        let mut term = kitty_term();
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, &pushes(4097));
    }

    #[test]
    fn the_guard_keeps_the_stack_bounded() {
        let mut term = kitty_term();
        let mut parser: Processor = Processor::new();
        let mut guard = KeyboardStackGuard::default();
        let mut bytes = pushes(10_000);
        guard.filter(&mut bytes);
        parser.advance(&mut term, &bytes);
        // 上限までの push は効いている（モードは最後に受け付けた push のまま）。
        assert!(term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        // 捨てた push は 10_000 - 上限 個。
        let rejected = bytes.windows(4).filter(|window| *window == b"[>1~").count();
        assert_eq!(rejected, 10_000 - STACK_LIMIT);
    }

    #[test]
    fn sequences_split_across_reads_are_still_counted() {
        let mut term = kitty_term();
        let mut parser: Processor = Processor::new();
        let mut guard = KeyboardStackGuard::default();
        // 1 byte ずつ届いても（`ESC [ >` と `u` が別の読みに分かれても）上限で止まる。
        for mut byte in pushes(5_000).into_iter().map(|byte| [byte]) {
            guard.filter(&mut byte);
            parser.advance(&mut term, &byte);
        }
        assert!(term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
    }

    #[test]
    fn ordinary_push_and_pop_pass_through_untouched() {
        let mut term = kitty_term();
        let mut parser: Processor = Processor::new();
        let mut guard = KeyboardStackGuard::default();
        let original = b"hello \x1b[>1u world \x1b[<u done".to_vec();
        let mut bytes = original.clone();
        guard.filter(&mut bytes);
        assert_eq!(bytes, original, "上限内では 1 byte も変えない");
        parser.advance(&mut term, &bytes[..11]);
        assert!(term.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        parser.advance(&mut term, &bytes[11..]);
        assert!(!term.mode().intersects(TermMode::KITTY_KEYBOARD_PROTOCOL));
    }

    #[test]
    fn each_screen_has_its_own_stack() {
        let mut guard = KeyboardStackGuard::default();
        // 主画面を上限まで積む → 代替画面（1049h）はスタックが別なので積める → 戻ると主画面は満杯。
        let mut main = pushes(STACK_LIMIT);
        guard.filter(&mut main);
        let mut alternate = [b"\x1b[?1049h".as_slice(), &pushes(1)].concat();
        guard.filter(&mut alternate);
        assert!(alternate.ends_with(b"[>1u"), "代替画面の push は受け付ける");
        let mut back = [b"\x1b[?1049l".as_slice(), &pushes(1)].concat();
        guard.filter(&mut back);
        assert!(back.ends_with(b"[>1~"), "主画面は満杯のまま");
        // RIS（ESC c）で両方空になる。
        let mut reset = [b"\x1bc".as_slice(), &pushes(1)].concat();
        guard.filter(&mut reset);
        assert!(reset.ends_with(b"[>1u"));
    }
}
