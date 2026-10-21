//! RPC の上のストリーム（`docs/REMOTE-CHANNEL.md` §2・§5 の R1 / R2 / R5）の試験。
//!
//! ssh の代わりに loopback TCP の対で同じ protocol を喋らせる（Windows でも走る）。daemon と同じく
//! 接続をまたいでプロセスを持つ server（[`DaemonConnector`]）で、切って繋ぎ直しても続くことを確かめる。

use super::*;
use std::io::BufRead;
use std::net::TcpStream;

/// 全二重の byte stream の対（SSH の標準入出力の代わり）。
pub(crate) fn stream_pair() -> std::io::Result<(TcpStream, TcpStream)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    client.set_nodelay(true)?;
    server.set_nodelay(true)?;
    Ok((client, server))
}

fn scratch(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let path = std::env::temp_dir().join(format!(
        "necoder-stream-{tag}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _stale = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("一時ディレクトリを作れる");
    paths::canonicalize(&path).expect("一時ディレクトリを解決できる")
}

/// daemon と同じく、接続をまたいで project とプロセスを持つ server へ繋ぐ connector。
/// [`Self::drop_connection`] は「スリープ・VPN 断で ssh が切れた」の代わり。
struct DaemonConnector {
    state: SharedServerState,
    streams: Arc<ServerStreams>,
    current: Mutex<Option<TcpStream>>,
    connections: AtomicU64,
}

impl DaemonConnector {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(Mutex::new(ServerState::new())),
            streams: Arc::new(ServerStreams::default()),
            current: Mutex::new(None),
            connections: AtomicU64::new(0),
        })
    }

    fn drop_connection(&self) {
        if let Some(stream) = self.current.lock().unwrap().take() {
            stream.shutdown(std::net::Shutdown::Both).unwrap();
        }
    }
}

impl Drop for DaemonConnector {
    fn drop(&mut self) {
        self.streams.close_all();
    }
}

impl Connector for DaemonConnector {
    fn connect(&self, event_sink: Arc<EventSink>) -> Result<Arc<RpcClient>> {
        self.connections.fetch_add(1, Ordering::Relaxed);
        let (client_stream, server_stream) = stream_pair()?;
        let reader = server_stream.try_clone()?;
        let closer = server_stream.try_clone()?;
        let link = Link::new(
            Box::new(server_stream),
            Some(Box::new(move || {
                let _closed = closer.shutdown(std::net::Shutdown::Both);
            })),
        );
        self.streams.connect(&link);
        let state = self.state.clone();
        let streams = self.streams.clone();
        thread::spawn(move || {
            let _served = serve_connection(reader, link.clone(), state, streams.clone());
            streams.disconnect(&link);
        });
        *self.current.lock().unwrap() = Some(client_stream.try_clone()?);
        let reader = Box::new(client_stream.try_clone()?);
        let client = RpcClient::new(reader, Box::new(client_stream), None, event_sink);
        verify_hello(&client)?;
        Ok(client)
    }
}

/// 1 接続だけの server（`serve --stdio` と同じ）に繋いだ host。
fn serve_once(root: &Path) -> (Arc<RemoteHost>, thread::JoinHandle<()>) {
    let (client_stream, server_stream) = stream_pair().expect("対を作れる");
    let server = thread::spawn(move || {
        let reader = server_stream.try_clone().expect("複製できる");
        serve_stream(reader, server_stream).expect("server が正常に終わる");
    });
    let remote = RemoteHost::connect_io(
        Box::new(client_stream.try_clone().expect("複製できる")),
        Box::new(client_stream),
        "ssh://test@example".to_string(),
        "test".to_string(),
        root,
    )
    .expect("繋がる");
    (remote, server)
}

/// 1 接続だけの server を終わらせて待つ（試験の接続は手元が切っても reader が socket を握っているので、
/// 明示的に Shutdown を送る）。
fn finish(remote: Arc<RemoteHost>, server: thread::JoinHandle<()>) {
    remote.shutdown_session().expect("Shutdown が通る");
    drop(remote);
    server.join().expect("server が終わる");
}

