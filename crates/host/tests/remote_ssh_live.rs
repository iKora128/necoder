//! 実 SSH ホストに対する end-to-end / 障害注入テスト（ROADMAP M13 remote 受入・障害注入）。
//!
//! 実行には稼働中の SSH リモートが要るため、既定では **skip**（`NECODER_REMOTE_TEST_URI`
//! 未設定なら全 test が早期 return）。CI や通常の `cargo test` は素通りする。
//!
//! ローカル検証（Docker の Linux ホストに対して・`scripts/test-remote-ssh-docker.sh` が面倒を見る）:
//! ```sh
//! NECODER_SSH_CONFIG=<scratch>/ssh_config \
//! NECODER_REMOTE_TEST_URI=ssh://necoder-docker/home/dev/work/sample \
//!   cargo test -p host --test remote_ssh_live -- --nocapture --test-threads=1
//! ```
//! Windows の client からは `scripts/test-remote-ssh-wsl.ps1`（WSL のユーザー権限の sshd へ繋ぐ）。
//! remote-server バイナリは接続の bootstrap（`docs/REMOTE-CHANNEL.md` §3）が
//! `~/.local/share/necoder/remote/artifacts/<triple>/`（または `NECODER_REMOTE_SERVER_BINARY`）から
//! 同じ接続の中で送り込む。

use host::{CommandSpec, Host, RemoteHost, SshProject, TextSearchSpec, WriteCondition};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// テスト対象の URI。未設定なら「skip」を println して None を返す。
fn test_uri() -> Option<String> {
    match std::env::var("NECODER_REMOTE_TEST_URI") {
        Ok(uri) if !uri.trim().is_empty() => Some(uri),
        _ => {
            println!("skip: NECODER_REMOTE_TEST_URI 未設定（実 SSH ホストが要る）");
            None
        }
    }
}

fn connect(uri: &str) -> Arc<RemoteHost> {
    let project = SshProject::parse(uri).expect("SSH URI をパースできる");
    RemoteHost::connect_ssh(&project, "necoder-remote-server")
        .expect("SSH 接続 + remote-server 配備に成功する")
}

/// URI から out-of-band な ssh の宛先（`[user@]host`）を取り出す。alias 形（config で解決）想定。
fn ssh_destination(uri: &str) -> String {
    let authority = uri
        .strip_prefix("ssh://")
        .unwrap_or(uri)
        .split('/')
        .next()
        .unwrap_or("");
    // port 付き（host:port）は alias 検証では使わないので単純化（host 部だけ返す）。
    authority.to_string()
}

/// 障害注入用に別チャネルの ssh でリモートコマンドを実行（本体の接続とは独立）。
fn out_of_band_ssh(uri: &str, remote_command: &str) {
    let mut command = Command::new("ssh");
    if let Some(config) = std::env::var_os("NECODER_SSH_CONFIG") {
        command.arg("-F").arg(config);
    }
    let status = command
        .arg(ssh_destination(uri))
        .arg(remote_command)
        .status()
        .expect("out-of-band ssh を起動できる");
    println!("out-of-band ssh [{remote_command}] -> {status}");
}

/// 接続断からの回復にわずかな時間がかかることがあるので、read を短くリトライする。
fn read_with_retry(remote: &RemoteHost, path: &std::path::Path) -> host::FileContent {
    let mut last_error = None;
    for attempt in 0..6 {
        match remote.read_file(path) {
            Ok(content) => return content,
            Err(error) => {
                println!("read リトライ {attempt}: {error:#}");
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(400));
            }
        }
    }
    panic!("再接続後も read に失敗: {:?}", last_error);
}

