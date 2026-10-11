//! child_process — 子プロセスの `Command` を作る**唯一の入口**。
//!
//! ## なぜこの crate が要るか（Windows）
//!
//! necoder は Windows で GUI サブシステムの実行ファイルとして動く（`necoder/src/main.rs` の
//! `windows_subsystem`）。GUI アプリ自身はコンソールを持たないので、そこから `git.exe` や
//! `node.exe` のような**コンソールアプリ**を素の `Command::new` で起こすと、Windows は子のために
//! **新しいコンソール窓を毎回開く**。git の状態取得は数秒おきに走るので、黒い窓が点滅し続ける。
//!
//! [`command`] は Windows でだけ `CREATE_NO_WINDOW` を付ける。子は窓の無いコンソールを持ち、
//! その子（エージェントが起こす git など）も同じ見えないコンソールを継ぐので、孫でも窓は出ない。
//! GUI アプリ（エクスプローラ・ブラウザ・インストーラ）を起こす場合、このフラグは無視される。
//!
//! 統合ターミナルは ConPTY（portable-pty）経由で起こすので、この crate を通らない。
//!
//! ## mac / Linux
//!
//! `std::process::Command::new` と同じ物を返す（**挙動を 1 つも変えない**・WINDOWS-PORT.md §D8）。

use std::ffi::OsStr;
use std::process::Command;

/// Win32 の `CREATE_NO_WINDOW`（`winbase.h`）。windows-sys の置き場は版で動くのでここで定義する。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 子プロセスの `Command` を作る。Windows ではコンソール窓を出さない（crate の doc 参照）。
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    hide_console_window(&mut command);
    command
}

/// 既に作った `Command` に、Windows でコンソール窓を出さない指定を足す。
/// 外部ライブラリから受け取った `Command` など、[`command`] を通せない物に使う。
pub fn hide_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_keeps_the_program_and_arguments() {
        let mut built = command("git");
        built.args(["status", "--porcelain"]);
        assert_eq!(built.get_program(), "git");
        let args: Vec<_> = built.get_args().collect();
        assert_eq!(args, ["status", "--porcelain"]);
    }

    #[cfg(unix)]
    #[test]
    fn command_runs_like_std_command_on_unix() {
        let output = command("sh")
            .args(["-c", "printf necoder"])
            .output()
            .expect("sh が起動できる");
        assert_eq!(output.stdout, b"necoder");
    }
}
