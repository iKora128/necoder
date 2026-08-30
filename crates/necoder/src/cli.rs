//! `necoder cli` / `install-cli` / `uninstall-cli` — ターミナル用 `ne` コマンドの実体と設置。
//!
//! `ne <path>` シム（cli_shim crate が生成）は `necoder cli <path>` に落ちる。ここでの分岐:
//! - **実行中の GUI がいる**（`gui.sock` に接続できる）→ IPC の `open` でそのウィンドウに
//!   開かせて前面化する（Turso は排他ロック＝二重インスタンスは hot exit / 台帳が壊れるので
//!   起動しない。control_ipc.rs の単一 writer 原則と同じ理由）
//! - **socket 不通** → .app なら `-n` を付けない「書類を開く」で LaunchServices へ渡す
//!   （GUI が実は生きていれば openFiles で届き Finder と同じ `open_external_paths` 経路＝
//!   そのウィンドウのレールに開く・不在なら起動して同じ経路で届く）。「socket が死んでいるが
//!   GUI は生きている」瞬間に二重インスタンスを作らないための要。dev ビルドは従来どおり
//!   自分自身を**切り離して**起動する（`ne` はすぐ戻る）
//!
//! パスの絶対化はここ（cwd を知っている側）でやる。GUI 側は絶対パスしか受けない
//! （Finder 由来の `open_external_paths` と同じ契約）。

use anyhow::{Context as _, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

/// `necoder <cli|install-cli|uninstall-cli> …` を処理したら true（GUI は開かない）。
/// 失敗は exit code 1（シェル/スクリプトから `&&` で繋げるように）。
pub(crate) fn run() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("cli") => run_open(&args[1..]),
        Some("install-cli") => run_install(),
        Some("uninstall-cli") => run_uninstall(),
        _ => return false,
    };
    if let Err(error) = result {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
    true
}