/// 接続先の端末の `ne .` が、接続先の GUI ではなく接続元へ戻る。daemon の `ne` の受け口 → Event →
/// 手元の client → GUI の制御口（`open_remote`）と渡り、SSH authority は手元で刻まれる。
#[cfg(unix)]
#[test]
fn remote_terminal_ne_opens_on_the_client() {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::os::unix::net::UnixListener;

    let Some(uri) = test_uri() else { return };
    let gui_socket =
        std::path::PathBuf::from(format!("/tmp/necoder-live-gui-{}.sock", std::process::id()));
    let _stale = std::fs::remove_file(&gui_socket);
    let listener = UnixListener::bind(&gui_socket).expect("偽 GUI socket を bind できる");
    // SAFETY: このテストだけが GUI の口を差し替える（--test-threads=1 で走らせる前提）。
    unsafe { std::env::set_var("NECODER_GUI_SOCK", &gui_socket) };
    let remote = connect(&uri);

    let expected_authority = SshProject::parse(&uri).unwrap().identity();
    let expected_path = remote.root().to_string_lossy().to_string();
    let gui = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("client が GUI へ接続する");
        let mut line = String::new();
        BufReader::new(&mut stream)
            .read_line(&mut line)
            .expect("GUI request を読める");
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "open_remote");
        assert_eq!(request["params"]["authority"], expected_authority);
        assert_eq!(request["params"]["paths"][0], expected_path);
        writeln!(
            stream,
            "{}",
            serde_json::json!({ "ok": true, "result": { "opened": 1 } })
        )
        .unwrap();
    });

    let pty = open_terminal(&remote);
    pty.writer()
        .write_all(b"ne .\nexit\n")
        .expect("ne を入力できる");
    let mut screen = Vec::new();
    pty.reader()
        .read_to_end(&mut screen)
        .expect("端末が終わるまで読める");
    let screen = String::from_utf8_lossy(&screen);
    assert!(
        screen.contains("接続元の necoder で開きました"),
        "ne の成功表示が無い: {screen:?}"
    );
    gui.join().unwrap();
    unsafe { std::env::remove_var("NECODER_GUI_SOCK") };
    drop(pty);
    drop(remote);
    let _cleanup = std::fs::remove_file(gui_socket);
}

fn open_terminal(remote: &RemoteHost) -> host::HostPty {
    let Some(host::TerminalLaunch::Remote(terminal)) = remote
        .terminal_launch(remote.root())
        .expect("terminal launch を組める")
    else {
        panic!("remote の端末は接続先の PTY");
    };
    terminal
        .open(host::PtySize::default())
        .expect("端末を開ける")
}

/// 端末の画面を `needle` が出るまで読む。
fn read_screen_until(screen: &mut dyn std::io::Read, needle: &str) -> String {
    let mut seen = String::new();
    let mut chunk = [0u8; 4096];
    let started = Instant::now();
    while !seen.contains(needle) {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{needle} が出ない: {seen}"
        );
        let read = screen.read(&mut chunk).expect("画面を読める");
        assert!(read > 0, "{needle} より前に端末が終わった: {seen}");
        seen.push_str(&String::from_utf8_lossy(&chunk[..read]));
    }
    seen
}

/// **端末は接続が切れても死なない**（スリープ・VPN 断の代わりに ssh を落とす）。シェルは daemon で
/// 生きていて、切れている間に打った分も、繋ぎ直した後に実行されて画面に出る。
#[test]
fn remote_ssh_terminal_survives_a_dropped_connection() {
    use std::io::Write as _;

    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let pty = open_terminal(&remote);
    let mut keyboard = pty.writer();
    let mut screen = pty.reader();
    keyboard
        .write_all(b"export NECODER_LIVE=kept; echo before-$((1+1))\n")
        .expect("打てる");
    read_screen_until(&mut screen, "before-2");

    remote.debug_drop_connection();
    println!("ssh を落とした");
    keyboard
        .write_all(b"echo during-$NECODER_LIVE-$((2+2))\n")
        .expect("切れていても打てる");
    // 次の操作（または heartbeat）が繋ぎ直す。
    let _reconnected = read_with_retry(&remote, &remote.root().join("src/lib.rs"));
    read_screen_until(&mut screen, "during-kept-4");
    assert!(remote.debug_reconnect_generation() > 0, "繋ぎ直している");
    keyboard.write_all(b"exit\n").expect("打てる");
    assert!(pty.wait_exit(Duration::from_secs(10)).is_some());
}

