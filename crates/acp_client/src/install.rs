//! ローカル npm アダプタの導入と起動を分離する。Node は PATH から借りる（無ければ設定の「インストール」が
//! necoder の道具箱へ入れる・`setup`）。完了した版別ディレクトリだけを公開し、起動中の版を書き換えない。
//!
//! Windows でも同じ経路を使う（2026-10-19 まで Windows は npx 任せで、最初のスレッドが毎回 npx の解決を
//! 待っていた）。npm は Node.js に同梱の `npm-cli.js` を node で直接起こす（[`Npm`]）。
use super::*;
use std::ffi::OsString;

fn directory(root: &Path, package: &str) -> PathBuf {
    // '%' は許可するパッケージ名に含まれないため scope 区切りの置換は衝突しない。
    root.join(package.replace('/', "%2F"))
}

/// npm を起こす形（プログラムと、その頭に付ける引数）。
///
/// Windows は **Node.js に同梱の `npm-cli.js` を node で直接起こす**。`npm.cmd` を経由すると、
/// cmd の引数の解釈（`<` `>` `^` `&`）と PowerShell の実行ポリシー（`npm.ps1`）で事故が多い。
/// mac / Linux は従来どおり PATH の `npm`（無ければ同じく node + `npm-cli.js`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Npm {
    program: PathBuf,
    leading: Vec<OsString>,
}

impl Npm {
    /// `node` の横にある npm を探す。
    pub(crate) fn find(node: &Path) -> Option<Self> {
        let beside = || {
            npm_cli_beside(node).map(|script| Self {
                program: node.to_path_buf(),
                leading: vec![script.into_os_string()],
            })
        };
        let on_path = || {
            find_in_path("npm").map(|program| Self {
                program,
                leading: Vec::new(),
            })
        };
        if cfg!(windows) {
            beside().or_else(on_path)
        } else {
            on_path().or_else(beside)
        }
    }

    /// 頭の引数まで付けた `Command`（コンソール窓は出さない）。
    pub(crate) fn command(&self) -> std::process::Command {
        let mut command = child_process::command(&self.program);
        command.args(&self.leading);
        command
    }

    #[cfg(all(test, unix))]
    fn program(program: &Path) -> Self {
        Self {
            program: program.to_path_buf(),
            leading: Vec::new(),
        }
    }
}

