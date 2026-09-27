//! deploy — レジストリの binary の配布を手元に置いて起動できるようにする（H2-b・issue #38）。
//!
//! 外から落とした実行ファイルを走らせるので、**落とす元・置き場・検証**をここに固定する:
//!
//! - **落とす元**: ACP の公開レジストリ（[`crate::registry::REGISTRY_URL`]）の JSON に書かれた
//!   `distribution.binary.<os>-<arch>.archive` の URL **だけ**。necoder は URL を組み立てない・書き換えない。
//!   取るのは `curl`（`--proto =https`・リダイレクト先も https だけ）。https でない URL は落とさない
//! - **置き場**: `<data>/external_agents/binary/<id>/<version>/<os>-<arch>/`（[`binary_root`]・
//!   `paths::external_agents_dir`）。版ごとに別のフォルダ＝更新しても走っている版を書き換えない。
//!   同じ版が置いてあれば落とし直さない（npx の版の解決と同じくキャッシュを使う）。新しい版を置いたら、
//!   1 つ前の版だけ残して古い版を消す（走っている古いスレッドの足元をすぐには消さない）
//! - **検証**: レジストリに `sha256` があれば、落とした書庫の sha256 と照合し、違えば展開せずに捨てる。
//!   無い物は検証できない — 追加の画面と設定の行に「検証の値がありません」と出し、完了の印にも残す
//! - **展開**: 置き場の隣の一時フォルダに落として、中身の名前を先に確かめる（絶対パス・`..` を含む書庫は
//!   展開しない）→ 展開 → 起動するコマンドが置き場の中にあることを確かめる → 完了の印を書いて rename で
//!   公開する。途中で落ちても半端な物を使わない（印の無いフォルダは「置いていない」）
//!
//! 展開は OS の `tar`（macOS / Windows の bsdtar は zip も読む）と、Linux の zip だけ `unzip` に委ねる
//! （`curl` と同じく依存を増やさない）。検証の sha256 だけは crate（`sha2`）を使う。

use crate::registry::BinaryDistribution;
use sha2::Digest as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 置いた版の完了の印（置き場のフォルダの中）。これが在るフォルダだけを「置いてある」と見る。
const MARKER: &str = ".necoder-deployed.json";

/// binary を置く根（`<data>/external_agents/binary`）。
pub fn binary_root() -> Option<PathBuf> {
    Some(paths::external_agents_dir()?.join("binary"))
}

/// 配備できなかった理由（UI が言葉にする・acp_client は i18n を持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployError {
    /// 置き場が決まらない（ホームが分からない等）。
    NoPlace,
    /// id / 版 / プラットフォームにフォルダ名に使えない文字がある。
    InvalidName(String),
    /// https でない URL（落とさない）。
    InsecureUrl(String),
    /// 落とせない（ネットワーク・404 等）。
    Download { url: String, reason: String },
    /// 落とした物の sha256 がレジストリの値と合わない（展開せずに捨てた）。
    ChecksumMismatch { expected: String, actual: String },
    /// 書庫の中に置き場の外を指す名前がある（展開しない）。
    UnsafeArchive(String),
    /// 展開できない（書庫の形が分からない・tar / unzip の失敗）。
    Extract(String),
    /// 起動するコマンドが展開した中に無い（または置き場の外を指す）。
    CommandMissing(String),
    /// ファイルの読み書きの失敗。
    Io(String),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployError::NoPlace => write!(formatter, "binary の置き場が決まらない"),
            DeployError::InvalidName(name) => write!(formatter, "フォルダ名に使えない: {name}"),
            DeployError::InsecureUrl(url) => {
                write!(formatter, "https でない URL は落とさない: {url}")
            }
            DeployError::Download { url, reason } => {
                write!(formatter, "{url} を落とせない: {reason}")
            }
            DeployError::ChecksumMismatch { expected, actual } => write!(
                formatter,
                "sha256 が合わない（レジストリ {expected} / 落とした物 {actual}）"
            ),
            DeployError::UnsafeArchive(entry) => {
                write!(formatter, "置き場の外を指す名前がある書庫: {entry}")
            }
            DeployError::Extract(reason) => write!(formatter, "展開できない: {reason}"),
            DeployError::CommandMissing(cmd) => {
                write!(formatter, "起動するコマンドが展開した中に無い: {cmd}")
            }
            DeployError::Io(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for DeployError {}

/// 置いてある 1 版。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployed {
    /// 起動するコマンド（置き場の中の絶対パス）。
    pub command: PathBuf,
    /// sha256 を照合したか（`false` = レジストリに値が無かった）。
    pub verified: bool,
    /// 置いてある版。
    pub version: String,
}

