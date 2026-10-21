//! 接続先（daemon）側のストリーム。プロセス・PTY・TCP を持ち、client との接続が切れても生かしておく。
//! 設計は `docs/REMOTE-CHANNEL.md` §2、wire の型は `stream.rs`。
//!
//! 送り手として「Ack の来ていない出力」を控えに持つ（窓 = [`STREAM_WINDOW`]）。client が繋ぎ直して
//! Attach したら、受け取ったと言う位置から送り直す。

use crate::stream::{
    ProcessExit, PtySize, StreamKind, StreamMessage, CHANNEL_ERROR, CHANNEL_INPUT, CHANNEL_OUTPUT,
    STREAM_ACK_EVERY, STREAM_CHUNK, STREAM_WINDOW,
};
use crate::{write_frame, CommandSpec, Frame};
use anyhow::{anyhow, bail, Context as _, Result};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

/// 止めてから強制終了するまでの猶予（行儀よく終われる時間）。
const STOP_GRACE: Duration = Duration::from_secs(2);
/// `ne` の open を client に頼んでから待つ上限。
#[cfg(unix)]
const CLI_OPEN_TIMEOUT: Duration = Duration::from_secs(60);

/// client との 1 接続の書き口。daemon は 1 つの session に 1 つだけ持つ（新しい接続が来たら古い方を切る）。
pub(crate) struct Link {
    writer: Mutex<Box<dyn Write + Send>>,
    alive: AtomicBool,
    /// 接続そのものを切る口（daemon が新しい接続に乗り換える時に、古い接続の読みを終わらせる）。
    shutdown: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Link {
    pub(crate) fn new(
        writer: Box<dyn Write + Send>,
        shutdown: Option<Box<dyn FnOnce() + Send>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            writer: Mutex::new(writer),
            alive: AtomicBool::new(true),
            shutdown: Mutex::new(shutdown),
        })
    }

    pub(crate) fn write(&self, frame: &Frame) -> Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            bail!("client との接続は切れている");
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("remote writer lock poisoned"))?;
        write_frame(&mut **writer, frame).inspect_err(|_| {
            self.alive.store(false, Ordering::Release);
        })
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// 接続を切る（読みのループが終わる）。
    pub(crate) fn close(&self) {
        self.alive.store(false, Ordering::Release);
        let shutdown = self.shutdown.lock().ok().and_then(|mut slot| slot.take());
        if let Some(shutdown) = shutdown {
            shutdown();
        }
    }
}

/// daemon が持つストリームの全部と、いま繋がっている client。
#[derive(Default)]
pub(crate) struct ServerStreams {
    entries: Mutex<HashMap<u64, Arc<ServerStream>>>,
    current: Mutex<Option<Arc<Link>>>,
    cli_waiting: Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>,
    #[cfg(unix)]
    next_cli_request: std::sync::atomic::AtomicU64,
    /// 端末に足す環境変数（`ne` の shim の PATH と受け口・daemon だけが設定する）。
    #[cfg(unix)]
    terminal_env: Mutex<Vec<(String, String)>>,
}

impl ServerStreams {
    /// 新しい接続をいまの client にする。前の接続は切る（1 session = 1 client・最新が勝つ）。
    pub(crate) fn connect(&self, link: &Arc<Link>) {
        let previous = self
            .current
            .lock()
            .ok()
            .and_then(|mut current| current.replace(link.clone()));
        if let Some(previous) = previous {
            if !Arc::ptr_eq(&previous, link) {
                previous.close();
            }
        }
    }