/// Node.js の配布物で `node` の横にある `npm-cli.js`（Windows は `<dir>\node_modules\npm`・
/// mac / Linux は `<dir>/../lib/node_modules/npm`）。symlink（Homebrew・nvm）は辿ってから探す。
fn npm_cli_beside(node: &Path) -> Option<PathBuf> {
    let node = paths::canonicalize(node).unwrap_or_else(|_| node.to_path_buf());
    let directory = node.parent()?;
    let mut candidates = vec![directory.join("node_modules/npm/bin/npm-cli.js")];
    if let Some(parent) = directory.parent() {
        candidates.push(parent.join("lib/node_modules/npm/bin/npm-cli.js"));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// 導入・掃除を 1 本ずつにする（同じアプリで複数スレッドが初回起動しても npm install は一度だけ。
/// 掃除が導入したばかりの版を消さない）。
static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn install_guard() -> Result<std::sync::MutexGuard<'static, ()>> {
    INSTALL
        .lock()
        .map_err(|_| anyhow::anyhow!("導入ロックが壊れている"))
}

/// アダプタを（まだ無ければ）necoder の置き場へ入れておく（設定の「インストール」が、最初のスレッドを
/// 待たせないために先に呼ぶ）。`package` は版まで固定した名前（`@scope/name@1.2.3`）。
pub(crate) fn preinstall(
    package: &str,
    bin: &str,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    preinstall_leased(package, bin, environment).map(|(entry, _lease)| entry)
}

/// [`preinstall`] して、その版の「使用中」の印（[`VersionLease`]）も取る（導入の直後に掃除で消されない）。
fn preinstall_leased(
    package: &str,
    bin: &str,
    environment: &BTreeMap<String, String>,
) -> Result<(PathBuf, VersionLease)> {
    let root = paths::external_agents_dir()
        .context("導入先がない")?
        .join("npm");
    anyhow::ensure!(
        exact_package(package).is_some(),
        "版が固定されていない: {package}"
    );
    let directory = directory(&root, package);
    let _guard = install_guard()?;
    if installed_entry(&directory, package, bin).is_none() {
        let node = find_in_path("node").context("node がない")?;
        let npm = Npm::find(&node).context("npm がない")?;
        install_package(&root, &directory, package, bin, &npm, environment)?;
    }
    let lease = VersionLease::take(&directory).context("この版は消している最中")?;
    let entry = installed_entry(&directory, package, bin).context("導入結果がない")?;
    Ok((entry, lease))
}

/// 版別ディレクトリの「使用中」の印。これを持つ [`AgentCommand`] が生きている間（＝そのコマンドで
/// 起こしたプロセスが動いている間）、古い版の掃除はその版を消さない。
///
/// 印は置き場の `<版のディレクトリ名>.lock` への**共有ロック**（OS のファイルロック・別の necoder の
/// プロセスにも効く・落ちれば OS が外す）。掃除は**排他ロック**が取れた版だけを消す。版の中に置かない
/// のは、Windows では開いているファイルを含むディレクトリを動かせないため。
///
/// アダプタは起動後も自分の版のファイルを使う。codex-acp は `initialize` で Codex 本体を、会話中も
/// 同梱の `rg` などを版のディレクトリから起こす。2026-11-02 に codex-acp が 2.5.0 へ上がった時、
/// 導入の後始末が前の版を起動途中のまま消し、Codex のスレッドが「起動直後に終了」した。
#[derive(Debug)]
pub(crate) struct VersionLease {
    /// `None` = このファイルシステムではロックを取れない（印なしで使う・掃除も消さない側に倒れる）。
    _lock: Option<std::fs::File>,
}

impl VersionLease {
    /// `directory` の版に印を付ける。掃除が消している最中（排他ロック中）なら `None`。
    fn take(directory: &Path) -> Option<Self> {
        let file = match open_lock(directory) {
            Ok(file) => file,
            Err(error) => {
                eprintln!(
                    "ACP アダプタの使用中の印を作れない（{}）: {error}",
                    directory.display()
                );
                return Some(Self { _lock: None });
            }
        };
        match file.try_lock_shared() {
            Ok(()) => Some(Self { _lock: Some(file) }),
            Err(std::fs::TryLockError::WouldBlock) => None,
            Err(std::fs::TryLockError::Error(error)) => {
                eprintln!(
                    "ACP アダプタの使用中の印を付けられない（{}）: {error}",
                    directory.display()
                );
                Some(Self { _lock: None })
            }
        }
    }
}

/// 版の「使用中」の印のファイル（`<置き場>/<版のディレクトリ名>.lock`）。
fn lock_path(directory: &Path) -> PathBuf {
    let mut name = directory.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    directory.with_file_name(name)
}

fn open_lock(directory: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(directory))
}

pub(super) fn cached_command(
    package: &str,
    bin: &str,
    args: Vec<String>,
    cwd: PathBuf,
) -> Option<AgentCommand> {
    let root = paths::external_agents_dir()?.join("npm");
    let node = find_in_path("node")?;
    let directory = directory(&root, package);
    installed_entry(&directory, package, bin)?;
    // 印を付けてから確かめ直す（付ける前に掃除が消し終えていたら使わない）。
    let lease = VersionLease::take(&directory)?;
    let entry = installed_entry(&directory, package, bin)?;
    let mut direct_args = vec![entry.to_string_lossy().into_owned()];
    direct_args.extend(args);
    let mut command = AgentCommand::new(node, direct_args, cwd);
    command.lease = Some(lease);
    Some(command)
}

pub(super) fn prepare(mut command: AgentCommand) -> AgentCommand {
    let Some((package, bin)) = command.managed_npm.take() else {
        return command;
    };
    // 非固定版は従来の起動経路（npx）を使う。
    if exact_package(&package).is_none() {
        return command;
    }
    // 同じアプリで複数スレッドが初回起動しても npm install は一度だけ（preinstall のロック）。
    let prepared = (|| -> Result<(PathBuf, PathBuf, VersionLease)> {
        let node = find_in_path("node").context("node がない")?;
        let (entry, lease) = preinstall_leased(&package, &bin, &command.env)?;
        Ok((node, entry, lease))
    })();
    match prepared {
        Ok((node, entry, lease)) => {
            // 元の -y と npm spec を除き、エージェント自身の引数だけを保つ。
            let extra = command.args.split_off(2);
            command.path = node;
            command.args = vec![entry.to_string_lossy().into_owned()];
            command.args.extend(extra);
            command.lease = Some(lease);
        }
        Err(error) => eprintln!("ACP アダプタの管理導入に失敗、npx で起動: {error:#}"),
    }
    command
}

