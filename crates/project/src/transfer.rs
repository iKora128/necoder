//! 手元と接続先の間のファイルの受け渡し（O37・G09）。SSH の project のエクスプローラへ Finder から
//! 落とす（アップロード）・接続先のファイル / フォルダを手元へ保存する（ダウンロード）。
//!
//! どちらも [`Host`] の `metadata` / `read_dir` / `read_file` / `write_file` だけで組む（接続先の
//! daemon に新しい request を足さない）。blocking なので呼び出し側は background executor で回す。
//!
//! - **上書きしない**: 送る先に同じ名前があれば断る（Finder からのコピーと同じ）。ダウンロードで
//!   ファイルを 1 つ保存する時だけは、保存ダイアログが上書きを確かめるのでそれに従う。
//! - **シンボリックリンクは辿らない**（フォルダの中のものは数えて飛ばす）。ループと、フォルダの外への
//!   逸脱を避ける。落とした（選んだ）もの自体はリンクでも中身を送る（人が選んだものなので）。
//! - **送る前に数える**: フォルダは先に全部たどって、数と大きさが上限を超えるなら 1 本も送らずに断る
//!   （途中まで送って止まった、を作らない）。

use anyhow::{bail, Context as _, Result};
use host::{CommandSpec, Host, WriteCondition};
use std::path::{Path, PathBuf};

/// 1 回に送る（受け取る）ファイルの数の上限。
pub const MAX_TRANSFER_FILES: usize = 10_000;
/// 1 本の大きさの上限（接続先の daemon の 1 通の上限より小さく）。
pub const MAX_TRANSFER_FILE_BYTES: u64 = 128 * 1024 * 1024;

/// 送った（受け取った）もの。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferSummary {
    /// ファイルの数。
    pub files: usize,
    /// バイト数の合計。
    pub bytes: u64,
    /// 飛ばしたもの（フォルダの中のシンボリックリンク・特殊ファイル）。
    pub skipped: usize,
}

impl TransferSummary {
    fn add(&mut self, other: &TransferSummary) {
        self.files += other.files;
        self.bytes += other.bytes;
        self.skipped += other.skipped;
    }
}

/// 複数を送った・受け取った時の合計。
pub fn total_of<'a>(summaries: impl IntoIterator<Item = &'a TransferSummary>) -> TransferSummary {
    let mut total = TransferSummary::default();
    for summary in summaries {
        total.add(summary);
    }
    total
}

