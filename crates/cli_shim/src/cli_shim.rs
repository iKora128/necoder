//! cli_shim — ターミナル用 `ne` コマンド（`/usr/local/bin/ne`）の生成・設置・検出。
//!
//! VSCode の「Shell Command: Install 'code' command in PATH」相当（FEATURES §13 の
//! 「v1: CLI（`ne <path>` で開く）」）。実体は necoder バイナリへ委譲する短いシェル
//! スクリプトで、`ne <path>` は `necoder cli <path>` に落ちる（実行中 GUI へは IPC 転送・
//! 不在なら .app を切り離して起動。`crates/necoder/src/cli.rs` 参照）。
//!
//! GUI（設定画面）と CLI（`necoder install-cli`）の両方から使うため GPUI 非依存。
//! 設置先は `/usr/local/bin`（stock zsh の PATH に既定で載る場所。`~/.local/bin` は
//! プロファイル編集が要るため見送り）。書き込めない時の管理者昇格は macOS のみ
//! [`install_with_admin_prompt`]（osascript のパスワードダイアログ・Zed / VSCode と同型）。
//! Windows は W フェーズで対応（[`supported`] が false・設定画面はセクションごと隠す）。

use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

/// コマンド名（GLOSSARY: `ne`）。
pub const COMMAND_NAME: &str = "ne";

/// この OS でシム設置に対応しているか（Windows は W フェーズ・WINDOWS-PORT.md）。
pub fn supported() -> bool {
    cfg!(unix)
}

/// 設置先ディレクトリ。PATH に既定で載っている場所であることが要件。
pub fn shim_dir() -> PathBuf {
    PathBuf::from("/usr/local/bin")
}

/// 設置先のフルパス（`/usr/local/bin/ne`）。
pub fn shim_path() -> PathBuf {
    shim_dir().join(COMMAND_NAME)
}

/// シムの中身。`binary` = 委譲先の necoder 実体（.app 内 or dev ビルド）。
/// `NECODER_BIN=` 行は [`installed_target_at`] が読み戻す機械可読の印でもある。
pub fn shim_script(binary: &Path) -> String {
    format!(
        r#"#!/bin/sh
# ne — necoder をターミナルから開く CLI（necoder が生成。再設置は 設定 > コマンドライン）
NECODER_BIN="{binary}"
if [ ! -x "$NECODER_BIN" ]; then
    echo "necoder の実体が見つかりません: $NECODER_BIN" >&2
    echo "necoder の設定画面（コマンドライン）から再インストールしてください" >&2
    exit 1
fi
# 既存サブコマンドは素通し（`ne fleet status` / `ne config get theme` / `ne mcp`）
case "$1" in
    config|fleet|mcp) exec "$NECODER_BIN" "$@" ;;
esac
exec "$NECODER_BIN" cli "$@"
"#,
        binary = binary.display()
    )
}

/// 設置済みシムが委譲している necoder 実体（`NECODER_BIN="…"` 行を読む）。
/// necoder 製でない同名コマンドや読めないファイルは `None`。
pub fn installed_target_at(path: &Path) -> Option<PathBuf> {
    let script = std::fs::read_to_string(path).ok()?;
    let line = script
        .lines()
        .find_map(|line| line.strip_prefix("NECODER_BIN=\""))?;
    let target = line.strip_suffix('"')?;
    (!target.is_empty()).then(|| PathBuf::from(target))
}

/// 既定の設置先の状態。`None` = 未設置（or necoder 製でない）。
pub fn installed_target() -> Option<PathBuf> {
    installed_target_at(&shim_path())
}

/// このプロセスが「委譲先にすべき necoder 実体」。シンボリックリンク越しでも実体へ解決する。
pub fn current_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("自分の実行ファイルの場所が分かりません")?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// 直接書き込みで設置（0755）。他所製の同名コマンドは**上書きしない**（明確に断る）。
/// 権限が無ければ `PermissionDenied` の io エラーを返す — 呼び手が昇格（管理者ダイアログ /
/// sudo 案内）へ倒す。
pub fn install_at(path: &Path, binary: &Path) -> Result<()> {
    if path.exists() && installed_target_at(path).is_none() {
        bail!(
            "{} には necoder 製でない `{}` が既にあります。手で除いてから再実行してください",
            path.display(),
            COMMAND_NAME
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("{} を作れません", parent.display()))?;
    }
    std::fs::write(path, shim_script(binary))
        .with_context(|| format!("{} に書き込めません", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("{} に実行権限を付けられません", path.display()))?;
    }
    Ok(())
}

/// 既定の設置先へ設置。
pub fn install(binary: &Path) -> Result<()> {
    install_at(&shim_path(), binary)
}

/// 設置済みシムを削除。necoder 製でないファイルは消さない（存在しなければ何もしない）。
pub fn uninstall_at(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if installed_target_at(path).is_none() {
        bail!(
            "{} は necoder 製の `{}` ではないので消しません",
            path.display(),
            COMMAND_NAME
        );
    }
    std::fs::remove_file(path).with_context(|| format!("{} を消せません", path.display()))
}

/// 既定の設置先から削除。
pub fn uninstall() -> Result<()> {
    uninstall_at(&shim_path())
}

/// エラーが権限不足（昇格すれば通る見込み）かどうか。anyhow の鎖から io エラーを探す。
pub fn is_permission_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
}

