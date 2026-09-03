//! updater — 自動アップデート（M13。**自前 = velq/karui と同型**・Sparkle 不採用は DECISIONS 済）。
//!
//! 流れ: 起動しばらく後に GitHub Releases の latest を確認 → 新しければ statusbar に
//! 「⬆ vX.Y.Z」チップ。クリック後の適用経路は OS で分かれる（[`UpdateAction`]）:
//!
//! - **macOS**: .dmg をダウンロード → **Apple 署名/公証を検証**（`spctl --assess`。
//!   CI が codesign+notarize した dmg なので追加の自前鍵は不要）→ 実行中の .app を
//!   差し替え → 再起動を促す。
//! - **Windows（当面・WINDOWS-PORT §W6）**: Release ページを既定ブラウザで開き、
//!   zip の入れ替えは手動。アプリ内適用（zip 展開 → 実行中 exe の差し替え）は後続。
//!
//! ネットワークは `curl`（macOS / Windows 10+ 標準）に委ねる＝依存ゼロ。
//! ローカル専用（remote host は無関係）。

use anyhow::{Context as _, Result};
use std::path::PathBuf;
use std::process::Command;

/// リリース確認先（GitHub Releases API）。
const RELEASES_URL: &str = "https://api.github.com/repos/iKora128/necoder/releases/latest";

/// 見つかった新しいリリース。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// 例 "0.2.0"（先頭の v は剥がす）。
    pub version: String,
    /// チップをクリックしたときの適用経路。
    pub action: UpdateAction,
}

/// チップクリック後の適用経路（OS で分かれる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAction {
    /// macOS: .dmg を DL → 署名検証 → 実行中の .app を差し替え（アプリ内更新）。
    InstallDmg {
        /// .dmg アセットの browser_download_url。
        dmg_url: String,
    },
    /// Windows（当面・WINDOWS-PORT §W6）: Release ページを既定ブラウザで開く。
    /// zip の入れ替えは手動＝クリックが「必ず失敗する」形にはならない。
    OpenReleasePage {
        /// リリースの html_url（人間向けページ）。
        html_url: String,
    },
}

/// このプラットフォームの更新導線。None = チップを出さない。
///
/// - macOS: アプリ内更新（.dmg + `spctl` + `hdiutil`）。
/// - Windows: Release ページを開くだけ（アプリ内適用は WINDOWS-PORT §W6 の残項目）。
/// - それ以外（Linux 等）: **配布物が無い**ので確認自体をしない。
fn platform_update_route() -> Option<UpdateRoute> {
    if cfg!(target_os = "macos") {
        Some(UpdateRoute::InAppDmg)
    } else if cfg!(target_os = "windows") {
        Some(UpdateRoute::ReleasePage)
    } else {
        None
    }
}

/// [`parse_latest_release`] にどの適用経路の情報を求めるか（テストから OS 非依存で使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateRoute {
    /// .dmg アセットを探す（macOS）。
    InAppDmg,
    /// Windows zip アセットの存在を確認して Release ページ URL を返す。
    ReleasePage,
}

/// `x.y.z` を比較して latest が current より新しいか（数値比較・桁欠けは 0 扱い）。
fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |value: &str| -> Vec<u64> {
        value
            .trim_start_matches('v')
            .split('.')
            .map(|part| {
                part.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (latest, current) = (parse(latest), parse(current));
    for index in 0..latest.len().max(current.len()) {
        let a = latest.get(index).copied().unwrap_or(0);
        let b = current.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// GitHub Releases API の JSON から「current より新しいリリース + 適用経路の情報」を取り出す。
/// 新しくない・この OS 向けアセットが無い・draft/prerelease は None。
pub fn parse_latest_release(
    json: &str,
    current_version: &str,
    route: UpdateRoute,
) -> Option<UpdateInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value
        .get("draft")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || value
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return None;
    }
    let tag = value.get("tag_name")?.as_str()?;
    if !version_newer(tag, current_version) {
        return None;
    }
    let assets = value.get("assets")?.as_array()?;
    let asset_url = |predicate: fn(&str) -> bool| {
        assets.iter().find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            if predicate(name) {
                asset
                    .get("browser_download_url")?
                    .as_str()
                    .map(str::to_string)
            } else {
                None
            }
        })
    };
    let action = match route {
        UpdateRoute::InAppDmg => UpdateAction::InstallDmg {
            dmg_url: asset_url(|name| name.ends_with(".dmg"))?,
        },
        UpdateRoute::ReleasePage => {
            // Windows 版の zip（release.yml が付ける `necoder-windows-x64.zip`）が
            // 付いているリリースだけ案内する。zip の無いリリースへ誘導しない。
            asset_url(|name| name.contains("windows") && name.ends_with(".zip"))?;
            UpdateAction::OpenReleasePage {
                html_url: value.get("html_url")?.as_str()?.to_string(),
            }
        }
    };
    Some(UpdateInfo {
        version: tag.trim_start_matches('v').to_string(),
        action,
    })
}

