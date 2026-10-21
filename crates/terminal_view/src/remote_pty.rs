//! 接続先の端末（PTY は接続先の daemon が持ち、入出力は RPC のストリームで届く・
//! `docs/REMOTE-CHANNEL.md` §1）を、alacritty の `EventLoop` が読める「PTY」に見せる。
//!
//! `EventLoop` は poller に登録できる読み口を要る（`EventedReadWrite`）。そこで手元に全二重の
//! socket の対を作り、片側を `EventLoop` に渡し、もう片側と RPC のストリームの間を背景のスレッドで
//! 中継する。終わりは別の対に 1 byte 書いて知らせる（alacritty の unix 版が SIGCHLD の pipe で
//! 知らせるのと同じ形）。以前は手元の PTY で `ssh -tt` を動かしていた（Windows では ControlMaster が
//! 無く動かず、接続が切れるとシェルごと死んでいた）。
//!
//! 開くのは背景で行う（接続を待つので UI スレッドに載せない）。開くまでに打った鍵は socket に溜まる。

use std::io::{self, Read, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, PollMode, Poller};

/// `EventLoop` が見るキー（alacritty_terminal 0.26 の `tty::PTY_READ_WRITE_TOKEN` /
/// `PTY_CHILD_EVENT_TOKEN`）。**OS で値が違う**（unix は 0 / 1・Windows は 2 / 1）。unix 版は crate の
/// 外へ公開されていないので値を写す。違うキーで知らせると EventLoop は黙って捨てる（2026-10-21 に
/// Windows で踏んだ: 画面が空のまま、打鍵も届かなかった）。版を上げたら確かめる。
#[cfg(unix)]
const READ_WRITE_KEY: usize = 0;
#[cfg(windows)]
const READ_WRITE_KEY: usize = alacritty_terminal::tty::PTY_READ_WRITE_TOKEN;
#[cfg(unix)]
const CHILD_EVENT_KEY: usize = 1;
#[cfg(windows)]
const CHILD_EVENT_KEY: usize = alacritty_terminal::tty::PTY_CHILD_EVENT_TOKEN;

/// シェルが終わってから、出力の終わりを待つ上限（バックグラウンドの子が端末を握っていると
/// 出力は終わらない。手元の端末と同じく、シェルが終われば閉じる）。
const EXIT_DRAIN: Duration = Duration::from_millis(300);

#[cfg(unix)]
type PairStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type PairStream = std::net::TcpStream;

/// 全二重の対。unix は Unix socket の対、Windows は loopback TCP の対（Windows の poller が
/// 待てるのは socket だけ）。TCP は待ち受けに他人が先に繋いでいないかを相手の番号で確かめる。
fn stream_pair() -> io::Result<(PairStream, PairStream)> {
    #[cfg(unix)]
    {
        PairStream::pair()
    }
    #[cfg(windows)]
    {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let ours = PairStream::connect(listener.local_addr()?)?;
        let expected = ours.local_addr()?;
        loop {
            let (theirs, peer) = listener.accept()?;
            if peer == expected {
                ours.set_nodelay(true)?;
                theirs.set_nodelay(true)?;
                return Ok((ours, theirs));
            }
            // 同じ番号へ別のプロセスが先に繋いだ。それは捨てて、自分の接続を待つ。
            drop(theirs);
        }
    }
}

/// 終わりの知らせ（1 回だけ）。
struct ExitNotice {
    status: Mutex<Option<ExitStatus>>,
    sent: AtomicBool,
    signal: Mutex<Option<PairStream>>,
}

impl ExitNotice {
    fn send(&self, exit: Option<host::ProcessExit>) {
        if self.sent.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut status) = self.status.lock() {
            *status = exit.map(exit_status);
        }
        let signal = self.signal.lock().ok().and_then(|mut signal| signal.take());
        if let Some(mut signal) = signal {
            if let Err(error) = signal.write_all(&[1]) {
                eprintln!("ターミナル: 接続先の端末の終わりを知らせられない: {error}");
            }
        }
    }
}