    /// 接続が終わった。そこに付いていたストリームを外す（TCP は閉じる）。
    pub(crate) fn disconnect(&self, link: &Arc<Link>) {
        link.close();
        if let Ok(mut current) = self.current.lock() {
            if current
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, link))
            {
                *current = None;
            }
        }
        for stream in self.snapshot() {
            if !stream.is_on(link) {
                continue;
            }
            if stream.kind == StreamKind::Tcp {
                self.close_stream(&stream);
            } else {
                stream.detach();
            }
        }
    }

    #[cfg(unix)]
    pub(crate) fn has_client(&self) -> bool {
        self.current
            .lock()
            .map(|current| current.as_ref().is_some_and(|link| link.is_alive()))
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn current_link(&self) -> Option<Arc<Link>> {
        self.current.lock().ok().and_then(|current| current.clone())
    }

    fn snapshot(&self) -> Vec<Arc<ServerStream>> {
        self.entries
            .lock()
            .map(|entries| entries.values().cloned().collect())
            .unwrap_or_default()
    }

    fn get(&self, id: u64) -> Option<Arc<ServerStream>> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&id).cloned())
    }

    fn insert(&self, stream: Arc<ServerStream>) -> Result<()> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow!("stream table lock poisoned"))?;
        if entries.contains_key(&stream.id) {
            bail!("stream id {} は使われている", stream.id);
        }
        entries.insert(stream.id, stream);
        Ok(())
    }

    #[cfg(unix)]
    pub(crate) fn set_terminal_env(&self, env: Vec<(String, String)>) {
        if let Ok(mut slot) = self.terminal_env.lock() {
            *slot = env;
        }
    }

    /// 接続の読みのスレッドから呼ぶ。**ブロックしない**。
    pub(crate) fn dispatch(&self, frame: Frame) {
        let Ok(message) = serde_json::from_slice::<StreamMessage>(&frame.meta) else {
            eprintln!("necoder-remote-server: 読めないストリームの frame を捨てた");
            return;
        };
        let Some(stream) = self.get(frame.id) else {
            return;
        };
        match message {
            StreamMessage::Data { channel, offset } if channel == CHANNEL_INPUT => {
                stream.receive_input(offset, frame.body);
            }
            StreamMessage::Eof { channel, offset } if channel == CHANNEL_INPUT => {
                stream.receive_input_eof(offset);
            }
            StreamMessage::Ack { channel, offset } => stream.receive_ack(channel, offset),
            StreamMessage::Resize(size) => stream.resize(size),
            StreamMessage::Signal { name } => stream.signal(&name),
            StreamMessage::Close => self.close_stream(&stream),
            StreamMessage::Data { .. }
            | StreamMessage::Eof { .. }
            | StreamMessage::Exit(_)
            | StreamMessage::Lost { .. } => {}
        }
    }

    fn close_stream(&self, stream: &Arc<ServerStream>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&stream.id);
        }
        stream.close();
    }

    /// 全部止める（daemon の終わり・`serve --stdio` の接続の終わり）。
    pub(crate) fn close_all(&self) {
        let streams: Vec<Arc<ServerStream>> = self
            .entries
            .lock()
            .map(|mut entries| entries.drain().map(|(_, stream)| stream).collect())
            .unwrap_or_default();
        for stream in streams {
            stream.close();
        }
    }

    /// client が繋ぎ直した。`link` へ付け替え、受け取ったと言う位置から送り直す。
    /// 戻り値は daemon が受け取った入力の位置（client はそこから送り直す）。
    pub(crate) fn attach(
        &self,
        link: &Arc<Link>,
        stream_id: u64,
        output_offset: u64,
        error_offset: u64,
    ) -> Result<(u64, Arc<ServerStream>)> {
        let stream = self
            .get(stream_id)
            .with_context(|| format!("stream {stream_id} はもう無い"))?;
        let input_offset = stream.attach(link, [output_offset, error_offset]);
        Ok((input_offset, stream))
    }

    /// パイプの stdio でプロセスを起こす（LSP・ACP・人のコマンド）。`spec.cwd` は解決済みの絶対パス。
    pub(crate) fn spawn_process(
        &self,
        link: &Arc<Link>,
        stream_id: u64,
        spec: &CommandSpec,
    ) -> Result<()> {
        let mut command = child_process::command(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .envs(&spec.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // グループごと止められるよう自分のグループを持たせる（LSP の子・`npm` の孫も止まる）。
            command.process_group(0);
            // SAFETY: `reset_child_signals` は async-signal-safe な libc の呼び出しだけで出来ている。
            unsafe {
                command.pre_exec(crate::reset_child_signals);
            }
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("process を起動できない: {}", spec.program))?;
        let stdin = child.stdin.take().context("process stdin が無い")?;
        let stdout = child.stdout.take().context("process stdout が無い")?;
        let stderr = child.stderr.take().context("process stderr が無い")?;
        let stream = ServerStream::new(
            stream_id,
            StreamKind::Process,
            link,
            Control::process(&child),
        );
        if let Err(error) = self.insert(stream.clone()) {
            stop_child(child);
            return Err(error);
        }
        stream.start_input(Box::new(stdin))?;
        spawn_named("necoder-stream-stdout", {
            let stream = stream.clone();
            move || stream.pump_output(0, stdout)
        })?;
        spawn_named("necoder-stream-stderr", {
            let stream = stream.clone();
            move || stream.pump_output(1, stderr)
        })?;
        spawn_named("necoder-stream-wait", move || stream.wait_child(child))?;
        Ok(())
    }

    /// ログインシェルを PTY で起こす（端末）。`cwd` は解決済みの絶対パス。
    #[cfg(unix)]
    pub(crate) fn open_terminal(
        &self,
        link: &Arc<Link>,
        stream_id: u64,
        cwd: &std::path::Path,
        size: PtySize,
        env: &HashMap<String, String>,
    ) -> Result<()> {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::process::CommandExt as _;

        let (master, slave) = open_pty(size)?;
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string());
        let mut command = std::process::Command::new(&shell);
        command
            .arg("-l")
            .current_dir(cwd)
            .envs(env)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        if let Ok(extra) = self.terminal_env.lock() {
            for (key, value) in extra.iter() {
                command.env(key, value);
            }
        }
        // SAFETY: 子の側で async-signal-safe な libc の関数だけを呼ぶ。setsid で新しいセッションの
        // 長になり、標準入力（= slave）を制御端末にしてから、シグナルを既定へ戻す。
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                crate::reset_child_signals()
            });
        }
        let child = command
            .spawn()
            .with_context(|| format!("端末のシェルを起動できない: {shell}"))?;
        // slave の複製は Command が持っている。親の側で閉じないと、シェルが終わっても読みが EIO にならない。
        drop(command);
        let reader = std::fs::File::from(master.try_clone()?);
        let writer = std::fs::File::from(master.try_clone()?);
        let control = Control::terminal(&child, master);
        let stream = ServerStream::new(stream_id, StreamKind::Terminal, link, control);
        if let Err(error) = self.insert(stream.clone()) {
            stop_child(child);
            return Err(error);
        }
        remote_stream_trace(&format!(
            "端末を開いた: stream {stream_id} fd {}",
            reader.as_raw_fd()
        ));
        stream.start_input(Box::new(writer))?;
        spawn_named("necoder-stream-pty", {
            let stream = stream.clone();
            move || stream.pump_output(0, PtyReader(reader))
        })?;
        spawn_named("necoder-stream-wait", move || stream.wait_child(child))?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub(crate) fn open_terminal(
        &self,
        _link: &Arc<Link>,
        _stream_id: u64,
        _cwd: &std::path::Path,
        _size: PtySize,
        _env: &HashMap<String, String>,
    ) -> Result<()> {
        bail!("この remote-server は端末を開けない（接続先は unix のみ）")
    }

    /// 接続先の 127.0.0.1:`port` へ TCP で繋ぐ（ポート転送の 1 本）。
    pub(crate) fn open_tcp(&self, link: &Arc<Link>, stream_id: u64, port: u16) -> Result<()> {
        let socket = connect_local_port(port)?;
        let reader = socket.try_clone()?;
        let writer = TcpWriteHalf(socket.try_clone()?);
        let stream = ServerStream::new(stream_id, StreamKind::Tcp, link, Control::Tcp(socket));
        self.insert(stream.clone())?;
        stream.start_input(Box::new(writer))?;
        spawn_named("necoder-stream-tcp", move || stream.pump_output(0, reader))?;
        Ok(())
    }

    /// `ne` の open を client に頼み、client（= 接続元の GUI）の答えを返す。
    #[cfg(unix)]
    pub(crate) fn cli_open(&self, paths: Vec<String>) -> serde_json::Value {
        let Some(link) = self.current_link().filter(|link| link.is_alive()) else {
            return serde_json::json!({
                "ok": false,
                "error": "接続元の necoder が繋がっていません",
            });
        };
        let request = self.next_cli_request.fetch_add(1, Ordering::Relaxed) + 1;
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut waiting) = self.cli_waiting.lock() {
            waiting.insert(request, sender);
        }
        let sent = serde_json::to_vec(&crate::ServerEvent::CliOpen { request, paths })
            .map_err(anyhow::Error::from)
            .and_then(|meta| {
                link.write(&Frame {
                    kind: crate::FrameKind::Event,
                    id: 0,
                    meta,
                    body: Vec::new(),
                })
            });
        let answer = match sent {
            Ok(()) => receiver.recv_timeout(CLI_OPEN_TIMEOUT).unwrap_or_else(
                |_| serde_json::json!({ "ok": false, "error": "接続元の necoder から答えが無い" }),
            ),
            Err(error) => serde_json::json!({
                "ok": false,
                "error": format!("接続元へ頼めない: {error:#}"),
            }),
        };
        if let Ok(mut waiting) = self.cli_waiting.lock() {
            waiting.remove(&request);
        }
        answer
    }

    /// client からの `ne` の open の答え（[`crate::Request::CliOpenResult`]）。
    pub(crate) fn cli_result(&self, request: u64, response: serde_json::Value) {
        let sender = self
            .cli_waiting
            .lock()
            .ok()
            .and_then(|mut waiting| waiting.remove(&request));
        if let Some(sender) = sender {
            let _late = sender.send(response);
        }
    }
}