/// この OS で更新確認が意味を持つか（= [`platform_update_route`] があるか）。
/// About モーダルは false のとき「アップデートを確認」ボタンごと出さない
/// （「押せるのに必ず失敗する」導線を作らない規律の手動版）。
pub fn manual_check_supported() -> bool {
    platform_update_route().is_some()
}

/// latest リリースを確認する（背景スレッドで呼ぶ）。ネット断・API 制限は静かに None。
///
/// **「押せるのに必ず失敗する」チップを出さない**のが規律（2026-08-24・Windows 版を
/// 配る直前に気づいた）。だから経路は [`platform_update_route`] で OS ごとに選び、
/// 適用できない OS（Linux 等）では確認自体をしない。Windows はアプリ内適用の代わりに
/// Release ページを開く＝クリックは常に成立する。
pub fn check_for_update(current_version: &str) -> Option<UpdateInfo> {
    check_for_update_manual(current_version).ok().flatten()
}

/// 手動の更新確認（メニュー / About モーダル）。自動チェックと違い**結果を言い分ける**:
/// `Ok(Some)` = 新版あり / `Ok(None)` = 最新 / `Err` = 確認そのものに失敗（ネット断等）。
/// 自動チェック（[`check_for_update`]）はこれを畳んで「失敗も最新も黙る」従来挙動のまま。
pub fn check_for_update_manual(current_version: &str) -> Result<Option<UpdateInfo>> {
    let Some(route) = platform_update_route() else {
        // 配布物の無い OS（Linux 等）。呼び手はボタンを出さない前提だが、来ても壊れない。
        return Ok(None);
    };
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "User-Agent: necoder-updater",
            RELEASES_URL,
        ])
        .output()
        .context(i18n::t!("update.err_curl"))?;
    anyhow::ensure!(output.status.success(), i18n::t!("update.err_check"));
    Ok(parse_latest_release(
        &String::from_utf8_lossy(&output.stdout),
        current_version,
        route,
    ))
}

/// 更新の進み（statusbar チップの進捗バー用）。`download_and_install` が背景スレッドから報告する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UpdateProgress {
    /// ダウンロード中。`fraction` は 0.0..=1.0（Content-Length が取れなければ None）。
    Downloading { fraction: Option<f32> },
    /// Apple 署名/公証の検証中。
    Verifying,
    /// 実行中の .app を差し替え中。
    Replacing,
}

/// `curl -I` の応答ヘッダから Content-Length を取る（リダイレクト追従で複数応答が並ぶので最後のもの）。
fn parse_content_length(headers: &str) -> Option<u64> {
    headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
        .last()
}

/// ダウンロードの総バイト数を HEAD で聞く（進捗バーの分母）。取れなければ None＝不定表示。
fn remote_content_length(url: &str) -> Option<u64> {
    let output = Command::new("curl")
        .args(["-sIL", "--max-time", "20"])
        .arg(url)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| parse_content_length(&String::from_utf8_lossy(&output.stdout)))
        .flatten()
}