/// 1 件の配備先（レジストリの id・版・プラットフォームと、その配布）。
pub struct BinaryTarget<'a> {
    pub id: &'a str,
    pub version: &'a str,
    /// レジストリのキー（`darwin-aarch64` 等）。
    pub platform: &'a str,
    pub distribution: &'a BinaryDistribution,
}

impl BinaryTarget<'_> {
    /// この版の置き場（`<root>/<id>/<version>/<platform>`）。名前はフォルダ名に使える文字だけ。
    pub fn directory(&self, root: &Path) -> Result<PathBuf, DeployError> {
        Ok(root
            .join(safe_component(self.id)?)
            .join(safe_component(self.version)?)
            .join(safe_component(self.platform)?))
    }

    /// 置いてあればその版（完了の印と起動するコマンドが在る時だけ）。ファイルを 2 つ見るだけ。
    pub fn installed(&self, root: &Path) -> Option<Deployed> {
        read_marker(&self.directory(root).ok()?)
    }

    /// 置いてあればそれを、無ければ落として検証・展開して置く（**blocking・ネットワーク**。背景で呼ぶ）。
    pub fn deploy(&self, root: &Path) -> Result<Deployed, DeployError> {
        self.deploy_with(root, download_with_curl)
    }

    /// [`Self::deploy`] の本体（落とし方を渡せる・テストは手元のファイルを写す）。
    pub(crate) fn deploy_with(
        &self,
        root: &Path,
        fetch: impl Fn(&str, &Path) -> Result<(), DeployError>,
    ) -> Result<Deployed, DeployError> {
        // 同じアプリで 2 本のスレッドが同時に初回を起こしても、落とすのは 1 回（後の方は置いた物を使う）。
        static DEPLOYING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = DEPLOYING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let directory = self.directory(root)?;
        if let Some(deployed) = read_marker(&directory) {
            return Ok(deployed);
        }
        let url = self.distribution.archive.as_str();
        if !url.starts_with("https://") {
            return Err(DeployError::InsecureUrl(url.to_string()));
        }
        let agent_root = root.join(safe_component(self.id)?);
        std::fs::create_dir_all(&agent_root).map_err(io_error("置き場を作れない"))?;
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        );
        let download = agent_root.join(format!(".download-{unique}"));
        let staging = agent_root.join(format!(".staging-{unique}"));
        let result = self.stage(url, &download, &staging, &directory, &fetch);
        if let Err(error) = std::fs::remove_file(&download) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("落とした書庫を消せない: {}: {error}", download.display());
            }
        }
        match result {
            Ok(deployed) => {
                prune_other_versions(root, self.id, self.version, self.platform);
                Ok(deployed)
            }
            Err(error) => {
                if staging.exists() {
                    if let Err(cleanup) = std::fs::remove_dir_all(&staging) {
                        eprintln!("展開の途中の物を消せない: {}: {cleanup}", staging.display());
                    }
                }
                Err(error)
            }
        }
    }

    /// 落とす → 照合 → 展開 → コマンドを確かめる → 印を書く → rename で公開。
    fn stage(
        &self,
        url: &str,
        download: &Path,
        staging: &Path,
        directory: &Path,
        fetch: &impl Fn(&str, &Path) -> Result<(), DeployError>,
    ) -> Result<Deployed, DeployError> {
        fetch(url, download)?;
        let expected = self
            .distribution
            .sha256
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase());
        if let Some(expected) = &expected {
            let actual = sha256_file(download)?;
            if &actual != expected {
                return Err(DeployError::ChecksumMismatch {
                    expected: expected.clone(),
                    actual,
                });
            }
        }
        std::fs::create_dir_all(staging).map_err(io_error("展開先を作れない"))?;
        let relative = command_relative(&self.distribution.cmd)?;
        extract(download, url, staging, &relative)?;
        let command = staging.join(&relative);
        if !command.is_file() {
            return Err(DeployError::CommandMissing(self.distribution.cmd.clone()));
        }
        make_executable(&command)?;
        let marker = serde_json::json!({
            "id": self.id,
            "version": self.version,
            "platform": self.platform,
            "url": url,
            "sha256": expected,
            "verified": expected.is_some(),
            "command": relative.to_string_lossy(),
        });
        std::fs::write(staging.join(MARKER), marker.to_string())
            .map_err(io_error("完了の印を書けない"))?;
        let parent = directory
            .parent()
            .ok_or_else(|| DeployError::Io("置き場の親が無い".to_string()))?;
        std::fs::create_dir_all(parent).map_err(io_error("版の置き場を作れない"))?;
        // 印の無い置き場（消している途中で落ちた等）は、置いていないのと同じ＝片付けてから置く。
        if directory.exists() && read_marker(directory).is_none() {
            std::fs::remove_dir_all(directory).map_err(io_error("壊れた置き場を片付けられない"))?;
        }
        if let Err(error) = std::fs::rename(staging, directory) {
            // 別の necoder が先に同じ版を置いた時だけ、その完成品を使う（自分の分は捨てる）。
            if let Some(deployed) = read_marker(directory) {
                if let Err(cleanup) = std::fs::remove_dir_all(staging) {
                    eprintln!(
                        "使わなかった展開を消せない: {}: {cleanup}",
                        staging.display()
                    );
                }
                return Ok(deployed);
            }
            return Err(DeployError::Io(format!(
                "置き場へ移せない: {}: {error}",
                directory.display()
            )));
        }
        read_marker(directory)
            .ok_or_else(|| DeployError::CommandMissing(self.distribution.cmd.clone()))
    }
}