fn spawn_named(name: &str, body: impl FnOnce() + Send + 'static) -> Result<()> {
    thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
        .map(|_| ())
        .with_context(|| format!("{name} スレッドを起動できない"))
}

fn stop_child(mut child: std::process::Child) {
    let _gone = child.kill();
    let _reaped = child.wait();
}

/// ストリームの持ち物（止める・大きさを変える口）。
enum Control {
    /// パイプのプロセス。unix では `pid` が自分のプロセスグループの長（回収したら 0）。
    Process {
        pid: u32,
    },
    /// 端末。`master` は大きさを変える口。
    #[cfg(unix)]
    Terminal {
        pid: u32,
        master: std::os::fd::OwnedFd,
    },
    Tcp(std::net::TcpStream),
}

impl Control {
    fn process(child: &std::process::Child) -> Self {
        Self::Process { pid: child.id() }
    }

    #[cfg(unix)]
    fn terminal(child: &std::process::Child, master: std::os::fd::OwnedFd) -> Self {
        Self::Terminal {
            pid: child.id(),
            master,
        }
    }
}

#[derive(Default)]
struct Outbound {
    /// Ack の来ていない出力（`base` から）。
    buffer: VecDeque<u8>,
    base: u64,
    /// いまの接続に送った位置。
    sent: u64,
    eof: bool,
    eof_sent: bool,
}

