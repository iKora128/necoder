//! RPC の上のストリーム（プロセスの stdio・PTY・TCP）。設計は `docs/REMOTE-CHANNEL.md` §2。
//!
//! 1 本の接続（ssh の標準入出力）に、ファイル操作の request と並んで、プロセスや端末の入出力を
//! 載せる。ストリームは id で区別し、channel ごとに**バイトの位置（offset）**を数える。
//! 位置があるので、接続が切れて繋ぎ直しても「どこまで受け取ったか」を突き合わせて続きから流せる。
//!
//! このファイルは wire の型と**手元（client）側**。接続先（daemon）側は `server_stream.rs`。

use crate::{Frame, FrameKind, Request, Response, RpcClient};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

/// 1 channel の「送ったが受け手が消費していない」量の上限。超えたら送り手が待つ（流量制御）。
pub(crate) const STREAM_WINDOW: usize = 1024 * 1024;
/// Data frame 1 つの中身の上限。大きな出力が他の request の前に長く居座らない大きさ。
pub(crate) const STREAM_CHUNK: usize = 32 * 1024;
/// 受け手はこれだけ消費するごとに Ack を返す（窓の 1/16 = 送り手は止まらずに流し続けられる）。
pub(crate) const STREAM_ACK_EVERY: u64 = 64 * 1024;

/// 入力（client → server。プロセスの stdin・端末の打鍵・TCP の上り）。
pub(crate) const CHANNEL_INPUT: u8 = 0;
/// 出力（stdout・端末の画面・TCP の下り）。
pub(crate) const CHANNEL_OUTPUT: u8 = 1;
/// stderr（パイプのプロセスだけ）。
pub(crate) const CHANNEL_ERROR: u8 = 2;

/// Stream frame（[`FrameKind::Stream`]）の meta。frame の id がストリーム id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum StreamMessage {
    /// body が中身。`offset` はこの塊の先頭がその channel の何バイト目か。
    Data { channel: u8, offset: u64 },
    /// 受け手が `offset` まで消費した。送り手はそこまでの控えを捨て、窓を進める。
    Ack { channel: u8, offset: u64 },
    /// その channel はもう来ない（`offset` = 全長）。
    Eof { channel: u8, offset: u64 },
    /// プロセスが終わった。出力の残りはこの後にも届きうる（バックグラウンドの子がパイプを握っている時）。
    Exit(ProcessExit),
    /// 端末の大きさ（client → server）。
    Resize(PtySize),
    /// プロセスグループへシグナルを送る（`INT` / `TERM` / `KILL` / `HUP`・client → server）。
    Signal { name: String },
    /// もう要らない（client → server: 止めて片付ける / server → client: 閉じた）。
    Close,
    /// 続けられない（daemon が知らない・控えが溢れた）。
    Lost { reason: String },
}

