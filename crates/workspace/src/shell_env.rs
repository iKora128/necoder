//! shell_env — Finder/Dock 起動の .app に「ログインシェルの PATH」を持ち込む。
//!
//! GUI 起動（Finder / Dock / `open -a`）のプロセスは launchd から環境を継ぐので、PATH が
//! `/usr/bin:/bin:/usr/sbin:/sbin` の 4 つしか無い。`.zprofile`/`.zshrc` が足す nvm の node も
//! `~/.local/bin` の CLI も `~/.cargo/bin` の rust-analyzer も**見えない**。これは子プロセスを
//! 立てる機能すべてに効く:
//!   - ACP エージェント — `claude-agent-acp` は `#!/usr/bin/env node` ＝ **node が PATH に要る**。
//!     無いと `env: node: No such file or directory` で即死し、しかも「バイナリは在る」ので
//!     解決段階では成功して見える（＝ハンドシェイク待ちで固まる／不可解なエラーになる）
//!   - LSP サーバ（rust-analyzer 等）の解決（`lang::lsp`）
//!   - スレッドタイトルの一発生成（`claude -p` / `codex exec`）
//!
//! ターミナルから起動したときは既に本物の環境があるので**何もしない**（`logging` と同じ流儀＝
//! 開発フローの挙動を一切変えない）。
//!
//! 取得は `$SHELL -l -i -c` を 1 回。nvm のように **interactive でしか読まれない**初期化が
//! あるため `-i` が要る（`-l` だけでは node が出ない実例）。~0.5s かかるので起動時間の予算
//! （~215ms）を毎回食わないよう結果をキャッシュし、2 回目以降は即読み + 裏で取り直す。
//!
//! 置き場: `~/Library/Application Support/Shirushi/shell-path`（`SHIRUSHI_SHELL_PATH_CACHE`
//! で差し替え・`SHIRUSHI_NO_SHELL_ENV=1` で機能ごと止める）。
//!
//! 扱うのは PATH だけ。API キー等は取り込まない — 認証は各 CLI 側に委譲する決定（DECISIONS）
//! なので、鍵をこのプロセスへ持ち込む理由が無い。

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// launchd が GUI プロセスへ渡す既定の PATH。これと一致する＝シェルを経由していない起動。
#[cfg(unix)]
const LAUNCHD_DEFAULT_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// 同期取得を諦めるまでの時間。rc ファイルが入力待ちで固まっても起動を道連れにしない。
#[cfg(unix)]
const LOAD_TIMEOUT: Duration = Duration::from_secs(5);

/// 出力から PATH を切り出す区切り。rc ファイルが何か表示しても（`-i` なので有り得る）混ざらない。
#[cfg(unix)]
const MARKER: char = '\u{1}';

/// PATH キャッシュの置き場（`SHIRUSHI_SHELL_PATH_CACHE` で差し替え）。
#[cfg(unix)]
pub fn cache_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SHIRUSHI_SHELL_PATH_CACHE") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")?;
    Some(std::path::Path::new(&home).join("Library/Application Support/Shirushi/shell-path"))
}

/// GUI 起動なら、ログインシェルの PATH をこのプロセスへ取り込む。取り込んだ PATH を返す。
///
/// main() の冒頭（GPUI 起動前＝まだ実質シングルスレッド）で 1 回だけ呼ぶこと。`set_var` は
/// 他スレッドの env 読みと競合し得るので、**窓が開く前**である必要がある。
#[cfg(unix)]
pub fn inherit_login_shell_path() -> Option<String> {
    if std::env::var_os("SHIRUSHI_NO_SHELL_ENV").is_some() {
        return None;
    }
    // ターミナル起動（＝既に本物の環境を継いでいる）なら触らない。
    if std::env::var("PATH").is_ok_and(|path| path != LAUNCHD_DEFAULT_PATH) {
        return None;
    }
    match read_cache() {
        // 2 回目以降: キャッシュを即反映して起動を待たせず、次回のために裏で取り直す。
        // 反映するのはこのスレッドだけ（背景スレッドは書き出すだけ）＝ set_var の競合を作らない。
        Some(path) => {
            std::env::set_var("PATH", &path);
            std::thread::spawn(|| {
                if let Some(fresh) = load_from_login_shell() {
                    write_cache(&fresh);
                }
            });
            Some(path)
        }
        // 初回（インストール直後）だけ同期で取る。ここで取らないと最初の ACP 起動が落ちる。
        None => {
            let path = load_from_login_shell()?;
            std::env::set_var("PATH", &path);
            write_cache(&path);
            Some(path)
        }
    }
}