#[derive(Default)]
struct StreamState {
    /// 0 = 出力・1 = stderr。
    outputs: [Outbound; 2],
    input_received: u64,
    input_written: u64,
    input_acked: u64,
    input_eof: bool,
    exit: Option<ProcessExit>,
    exit_sent: bool,
    closed: bool,
}

enum InputChunk {
    Data(Vec<u8>),
    Eof,
}

pub(crate) struct ServerStream {
    id: u64,
    kind: StreamKind,
    /// 送る先。切れている間は `None`（出力は控えに溜まる）。
    link: Mutex<Option<Arc<Link>>>,
    state: Mutex<StreamState>,
    changed: Condvar,
    /// 出力を送る順番を守る（pump と、Attach 後の送り直しが交差しない）。
    send_lock: Mutex<()>,
    input: Mutex<Option<mpsc::Sender<InputChunk>>>,
    control: Mutex<Control>,
}

impl ServerStream {
    fn new(id: u64, kind: StreamKind, link: &Arc<Link>, control: Control) -> Arc<Self> {
        Arc::new(Self {
            id,
            kind,
            link: Mutex::new(Some(link.clone())),
            state: Mutex::new(StreamState::default()),
            changed: Condvar::new(),
            send_lock: Mutex::new(()),
            input: Mutex::new(None),
            control: Mutex::new(control),
        })
    }