/// 標準入力を 1 行ずつそのまま返すコマンド（unix は `cat`・Windows は PowerShell で 1 行ごとに flush）。
fn echo_spec(cwd: &Path) -> CommandSpec {
    if cfg!(windows) {
        CommandSpec::new("powershell", cwd).args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "while (($line = [Console]::In.ReadLine()) -ne $null) { [Console]::Out.WriteLine($line); [Console]::Out.Flush() }",
        ])
    } else {
        CommandSpec::new("cat", cwd)
    }
}

fn read_line(reader: &mut impl BufRead) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).expect("行を読める");
    line.trim_end().to_string()
}

/// R1: パイプのプロセスの stdin → stdout が RPC を往復し、stdin を閉じれば終わって終了コードが届く。
#[test]
fn process_stdio_round_trips_over_the_rpc() {
    let root = scratch("echo");
    let (remote, server) = serve_once(&root);
    let mut process = remote.spawn_process(&echo_spec(&root)).expect("起こせる");
    let mut stdin = process.take_stdin().expect("stdin");
    let mut stdout = std::io::BufReader::new(process.take_stdout().expect("stdout"));
    // 起動の応答より先に書いても、溜めておいて起きてから届く。
    stdin.write_all(b"first\n").expect("書ける");
    assert_eq!(read_line(&mut stdout), "first");
    stdin.write_all(b"second\n").expect("書ける");
    assert_eq!(read_line(&mut stdout), "second");
    drop(stdin);
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).expect("最後まで読める");
    assert_eq!(rest.trim(), "", "stdin を閉じたら出力も終わる");
    let mut exit = None;
    for _ in 0..200 {
        exit = process.try_exit_status();
        if exit.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(exit.map(|exit| exit.code()), Some(Some(0)));
    drop(process);
    finish(remote, server);
    let _removed = std::fs::remove_dir_all(root);
}

/// R1: stderr の末尾と終了コードも運ぶ（すぐ落ちたエージェントの理由を見せる）。
#[test]
fn process_stderr_and_exit_code_are_carried() {
    let root = scratch("stderr");
    let (remote, server) = serve_once(&root);
    let spec = if cfg!(windows) {
        CommandSpec::new("cmd", &root).args(["/C", "echo API_KEY is not set 1>&2 & exit 3"])
    } else {
        CommandSpec::new("sh", &root).args(["-c", "echo 'API_KEY is not set' >&2; exit 3"])
    };
    let mut process = remote.spawn_process(&spec).expect("起こせる");
    let tail = process.stderr_tail();
    assert!(tail.wait_closed(Duration::from_secs(10)), "stderr が閉じる");
    // cmd の echo は `&` の前の空白も出す。
    let lines: Vec<String> = tail
        .lines()
        .iter()
        .map(|line| line.trim().to_string())
        .collect();
    assert_eq!(lines, vec!["API_KEY is not set".to_string()]);
    let exit = (0..200).find_map(|_| {
        let exit = process.try_exit_status();
        if exit.is_none() {
            thread::sleep(Duration::from_millis(10));
        }
        exit
    });
    assert_eq!(exit.map(|exit| exit.code()), Some(Some(3)));
    drop(process);
    finish(remote, server);
    let _removed = std::fs::remove_dir_all(root);
}

/// R1: 起こせないプロセスは stdout がすぐ閉じ、理由は stderr の末尾に出る（待たずに返す経路でも分かる）。
#[test]
fn a_process_that_cannot_start_reports_why() {
    let root = scratch("missing");
    let (remote, server) = serve_once(&root);
    let mut process = remote
        .spawn_process(&CommandSpec::new("necoder-no-such-program", &root))
        .expect("handle はすぐ返る");
    let mut stdout = process.take_stdout().expect("stdout");
    let mut output = Vec::new();
    stdout.read_to_end(&mut output).expect("すぐ閉じる");
    assert!(output.is_empty());
    let tail = process.stderr_tail();
    assert!(tail.wait_closed(Duration::from_secs(10)));
    assert!(
        tail.lines()
            .iter()
            .any(|line| line.contains("necoder-no-such-program")),
        "理由が残る: {:?}",
        tail.lines()
    );
    assert!(!process.is_alive());
    drop(process);
    finish(remote, server);
    let _removed = std::fs::remove_dir_all(root);
}