/// .dmg をダウンロード → Apple 署名/公証を検証 → 実行中の .app を差し替える（背景で呼ぶ）。
/// 成功したら Ok(()) = 「再起動」を UI が出す。macOS 専用（[`UpdateAction::InstallDmg`]）。
/// `report` は段階と進み（ダウンロードは ~120ms ごと）を UI へ知らせるコールバック。
pub fn download_and_install(dmg_url: &str, report: &dyn Fn(UpdateProgress)) -> Result<()> {
    let bundle = running_app_bundle().context(i18n::t!("update.err_no_bundle"))?;
    // 予測不能な 0700 の作業ディレクトリに落とす（旧: /tmp の固定名 = spctl 検証→マウントの間に
    // 差し替えられる理屈上の隙。sticky /tmp で他ユーザーの unlink は防げるが、名前も読めなくする）。
    let staging_dir = unique_staging_dir()?;
    let staging = staging_dir.join("necoder.dmg");
    // 1) ダウンロード。curl を子プロセスで走らせ、落ちてくるファイルの大きさを見て進みを報告する
    //    （curl の進捗出力を解析するより単純で、-o 先の metadata は常に取れる）。
    report(UpdateProgress::Downloading { fraction: None });
    let total = remote_content_length(dmg_url);
    let mut child = Command::new("curl")
        .args(["-fsSL", "--max-time", "300", "-o"])
        .arg(&staging)
        .arg(dmg_url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(i18n::t!("update.err_curl"))?;
    let status = loop {
        if let Some(status) = child.try_wait().context(i18n::t!("update.err_curl"))? {
            break status;
        }
        let received = std::fs::metadata(&staging)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let fraction = total
            .filter(|total| *total > 0)
            .map(|total| (received as f64 / total as f64).min(1.0) as f32);
        report(UpdateProgress::Downloading { fraction });
        std::thread::sleep(std::time::Duration::from_millis(120));
    };
    anyhow::ensure!(status.success(), i18n::t!("update.err_download"));
    report(UpdateProgress::Downloading {
        fraction: Some(1.0),
    });
    // 2) Apple 署名/公証の検証（改ざん・未署名はここで弾く）。
    report(UpdateProgress::Verifying);
    let assess = Command::new("spctl")
        .args([
            "--assess",
            "--type",
            "open",
            "--context",
            "context:primary-signature",
            "-v",
        ])
        .arg(&staging)
        .output()
        .context(i18n::t!("update.err_spctl"))?;
    anyhow::ensure!(
        assess.status.success(),
        i18n::t!("update.err_signature", "detail" => String::from_utf8_lossy(&assess.stderr))
    );
    // 3) マウント → .app を差し替え → アンマウント。
    report(UpdateProgress::Replacing);
    let mount_point = staging_dir.join("mount");
    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount_point)
        .arg(&staging)
        .output()
        .context(i18n::t!("update.err_attach"))?;
    anyhow::ensure!(attach.status.success(), i18n::t!("update.err_mount"));
    let result = (|| -> Result<()> {
        let new_app = mount_point.join("necoder.app");
        anyhow::ensure!(new_app.exists(), i18n::t!("update.err_no_app"));
        // ditto は bundle を安全に複製する（メタデータ/署名を保つ）。実行中でも置換可能。
        let copy = Command::new("ditto")
            .arg(&new_app)
            .arg(&bundle)
            .status()
            .context(i18n::t!("update.err_ditto"))?;
        anyhow::ensure!(copy.success(), i18n::t!("update.err_replace"));
        Ok(())
    })();
    let _detach = Command::new("hdiutil")
        .args(["detach"])
        .arg(&mount_point)
        .status();
    let _cleanup = std::fs::remove_dir_all(&staging_dir);
    result
}

/// 差し替え後の再起動: 自プロセスの終了を待ってから .app を開き直す子プロセスを切り離して起動する。
/// 呼び出し側はこの後、通常の Quit（hot exit 破棄・窓セッション整理）を発行する。
/// `open` は LaunchServices 経由なので、新しい bundle の署名でそのまま起動する。macOS 専用。
pub fn spawn_relauncher() -> Result<()> {
    let bundle = running_app_bundle().context(i18n::t!("update.err_no_bundle"))?;
    let pid = std::process::id();
    // $0 = bundle パス（引数で渡す＝パスのクォート問題を持ち込まない）。
    let script =
        format!("while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open \"$0\"");
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(script)
        .arg(&bundle)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // 親（このアプリ）の終了に巻き込まれないよう別プロセスグループへ。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command.spawn().context(i18n::t!("update.err_relaunch"))?;
    Ok(())
}

/// 更新作業用の一意なディレクトリ（unix は 0700）を作る。既存衝突は fail-closed（作り直さない）。
fn unique_staging_dir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("necoder-update-{}-{nanos}", std::process::id()));
    // `mode()` を呼ぶのは unix だけ＝Windows では `mut` が不要になる（W2 の受入条件は警告 0）。
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .context(i18n::t!("update.err_staging"))?;
    Ok(dir)
}