/// 同じエージェント・同じプラットフォームで置いてある版のうち、`except` 以外で一番新しい物
/// （新しい版を落とせなかった時に手元の版で起こすため）。
pub fn latest_other_installed(
    root: &Path,
    id: &str,
    platform: &str,
    except: &str,
) -> Option<Deployed> {
    let mut found: Vec<(std::time::SystemTime, Deployed)> = installed_versions(root, id, platform)
        .into_iter()
        .filter(|(version, _)| version != except)
        .filter_map(|(_, directory)| {
            let modified = std::fs::metadata(directory.join(MARKER))
                .and_then(|metadata| metadata.modified())
                .ok()?;
            Some((modified, read_marker(&directory)?))
        })
        .collect();
    found.sort_by(|left, right| right.0.cmp(&left.0));
    found.into_iter().next().map(|(_, deployed)| deployed)
}

/// `<root>/<id>/*/<platform>` のうち完了の印がある物（版の名前と置き場）。
fn installed_versions(root: &Path, id: &str, platform: &str) -> Vec<(String, PathBuf)> {
    let (Ok(id), Ok(platform)) = (safe_component(id), safe_component(platform)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root.join(id)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let version = entry.file_name().to_string_lossy().into_owned();
            if version.starts_with('.') {
                return None; // 途中の物（.staging- / .download-）
            }
            let directory = entry.path().join(platform);
            directory
                .join(MARKER)
                .is_file()
                .then_some((version, directory))
        })
        .collect()
}

/// 新しい版を置いた後、`keep` と 1 つ前（印の新しい順）の版だけ残して消す。消せなくても起動は続ける。
fn prune_other_versions(root: &Path, id: &str, keep: &str, platform: &str) {
    let mut others: Vec<(std::time::SystemTime, PathBuf)> = installed_versions(root, id, platform)
        .into_iter()
        .filter(|(version, _)| version != keep)
        .map(|(_, directory)| {
            let modified = std::fs::metadata(directory.join(MARKER))
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, directory)
        })
        .collect();
    others.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, directory) in others.into_iter().skip(1) {
        if let Err(error) = std::fs::remove_dir_all(&directory) {
            eprintln!("古い版を消せない: {}: {error}", directory.display());
            continue;
        }
        // 版のフォルダが空になったら一緒に消す（他のプラットフォームが残っていれば残る）。
        if let Some(version_directory) = directory.parent() {
            let empty = std::fs::read_dir(version_directory)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if empty {
                if let Err(error) = std::fs::remove_dir(version_directory) {
                    eprintln!(
                        "古い版のフォルダを消せない: {}: {error}",
                        version_directory.display()
                    );
                }
            }
        }
    }
}