/// 手元の `source`（ファイル / フォルダ）を `host` の `target_dir` の中へ送る（アップロード）。
/// 送った先のパスを返す。送る先に同じ名前があれば断る。
pub fn upload_into_on(
    host: &dyn Host,
    source: &Path,
    target_dir: &Path,
) -> Result<(PathBuf, TransferSummary)> {
    let name = source
        .file_name()
        .with_context(|| format!("名前が取れない: {}", source.display()))?;
    let destination = target_dir.join(name);
    if host.metadata(&destination).is_ok() {
        bail!("既に存在する: {}", destination.display());
    }
    let metadata =
        std::fs::metadata(source).with_context(|| format!("読めない: {}", source.display()))?;
    if !metadata.is_dir() {
        if !metadata.is_file() {
            bail!("送れない種類のファイル: {}", source.display());
        }
        let bytes = read_local_file(source, metadata.len())?;
        let summary = TransferSummary {
            files: 1,
            bytes: bytes.len() as u64,
            skipped: 0,
        };
        host.write_file(&destination, &bytes, WriteCondition::NotExists)
            .with_context(|| format!("送れない: {}", destination.display()))?;
        return Ok((destination, summary));
    }

    let plan = plan_local_folder(source)?;
    let mut summary = TransferSummary {
        skipped: plan.skipped,
        ..TransferSummary::default()
    };
    for (relative, length) in &plan.files {
        let bytes = read_local_file(&source.join(relative), *length)?;
        let target = destination.join(relative);
        host.write_file(&target, &bytes, WriteCondition::NotExists)
            .with_context(|| format!("送れない: {}", target.display()))?;
        summary.files += 1;
        summary.bytes += bytes.len() as u64;
    }
    // ファイルを書けば親のフォルダはできる。できないのは空のフォルダだけなので、それだけ作る
    // （接続先は常に POSIX。POSIX のシェルが無い host では空のフォルダは送らない）。
    let empty: Vec<PathBuf> = plan
        .empty_directories
        .iter()
        .map(|relative| join_relative(&destination, relative))
        .collect();
    if !empty.is_empty() && host.has_posix_shell() {
        let mut arguments = vec!["-p".to_string(), "--".to_string()];
        arguments.extend(empty.iter().map(|path| path.to_string_lossy().to_string()));
        let output = host
            .run_command(&CommandSpec::new("mkdir", target_dir).args(arguments))
            .context("空のフォルダを作れない")?;
        if !output.success() {
            bail!(
                "空のフォルダを作れない: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok((destination, summary))
}

/// `host` の `source`（ファイル / フォルダ）を手元の `target` へ保存する（ダウンロード）。
/// `target` は保存した後の名前そのもの（フォルダなら中身がその下に入る）。フォルダは `target` が
/// 既にあれば断る。ファイルは保存ダイアログが上書きを確かめた後なので上書きする。
pub fn download_to_on(host: &dyn Host, source: &Path, target: &Path) -> Result<TransferSummary> {
    let metadata = host
        .metadata(source)
        .with_context(|| format!("読めない: {}", source.display()))?;
    if !metadata.is_dir {
        if metadata.len > MAX_TRANSFER_FILE_BYTES {
            bail!("大きすぎる: {}", source.display());
        }
        let content = host
            .read_file(source)
            .with_context(|| format!("受け取れない: {}", source.display()))?;
        if content.bytes.len() as u64 > MAX_TRANSFER_FILE_BYTES {
            bail!("大きすぎる: {}", source.display());
        }
        write_local_file(target, &content.bytes)?;
        return Ok(TransferSummary {
            files: 1,
            bytes: content.bytes.len() as u64,
            skipped: 0,
        });
    }

    if target.symlink_metadata().is_ok() {
        bail!("既に存在する: {}", target.display());
    }
    let plan = plan_remote_folder(host, source)?;
    std::fs::create_dir_all(target).with_context(|| format!("作れない: {}", target.display()))?;
    for relative in &plan.empty_directories {
        let directory = join_relative(target, relative);
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("作れない: {}", directory.display()))?;
    }
    let mut summary = TransferSummary {
        skipped: plan.skipped,
        ..TransferSummary::default()
    };
    for (relative, _) in &plan.files {
        let content = host
            .read_file(&source.join(relative))
            .with_context(|| format!("受け取れない: {}", source.join(relative).display()))?;
        if content.bytes.len() as u64 > MAX_TRANSFER_FILE_BYTES {
            bail!("大きすぎる: {}", source.join(relative).display());
        }
        write_local_file(&target.join(relative), &content.bytes)?;
        summary.files += 1;
        summary.bytes += content.bytes.len() as u64;
    }
    Ok(summary)
}

/// フォルダをたどった結果（相対パス）。
#[derive(Debug, Default)]
struct FolderPlan {
    /// 送るファイルと大きさ（接続先は大きさが分からないので 0）。
    files: Vec<(PathBuf, u64)>,
    /// 中に何も無いフォルダ（ファイルを書いても親としてできないもの・空の相対パス = 根そのもの）。
    empty_directories: Vec<PathBuf>,
    skipped: usize,
}

impl FolderPlan {
    fn push_file(&mut self, relative: PathBuf, length: u64, root: &Path) -> Result<()> {
        if self.files.len() >= MAX_TRANSFER_FILES {
            bail!(
                "ファイルが多すぎる（{} 件まで）: {}",
                MAX_TRANSFER_FILES,
                root.display()
            );
        }
        if length > MAX_TRANSFER_FILE_BYTES {
            bail!("大きすぎる: {}", root.join(&relative).display());
        }
        self.files.push((relative, length));
        Ok(())
    }
}

fn plan_local_folder(root: &Path) -> Result<FolderPlan> {
    let mut plan = FolderPlan::default();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let directory = root.join(&relative);
        let entries = std::fs::read_dir(&directory)
            .with_context(|| format!("読めない: {}", directory.display()))?;
        let mut is_empty = true;
        for entry in entries {
            let entry = entry?;
            is_empty = false;
            let child = relative.join(entry.file_name());
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(child);
            } else if file_type.is_file() {
                let length = entry.metadata()?.len();
                plan.push_file(child, length, root)?;
            } else {
                plan.skipped += 1;
            }
        }
        if is_empty {
            plan.empty_directories.push(relative);
        }
    }
    plan.files.sort();
    Ok(plan)
}