/// ログインシェルを 1 回走らせて PATH を得る。固まったら [`LOAD_TIMEOUT`] で諦める
/// （子は放置＝いずれ終わる。起動を止めないことを優先する）。
#[cfg(unix)]
fn load_from_login_shell() -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(run_login_shell()).ok();
    });
    rx.recv_timeout(LOAD_TIMEOUT).ok().flatten()
}

#[cfg(unix)]
fn run_login_shell() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = Command::new(&shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!("printf '{MARKER}%s{MARKER}' \"$PATH\""),
        ])
        // 子には launchd 既定の PATH を渡す。rc は既存 PATH に**前置**するのが通例なので、
        // 取り込み済みの PATH を渡すと起動のたびに同じ dir が積み増される。
        .env("PATH", LAUNCHD_DEFAULT_PATH)
        .stdin(Stdio::null()) // rc が入力を待っても即 EOF
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // rc の警告類は捨てる（PATH だけが欲しい）
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let path = text.split(MARKER).nth(1)?.trim();
    // 空 or 明らかに壊れた結果でユーザーの PATH を潰さない。
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(unix)]
fn read_cache() -> Option<String> {
    let path = std::fs::read_to_string(cache_path()?).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(unix)]
fn write_cache(path: &str) {
    let Some(cache) = cache_path() else {
        return;
    };
    if let Some(parent) = cache.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Err(error) = std::fs::write(&cache, path) {
        eprintln!("shell PATH キャッシュを書けない（次回も同期取得になる）: {error}");
    }
}

/// Windows は環境の渡り方が別物（launchd 相当が無い）。移植のときに専用実装を入れるまで no-op。
#[cfg(not(unix))]
pub fn inherit_login_shell_path() -> Option<String> {
    None
}

#[cfg(not(unix))]
pub fn cache_path() -> Option<PathBuf> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// ターミナル起動（PATH が launchd 既定と違う）では何もしない＝開発フローを変えない。
    #[test]
    fn shell_launch_is_left_alone() {
        let original = std::env::var_os("PATH");
        std::env::set_var("PATH", "/opt/homebrew/bin:/usr/bin:/bin");
        assert_eq!(inherit_login_shell_path(), None);
        assert_eq!(
            std::env::var("PATH").ok().as_deref(),
            Some("/opt/homebrew/bin:/usr/bin:/bin"),
            "ターミナル起動の PATH は書き換えない"
        );
        match original {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }

    /// GUI 起動（PATH が launchd 既定）ならキャッシュを即反映する。
    /// キャッシュ有りの経路＝シェルを起こさないので速い（起動時間の予算を食わない）。
    #[test]
    fn gui_launch_applies_cached_path() {
        let scratch =
            std::env::temp_dir().join(format!("shirushi-shell-env-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("scratch を作れる");
        let cache = scratch.join("shell-path");
        std::fs::write(&cache, "/opt/test/bin:/usr/bin:/bin\n").expect("キャッシュを書ける");

        let original = std::env::var_os("PATH");
        std::env::set_var("SHIRUSHI_SHELL_PATH_CACHE", &cache);
        std::env::set_var("PATH", LAUNCHD_DEFAULT_PATH);

        let applied = inherit_login_shell_path();
        assert_eq!(applied.as_deref(), Some("/opt/test/bin:/usr/bin:/bin"));
        assert_eq!(
            std::env::var("PATH").ok().as_deref(),
            Some("/opt/test/bin:/usr/bin:/bin"),
            "GUI 起動ではキャッシュした PATH がプロセスに載る"
        );

        std::env::remove_var("SHIRUSHI_SHELL_PATH_CACHE");
        match original {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        std::fs::remove_dir_all(&scratch).ok();
    }

    /// 実プロセス検証: ログインシェルから PATH が取れて、launchd 既定より広いこと。
    /// `cargo test -p workspace -- --ignored --nocapture live_login_shell_path`
    #[test]
    #[ignore = "ログインシェル（実プロセス・~0.5s）が要る"]
    fn live_login_shell_path() {
        let path = run_login_shell().expect("ログインシェルから PATH が取れる");
        println!("ログインシェルの PATH: {path}");
        assert!(
            path.split(':').count() > LAUNCHD_DEFAULT_PATH.split(':').count(),
            "launchd 既定より広い PATH が取れる: {path}"
        );
    }
}