    fn lock(&self) -> MutexGuard<'_, StreamState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn link(&self) -> Option<Arc<Link>> {
        self.link
            .lock()
            .ok()
            .and_then(|link| link.clone())
            .filter(|link| link.is_alive())
    }

    fn is_on(&self, link: &Arc<Link>) -> bool {
        self.link
            .lock()
            .map(|current| {
                current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, link))
            })
            .unwrap_or(false)
    }

    fn detach(&self) {
        if let Ok(mut link) = self.link.lock() {
            *link = None;
        }
        self.changed.notify_all();
    }

    fn attach(&self, link: &Arc<Link>, received: [u64; 2]) -> u64 {
        if let Ok(mut slot) = self.link.lock() {
            *slot = Some(link.clone());
        }
        let _order = self
            .send_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = self.lock();
        for (output, received) in state.outputs.iter_mut().zip(received) {
            if received > output.base {
                let drop_count = ((received - output.base) as usize).min(output.buffer.len());
                output.buffer.drain(..drop_count);
                output.base += drop_count as u64;
            }
            output.sent = output.base;
            output.eof_sent = false;
        }
        state.exit_sent = false;
        let input_offset = state.input_received;
        drop(state);
        self.changed.notify_all();
        input_offset
    }

    /// 入力の書き手（プロセスの stdin・PTY・TCP の上り）を立てる。
    fn start_input(self: &Arc<Self>, sink: Box<dyn Write + Send>) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut input) = self.input.lock() {
            *input = Some(sender);
        }
        let stream = self.clone();
        spawn_named("necoder-stream-stdin", move || {
            stream.pump_input(receiver, sink)
        })
    }

    fn pump_input(&self, receiver: mpsc::Receiver<InputChunk>, sink: Box<dyn Write + Send>) {
        let mut sink = Some(sink);
        for chunk in receiver {
            match chunk {
                InputChunk::Data(bytes) => {
                    if let Some(writer) = sink.as_mut() {
                        // 相手が読まずに終わった（EPIPE）。以後の入力は捨てる（窓は進める）。
                        if writer
                            .write_all(&bytes)
                            .and_then(|()| writer.flush())
                            .is_err()
                        {
                            sink = None;
                        }
                    }
                    self.input_written(bytes.len() as u64);
                }
                InputChunk::Eof => {
                    drop(sink.take());
                }
            }
        }
    }

    fn input_written(&self, count: u64) {
        let ack = {
            let mut state = self.lock();
            state.input_written += count;
            (state.input_written - state.input_acked >= STREAM_ACK_EVERY).then(|| {
                state.input_acked = state.input_written;
                state.input_written
            })
        };
        if let Some(offset) = ack {
            self.send_control(&StreamMessage::Ack {
                channel: CHANNEL_INPUT,
                offset,
            });
        }
    }

    fn receive_input(&self, offset: u64, bytes: Vec<u8>) {
        let chunk = {
            let mut state = self.lock();
            if state.input_eof || state.closed {
                return;
            }
            let end = offset + bytes.len() as u64;
            if end <= state.input_received {
                return;
            }
            if offset > state.input_received {
                eprintln!(
                    "necoder-remote-server: stream {} の入力が飛んだ（{} → {offset}）",
                    self.id, state.input_received
                );
                return;
            }
            let skip = (state.input_received - offset) as usize;
            state.input_received = end;
            bytes[skip..].to_vec()
        };
        if let Ok(input) = self.input.lock() {
            if let Some(sender) = input.as_ref() {
                let _closed = sender.send(InputChunk::Data(chunk));
            }
        }
    }

    fn receive_input_eof(&self, offset: u64) {
        {
            let mut state = self.lock();
            if state.input_eof || offset > state.input_received {
                return;
            }
            state.input_eof = true;
        }
        if let Ok(input) = self.input.lock() {
            if let Some(sender) = input.as_ref() {
                let _closed = sender.send(InputChunk::Eof);
            }
        }
    }

    fn receive_ack(&self, channel: u8, offset: u64) {
        let index = match channel {
            CHANNEL_OUTPUT => 0,
            CHANNEL_ERROR => 1,
            _ => return,
        };
        {
            let mut state = self.lock();
            let output = &mut state.outputs[index];
            if offset > output.base {
                let drop_count = ((offset - output.base) as usize).min(output.buffer.len());
                output.buffer.drain(..drop_count);
                output.base += drop_count as u64;
                output.sent = output.sent.max(output.base);
            }
        }
        self.changed.notify_all();
    }

    /// 出力を読み続けて控えに積み、いまの接続へ流す。
    fn pump_output(&self, index: usize, mut source: impl Read) {
        let mut buffer = vec![0u8; STREAM_CHUNK];
        loop {
            let read = match source.read(&mut buffer) {
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                // PTY は相手（slave）が全部閉じると EIO を返す。それも終わり。
                Err(_) => 0,
            };
            {
                let mut state = self.lock();
                if read == 0 {
                    state.outputs[index].eof = true;
                } else {
                    loop {
                        if state.closed {
                            return;
                        }
                        let pending = state.outputs[index].buffer.len();
                        if pending + read <= STREAM_WINDOW {
                            break;
                        }
                        // 端末は切れている間も読み続ける（シェルやビルドを止めない）。古い方から捨てる。
                        if self.kind == StreamKind::Terminal && self.link().is_none() {
                            let output = &mut state.outputs[index];
                            let drop_count = (pending + read - STREAM_WINDOW).min(pending);
                            output.buffer.drain(..drop_count);
                            output.base += drop_count as u64;
                            output.sent = output.sent.max(output.base);
                            break;
                        }
                        state = self
                            .changed
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    state.outputs[index].buffer.extend(&buffer[..read]);
                }
            }
            self.flush();
            if read == 0 {
                return;
            }
        }
    }

    /// 控えのうちまだ送っていない分を、いまの接続へ送る。
    pub(crate) fn flush(&self) {
        let _order = self
            .send_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(link) = self.link() else {
            return;
        };
        let frames = {
            let mut state = self.lock();
            let mut frames = Vec::new();
            for (output, channel) in state
                .outputs
                .iter_mut()
                .zip([CHANNEL_OUTPUT, CHANNEL_ERROR])
            {
                let end = output.base + output.buffer.len() as u64;
                while output.sent < end {
                    let start = (output.sent - output.base) as usize;
                    let length = ((end - output.sent) as usize).min(STREAM_CHUNK);
                    let chunk: Vec<u8> = output
                        .buffer
                        .range(start..start + length)
                        .copied()
                        .collect();
                    frames.push((
                        StreamMessage::Data {
                            channel,
                            offset: output.sent,
                        },
                        chunk,
                    ));
                    output.sent += length as u64;
                }
                if output.eof && !output.eof_sent {
                    output.eof_sent = true;
                    frames.push((
                        StreamMessage::Eof {
                            channel,
                            offset: end,
                        },
                        Vec::new(),
                    ));
                }
            }
            if let Some(exit) = state.exit {
                if !state.exit_sent {
                    state.exit_sent = true;
                    frames.push((StreamMessage::Exit(exit), Vec::new()));
                }
            }
            frames
        };
        for (message, body) in frames {
            let written =
                Frame::stream(self.id, &message, body).and_then(|frame| link.write(&frame));
            if written.is_err() {
                // 接続が切れた。控えは残っているので、繋ぎ直した Attach で送り直す。
                break;
            }
        }
    }

    fn send_control(&self, message: &StreamMessage) {
        let Some(link) = self.link() else {
            return;
        };
        let written =
            Frame::stream(self.id, message, Vec::new()).and_then(|frame| link.write(&frame));
        if let Err(error) = written {
            remote_stream_trace(&format!("stream {} の知らせを送れない: {error:#}", self.id));
        }
    }

    fn wait_child(&self, mut child: std::process::Child) {
        let exit = match child.wait() {
            Ok(status) => ProcessExit::from_status(status),
            Err(error) => {
                eprintln!("necoder-remote-server: 子の終わりを待てない: {error}");
                ProcessExit::UNKNOWN
            }
        };
        self.lock().exit = Some(exit);
        // 回収したので pid はもう他人の物になりうる。以後シグナルを送らない。
        if let Ok(mut control) = self.control.lock() {
            if let Control::Process { pid } = &mut *control {
                *pid = 0;
            }
            #[cfg(unix)]
            if let Control::Terminal { pid, .. } = &mut *control {
                *pid = 0;
            }
        }
        self.changed.notify_all();
        self.flush();
    }

    fn resize(&self, size: PtySize) {
        #[cfg(unix)]
        if let Ok(control) = self.control.lock() {
            if let Control::Terminal { master, .. } = &*control {
                use std::os::fd::AsRawFd as _;
                let window = winsize(size);
                // SAFETY: `master` は生きている PTY の fd。TIOCSWINSZ は winsize を読むだけ。
                let result = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &window) };
                if result != 0 {
                    remote_stream_trace(&format!(
                        "端末の大きさを変えられない: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
        }
        #[cfg(not(unix))]
        let _unused = size;
    }

    fn signal(&self, name: &str) {
        let pid = match self.control.lock().as_deref() {
            Ok(Control::Process { pid }) => *pid,
            #[cfg(unix)]
            Ok(Control::Terminal { pid, .. }) => *pid,
            _ => return,
        };
        signal_group(pid, name);
    }

    /// client がもう要らない。プロセスを止め（猶予の後に強制）、TCP を閉じる。
    fn close(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            state.closed = true;
        }
        self.changed.notify_all();
        if let Ok(mut input) = self.input.lock() {
            input.take();
        }
        let first = match self.kind {
            StreamKind::Terminal => "HUP",
            StreamKind::Process | StreamKind::Tcp => "TERM",
        };
        let pid = match self.control.lock().as_deref() {
            Ok(Control::Tcp(socket)) => {
                let _closed = socket.shutdown(std::net::Shutdown::Both);
                return;
            }
            Ok(Control::Process { pid }) => *pid,
            #[cfg(unix)]
            Ok(Control::Terminal { pid, .. }) => *pid,
            Err(_) => return,
        };
        if pid == 0 {
            return;
        }
        signal_group(pid, first);
        let stream = self.clone();
        let spawned = spawn_named("necoder-stream-stop", move || {
            thread::sleep(STOP_GRACE);
            if stream.lock().exit.is_none() {
                stream.signal("KILL");
            }
        });
        if let Err(error) = spawned {
            eprintln!("necoder-remote-server: {error:#}");
            signal_group(pid, "KILL");
        }
    }
}

/// プロセスグループへシグナルを送る（unix）。`pid` は `process_group(0)` / `setsid` で長にした子。
#[cfg(unix)]
fn signal_group(pid: u32, name: &str) {
    let signal = match name {
        "INT" => libc::SIGINT,
        "TERM" => libc::SIGTERM,
        "KILL" => libc::SIGKILL,
        "HUP" => libc::SIGHUP,
        _ => return,
    };
    let Ok(group) = libc::pid_t::try_from(pid) else {
        return;
    };
    if group <= 0 {
        return;
    }
    // SAFETY: killpg はシグナルを送るだけ。相手は回収前の子のグループ（回収後は pid を 0 にしている）。
    let result = unsafe { libc::killpg(group, signal) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            remote_stream_trace(&format!("シグナルを送れない（{name}）: {error}"));
        }
    }
}

