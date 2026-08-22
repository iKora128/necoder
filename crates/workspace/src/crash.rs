//! crash — panic hook とクラッシュログ + バグ報告 URL（M13 公開準備）。
//!
//! 方針: telemetry は採らない（FEATURES §13 never）。落ちたらローカルにログを書き、
//! 次回起動時に statusbar チップで 1 回だけ知らせ、クリックで GitHub の new issue
//! （本文に環境とログ抜粋を事前記入）をブラウザで開く。ネットへは一切自動送信しない。
//!
//! 流れ: main() 冒頭で [`install_panic_hook`] → panic 時に `crashes/crash-<unix秒>-<pid>.log`
//! と `pending` マーカーを書く（既定 hook にも連鎖 = stderr の表示はそのまま）→ 次回起動の
//! [`take_pending_crash`] がマーカーを消費 → チップ → [`bug_report_url`] + [`open_url`]。

use std::path::{Path, PathBuf};

/// バグ報告先（GitHub new issue）。updater の RELEASES_URL と同じリポジトリ。
const NEW_ISSUE_URL: &str = "https://github.com/iKora128/necoder/issues/new";

/// Issue 本文に貼るログ抜粋の上限（URL に載せるため控えめに。全文はローカルに残る）。
const EXCERPT_MAX_BYTES: usize = 1800;

/// 残すクラッシュログの数（古いものから消す）。
const KEEP_LOGS: usize = 20;

/// クラッシュログの置き場（`~/Library/Application Support/necoder/crashes/`）。
/// テスト・offscreen 検証は `NECODER_CRASH_DIR` で差し替える（ユーザーの実ログを触らない）。
pub fn crash_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NECODER_CRASH_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/necoder/crashes"))
}

/// panic hook を仕込む（main() 冒頭・GPUI 起動前に呼ぶ）。どのスレッドの panic でも
/// クラッシュログと `pending` マーカーを書き、既定 hook（stderr 表示）へ連鎖する。
pub fn install_panic_hook() {
    let Some(dir) = crash_dir() else {
        return;
    };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // ログが書けなくても panic 処理自体は止めない（既定 hook の stderr が最後の砦）。
        if let Err(error) = write_crash_log(&dir, info) {
            eprintln!("クラッシュログを書けない: {error}");
        }
        previous(info);
    }));
}

/// クラッシュログ 1 本を書き、`pending` マーカー（次回起動の通知フラグ）を更新する。
fn write_crash_log(dir: &Path, info: &std::panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(payload 不明)".to_string());
    let location = info
        .location()
        .map(|location| location.to_string())
        .unwrap_or_else(|| "(場所不明)".to_string());
    let thread = std::thread::current()
        .name()
        .unwrap_or("(無名スレッド)")
        .to_string();
    let backtrace = std::backtrace::Backtrace::force_capture();
    let body = format!(
        "necoder v{version}\nos: {os} {arch}\ntime(unix): {unix_seconds}\nthread: {thread}\nlocation: {location}\npanic: {payload}\n\n{backtrace}\n",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    );
    let log_path = dir.join(format!("crash-{unix_seconds}-{}.log", std::process::id()));
    std::fs::write(&log_path, body)?;
    std::fs::write(dir.join("pending"), log_path.to_string_lossy().as_bytes())?;
    prune_old_logs(dir);
    Ok(())
}

/// 古いクラッシュログを掃除する（新しい [`KEEP_LOGS`] 本だけ残す）。
fn prune_old_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("crash-") && name.ends_with(".log"))
        })
        .collect();
    // ファイル名 = crash-<unix秒>-<pid>.log なので名前順 ≒ 時刻順。
    logs.sort();
    while logs.len() > KEEP_LOGS {
        let _ = std::fs::remove_file(logs.remove(0));
    }
}

/// 前回クラッシュのログパスを返し、`pending` マーカーを消費する（通知は 1 回だけ）。
/// ログ本体は crashes/ に残る（Issue に貼る全文・後からの調査用）。
pub fn take_pending_crash() -> Option<PathBuf> {
    take_pending_crash_in(&crash_dir()?)
}

fn take_pending_crash_in(dir: &Path) -> Option<PathBuf> {
    let marker = dir.join("pending");
    let recorded = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let log_path = PathBuf::from(recorded.trim());
    log_path.exists().then_some(log_path)
}

/// GitHub new issue の URL を組み立てる（title + 本文に環境とログ抜粋を事前記入）。
/// crash_log があれば末尾抜粋を fenced block で貼る。ネット送信はしない（URL を返すだけ）。
pub fn bug_report_url(crash_log: Option<&Path>) -> String {
    let title = if crash_log.is_some() {
        "Crash report"
    } else {
        "Bug report"
    };
    let mut body = String::new();
    body.push_str(
        "## What happened / 何が起きたか\n\n\n\n## Steps to reproduce / 再現手順\n\n1. \n\n",
    );
    body.push_str(&format!(
        "## Environment / 環境\n\n- necoder v{}\n- {} {}{}\n- locale: {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        os_version()
            .map(|version| format!(" ({version})"))
            .unwrap_or_default(),
        i18n::locale(),
    ));
    if let Some(log_path) = crash_log {
        body.push_str(&format!(
            "\n## Crash log（`{}` の抜粋）\n\n```\n{}\n```\n",
            log_path.display(),
            crash_log_excerpt(log_path),
        ));
    }
    format!(
        "{NEW_ISSUE_URL}?title={}&body={}",
        percent_encode(title),
        percent_encode(&body)
    )
}