/// 接続先の終わり方を、手元の `ExitStatus` の形にする（alacritty の `ChildEvent` がこれを取る）。
fn exit_status(exit: host::ProcessExit) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        match (exit.code, exit.signal) {
            (Some(code), _) => ExitStatus::from_raw((code & 0xff) << 8),
            (None, Some(signal)) => ExitStatus::from_raw(signal & 0x7f),
            (None, None) => ExitStatus::from_raw(0),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt as _;
        ExitStatus::from_raw(exit.code.unwrap_or(1) as u32)
    }
}

/// [`crate::TerminalView`] の `EventLoop` に渡す「PTY」。drop すると接続先の端末を閉じる。
pub(crate) struct RemotePty {
    /// `EventLoop` が読み書きする側（非ブロッキング）。
    data: PairStream,
    /// 終わりの知らせを読む側（非ブロッキング）。
    exit_signal: PairStream,
    exit: Arc<ExitNotice>,
    resize: mpsc::Sender<host::PtySize>,
}

/// 中継の側の口（[`RemotePty::pair`] の片割れ）。
struct RelayEnds {
    /// `EventLoop` の反対側（画面のバイトを書き、打鍵を読む）。
    relay: PairStream,
    resizes: mpsc::Receiver<host::PtySize>,
    exit: Arc<ExitNotice>,
}

impl RemotePty {
    /// 背景で接続先の端末を開き、中継を始める。開けなかったら `failure` に理由を付けて画面に出し、終わる。
    pub(crate) fn start(
        terminal: host::RemoteTerminal,
        size: host::PtySize,
        failure: impl Fn(&str) -> String + Send + 'static,
    ) -> io::Result<Self> {
        let (pty, ends) = Self::pair()?;
        thread::Builder::new()
            .name("necoder-remote-terminal".to_string())
            .spawn(move || relay_terminal(terminal, size, ends, failure))?;
        Ok(pty)
    }

    /// `EventLoop` に渡す側と、中継の側の口を作る。
    fn pair() -> io::Result<(Self, RelayEnds)> {
        let (data, relay) = stream_pair()?;
        let (exit_signal, exit_writer) = stream_pair()?;
        data.set_nonblocking(true)?;
        exit_signal.set_nonblocking(true)?;
        let exit = Arc::new(ExitNotice {
            status: Mutex::new(None),
            sent: AtomicBool::new(false),
            signal: Mutex::new(Some(exit_writer)),
        });
        let (resize, resizes) = mpsc::channel();
        Ok((
            Self {
                data,
                exit_signal,
                exit: exit.clone(),
                resize,
            },
            RelayEnds {
                relay,
                resizes,
                exit,
            },
        ))
    }
}

