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
//!
//! ## シェルの区切り（OSC 133・O25 / C20）
//!
//! 同じ読み口で、シェルが出すコマンドの区切り（OSC 133 と VS Code の OSC 633 の A〜D）も拾う。
//! 本物の解析器は OSC 133 を黙って捨てるので、EventLoop に手を入れずに見られるのはここだけ。
//! 分かるのは「いまコマンドが動いているか」と直前の終了コードで（[`ShellActivity`]）、前面の
//! プロセスを調べられない端末（Windows の ConPTY・SSH 先）でも閉じる前に確かめられるようになる。
//! 区切りを出すのはシェルの側（Oh My Posh の shell integration・自分で足した precmd 等・例はマニュアル）。

use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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

/// シェルの区切り（OSC 133 / 633）の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellMark {
    /// A: プロンプトの始まり。
    PromptStart,
    /// B: プロンプトの終わり（ここから人が打つ）。
    CommandStart,
    /// C: コマンドが動き始めた（ここから出力）。
    CommandExecuted,
    /// D: コマンドが終わった（終了コードが付いていれば）。
    CommandFinished(Option<i32>),
}

/// 終了コードが無いことを表す値（[`ShellActivity::exit`]）。
const NO_EXIT: i64 = i64::MIN;

/// シェルの区切りから分かる、端末の中のシェルの様子。PTY の読み取りスレッドが書き、画面の側が読む。
#[derive(Debug)]
pub(crate) struct ShellActivity {
    /// 区切りを 1 度でも見た（シェルが OSC 133 / 633 を出している）。
    seen: AtomicBool,
    /// コマンドが動いている（C の後で、D・次のプロンプト（A / B）の前）。
    running: AtomicBool,
    /// 直前に終わったコマンドの終了コード（[`NO_EXIT`] = 分からない）。
    exit: AtomicI64,
}

impl Default for ShellActivity {
    fn default() -> Self {
        Self {
            seen: AtomicBool::new(false),
            running: AtomicBool::new(false),
            exit: AtomicI64::new(NO_EXIT),
        }
    }
}

impl ShellActivity {
    pub(crate) fn seen(&self) -> bool {
        self.seen.load(Ordering::Relaxed)
    }

    pub(crate) fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub(crate) fn last_exit(&self) -> Option<i32> {
        i32::try_from(self.exit.load(Ordering::Relaxed)).ok()
    }

    pub(crate) fn apply(&self, mark: ShellMark) {
        self.seen.store(true, Ordering::Relaxed);
        match mark {
            // 次のプロンプトが出た＝前のコマンドは終わっている（D を出さないシェルもある）。
            ShellMark::PromptStart | ShellMark::CommandStart => {
                self.running.store(false, Ordering::Relaxed);
            }
            ShellMark::CommandExecuted => self.running.store(true, Ordering::Relaxed),
            ShellMark::CommandFinished(code) => {
                self.running.store(false, Ordering::Relaxed);
                self.exit
                    .store(code.map_or(NO_EXIT, i64::from), Ordering::Relaxed);
            }
        }
    }
}

/// OSC の頭を貯める長さの上限（`133;D;<終了コード>` が入れば足りる。後ろの引数は捨てる）。
const OSC_HEAD_LIMIT: usize = 24;