/// 完了の印を読む（印と起動するコマンドが在る時だけ `Some`）。
fn read_marker(directory: &Path) -> Option<Deployed> {
    let text = std::fs::read_to_string(directory.join(MARKER)).ok()?;
    let marker: serde_json::Value = serde_json::from_str(&text).ok()?;
    let relative = command_relative(marker.get("command")?.as_str()?).ok()?;
    let command = directory.join(relative);
    command.is_file().then(|| Deployed {
        command,
        verified: marker
            .get("verified")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        version: marker
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// フォルダ名に使える名前か（英数字と `.` `_` `-` `+` だけ・`.` / `..` / 空は不可）。レジストリの id・版・
/// プラットフォームのキーは他人が書くので、置き場の外を指す名前を通さない。
fn safe_component(name: &str) -> Result<&str, DeployError> {
    let allowed = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-+".contains(character));
    if allowed {
        Ok(name)
    } else {
        Err(DeployError::InvalidName(name.to_string()))
    }
}

/// レジストリの `cmd`（`./amp-acp`・`./dist-package/cursor-agent`・`./goose-package\goose.exe`）を、
/// 置き場の中の相対パスへ直す。`/` と `\` のどちらも区切りとして読み、`..`・絶対パスは通さない。
fn command_relative(cmd: &str) -> Result<PathBuf, DeployError> {
    let mut relative = PathBuf::new();
    for part in cmd.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return Err(DeployError::CommandMissing(cmd.to_string())),
            part if part.contains(':') => return Err(DeployError::CommandMissing(cmd.to_string())),
            part => relative.push(part),
        }
    }
    if relative.as_os_str().is_empty() || cmd.starts_with('/') || cmd.starts_with('\\') {
        return Err(DeployError::CommandMissing(cmd.to_string()));
    }
    Ok(relative)
}

/// 書庫の形（URL の最後の名前で決める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Tar,
    Zip,
    /// 書庫ではなく実行ファイルそのもの（拡張子なし・`.exe`）。
    Raw,
    /// 読めない形（1 ファイルだけの `.gz` / `.dmg` 等）。
    Unsupported,
}

fn archive_kind(url: &str) -> ArchiveKind {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    const TAR: [&str; 9] = [
        ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tbz", ".tar.xz", ".txz", ".tar.zst", ".tar",
    ];
    const UNSUPPORTED: [&str; 9] = [
        ".gz", ".bz2", ".xz", ".zst", ".7z", ".rar", ".dmg", ".pkg", ".msi",
    ];
    if name.ends_with(".zip") {
        ArchiveKind::Zip
    } else if TAR.iter().any(|extension| name.ends_with(extension)) {
        ArchiveKind::Tar
    } else if UNSUPPORTED
        .iter()
        .any(|extension| name.ends_with(extension))
    {
        ArchiveKind::Unsupported
    } else {
        ArchiveKind::Raw
    }
}

/// 書庫の中の名前が置き場の中に収まるか（絶対パス・ドライブ名・`..` を含む名前があれば展開しない）。
fn check_entries(entries: &[String]) -> Result<(), DeployError> {
    for entry in entries {
        let absolute =
            entry.starts_with('/') || entry.starts_with('\\') || entry.chars().nth(1) == Some(':');
        let escapes = entry.split(['/', '\\']).any(|part| part == "..");
        if absolute || escapes {
            return Err(DeployError::UnsafeArchive(entry.clone()));
        }
    }
    Ok(())
}

/// `tar` の実行ファイル。Windows は OS に付いている bsdtar（zip も読める）を先に使う
/// （Git for Windows の GNU tar は zip を読めない）。
fn tar_program() -> PathBuf {
    if cfg!(windows) {
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            let system_tar = PathBuf::from(system_root).join("System32").join("tar.exe");
            if system_tar.is_file() {
                return system_tar;
            }
        }
    }
    PathBuf::from("tar")
}

