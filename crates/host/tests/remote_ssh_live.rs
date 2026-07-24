//! 実 SSH ホストに対する end-to-end / 障害注入テスト（ROADMAP M13 remote 受入・障害注入）。
//!
//! 実行には稼働中の SSH リモートが要るため、既定では **skip**（`SHIRUSHI_REMOTE_TEST_URI`
//! 未設定なら全 test が早期 return）。CI や通常の `cargo test` は素通りする。
//!
//! ローカル検証（Docker の Linux ホストに対して）:
//! ```sh
//! SHIRUSHI_SSH_CONFIG=<scratch>/ssh_config \
//! SHIRUSHI_REMOTE_TEST_URI=ssh://shirushi-docker/home/dev/work/sample \
//!   cargo test -p host --test remote_ssh_live -- --nocapture --test-threads=1
//! ```
//! remote-server バイナリはホスト側の配備ロジック（`ensure_remote_server`）が
//! `~/.local/share/shirushi/remote/artifacts/<triple>/` から自動アップロードする。
#![cfg(unix)]

use host::{CommandSpec, Host, RemoteHost, SshProject, TextSearchSpec, WriteCondition};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// テスト対象の URI。未設定なら「skip」を println して None を返す。
fn test_uri() -> Option<String> {
    match std::env::var("SHIRUSHI_REMOTE_TEST_URI") {
        Ok(uri) if !uri.trim().is_empty() => Some(uri),
        _ => {
            println!("skip: SHIRUSHI_REMOTE_TEST_URI 未設定（実 SSH ホストが要る）");
            None
        }
    }
}

fn connect(uri: &str) -> Arc<RemoteHost> {
    let project = SshProject::parse(uri).expect("SSH URI をパースできる");
    RemoteHost::connect_ssh(&project, "shirushi-remote-server")
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
    if let Some(config) = std::env::var_os("SHIRUSHI_SSH_CONFIG") {
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
        entries.iter().any(|entry| entry.name == "src" && entry.is_dir),
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
    let _rm = remote.run_command(&CommandSpec::new("rm", &root).args(["-f", ".remote_edit_probe.txt"]));

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
    assert!(hits.len() >= 3, "サンプルの TODO が複数ヒットする: {hits:?}");

    // ⑤ command: run_command が remote で実行され stdout を返す
    let output = remote
        .run_command(&CommandSpec::new("uname", &root).args(["-m"]))
        .expect("uname 実行");
    assert!(output.success());
    let arch = String::from_utf8_lossy(&output.stdout);
    println!("remote uname -m = {}", arch.trim());
    assert!(!arch.trim().is_empty());
}

#[test]
fn remote_ssh_reconnects_after_control_master_kill() {
    let Some(uri) = test_uri() else { return };
    let remote = connect(&uri);
    let lib = remote.root().join("src/lib.rs");
    let before = remote.read_file(&lib).expect("最初の read");

    // ControlMaster を落とす → 多重化された RPC チャネルが切れる
    remote.debug_stop_master();
    println!("ControlMaster を kill した");

    // 次の read は ReconnectingClient が master 再生成 + 再接続で透過回復するはず
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
    out_of_band_ssh(&uri, "pkill -f shirushi-remote-server || true");
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
        .run_command(
            &CommandSpec::new("sh", &root).args([
                "-c",
                "mkdir -p .bigtree && cd .bigtree && \
                 for i in $(seq 1 5000); do : > f$i.txt; done && echo made",
            ]),
        )
        .expect("巨大 tree 作成");
    assert!(make.success(), "巨大 tree を作れる: {make:?}");

    // limit 付き list_files は limit で打ち切り、ハングも爆発もしない
    let limit = 500;
    let started = Instant::now();
    let files = remote.list_files(&big, limit).expect("巨大 tree の list_files");
    let elapsed = started.elapsed();
    println!("huge tree list_files: {} 件 / {:?}", files.len(), elapsed);
    assert!(files.len() <= limit, "limit を超えない");
    assert!(files.len() >= limit / 2, "十分な数を拾う（打ち切りが効いている）");
    assert!(elapsed < Duration::from_secs(20), "限度時間内に返る");

    // 後始末
    let _rm = remote.run_command(&CommandSpec::new("rm", &root).args(["-rf", ".bigtree"]));
    let _ = big;
}

/// リモートの latency / idle CPU / memory を実測する（ROADMAP M13 benchmark 残件）。
/// Shirushi 対 VSCode Remote の肝＝「remote 側の常駐フットプリントの軽さ」を数値で残す。
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
    println!(
        "── remote benchmark ({}) ──",
        remote.display_name()
    );
    println!(
        "round-trip read_file  x{iterations}: avg {:.3}ms / max {:.3}ms",
        avg.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );

    // ② メモリ: リモート常駐 server の合計 RSS（KB）
    let rss_kb = remote_metric(&remote, &root,
        "ps -o rss= -C shirushi-remote-server 2>/dev/null | awk '{s+=$1} END{print s+0}'");
    println!("remote server RSS 合計: {rss_kb} KB (= {:.1} MB)", rss_kb as f64 / 1024.0);

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
    assert!(idle_cpu_percent < 10.0, "idle CPU が高すぎる: {idle_cpu_percent:.2}%");
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
    out_of_band_ssh(&uri, "echo 'watched change' >> /home/dev/work/sample/README.md");

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
    let _reset = remote.run_command(
        &CommandSpec::new("git", remote.root()).args(["checkout", "--", "README.md"]),
    );
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

    // ControlMaster を落として再接続を誘発 → request 一発で確定（generation++）
    remote.debug_stop_master();
    let _reconnected = read_with_retry(&remote, &lib);
    // keeper が generation 変化を見て監視を張り直すまで待つ（1.5s tick + 余裕）
    std::thread::sleep(Duration::from_secs(3));

    // 再接続後に out-of-band 編集 → 再購読済みなら通知が来る
    out_of_band_ssh(
        &uri,
        "echo 'after reconnect' >> /home/dev/work/sample/docs/notes.md",
    );
    let event = watch
        .recv_timeout(Duration::from_secs(8))
        .expect("再接続後も変更イベントが届く（keeper が再購読した）");
    println!("watch event after reconnect: {event:?}");
    assert!(
        event.iter().any(|path| path.ends_with("notes.md")),
        "再購読後に notes.md の変更が通知される: {event:?}"
    );
    let _reset = remote.run_command(
        &CommandSpec::new("git", remote.root()).args(["checkout", "--", "docs/notes.md"]),
    );
}