/// 実行中バイナリの .app バンドル（`…/necoder.app/Contents/MacOS/necoder` → `…/necoder.app`）。
fn running_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.ancestors().nth(3)?.to_path_buf();
    (bundle.extension().is_some_and(|ext| ext == "app")).then_some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 進捗バーの分母: リダイレクト追従の `curl -sIL` は応答ヘッダが複数並ぶ（302 → 200）。
    /// 最後の応答の Content-Length を取り、大文字小文字は区別しない。
    #[test]
    fn content_length_comes_from_the_last_response() {
        let headers = "HTTP/2 302\r\nlocation: https://objects.example/x\r\ncontent-length: 0\r\n\r\n\
                       HTTP/2 200\r\nContent-Type: application/octet-stream\r\nContent-Length: 12345678\r\n\r\n";
        assert_eq!(parse_content_length(headers), Some(12_345_678));
        assert_eq!(parse_content_length("HTTP/2 200\r\nx: y\r\n"), None);
        assert_eq!(parse_content_length("content-length: abc"), None);
    }

    /// **押せるのに必ず失敗するチップを出さない**（WINDOWS-PORT.md §W6）。
    ///
    /// アプリ内適用（`.dmg` + `spctl` + `hdiutil`）は macOS 専用。Windows のチップは
    /// アプリ内適用ではなく **Release ページを開くだけ**の経路に必ず落ちること。
    /// 配布物の無い Linux 等では確認自体をしない（チップも出ない）。
    /// **この期待は書いておかないと、うっかり有効化して初回リリースで踏む。**
    #[test]
    fn update_route_matches_what_the_platform_can_actually_apply() {
        let route = platform_update_route();
        if cfg!(target_os = "macos") {
            assert_eq!(route, Some(UpdateRoute::InAppDmg));
        } else if cfg!(target_os = "windows") {
            assert_eq!(route, Some(UpdateRoute::ReleasePage));
        } else {
            assert_eq!(
                route, None,
                "配布物の無い OS で更新チップを出してはいけない"
            );
            assert!(check_for_update("0.0.1").is_none());
        }
    }

    #[test]
    fn version_compare_is_numeric_per_part() {
        assert!(version_newer("v0.2.0", "0.1.0"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(version_newer("0.1.10", "0.1.9")); // 文字列比較だと逆転する例
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("v0.1.0", "0.2.0"));
        assert!(version_newer("0.2", "0.1.5")); // 桁欠けは 0 扱い
    }

    const RELEASE_JSON: &str = r#"{
        "tag_name": "v0.2.0", "draft": false, "prerelease": false,
        "html_url": "https://github.com/iKora128/necoder/releases/tag/v0.2.0",
        "assets": [
            {"name": "SHA256SUMS", "browser_download_url": "https://example.com/sums"},
            {"name": "necoder.dmg", "browser_download_url": "https://example.com/necoder.dmg"},
            {"name": "necoder-windows-x64.zip", "browser_download_url": "https://example.com/necoder-windows-x64.zip"}
        ]
    }"#;

    #[test]
    fn latest_release_parses_dmg_and_filters() {
        let update =
            parse_latest_release(RELEASE_JSON, "0.1.0", UpdateRoute::InAppDmg).expect("新しい");
        assert_eq!(update.version, "0.2.0");
        assert_eq!(
            update.action,
            UpdateAction::InstallDmg {
                dmg_url: "https://example.com/necoder.dmg".to_string()
            }
        );
        // 同じバージョンなら None。prerelease も None。
        assert!(parse_latest_release(RELEASE_JSON, "0.2.0", UpdateRoute::InAppDmg).is_none());
        let pre = RELEASE_JSON.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_latest_release(&pre, "0.1.0", UpdateRoute::InAppDmg).is_none());
        // dmg アセットの無いリリースは mac に案内しない。
        let no_dmg = RELEASE_JSON.replace("necoder.dmg", "necoder.txt");
        assert!(parse_latest_release(&no_dmg, "0.1.0", UpdateRoute::InAppDmg).is_none());
    }

    /// Windows（当面）: zip 付きリリースだけ「Release ページを開く」チップを出す。
    #[test]
    fn release_page_route_requires_a_windows_zip() {
        let update =
            parse_latest_release(RELEASE_JSON, "0.1.0", UpdateRoute::ReleasePage).expect("新しい");
        assert_eq!(update.version, "0.2.0");
        assert_eq!(
            update.action,
            UpdateAction::OpenReleasePage {
                html_url: "https://github.com/iKora128/necoder/releases/tag/v0.2.0".to_string()
            }
        );
        // zip の無いリリース（Windows ビルドが無い）へは誘導しない。
        let no_zip = RELEASE_JSON.replace("necoder-windows-x64.zip", "necoder-windows-x64.msix");
        assert!(parse_latest_release(&no_zip, "0.1.0", UpdateRoute::ReleasePage).is_none());
    }
}