/// R1: 窓（1 MiB）より大きな出力も欠けず・重複せず届き、その間もファイル操作が詰まらない。
#[cfg(unix)]
#[test]
fn large_output_is_flow_controlled_and_complete() {
    let root = scratch("large");
    std::fs::write(root.join("probe.txt"), "probe").expect("書ける");
    let (remote, server) = serve_once(&root);
    let spec = CommandSpec::new("sh", &root).args([
        "-c",
        "i=0; while [ $i -lt 40000 ]; do echo \"line-$i-0123456789abcdefghijklmnopqrstuvwxyz\"; i=$((i+1)); done",
    ]);
    let mut process = remote.spawn_process(&spec).expect("起こせる");
    let stdout = std::io::BufReader::new(process.take_stdout().expect("stdout"));
    let mut lines = stdout.lines();
    // 読み手が止まっている間に、ほかの request は通る（窓で daemon が止まって待つ）。
    thread::sleep(Duration::from_millis(300));
    let started = std::time::Instant::now();
    assert!(
        remote
            .metadata(&root.join("probe.txt"))
            .expect("通る")
            .is_file
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    for expected in 0..40000 {
        let line = lines.next().expect("行がある").expect("読める");
        assert_eq!(
            line,
            format!("line-{expected}-0123456789abcdefghijklmnopqrstuvwxyz")
        );
    }
    assert!(lines.next().is_none(), "余分な行が無い");
    drop(process);
    finish(remote, server);
    let _removed = std::fs::remove_dir_all(root);
}

/// R2: 接続が切れて繋ぎ直しても、プロセスの出力は欠けず・重複せず続き、切れている間に書いた入力も届く。
#[cfg(unix)]
#[test]
fn a_process_survives_a_dropped_connection() {
    let root = scratch("resume");
    let connector = DaemonConnector::new();
    let remote = RemoteHost::connect_with(
        connector.clone() as Arc<dyn Connector>,
        "ssh://test@example".to_string(),
        &root,
    )
    .expect("繋がる");
    // 入力を 1 行ずつ番号付きで返し、合間に自分でも数えて出す。
    let spec = CommandSpec::new("sh", &root).args([
        "-c",
        "i=0; while IFS= read -r input; do echo \"got:$input\"; while [ $i -lt 200 ]; do echo \"tick-$i\"; i=$((i+1)); done; done",
    ]);
    let mut process = remote.spawn_process(&spec).expect("起こせる");
    let mut stdin = process.take_stdin().expect("stdin");
    let mut stdout = std::io::BufReader::new(process.take_stdout().expect("stdout"));
    stdin.write_all(b"one\n").expect("書ける");
    assert_eq!(read_line(&mut stdout), "got:one");
    for expected in 0..50 {
        assert_eq!(read_line(&mut stdout), format!("tick-{expected}"));
    }

    connector.drop_connection();
    // 切れている間に書いた入力は控えに積まれ、繋ぎ直したら送り直される。
    stdin.write_all(b"two\n").expect("切れていても書ける");
    assert!(
        remote.client.heartbeat_once(Duration::from_millis(500)),
        "繋ぎ直せる"
    );
    assert_eq!(connector.connections.load(Ordering::Relaxed), 2);
    for expected in 50..200 {
        assert_eq!(
            read_line(&mut stdout),
            format!("tick-{expected}"),
            "切れる前の続きから欠けずに届く"
        );
    }
    assert_eq!(
        read_line(&mut stdout),
        "got:two",
        "切れている間の入力も届く"
    );
    assert!(process.is_alive());
    drop(stdin);
    drop(process);
    drop(remote);
    let _removed = std::fs::remove_dir_all(root);
}

/// R2（どの OS でも）: 切れている間に書いた入力は控えに残り、繋ぎ直したら届く。プロセスは生きたまま。
#[test]
fn input_written_while_disconnected_is_delivered_after_reconnect() {
    let root = scratch("input-resume");
    let connector = DaemonConnector::new();
    let remote = RemoteHost::connect_with(
        connector.clone() as Arc<dyn Connector>,
        "ssh://test@example".to_string(),
        &root,
    )
    .expect("繋がる");
    let mut process = remote.spawn_process(&echo_spec(&root)).expect("起こせる");
    let mut stdin = process.take_stdin().expect("stdin");
    let mut stdout = std::io::BufReader::new(process.take_stdout().expect("stdout"));
    stdin.write_all(b"one\n").expect("書ける");
    assert_eq!(read_line(&mut stdout), "one");
    connector.drop_connection();
    stdin.write_all(b"two\n").expect("切れていても書ける");
    assert!(
        remote.client.heartbeat_once(Duration::from_millis(500)),
        "繋ぎ直せる"
    );
    assert_eq!(read_line(&mut stdout), "two", "切れている間の入力が届く");
    assert!(process.is_alive(), "プロセスは daemon で生きていた");
    drop(stdin);
    drop(process);
    drop(remote);
    let _removed = std::fs::remove_dir_all(root);
}

/// R4: 接続先の端末（PTY）でログインシェルが動き、打鍵と画面が往復し、大きさを変えられ、exit で終わる。
#[cfg(unix)]
#[test]
fn a_remote_terminal_runs_a_login_shell() {
    let root = scratch("terminal");
    let (remote, server) = serve_once(&root);
    let Some(TerminalLaunch::Remote(terminal)) =
        remote.terminal_launch(&root).expect("launch を組める")
    else {
        panic!("remote の端末は接続先の PTY");
    };
    let pty = terminal
        .open(PtySize {
            columns: 100,
            lines: 30,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("開ける");
    let mut screen = pty.reader();
    let mut keyboard = pty.writer();
    keyboard
        .write_all(b"stty size; echo answer=$((6*7)); exit 7\n")
        .expect("打てる");
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = screen.read(&mut chunk).expect("読める");
        if read == 0 {
            break;
        }
        seen.extend_from_slice(&chunk[..read]);
    }
    let seen = String::from_utf8_lossy(&seen);
    assert!(seen.contains("30 100"), "PTY の大きさ: {seen}");
    assert!(seen.contains("answer=42"), "シェルが動いた: {seen}");
    let exit = pty.wait_exit(Duration::from_secs(10));
    assert_eq!(exit.and_then(|exit| exit.code()), Some(7));
    drop(pty);
    finish(remote, server);
    let _removed = std::fs::remove_dir_all(root);
}

/// R2 / R4: 接続が切れても端末のシェルは daemon で生きていて、繋ぎ直すと切れている間の出力も見える。
#[cfg(unix)]
#[test]
fn a_remote_terminal_survives_a_dropped_connection() {
    let root = scratch("terminal-resume");
    let connector = DaemonConnector::new();
    let remote = RemoteHost::connect_with(
        connector.clone() as Arc<dyn Connector>,
        "ssh://test@example".to_string(),
        &root,
    )
    .expect("繋がる");
    let Some(TerminalLaunch::Remote(terminal)) =
        remote.terminal_launch(&root).expect("launch を組める")
    else {
        panic!("remote の端末は接続先の PTY");
    };
    let pty = terminal.open(PtySize::default()).expect("開ける");
    let mut keyboard = pty.writer();
    let mut screen = pty.reader();
    let mut seen = String::new();
    let mut read_until = |needle: &str, seen: &mut String| {
        let mut chunk = [0u8; 4096];
        while !seen.contains(needle) {
            let read = screen.read(&mut chunk).expect("読める");
            assert!(read > 0, "{needle} より前に終わった: {seen}");
            seen.push_str(&String::from_utf8_lossy(&chunk[..read]));
        }
    };
    keyboard
        .write_all(b"echo before-$((1+1))\n")
        .expect("打てる");
    read_until("before-2", &mut seen);

    connector.drop_connection();
    // 切れている間に打った分は、繋ぎ直してから届いて実行される。
    keyboard
        .write_all(b"sleep 0.2; echo during-$((2+2))\n")
        .expect("切れていても打てる");
    thread::sleep(Duration::from_millis(100));
    assert!(remote.client.heartbeat_once(Duration::from_millis(500)));
    read_until("during-4", &mut seen);
    keyboard.write_all(b"exit\n").expect("打てる");
    assert!(pty.wait_exit(Duration::from_secs(10)).is_some());
    drop(pty);
    drop(remote);
    let _removed = std::fs::remove_dir_all(root);
}

/// R5: ポート転送は手元の待ち受けから接続先の TCP へ中継する（試験では接続先も手元）。
#[test]
fn forwarded_ports_relay_tcp_both_ways() {
    let root = scratch("forward");
    let (remote, server) = serve_once(&root);
    let echo = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("待ち受けられる");
    let remote_port = echo.local_addr().expect("番号").port();
    let echo_server = thread::spawn(move || {
        let (mut connection, _) = echo.accept().expect("繋がってくる");
        let mut buffer = [0u8; 64];
        let read = connection.read(&mut buffer).expect("読める");
        connection
            .write_all(&[b"echo:", &buffer[..read]].concat())
            .expect("返せる");
    });
    let local_port = remote.forward_port(remote_port).expect("転送できる");
    assert_ne!(
        local_port, remote_port,
        "同じ番号は接続先（＝手元）が使っている"
    );
    assert_eq!(remote.forwarded_ports(), vec![(remote_port, local_port)]);
    let mut client = TcpStream::connect(("127.0.0.1", local_port)).expect("手元に繋げる");
    client.write_all(b"ping").expect("送れる");
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).expect("読める");
    assert_eq!(answer, b"echo:ping");
    echo_server.join().expect("echo が終わる");
    remote.cancel_forward(remote_port).expect("やめられる");
    assert!(remote.forwarded_ports().is_empty());
    finish(remote, server);
    let _removed = std::fs::remove_dir_all(root);
}

/// R5: 接続先の端末の `ne` は daemon → Event → 手元の client → GUI の制御口（`open_remote`）と渡り、
/// 接続先 identity は手元で刻まれる。
#[cfg(unix)]
#[test]
fn ne_in_a_remote_terminal_reaches_the_local_gui() {
    use std::os::unix::net::UnixListener;

    let root = scratch("ne");
    let gui_socket = root.join("gui.sock");
    let gui_listener = UnixListener::bind(&gui_socket).expect("GUI の口を作れる");
    // SAFETY: このテストだけが GUI の口を差し替える（ほかのテストは GUI に繋がない）。
    unsafe { std::env::set_var("NECODER_GUI_SOCK", &gui_socket) };
    let gui = thread::spawn(move || {
        let (mut stream, _) = gui_listener.accept().expect("繋がってくる");
        let mut request = String::new();
        std::io::BufReader::new(&mut stream)
            .read_line(&mut request)
            .expect("読める");
        let request: serde_json::Value = serde_json::from_str(&request).expect("JSON");
        assert_eq!(request["method"], "open_remote");
        assert_eq!(request["params"]["authority"], "ssh://test@example");
        assert_eq!(request["params"]["paths"][0], "/srv/project/README.md");
        writeln!(
            stream,
            "{}",
            serde_json::json!({ "ok": true, "result": { "opened": 1 } })
        )
        .expect("返せる");
    });
    let connector = DaemonConnector::new();
    let remote = RemoteHost::connect_with(
        connector.clone() as Arc<dyn Connector>,
        "ssh://test@example".to_string(),
        &root,
    )
    .expect("繋がる");
    let answer = connector
        .streams
        .cli_open(vec!["/srv/project/README.md".to_string()]);
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["opened"], 1);
    gui.join().expect("GUI が答えた");
    unsafe { std::env::remove_var("NECODER_GUI_SOCK") };
    drop(remote);
    let _removed = std::fs::remove_dir_all(root);
}

/// 新しい接続が来たら古い接続は切る（1 session = 1 client）。古い TCP の死を待たずに乗り換えられる。
#[test]
fn the_newest_connection_wins() {
    let streams = Arc::new(ServerStreams::default());
    let (old_client, old_server) = stream_pair().expect("対");
    let closer = old_server.try_clone().expect("複製");
    let old = Link::new(
        Box::new(old_server),
        Some(Box::new(move || {
            let _closed = closer.shutdown(std::net::Shutdown::Both);
        })),
    );
    streams.connect(&old);
    let (_new_client, new_server) = stream_pair().expect("対");
    let new = Link::new(Box::new(new_server), None);
    streams.connect(&new);
    assert!(!old.is_alive(), "古い接続は切った");
    assert!(new.is_alive());
    let mut buffer = [0u8; 1];
    assert_eq!(
        (&old_client).read(&mut buffer).expect("閉じたことが分かる"),
        0,
        "古い接続の相手は EOF を受ける"
    );
}