/// 端末の大きさ（文字数とピクセル）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySize {
    pub columns: u16,
    pub lines: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for PtySize {
    fn default() -> Self {
        Self {
            columns: 80,
            lines: 24,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// プロセスの終わり方。手元でも接続先でも同じ形で持つ（`std::process::ExitStatus` は
/// 手元の OS の物しか作れないので、接続先の終わり方を運べない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ProcessExit {
    /// 終わり方が分からない（接続先の daemon がプロセスを見失った等）。
    pub const UNKNOWN: Self = Self {
        code: None,
        signal: None,
    };

    pub fn code(&self) -> Option<i32> {
        self.code
    }

    pub fn signal(&self) -> Option<i32> {
        self.signal
    }

    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    pub(crate) fn from_status(status: std::process::ExitStatus) -> Self {
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        #[cfg(not(unix))]
        let signal = None;
        Self {
            code: status.code(),
            signal,
        }
    }
}

impl Frame {
    pub(crate) fn stream(id: u64, message: &StreamMessage, body: Vec<u8>) -> Result<Self> {
        Ok(Self {
            kind: FrameKind::Stream,
            id,
            meta: serde_json::to_vec(message)?,
            body,
        })
    }
}

/// ストリームの種類。切れた時の扱いが違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamKind {
    /// パイプの stdio（LSP・ACP・人のコマンド）。欠けを許さない。
    Process,
    /// 端末。切れている間に daemon が捨てた分の欠けを許す（画面は続きから描ける）。
    Terminal,
    /// TCP（ポート転送）。接続が切れたら続けない（ブラウザが繋ぎ直す）。
    Tcp,
}

impl StreamKind {
    fn resumable(self) -> bool {
        !matches!(self, Self::Tcp)
    }

    fn tolerates_gap(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

/// 手元側のストリームの束。[`crate::ReconnectingClient`] が 1 つ持ち、接続を張り直しても生き続ける。
///
/// 送る先（いまの接続）は弱参照で持つ。強く持つと「接続の reader スレッド → この束 → 接続」が
/// 輪になり、窓を閉じても ssh が終わらない。
#[derive(Default)]
pub(crate) struct StreamHub {
    streams: Mutex<HashMap<u64, Arc<ClientStream>>>,
    link: Mutex<Option<Weak<RpcClient>>>,
    next_id: AtomicU64,
    /// 切れている間に閉じたストリーム（繋ぎ直したら daemon に Close を送って片付けさせる）。
    closed_while_away: Mutex<Vec<u64>>,
}

impl StreamHub {
    /// 送る先を差し替える（初回接続・再接続のたび）。
    pub(crate) fn set_link(&self, link: &Arc<RpcClient>) {
        if let Ok(mut slot) = self.link.lock() {
            *slot = Some(Arc::downgrade(link));
        }
    }

    pub(crate) fn current_link(&self) -> Option<Arc<RpcClient>> {
        self.link
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
    }

    /// 新しいストリームを登録して id を返す。**request を送る前に**登録する（応答より先に
    /// 出力が届いても取りこぼさない）。
    pub(crate) fn register(self: &Arc<Self>, kind: StreamKind) -> Arc<ClientStream> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let stream = Arc::new(ClientStream {
            id,
            kind,
            hub: self.clone(),
            state: Mutex::new(StreamState::default()),
            changed: Condvar::new(),
            send_lock: Mutex::new(()),
        });
        if let Ok(mut streams) = self.streams.lock() {
            streams.insert(id, stream.clone());
        }
        stream
    }

    fn forget(&self, id: u64) {
        if let Ok(mut streams) = self.streams.lock() {
            streams.remove(&id);
        }
    }

    fn send(&self, id: u64, message: &StreamMessage, body: Vec<u8>) -> Result<()> {
        let link = self.current_link().context("接続先に繋がっていない")?;
        link.send_frame(&Frame::stream(id, message, body)?)
    }

    /// 接続の reader スレッドから呼ぶ。**ブロックしない**（受け手の消費を待たない。量は送り手が窓で抑える）。
    pub(crate) fn dispatch(&self, frame: Frame) {
        let Ok(message) = serde_json::from_slice::<StreamMessage>(&frame.meta) else {
            eprintln!("Remote SSH: 読めないストリームの frame を捨てた");
            return;
        };
        let stream = self
            .streams
            .lock()
            .ok()
            .and_then(|streams| streams.get(&frame.id).cloned());
        match stream {
            Some(stream) => stream.receive(message, frame.body),
            // 手元が知らない（閉じた後に届いた）。daemon 側に残らないよう閉じてもらう。
            None if !matches!(message, StreamMessage::Close | StreamMessage::Lost { .. }) => {
                if let Err(error) = self.send(frame.id, &StreamMessage::Close, Vec::new()) {
                    remote_stream_trace(&format!("知らないストリームを閉じられない: {error:#}"));
                }
            }
            None => {}
        }
    }

    /// 繋ぎ直した接続で、続けられるストリームを付け直す（daemon が持っている続きを受け取り直す）。
    /// 続けられない物（TCP・daemon が知らない物）は閉じたことにする。
    pub(crate) fn reattach(&self, link: &Arc<RpcClient>) {
        let closed: Vec<u64> = self
            .closed_while_away
            .lock()
            .map(|mut closed| std::mem::take(&mut *closed))
            .unwrap_or_default();
        for id in closed {
            let sent = Frame::stream(id, &StreamMessage::Close, Vec::new())
                .and_then(|frame| link.send_frame(&frame));
            if let Err(error) = sent {
                remote_stream_trace(&format!("閉じたストリームを片付けられない: {error:#}"));
            }
        }
        let streams: Vec<Arc<ClientStream>> = self
            .streams
            .lock()
            .map(|streams| streams.values().cloned().collect())
            .unwrap_or_default();
        for stream in streams {
            if !stream.kind.resumable() {
                stream.lose("接続が切れたので閉じた");
                continue;
            }
            if let Err(error) = stream.reattach(link) {
                stream.lose(&format!("繋ぎ直した接続で続けられない: {error:#}"));
            }
        }
    }
}

/// 手元側の 1 ストリーム。
pub(crate) struct ClientStream {
    id: u64,
    kind: StreamKind,
    hub: Arc<StreamHub>,
    state: Mutex<StreamState>,
    changed: Condvar,
    /// 入力を送る順番を守る（書き手と、繋ぎ直しの再送が交差しない）。`state` とは別にする —
    /// 送る間 `state` を握ると、接続の reader スレッドが Ack を書き込めずに止まる。
    send_lock: Mutex<()>,
}

#[derive(Default)]
struct StreamState {
    /// 0 = 出力（[`CHANNEL_OUTPUT`]）・1 = stderr（[`CHANNEL_ERROR`]）。
    incoming: [Incoming; 2],
    outgoing: Outgoing,
    exit: Option<ProcessExit>,
    lost: Option<String>,
    /// 手元が閉じた（もう読まない・書かない）。
    closed: bool,
    /// daemon がこのストリームを持った（開く request の応答が来た）。それまでの入力は送らずに溜める
    /// （daemon が知らない id への入力は捨てられ、位置が飛んで以後の入力が全部届かなくなる）。
    ready: bool,
    /// 最後に頼んだ端末の大きさ（繋ぎ直したら送り直す）。
    size: Option<PtySize>,
}

#[derive(Default)]
struct Incoming {
    buffer: VecDeque<u8>,
    /// 次に届くはずの位置（ここまで受け取った）。
    received: u64,
    consumed: u64,
    acked: u64,
    eof: bool,
}

#[derive(Default)]
struct Outgoing {
    /// daemon がまだ受け取ったと言っていない入力（`base` から）。繋ぎ直したらここから送り直す。
    pending: VecDeque<u8>,
    base: u64,
    /// いまの接続に送った位置。
    sent: u64,
    /// 入力を閉じた（全部送ったら Eof を送る）。
    eof: bool,
    eof_sent: bool,
}

fn channel_index(channel: u8) -> Option<usize> {
    match channel {
        CHANNEL_OUTPUT => Some(0),
        CHANNEL_ERROR => Some(1),
        _ => None,
    }
}

impl ClientStream {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn receive(&self, message: StreamMessage, body: Vec<u8>) {
        let mut state = self.lock();
        match message {
            StreamMessage::Data { channel, offset } => {
                let Some(index) = channel_index(channel) else {
                    return;
                };
                let end = offset + body.len() as u64;
                let incoming = &mut state.incoming[index];
                if end <= incoming.received {
                    // 繋ぎ直しの重複（もう受け取っている）。
                    return;
                }
                if offset > incoming.received {
                    if !self.kind.tolerates_gap() {
                        drop(state);
                        self.lose("出力の一部が届かなかった");
                        return;
                    }
                    incoming.received = offset;
                }
                let skip = (incoming.received - offset) as usize;
                incoming.buffer.extend(&body[skip..]);
                incoming.received = end;
            }
            StreamMessage::Ack { channel, offset } if channel == CHANNEL_INPUT => {
                let outgoing = &mut state.outgoing;
                if offset > outgoing.base {
                    let drop_count =
                        ((offset - outgoing.base) as usize).min(outgoing.pending.len());
                    outgoing.pending.drain(..drop_count);
                    outgoing.base += drop_count as u64;
                    outgoing.sent = outgoing.sent.max(outgoing.base);
                }
            }
            StreamMessage::Eof { channel, offset } => {
                let Some(index) = channel_index(channel) else {
                    return;
                };
                let incoming = &mut state.incoming[index];
                if incoming.received >= offset || self.kind.tolerates_gap() {
                    incoming.eof = true;
                } else {
                    drop(state);
                    self.lose("出力の終わりより前が届かなかった");
                    return;
                }
            }
            StreamMessage::Exit(exit) => state.exit = Some(exit),
            StreamMessage::Lost { reason } => {
                state.lost.get_or_insert(reason);
            }
            StreamMessage::Close => {
                for incoming in &mut state.incoming {
                    incoming.eof = true;
                }
                state
                    .lost
                    .get_or_insert_with(|| "接続先で閉じた".to_string());
            }
            StreamMessage::Ack { .. } | StreamMessage::Resize(_) | StreamMessage::Signal { .. } => {
            }
        }
        self.changed.notify_all();
    }

    /// 続けられなくなった（読む側は残りを読んだら終わる・書く側は失敗する）。
    pub(crate) fn lose(&self, reason: &str) {
        let mut state = self.lock();
        state.lost.get_or_insert_with(|| reason.to_string());
        drop(state);
        self.changed.notify_all();
    }

    /// `channel` を読む。届くまで待つ。終わり（Eof・見失った・閉じた）なら `Ok(0)`。
    fn read_channel(&self, channel: u8, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let Some(index) = channel_index(channel) else {
            return Ok(0);
        };
        let mut state = self.lock();
        loop {
            if !state.incoming[index].buffer.is_empty() {
                break;
            }
            if state.incoming[index].eof || state.lost.is_some() || state.closed {
                return Ok(0);
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let incoming = &mut state.incoming[index];
        let count = buffer.len().min(incoming.buffer.len());
        for (slot, byte) in buffer.iter_mut().zip(incoming.buffer.drain(..count)) {
            *slot = byte;
        }
        incoming.consumed += count as u64;
        let ack = (incoming.consumed - incoming.acked >= STREAM_ACK_EVERY).then(|| {
            incoming.acked = incoming.consumed;
            incoming.consumed
        });
        drop(state);
        if let Some(offset) = ack {
            // 切れていれば送れないが、繋ぎ直した時に送り直す（[`Self::reattach`]）。
            let _not_connected =
                self.hub
                    .send(self.id, &StreamMessage::Ack { channel, offset }, Vec::new());
        }
        Ok(count)
    }

    /// 入力を書く。窓が空くまで待つ。切れている間は控えに積むだけで、繋ぎ直したら送る。
    fn write_input(&self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut state = self.lock();
        loop {
            if state.closed || state.lost.is_some() || state.outgoing.eof {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    state
                        .lost
                        .clone()
                        .unwrap_or_else(|| "ストリームは閉じている".to_string()),
                ));
            }
            if state.outgoing.pending.len() < STREAM_WINDOW {
                break;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let count = bytes
            .len()
            .min(STREAM_WINDOW - state.outgoing.pending.len());
        state.outgoing.pending.extend(&bytes[..count]);
        drop(state);
        self.flush_input();
        Ok(count)
    }

    /// 入力を閉じる（全部送った後に Eof）。
    fn close_input(&self) {
        let mut state = self.lock();
        if state.outgoing.eof {
            return;
        }
        state.outgoing.eof = true;
        drop(state);
        self.changed.notify_all();
        self.flush_input();
    }

    /// まだ送っていない入力をいまの接続へ送る。
    fn flush_input(&self) {
        let _order = self
            .send_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.flush_input_locked();
    }

    fn flush_input_locked(&self) {
        let frames = {
            let mut state = self.lock();
            if !state.ready {
                return;
            }
            let outgoing = &mut state.outgoing;
            let end = outgoing.base + outgoing.pending.len() as u64;
            let mut frames = Vec::new();
            while outgoing.sent < end {
                let start = (outgoing.sent - outgoing.base) as usize;
                let length = ((end - outgoing.sent) as usize).min(STREAM_CHUNK);
                let chunk: Vec<u8> = outgoing
                    .pending
                    .range(start..start + length)
                    .copied()
                    .collect();
                frames.push((
                    StreamMessage::Data {
                        channel: CHANNEL_INPUT,
                        offset: outgoing.sent,
                    },
                    chunk,
                ));
                outgoing.sent += length as u64;
            }
            if outgoing.eof && !outgoing.eof_sent {
                outgoing.eof_sent = true;
                frames.push((
                    StreamMessage::Eof {
                        channel: CHANNEL_INPUT,
                        offset: end,
                    },
                    Vec::new(),
                ));
            }
            frames
        };
        for (message, body) in frames {
            // 切れていれば残りも届かない。控えは残っているので、繋ぎ直した時に送り直す。
            if self.hub.send(self.id, &message, body).is_err() {
                break;
            }
        }
    }

    fn send_control(&self, message: StreamMessage) -> Result<()> {
        self.hub.send(self.id, &message, Vec::new())
    }

    /// daemon がこのストリームを持った。溜めた入力と大きさを流す。
    pub(crate) fn mark_ready(&self) {
        let size = {
            let mut state = self.lock();
            state.ready = true;
            state.size
        };
        if let Some(size) = size {
            if let Err(error) = self.send_control(StreamMessage::Resize(size)) {
                remote_stream_trace(&format!("端末の大きさを送れない: {error:#}"));
            }
        }
        self.flush_input();
    }

    /// 開けなかった。理由を stderr の末尾に残し（[`crate::StderrTail`] が拾う）、読む側を終わらせる。
    pub(crate) fn fail(&self, reason: &str) {
        let mut state = self.lock();
        let error = &mut state.incoming[1];
        error.buffer.extend(reason.as_bytes());
        error.buffer.push_back(b'\n');
        error.received += reason.len() as u64 + 1;
        for incoming in &mut state.incoming {
            incoming.eof = true;
        }
        state.exit.get_or_insert(ProcessExit::UNKNOWN);
        state.lost.get_or_insert_with(|| reason.to_string());
        drop(state);
        self.changed.notify_all();
        self.hub.forget(self.id);
    }

    /// 端末の大きさを変える。切れていれば繋ぎ直した時に送る。
    pub(crate) fn resize(&self, size: PtySize) {
        let ready = {
            let mut state = self.lock();
            state.size = Some(size);
            state.ready
        };
        if !ready {
            return;
        }
        if let Err(error) = self.send_control(StreamMessage::Resize(size)) {
            remote_stream_trace(&format!(
                "端末の大きさを送れない（繋ぎ直したら送る）: {error:#}"
            ));
        }
    }

    pub(crate) fn exit(&self) -> Option<ProcessExit> {
        let state = self.lock();
        state
            .exit
            .or_else(|| state.lost.as_ref().map(|_| ProcessExit::UNKNOWN))
    }

    /// 終わるまで最長 `timeout` 待つ。
    pub(crate) fn wait_exit(&self, timeout: Duration) -> Option<ProcessExit> {
        let state = self.lock();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.exit.is_none() && state.lost.is_none() && !state.closed
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .exit
            .or_else(|| state.lost.as_ref().map(|_| ProcessExit::UNKNOWN))
    }

    /// 手元がもう要らない。daemon にプロセスを止めて片付けさせる。2 回目以降は何もしない。
    pub(crate) fn close(&self) {
        {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            state.closed = true;
        }
        self.changed.notify_all();
        self.hub.forget(self.id);
        if self.send_control(StreamMessage::Close).is_err() {
            if let Ok(mut closed) = self.hub.closed_while_away.lock() {
                closed.push(self.id);
            }
        }
    }

    /// 繋ぎ直した接続で、受け取った位置から続きを頼み、daemon が受け取っていない入力を送り直す。
    fn reattach(&self, link: &Arc<RpcClient>) -> Result<()> {
        let (output_offset, error_offset, closed) = {
            let state = self.lock();
            (
                state.incoming[0].received,
                state.incoming[1].received,
                // 開いている途中の物は、開く request の結果（失敗）で決まる。
                state.closed || state.lost.is_some() || !state.ready,
            )
        };
        if closed {
            return Ok(());
        }
        let (response, _) = link.request(
            &Request::Attach {
                stream_id: self.id,
                output_offset,
                error_offset,
            },
            Vec::new(),
        )?;
        let Response::Attached { input_offset } = response else {
            bail!("Attach に不正な応答");
        };
        let _order = self
            .send_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (acks, size) = {
            let mut state = self.lock();
            let outgoing = &mut state.outgoing;
            if input_offset > outgoing.base {
                let drop_count =
                    ((input_offset - outgoing.base) as usize).min(outgoing.pending.len());
                outgoing.pending.drain(..drop_count);
                outgoing.base += drop_count as u64;
            }
            outgoing.sent = outgoing.base;
            outgoing.eof_sent = false;
            let mut acks = Vec::new();
            for (index, channel) in [CHANNEL_OUTPUT, CHANNEL_ERROR].into_iter().enumerate() {
                let incoming = &mut state.incoming[index];
                if incoming.consumed > 0 {
                    incoming.acked = incoming.consumed;
                    acks.push((channel, incoming.consumed));
                }
            }
            (acks, state.size)
        };
        self.changed.notify_all();
        for (channel, offset) in acks {
            link.send_frame(&Frame::stream(
                self.id,
                &StreamMessage::Ack { channel, offset },
                Vec::new(),
            )?)?;
        }
        if let Some(size) = size {
            link.send_frame(&Frame::stream(
                self.id,
                &StreamMessage::Resize(size),
                Vec::new(),
            )?)?;
        }
        self.flush_input_locked();
        Ok(())
    }
}

/// ストリームの 1 channel を読む口（`Read`）。
pub(crate) struct StreamReader {
    stream: Arc<ClientStream>,
    channel: u8,
}

impl StreamReader {
    pub(crate) fn new(stream: Arc<ClientStream>, channel: u8) -> Self {
        Self { stream, channel }
    }
}

impl Read for StreamReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read_channel(self.channel, buffer)
    }
}

/// ストリームの入力を書く口（`Write`）。drop すると入力を閉じる（プロセスの stdin が EOF になる）。
pub(crate) struct StreamWriter {
    stream: Arc<ClientStream>,
}

impl StreamWriter {
    pub(crate) fn new(stream: Arc<ClientStream>) -> Self {
        Self { stream }
    }
}

impl Write for StreamWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.write_input(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for StreamWriter {
    fn drop(&mut self) {
        self.stream.close_input();
    }
}

/// 接続先の端末（PTY は接続先の daemon が持つ）。`TerminalView` が alacritty の EventLoop に繋ぐ。
/// drop すると端末を閉じる（接続先のシェルに SIGHUP）。
pub struct HostPty {
    stream: Arc<ClientStream>,
    /// 接続（ssh）を生かしておく。端末が開いている間に窓の host が先に消えても切れない。
    _connection: Arc<dyn Send + Sync>,
}

impl HostPty {
    pub(crate) fn new(stream: Arc<ClientStream>, connection: Arc<dyn Send + Sync>) -> Self {
        Self {
            stream,
            _connection: connection,
        }
    }

    /// 画面に出すバイト列（届くまで待つ・終われば `Ok(0)`）。
    pub fn reader(&self) -> Box<dyn Read + Send> {
        Box::new(StreamReader::new(self.stream.clone(), CHANNEL_OUTPUT))
    }

    /// 打鍵を送る口。
    pub fn writer(&self) -> Box<dyn Write + Send> {
        Box::new(PtyInput(self.stream.clone()))
    }

    pub fn resize(&self, size: PtySize) {
        self.stream.resize(size);
    }

    /// 終わっていればその終わり方。
    pub fn exit(&self) -> Option<ProcessExit> {
        self.stream.exit()
    }

    /// 終わるまで最長 `timeout` 待つ。
    pub fn wait_exit(&self, timeout: Duration) -> Option<ProcessExit> {
        self.stream.wait_exit(timeout)
    }

    /// 終わりを別のスレッドから待つ口（端末を閉じる権限は持たない）。
    pub fn exit_watch(&self) -> PtyExitWatch {
        PtyExitWatch(self.stream.clone())
    }
}

/// [`HostPty::exit_watch`]。持っていても端末は閉じない（閉じるのは [`HostPty`] の drop だけ）。
#[derive(Clone)]
pub struct PtyExitWatch(Arc<ClientStream>);

impl PtyExitWatch {
    /// 終わるまで最長 `timeout` 待つ。手元が端末を閉じた時も待つのをやめる（`None`）。
    pub fn wait(&self, timeout: Duration) -> Option<ProcessExit> {
        self.0.wait_exit(timeout)
    }
}

impl Drop for HostPty {
    fn drop(&mut self) {
        self.stream.close();
    }
}

/// 端末の打鍵（drop しても入力を閉じない＝端末の EOF は `Ctrl+D` で人が決める）。
struct PtyInput(Arc<ClientStream>);

impl Write for PtyInput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write_input(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 接続先の TCP 接続（ポート転送の 1 本）。
pub(crate) struct TcpStreamHandle {
    stream: Arc<ClientStream>,
}

impl TcpStreamHandle {
    pub(crate) fn new(stream: Arc<ClientStream>) -> Self {
        Self { stream }
    }

    pub(crate) fn reader(&self) -> StreamReader {
        StreamReader::new(self.stream.clone(), CHANNEL_OUTPUT)
    }

    pub(crate) fn writer(&self) -> StreamWriter {
        StreamWriter::new(self.stream.clone())
    }
}

impl Drop for TcpStreamHandle {
    fn drop(&mut self) {
        self.stream.close();
    }
}

fn remote_stream_trace(message: &str) {
    if std::env::var_os("NECODER_REMOTE_TRACE").is_some() {
        eprintln!("[remote-stream] {message}");
    }
}
