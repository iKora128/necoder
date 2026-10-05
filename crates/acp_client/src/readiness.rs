//! readiness — エージェントがあるホスト（このマシン / SSH 先）で動くかと、その版を**読むだけ**で確かめる
//! （設定 › AI エージェントのページ・issue #38 H2 の見せ方）。
//!
//! - **事実**（[`HostFacts`]）: PATH の実行ファイルと、それが入っている npm パッケージの版・npx のキャッシュと
//!   necoder の置き場に入れてある版・置いた binary の版・ログインの跡（資格情報のファイルが在るかと、
//!   環境変数の名前）。このマシンは直接読む（[`local_facts`]）。SSH 先は、そこで開いているプロジェクトの
//!   接続で読み取りだけのシェルを 1 回流す（[`probe_remote`]・`command -v` / `package.json` / `test -e` /
//!   `printenv`）。どちらも CLI を子プロセスで起こさない（`--version` も聞かない）・資格情報の中身は読まない
//! - **見立て**（[`assess`]）: 事実から「使える / 足りない物（node・uv・CLI 本体・コマンド・ログイン）/
//!   このホストでは起動できない」と、版（今の版とレジストリの版・次の起動で合わせるか）を決める。
//!   起動の解決（[`crate::AgentKind::resolve_command_on`]・[`crate::custom`]）と同じ順で考えるだけで、
//!   起動の振る舞いは変えない

use crate::custom::{registry_launch, Agent, CustomAgent, CustomLaunch, LaunchError};
use crate::registry::{split_npm_spec, DistributionKind, Launch, Registry};
use crate::{AgentAuthState, AgentKind, AgentOverride};
use anyhow::{Context as _, Result};
use host::{CommandSpec, Host};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// 道具（エージェントを取りに行く・起こすのに使う物）。
const TOOLS: [&str; 4] = ["node", "npx", "uv", "uvx"];

/// 実行ファイルから上へ package.json を探す段数（`bin/x` → `lib/node_modules/<scope>/<name>/dist/x.js`）。
const PACKAGE_SEARCH_DEPTH: usize = 8;

/// あるホストで読んだ事実（読むだけ・資格情報の中身は持たない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// SSH 先の事実か（`false` = このマシン）。
    pub remote: bool,
    /// 見つかった実行ファイル（名前 → パス）。
    pub commands: BTreeMap<String, String>,
    /// 実行ファイルが入っている npm パッケージ（実行ファイルの名前 → (パッケージ名, 版)）。
    pub command_packages: BTreeMap<String, (String, String)>,
    /// 入れてある npm パッケージの版（パッケージ名 → 版・npx のキャッシュと necoder の置き場）。
    pub npm_versions: BTreeMap<String, BTreeSet<String>>,
    /// 置いてあるレジストリの binary（id → 版 → sha256 で照合したか・このマシンだけ）。
    pub binary_versions: BTreeMap<String, BTreeMap<String, bool>>,
    /// ホームから見て在るファイル（ログインの跡・相対パス）。
    pub home_files: BTreeSet<String>,
    /// 値のある環境変数の名前（ログインの跡・値は持たない）。
    pub env: BTreeSet<String>,
}

impl HostFacts {
    pub fn has_command(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    /// node で動く物を起こせるか（node か npx が在る）。
    fn node(&self) -> bool {
        self.has_command("node") || self.has_command("npx")
    }

    /// `command` が npm パッケージ `package` の物なら、その版。
    fn command_version(&self, command: &str, package: &str) -> Option<String> {
        self.command_packages
            .get(command)
            .filter(|(name, _)| name == package)
            .map(|(_, version)| version.clone())
    }
}

/// 足りない物（並びは表示の順）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Missing {
    /// node（npx で取って起こす・`npm i -g` で入れる物）。
    Node,
    /// uv（uvx で起こす・`uv tool install` で入れる物）。
    Uv,
    /// エージェントの CLI 本体（PATH の名前）。
    Cli(String),
    /// 自分のコマンド（起動の上書き・足したエージェント）が見つからない。
    Command(String),
    /// ログインしていない（CLI はある）。
    Login,
}

/// このホストで動くか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// 使える（要る物が揃い、ログインも確かめた・確かめようのない足した物は要る物が揃っている）。
    Ready,
    /// 要る物は揃っているが、ログインは跡しか見ていない（確かめられなかった・SSH 先は確かめない）。
    Unverified,
    /// 足りない物がある。
    Missing(Vec<Missing>),
    /// このホストでは起動できない（レジストリに無い・配布が無い・binary を SSH 先で等）。
    Unavailable(LaunchError),
}

/// 版の行に出す物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionInfo {
    /// 出す版が無い。
    None,
    /// 自分のコマンドで起動する（版は necoder が決めない）。`command` は引数まで。
    OwnCommand { command: String },
    /// PATH の物をそのまま使う（版はその物まかせ・necoder は合わせない）。`registry` は参考。
    Path {
        command: String,
        current: Option<String>,
        registry: Option<String>,
    },
    /// このホストに無く、レジストリに版がある（参考）。
    Registry { version: String },
    /// necoder がレジストリの版で起こす（npx / uvx / binary）。
    Managed {
        kind: DistributionKind,
        /// このホストに入れてある・置いてある版（無い・読めない時は `None`）。
        current: Option<String>,
        /// 起こす版（レジストリの版）。
        target: String,
        /// 次の起動で `target` を取りに行けるか（道具が在る）。
        fetchable: bool,
        /// binary の検証の値があるか（置いてある版は印のとおり・binary だけ意味を持つ）。
        verified: bool,
    },
}

/// 1 エージェントの見立て。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReadiness {
    pub readiness: Readiness,
    pub version: VersionInfo,
    /// レジストリから足した物の配布の形（このホストで使う物・使えなければ持っている物の先頭）。
    pub distribution: Option<DistributionKind>,
}