/// `necoder cli [<path>|ssh://…]...` — `ne` の本体。
fn run_open(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            println!(
                "使い方: {} [<path>|ssh://user@host/path]...",
                cli_shim::COMMAND_NAME
            );
            println!("  引数なし: 実行中の necoder を前面に出す（いなければ前回状態で起動）");
            println!(
                "  そのほか: {} <config|fleet|mcp> … も素通しで使えます",
                cli_shim::COMMAND_NAME
            );
            return Ok(());
        }
        Some("-V") | Some("--version") => {
            println!("necoder {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }
    let cwd = std::env::current_dir().context("カレントディレクトリが分かりません")?;
    let (paths, has_remote) = resolve_open_args(args, &cwd);
    // ssh:// は接続処理が新プロセス側にしかないので、IPC 転送せず常に新しいインスタンスで開く。
    if has_remote {
        return launch_detached(&paths);
    }
    if let Some(result) = try_open_in_running_gui(&paths)? {
        let opened = result
            .get("opened")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if let Some(skipped) = result.get("skipped").and_then(serde_json::Value::as_array) {
            for path in skipped.iter().filter_map(serde_json::Value::as_str) {
                eprintln!("見つからない（スキップ）: {path}");
            }
        }
        if opened > 0 {
            println!("実行中の necoder で開きました（{opened} 件）");
        } else {
            println!("実行中の necoder を前面に出しました");
        }
        return Ok(());
    }
    open_via_launch_services(&paths)
}

/// IPC 不通時の非 remote フォールバック。.app 内なら **`-n` を付けず「書類を開く」**で
/// LaunchServices に渡す — 実行中インスタンスがいれば openFiles でそのウィンドウに届き
/// （Finder「このアプリケーションで開く」と同じ `open_external_paths` 経路＝レールに追加）、
/// いなければ起動して同じ経路で届く。socket は前任プロセス在命中に起動した GUI が bind を
/// 持てないことがあり（control_ipc.rs 参照）、その状態で `-n` を使うと二重インスタンス
/// （Turso 排他ロック破り）になるため、書類経由に一本化する。dev ビルドは従来どおり spawn。
fn open_via_launch_services(paths: &[String]) -> Result<()> {
    let exe = cli_shim::current_binary()?;
    let Some(bundle) = bundle_root(&exe) else {
        return launch_detached(paths);
    };
    // 存在しないパスが混ざると open(1) は全体を失敗させる → ここで警告してスキップ
    // （IPC 応答の skipped と同じ見せ方）。
    let (existing, skipped): (Vec<&String>, Vec<&String>) = paths
        .iter()
        .partition(|path| Path::new(path.as_str()).exists());
    for path in &skipped {
        eprintln!("見つからない（スキップ）: {path}");
    }
    let mut command = std::process::Command::new("/usr/bin/open");
    command.arg("-a").arg(&bundle);
    for path in &existing {
        command.arg(path.as_str());
    }
    let status = command
        .status()
        .with_context(|| format!("{} を起動できません", bundle.display()))?;
    anyhow::ensure!(status.success(), "open が失敗しました（{status}）");
    if existing.is_empty() {
        println!("necoder を前面に出しました");
    } else {
        println!("necoder で開きました（{} 件）", existing.len());
    }
    Ok(())
}

/// GUI 起動の直前に呼ぶ**単一インスタンスの防波堤**（main.rs）。生きた GUI が socket に
/// 居れば argv のパスを IPC `open` で渡して true ＝呼び手は即 return（この起動は窓を作らず
/// Dock にアイコンが増えない）。`open -n` 直叩き・旧シム経由など「二重起動になる残りの入口」を
/// すべてここで畳む（Turso 排他ロックの保険）。対象外:
/// - dev ビルド（.app 外）: 製品版と並行して dev 版を起動する開発フローを塞がない
/// - ssh:// を含む起動: 接続処理が新プロセス側にしかない（cli.rs の分岐と同じ理由）
pub(crate) fn forward_launch_to_running_gui() -> bool {
    let Ok(exe) = cli_shim::current_binary() else {
        return false;
    };
    if bundle_root(&exe).is_none() {
        return false;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let (paths, has_remote) = resolve_open_args(&args, &cwd);
    if has_remote {
        return false;
    }
    match try_open_in_running_gui(&paths) {
        Ok(Some(_)) => true,
        Ok(None) => false, // GUI 不在 → 通常起動
        // 接続できたのに転送失敗 = GUI は生きている。二重起動（DB ロック破り）よりは
        // この起動を畳む方へ倒す（cli.rs の run_open と同じ原則）。
        Err(error) => {
            eprintln!("実行中の necoder へ渡せませんでした（起動を中止）: {error:#}");
            true
        }
    }
}

/// 実行中 GUI がいれば IPC `open` を送り、その応答を返す。いなければ `None`（→ 新規起動へ）。
/// **接続できたのに open が失敗**したら Err（そのまま二重起動すると DB ロックで壊れるため）。
fn try_open_in_running_gui(paths: &[String]) -> Result<Option<serde_json::Value>> {
    let Some(socket_path) = workspace::control_socket_path() else {
        return Ok(None);
    };
    if workspace::ControlStream::connect(&socket_path).is_err() {
        return Ok(None); // GUI 不在（死んだ socket ファイル含む）
    }
    let result = crate::fleet::gui_request("open", json!({ "paths": paths }))
        .context("実行中の necoder へ渡せませんでした")?;
    Ok(Some(result))
}

/// 引数をパス（絶対化済み）へ解決する。ssh:// URI はそのまま通し、`has_remote` を立てる。
/// 存在しないパスも**落とさず**通す（GUI 側 / 新プロセスの resolve_projects が警告してスキップ
/// — 判定を一箇所に保つ）。
fn resolve_open_args(args: &[String], cwd: &Path) -> (Vec<String>, bool) {
    let mut has_remote = false;
    let paths = args
        .iter()
        .map(|arg| {
            if arg.starts_with("ssh://") {
                has_remote = true;
                return arg.clone();
            }
            let path = Path::new(arg);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            // `..`/シンボリックリンクを畳む（存在しないパスは素の絶対形のまま）
            std::fs::canonicalize(&absolute)
                .unwrap_or(absolute)
                .display()
                .to_string()
        })
        .collect();
    (paths, has_remote)
}

/// 新インスタンス起動（ssh:// 用・dev ビルドの汎用フォールバック）。.app 内なら
/// LaunchServices（`open -n`）で、dev ビルドなら自分自身を切り離して spawn する
/// （どちらも `ne` はすぐ戻る・端末を閉じても巻き込まれない）。
fn launch_detached(paths: &[String]) -> Result<()> {
    let exe = cli_shim::current_binary()?;
    if let Some(bundle) = bundle_root(&exe) {
        let status = std::process::Command::new("/usr/bin/open")
            .arg("-n") // ssh:// は新プロセスでしか処理できない＝明示的に新インスタンス（--args は既存活性化だと黙って落ちるため）
            .arg(&bundle)
            .arg("--args")
            .args(paths)
            .status()
            .with_context(|| format!("{} を起動できません", bundle.display()))?;
        anyhow::ensure!(status.success(), "open が失敗しました（{status}）");
    } else {
        let mut command = std::process::Command::new(&exe);
        command
            .args(paths)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // 端末の Ctrl-C（フォアグラウンドグループへのシグナル）に巻き込まれないよう別グループへ。
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        command
            .spawn()
            .with_context(|| format!("{} を起動できません", exe.display()))?;
    }
    println!("necoder を起動しました");
    Ok(())
}

/// 実行ファイルが .app バンドル内（`…/necoder.app/Contents/MacOS/<bin>`）ならバンドルの根を返す。
fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.ancestors().nth(3)?;
    (bundle
        .extension()
        .is_some_and(|extension| extension == "app"))
    .then(|| bundle.to_path_buf())
}

/// `necoder install-cli` — `ne` シムを /usr/local/bin へ設置（権限が無ければ sudo を案内）。
fn run_install() -> Result<()> {
    anyhow::ensure!(
        cli_shim::supported(),
        "この OS の CLI 設置は未対応です（WINDOWS-PORT.md・W フェーズ）"
    );
    let binary = cli_shim::current_binary()?;
    match cli_shim::install(&binary) {
        Ok(()) => {
            println!(
                "インストール完了: {} → {}",
                cli_shim::shim_path().display(),
                binary.display()
            );
            Ok(())
        }
        Err(error) if cli_shim::is_permission_error(&error) => {
            anyhow::bail!(
                "{} に書き込めません。次のどちらかで:\n  sudo \"{}\" install-cli\n  または necoder の 設定 > コマンドライン からインストール（パスワードダイアログ）",
                cli_shim::shim_dir().display(),
                binary.display()
            )
        }
        Err(error) => Err(error),
    }
}

/// `necoder uninstall-cli` — `ne` シムを削除（necoder 製でなければ消さない）。
fn run_uninstall() -> Result<()> {
    anyhow::ensure!(
        cli_shim::supported(),
        "この OS の CLI 設置は未対応です（WINDOWS-PORT.md・W フェーズ）"
    );
    let shim = cli_shim::shim_path();
    if cli_shim::installed_target().is_none() && !shim.exists() {
        println!("{} は未設置です", shim.display());
        return Ok(());
    }
    match cli_shim::uninstall() {
        Ok(()) => {
            println!("削除しました: {}", shim.display());
            Ok(())
        }
        Err(error) if cli_shim::is_permission_error(&error) => {
            let binary = cli_shim::current_binary()?;
            anyhow::bail!(
                "{} を消せません。次のどちらかで:\n  sudo \"{}\" uninstall-cli\n  または necoder の 設定 > コマンドライン から削除（パスワードダイアログ）",
                shim.display(),
                binary.display()
            )
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unix 形式の絶対パス（`/work/…`）を前提に書いている（Windows では `/…` は絶対パスに
    // ならず区切りも `\` になる）。`ne` の Windows 対応は W フェーズ（supported()=false）。
    #[cfg(unix)]
    #[test]
    fn resolve_absolutizes_against_cwd() {
        let cwd = Path::new("/work/repo");
        let (paths, has_remote) = resolve_open_args(
            &[
                ".".into(),
                "src".into(),
                "/no-such-necoder-root".into(),
                "ssh://dev@host/app".into(),
            ],
            cwd,
        );
        assert!(has_remote);
        // すべて存在しないパス＝canonicalize されず素の絶対形のまま（存在すれば実体へ解決される）
        assert_eq!(paths[0], "/work/repo/.");
        assert_eq!(paths[1], "/work/repo/src");
        assert_eq!(paths[2], "/no-such-necoder-root");
        assert_eq!(paths[3], "ssh://dev@host/app");
    }

    #[test]
    fn bundle_root_detects_app_bundle() {
        assert_eq!(
            bundle_root(Path::new(
                "/Applications/necoder.app/Contents/MacOS/necoder"
            )),
            Some(PathBuf::from("/Applications/necoder.app"))
        );
        assert_eq!(bundle_root(Path::new("/work/target/debug/necoder")), None);
        assert_eq!(bundle_root(Path::new("necoder")), None);
    }
}