/// クラッシュログの抜粋（ヘッダ + backtrace 先頭）。URL に載るよう [`EXCERPT_MAX_BYTES`] で切る。
fn crash_log_excerpt(log_path: &Path) -> String {
    let Ok(full) = std::fs::read_to_string(log_path) else {
        return "(ログを読めない)".to_string();
    };
    truncate_on_line(&full, EXCERPT_MAX_BYTES)
}

/// 行単位で `max_bytes` に収める（途中で切れた行は捨て、切ったことを示す 1 行を足す）。
fn truncate_on_line(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.trim_end().to_string();
    }
    let mut out = String::new();
    for line in text.lines() {
        if out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("… (truncated)");
    out
}

/// OS のバージョン表記（macOS: `sw_vers -productVersion`）。取れなければ None（静かに）。
fn os_version() -> Option<String> {
    if std::env::consts::OS != "macos" {
        return None;
    }
    let output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then(|| format!("macOS {version}"))
}

/// RFC 3986 unreserved 以外を %XX にする（URL クエリ用・依存ゼロ）。
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// URL を既定ブラウザで開く（macOS: `open` / それ以外: `xdg-open`）。
pub fn open_url(url: &str) -> anyhow::Result<()> {
    let command = if std::env::consts::OS == "macos" {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(command)
        .arg(url)
        .status()
        .map_err(|error| anyhow::anyhow!("ブラウザを開けない（{command}）: {error}"))?;
    anyhow::ensure!(status.success(), "ブラウザを開けない（{command} が失敗）");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("necoder_crash_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pending_marker_round_trip_consumes_once() {
        let dir = temp_dir("pending");
        let log = dir.join("crash-100-1.log");
        std::fs::write(&log, "necoder v0.0.0\npanic: test\n").unwrap();
        std::fs::write(dir.join("pending"), log.to_string_lossy().as_bytes()).unwrap();

        // 1 回目 = ログパスが取れてマーカーは消える。ログ本体は残る。
        assert_eq!(take_pending_crash_in(&dir), Some(log.clone()));
        assert!(log.exists());
        // 2 回目 = もう出ない（通知は 1 回だけ）。
        assert_eq!(take_pending_crash_in(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_marker_pointing_to_missing_log_is_ignored() {
        let dir = temp_dir("missing");
        std::fs::write(dir.join("pending"), "/no/such/crash.log").unwrap();
        assert_eq!(take_pending_crash_in(&dir), None);
        assert!(!dir.join("pending").exists()); // マーカーは消費される
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_only_newest_logs() {
        let dir = temp_dir("prune");
        for index in 0..(KEEP_LOGS + 5) {
            std::fs::write(dir.join(format!("crash-{index:04}-1.log")), "x").unwrap();
        }
        std::fs::write(dir.join("pending"), "unrelated").unwrap(); // マーカーは掃除対象外
        prune_old_logs(&dir);
        let remaining: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("crash-"))
            .collect();
        assert_eq!(remaining.len(), KEEP_LOGS);
        assert!(dir.join("pending").exists());
        // 消えたのは古い方（0000〜0004）。
        assert!(!dir.join("crash-0000-1.log").exists());
        assert!(dir
            .join(format!("crash-{:04}-1.log", KEEP_LOGS + 4))
            .exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bug_report_url_embeds_environment_and_excerpt() {
        let dir = temp_dir("url");
        let log = dir.join("crash-1-1.log");
        std::fs::write(
            &log,
            "necoder v0.1.0\npanic: boom at src/x.rs:1\nbacktrace line\n",
        )
        .unwrap();

        let url = bug_report_url(Some(&log));
        assert!(url.starts_with(
            "https://github.com/iKora128/necoder/issues/new?title=Crash%20report&body="
        ));
        // 本文（URL エンコード済み）にバージョンと panic 行が含まれる。
        assert!(url.contains(&percent_encode(&format!(
            "necoder v{}",
            env!("CARGO_PKG_VERSION")
        ))));
        assert!(url.contains(&percent_encode("panic: boom at src/x.rs:1")));

        // ログ無し = Bug report タイトルで、Crash log 節が無い。
        let url = bug_report_url(None);
        assert!(url.contains("title=Bug%20report"));
        assert!(!url.contains(&percent_encode("## Crash log")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn excerpt_truncates_on_line_boundary() {
        let text = "header\n".repeat(1000);
        let excerpt = truncate_on_line(&text, 100);
        assert!(excerpt.len() <= 100 + "… (truncated)".len());
        assert!(excerpt.ends_with("… (truncated)"));
        // 収まるならそのまま（末尾の空行だけ落とす）。
        assert_eq!(truncate_on_line("short\n", 100), "short");
    }

    #[test]
    fn percent_encoding_is_strict_rfc3986() {
        assert_eq!(percent_encode("a-z_A.Z~09"), "a-z_A.Z~09");
        assert_eq!(percent_encode("a b\nあ"), "a%20b%0A%E3%81%82");
        assert_eq!(percent_encode("`#?&="), "%60%23%3F%26%3D");
    }
}