/// バイト列から OSC 133 / 633 の区切りだけを拾う小さな状態機械。読みの塊の境目で列が切れても
/// 続きから読む。ほかの OSC（リンク・クリップボード・タイトル等）は中身を貯めずに読み飛ばす。
#[derive(Default)]
pub(crate) struct ShellMarkScanner {
    state: MarkState,
    /// `ESC ]` の後の最初の数文字。
    head: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum MarkState {
    #[default]
    Ground,
    /// ESC を読んだ。
    Escape,
    /// `ESC ]` の後（区切りかもしれない間は頭を貯める）。
    Osc,
    /// 区切りでない OSC（終わりまで読み飛ばす）。
    OtherOsc,
    /// OSC の中で ESC を読んだ（次が `\` なら終わり＝ ST）。`relevant` = 区切りの OSC だった。
    OscEscape { relevant: bool },
}

impl ShellMarkScanner {
    /// `bytes` を読み、見つけた区切りを順に `on_mark` へ渡す。
    pub(crate) fn scan(&mut self, bytes: &[u8], mut on_mark: impl FnMut(ShellMark)) {
        for &byte in bytes {
            self.state = match self.state {
                MarkState::Ground | MarkState::Escape if byte == 0x1b => MarkState::Escape,
                MarkState::Ground => MarkState::Ground,
                MarkState::Escape if byte == b']' => self.start_osc(),
                MarkState::Escape => MarkState::Ground,
                MarkState::Osc | MarkState::OtherOsc => match byte {
                    // BEL で終わり。CAN / SUB は取り消し。
                    0x07 => {
                        if self.state == MarkState::Osc {
                            self.finish(&mut on_mark);
                        }
                        MarkState::Ground
                    }
                    0x18 | 0x1a => MarkState::Ground,
                    0x1b => MarkState::OscEscape {
                        relevant: self.state == MarkState::Osc,
                    },
                    _ if self.state == MarkState::OtherOsc => MarkState::OtherOsc,
                    _ => {
                        if self.head.len() < OSC_HEAD_LIMIT {
                            self.head.push(byte);
                        }
                        if self.may_be_mark() {
                            MarkState::Osc
                        } else {
                            MarkState::OtherOsc
                        }
                    }
                },
                MarkState::OscEscape { relevant } => match byte {
                    b'\\' => {
                        if relevant {
                            self.finish(&mut on_mark);
                        }
                        MarkState::Ground
                    }
                    // 終わらないまま次の列が始まった（前の OSC は捨てる）。
                    b']' => self.start_osc(),
                    0x1b => MarkState::Escape,
                    _ => MarkState::Ground,
                },
            };
        }
    }

    fn start_osc(&mut self) -> MarkState {
        self.head.clear();
        MarkState::Osc
    }

    /// 貯めた頭が、まだ `133;` / `633;` の区切りになりうるか。
    fn may_be_mark(&self) -> bool {
        [b"133;", b"633;"].iter().any(|prefix| {
            let length = self.head.len().min(prefix.len());
            self.head[..length] == prefix[..length]
        })
    }

    fn finish(&mut self, on_mark: &mut impl FnMut(ShellMark)) {
        if let Some(mark) = parse_shell_mark(&self.head) {
            on_mark(mark);
        }
        self.head.clear();
    }
}

/// OSC の中身（`133;A`・`133;D;1`・`633;C` …）を区切りに読む。ほかの種類（`633;E` 等）は `None`。
fn parse_shell_mark(head: &[u8]) -> Option<ShellMark> {
    let rest = head
        .strip_prefix(b"133;")
        .or_else(|| head.strip_prefix(b"633;"))?;
    let (&kind, parameters) = rest.split_first()?;
    let parameters = match parameters {
        [] => None,
        [b';', parameters @ ..] => Some(parameters),
        // `133;AB` のように 1 文字でない物は区切りでない。
        _ => return None,
    };
    match kind {
        b'A' => Some(ShellMark::PromptStart),
        b'B' => Some(ShellMark::CommandStart),
        b'C' => Some(ShellMark::CommandExecuted),
        b'D' => {
            let code = parameters.and_then(|parameters| {
                let first = parameters.split(|&byte| byte == b';').next()?;
                std::str::from_utf8(first).ok()?.parse::<i32>().ok()
            });
            Some(ShellMark::CommandFinished(code))
        }
        _ => None,
    }
}

/// [`KeyboardStackGuard`] を挟み、シェルの区切りを [`ShellActivity`] へ写す PTY。読み以外（書き・登録・
/// リサイズ・子の終了）は素通し。
pub(crate) struct GuardedPty<P> {
    pty: P,
    guard: KeyboardStackGuard,
    marks: ShellMarkScanner,
    activity: Arc<ShellActivity>,
}

impl<P> GuardedPty<P> {
    pub(crate) fn new(pty: P, activity: Arc<ShellActivity>) -> Self {
        Self {
            pty,
            guard: KeyboardStackGuard::default(),
            marks: ShellMarkScanner::default(),
            activity,
        }
    }
}

impl<P: EventedReadWrite> io::Read for GuardedPty<P> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.pty.reader().read(buffer)?;
        self.guard.filter(&mut buffer[..count]);
        let activity = &self.activity;
        self.marks
            .scan(&buffer[..count], |mark| activity.apply(mark));
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

