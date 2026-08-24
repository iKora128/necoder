//! control_transport — 制御 IPC の足回り（WINDOWS-PORT.md §D2）。
//!
//! `control_ipc.rs`（GUI 側 listener）と `fleet.rs`（CLI 側 client）が
//! `std::os::unix::net` を cfg 無しで使っていたため、Windows ではそもそもコンパイルできなかった。
//! ここで **1 接続 1 リクエストの双方向バイトストリーム**として抽象し、業務ロジックから
//! `#[cfg(target_os)]` を追い出す（ARCHITECTURE の依存方向は変えない）。
//!
//! | | unix | windows |
//! |---|---|---|
//! | 実体 | Unix domain socket | 名前付きパイプ |
//! | 場所 | `~/.necoder/gui.sock` | `\\.\pipe\necoder-gui-<user>` |
//! | 保護 | 0600（同一ユーザーのみ） | 既定 DACL（作成ユーザー + SYSTEM + Administrators）+ リモート拒否 |
//!
//! **TCP loopback は採らない。** 他ユーザー・他プロセスから叩けてしまい、`control_ipc.rs` 冒頭が
//! 守っている「守るべき操作（spawn / send / 遷移）の単一経路」が崩れるため。
//!
//! パスの決定は `paths` crate（§D1）。この層は「渡されたパスで待つ／繋ぐ」だけを担う。

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

/// 制御 IPC の待ち受け。1 接続 1 リクエスト。
pub struct ControlListener(imp::Listener);

/// 制御 IPC の 1 接続（双方向バイトストリーム）。
pub struct ControlStream(imp::Stream);

impl ControlListener {
    /// 待ち受けを開始する。
    ///
    /// 既に別プロセスが同じ場所で待っている場合は [`io::ErrorKind::AddrInUse`] を返す。
    pub fn bind(path: &Path) -> io::Result<ControlListener> {
        imp::Listener::bind(path).map(ControlListener)
    }

    /// 次の接続を待つ。accept ループ用（呼び出し側は専用スレッドでこれを回す）。
    pub fn accept(&mut self) -> io::Result<ControlStream> {
        self.0.accept().map(ControlStream)
    }
}

impl ControlStream {
    /// 待っている GUI へ繋ぐ。GUI が居なければエラー。
    pub fn connect(path: &Path) -> io::Result<ControlStream> {
        imp::Stream::connect(path).map(ControlStream)
    }

    /// 読み取りのタイムアウト。相手が黙り込んでもスレッドを永久に抱え込まないため。
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }

    /// 書き込みのタイムアウト。
    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_write_timeout(timeout)
    }
}

impl Read for ControlStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for ControlStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

// ---------------------------------------------------------------------------
// unix — 従来の実装をそのまま包む（mac の挙動を 1 ミリも変えない・§D8）
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod imp {
    use std::io::{self, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    pub struct Listener(UnixListener);

    pub struct Stream(UnixStream);

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Listener> {
            // 生きている相手が居るなら二重 bind しない。死んだ socket ファイルなら除去して継ぐ。
            // （unix の socket ファイルはプロセスが死んでも残るため、この判定は必須）
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "制御 IPC は既に別プロセスが待ち受けています",
                ));
            }
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            let listener = UnixListener::bind(path)?;
            // 0600: 同一ユーザーのみ（socket は uid で守られるが明示しておく）。
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Listener(listener))
        }

        pub fn accept(&mut self) -> io::Result<Stream> {
            self.0.accept().map(|(stream, _address)| Stream(stream))
        }
    }

    impl Stream {
        pub fn connect(path: &Path) -> io::Result<Stream> {
            UnixStream::connect(path).map(Stream)
        }

        pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(timeout)
        }

        pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_write_timeout(timeout)
        }
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
}

