//! logging — Finder/Dock 起動時に stdout/stderr をログファイルへ付け替える（M13 公開準備）。
//!
//! 本アプリの実行時ログは eprintln!/println! ベース。ターミナル起動なら人間に見えるが、
//! Finder / Dock から起動した .app は stderr が /dev/null に落ち、パニックしない不具合
//! （SSH 接続失敗・ACP ハンドシェイク失敗など）でユーザーが添付できるものが何も無い。
//! そこで **出力の行き先が /dev/null のときだけ** dup2 でログファイルへ差し替える。
//! tty（ターミナル起動）と pipe（NECODER_* プローブの捕捉・MCP の stdio）は素通し
//! ＝開発フロー・プロトコルチャネルの挙動を一切変えない。将来 tracing 等へ載せ替える
//! 場合もこの層の内側で済む（呼び出しは main() 冒頭の 1 箇所）。
//!
//! 置き場: `~/Library/Application Support/necoder/logs/necoder-<unix秒>-<pid>.log`
//! （`NECODER_LOG_DIR` で差し替え・crash.rs の crashes/ と同じ並び・新しい 20 本だけ保持）。

use std::path::PathBuf;

/// 保持するログ本数（crash.rs の KEEP_LOGS と同じ方針）。
#[cfg(unix)]
const KEEP_LOGS: usize = 20;

/// ログの置き場。テスト・offscreen 検証は `NECODER_LOG_DIR` で差し替える。
#[cfg(unix)]
pub fn log_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NECODER_LOG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(std::path::Path::new(&home).join("Library/Application Support/necoder/logs"))
}

/// Finder/Dock 起動（stderr の行き先が /dev/null）のときだけ stdout/stderr をログへ
/// 付け替える。main() の最初（panic hook より前）に呼ぶ＝クラッシュのバックトレースも
/// 同じログに残る。付け替えたときはログパスを返す。
#[cfg(unix)]
pub fn redirect_output_for_gui_launch() -> Option<PathBuf> {
    if std::env::var_os("NECODER_NO_LOG_REDIRECT").is_some() {
        return None;
    }
    // stderr を誰か（tty / pipe / ファイル）が見ているなら何もしない。
    if !fd_points_to_dev_null(libc::STDERR_FILENO) {
        return None;
    }
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("necoder-{unix_seconds}-{}.log", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let raw_fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    // stdout は MCP のようにプロトコルチャネルの可能性があるため、/dev/null のときだけ。
    unsafe {
        libc::dup2(raw_fd, libc::STDERR_FILENO);
        if fd_points_to_dev_null(libc::STDOUT_FILENO) {
            libc::dup2(raw_fd, libc::STDOUT_FILENO);
        }
    }
    drop(file); // dup2 済み＝元の fd は閉じてよい
    eprintln!(
        "necoder v{} ({} {}) — GUI 起動ログ開始",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    prune_old_logs(&dir);
    Some(path)
}

/// fd の行き先が /dev/null か（= 出力が捨てられているか）。tty は isatty で除外する。
#[cfg(unix)]
fn fd_points_to_dev_null(fd: libc::c_int) -> bool {
    unsafe {
        if libc::isatty(fd) == 1 {
            return false;
        }
        let mut fd_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::fstat(fd, fd_stat.as_mut_ptr()) != 0 {
            return false;
        }
        let fd_stat = fd_stat.assume_init();
        // pipe / 通常ファイルは読み手がいる＝触らない。/dev/null は文字デバイス。
        if (fd_stat.st_mode & libc::S_IFMT) != libc::S_IFCHR {
            return false;
        }
        let mut null_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::stat(c"/dev/null".as_ptr(), null_stat.as_mut_ptr()) != 0 {
            return false;
        }
        fd_stat.st_rdev == null_stat.assume_init().st_rdev
    }
}

/// 新しい KEEP_LOGS 本だけ残す。`necoder-<unix秒>-<pid>.log` は unix 秒が 10 桁で
/// 辞書順 = 時刻順になる（crash.rs の prune と同じ方針・対象外の名前は触らない）。
#[cfg(unix)]
fn prune_old_logs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("necoder-") && name.ends_with(".log"))
        })
        .collect();
    if logs.len() <= KEEP_LOGS {
        return;
    }
    logs.sort();
    let excess = logs.len() - KEEP_LOGS;
    for path in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// Windows は stdio 事情が別物（そもそも GUI subsystem で stderr が無い）。
/// 移植のときに専用実装を入れるまで no-op。
#[cfg(not(unix))]
pub fn redirect_output_for_gui_launch() -> Option<PathBuf> {
    None
}

#[cfg(not(unix))]
pub fn log_dir() -> Option<PathBuf> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_newest_logs_and_ignores_other_files() {
        let dir = std::env::temp_dir().join(format!("necoder-log-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..(KEEP_LOGS + 5) {
            let path = dir.join(format!("necoder-{:010}-1.log", 1_700_000_000 + index));
            std::fs::write(&path, "x").unwrap();
        }
        let other = dir.join("notes.txt");
        std::fs::write(&other, "x").unwrap();
        prune_old_logs(&dir);
        let remaining = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".log"))
            .count();
        assert_eq!(remaining, KEEP_LOGS);
        assert!(other.exists(), "対象外ファイルは消さない");
        assert!(
            !dir.join(format!("necoder-{:010}-1.log", 1_700_000_000))
                .exists(),
            "最古が消える"
        );
        assert!(
            dir.join(format!(
                "necoder-{:010}-1.log",
                1_700_000_000 + KEEP_LOGS + 4
            ))
            .exists(),
            "最新は残る"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dev_null_detection_distinguishes_captured_output() {
        // テスト実行時の stderr は tty か pipe（cargo の捕捉）＝「見ている人がいる」。
        assert!(!fd_points_to_dev_null(libc::STDERR_FILENO));
        // 実際に /dev/null を開いた fd は検知できる。
        let null = std::fs::File::open("/dev/null").unwrap();
        assert!(fd_points_to_dev_null(std::os::fd::AsRawFd::as_raw_fd(
            &null
        )));
    }
}