#[test]
fn remote_ssh_crud_search_command() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let root = remote.root().to_path_buf();
    println!("connected: root={}", root.display());

    // ① tree: list_files / read_dir がプロジェクト構成を返す
    let files = remote.list_files(&root, 100).expect("list_files");
    println!("list_files: {} 件", files.len());
    assert!(
        files.iter().any(|path| path.ends_with("src/main.rs")),
        "src/main.rs が列挙される: {files:?}"
    );
    let entries = remote.read_dir(&root).expect("read_dir");
    assert!(
        entries
            .iter()
            .any(|entry| entry.name == "src" && entry.is_dir),
        "src ディレクトリが read_dir に出る"
    );

    // ② open: read_file が内容を返す（revision 付き）
    let lib = root.join("src/lib.rs");
    let content = remote.read_file(&lib).expect("read src/lib.rs");
    assert!(
        String::from_utf8_lossy(&content.bytes).contains("pub fn add"),
        "lib.rs の中身が読める"
    );

    // ③ edit + save: 新規作成 → 更新（revision 一致）→ 古い revision で更新（conflict）
    let probe = root.join(".remote_edit_probe.txt");
    let first = remote
        .write_file(&probe, b"one", WriteCondition::NotExists)
        .expect("新規作成");
    let after = read_with_retry(&remote, &probe);
    assert_eq!(after.bytes, b"one");
    let _second = remote
        .write_file(&probe, b"two", WriteCondition::Matches(first))
        .expect("revision 一致で更新");
    assert_eq!(read_with_retry(&remote, &probe).bytes, b"two");
    // 古い（stale）revision での上書きは拒否される = 競合検出
    let stale = remote.write_file(&probe, b"three", WriteCondition::Matches(first));
    assert!(stale.is_err(), "stale revision の write は拒否される");
    // 後始末
    let _rm =
        remote.run_command(&CommandSpec::new("rm", &root).args(["-f", ".remote_edit_probe.txt"]));

    // ④ search: TODO をプロジェクト横断で拾う
    let hits = remote
        .search_project(
            &root,
            &TextSearchSpec {
                pattern: "TODO".to_string(),
                is_regex: false,
                case_sensitive: false,
                max_matches: 50,
            },
            100,
        )
        .expect("search_project");
    println!("search TODO: {} hits", hits.len());
    assert!(
        hits.len() >= 3,
        "サンプルの TODO が複数ヒットする: {hits:?}"
    );

    // ⑤ command: run_command が remote で実行され stdout を返す
    let output = remote
        .run_command(&CommandSpec::new("uname", &root).args(["-m"]))
        .expect("uname 実行");
    assert!(output.success());
    let arch = String::from_utf8_lossy(&output.stdout);
    println!("remote uname -m = {}", arch.trim());
    assert!(!arch.trim().is_empty());
}

/// 疎通を**黙って**止める / 戻すコマンド（`docker pause` / `unpause` 等）。
/// 両方揃っているときだけ該当 test を走らせる。
///
/// ssh を落とすのとの違いが肝: 落とすと接続が閉じる（EOF が届く）が、凍結は
/// **TCP を張ったまま相手が黙る**。laptop の sleep 復帰直後がこの形で、Ping の時間切れでしか気付けない。
fn freeze_commands() -> Option<(String, String)> {
    let read = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|command| !command.trim().is_empty())
    };
    match (
        read("NECODER_REMOTE_TEST_FREEZE"),
        read("NECODER_REMOTE_TEST_UNFREEZE"),
    ) {
        (Some(freeze), Some(unfreeze)) => Some((freeze, unfreeze)),
        _ => {
            println!(
                "skip: NECODER_REMOTE_TEST_FREEZE / _UNFREEZE 未設定（疎通を止める手段が要る）"
            );
            None
        }
    }
}

fn run_shell(command: &str) -> bool {
    let mut shell = if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.arg("/C");
        shell
    } else {
        let mut shell = Command::new("sh");
        shell.arg("-c");
        shell
    };
    match shell.arg(command).status() {
        Ok(status) => {
            println!("shell [{command}] -> {status}");
            status.success()
        }
        Err(error) => {
            println!("shell [{command}] を起動できない: {error}");
            false
        }
    }
}

/// 凍結を必ず解く番人。panic しても後続の test を巻き込まないように Drop で戻す。
struct FrozenLink {
    unfreeze: String,
    thawed: bool,
}

impl FrozenLink {
    fn thaw(&mut self) {
        if !self.thawed {
            self.thawed = run_shell(&self.unfreeze);
        }
    }
}

impl Drop for FrozenLink {
    fn drop(&mut self) {
        self.thaw();
    }
}

