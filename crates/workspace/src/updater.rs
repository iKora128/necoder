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

/// リリースの人間向けページ（`v` 付きのタグ）。更新後の「変更点」の行き先。
pub fn release_page_url(version: &str) -> String {
    format!(
        "https://github.com/iKora128/necoder/releases/tag/v{}",
        version.trim_start_matches('v')
    )
}

/// 更新の後の最初の起動か（O45・H17）。状態フォルダの `last_version` と比べ、新しくなっていれば
/// 前の版を返す（今の版を書く）。初めての起動・同じ版・下がった版は `None`（書くだけ）。
/// 1 つのプロセスで返すのは最初の 1 回だけ（窓がいくつ開いても知らせは 1 回）。
pub fn take_update_notice(current: &str) -> Option<String> {
    static TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if TAKEN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let path = paths::state_dir()?.join("last_version");
    note_version(&path, current)
}

/// [`take_update_notice`] の中身（置き場を渡せる・テスト用）。
fn note_version(path: &std::path::Path, current: &str) -> Option<String> {
    let previous = std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    if previous.as_deref() != Some(current) {
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("版の記録の置き場を作れない: {error:#}");
            }
        }
        if let Err(error) = std::fs::write(path, current) {
            eprintln!("版を記録できない: {error:#}");
        }
    }
    previous.filter(|previous| version_newer(current, previous))
}

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
    // Dev.app は実行中の .app をリリース版で置き換えると LSEnvironment ごと消え、次の起動から常用の
    // データを共有してしまう。自動の確認も About のボタンも、経路が無い OS と同じく出さない。
    if paths::is_dev_bundle() {
        return None;
    }
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
        .next_back()
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
    // 現在の正規署名から Team ID を取り、Apple Developer ID と bundle ID も両方確認する。
    let requirement = update_signing_requirement(&bundle)?;
    // 予測不能な 0700 の作業ディレクトリに落とす（旧: /tmp の固定名 = spctl 検証→マウントの間に
    // 差し替えられる理屈上の隙。sticky /tmp で他ユーザーの unlink は防げるが、名前も読めなくする）。
    let staging_guard = UpdateDirectory(unique_staging_dir()?);
    let staging_dir = &staging_guard.0;
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
        verify_update_signature(&new_app, &requirement)?;
        stage_and_replace_bundle(&bundle, &new_app, &|candidate| {
            verify_update_signature(candidate, &requirement)
        })
    })();
    let _detach = Command::new("hdiutil")
        .args(["detach"])
        .arg(&mount_point)
        .status();
    result
}

/// codesign の表示結果から Team ID だけを受ける。要件式へ文字列を挿すため形式も絞る。
fn signing_requirement(signature_info: &str) -> Option<String> {
    let team = signature_info
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))?;
    if team.len() != 10
        || !team
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return None;
    }
    Some(format!(
        "anchor apple generic and identifier \"dev.necoder.editor\" \
         and certificate leaf[subject.OU] = \"{team}\" \
         and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    ))
}

fn update_signing_requirement(bundle: &std::path::Path) -> Result<String> {
    let signature = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4"])
        .arg(bundle)
        .output()
        .context(i18n::t!("update.err_spctl"))?;
    anyhow::ensure!(
        signature.status.success(),
        i18n::t!("update.err_signature", "detail" => String::from_utf8_lossy(&signature.stderr))
    );
    let requirement = signing_requirement(&String::from_utf8_lossy(&signature.stderr)).context(
        i18n::t!("update.err_signature", "detail" => String::from_utf8_lossy(&signature.stderr)),
    )?;
    verify_update_signature(bundle, &requirement)?;
    Ok(requirement)
}

fn verify_update_signature(bundle: &std::path::Path, requirement: &str) -> Result<()> {
    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(format!("-R={requirement}"))
        .arg(bundle)
        .output()
        .context(i18n::t!("update.err_spctl"))?;
    anyhow::ensure!(
        verified.status.success(),
        i18n::t!("update.err_signature", "detail" => String::from_utf8_lossy(&verified.stderr))
    );
    Ok(())
}

struct UpdateDirectory(PathBuf);

impl Drop for UpdateDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!(
                "更新作業の置き場を消せない（{}）: {error}",
                self.0.display()
            );
        }
    }
}

/// 同じファイルシステムの空の置き場へ全体を複製し、検証が成功してから一括で交換する。
/// 旧 bundle に重ね書きしないため、コピー失敗も削除された旧リソースも持ち込まない。
fn stage_and_replace_bundle(
    bundle: &std::path::Path,
    source: &std::path::Path,
    verify: &dyn Fn(&std::path::Path) -> Result<()>,
) -> Result<()> {
    let parent = bundle.parent().context(i18n::t!("update.err_no_bundle"))?;
    let staging = UpdateDirectory(unique_staging_dir_in(parent)?);
    let candidate = staging.0.join("necoder.app");
    let copy = Command::new("/usr/bin/ditto")
        .arg(source)
        .arg(&candidate)
        .status()
        .context(i18n::t!("update.err_ditto"))?;
    anyhow::ensure!(copy.success(), i18n::t!("update.err_replace"));
    verify(&candidate)?;
    swap_bundles(bundle, &candidate)
}

