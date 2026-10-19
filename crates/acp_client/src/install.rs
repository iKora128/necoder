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

/// アダプタを（まだ無ければ）necoder の置き場へ入れておく（設定の「インストール」が、最初のスレッドを
/// 待たせないために先に呼ぶ）。`package` は版まで固定した名前（`@scope/name@1.2.3`）。
pub(crate) fn preinstall(
    package: &str,
    bin: &str,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    let root = paths::external_agents_dir()
        .context("導入先がない")?
        .join("npm");
    anyhow::ensure!(
        exact_package(package).is_some(),
        "版が固定されていない: {package}"
    );
    let directory = directory(&root, package);
    static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INSTALL
        .lock()
        .map_err(|_| anyhow::anyhow!("導入ロックが壊れている"))?;
    if let Some(entry) = installed_entry(&directory, package, bin) {
        return Ok(entry);
    }
    let node = find_in_path("node").context("node がない")?;
    let npm = Npm::find(&node).context("npm がない")?;
    install_package(&root, &directory, package, bin, &npm, environment)
}

pub(super) fn cached_command(
    package: &str,
    bin: &str,
    args: Vec<String>,
    cwd: PathBuf,
) -> Option<AgentCommand> {
    let root = paths::external_agents_dir()?.join("npm");
    let node = find_in_path("node")?;
    let entry = installed_entry(&directory(&root, package), package, bin)?;
    let mut direct_args = vec![entry.to_string_lossy().into_owned()];
    direct_args.extend(args);
    Some(AgentCommand::new(node, direct_args, cwd))
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
    let prepared = (|| -> Result<(PathBuf, PathBuf)> {
        let entry = preinstall(&package, &bin, &command.env)?;
        let node = find_in_path("node").context("node がない")?;
        Ok((node, entry))
    })();
    match prepared {
        Ok((node, entry)) => {
            // 元の -y と npm spec を除き、エージェント自身の引数だけを保つ。
            let extra = command.args.split_off(2);
            command.path = node;
            command.args = vec![entry.to_string_lossy().into_owned()];
            command.args.extend(extra);
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
/// 走っているエージェントは起動時に読み込み済みなので動き続ける。消した版を**新しく**起こすことは
/// できなくなるが、解決は常に現行版を選ぶので誰も古い版を起こさない。消せなくても導入自体は成功
/// （次の更新でまた試みる）なので、報告だけして止めない。
fn remove_stale_versions(root: &Path, package: &str, keep: &Path) {
    let Some((name, _)) = exact_package(package) else {
        return;
    };
    let prefix = format!("{}@", name.replace('/', "%2F"));
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("古い ACP アダプタを探せない: {error}");
            return;
        }
    };
    for entry in entries.flatten() {
        let stale = entry.path();
        if stale == keep || !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(&stale) {
            eprintln!("古い ACP アダプタを消せない（{}）: {error}", stale.display());
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