fn plan_remote_folder(host: &dyn Host, root: &Path) -> Result<FolderPlan> {
    let mut plan = FolderPlan::default();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let directory = root.join(&relative);
        let entries = host
            .read_dir(&directory)
            .with_context(|| format!("読めない: {}", directory.display()))?;
        if entries.is_empty() {
            plan.empty_directories.push(relative.clone());
        }
        for entry in entries {
            // 名前は接続先から来る。`..` や絶対パスで保存先の外へ書かせない（手元で使えない名前も飛ばす）。
            if entry.is_symlink || !is_plain_name(&entry.name) {
                plan.skipped += 1;
                continue;
            }
            let child = relative.join(&entry.name);
            if entry.is_dir {
                pending.push(child);
            } else {
                plan.push_file(child, 0, root)?;
            }
        }
    }
    plan.files.sort();
    Ok(plan)
}

/// 1 つの名前として手元で使えるか（区切り・`..`・ドライブ名などを含まない）。
fn is_plain_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// `base` の下の `relative`（空なら `base` そのもの。`join("")` は末尾に区切りを足すので避ける）。
fn join_relative(base: &Path, relative: &Path) -> PathBuf {
    if relative.as_os_str().is_empty() {
        base.to_path_buf()
    } else {
        base.join(relative)
    }
}

fn read_local_file(path: &Path, length: u64) -> Result<Vec<u8>> {
    if length > MAX_TRANSFER_FILE_BYTES {
        bail!("大きすぎる: {}", path.display());
    }
    std::fs::read(path).with_context(|| format!("読めない: {}", path.display()))
}