fn install_package(
    root: &Path,
    directory: &Path,
    package: &str,
    bin: &str,
    npm: &Npm,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    std::fs::create_dir_all(&root)?;
    let staging = tempfile::Builder::new()
        .prefix("install-")
        .tempdir_in(&root)?;
    std::fs::write(staging.path().join("package.json"), r#"{"private":true}"#)?;
    let mut child = npm
        .command()
        .args([
            "install",
            "--global=false",
            "--save-exact",
            "--no-audit",
            "--no-fund",
        ])
        .arg("--prefix")
        .arg(staging.path())
        .args(["--", package])
        .current_dir(staging.path())
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            outcome => {
                let _killed = child.kill();
                let _waited = child.wait();
                anyhow::bail!("npm install が完了しない: {outcome:?}");
            }
        }
    };
    anyhow::ensure!(status.success(), "npm install: {status}");
    anyhow::ensure!(
        installed_entry(staging.path(), &package, &bin).is_some(),
        "導入した実行ファイルを検証できない"
    );
    if let Err(error) = std::fs::rename(staging.path(), &directory) {
        // 別 necoder プロセスが先に同じ版を公開した場合だけ、その完成品を使う。
        anyhow::ensure!(
            installed_entry(&directory, &package, &bin).is_some(),
            "導入先を確定できない: {error}"
        );
    }
    let entry = installed_entry(&directory, &package, &bin).context("導入結果がない")?;
    remove_stale_versions(root, package, directory);
    Ok(entry)
}

/// 同じパッケージの**古い版**を消す。アダプタは 1 版 260 MB あり、レジストリが版を上げるたびに
/// 積もるため、新しい版を公開できた時点で前の版を畳む。
///
/// **使用中の版（[`VersionLease`] を誰かが持つ）は消さない**。アダプタは起動後も自分の版のファイルを
/// 使うので、動いているセッションの版を消すと起動途中なら落ち、会話中なら道具が壊れる。残した版は、
/// 次の導入か起動時の掃除（[`sweep_stale_versions`]）が使われなくなった後に消す。
fn remove_stale_versions(root: &Path, package: &str, keep: &Path) {
    let Some((name, _)) = exact_package(package) else {
        return;
    };
    let prefix = format!("{}@", name.replace('/', "%2F"));
    for stale in version_directories(root) {
        let matches_package = stale
            .file_name()
            .is_some_and(|file_name| file_name.to_string_lossy().starts_with(&prefix));
        if stale != keep && matches_package {
            remove_version(&stale);
        }
    }
    remove_leftovers(root);
}

/// 置き場の掃除（起動時に背景で呼ぶ・**blocking**）。`keep`（今の解決が使う、版まで固定したパッケージ名）
/// に無い版を、使用中でなければ消す。導入の後始末で使用中だったため残した版と、途中で止まった削除の跡も
/// ここで片付く。導入と同じロックの中で回す（導入したばかりの版を、古い一覧で消さない）。
pub(crate) fn sweep_stale_versions(keep: &[String]) {
    let Some(root) = paths::external_agents_dir().map(|dir| dir.join("npm")) else {
        return;
    };
    let _guard = match install_guard() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("ACP アダプタの置き場を掃除できない: {error:#}");
            return;
        }
    };
    sweep_stale_versions_in(&root, keep);
}

fn sweep_stale_versions_in(root: &Path, keep: &[String]) {
    let keep: std::collections::HashSet<PathBuf> = keep
        .iter()
        .filter(|package| exact_package(package).is_some())
        .map(|package| directory(root, package))
        .collect();
    for version in version_directories(root) {
        if !keep.contains(&version) {
            remove_version(&version);
        }
    }
    remove_leftovers(root);
}

/// 置き場の版別ディレクトリ（`<名前>@<版>` の形のディレクトリだけ。導入途中の `install-*`・削除途中の
/// `.removing-*`・印のファイルは含めない）。
fn version_directories(root: &Path) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("ACP アダプタの置き場を読めない: {error}");
            }
            return Vec::new();
        }
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy().replace("%2F", "/");
                exact_package(&name).is_some()
            })
        })
        .collect()
}