/// 開いて、画面 → socket・socket → 打鍵の中継を立て、大きさの変更を流し続ける。`RemotePty` が
/// drop されると大きさの口が閉じ、ここで端末（[`host::HostPty`]）を drop する＝接続先のシェルを閉じる。
fn relay_terminal(
    terminal: host::RemoteTerminal,
    size: host::PtySize,
    ends: RelayEnds,
    failure: impl Fn(&str) -> String,
) {
    let RelayEnds {
        mut relay,
        resizes,
        exit,
    } = ends;
    crate::term_probe("remote", format_args!("{} を開く", terminal.label()));
    let pty = match terminal.open(size) {
        Ok(pty) => {
            crate::term_probe("remote", "開いた");
            pty
        }
        Err(error) => {
            crate::term_probe("remote", format_args!("開けない: {error:#}"));
            let message = failure(&format!("{error:#}"));
            if let Err(error) = relay.write_all(message.as_bytes()) {
                eprintln!("ターミナル: 開けなかった理由を出せない: {error}");
            }
            exit.send(None);
            return;
        }
    };
    let watch = pty.exit_watch();
    let output = {
        let mut screen = pty.reader();
        let exit = exit.clone();
        let watch = watch.clone();
        let mut relay = match relay.try_clone() {
            Ok(relay) => relay,
            Err(error) => {
                eprintln!("ターミナル: 中継の口を複製できない: {error}");
                exit.send(None);
                return;
            }
        };
        thread::Builder::new()
            .name("necoder-remote-terminal-output".to_string())
            .spawn(move || {
                let mut chunk = [0u8; 32 * 1024];
                loop {
                    match screen.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if relay.write_all(&chunk[..read]).is_err() {
                                break;
                            }
                        }
                    }
                }
                exit.send(watch.wait(EXIT_DRAIN));
            })
    };
    // シェルが終わったのに出力が終わらない（バックグラウンドの子が端末を握っている）時も閉じる。
    let exited = {
        let exit = exit.clone();
        thread::Builder::new()
            .name("necoder-remote-terminal-exit".to_string())
            .spawn(move || {
                const PATIENCE: Duration = Duration::from_secs(3600);
                loop {
                    let waited = std::time::Instant::now();
                    if let Some(status) = watch.wait(PATIENCE) {
                        thread::sleep(EXIT_DRAIN);
                        exit.send(Some(status));
                        return;
                    }
                    // 待ち切る前に返った＝手元が端末を閉じた。もう知らせる相手は居ない。
                    if exit.sent.load(Ordering::Acquire) || waited.elapsed() < PATIENCE {
                        return;
                    }
                }
            })
    };
    let input = {
        let mut keyboard = pty.writer();
        thread::Builder::new()
            .name("necoder-remote-terminal-input".to_string())
            .spawn(move || {
                let mut chunk = [0u8; 4096];
                loop {
                    match relay.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if keyboard.write_all(&chunk[..read]).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
    };
    for spawned in [output, exited, input] {
        if let Err(error) = spawned {
            eprintln!("ターミナル: 接続先の端末の中継を起動できない: {error}");
            exit.send(None);
            return;
        }
    }
    for size in resizes {
        pty.resize(size);
    }
    // `RemotePty` が drop された（端末を閉じた）。ここで pty を drop して接続先のシェルを閉じる。
    drop(pty);
}

impl EventedReadWrite for RemotePty {
    type Reader = PairStream;
    type Writer = PairStream;

    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        interest.key = READ_WRITE_KEY;
        // SAFETY: 登録する 2 つの socket はこの値が持ち、`EventLoop` が deregister するまで生きている
        // （`EventLoop` は終わる時に deregister を呼ぶ）。
        let registered = unsafe {
            poll.add_with_mode(&self.data, interest, mode)
                .and_then(|()| {
                    poll.add_with_mode(
                        &self.exit_signal,
                        Event::readable(CHILD_EVENT_KEY),
                        PollMode::Level,
                    )
                })
        };
        // alacritty は登録の失敗を `log` へ出すだけで黙って EventLoop を畳む（necoder は `log` を拾わない）。
        if let Err(error) = &registered {
            eprintln!("ターミナル: 接続先の端末を poller に登録できない: {error}");
        }
        registered
    }

    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        mode: PollMode,
    ) -> io::Result<()> {
        interest.key = READ_WRITE_KEY;
        let modified = poll
            .modify_with_mode(&self.data, interest, mode)
            .and_then(|()| {
                poll.modify_with_mode(
                    &self.exit_signal,
                    Event::readable(CHILD_EVENT_KEY),
                    PollMode::Level,
                )
            });
        if let Err(error) = &modified {
            eprintln!("ターミナル: 接続先の端末の待ち方を変えられない: {error}");
        }
        modified
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        poll.delete(&self.data)?;
        poll.delete(&self.exit_signal)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.data
    }

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.data
    }
}

impl EventedPty for RemotePty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        let mut byte = [0u8; 1];
        match self.exit_signal.read(&mut byte) {
            Ok(1) => {
                let status = self.exit.status.lock().ok().and_then(|status| *status);
                Some(ChildEvent::Exited(status))
            }
            Ok(_) => None,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
            Err(error) => {
                eprintln!("ターミナル: 接続先の端末の終わりを読めない: {error}");
                None
            }
        }
    }
}