/// Windows（試験用の remote-server だけ）: グループの口が無いので `taskkill` で木ごと止める。
#[cfg(not(unix))]
fn signal_group(pid: u32, name: &str) {
    if pid == 0 || !matches!(name, "TERM" | "KILL" | "INT" | "HUP") {
        return;
    }
    let result = child_process::command("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(error) = result {
        remote_stream_trace(&format!("プロセスを止められない: {error}"));
    }
}

/// 接続先の手元の TCP ポートへ繋ぐ（IPv4 の次に IPv6 の loopback）。
fn connect_local_port(port: u16) -> Result<std::net::TcpStream> {
    let candidates = [
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
    ];
    let mut last_error = None;
    for address in candidates {
        match std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            Ok(socket) => {
                socket.set_nodelay(true)?;
                return Ok(socket);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(anyhow!(
        "接続先の {port} 番に繋げない: {}",
        last_error.map_or_else(|| "理由不明".to_string(), |error| error.to_string())
    ))
}

/// TCP の上りの書き口。drop で書く側だけ閉じる（相手に EOF が届き、下りは続く）。
struct TcpWriteHalf(std::net::TcpStream);

impl Write for TcpWriteHalf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl Drop for TcpWriteHalf {
    fn drop(&mut self) {
        let _closed = self.0.shutdown(std::net::Shutdown::Write);
    }
}

/// PTY の master を読む口。
#[cfg(unix)]
struct PtyReader(std::fs::File);

#[cfg(unix)]
impl Read for PtyReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buffer)
    }
}

