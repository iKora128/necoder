#![cfg(unix)]

use host::{Host, RemoteHost, WriteCondition};
use std::process::Command;

#[test]
fn proxy_bootstraps_daemon_and_serves_real_protocol() {
    let scratch = std::env::temp_dir().join(format!("shirushi-daemon-test-{}", std::process::id()));
    let _cleanup = std::fs::remove_dir_all(&scratch);
    let project = scratch.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let file = project.join("hello.txt");
    std::fs::write(&file, "before").unwrap();

    let session = format!("{:016x}{:048x}", std::process::id(), 0);
    let proxy = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_shirushi-remote-server"));
        command
            .args(["proxy", "--session", &session])
            .env("HOME", &scratch);
        command
    };
    let remote = RemoteHost::connect_process(
        proxy(),
        "daemon-test".to_string(),
        "Daemon Test".to_string(),
        &project,
    )
    .unwrap();

    let remote_file = remote.root().join("hello.txt");
    let content = remote.read_file(&remote_file).unwrap();
    remote
        .write_file(
            &remote_file,
            b"after",
            WriteCondition::Matches(content.revision),
        )
        .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");

    // SSH proxy が落ちても daemon は session ID ごと残り、次の proxy が同じ状態へ再接続できる。
    drop(remote);
    let reconnected = RemoteHost::connect_process(
        proxy(),
        "daemon-test".to_string(),
        "Daemon Test".to_string(),
        &project,
    )
    .unwrap();
    let content = reconnected
        .read_file(&reconnected.root().join("hello.txt"))
        .unwrap();
    assert_eq!(content.bytes, b"after");

    reconnected.shutdown_session().unwrap();
    drop(reconnected);
    std::fs::remove_dir_all(scratch).unwrap();
}