fn swap_bundles(bundle: &std::path::Path, candidate: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let bundle = std::ffi::CString::new(bundle.as_os_str().as_bytes())?;
        let candidate = std::ffi::CString::new(candidate.as_os_str().as_bytes())?;
        // SAFETY: NUL 終端した 2 つのパスを渡す。成功時だけ両方を原子的に交換する API。
        let result =
            unsafe { libc::renamex_np(bundle.as_ptr(), candidate.as_ptr(), libc::RENAME_SWAP) };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).context(i18n::t!("update.err_replace"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (bundle, candidate);
        anyhow::bail!(i18n::t!("update.err_replace"))
    }
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
    unique_staging_dir_in(&std::env::temp_dir())
}

fn unique_staging_dir_in(parent: &std::path::Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = parent.join(format!(".necoder-update-{}-{nanos}", std::process::id()));
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

    #[test]
    fn update_requirements_pin_the_publisher_and_the_app() {
        let requirement =
            signing_requirement("Identifier=dev.necoder.editor\nTeamIdentifier=ABCDE12345\n")
                .unwrap();
        assert!(requirement.contains("anchor apple generic"));
        assert!(requirement.contains("identifier \"dev.necoder.editor\""));
        assert!(requirement.contains("certificate leaf[subject.OU] = \"ABCDE12345\""));
        assert!(requirement.contains("1.2.840.113635.100.6.1.13"));
        for invalid in [
            "",
            "TeamIdentifier=not set",
            "TeamIdentifier=abcde12345",
            "TeamIdentifier=ABCDE12345 or true",
        ] {
            assert!(signing_requirement(invalid).is_none(), "{invalid}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_ad_hoc_signature_is_not_trusted_for_updates() {
        let fixture = UpdateDirectory(unique_staging_dir().unwrap());
        let executable = fixture.0.join("adhoc");
        std::fs::copy("/usr/bin/true", &executable).unwrap();
        assert!(Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--identifier",
                "dev.necoder.editor"
            ])
            .arg(&executable)
            .output()
            .unwrap()
            .status
            .success());
        assert!(update_signing_requirement(&executable).is_err());
        let requirement = signing_requirement("TeamIdentifier=ABCDE12345").unwrap();
        assert!(verify_update_signature(&executable, &requirement).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_replacement_preserves_the_original_until_copy_and_verification_succeed() {
        let fixture = UpdateDirectory(unique_staging_dir().unwrap());
        let old = fixture.0.join("installed.app");
        let source = fixture.0.join("source.app");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&source).unwrap();
        std::fs::write(old.join("version"), "old").unwrap();
        std::fs::write(old.join("removed-resource"), "old resource").unwrap();
        std::fs::write(source.join("version"), "new").unwrap();

        assert!(stage_and_replace_bundle(&old, &source, &|_| anyhow::bail!(
            "synthetic signature failure"
        ))
        .is_err());
        assert_eq!(std::fs::read_to_string(old.join("version")).unwrap(), "old");
        assert!(old.join("removed-resource").exists());
        assert!(swap_bundles(&old, &fixture.0.join("missing.app")).is_err());
        assert_eq!(std::fs::read_to_string(old.join("version")).unwrap(), "old");

        stage_and_replace_bundle(&old, &source, &|candidate| {
            anyhow::ensure!(
                std::fs::read_to_string(candidate.join("version"))? == "new",
                "copy mismatch"
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(old.join("version")).unwrap(), "new");
        assert!(
            !old.join("removed-resource").exists(),
            "削除されたリソースが残らない"
        );
        assert_eq!(
            std::fs::read_dir(&fixture.0).unwrap().count(),
            2,
            "作業ディレクトリを残さない"
        );
    }

    /// O45・H17: 更新の後の最初の起動だけ前の版を返す（初回・同じ版・下がった版は何も言わない）。
    #[test]
    fn only_the_first_launch_after_an_upgrade_is_noted() {
        let dir = std::env::temp_dir().join(format!("necoder_last_version_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("last_version");
        assert_eq!(note_version(&path, "0.1.20"), None, "初めての起動");
        assert_eq!(note_version(&path, "0.1.20"), None, "同じ版");
        assert_eq!(note_version(&path, "0.1.21"), Some("0.1.20".to_string()));
        assert_eq!(note_version(&path, "0.1.21"), None, "知らせは 1 回");
        assert_eq!(note_version(&path, "0.1.19"), None, "下がった版");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "0.1.19");
        assert_eq!(
            release_page_url("0.1.21"),
            "https://github.com/iKora128/necoder/releases/tag/v0.1.21"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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