#[cfg(unix)]
fn winsize(size: PtySize) -> libc::winsize {
    libc::winsize {
        ws_row: size.lines.max(1),
        ws_col: size.columns.max(1),
        ws_xpixel: size.pixel_width,
        ws_ypixel: size.pixel_height,
    }
}

/// PTY の対を開く。どちらの fd も子へ漏れないよう close-on-exec にする（漏れると master を
/// シェルが握り、シェルが終わっても読みが終わらない）。
#[cfg(unix)]
fn open_pty(size: PtySize) -> Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd as _;

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut window = winsize(size);
    // SAFETY: 出力引数は有効な c_int へのポインタ。name と termios は使わない（null）。
    // termios / winsize は Linux では `*const`、macOS では `*mut` を取る。`null_mut()` と `&mut` は
    // どちらへも渡せる（`*mut` → `*const` は暗黙に変わる）。
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut window,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("PTY を開けない");
    }
    // SAFETY: openpty が成功したので、どちらも開いたばかりで他に持ち主の無い fd。
    let (master, slave) = unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(master),
            std::os::fd::OwnedFd::from_raw_fd(slave),
        )
    };
    for fd in [&master, &slave] {
        use std::os::fd::AsRawFd as _;
        // SAFETY: 生きている fd に close-on-exec を立てるだけ。
        let flagged = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        if flagged != 0 {
            return Err(std::io::Error::last_os_error()).context("PTY の fd を設定できない");
        }
    }
    Ok((master, slave))
}

fn remote_stream_trace(message: &str) {
    if std::env::var_os("NECODER_REMOTE_TRACE").is_some() {
        eprintln!("[remote-stream] {message}");
    }
}