/// 管理者昇格つき設置（macOS・GUI から）。まず直接書き、権限で弾かれたら osascript の
/// 管理者プロンプト（OS のパスワードダイアログ）で `/usr/local/bin` へ `install` する。
/// `prompt` はダイアログに出す説明文（i18n は呼び手が持つ — この crate は GPUI/i18n 非依存）。
#[cfg(target_os = "macos")]
pub fn install_with_admin_prompt(binary: &Path, prompt: &str) -> Result<()> {
    let error = match install(binary) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if !is_permission_error(&error) {
        return Err(error);
    }
    // 昇格側の引用を最小にする: 中身は無権限で temp に書き、admin では install(1) だけ動かす。
    let staging = std::env::temp_dir().join(format!("necoder-ne-shim-{}", std::process::id()));
    std::fs::write(&staging, shim_script(binary))
        .with_context(|| format!("{} に書けません", staging.display()))?;
    let script = format!(
        "mkdir -p {dir} && /usr/bin/install -m 0755 {src} {dest}",
        dir = shell_quote(&shim_dir().display().to_string()),
        src = shell_quote(&staging.display().to_string()),
        dest = shell_quote(&shim_path().display().to_string()),
    );
    let result = run_admin_shell(&script, prompt);
    let _ = std::fs::remove_file(&staging);
    result
}

/// 管理者昇格つき削除（macOS・GUI から）。
#[cfg(target_os = "macos")]
pub fn uninstall_with_admin_prompt(prompt: &str) -> Result<()> {
    let error = match uninstall() {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if !is_permission_error(&error) {
        return Err(error);
    }
    // ここに来るのは necoder 製と確認済みのシムだけ（uninstall が先に断っている）。
    let script = format!("rm {}", shell_quote(&shim_path().display().to_string()));
    run_admin_shell(&script, prompt)
}

/// macOS 以外の unix: 昇格ダイアログの仕組みが無いので直接設置のみ（失敗はそのまま返す）。
#[cfg(all(unix, not(target_os = "macos")))]
pub fn install_with_admin_prompt(binary: &Path, _prompt: &str) -> Result<()> {
    install(binary)
}

/// macOS 以外の unix: 直接削除のみ。
#[cfg(all(unix, not(target_os = "macos")))]
pub fn uninstall_with_admin_prompt(_prompt: &str) -> Result<()> {
    uninstall()
}

/// Windows は W フェーズ（[`supported`] = false・設定画面はセクションごと出さない）。
/// 呼ばれても壊れないよう明確に断るだけのスタブ。
#[cfg(windows)]
pub fn install_with_admin_prompt(_binary: &Path, _prompt: &str) -> Result<()> {
    bail!("Windows の CLI 設置は未対応です（WINDOWS-PORT.md・W フェーズ）")
}

/// Windows スタブ（[`install_with_admin_prompt`] と同じ扱い）。
#[cfg(windows)]
pub fn uninstall_with_admin_prompt(_prompt: &str) -> Result<()> {
    bail!("Windows の CLI 設置は未対応です（WINDOWS-PORT.md・W フェーズ）")
}

/// osascript `do shell script … with administrator privileges` を 1 回実行する。
/// キャンセル（-128）や認証失敗は Err（stderr の生文をそのまま伝える）。
#[cfg(target_os = "macos")]
fn run_admin_shell(shell_script: &str, prompt: &str) -> Result<()> {
    let applescript = format!(
        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
        applescript_quote(shell_script),
        applescript_quote(prompt),
    );
    let output = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&applescript)
        .output()
        .context("osascript を起動できません")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("管理者権限での設置に失敗: {}", stderr.trim())
}

/// AppleScript 文字列リテラル用のエスケープ（`\` と `"`）。
#[cfg(target_os = "macos")]
fn applescript_quote(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

/// シェルのシングルクォート（`'` は `'\''` に割る）。パスを安全に埋め込む。
#[cfg(target_os = "macos")]
fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_shim(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("necoder_cli_shim_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(COMMAND_NAME)
    }

    #[test]
    fn install_then_read_back_roundtrip() {
        let path = temp_shim("roundtrip");
        let binary = PathBuf::from("/Applications/necoder.app/Contents/MacOS/necoder");
        install_at(&path, &binary).expect("設置できるはず");
        assert_eq!(installed_target_at(&path), Some(binary.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "実行権限が付いているはず");
        }
        // 別の実体へ再設置（向け直し）も通る
        let dev_binary = PathBuf::from("/tmp/target/debug/necoder");
        install_at(&path, &dev_binary).expect("向け直しできるはず");
        assert_eq!(installed_target_at(&path), Some(dev_binary));
        uninstall_at(&path).expect("削除できるはず");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn refuses_foreign_command() {
        let path = temp_shim("foreign");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, "#!/bin/sh\necho other tool\n").expect("write");
        assert_eq!(installed_target_at(&path), None, "他所製は検出しない");
        assert!(
            install_at(&path, Path::new("/tmp/necoder")).is_err(),
            "上書きしない"
        );
        assert!(uninstall_at(&path).is_err(), "消さない");
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn shim_delegates_subcommands_and_cli() {
        let script = shim_script(Path::new("/usr/local/necoder"));
        assert!(script.starts_with("#!/bin/sh"));
        assert!(script.contains(r#"config|fleet|mcp) exec "$NECODER_BIN" "$@" ;;"#));
        assert!(script.contains(r#"exec "$NECODER_BIN" cli "$@""#));
    }
}