/// 展開する（中身の名前を先に確かめてから）。書庫でない実行ファイルは `cmd` の名前で置く。
fn extract(
    archive: &Path,
    url: &str,
    destination: &Path,
    command: &Path,
) -> Result<(), DeployError> {
    // zip を GNU tar は読めないので、Linux だけ unzip を使う（macOS / Windows の tar は bsdtar）。
    let zip_with_unzip = cfg!(target_os = "linux");
    match archive_kind(url) {
        ArchiveKind::Unsupported => Err(DeployError::Extract(format!(
            "この書庫の形は読めない: {url}"
        ))),
        ArchiveKind::Raw => {
            let target = destination.join(command);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(io_error("実行ファイルの置き場を作れない"))?;
            }
            std::fs::copy(archive, &target).map_err(io_error("実行ファイルを置けない"))?;
            Ok(())
        }
        ArchiveKind::Zip if zip_with_unzip => {
            let listing = run(Command::new("unzip").arg("-Z1").arg(archive))?;
            check_entries(&lines(&listing))?;
            run(Command::new("unzip")
                .args(["-q", "-o"])
                .arg(archive)
                .arg("-d")
                .arg(destination))?;
            Ok(())
        }
        ArchiveKind::Tar | ArchiveKind::Zip => {
            let listing = run(Command::new(tar_program()).arg("-tf").arg(archive))?;
            check_entries(&lines(&listing))?;
            run(Command::new(tar_program())
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(destination))?;
            Ok(())
        }
    }
}