/// **無言で死んだ回線から自力で復帰する**（2026-09-07・ユーザー報告「sleep 復帰後に固まる」の実 SSH 版）。
///
/// 相手を凍結すると、TCP は張られたまま応答だけが消える。heartbeat が
/// [`host`] 側の Ping timeout（5s）で気付き、古い接続に乗った request を落として
/// ssh を張り直す。疎通が戻ったら**ユーザーが何もしなくても**読めるようになること、
/// そしてそれが**有界な時間**で起きることを見る（直す前は ServerAlive が気付く 45s 級を待っていた）。
#[test]
fn remote_ssh_recovers_from_a_silently_dead_link() {
    let Some(uri) = test_uri() else { return };
    let Some((freeze, unfreeze)) = freeze_commands() else {
        return;
    };
    let remote = connect(&uri);
    let lib = remote.root().join("src/lib.rs");
    let before = remote.read_file(&lib).expect("最初の read");

    assert!(run_shell(&freeze), "疎通を止められなかった");
    let mut frozen = FrozenLink {
        unfreeze,
        thawed: false,
    };
    // heartbeat（5s 間隔・Ping timeout 5s）が「黙っている」と判定して張り直しに入るまで待つ。
    std::thread::sleep(Duration::from_secs(12));
    frozen.thaw();

    // 疎通が戻った後、操作が通るまで。ここが伸びるのが「復帰後しばらく固まる」の正体。
    let started = Instant::now();
    let deadline = Duration::from_secs(45);
    let mut last_error = None;
    let mut recovered = None;
    while started.elapsed() < deadline {
        match remote.read_file(&lib) {
            Ok(content) => {
                recovered = Some(content);
                break;
            }
            Err(error) => {
                println!("復帰待ち {:?}: {error:#}", started.elapsed());
                last_error = Some(error);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    let elapsed = started.elapsed();
    let content = recovered
        .unwrap_or_else(|| panic!("{deadline:?} 経っても復帰しない（最後の失敗: {last_error:?}）"));
    assert_eq!(content.bytes, before.bytes, "復帰後も同じ内容を読める");
    // 「たまたま元の接続が生き返った」のではなく、黙っていることを検知して張り直したこと。
    // 凍結は heartbeat の Ping timeout（5s）より長いので、必ず 1 回は世代が進む。
    assert!(
        remote.debug_reconnect_generation() > 0,
        "無言断を検知して張り直していない（Ping が時間切れになれば世代が進むはず）"
    );
    println!(
        "無言断からの復帰: {elapsed:?}（再接続 {} 回）",
        remote.debug_reconnect_generation()
    );
}

#[test]
fn remote_ssh_reconnects_after_the_connection_drops() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let lib = remote.root().join("src/lib.rs");
    let before = remote.read_file(&lib).expect("最初の read");

    // ssh を落とす → RPC の接続が切れる（スリープ・VPN 断と同じ）
    remote.debug_drop_connection();
    println!("ssh を落とした");

    // 次の read は ReconnectingClient が繋ぎ直して透過回復するはず
    let after = read_with_retry(&remote, &lib);
    assert_eq!(after.bytes, before.bytes, "再接続後も同じ内容を読める");
}

#[test]
fn remote_ssh_reconnects_after_server_kill() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let lib = remote.root().join("src/lib.rs");
    let before = remote.read_file(&lib).expect("最初の read");

    // remote の server プロセス（daemon + proxy）を別チャネルで kill
    out_of_band_ssh(&uri, "pkill -f necoder-remote-server || true");
    println!("remote server を kill した");
    std::thread::sleep(Duration::from_millis(500));

    // 再接続で新しい daemon が立ち上がり project を開き直す（scoped_request が OpenProject 再送）
    let after = read_with_retry(&remote, &lib);
    assert_eq!(after.bytes, before.bytes, "server 再起動後も内容を読める");
}

#[test]
fn remote_ssh_huge_tree_respects_limit() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let root = remote.root().to_path_buf();

    // 巨大 tree を作る（5000 ファイル・1 コマンドで）。gitignore されない素の階層。
    let big = root.join(".bigtree");
    let make = remote
        .run_command(&CommandSpec::new("sh", &root).args([
            "-c",
            "mkdir -p .bigtree && cd .bigtree && \
                 for i in $(seq 1 5000); do : > f$i.txt; done && echo made",
        ]))
        .expect("巨大 tree 作成");
    assert!(make.success(), "巨大 tree を作れる: {make:?}");

    // limit 付き list_files は limit で打ち切り、ハングも爆発もしない
    let limit = 500;
    let started = Instant::now();
    let files = remote
        .list_files(&big, limit)
        .expect("巨大 tree の list_files");
    let elapsed = started.elapsed();
    println!("huge tree list_files: {} 件 / {:?}", files.len(), elapsed);
    assert!(files.len() <= limit, "limit を超えない");
    assert!(
        files.len() >= limit / 2,
        "十分な数を拾う（打ち切りが効いている）"
    );
    assert!(elapsed < Duration::from_secs(20), "限度時間内に返る");

    // 後始末
    let _rm = remote.run_command(&CommandSpec::new("rm", &root).args(["-rf", ".bigtree"]));
    let _ = big;
}