impl OnResize for RemotePty {
    fn on_resize(&mut self, window_size: WindowSize) {
        let size = host::PtySize {
            columns: window_size.num_cols,
            lines: window_size.num_lines,
            pixel_width: window_size.num_cols.saturating_mul(window_size.cell_width),
            pixel_height: window_size
                .num_lines
                .saturating_mul(window_size.cell_height),
        };
        // 中継が先に終わっている（開けなかった・終わった）なら送る先は無い。
        let _closed = self.resize.send(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 開くまで・開いた後の打鍵が届き、画面の出力が読め、終わりが `ChildEvent` で届く
    /// （接続先の代わりに手元の host で同じ protocol を喋る端末）。
    #[test]
    fn a_failed_open_is_shown_and_ends_the_terminal() {
        let terminal = host::RemoteTerminal::from_opener("test", |_size| {
            Err(anyhow::anyhow!("接続先に繋がらない（試験）"))
        });
        let mut pty = RemotePty::start(terminal, host::PtySize::default(), |error| {
            format!("[開けなかった: {error}]")
        })
        .expect("中継を立てられる");
        let mut seen = String::new();
        let mut chunk = [0u8; 256];
        let started = std::time::Instant::now();
        while !seen.contains("繋がらない") {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "理由が出ない: {seen}"
            );
            match pty.data.read(&mut chunk) {
                Ok(read) => seen.push_str(&String::from_utf8_lossy(&chunk[..read])),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("読めない: {error}"),
            }
        }
        let event = (0..500).find_map(|_| {
            let event = pty.next_child_event();
            if event.is_none() {
                thread::sleep(Duration::from_millis(10));
            }
            event
        });
        assert!(
            matches!(event, Some(ChildEvent::Exited(None))),
            "終わりが届く"
        );
    }

    /// alacritty の EventLoop を通して、**端末が生きている間に**中継のバイトが画面に出て、打鍵が中継へ
    /// 届き、終わりの知らせで止まる（Windows は loopback TCP を poller で待つ・unix は Unix socket。
    /// キーを取り違えると「終わる時の読み切り」でしか読まれない＝この試験が落ちる）。
    #[test]
    fn the_event_loop_relays_both_ways_while_the_terminal_lives() {
        use alacritty_terminal::event::{Event as AlacEvent, EventListener, Notify as _};
        use alacritty_terminal::event_loop::{EventLoop, Notifier};
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::sync::FairMutex;
        use alacritty_terminal::term::{Config, Term};

        #[derive(Clone)]
        struct Events(mpsc::Sender<AlacEvent>);
        impl EventListener for Events {
            fn send_event(&self, event: AlacEvent) {
                let _closed = self.0.send(event);
            }
        }
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

        let (pty, ends) = RemotePty::pair().expect("対を作れる");
        let RelayEnds {
            mut relay,
            resizes: _resizes,
            exit,
        } = ends;
        let (sender, events) = mpsc::channel();
        let listener = Events(sender);
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &Size,
            listener.clone(),
        )));
        // 本番と同じく区切りを拾う包み（`pty_guard`）を挟む。
        let guarded = crate::pty_guard::GuardedPty::new(
            pty,
            Arc::new(crate::pty_guard::ShellActivity::default()),
        );
        let event_loop = EventLoop::new(term.clone(), listener, guarded, true, false)
            .expect("EventLoop を作れる");
        let notifier = Notifier(event_loop.channel());
        let _thread = event_loop.spawn();

        // EventLoop が待ち始めた後に、接続先の画面のバイトが届く。
        thread::sleep(Duration::from_millis(200));
        relay.write_all(b"remote-prompt$ ").expect("中継へ書ける");
        let started = std::time::Instant::now();
        loop {
            let text: String = {
                let term = term.lock();
                let grid = term.grid();
                (0..80)
                    .map(|column| grid[Line(0)][Column(column)].c)
                    .collect()
            };
            if text.contains("remote-prompt$") {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "生きている端末で、中継のバイトが画面に出ない: {text:?}"
            );
            let _event = events.recv_timeout(Duration::from_millis(50));
        }

        // 打鍵は EventLoop から中継へ届く。
        notifier.notify(&b"ls\r"[..]);
        relay
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("待ち時間を決められる");
        let mut typed = [0u8; 3];
        relay.read_exact(&mut typed).expect("打鍵が中継に届く");
        assert_eq!(&typed, b"ls\r");

        // 終わりの知らせで EventLoop が端末を閉じる。
        exit.send(Some(host::ProcessExit {
            code: Some(0),
            signal: None,
        }));
        let exited = (0..100).any(|_| {
            matches!(
                events.recv_timeout(Duration::from_millis(50)),
                Ok(AlacEvent::Exit)
            )
        });
        assert!(exited, "終わりで EventLoop が止まる");
    }
}