// ---------------------------------------------------------------------------
// windows — 名前付きパイプ
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::io::{self, Read, Write};
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, WaitNamedPipeW,
    };

    // Win32 の定数は windows-sys の版によって置き場（feature）が動くため、ABI が固定である
    // ことを利用してここでローカル定義する。値は Windows SDK の winbase.h / winnt.h に由来。
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;
    const PIPE_READMODE_BYTE: u32 = 0x0000_0000;
    const PIPE_WAIT: u32 = 0x0000_0000;
    /// ネットワーク越しの接続を拒否する。ローカル同一ユーザー専用の口にするため必須。
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
    /// `WaitNamedPipeW` / `CreateNamedPipeW` の既定タイムアウト（ミリ秒）。
    const PIPE_DEFAULT_TIMEOUT_MS: u32 = 50;

    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_BROKEN_PIPE: u32 = 109;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_NO_DATA: u32 = 232;
    const ERROR_PIPE_CONNECTED: u32 = 535;

    /// 読み取り待ちのポーリング間隔。`PeekNamedPipe` で「まだ来ていない」を見るための刻み。
    const POLL_INTERVAL: Duration = Duration::from_millis(5);

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }

    fn io_error(code: u32, context: &str) -> io::Error {
        io::Error::new(
            io::Error::from_raw_os_error(code as i32).kind(),
            format!("{context}（Win32 エラー {code}）"),
        )
    }

    /// パイプの 1 インスタンスを作る。
    ///
    /// `first` を立てると `FILE_FLAG_FIRST_PIPE_INSTANCE` が付き、**既に別プロセスが同じ名前の
    /// パイプを持っている場合に失敗する**。＝unix の「socket ファイルが残っているだけか、
    /// 本当に生きているか」の判定より確実な二重起動検出になる。
    fn create_instance(name: &[u16], first: bool) -> io::Result<HANDLE> {
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                PIPE_DEFAULT_TIMEOUT_MS,
                // 既定のセキュリティ記述子 = 作成ユーザー + SYSTEM + Administrators。
                // 他の一般ユーザーからは開けない＝unix の 0600 と実質同じ保護。
                ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let code = last_error();
            if first && code == ERROR_ACCESS_DENIED {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "制御 IPC は既に別プロセスが待ち受けています",
                ));
            }
            return Err(io_error(code, "名前付きパイプを作れません"));
        }
        Ok(handle)
    }

    pub struct Listener {
        name: Vec<u16>,
        /// 次の接続を受けるために用意してあるインスタンス。
        ///
        /// 名前付きパイプは「1 インスタンス = 1 接続」で、接続が付いたら次の客のために
        /// **新しいインスタンスを作り直す**必要がある（unix の listener が accept で
        /// 何度でも新しい fd を返すのとはモデルが違う）。
        pending: HANDLE,
    }

    // HANDLE は *mut c_void ＝ 既定では Send でない。accept ループは専用スレッドが
    // listener を所有し、接続ごとの Stream も生成スレッドへ move する。どちらも
    // 「1 つの HANDLE を同時に 2 スレッドが触らない」ことは型で保証されている。
    unsafe impl Send for Listener {}
    unsafe impl Send for Stream {}

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Listener> {
            let name = wide(path);
            let pending = create_instance(&name, true)?;
            Ok(Listener { name, pending })
        }

        pub fn accept(&mut self) -> io::Result<Stream> {
            let handle = self.pending;
            let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) };
            if connected == 0 {
                let code = last_error();
                // クライアントが ConnectNamedPipe より先に繋いでいた場合。接続済みなので成功扱い。
                if code != ERROR_PIPE_CONNECTED {
                    return Err(io_error(code, "パイプへの接続待ちに失敗しました"));
                }
            }
            // 次の客のためのインスタンスを用意する。ここで失敗すると以後 accept できないので、
            // 接続済みハンドルは返さずに畳む（呼び出し側のループが終了する）。
            let next = create_instance(&self.name, false)?;
            self.pending = next;
            Ok(Stream {
                handle,
                is_server: true,
                read_timeout: None,
            })
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.pending);
            }
        }
    }

    pub struct Stream {
        handle: HANDLE,
        /// サーバ側のハンドルか。畳み方が違う（サーバは Disconnect が要る）。
        is_server: bool,
        read_timeout: Option<Duration>,
    }

    impl Stream {
        pub fn connect(path: &Path) -> io::Result<Stream> {
            let name = wide(path);
            // 全インスタンスが埋まっていると ERROR_PIPE_BUSY。空くのを少しだけ待つ。
            for _ in 0..2 {
                let handle = unsafe {
                    CreateFileW(
                        name.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        0,
                        ptr::null(),
                        OPEN_EXISTING,
                        0,
                        ptr::null_mut(),
                    )
                };
                if handle != INVALID_HANDLE_VALUE {
                    return Ok(Stream {
                        handle,
                        is_server: false,
                        read_timeout: None,
                    });
                }
                let code = last_error();
                if code != ERROR_PIPE_BUSY {
                    return Err(io_error(code, "GUI の制御パイプに繋げません"));
                }
                unsafe {
                    WaitNamedPipeW(name.as_ptr(), 1_000);
                }
            }
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "GUI の制御パイプが混み合っています",
            ))
        }

        pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.read_timeout = timeout;
            Ok(())
        }

        pub fn set_write_timeout(&mut self, _timeout: Option<Duration>) -> io::Result<()> {
            // 名前付きパイプの同期書き込みにタイムアウトを付けるには overlapped I/O が要る。
            // ここで扱うのは同一ユーザーのローカル 1 行 JSON（64KB バッファに収まる）なので、
            // 相手が読まなくてもバッファに入りきって返る＝実務上ブロックしない。
            Ok(())
        }

        /// 読めるようになるまで待つ。`read_timeout` が設定されていなければ何もしない。
        fn wait_for_readable(&self) -> io::Result<()> {
            let Some(timeout) = self.read_timeout else {
                return Ok(());
            };
            let deadline = Instant::now() + timeout;
            loop {
                let mut available: u32 = 0;
                let peeked = unsafe {
                    PeekNamedPipe(
                        self.handle,
                        ptr::null_mut(),
                        0,
                        ptr::null_mut(),
                        &mut available,
                        ptr::null_mut(),
                    )
                };
                if peeked == 0 {
                    // 相手が閉じた等。実 read にエラー（または EOF）を出させる。
                    return Ok(());
                }
                if available > 0 {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "制御 IPC の読み取りがタイムアウトしました",
                    ));
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.wait_for_readable()?;
            let mut read_bytes: u32 = 0;
            let ok = unsafe {
                ReadFile(
                    self.handle,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    &mut read_bytes,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                let code = last_error();
                // 相手が閉じた = EOF。unix の read が 0 を返すのと同じ意味にする。
                if code == ERROR_BROKEN_PIPE || code == ERROR_NO_DATA {
                    return Ok(0);
                }
                return Err(io_error(code, "制御 IPC の読み取りに失敗しました"));
            }
            Ok(read_bytes as usize)
        }
    }

    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteFile(
                    self.handle,
                    buf.as_ptr() as *const _,
                    buf.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io_error(last_error(), "制御 IPC の書き込みに失敗しました"));
            }
            Ok(written as usize)
        }

        fn flush(&mut self) -> io::Result<()> {
            // FlushFileBuffers はサーバ側では「クライアントが読み切るまで待つ」。
            // 相手が既に居なければエラーが返るだけでハングはしない。
            unsafe {
                FlushFileBuffers(self.handle);
            }
            Ok(())
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                if self.is_server {
                    // 書いた分をクライアントが読み切ってから切る（切断が先だとデータが消える）。
                    FlushFileBuffers(self.handle);
                    DisconnectNamedPipe(self.handle);
                }
                CloseHandle(self.handle);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// テスト — unix / windows のどちらでも同じ内容を検証する
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader};

    /// テスト用の待ち受け先。unix は一時ディレクトリの socket、windows は専用のパイプ名。
    ///
    /// **名前は短くする。** macOS の `SUN_LEN` は ~104 バイトで、しかも `TMPDIR` が
    /// `/var/folders/xx/…24文字…/T/` と長い。分かりやすい名前を付けると bind に失敗する
    /// （本体の `control_socket_path` が `~/.necoder/gui.sock` と短いのも同じ理由）。
    fn test_endpoint(label: &str) -> std::path::PathBuf {
        let unique = format!("nec-t{}-{label}", std::process::id());
        if cfg!(windows) {
            std::path::PathBuf::from(format!(r"\\.\pipe\{unique}"))
        } else {
            std::env::temp_dir().join(format!("{unique}.sock"))
        }
    }

    /// 1 接続 1 リクエストの往復。`control_ipc` が実際に使う形そのもの。
    #[test]
    fn round_trip_carries_a_request_and_a_response() {
        let endpoint = test_endpoint("rt");
        let mut listener = ControlListener::bind(&endpoint).expect("bind できない");

        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept できない");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("要求を読めない");
            let stream = reader.get_mut();
            writeln!(stream, "{{\"ok\":true,\"echo\":{}}}", line.trim())
                .expect("応答を書けない");
            let _ = stream.flush();
        });

        let mut client = ControlStream::connect(&endpoint).expect("connect できない");
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout を設定できない");
        writeln!(client, "42").expect("要求を書けない");
        client.flush().expect("flush できない");

        let mut reader = BufReader::new(client);
        let mut response = String::new();
        reader.read_line(&mut response).expect("応答を読めない");
        assert_eq!(response.trim(), r#"{"ok":true,"echo":42}"#);

        server.join().expect("サーバスレッドが落ちた");
        if !cfg!(windows) {
            let _ = std::fs::remove_file(&endpoint);
        }
    }

    /// 同じ場所への二重 bind は弾く（＝GUI が二重に待ち受けない）。
    #[test]
    fn binding_twice_is_rejected() {
        let endpoint = test_endpoint("db");
        let _first = ControlListener::bind(&endpoint).expect("1 本目が bind できない");
        let second = ControlListener::bind(&endpoint);
        assert!(
            second.is_err(),
            "既に待ち受けている場所へ 2 本目が bind できてしまった"
        );
        assert_eq!(
            second.err().map(|error| error.kind()),
            Some(io::ErrorKind::AddrInUse)
        );
        if !cfg!(windows) {
            let _ = std::fs::remove_file(&endpoint);
        }
    }

    /// 誰も待っていない場所へは繋がらない（CLI が「GUI が起動していません」と言える根拠）。
    #[test]
    fn connecting_without_a_listener_fails() {
        let endpoint = test_endpoint("nl");
        assert!(ControlStream::connect(&endpoint).is_err());
    }

    /// 相手が黙っていても読み取りはタイムアウトする（スレッドを永久に抱え込まない）。
    #[test]
    fn read_times_out_when_the_peer_stays_silent() {
        let endpoint = test_endpoint("to");
        let mut listener = ControlListener::bind(&endpoint).expect("bind できない");

        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().expect("accept できない");
            stream
                .set_read_timeout(Some(Duration::from_millis(200)))
                .expect("timeout を設定できない");
            let mut buffer = [0_u8; 16];
            let outcome = stream.read(&mut buffer);
            // 何も送られてこないので TimedOut になる（EOF ではない＝相手はまだ生きている）
            matches!(
                outcome.as_ref().map_err(|error| error.kind()),
                Err(io::ErrorKind::TimedOut)
            )
        });

        let client = ControlStream::connect(&endpoint).expect("connect できない");
        let timed_out = server.join().expect("サーバスレッドが落ちた");
        drop(client);
        assert!(timed_out, "黙っている相手に対して読み取りが待ち続けた");
        if !cfg!(windows) {
            let _ = std::fs::remove_file(&endpoint);
        }
    }
}