fn lines(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// コマンドを走らせて stdout を返す（失敗は stderr を添えて展開の失敗にする）。
fn run(command: &mut Command) -> Result<Vec<u8>, DeployError> {
    let output = command
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| DeployError::Extract(format!("{command:?} を起こせない: {error}")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(DeployError::Extract(format!(
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// 起動するコマンドに実行の権限を付ける（zip では落ちていることがある）。
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), DeployError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = std::fs::metadata(path)
        .map_err(io_error("実行ファイルを読めない"))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(path, permissions).map_err(io_error("実行の権限を付けられない"))
}

/// Windows は拡張子で実行できるかが決まる（権限の bit は無い）ので何もしない。
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), DeployError> {
    Ok(())
}

/// ファイルの sha256（16 進・小文字）。大きな書庫でも読み込みは 64KB ずつ。
fn sha256_file(path: &Path) -> Result<String, DeployError> {
    let mut file = std::fs::File::open(path).map_err(io_error("落とした書庫を開けない"))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(io_error("落とした書庫を読めない"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// `curl` で落とす（https だけ・リダイレクト先も https・止まった転送は 60 秒で諦める）。
fn download_with_curl(url: &str, destination: &Path) -> Result<(), DeployError> {
    if !url.starts_with("https://") {
        return Err(DeployError::InsecureUrl(url.to_string()));
    }
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--speed-limit",
            "1024",
            "--speed-time",
            "60",
            "-H",
            "User-Agent: necoder",
            "-o",
        ])
        .arg(destination)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| DeployError::Download {
            url: url.to_string(),
            reason: format!("curl を起こせない: {error}"),
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeployError::Download {
            url: url.to_string(),
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn io_error(context: &'static str) -> impl Fn(std::io::Error) -> DeployError {
    move |error| DeployError::Io(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 使い捨ての置き場（テストごとに別・終わったら消す）。
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "necoder-deploy-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("置き場を作れる");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("テストの置き場を消せない: {error}");
            }
        }
    }

    fn distribution(archive: &str, cmd: &str, sha256: Option<String>) -> BinaryDistribution {
        BinaryDistribution {
            archive: archive.to_string(),
            cmd: cmd.to_string(),
            args: vec!["acp".to_string()],
            env: Default::default(),
            sha256,
        }
    }

    /// 手元のファイルを「落とした」ことにする（ネットワークへは行かない）。
    fn copy_from(source: PathBuf) -> impl Fn(&str, &Path) -> Result<(), DeployError> {
        move |_url, destination| {
            std::fs::copy(&source, destination)
                .map(|_| ())
                .map_err(|error| DeployError::Io(error.to_string()))
        }
    }

    /// `tar -czf` で書庫を作る（`files` は置き場の中の相対パスと中身）。
    fn tar_gz(scratch: &Scratch, files: &[(&str, &str)]) -> PathBuf {
        let content = scratch.0.join("content");
        for (relative, text) in files {
            let path = content.join(relative);
            std::fs::create_dir_all(path.parent().expect("親")).expect("作れる");
            std::fs::write(&path, text).expect("書ける");
        }
        let archive = scratch.0.join("agent.tar.gz");
        let status = Command::new(tar_program())
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&content)
            .arg(".")
            .status()
            .expect("tar を起こせる");
        assert!(status.success(), "書庫を作れる");
        archive
    }

    /// OS / arch の置き場は id・版・キーで分かれ、名前はフォルダ名に使える物だけ（置き場の外を指さない）。
    #[test]
    fn the_place_is_per_id_version_and_platform() {
        let binary = distribution("https://example.invalid/a.tar.gz", "./amp-acp", None);
        let target = BinaryTarget {
            id: "amp-acp",
            version: "0.9.0",
            platform: "darwin-aarch64",
            distribution: &binary,
        };
        assert_eq!(
            target.directory(Path::new("/root")),
            Ok(PathBuf::from("/root/amp-acp/0.9.0/darwin-aarch64"))
        );
        for bad in ["..", "../x", "a/b", ".hidden", ""] {
            let target = BinaryTarget {
                id: bad,
                version: "0.9.0",
                platform: "darwin-aarch64",
                distribution: &binary,
            };
            assert!(target.directory(Path::new("/root")).is_err(), "{bad:?}");
        }
        assert_eq!(
            command_relative("./dist-package/cursor-agent"),
            Ok(PathBuf::from("dist-package").join("cursor-agent"))
        );
        assert_eq!(
            command_relative("./goose-package\\goose.exe"),
            Ok(PathBuf::from("goose-package").join("goose.exe"))
        );
        assert_eq!(
            command_relative("amp-acp.exe"),
            Ok(PathBuf::from("amp-acp.exe"))
        );
        for bad in ["../evil", "/usr/bin/sh", "C:\\x.exe", "./"] {
            assert!(command_relative(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn archives_are_told_apart_by_their_name() {
        assert_eq!(archive_kind("https://x/a.tar.gz"), ArchiveKind::Tar);
        assert_eq!(archive_kind("https://x/a.tar.bz2"), ArchiveKind::Tar);
        assert_eq!(archive_kind("https://x/a.zip?download=1"), ArchiveKind::Zip);
        assert_eq!(
            archive_kind("https://x/sigit-linux-amd64"),
            ArchiveKind::Raw
        );
        assert_eq!(
            archive_kind("https://x/sigit-win-amd64.exe"),
            ArchiveKind::Raw
        );
        assert_eq!(archive_kind("https://x/agent.gz"), ArchiveKind::Unsupported);
        assert_eq!(
            archive_kind("https://x/agent.dmg"),
            ArchiveKind::Unsupported
        );
    }

    /// 書庫の中の名前が置き場の外を指していたら展開しない。
    #[test]
    fn entries_outside_the_place_are_refused() {
        let ok = vec![
            "./".to_string(),
            "./bin/agent".to_string(),
            "lib/a..b".to_string(),
        ];
        assert_eq!(check_entries(&ok), Ok(()));
        for bad in [
            "../evil",
            "bin/../../evil",
            "/etc/passwd",
            "\\\\server\\x",
            "C:\\evil",
        ] {
            assert_eq!(
                check_entries(&[bad.to_string()]),
                Err(DeployError::UnsafeArchive(bad.to_string())),
                "{bad}"
            );
        }
    }

    /// 検証: レジストリの sha256 と合えば置き、次からは落とし直さない。合わなければ展開せずに捨てる
    /// （置き場に何も残さない）。https でない URL は落とさない。
    #[cfg(unix)]
    #[test]
    fn a_binary_is_verified_extracted_and_reused() {
        let scratch = Scratch::new("verified");
        let archive = tar_gz(&scratch, &[("bin/fake-acp", "#!/bin/sh\necho ok\n")]);
        let sha256 = sha256_file(&archive).expect("sha256 を取れる");
        let root = scratch.0.join("binary");
        let good = distribution(
            "https://example.invalid/fake-acp.tar.gz",
            "./bin/fake-acp",
            Some(sha256.to_ascii_uppercase()),
        );
        let target = BinaryTarget {
            id: "fake-acp",
            version: "1.0.0",
            platform: "darwin-aarch64",
            distribution: &good,
        };
        assert_eq!(target.installed(&root), None, "まだ置いていない");
        let deployed = target
            .deploy_with(&root, copy_from(archive.clone()))
            .expect("置ける");
        assert!(deployed.verified, "大文字の値でも照合できる");
        assert_eq!(deployed.version, "1.0.0");
        assert_eq!(
            deployed.command,
            root.join("fake-acp/1.0.0/darwin-aarch64/bin/fake-acp")
        );
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&deployed.command)
                .expect("在る")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "実行できる");
        }
        assert_eq!(target.installed(&root), Some(deployed.clone()));
        // 置いてあれば落とし直さない（落とし方を呼ばない）。
        let again = target
            .deploy_with(&root, |_url: &str, _destination: &Path| {
                panic!("落とし直さない")
            })
            .expect("置いてある物を使う");
        assert_eq!(again, deployed);

        // 合わない sha256 は展開しない。
        let wrong = distribution(
            "https://example.invalid/fake-acp.tar.gz",
            "./bin/fake-acp",
            Some("00".repeat(32)),
        );
        let mismatched = BinaryTarget {
            id: "fake-acp",
            version: "2.0.0",
            platform: "darwin-aarch64",
            distribution: &wrong,
        };
        match mismatched.deploy_with(&root, copy_from(archive.clone())) {
            Err(DeployError::ChecksumMismatch { expected, actual }) => {
                assert_eq!(expected, "00".repeat(32));
                assert_eq!(actual, sha256);
            }
            other => panic!("照合に落ちる: {other:?}"),
        }
        assert!(!root.join("fake-acp/2.0.0").exists(), "何も置かない");
        let leftovers: Vec<_> = std::fs::read_dir(root.join("fake-acp"))
            .expect("読める")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "途中の物を残さない: {leftovers:?}");
        // 新しい版を落とせなかった時の代わり（手元の一番新しい別の版）。
        assert_eq!(
            latest_other_installed(&root, "fake-acp", "darwin-aarch64", "2.0.0"),
            Some(deployed)
        );

        let insecure = distribution("http://example.invalid/a.tar.gz", "./bin/fake-acp", None);
        let target = BinaryTarget {
            id: "fake-acp",
            version: "3.0.0",
            platform: "darwin-aarch64",
            distribution: &insecure,
        };
        assert_eq!(
            target.deploy_with(&root, copy_from(archive)),
            Err(DeployError::InsecureUrl(
                "http://example.invalid/a.tar.gz".to_string()
            ))
        );
    }

    /// 検証の値が無い物は照合せずに置き、そのことを印に残す。書庫でない実行ファイルは `cmd` の名前で置く。
    /// 新しい版を置くと、1 つ前の版だけ残して古い版を消す。
    #[cfg(unix)]
    #[test]
    fn unverified_raw_binaries_are_marked_and_old_versions_pruned() {
        let scratch = Scratch::new("raw");
        let raw = scratch.0.join("sigit-linux-amd64");
        std::fs::write(&raw, "#!/bin/sh\necho ok\n").expect("書ける");
        let root = scratch.0.join("binary");
        let mut versions = Vec::new();
        for version in ["1.0.0", "1.1.0", "1.2.0"] {
            let binary = distribution(
                "https://example.invalid/sigit-linux-amd64",
                "./sigit-linux-amd64",
                None,
            );
            let target = BinaryTarget {
                id: "sigit",
                version,
                platform: "linux-x86_64",
                distribution: &binary,
            };
            let deployed = target
                .deploy_with(&root, copy_from(raw.clone()))
                .expect("置ける");
            assert!(!deployed.verified, "値が無ければ照合していない");
            assert!(deployed.command.ends_with("sigit-linux-amd64"));
            versions.push(version);
            // 印の時刻で「新しい順」を決めるので、同じ時刻に並ばないよう少し待つ。
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!root.join("sigit/1.0.0").exists(), "2 つ前の版は消える");
        assert!(
            root.join("sigit/1.1.0/linux-x86_64").exists(),
            "1 つ前は残す"
        );
        assert!(root.join("sigit/1.2.0/linux-x86_64").exists());
    }

    /// 本物の確かめ（**ネットワークへ出る・手で流す**）: レジストリの写し（`NECODER_ACP_REGISTRY_CACHE`）
    /// から `NECODER_DEPLOY_PROBE`（既定 `amp-acp`）のこのマシン向けの binary を一時フォルダへ落として
    /// 照合・展開し、HOME も一時フォルダにして `initialize` だけ送る（session もプロンプトも送らない）。
    ///
    /// `NECODER_ACP_REGISTRY_CACHE=… cargo test -p acp_client real_registry_binary -- --ignored --nocapture`
    #[test]
    #[ignore = "ネットワークへ出る（レジストリの binary を落とす）"]
    fn a_real_registry_binary_is_deployed_and_initializes() {
        use std::io::{BufRead as _, Write as _};
        let Some(registry) = crate::registry::load_cached() else {
            eprintln!("レジストリの写しが無い（NECODER_ACP_REGISTRY_CACHE）");
            return;
        };
        let id = std::env::var("NECODER_DEPLOY_PROBE").unwrap_or_else(|_| "amp-acp".to_string());
        let entry = registry.agent(&id).expect("レジストリに在る");
        let Some(crate::registry::Launch::Binary {
            platform,
            distribution,
        }) = entry.launch()
        else {
            panic!("{id}: このマシン向けの binary が無い");
        };
        let scratch = Scratch::new("real");
        let target = BinaryTarget {
            id: &entry.id,
            version: &entry.version,
            platform: &platform,
            distribution: &distribution,
        };
        let started = std::time::Instant::now();
        let deployed = target.deploy(&scratch.0.join("binary")).expect("置ける");
        println!(
            "  置いた: {} ({:.1}s・照合 {})",
            deployed.command.display(),
            started.elapsed().as_secs_f32(),
            deployed.verified
        );
        let home = scratch.0.join("home");
        std::fs::create_dir_all(&home).expect("HOME を作れる");
        let mut child = Command::new(&deployed.command)
            .args(&distribution.args)
            .envs(&distribution.env)
            .env("HOME", &home)
            .current_dir(&home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("起こせる");
        let mut stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":1,"clientCapabilities":{{}}}}}}"#
        )
        .expect("送れる");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = std::io::BufReader::new(stdout).read_line(&mut line);
            if sender.send(read.map(|_| line)).is_err() {
                eprintln!("応答の受け手がもういない");
            }
        });
        let answer = receiver.recv_timeout(std::time::Duration::from_secs(30));
        if let Err(error) = child.kill() {
            eprintln!("止められない: {error}");
        }
        if let Err(error) = child.wait() {
            eprintln!("待てない: {error}");
        }
        let answer = answer.expect("30 秒以内に答える").expect("読める");
        println!("  initialize の応答: {}", answer.trim());
        let value: serde_json::Value = serde_json::from_str(answer.trim()).expect("JSON-RPC");
        assert!(
            value.get("result").is_some(),
            "initialize に結果が返る: {answer}"
        );
    }

    /// 本番の確かめ: 偽のエージェント（python の ACP サーバを起こすシェル）を書庫にして置き、置いた
    /// コマンドで `initialize` → `session/new` まで通す（プロンプトは送らない・ネットワークへは行かない）。
    #[cfg(unix)]
    #[test]
    fn a_deployed_binary_opens_a_session() {
        let Some(python) = crate::find_in_path("python3") else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let scratch = Scratch::new("session");
        let agent = format!(
            "#!/bin/sh\nexec '{}' -c '{}' \"$@\"\n",
            python.display(),
            r#"
import json, sys
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    rid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": 1}}), flush=True)
    elif method == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "binary-1"}}), flush=True)
"#
        );
        let archive = tar_gz(&scratch, &[("fake-acp", agent.as_str())]);
        let binary = distribution(
            "https://example.invalid/fake-acp.tar.gz",
            "./fake-acp",
            Some(sha256_file(&archive).expect("sha256")),
        );
        let target = BinaryTarget {
            id: "fake-acp",
            version: "1.0.0",
            platform: "darwin-aarch64",
            distribution: &binary,
        };
        let deployed = target
            .deploy_with(&scratch.0.join("binary"), copy_from(archive))
            .expect("置ける");
        let command = crate::AgentCommand::new(deployed.command, Vec::new(), std::env::temp_dir());
        let opened = futures::executor::block_on(crate::probe_session(
            &command,
            std::time::Duration::from_secs(10),
        ));
        assert!(opened, "置いたコマンドで initialize → session/new まで通る");
    }
}