/// エージェントの一覧から、読みに行く物を決める。
#[derive(Debug, Default, PartialEq, Eq)]
struct Wanted {
    commands: BTreeSet<String>,
    /// PATH に無ければ Zed の npx キャッシュも見る名前（組み込みのアダプタ・このマシンだけ）。
    adapters: BTreeSet<String>,
    packages: BTreeSet<String>,
    files: BTreeSet<&'static str>,
    env: BTreeSet<&'static str>,
    /// (id, プラットフォームのキー)・このマシンの binary だけ。
    binaries: BTreeSet<(String, String)>,
}

fn host_platform(remote: bool) -> Option<&'static str> {
    if remote {
        None
    } else {
        crate::registry::platform_key()
    }
}

/// 組み込みがレジストリから起こす npm の配布（パッケージ名, 版）。レジストリの項目が npx でなければ `None`
/// （起動の解決と同じく、組み込みはレジストリの npm の版だけを使う）。
fn builtin_registry_npm(kind: &AgentKind, registry: Option<&Registry>) -> Option<(String, String)> {
    let entry = registry?.agent(kind.registry_id?)?;
    let Some(Launch::Npx(npx)) = entry.launch() else {
        return None;
    };
    let (name, version) = split_npm_spec(&npx.package);
    Some((
        name.to_string(),
        version.unwrap_or(&entry.version).to_string(),
    ))
}

fn wanted(
    agents: &[Agent],
    overrides: &BTreeMap<String, AgentOverride>,
    registry: Option<&Registry>,
    remote: bool,
) -> Wanted {
    let mut wanted = Wanted::default();
    wanted
        .commands
        .extend(TOOLS.iter().map(|tool| tool.to_string()));
    for agent in agents {
        match agent {
            Agent::Builtin(kind) => {
                wanted.commands.insert(kind.bin.to_string());
                wanted.commands.insert(kind.cli_bin.to_string());
                wanted.adapters.insert(kind.bin.to_string());
                if let Some(command) = overrides.get(kind.id).and_then(|own| own.command.as_ref()) {
                    wanted.commands.insert(command.clone());
                }
                if let Some((name, _)) = builtin_registry_npm(kind, registry) {
                    wanted.packages.insert(name);
                }
                if let Some(package) = kind.package {
                    wanted
                        .packages
                        .insert(split_npm_spec(package).0.to_string());
                }
                let traces = kind.login_traces();
                wanted
                    .env
                    .extend(traces.ready_env.iter().chain(traces.env).copied());
                wanted.files.extend(traces.files.iter().copied());
            }
            Agent::Custom(custom) => match &custom.launch {
                CustomLaunch::Command { command, .. } => {
                    wanted.commands.insert(command.clone());
                }
                CustomLaunch::Registry { .. } => {
                    match registry_launch(&custom.id, registry, host_platform(remote)) {
                        Ok((_, Launch::Npx(npx))) => {
                            wanted
                                .packages
                                .insert(split_npm_spec(&npx.package).0.to_string());
                        }
                        Ok((_, Launch::Binary { platform, .. })) => {
                            wanted.binaries.insert((custom.id.clone(), platform));
                        }
                        Ok((_, Launch::Uvx(_))) | Err(_) => {}
                    }
                }
            },
        }
    }
    wanted
}

// ── このマシン ───────────────────────────────────────────────────────────────

/// このマシンの事実を読む（**blocking・ファイルと PATH だけ**。背景で呼ぶ）。
pub fn local_facts(
    agents: &[Agent],
    overrides: &BTreeMap<String, AgentOverride>,
    registry: Option<&Registry>,
) -> HostFacts {
    let wanted = wanted(agents, overrides, registry, false);
    let mut facts = HostFacts::default();
    for name in &wanted.commands {
        let found = crate::find_in_path(name).or_else(|| {
            wanted
                .adapters
                .contains(name)
                .then(|| crate::zed_cached_agent(name))
                .flatten()
        });
        if let Some(path) = found {
            if let Some(package) = npm_package_of(&path) {
                facts.command_packages.insert(name.clone(), package);
            }
            facts
                .commands
                .insert(name.clone(), path.to_string_lossy().into_owned());
        }
    }
    let npx_cache = crate::npm_npx_cache_root();
    for package in &wanted.packages {
        let mut versions: BTreeSet<String> = crate::install::installed_versions(package)
            .into_iter()
            .collect();
        if let Some(cache) = &npx_cache {
            versions.extend(npx_cache_versions(cache, package));
        }
        if !versions.is_empty() {
            facts.npm_versions.insert(package.clone(), versions);
        }
    }
    if let Some(root) = crate::deploy::binary_root() {
        for (id, platform) in &wanted.binaries {
            let deployed: BTreeMap<String, bool> =
                crate::deploy::deployed_versions(&root, id, platform)
                    .into_iter()
                    .map(|deployed| (deployed.version, deployed.verified))
                    .collect();
            if !deployed.is_empty() {
                facts.binary_versions.insert(id.clone(), deployed);
            }
        }
    }
    for file in &wanted.files {
        let present = if *file == crate::OPENCODE_AUTH {
            crate::opencode_auth_exists()
        } else {
            crate::home_path_exists(file)
        };
        if present {
            facts.home_files.insert(file.to_string());
        }
    }
    for name in &wanted.env {
        if crate::env_has_any(&[*name]) {
            facts.env.insert(name.to_string());
        }
    }
    facts
}

/// 実行ファイルが入っている npm パッケージ（名前, 版）。symlink を辿り、上へ package.json を探す。
fn npm_package_of(path: &Path) -> Option<(String, String)> {
    let real = paths::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut directory = real.parent();
    for _ in 0..PACKAGE_SEARCH_DEPTH {
        let current = directory?;
        if let Some(found) = read_package(&current.join("package.json")) {
            return Some(found);
        }
        directory = current.parent();
    }
    None
}

/// package.json の名前と版（両方ある時だけ）。
fn read_package(manifest: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((
        json.get("name")?.as_str()?.to_string(),
        json.get("version")?.as_str()?.to_string(),
    ))
}

