//! PTY のスモークテスト（WINDOWS-PORT.md §W4）。
//!
//! **UI を一切通さずに**、`alacritty_terminal` の PTY + EventLoop がこのプラットフォームで
//! 実際に動くか（シェルが起きて、書き込んだコマンドの出力が `Term` に届くか）を確かめる。
//!
//! ## なぜ要るか
//!
//! 2026-08-23、Windows で「ターミナルのドックは開くが中身が真っ黒」という状態に当たった。
//! `powershell.exe` は子プロセスとして生きていたので **PTY が悪いのか、前景への描画が悪いのか**が
//! 切り分けられず時間を溶かした。このテストが green なら **PTY は無実**で、疑うべきは
//! `TerminalView` の pump（`Wakeup` → `sync`）側だと即断できる。
//!
//! 組み立て方は `TerminalView::new_with_shell` と揃えてある（`Config` / `WindowSize` / `EventLoop`）。

use alacritty_terminal::event::{Event as AlacEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// PTY の起動と最初の出力までに許す時間。PowerShell の初回起動は 1〜3s かかることがある。
const DEADLINE: Duration = Duration::from_secs(45);

/// 出力に現れたら成功とみなす目印。シェルの banner や prompt に紛れない語にする。
const MARKER: &str = "NECODER_PTY_OK";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Size {
    columns: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

struct Recorder(Sender<AlacEvent>);

impl EventListener for Recorder {
    fn send_event(&self, event: AlacEvent) {
        // 受信側が畳まれていたら黙って捨てる（テスト終了時の正常系）。
        let _ = self.0.send(event);
    }
}

/// このプラットフォームで PTY に起こすシェル。`TerminalView` と同じ考え方。
///
/// Windows は `alacritty_terminal` の既定が `powershell` 固定なので明示する（`host` crate の
/// `pick_windows_shell` と揃える。あちらは `pwsh` を優先するが、ここは**どの機械でも必ず在る**
/// `powershell` に固定して、テストが環境で揺れないようにする）。
/// unix は `None` ＝ `$SHELL`（alacritty の既定）に任せる。
fn shell_for_platform() -> Option<tty::Shell> {
    if cfg!(windows) {
        Some(tty::Shell::new("powershell".to_string(), Vec::new()))
    } else {
        None
    }
}

/// `Term` の可視領域を 1 本の文字列にする。
fn visible_text(term: &Term<Recorder>) -> String {
    term.renderable_content()
        .display_iter
        .map(|indexed| indexed.cell.c)
        .collect()
}

#[test]
fn pty_starts_a_shell_and_delivers_its_output() {
    let size = Size {
        columns: 80,
        lines: 24,
    };
    let (events_tx, events_rx) = channel();
    let listener = Recorder(events_tx);
    let term = Arc::new(FairMutex::new(Term::new(
        Config::default(),
        &size,
        Recorder(listener.0.clone()),
    )));

    let mut env = HashMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    let options = tty::Options {
        shell: shell_for_platform(),
        working_directory: None,
        drain_on_exit: true,
        env,
        ..Default::default()
    };
    let window_size = WindowSize {
        num_lines: size.lines as u16,
        num_cols: size.columns as u16,
        cell_width: 8,
        cell_height: 16,
    };

    let pty = tty::new(&options, window_size, 0).expect("PTY を作れない（ConPTY / posix_openpt）");
    let event_loop =
        EventLoop::new(term.clone(), listener, pty, true, false).expect("EventLoop を作れない");
    let notifier = Notifier(event_loop.channel());
    let io_thread = event_loop.spawn();

    // シェルが起きてプロンプトを出すまで少し待ってからコマンドを流す
    // （起動途中に書くと ConPTY 側で取りこぼすことがある）。
    std::thread::sleep(Duration::from_millis(1500));
    notifier.notify(format!("echo {MARKER}\r\n").into_bytes());

    let deadline = Instant::now() + DEADLINE;
    let mut wakeups = 0_usize;
    let mut found = false;
    while Instant::now() < deadline {
        // Wakeup が来たら（＝PTY が何か吐いた）画面を覗く。
        match events_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(AlacEvent::Wakeup) => wakeups += 1,
            Ok(_) => {}
            Err(_) => {}
        }
        if visible_text(&term.lock()).contains(MARKER) {
            found = true;
            break;
        }
    }

    let screen = visible_text(&term.lock());
    let _ = notifier.0.send(Msg::Shutdown);
    let _ = io_thread.join();

    assert!(
        found,
        "PTY からの出力に `{MARKER}` が現れなかった（Wakeup {wakeups} 回）。\n\
         PTY 自体が動いていない可能性が高い。画面の中身:\n{}",
        screen.trim_end()
    );
    assert!(
        wakeups > 0,
        "画面には出ているのに Wakeup が 1 度も来ていない＝EventListener の経路が切れている"
    );
}
