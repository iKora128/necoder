//! Windows のコンソールの扱い（GUI サブシステムの necoder.exe・WINDOWS-PORT.md「罠」の表）。
//!
//! necoder.exe は `windows_subsystem = "windows"` で作る。付けないと Windows はコンソールアプリとして
//! 起動し、エクスプローラやスタートメニューから開いても**黒いコンソール窓がずっと付いてくる**
//! （2026-10-06 Windows 11 のユーザー報告）。
//!
//! 引き換えに、端末（cmd / PowerShell / Windows Terminal）から `necoder config …` のような CLI として
//! 叩いた時、何も表示されなくなる — GUI アプリには標準出力が渡されないため。そこで起動直後に
//! [`attach_parent_console`] で**親のコンソールへ繋ぎ直す**。
//!
//! - エクスプローラから開いた: 親にコンソールが無い → 何もしない（窓は出ない）
//! - 端末から開いた: 標準出力・標準エラーを端末へ向ける（GUI を開いた時のログも端末に出る）
//! - エージェントが `necoder mcp` をパイプで起こした・ssh が askpass として起こした: 標準入出力は
//!   パイプとして既に渡っている → **触らない**（パイプを端末で上書きすると MCP が壊れる）
//!
//! GUI サブシステムのプロセスを cmd.exe は待たないので、端末ではプロンプトが先に戻り、出力はその後に
//! 続く。CLI の結果が読めることを優先した割り切り。

/// 親のコンソールに繋ぎ、渡されていない標準出力・標準エラーをそこへ向ける（モジュールの doc 参照）。
/// 標準入力は繋がない — cmd.exe は待たずにプロンプトへ戻るので、入力の取り合いになる。
#[cfg(windows)]
pub fn attach_parent_console() {
    use std::ffi::c_void;
    use std::os::windows::io::IntoRawHandle as _;
    use windows_sys::Win32::System::Console::{AttachConsole, GetStdHandle, SetStdHandle};

    // 定数は windows-sys の版で置き場が動くのでここで定義する（control_transport と同じ方針）。
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;

    let missing = |handle: *mut c_void| handle.is_null() || handle == INVALID_HANDLE_VALUE;
    // SAFETY: どれも値だけを渡す Win32 呼び出し。GetStdHandle の戻り値は比べるだけで使わない。
    let (output_missing, error_missing) = unsafe {
        (
            missing(GetStdHandle(STD_OUTPUT_HANDLE)),
            missing(GetStdHandle(STD_ERROR_HANDLE)),
        )
    };
    if !output_missing && !error_missing {
        return; // パイプやファイルで渡されている（MCP・askpass・リダイレクト）
    }
    // SAFETY: 値だけを渡す Win32 呼び出し。
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return; // 親にコンソールが無い（エクスプローラ等から開いた）
    }
    // 繋いだコンソールの画面バッファ。プロセスが終わるまで標準出力として使うので、閉じずに手放す。
    let Ok(console) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
    else {
        return;
    };
    let console = console.into_raw_handle();
    // Rust の標準出力は書くたびに GetStdHandle を引くので、差し替えはそのまま効く。
    // SAFETY: `console` は今開いた有効なハンドルで、以後このプロセスが標準出力として持ち続ける。
    unsafe {
        if output_missing {
            SetStdHandle(STD_OUTPUT_HANDLE, console);
        }
        if error_missing {
            SetStdHandle(STD_ERROR_HANDLE, console);
        }
    }
}

/// mac / Linux には GUI サブシステムの区別が無い（何もしない）。
#[cfg(not(windows))]
pub fn attach_parent_console() {}