fn write_local_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("作れない: {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("書けない: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::LocalHost;

    /// 手元と「接続先」（どちらも一時フォルダ・host は LocalHost）。
    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("necoder_transfer_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let local = base.join("local");
        let remote = base.join("remote");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        (local, remote)
    }

    fn cleanup(local: &Path) {
        if let Some(base) = local.parent() {
            let _ = std::fs::remove_dir_all(base);
        }
    }

    #[test]
    fn a_file_is_uploaded_once_and_never_overwritten() {
        let (local, remote) = scratch("upload_file");
        let host = LocalHost::shared();
        std::fs::write(local.join("notes.txt"), "hello").unwrap();

        let (destination, summary) =
            upload_into_on(host.as_ref(), &local.join("notes.txt"), &remote).unwrap();
        assert_eq!(destination, remote.join("notes.txt"));
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "hello");
        assert_eq!(
            summary,
            TransferSummary {
                files: 1,
                bytes: 5,
                skipped: 0
            }
        );

        std::fs::write(local.join("notes.txt"), "changed").unwrap();
        let error = upload_into_on(host.as_ref(), &local.join("notes.txt"), &remote)
            .expect_err("同じ名前があれば断る");
        assert!(format!("{error:#}").contains("既に存在する"), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "hello",
            "上書きしない"
        );
        cleanup(&local);
    }

    #[test]
    fn a_folder_is_uploaded_with_its_tree() {
        let (local, remote) = scratch("upload_folder");
        let host = LocalHost::shared();
        let folder = local.join("site");
        std::fs::create_dir_all(folder.join("css")).unwrap();
        std::fs::create_dir_all(folder.join("empty")).unwrap();
        std::fs::write(folder.join("index.html"), "<p>").unwrap();
        std::fs::write(folder.join("css/main.css"), "p{}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(folder.join("index.html"), folder.join("link.html")).unwrap();

        let (destination, summary) = upload_into_on(host.as_ref(), &folder, &remote).unwrap();
        assert_eq!(destination, remote.join("site"));
        assert_eq!(
            std::fs::read_to_string(destination.join("css/main.css")).unwrap(),
            "p{}"
        );
        assert_eq!(summary.files, 2);
        assert_eq!(summary.bytes, 6);
        if host.has_posix_shell() {
            assert!(destination.join("empty").is_dir(), "空のフォルダも作る");
        }
        #[cfg(unix)]
        {
            assert_eq!(summary.skipped, 1, "中のリンクは辿らない");
            assert!(destination.join("link.html").symlink_metadata().is_err());
        }

        std::fs::create_dir_all(local.join("blank")).unwrap();
        let (blank, summary) =
            upload_into_on(host.as_ref(), &local.join("blank"), &remote).unwrap();
        assert_eq!(summary.files, 0);
        if host.has_posix_shell() {
            assert!(
                blank.is_dir(),
                "空のフォルダを落としても、そのフォルダはできる"
            );
        }
        cleanup(&local);
    }

    #[test]
    fn files_and_folders_are_downloaded() {
        let (local, remote) = scratch("download");
        let host = LocalHost::shared();
        std::fs::create_dir_all(remote.join("logs/old")).unwrap();
        std::fs::create_dir_all(remote.join("logs/none")).unwrap();
        std::fs::write(remote.join("logs/app.log"), "line\n").unwrap();
        std::fs::write(remote.join("logs/old/1.log"), "x").unwrap();

        let saved = local.join("app copy.log");
        let summary = download_to_on(host.as_ref(), &remote.join("logs/app.log"), &saved).unwrap();
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "line\n");
        assert_eq!(summary.files, 1);
        std::fs::write(remote.join("logs/app.log"), "newer\n").unwrap();
        download_to_on(host.as_ref(), &remote.join("logs/app.log"), &saved)
            .expect("ファイルは保存ダイアログが確かめたので上書きする");
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "newer\n");

        let folder = local.join("logs");
        let summary = download_to_on(host.as_ref(), &remote.join("logs"), &folder).unwrap();
        assert_eq!(summary.files, 2);
        assert_eq!(
            std::fs::read_to_string(folder.join("old/1.log")).unwrap(),
            "x"
        );
        assert!(folder.join("none").is_dir(), "空のフォルダも作る");
        let error = download_to_on(host.as_ref(), &remote.join("logs"), &folder)
            .expect_err("フォルダは同じ名前があれば断る");
        assert!(format!("{error:#}").contains("既に存在する"), "{error:#}");
        cleanup(&local);
    }

    #[test]
    fn too_many_files_are_refused_before_sending_any() {
        let mut plan = FolderPlan::default();
        let root = Path::new("/project");
        for index in 0..MAX_TRANSFER_FILES {
            plan.push_file(PathBuf::from(format!("{index}")), 1, root)
                .unwrap();
        }
        let error = plan
            .push_file(PathBuf::from("one-more"), 1, root)
            .expect_err("上限を超えたら断る");
        assert!(format!("{error:#}").contains("多すぎる"), "{error:#}");
        let error = FolderPlan::default()
            .push_file(PathBuf::from("big"), MAX_TRANSFER_FILE_BYTES + 1, root)
            .expect_err("大きすぎるファイルは断る");
        assert!(format!("{error:#}").contains("大きすぎる"), "{error:#}");
    }

    #[test]
    fn names_from_the_remote_cannot_leave_the_chosen_folder() {
        assert!(is_plain_name("app.log"));
        assert!(is_plain_name(".env"));
        for name in ["", ".", "..", "../x", "a/b", "/etc/passwd"] {
            assert!(!is_plain_name(name), "{name:?}");
        }
    }

    #[test]
    fn totals_add_up() {
        let total = total_of(&[
            TransferSummary {
                files: 1,
                bytes: 10,
                skipped: 0,
            },
            TransferSummary {
                files: 2,
                bytes: 5,
                skipped: 1,
            },
        ]);
        assert_eq!(
            total,
            TransferSummary {
                files: 3,
                bytes: 15,
                skipped: 1
            }
        );
    }
}