#[test]
fn remote_ssh_process_respawns_after_master_kill() {
    // 再接続後の LSP/PTY handle 再同期の土台: ControlMaster が落ちても、新しいプロセスを
    // spawn し直せば ensure_master が master を張り直して stdio が通る（M13）。
    let Some(uri) = test_uri() else { return };
    use std::io::{BufRead, BufReader};
    let remote = connect(&uri);

    let spawn_echo = |marker: &str| -> String {
        // LSP/PTY と同じ経路（transport.command で別 ssh チャネル）。echo で一発出して sleep で常駐。
        let spec = CommandSpec::new("sh", remote.root())
            .args(["-c".to_string(), format!("echo {marker}; sleep 30")]);
        let mut process = remote.spawn_process(&spec).expect("プロセスを spawn");
        assert!(process.is_alive(), "spawn 直後は生きている");
        let mut reader = BufReader::new(process.take_stdout().expect("stdout"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("stdout を読む");
        line.trim().to_string()
        // process は drop → child kill（remote の sleep も SIGHUP で終わる）
    };

    // ① 通常の spawn（LSP/PTY と同じ ssh チャネル経路で stdio が通る）
    assert_eq!(spawn_echo("ready-before"), "ready-before");

    // ② ControlMaster を落とす（laptop sleep / VPN 断で LSP/PTY チャネルが死ぬのと同じ）
    remote.debug_stop_master();
    std::thread::sleep(Duration::from_millis(400));

    // ③ 再接続後に新プロセスを spawn → ensure_master が master を張り直して成功（= handle 再同期）
    assert_eq!(
        spawn_echo("ready-after"),
        "ready-after",
        "master kill 後も新しいプロセスを spawn して stdio が通る"
    );
}

/// remote-server 全プロセスの utime+stime（jiffies）合計を出す sh スクリプト。
fn proc_jiffies_script() -> String {
    // /proc/<pid>/stat の 14,15 番目（utime,stime）を全 remote-server 分足す。
    "total=0; for pid in $(pgrep -f shirushi-remote-server); do \
       set -- $(cut -d' ' -f14,15 /proc/$pid/stat 2>/dev/null); \
       total=$((total + ${1:-0} + ${2:-0})); \
     done; echo $total"
        .to_string()
}