/// リモートの latency / idle CPU / memory を実測する（ROADMAP M13 benchmark 残件）。
/// necoder 対 VSCode Remote の肝＝「remote 側の常駐フットプリントの軽さ」を数値で残す。
/// localhost（Docker）前提の緩い予算で自動検証しつつ、実数を stdout に出す。
#[test]
fn remote_ssh_bench_latency_and_memory() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let root = remote.root().to_path_buf();
    let lib = root.join("src/lib.rs");

    // ① round-trip latency: read_file を繰り返し、平均/最大を測る
    let iterations = 200;
    let mut max = Duration::ZERO;
    let started = Instant::now();
    for _ in 0..iterations {
        let call = Instant::now();
        remote.read_file(&lib).expect("read");
        max = max.max(call.elapsed());
    }
    let total = started.elapsed();
    let avg = total / iterations;
    println!("── remote benchmark ({}) ──", remote.display_name());
    println!(
        "round-trip read_file  x{iterations}: avg {:.3}ms / max {:.3}ms",
        avg.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );

    // ② メモリ: リモート常駐 server の合計 RSS（KB）
    let rss_kb = remote_metric(
        &remote,
        &root,
        "ps -o rss= -C necoder-remote-server 2>/dev/null | awk '{s+=$1} END{print s+0}'",
    );
    println!(
        "remote server RSS 合計: {rss_kb} KB (= {:.1} MB)",
        rss_kb as f64 / 1024.0
    );

    // ③ idle CPU: 2 秒間アイドルさせて server の CPU jiffies 差分から % を出す
    let cpu0 = remote_metric(&remote, &root, &proc_jiffies_script());
    std::thread::sleep(Duration::from_secs(2));
    let cpu1 = remote_metric(&remote, &root, &proc_jiffies_script());
    let hz = 100.0; // Linux 既定 CLK_TCK
    let idle_cpu_percent = ((cpu1.saturating_sub(cpu0)) as f64 / hz) / 2.0 * 100.0;
    println!("idle CPU（2秒窓）: {idle_cpu_percent:.2}%");
    println!("─────────────────────────");

    // 予算（localhost・緩め）。VSCode の node server（数百MB）と桁が違うことを保証する。
    assert!(
        rss_kb > 0 && rss_kb < 60_000,
        "remote server RSS が異常（{rss_kb} KB）— musl static は数MB のはず"
    );
    assert!(
        avg < Duration::from_millis(200),
        "localhost の round-trip が遅すぎる: {avg:?}"
    );
    assert!(
        idle_cpu_percent < 10.0,
        "idle CPU が高すぎる: {idle_cpu_percent:.2}%"
    );
}