/// 使われていない版を 1 つ消す。使用中（誰かが印を持つ）・印を確かめられない時は残して `false`。
///
/// 排他ロックを持ったまま版を脇（`.removing-*`）へ退けてから消す。退けた後に起こそうとした人は、印を
/// 付けてから版を確かめ直すので、無くなった版を使わない（[`VersionLease`]）。
fn remove_version(directory: &Path) -> bool {
    let lock = match open_lock(directory) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!(
                "古い ACP アダプタの使用中の印を確かめられない（{}）: {error}",
                directory.display()
            );
            return false;
        }
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return false,
        Err(std::fs::TryLockError::Error(error)) => {
            eprintln!(
                "古い ACP アダプタの使用中の印を確かめられない（{}）: {error}",
                directory.display()
            );
            return false;
        }
    }
    let name = directory
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let removing = directory.with_file_name(format!(".removing-{}-{name}", std::process::id()));
    if let Err(error) = std::fs::rename(directory, &removing) {
        eprintln!(
            "古い ACP アダプタを退けられない（{}）: {error}",
            directory.display()
        );
        return false;
    }
    drop(lock);
    remove_lock_file(&lock_path(directory));
    if let Err(error) = std::fs::remove_dir_all(&removing) {
        // 退けた跡は次の掃除（[`remove_leftovers`]）が消す。誰も起こさない名前なので害は無い。
        eprintln!(
            "古い ACP アダプタを消せない（{}）: {error}",
            removing.display()
        );
    }
    true
}

/// 途中で止まった削除の跡（`.removing-*`）と、版の無い印のファイルを消す。
fn remove_leftovers(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".removing-") {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                eprintln!(
                    "ACP アダプタの削除の跡を消せない（{}）: {error}",
                    path.display()
                );
            }
        } else if let Some(version) = name.strip_suffix(".lock") {
            if root.join(version).exists() {
                continue;
            }
            // 版の無い印でも、いま誰かが掴んでいる（版を確かめている最中）なら触らない。
            let Ok(lock) = std::fs::OpenOptions::new().write(true).open(&path) else {
                continue;
            };
            if lock.try_lock().is_ok() {
                drop(lock);
                remove_lock_file(&path);
            }
        }
    }
}

fn remove_lock_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "ACP アダプタの使用中の印を消せない（{}）: {error}",
                path.display()
            );
        }
    }
}

/// necoder の置き場（`external_agents/npm`）に入れてある `name` の版（読むだけ・設定の AI エージェントの
/// ページが「今の版」に使う）。完成した版（package.json の名前と版が置き場の名前と合う物）だけ。
pub(super) fn installed_versions(name: &str) -> Vec<String> {
    let Some(root) = paths::external_agents_dir().map(|dir| dir.join("npm")) else {
        return Vec::new();
    };
    installed_versions_in(&root, name)
}

/// [`installed_versions`] の本体（置き場を渡せる）。
fn installed_versions_in(root: &Path, name: &str) -> Vec<String> {
    let prefix = format!("{}@", name.replace('/', "%2F"));
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let directory = entry.file_name().to_string_lossy().into_owned();
            let version = directory.strip_prefix(&prefix)?;
            let manifest = entry
                .path()
                .join("node_modules")
                .join(name)
                .join("package.json");
            let manifest: serde_json::Value =
                serde_json::from_reader(std::fs::File::open(manifest).ok()?).ok()?;
            (manifest.get("name")?.as_str()? == name
                && manifest.get("version")?.as_str()? == version)
                .then(|| version.to_string())
        })
        .collect()
}

fn exact_package(package: &str) -> Option<(&str, &str)> {
    let (name, version) = package.rsplit_once('@')?;
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    };
    let valid_name = if let Some(scoped) = name.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(scope, name)| valid_part(scope) && valid_part(name))
    } else {
        valid_part(name)
    };
    let components: Vec<_> = version.split('.').collect();
    (valid_name
        && components.len() == 3
        && components
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit())))
    .then_some((name, version))
}

