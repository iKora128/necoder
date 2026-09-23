//! `SSH_ASKPASS` として呼ばれたときのモード（ROADMAP「残: GUI askpass」）。
//!
//! GUI から起こす `ssh` には TTY が無いので、パスワード / 鍵の passphrase / host key 確認を
//! 訊く先が無い。OpenSSH は `SSH_ASKPASS` があればそのプログラムを起動し、**標準出力の 1 行**を
//! 答えとして受け取る公開仕様なので、necoder 自身をそこに指して（`host::install_askpass`）
//! 起動中の GUI へ中継する。
//!
//! この経路に入るのは `NECODER_ASKPASS_TOKEN` がある時だけ = ssh が起こした子プロセスだけ。
//! 秘密は stdout に 1 度書くだけで、ファイルにも設定にも残さない。

use std::io::{BufRead as _, BufReader, Write as _};
use std::time::Duration;

/// GUI の応答を待つ上限。GUI 側（`control_ipc::ASKPASS_TIMEOUT`）と同じ 3 分に、
/// 往復ぶんの余裕を少し足す。
const REPLY_TIMEOUT: Duration = Duration::from_secs(200);

/// askpass モードなら答えを stdout へ出して**そのまま終了する**（戻らない）。
/// そうでなければ `false` を返して通常起動へ進む。
pub(crate) fn run() -> bool {
    let Ok(token) = std::env::var("NECODER_ASKPASS_TOKEN") else {
        return false;
    };
    // OpenSSH は prompt を argv[1] で渡す（"user@host's password:" など）。
    let prompt = std::env::args().nth(1).unwrap_or_default();
    match ask(&token, &prompt) {
        Some(secret) => {
            let mut stdout = std::io::stdout();
            // 改行までが答え（OpenSSH は末尾の改行を落とす）。
            if writeln!(stdout, "{secret}").is_err() || stdout.flush().is_err() {
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        // 取り消し・GUI 不在・timeout。**空文字を返さない**ことが大事で、
        // 空を返すと ssh は「空パスワードで失敗」を繰り返す（= 元の不具合と同じ見え方）。
        None => std::process::exit(1),
    }
}

/// 起動中 GUI へ 1 往復（`fleet::gui_request` と同じ 1 行 JSON プロトコル・待ち時間だけ長い）。
fn ask(token: &str, prompt: &str) -> Option<String> {
    let socket_path = workspace::control_socket_path()?;
    let mut stream = workspace::ControlStream::connect(&socket_path).ok()?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok()?;
    let request = serde_json::json!({
        "method": "askpass",
        "params": { "token": token, "prompt": prompt },
    });
    writeln!(stream, "{request}").ok()?;
    stream.flush().ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let response: serde_json::Value = serde_json::from_str(&line).ok()?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    response
        .get("result")
        .and_then(|result| result.get("secret"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}