/// npx のキャッシュ（`<cache>/<hash>/node_modules/<package>`）に入っている版。
fn npx_cache_versions(cache: &Path, package: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let (name, version) = read_package(
                &entry
                    .path()
                    .join("node_modules")
                    .join(package)
                    .join("package.json"),
            )?;
            (name == package).then_some(version)
        })
        .collect()
}

// ── SSH 先 ────────────────────────────────────────────────────────────────────

/// SSH 先で流すシェルの頭（関数だけ）。出力は 1 行 1 事実・タブ区切り:
/// `command <名前> <パス>` / `package <名前> <パッケージ名> <版>` / `npm <パッケージ名> <版>` /
/// `file <ホームからの道>` / `env <名前>`。読むだけ（何も書かない・起こすのは sed / readlink だけ）。
const REMOTE_PRELUDE: &str = r#"necoder_field() {
  tr ',{}' '\n\n\n' < "$2" 2>/dev/null | sed -n "s/^[[:space:]]*\"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" | head -n 1
}
necoder_package() {
  directory=$(dirname -- "$1")
  depth=0
  while [ "$depth" -lt 8 ] && [ -n "$directory" ] && [ "$directory" != / ]; do
    if [ -f "$directory/package.json" ]; then
      name=$(necoder_field name "$directory/package.json")
      version=$(necoder_field version "$directory/package.json")
      if [ -n "$name" ] && [ -n "$version" ]; then
        printf 'package\t%s\t%s\t%s\n' "$2" "$name" "$version"
        return 0
      fi
    fi
    directory=$(dirname -- "$directory")
    depth=$((depth + 1))
  done
  return 0
}
necoder_command() {
  found=$(command -v -- "$1" 2>/dev/null) || return 0
  [ -n "$found" ] || return 0
  printf 'command\t%s\t%s\n' "$1" "$found"
  real=$(readlink -f -- "$found" 2>/dev/null) || real=$found
  [ -n "$real" ] || real=$found
  necoder_package "$real" "$1"
}
necoder_npm() {
  for manifest in "${npm_config_cache:-$HOME/.npm}"/_npx/*/node_modules/"$1"/package.json; do
    [ -f "$manifest" ] || continue
    name=$(necoder_field name "$manifest")
    version=$(necoder_field version "$manifest")
    if [ "$name" = "$1" ] && [ -n "$version" ]; then
      printf 'npm\t%s\t%s\n' "$1" "$version"
    fi
  done
  return 0
}
necoder_file() {
  if [ -e "$HOME/$1" ]; then
    printf 'file\t%s\n' "$1"
  fi
  return 0
}
necoder_env() {
  if [ -n "$(printenv "$1" 2>/dev/null)" ]; then
    printf 'env\t%s\n' "$1"
  fi
  return 0
}
"#;

/// シェルの 1 語（単引用符で囲む・中の単引用符は閉じて逃がしてから開き直す）。
fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// SSH 先で流す読み取りだけのシェル。
fn remote_script(wanted: &Wanted) -> String {
    let mut script = String::from(REMOTE_PRELUDE);
    let mut line = |function: &str, value: &str| {
        script.push_str(function);
        script.push(' ');
        script.push_str(&shell_word(value));
        script.push('\n');
    };
    for name in &wanted.commands {
        line("necoder_command", name);
    }
    for package in &wanted.packages {
        line("necoder_npm", package);
    }
    for file in &wanted.files {
        line("necoder_file", file);
    }
    for name in &wanted.env {
        line("necoder_env", name);
    }
    script.push_str("exit 0\n");
    script
}

/// SSH 先のシェルの出力を事実にする（知らない行・ログインの挨拶などは読み飛ばす）。
pub fn parse_remote_probe(output: &str) -> HostFacts {
    let mut facts = HostFacts {
        remote: true,
        ..HostFacts::default()
    };
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["command", name, path] if !path.is_empty() => {
                facts.commands.insert(name.to_string(), path.to_string());
            }
            ["package", command, name, version] => {
                facts
                    .command_packages
                    .insert(command.to_string(), (name.to_string(), version.to_string()));
            }
            ["npm", package, version] => {
                facts
                    .npm_versions
                    .entry(package.to_string())
                    .or_default()
                    .insert(version.to_string());
            }
            ["file", file] => {
                facts.home_files.insert(file.to_string());
            }
            ["env", name] => {
                facts.env.insert(name.to_string());
            }
            _ => {}
        }
    }
    facts
}

/// SSH 先の事実を読む（**blocking・ネットワーク**。背景で呼ぶ）。`cwd` はその接続先で開いている
/// プロジェクトの根（SSH 先のコマンドはプロジェクトの中でしか流せない）。新しく繋ぎには行かない —
/// 開いているプロジェクトの接続を使う。
pub fn probe_remote(
    host: &dyn Host,
    cwd: &Path,
    agents: &[Agent],
    overrides: &BTreeMap<String, AgentOverride>,
    registry: Option<&Registry>,
) -> Result<HostFacts> {
    let script = remote_script(&wanted(agents, overrides, registry, true));
    let output = host
        .run_command_retry_safe(&CommandSpec::new("sh", cwd).args(["-lc".to_string(), script]))
        .with_context(|| format!("{} のエージェントの状態を読めない", host.display_name()))?;
    anyhow::ensure!(
        output.success(),
        "{} のエージェントの状態を読めない: {}",
        host.display_name(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(parse_remote_probe(&String::from_utf8_lossy(&output.stdout)))
}

// ── 見立て ────────────────────────────────────────────────────────────────────

/// 1 エージェントの見立て。`login` はこのマシンの組み込みのログインの確かめの結果（`None` = 跡だけで
/// 決める・SSH 先はいつも跡だけ）。`own` は組み込みの起動の上書き（settings.json の `agent_servers.<id>`）。
pub fn assess(
    agent: &Agent,
    facts: &HostFacts,
    login: Option<AgentAuthState>,
    own: Option<&AgentOverride>,
    registry: Option<&Registry>,
) -> AgentReadiness {
    match agent {
        Agent::Builtin(kind) => assess_builtin(kind, facts, login, own, registry),
        Agent::Custom(custom) => assess_custom(custom, facts, registry),
    }
}

/// 組み込みの CLI を入れる道具（`install_cmd` の頭の語から）。
fn install_tool(kind: &AgentKind) -> Option<Missing> {
    let command = kind.install_cmd.trim_start();
    if command.starts_with("npm ") {
        Some(Missing::Node)
    } else if command.starts_with("uv ") {
        Some(Missing::Uv)
    } else {
        None
    }
}

fn command_line(command: &str, args: &[String]) -> String {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn assess_builtin(
    kind: &AgentKind,
    facts: &HostFacts,
    login: Option<AgentAuthState>,
    own: Option<&AgentOverride>,
    registry: Option<&Registry>,
) -> AgentReadiness {
    let own_command = own.and_then(|own| own.command.as_deref());
    let registry_npm = builtin_registry_npm(kind, registry);
    let npm_based = registry_npm.is_some() || kind.package.is_some();
    let mut missing: BTreeSet<Missing> = BTreeSet::new();
    match own_command {
        Some(command) => {
            if !facts.has_command(command) {
                missing.insert(Missing::Command(command.to_string()));
            }
        }
        None => {
            // アダプタ: PATH（このマシンは Zed のキャッシュも）に在るか、npm の配布を node で取れるか。
            let adapter = facts.has_command(kind.bin) || (npm_based && facts.node());
            if !adapter {
                missing.insert(if npm_based {
                    Missing::Node
                } else {
                    Missing::Cli(kind.bin.to_string())
                });
            }
        }
    }
    let cli_present = facts.has_command(kind.cli_bin);
    if !cli_present {
        match install_tool(kind) {
            Some(Missing::Node) if !facts.node() => {
                missing.insert(Missing::Node);
            }
            Some(Missing::Uv) if !facts.has_command("uv") => {
                missing.insert(Missing::Uv);
            }
            _ => {}
        }
        missing.insert(Missing::Cli(kind.cli_bin.to_string()));
    }
    let mut unverified = false;
    if cli_present {
        let state = login.unwrap_or_else(|| {
            kind.auth_state_from_traces(
                |name| facts.env.contains(name),
                |file| facts.home_files.contains(file),
            )
        });
        match state {
            AgentAuthState::Available => {}
            AgentAuthState::Configured => unverified = true,
            AgentAuthState::SignedOut => {
                missing.insert(Missing::Login);
            }
        }
    }
    let readiness = if !missing.is_empty() {
        Readiness::Missing(missing.into_iter().collect())
    } else if unverified {
        Readiness::Unverified
    } else {
        Readiness::Ready
    };
    AgentReadiness {
        readiness,
        version: builtin_version(kind, facts, own, registry, registry_npm),
        distribution: None,
    }
}

fn builtin_version(
    kind: &AgentKind,
    facts: &HostFacts,
    own: Option<&AgentOverride>,
    registry: Option<&Registry>,
    registry_npm: Option<(String, String)>,
) -> VersionInfo {
    if let Some(own) = own {
        if let Some(command) = &own.command {
            return VersionInfo::OwnCommand {
                command: command_line(command, &own.args),
            };
        }
    }
    let path_current = |name: &str| facts.command_version(kind.bin, name);
    match registry_npm {
        Some((name, target)) => {
            let present = facts.npm_versions.get(&name);
            let target_present = present.is_some_and(|versions| versions.contains(&target));
            // PATH のアダプタを使うのは、SSH 先で PATH に在る時と、このマシンで npx が無く同じ版も無い時
            // （起動の解決の順と同じ）。
            let uses_path = facts.has_command(kind.bin)
                && (facts.remote || (!facts.has_command("npx") && !target_present));
            if uses_path {
                return VersionInfo::Path {
                    command: kind.bin.to_string(),
                    current: path_current(&name),
                    registry: Some(target),
                };
            }
            VersionInfo::Managed {
                kind: DistributionKind::Npx,
                current: pick(present.into_iter().flatten(), &target),
                target,
                fetchable: facts.has_command("npx"),
                verified: false,
            }
        }
        None => {
            let registry_version = kind
                .registry_id
                .and_then(|id| registry?.agent(id))
                .map(|entry| entry.version.clone());
            if facts.has_command(kind.bin) {
                VersionInfo::Path {
                    command: kind.bin.to_string(),
                    current: kind
                        .package
                        .and_then(|package| path_current(split_npm_spec(package).0)),
                    registry: registry_version,
                }
            } else if let Some(version) = registry_version {
                VersionInfo::Registry { version }
            } else {
                VersionInfo::None
            }
        }
    }
}

fn assess_custom(
    custom: &CustomAgent,
    facts: &HostFacts,
    registry: Option<&Registry>,
) -> AgentReadiness {
    let (command, args) = match &custom.launch {
        CustomLaunch::Command { command, args, .. } => (command, args),
        CustomLaunch::Registry { .. } => return assess_registry(custom, facts, registry),
    };
    AgentReadiness {
        readiness: if facts.has_command(command) {
            Readiness::Ready
        } else {
            Readiness::Missing(vec![Missing::Command(command.clone())])
        },
        version: VersionInfo::OwnCommand {
            command: command_line(command, args),
        },
        distribution: None,
    }
}

fn assess_registry(
    custom: &CustomAgent,
    facts: &HostFacts,
    registry: Option<&Registry>,
) -> AgentReadiness {
    let entry = registry.and_then(|registry| registry.agent(&custom.id));
    let first_kind = entry.and_then(|entry| entry.kinds().first().copied());
    let (entry, launch) = match registry_launch(&custom.id, registry, host_platform(facts.remote)) {
        Ok(found) => found,
        Err(error) => {
            let binary_only = entry.is_some_and(|entry| !entry.distribution.binary.is_empty());
            let error = match error {
                LaunchError::NoDistribution { .. } if facts.remote && binary_only => {
                    LaunchError::BinaryOnRemote
                }
                other => other,
            };
            return AgentReadiness {
                readiness: Readiness::Unavailable(error),
                version: VersionInfo::None,
                distribution: first_kind,
            };
        }
    };
    let distribution = Some(launch.kind());
    let (readiness, version) = match launch {
        Launch::Npx(npx) => {
            let (name, version) = split_npm_spec(&npx.package);
            let target = version.unwrap_or(&entry.version).to_string();
            (
                if facts.node() {
                    Readiness::Ready
                } else {
                    Readiness::Missing(vec![Missing::Node])
                },
                VersionInfo::Managed {
                    kind: DistributionKind::Npx,
                    current: pick(facts.npm_versions.get(name).into_iter().flatten(), &target),
                    target,
                    fetchable: facts.has_command("npx"),
                    verified: false,
                },
            )
        }
        Launch::Uvx(_) => {
            let uv = facts.has_command("uvx");
            (
                if uv {
                    Readiness::Ready
                } else {
                    Readiness::Missing(vec![Missing::Uv])
                },
                // uv のキャッシュの中は読まない（今の版は分からない）。uvx は版を指定して起こす。
                VersionInfo::Managed {
                    kind: DistributionKind::Uvx,
                    current: None,
                    target: entry.version.clone(),
                    fetchable: uv,
                    verified: false,
                },
            )
        }
        Launch::Binary { distribution, .. } => {
            let deployed = facts.binary_versions.get(&custom.id);
            let current = pick(
                deployed.into_iter().flat_map(BTreeMap::keys),
                &entry.version,
            );
            let verified = match (&current, deployed) {
                (Some(current), Some(deployed)) if *current == entry.version => {
                    deployed.get(current).copied().unwrap_or(false)
                }
                _ => distribution.sha256.is_some(),
            };
            (
                Readiness::Ready,
                VersionInfo::Managed {
                    kind: DistributionKind::Binary,
                    current,
                    target: entry.version.clone(),
                    fetchable: true,
                    verified,
                },
            )
        }
    };
    AgentReadiness {
        readiness,
        version,
        distribution,
    }
}

/// 入れてある版のうち、`target` があればそれ、無ければ一番新しい物。
fn pick<'a>(versions: impl IntoIterator<Item = &'a String>, target: &str) -> Option<String> {
    let versions: Vec<&String> = versions.into_iter().collect();
    if versions.iter().any(|version| version.as_str() == target) {
        return Some(target.to_string());
    }
    versions
        .into_iter()
        .max_by(|left, right| compare_versions(left, right))
        .cloned()
}

/// 版を数の並びとして比べる（`1.10.0` > `1.9.3`）。数で決まらなければ文字で比べる。
pub fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    fn numbers(version: &str) -> Vec<u64> {
        version
            .split(['.', '-', '+'])
            .map_while(|part| {
                let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .collect()
    }
    numbers(left)
        .cmp(&numbers(right))
        .then_with(|| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom::CustomAgent;
    use crate::AGENTS;
    use std::sync::Arc;

    fn builtin(id: &str) -> Agent {
        Agent::Builtin(
            AGENTS
                .iter()
                .find(|agent| agent.id == id)
                .expect("組み込みに在る"),
        )
    }

    fn custom(id: &str, launch: CustomLaunch) -> Agent {
        Agent::Custom(Arc::new(CustomAgent {
            id: id.to_string(),
            label: id.to_string(),
            launch,
        }))
    }

    fn from_registry(id: &str) -> Agent {
        custom(
            id,
            CustomLaunch::Registry {
                env: BTreeMap::new(),
            },
        )
    }

    /// 実物と同じ形のレジストリ（組み込みの npx・PATH の CLI の binary・足した物の 3 形）。
    fn registry() -> Registry {
        let platform = crate::registry::platform_key().unwrap_or("darwin-aarch64");
        crate::registry::parse(&format!(
            r#"{{"version":"1.0.0","agents":[
              {{"id":"claude-acp","name":"Claude Agent","version":"0.84.0","description":"",
                "distribution":{{"npx":{{"package":"@agentclientprotocol/claude-agent-acp@0.84.0"}}}}}},
              {{"id":"codex-acp","name":"Codex","version":"2.1.0","description":"",
                "distribution":{{"npx":{{"package":"@agentclientprotocol/codex-acp@2.1.0"}}}}}},
              {{"id":"github-copilot-cli","name":"Copilot","version":"1.0.90","description":"",
                "distribution":{{"npx":{{"package":"@github/copilot@1.0.90"}}}}}},
              {{"id":"opencode","name":"OpenCode","version":"1.18.34","description":"",
                "distribution":{{"binary":{{"{platform}":{{"archive":"https://example.invalid/o.tar.gz","cmd":"./opencode"}}}}}}}},
              {{"id":"kimi","name":"Kimi","version":"1.52.0","description":"",
                "distribution":{{"binary":{{"{platform}":{{"archive":"https://example.invalid/k.tar.gz","cmd":"./kimi"}}}}}}}},
              {{"id":"pi-acp","name":"pi ACP","version":"0.0.34","description":"",
                "distribution":{{"npx":{{"package":"pi-acp@0.0.34"}}}}}},
              {{"id":"fast-agent","name":"fast-agent","version":"0.10.1","description":"",
                "distribution":{{"uvx":{{"package":"fast-agent-acp==0.10.1"}}}}}},
              {{"id":"amp-acp","name":"Amp","version":"0.9.0","description":"",
                "distribution":{{"binary":{{"{platform}":{{"archive":"https://example.invalid/a.tar.gz",
                                                           "cmd":"./amp-acp","sha256":"{sha}"}}}}}}}}
            ]}}"#,
            sha = "0".repeat(64)
        ))
        .expect("見本を読める")
    }

    fn facts(commands: &[&str]) -> HostFacts {
        HostFacts {
            commands: commands
                .iter()
                .map(|name| (name.to_string(), format!("/bin/{name}")))
                .collect(),
            ..HostFacts::default()
        }
    }

    fn npm(facts: &mut HostFacts, package: &str, versions: &[&str]) {
        facts.npm_versions.insert(
            package.to_string(),
            versions.iter().map(|version| version.to_string()).collect(),
        );
    }

    /// このマシンの組み込み: 要る物が揃いログインも確かめた物は「使える」。今の版はキャッシュの版
    /// （レジストリの版が在ればそれ・無ければ一番新しい物）で、レジストリの版と並べる。
    #[test]
    fn a_builtin_is_ready_with_its_cached_adapter_version() {
        let registry = registry();
        let mut local = facts(&["node", "npx", "claude", "codex"]);
        npm(
            &mut local,
            "@agentclientprotocol/claude-agent-acp",
            &["0.81.2", "0.84.0"],
        );
        npm(
            &mut local,
            "@agentclientprotocol/codex-acp",
            &["1.11.0", "1.13.1", "1.9.9"],
        );
        let claude = assess(
            &builtin("claude"),
            &local,
            Some(AgentAuthState::Available),
            None,
            Some(&registry),
        );
        assert_eq!(claude.readiness, Readiness::Ready);
        assert_eq!(
            claude.version,
            VersionInfo::Managed {
                kind: DistributionKind::Npx,
                current: Some("0.84.0".to_string()),
                target: "0.84.0".to_string(),
                fetchable: true,
                verified: false,
            }
        );
        let codex = assess(
            &builtin("codex"),
            &local,
            Some(AgentAuthState::Configured),
            None,
            Some(&registry),
        );
        assert_eq!(codex.readiness, Readiness::Unverified);
        assert_eq!(
            codex.version,
            VersionInfo::Managed {
                kind: DistributionKind::Npx,
                current: Some("1.13.1".to_string()),
                target: "2.1.0".to_string(),
                fetchable: true,
                verified: false,
            },
            "数で比べて一番新しい物（1.13.1 > 1.9.9）"
        );
    }

    /// 足りない物: CLI 本体・その入れ方の道具（npm → node / uv tool → uv）・ログイン（CLI がある時だけ）。
    #[test]
    fn missing_tools_cli_and_login_are_listed_in_order() {
        let registry = registry();
        let bare = facts(&[]);
        assert_eq!(
            assess(&builtin("copilot"), &bare, None, None, Some(&registry)).readiness,
            Readiness::Missing(vec![Missing::Node, Missing::Cli("copilot".to_string())])
        );
        let with_node = facts(&["node", "npx"]);
        assert_eq!(
            assess(&builtin("copilot"), &with_node, None, None, Some(&registry)).readiness,
            Readiness::Missing(vec![Missing::Cli("copilot".to_string())]),
            "アダプタは npx で取れる・CLI だけ足りない"
        );
        assert_eq!(
            assess(&builtin("kimi"), &with_node, None, None, Some(&registry)).readiness,
            Readiness::Missing(vec![Missing::Uv, Missing::Cli("kimi".to_string())]),
            "kimi は uv で入れる・npm の配布が無いので CLI が無ければ起こせない"
        );
        let signed_out = facts(&["node", "npx", "claude"]);
        assert_eq!(
            assess(
                &builtin("claude"),
                &signed_out,
                Some(AgentAuthState::SignedOut),
                None,
                Some(&registry)
            )
            .readiness,
            Readiness::Missing(vec![Missing::Login])
        );
    }

    /// 起動を自分のコマンドで上書きした組み込みは、そのコマンドの有無を見て版を出さない。
    #[test]
    fn an_overridden_builtin_is_judged_by_its_own_command() {
        let own = AgentOverride {
            command: Some("my-codex-acp".to_string()),
            args: vec!["--verbose".to_string()],
            env: BTreeMap::new(),
        };
        let found = facts(&["codex", "my-codex-acp"]);
        let codex = assess(
            &builtin("codex"),
            &found,
            Some(AgentAuthState::Available),
            Some(&own),
            Some(&registry()),
        );
        assert_eq!(codex.readiness, Readiness::Ready);
        assert_eq!(
            codex.version,
            VersionInfo::OwnCommand {
                command: "my-codex-acp --verbose".to_string()
            }
        );
        let missing = facts(&["codex"]);
        assert_eq!(
            assess(
                &builtin("codex"),
                &missing,
                Some(AgentAuthState::Available),
                Some(&own),
                None
            )
            .readiness,
            Readiness::Missing(vec![Missing::Command("my-codex-acp".to_string())])
        );
    }

    /// レジストリが npx でない組み込み（opencode）は PATH の CLI をそのまま使う（necoder は合わせない）。
    /// SSH 先は PATH のアダプタを先に使う。ログインは跡だけ（確かめない）。
    #[test]
    fn path_clis_and_remote_adapters_are_not_moved_to_the_registry_version() {
        let registry = registry();
        let mut local = facts(&["opencode"]);
        local.command_packages.insert(
            "opencode".to_string(),
            ("opencode-ai".to_string(), "1.17.0".to_string()),
        );
        let opencode = assess(
            &builtin("opencode"),
            &local,
            Some(AgentAuthState::Available),
            None,
            Some(&registry),
        );
        assert_eq!(opencode.readiness, Readiness::Ready);
        assert_eq!(
            opencode.version,
            VersionInfo::Path {
                command: "opencode".to_string(),
                current: Some("1.17.0".to_string()),
                registry: Some("1.18.34".to_string()),
            }
        );

        let mut remote = facts(&["claude", "claude-agent-acp", "npx"]);
        remote.remote = true;
        remote.command_packages.insert(
            "claude-agent-acp".to_string(),
            (
                "@agentclientprotocol/claude-agent-acp".to_string(),
                "0.70.2".to_string(),
            ),
        );
        remote
            .home_files
            .insert(".claude/.credentials.json".to_string());
        let claude = assess(&builtin("claude"), &remote, None, None, Some(&registry));
        assert_eq!(claude.readiness, Readiness::Unverified, "跡だけ");
        assert_eq!(
            claude.version,
            VersionInfo::Path {
                command: "claude-agent-acp".to_string(),
                current: Some("0.70.2".to_string()),
                registry: Some("0.84.0".to_string()),
            }
        );
        remote.env.insert("ANTHROPIC_API_KEY".to_string());
        assert_eq!(
            assess(&builtin("claude"), &remote, None, None, Some(&registry)).readiness,
            Readiness::Ready,
            "API キーがあればそのまま使える"
        );
    }

    /// 足した物: 自分のコマンド・npx（node）・uvx（uv）・binary（手元は最初の起動で落とす・SSH 先は不可）・
    /// レジストリに無い物。
    #[test]
    fn added_agents_are_judged_by_their_distribution() {
        let registry = registry();
        let dsh = custom(
            "dsh",
            CustomLaunch::Command {
                command: "dsh-acp".to_string(),
                args: vec!["--acp".to_string()],
                env: BTreeMap::new(),
            },
        );
        let bare = facts(&[]);
        let judged = assess(&dsh, &bare, None, None, Some(&registry));
        assert_eq!(
            judged.readiness,
            Readiness::Missing(vec![Missing::Command("dsh-acp".to_string())])
        );
        assert_eq!(
            judged.version,
            VersionInfo::OwnCommand {
                command: "dsh-acp --acp".to_string()
            }
        );

        let pi = assess(&from_registry("pi-acp"), &bare, None, None, Some(&registry));
        assert_eq!(pi.readiness, Readiness::Missing(vec![Missing::Node]));
        assert_eq!(pi.distribution, Some(DistributionKind::Npx));
        let mut with_node = facts(&["node", "npx"]);
        npm(&mut with_node, "pi-acp", &["0.0.30"]);
        let pi = assess(
            &from_registry("pi-acp"),
            &with_node,
            None,
            None,
            Some(&registry),
        );
        assert_eq!(pi.readiness, Readiness::Ready);
        assert_eq!(
            pi.version,
            VersionInfo::Managed {
                kind: DistributionKind::Npx,
                current: Some("0.0.30".to_string()),
                target: "0.0.34".to_string(),
                fetchable: true,
                verified: false,
            }
        );

        assert_eq!(
            assess(
                &from_registry("fast-agent"),
                &with_node,
                None,
                None,
                Some(&registry)
            )
            .readiness,
            Readiness::Missing(vec![Missing::Uv])
        );

        if crate::registry::platform_key().is_some() {
            let mut deployed = facts(&[]);
            deployed.binary_versions.insert(
                "amp-acp".to_string(),
                [("0.8.0".to_string(), false)].into_iter().collect(),
            );
            let amp = assess(
                &from_registry("amp-acp"),
                &deployed,
                None,
                None,
                Some(&registry),
            );
            assert_eq!(amp.readiness, Readiness::Ready);
            assert_eq!(
                amp.version,
                VersionInfo::Managed {
                    kind: DistributionKind::Binary,
                    current: Some("0.8.0".to_string()),
                    target: "0.9.0".to_string(),
                    fetchable: true,
                    verified: true,
                },
                "次に落とす版はレジストリの sha256 で照合する"
            );
        }
        let mut remote = facts(&["node", "npx"]);
        remote.remote = true;
        let amp = assess(
            &from_registry("amp-acp"),
            &remote,
            None,
            None,
            Some(&registry),
        );
        assert_eq!(
            amp.readiness,
            Readiness::Unavailable(LaunchError::BinaryOnRemote)
        );
        assert_eq!(amp.distribution, Some(DistributionKind::Binary));
        assert_eq!(
            assess(
                &from_registry("gone-acp"),
                &remote,
                None,
                None,
                Some(&registry)
            )
            .readiness,
            Readiness::Unavailable(LaunchError::NotInRegistry {
                id: "gone-acp".to_string()
            })
        );
    }

    /// SSH 先の出力を読む（知らない行・挨拶は読み飛ばす）。
    #[test]
    fn the_remote_probe_output_is_parsed_line_by_line() {
        let facts = parse_remote_probe(
            "Welcome to dev-box\n\
             command\tnode\t/usr/bin/node\n\
             command\tclaude-agent-acp\t/usr/local/bin/claude-agent-acp\n\
             package\tclaude-agent-acp\t@agentclientprotocol/claude-agent-acp\t0.70.2\n\
             npm\tpi-acp\t0.0.30\n\
             npm\tpi-acp\t0.0.34\n\
             file\t.claude/.credentials.json\n\
             env\tKIMI_API_KEY\n\
             command\tbroken\n",
        );
        assert!(facts.remote);
        assert_eq!(
            facts.commands.keys().collect::<Vec<_>>(),
            vec!["claude-agent-acp", "node"]
        );
        assert_eq!(
            facts.command_version("claude-agent-acp", "@agentclientprotocol/claude-agent-acp"),
            Some("0.70.2".to_string())
        );
        assert_eq!(
            facts.command_version("claude-agent-acp", "other"),
            None,
            "名前が合わない package.json の版は使わない"
        );
        assert_eq!(facts.npm_versions["pi-acp"].len(), 2);
        assert!(facts.home_files.contains(".claude/.credentials.json"));
        assert!(facts.env.contains("KIMI_API_KEY"));
    }

    #[test]
    fn versions_compare_by_number() {
        use std::cmp::Ordering;
        assert_eq!(compare_versions("1.10.0", "1.9.3"), Ordering::Greater);
        assert_eq!(compare_versions("0.84.0", "0.84.0"), Ordering::Equal);
        assert_eq!(compare_versions("2.1.0", "10.0.0"), Ordering::Less);
        let versions = ["0.9.0".to_string(), "0.10.0".to_string()];
        assert_eq!(pick(&versions, "0.11.0"), Some("0.10.0".to_string()));
        assert_eq!(pick(&versions, "0.9.0"), Some("0.9.0".to_string()));
        assert_eq!(pick(&[], "0.9.0"), None);
    }

    /// SSH 先で流すシェルを、このマシンの `sh` で本当に流して確かめる（偽のホーム: PATH に node・npx・
    /// npm で入れたアダプタの symlink、npx のキャッシュ、ログインの跡のファイル・環境変数）。
    /// 引用・glob・symlink の辿り方・package.json の読み方が、パースと合っていること。
    #[cfg(unix)]
    #[test]
    fn the_remote_script_reads_a_fake_home() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let home = tempfile::tempdir().expect("一時フォルダ");
        let root = home.path();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("作れる");
        let executable = |path: &Path| {
            std::fs::write(path, "#!/bin/sh\n").expect("書ける");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("実行できる");
        };
        executable(&bin.join("node"));
        executable(&bin.join("npx"));
        let package = root.join("lib/node_modules/@agentclientprotocol/claude-agent-acp");
        std::fs::create_dir_all(package.join("dist")).expect("作れる");
        std::fs::write(
            package.join("package.json"),
            "{\n  \"name\": \"@agentclientprotocol/claude-agent-acp\",\n  \"version\": \"0.70.2\",\n  \"bin\": { \"claude-agent-acp\": \"dist/index.js\" }\n}\n",
        )
        .expect("書ける");
        executable(&package.join("dist/index.js"));
        symlink(package.join("dist/index.js"), bin.join("claude-agent-acp")).expect("張れる");
        let cached = root.join(".npm/_npx/0a1b/node_modules/pi-acp");
        std::fs::create_dir_all(&cached).expect("作れる");
        std::fs::write(
            cached.join("package.json"),
            "{ \"name\": \"pi-acp\",\n  \"version\": \"0.0.30\" }\n",
        )
        .expect("書ける");
        std::fs::create_dir_all(root.join(".claude")).expect("作れる");
        std::fs::write(root.join(".claude/.credentials.json"), "{}").expect("書ける");

        let mut wanted = Wanted::default();
        for name in ["node", "npx", "uvx", "claude-agent-acp", "it's"] {
            wanted.commands.insert(name.to_string());
        }
        wanted.packages.insert("pi-acp".to_string());
        wanted
            .packages
            .insert("@agentclientprotocol/claude-agent-acp".to_string());
        wanted.files.insert(".claude/.credentials.json");
        wanted.files.insert(".codex/auth.json");
        wanted.env.insert("KIMI_API_KEY");
        wanted.env.insert("XAI_API_KEY");
        // SSH 先（Linux）の sh は dash のことが多い。在れば dash でも流す。
        for shell in ["/bin/sh", "/bin/dash"] {
            if !Path::new(shell).exists() {
                continue;
            }
            let output = std::process::Command::new(shell)
                .arg("-c")
                .arg(remote_script(&wanted))
                .env_clear()
                .env("HOME", root)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("KIMI_API_KEY", "set")
                .env("XAI_API_KEY", "")
                .output()
                .expect("sh を起こせる");
            assert!(output.status.success(), "{shell}: {output:?}");
            assert_fake_home_facts(&parse_remote_probe(&String::from_utf8_lossy(
                &output.stdout,
            )));
        }
    }

    #[cfg(unix)]
    fn assert_fake_home_facts(facts: &HostFacts) {
        assert_eq!(
            facts.commands.keys().collect::<Vec<_>>(),
            vec!["claude-agent-acp", "node", "npx"]
        );
        assert_eq!(
            facts.command_version("claude-agent-acp", "@agentclientprotocol/claude-agent-acp"),
            Some("0.70.2".to_string()),
            "symlink を辿って package.json の版を読む"
        );
        assert_eq!(
            facts.npm_versions.get("pi-acp"),
            Some(&["0.0.30".to_string()].into_iter().collect())
        );
        assert!(
            !facts
                .npm_versions
                .contains_key("@agentclientprotocol/claude-agent-acp"),
            "npx のキャッシュに無い物は無い"
        );
        assert_eq!(
            facts.home_files.iter().collect::<Vec<_>>(),
            vec![".claude/.credentials.json"]
        );
        assert_eq!(
            facts.env.iter().collect::<Vec<_>>(),
            vec!["KIMI_API_KEY"],
            "空の値は跡にしない"
        );
    }

    /// 読みに行く物: 道具・組み込みのアダプタと CLI・上書きのコマンド・npm の名前・跡・置いた binary。
    #[test]
    fn what_to_read_follows_the_agents() {
        let registry = registry();
        let overrides: BTreeMap<String, AgentOverride> = [(
            "codex".to_string(),
            AgentOverride {
                command: Some("my-codex-acp".to_string()),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        )]
        .into_iter()
        .collect();
        let agents = vec![
            builtin("claude"),
            builtin("codex"),
            from_registry("pi-acp"),
            from_registry("amp-acp"),
            custom(
                "dsh",
                CustomLaunch::Command {
                    command: "dsh-acp".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            ),
        ];
        let local = wanted(&agents, &overrides, Some(&registry), false);
        for name in [
            "node",
            "uvx",
            "claude",
            "claude-agent-acp",
            "codex",
            "my-codex-acp",
            "dsh-acp",
        ] {
            assert!(local.commands.contains(name), "{name}");
        }
        assert!(local
            .packages
            .contains("@agentclientprotocol/claude-agent-acp"));
        assert!(local.packages.contains("pi-acp"));
        assert!(local.files.contains(".claude/.credentials.json"));
        assert!(local.env.contains("ANTHROPIC_API_KEY"));
        if let Some(platform) = crate::registry::platform_key() {
            assert!(local
                .binaries
                .contains(&("amp-acp".to_string(), platform.to_string())));
        }
        let remote = wanted(&agents, &overrides, Some(&registry), true);
        assert!(remote.binaries.is_empty(), "SSH 先に binary は置かない");
    }
}