    fn marks(chunks: &[&[u8]]) -> Vec<ShellMark> {
        let mut scanner = ShellMarkScanner::default();
        let mut found = Vec::new();
        for chunk in chunks {
            scanner.scan(chunk, |mark| found.push(mark));
        }
        found
    }

    /// O25・C20: OSC 133 / 633 の A〜D を、BEL でも ST でも、塊の境目で切れていても拾う。
    #[test]
    fn shell_marks_are_found_in_the_byte_stream() {
        assert_eq!(
            marks(&[b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07a b\r\n\x1b]133;D;0\x07"]),
            [
                ShellMark::PromptStart,
                ShellMark::CommandStart,
                ShellMark::CommandExecuted,
                ShellMark::CommandFinished(Some(0)),
            ]
        );
        // 1 バイトずつ届いても・ST（ESC \\）で終わっても・VS Code の 633 でも同じ。
        let split: Vec<&[u8]> = b"\x1b]633;C\x1b\\\x1b]133;D;127\x1b\\".chunks(1).collect();
        assert_eq!(
            marks(&split),
            [
                ShellMark::CommandExecuted,
                ShellMark::CommandFinished(Some(127))
            ]
        );
        // 後ろの引数（fish・kitty の `;click_events=1` 等）は読み捨てる。終了コードが無い D もある。
        assert_eq!(
            marks(&[
                b"\x1b]133;A;click_events=1;aid=1234567890abcdef\x07",
                b"\x1b]133;D\x07",
                b"\x1b]133;D;;aid=1\x07",
            ]),
            [
                ShellMark::PromptStart,
                ShellMark::CommandFinished(None),
                ShellMark::CommandFinished(None),
            ]
        );
    }

    /// ほかの OSC（タイトル・リンク・長いクリップボード）や区切りでない 133 は拾わない。途中で
    /// 取り消された列（CAN）・終わらずに次が始まった列も誤って拾わない。
    #[test]
    fn other_sequences_are_not_taken_for_shell_marks() {
        let clipboard = [b"\x1b]52;c;".as_slice(), &[b'Q'; 10_000], b"\x07"].concat();
        assert_eq!(
            marks(&[
                b"\x1b]0;133;C\x07",
                b"\x1b]8;;https://example.com/133;C\x1b\\link\x1b]8;;\x1b\\",
                &clipboard,
                b"\x1b]133;E;ls\x07\x1b]633;P;Cwd=/tmp\x07\x1b]133;AB\x07",
                b"\x1b]133;C\x18",
                b"133;C\x07 plain text \x07",
            ]),
            []
        );
        // 終わらないまま次の OSC が始まれば、前は捨てて後ろを読む。
        assert_eq!(
            marks(&[b"\x1b]133;C\x1b]133;D;2\x07"]),
            [ShellMark::CommandFinished(Some(2))]
        );
        let mut scanner = ShellMarkScanner::default();
        scanner.scan(&clipboard, |_| {});
        assert!(scanner.head.len() <= OSC_HEAD_LIMIT, "長い OSC を貯めない");
    }

    /// C で動いている・D / 次のプロンプトで止まる。終了コードは D の物。
    #[test]
    fn shell_activity_follows_the_marks() {
        let activity = ShellActivity::default();
        assert!(!activity.seen() && !activity.running());
        assert_eq!(activity.last_exit(), None);
        activity.apply(ShellMark::CommandExecuted);
        assert!(activity.seen() && activity.running());
        activity.apply(ShellMark::CommandFinished(Some(3)));
        assert!(!activity.running());
        assert_eq!(activity.last_exit(), Some(3));
        activity.apply(ShellMark::CommandExecuted);
        activity.apply(ShellMark::PromptStart);
        assert!(
            !activity.running(),
            "D を出さないシェルでも次のプロンプトで止まる"
        );
        activity.apply(ShellMark::CommandFinished(None));
        assert_eq!(activity.last_exit(), None);
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
