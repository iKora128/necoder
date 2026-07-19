//! updater — 自動アップデート（M13。**自前 = velq/karui と同型**・Sparkle 不採用は DECISIONS 済）。
//!
//! 流れ: 起動しばらく後に GitHub Releases の latest を確認 → 新しければ statusbar に
//! 「⬆ vX.Y.Z」チップ → クリックで .dmg をダウンロード → **Apple 署名/公証を検証**
//! （`spctl --assess`。CI が codesign+notarize した dmg なので追加の自前鍵は不要）→
//! 実行中の .app を差し替え → 再起動を促す。
//!
//! ネットワークは `curl`（macOS 標準）に委ねる＝依存ゼロ。ローカル専用（remote host は無関係）。

use anyhow::{Context as _, Result};
use std::path::PathBuf;
use std::process::Command;

/// リリース確認先（GitHub Releases API）。
const RELEASES_URL: &str = "https://api.github.com/repos/iKora128/shirushi/releases/latest";

/// 見つかった新しいリリース。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// 例 "0.2.0"（先頭の v は剥がす）。
    pub version: String,
    /// .dmg アセットの browser_download_url。
    pub dmg_url: String,
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

/// GitHub Releases API の JSON から「current より新しいリリース + .dmg URL」を取り出す。
/// 新しくない・dmg が無い・draft/prerelease は None。
pub fn parse_latest_release(json: &str, current_version: &str) -> Option<UpdateInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value.get("draft").and_then(|v| v.as_bool()).unwrap_or(false)
        || value.get("prerelease").and_then(|v| v.as_bool()).unwrap_or(false)
    {
        return None;
    }
    let tag = value.get("tag_name")?.as_str()?;
    if !version_newer(tag, current_version) {
        return None;
    }
    let assets = value.get("assets")?.as_array()?;
    let dmg_url = assets.iter().find_map(|asset| {
        let name = asset.get("name")?.as_str()?;
        if name.ends_with(".dmg") {
            asset.get("browser_download_url")?.as_str().map(str::to_string)
        } else {
            None
        }
    })?;
    Some(UpdateInfo { version: tag.trim_start_matches('v').to_string(), dmg_url })
}

/// latest リリースを確認する（背景スレッドで呼ぶ）。ネット断・API 制限は静かに None。
pub fn check_for_update(current_version: &str) -> Option<UpdateInfo> {
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "10", "-H", "User-Agent: shirushi-updater", RELEASES_URL])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_latest_release(&String::from_utf8_lossy(&output.stdout), current_version)
}

/// .dmg をダウンロード → Apple 署名/公証を検証 → 実行中の .app を差し替える（背景で呼ぶ）。
/// 成功したら Ok(()) = 「再起動で反映」を UI が案内する。
pub fn download_and_install(info: &UpdateInfo) -> Result<()> {
    let bundle = running_app_bundle()
        .context("実行中の .app が見つかりません（開発ビルドでは更新できません）")?;
    let staging = PathBuf::from(format!("/tmp/shirushi-update-{}.dmg", info.version));
    // 1) ダウンロード。
    let status = Command::new("curl")
        .args(["-fSL", "--max-time", "300", "-o"])
        .arg(&staging)
        .arg(&info.dmg_url)
        .status()
        .context("curl を実行できません")?;
    anyhow::ensure!(status.success(), "ダウンロードに失敗");
    // 2) Apple 署名/公証の検証（改ざん・未署名はここで弾く）。
    let assess = Command::new("spctl")
        .args(["--assess", "--type", "open", "--context", "context:primary-signature", "-v"])
        .arg(&staging)
        .output()
        .context("spctl を実行できません")?;
    anyhow::ensure!(
        assess.status.success(),
        "署名検証に失敗（未署名または改ざんの疑い）: {}",
        String::from_utf8_lossy(&assess.stderr)
    );
    // 3) マウント → .app を差し替え → アンマウント。
    let mount_point = PathBuf::from(format!("/tmp/shirushi-update-mount-{}", info.version));
    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount_point)
        .arg(&staging)
        .output()
        .context("hdiutil attach に失敗")?;
    anyhow::ensure!(attach.status.success(), "dmg のマウントに失敗");
    let result = (|| -> Result<()> {
        let new_app = mount_point.join("Shirushi.app");
        anyhow::ensure!(new_app.exists(), "dmg に Shirushi.app が見つかりません");
        // ditto は bundle を安全に複製する（メタデータ/署名を保つ）。実行中でも置換可能。
        let copy = Command::new("ditto")
            .arg(&new_app)
            .arg(&bundle)
            .status()
            .context("ditto に失敗")?;
        anyhow::ensure!(copy.success(), ".app の差し替えに失敗");
        Ok(())
    })();
    let _detach = Command::new("hdiutil").args(["detach"]).arg(&mount_point).status();
    let _cleanup = std::fs::remove_file(&staging);
    result
}

/// 実行中バイナリの .app バンドル（`…/Shirushi.app/Contents/MacOS/shirushi` → `…/Shirushi.app`）。
fn running_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.ancestors().nth(3)?.to_path_buf();
    (bundle.extension().is_some_and(|ext| ext == "app")).then_some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_is_numeric_per_part() {
        assert!(version_newer("v0.2.0", "0.1.0"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(version_newer("0.1.10", "0.1.9")); // 文字列比較だと逆転する例
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("v0.1.0", "0.2.0"));
        assert!(version_newer("0.2", "0.1.5")); // 桁欠けは 0 扱い
    }

    #[test]
    fn latest_release_parses_dmg_and_filters() {
        let json = r#"{
            "tag_name": "v0.2.0", "draft": false, "prerelease": false,
            "assets": [
                {"name": "SHA256SUMS", "browser_download_url": "https://example.com/sums"},
                {"name": "Shirushi.dmg", "browser_download_url": "https://example.com/Shirushi.dmg"}
            ]
        }"#;
        let update = parse_latest_release(json, "0.1.0").expect("新しい");
        assert_eq!(update.version, "0.2.0");
        assert_eq!(update.dmg_url, "https://example.com/Shirushi.dmg");
        // 同じバージョンなら None。prerelease も None。
        assert!(parse_latest_release(json, "0.2.0").is_none());
        let pre = json.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_latest_release(&pre, "0.1.0").is_none());
    }
}