/// リモートで整数を1つ返すコマンドを実行し、その値を読む（ベンチ計測用）。
fn remote_metric(remote: &RemoteHost, root: &std::path::Path, script: &str) -> u64 {
    let output = remote
        .run_command(&CommandSpec::new("sh", root).args(["-c", script]))
        .expect("metric コマンド実行");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

#[test]
fn remote_ssh_watch_pushes_external_edits() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);

    // watch 開始（Host trait 経由）→ daemon が poll 監視を始める
    let watch = remote
        .clone()
        .watch()
        .expect("watch 開始")
        .expect("remote host は watch を返す");
    // daemon が初回スナップショットを取るまで待つ
    std::thread::sleep(Duration::from_millis(400));

    // out-of-band（別 ssh）で project 内のファイルを編集
    out_of_band_ssh(
        &uri,
        &format!(
            "echo 'watched change' >> '{}/README.md'",
            remote.root().display()
        ),
    );

    // poll 間隔（700ms）内に変更イベントが push される
    let event = watch
        .recv_timeout(Duration::from_secs(6))
        .expect("外部編集の変更イベントが届く");
    println!("watch event: {event:?}");
    assert!(
        event.iter().any(|path| path.ends_with("README.md")),
        "README.md の変更が通知される: {event:?}"
    );

    // 後始末（README を元に戻す）
    let _reset = remote.run_command(&CommandSpec::new("git", remote.root()).args([
        "checkout",
        "--",
        "README.md",
    ]));
}

#[test]
fn remote_ssh_watch_resubscribes_after_reconnect() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let lib = remote.root().join("src/lib.rs");
    let watch = remote
        .clone()
        .watch()
        .expect("watch 開始")
        .expect("remote host は watch を返す");
    std::thread::sleep(Duration::from_millis(400));

    // ssh を落として再接続を誘発 → request 一発で確定（generation++）
    remote.debug_drop_connection();
    let _reconnected = read_with_retry(&remote, &lib);
    // keeper が generation 変化を見て監視を張り直すまで待つ（1.5s tick + 余裕）
    std::thread::sleep(Duration::from_secs(3));

    // 再接続後に out-of-band 編集 → 再購読済みなら通知が来る
    out_of_band_ssh(
        &uri,
        &format!(
            "echo 'after reconnect' >> '{}/docs/notes.md'",
            remote.root().display()
        ),
    );
    let event = watch
        .recv_timeout(Duration::from_secs(8))
        .expect("再接続後も変更イベントが届く（keeper が再購読した）");
    println!("watch event after reconnect: {event:?}");
    assert!(
        event.iter().any(|path| path.ends_with("notes.md")),
        "再購読後に notes.md の変更が通知される: {event:?}"
    );
    let _reset = remote.run_command(&CommandSpec::new("git", remote.root()).args([
        "checkout",
        "--",
        "docs/notes.md",
    ]));
}

#[test]
fn remote_ssh_process_survives_a_dropped_connection() {
    // LSP / ACP と同じ経路（RPC のストリーム）。接続が切れてもプロセスは daemon で生きていて、
    // 切れている間に書いた stdin も、繋ぎ直した後に届いて続きが読める（以前は ssh の子として死んでいた）。
    let Some(uri) = test_uri() else { return };
    use std::io::{BufRead, BufReader, Write as _};
    let remote = connect(&uri);
    let spec = CommandSpec::new("sh", remote.root()).args([
        "-c",
        "echo ready; while IFS= read -r line; do echo \"got:$line\"; done",
    ]);
    let mut process = remote.spawn_process(&spec).expect("プロセスを spawn");
    let mut stdin = process.take_stdin().expect("stdin");
    let mut reader = BufReader::new(process.take_stdout().expect("stdout"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("stdout を読む");
    assert_eq!(line.trim(), "ready");

    // ssh を落とす（laptop sleep / VPN 断で接続が死ぬのと同じ）
    remote.debug_drop_connection();
    stdin
        .write_all(b"while-away\n")
        .expect("切れていても書ける");
    let _reconnected = read_with_retry(&remote, &remote.root().join("src/lib.rs"));

    line.clear();
    reader.read_line(&mut line).expect("繋ぎ直した後も読める");
    assert_eq!(line.trim(), "got:while-away", "切れている間の入力が届いた");
    assert!(process.is_alive(), "プロセスは daemon で生きていた");
}

/// remote-server 全プロセスの utime+stime（jiffies）合計を出す sh スクリプト。
fn proc_jiffies_script() -> String {
    // /proc/<pid>/stat の 14,15 番目（utime,stime）を全 remote-server 分足す。
    "total=0; for pid in $(pgrep -f necoder-remote-server); do \
       set -- $(cut -d' ' -f14,15 /proc/$pid/stat 2>/dev/null); \
       total=$((total + ${1:-0} + ${2:-0})); \
     done; echo $total"
        .to_string()
}