fn installed_entry(directory: &Path, package: &str, bin: &str) -> Option<PathBuf> {
    let (name, version) = exact_package(package)?;
    let package_dir = directory.join("node_modules").join(name);
    let manifest: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(package_dir.join("package.json")).ok()?)
            .ok()?;
    if manifest.get("name")?.as_str()? != name || manifest.get("version")?.as_str()? != version {
        return None;
    }
    let bins = manifest.get("bin")?;
    let relative = bins.as_str().or_else(|| bins.get(bin)?.as_str())?;
    // paths::canonicalize は Windows の verbatim 形（`\\?\C:\…`）を剥がす＝node に渡す道が普通の形になる。
    let entry = paths::canonicalize(&package_dir.join(relative)).ok()?;
    if !entry.starts_with(paths::canonicalize(&package_dir).ok()?) {
        return None;
    }
    // node に渡せる JS エントリだけを対象にする（ネイティブ CLI を誤実行しない）。
    matches!(entry.extension()?.to_str()?, "js" | "mjs" | "cjs").then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn install_publishes_only_a_complete_adapter() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let npm = root.path().join("npm");
        std::fs::write(&npm, r##"#!/bin/sh
set -eu
test "$1" = install
test "$2" = --global=false
test "$3" = --save-exact
mkdir -p node_modules/adapter
printf '%s' '{"name":"adapter","version":"1.2.3","bin":{"adapter":"index.js"}}' > node_modules/adapter/package.json
printf '%s' 'console.log("adapter")' > node_modules/adapter/index.js
"##).unwrap();
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700)).unwrap();
        let destination = root.path().join("adapter@1.2.3");
        let entry = install_package(
            root.path(),
            &destination,
            "adapter@1.2.3",
            "adapter",
            &Npm::program(&npm),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(entry.starts_with(destination.canonicalize().unwrap()));
        assert!(entry.is_file());
        // 未完成の導入は版別ディレクトリとして公開しない。
        std::fs::write(&npm, "#!/bin/sh\nexit 1\n").unwrap();
        let failed = root.path().join("adapter@1.2.4");
        assert!(install_package(
            root.path(),
            &failed,
            "adapter@1.2.4",
            "adapter",
            &Npm::program(&npm),
            &BTreeMap::new()
        )
        .is_err());
        assert!(!failed.exists());
        assert!(entry.is_file());
        assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("install-")));
    }

    #[test]
    fn publishing_a_version_removes_the_older_ones_of_the_same_package() {
        let root = tempfile::tempdir().unwrap();
        let version = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.join("node_modules")).unwrap();
            path
        };
        let current = version("@scope%2Fadapter@1.2.4");
        let old = version("@scope%2Fadapter@1.2.3");
        let other_package = version("@scope%2Fother@1.2.3");
        // 版の区切りまで一致させる（`adapter-extra` を `adapter` の古い版と取り違えない）。
        let similar_name = version("@scope%2Fadapter-extra@1.2.3");
        remove_stale_versions(root.path(), "@scope/adapter@1.2.4", &current);
        assert!(current.is_dir());
        assert!(!old.exists());
        assert!(other_package.is_dir());
        assert!(similar_name.is_dir());
    }

    /// 動いているセッションの版（印を持つ版）は、新しい版を公開しても消さない。印が外れた後の掃除で消える。
    /// 2026-11-02: codex-acp 2.5.0 の導入が起動途中の前の版を消し、Codex のスレッドが起動直後に落ちた。
    #[test]
    fn a_version_in_use_survives_until_its_sessions_end() {
        let root = tempfile::tempdir().unwrap();
        let version = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.join("node_modules")).unwrap();
            path
        };
        let current = version("@scope%2Fadapter@1.2.4");
        let running = version("@scope%2Fadapter@1.2.3");
        let lease = VersionLease::take(&running).expect("消している最中ではない");

        remove_stale_versions(root.path(), "@scope/adapter@1.2.4", &current);
        assert!(running.is_dir(), "使用中の版は残す");

        drop(lease);
        // 並列の別の試験が子プロセスを起こす瞬間（fork から exec まで）は、印のファイル記述子が子にも
        // 複製されてロックが一瞬残る。本番では「少し長く使用中に見える」だけ（消さない側に倒れる）。
        for _ in 0..50 {
            sweep_stale_versions_in(root.path(), &["@scope/adapter@1.2.4".to_string()]);
            if !running.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!running.exists(), "使い終わった版は掃除で消える");
        assert!(current.is_dir());
        assert!(
            !lock_path(&running).exists(),
            "消した版の印のファイルも残さない"
        );
    }

    /// 掃除が消している最中（排他ロック中）の版には印を付けられない＝起こす側は使わない。
    #[test]
    fn a_version_being_removed_cannot_be_leased() {
        let root = tempfile::tempdir().unwrap();
        let removing = root.path().join("@scope%2Fadapter@1.2.3");
        std::fs::create_dir_all(&removing).unwrap();
        let exclusive = open_lock(&removing).unwrap();
        exclusive.try_lock().unwrap();
        assert!(VersionLease::take(&removing).is_none());
        drop(exclusive);
        assert!(VersionLease::take(&removing).is_some());
    }

    /// 掃除は `keep` にある版と別のパッケージの今の版を残し、途中で止まった削除の跡と版の無い印を片付ける。
    #[test]
    fn sweeping_keeps_current_versions_and_clears_leftovers() {
        let root = tempfile::tempdir().unwrap();
        let version = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.join("node_modules")).unwrap();
            path
        };
        let codex = version("@scope%2Fcodex@2.5.0");
        let old_codex = version("@scope%2Fcodex@1.8.0");
        let claude = version("claude@0.99.1");
        let interrupted = version(".removing-1-@scope%2Fcodex@1.0.0");
        let installing = version("install-abc123");
        let orphan_lock = root.path().join("@scope%2Fcodex@0.9.0.lock");
        std::fs::write(&orphan_lock, "").unwrap();

        sweep_stale_versions_in(
            root.path(),
            &[
                "@scope/codex@2.5.0".to_string(),
                "claude@0.99.1".to_string(),
            ],
        );
        assert!(codex.is_dir());
        assert!(claude.is_dir());
        assert!(!old_codex.exists());
        assert!(!interrupted.exists(), "止まった削除の跡は消す");
        assert!(!orphan_lock.exists(), "版の無い印は消す");
        assert!(
            installing.is_dir(),
            "導入途中のディレクトリは掃除の対象にしない（導入の側が片付ける）"
        );
    }

    /// 置き場に入れてある版を読む（設定の「今の版」）。名前が似た別のパッケージと、中身の版が置き場の
    /// 名前と合わない物（途中で止まった導入など）は数えない。
    #[test]
    fn installed_versions_are_read_from_completed_installs_only() {
        let root = tempfile::tempdir().unwrap();
        let install = |directory: &str, name: &str, version: &str| {
            let package = root.path().join(directory).join("node_modules").join(name);
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(
                package.join("package.json"),
                format!(r#"{{"name":"{name}","version":"{version}"}}"#),
            )
            .unwrap();
        };
        install("@scope%2Fadapter@0.81.2", "@scope/adapter", "0.81.2");
        install(
            "@scope%2Fadapter-extra@1.0.0",
            "@scope/adapter-extra",
            "1.0.0",
        );
        install("@scope%2Fadapter@0.84.0", "@scope/adapter", "0.83.0");
        let mut versions = installed_versions_in(root.path(), "@scope/adapter");
        versions.sort();
        assert_eq!(versions, vec!["0.81.2".to_string()]);
        assert!(installed_versions_in(&root.path().join("missing"), "@scope/adapter").is_empty());
    }

    #[test]
    fn managed_entry_requires_exact_version_and_contained_javascript() {
        let root = tempfile::tempdir().unwrap();
        let package = root.path().join("node_modules/@scope/adapter");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("index.js"), "").unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@scope/adapter","version":"1.2.3","bin":{"adapter":"index.js"}}"#,
        )
        .unwrap();
        assert!(installed_entry(root.path(), "@scope/adapter@1.2.3", "adapter").is_some());
        assert!(installed_entry(root.path(), "@scope/adapter@1.2.4", "adapter").is_none());
        for invalid in [
            "@scope/adapter@latest",
            "../adapter@1.2.3",
            "x@^1.2.3",
            "x@1.2.3/../../escape",
        ] {
            assert!(exact_package(invalid).is_none());
        }
        std::fs::write(root.path().join("escape.js"), "").unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@scope/adapter","version":"1.2.3","bin":{"adapter":"../../../escape.js"}}"#,
        )
        .unwrap();
        assert!(installed_entry(root.path(), "@scope/adapter@1.2.3", "adapter").is_none());
    }
}
