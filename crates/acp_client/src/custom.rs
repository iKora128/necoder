//! custom — 設定で足したエージェント（H1・H2）と、組み込みとまとめた「選べるエージェント」の一覧。
//!
//! 足し方は 2 つ: 自分のコマンド（H1・`type: "custom"`）と、ACP の公開レジストリの項目（H2・
//! `type: "registry"`・id がレジストリの id。設定の「エージェントを追加」が書く）。レジストリの物は
//! このマシン向けの binary → npx → uvx の順で配布を選ぶ（binary は [`crate::deploy`] が落として置く）。
//!
//! 組み込みの [`AgentKind`]（[`AGENTS`]）は necoder が検証した `const`。こちらは settings.json の
//! `agent_servers.<新しい id>` から実行時に決まる。acp_client は設定のスキーマを知らない
//! （依存の向き）ので、settings 層が [`CustomAgentSpec`] に写して [`AgentCatalog::new`] に渡す。
//! 一覧は設定の写しとして settings 層が窓（App）ごとに持つ — プロセス全体の可変な置き場は作らない
//! （テストが並んで走っても互いの一覧を書き換えない）。
//!
//! スレッドは相手を**表示名**で覚える（DB の `threads.agent`）。だから表示名は組み込みの名前・
//! 他の足したエージェントと重ならないようにずらす（`名前 (id)`）。引く時は表示名 → id の順に当てる
//! （`name` を後から足した・消した設定でも、前に作ったスレッドが相手を見失わない）。
//!
//! 既存の 7 件の id（`codex` など）を `agent_servers` に書いた物は、今までどおり**その起動の上書き**で、
//! 新しいエージェントにはしない（`name` も読まない）。

use crate::deploy::{BinaryTarget, DeployError, Deployed};
use crate::registry::{Launch, Registry, RegistryAgent};
use crate::{
    bounded_npm_spec, find_in_path, find_on_remote, AgentCommand, AgentKind, AgentOverride, AGENTS,
};
use anyhow::Result;
use host::Host;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 足したエージェントのモノグラムの地の色（ブランド色を持たないので 1 色に揃える）。
pub const CUSTOM_BRAND_COLOR: u32 = 0x4b_55_63;

/// 足したエージェントの起動（設定の `agent_servers.<id>` を acp_client の言葉にした物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomLaunch {
    /// 自分のコマンド（`type: "custom"`）。necoder はこのコマンドをそのまま起動する（版の解決もしない）。
    Command {
        /// 絶対パスか PATH 上の名前。
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// 公開レジストリの項目（`type: "registry"`・id がレジストリの id）。起動方法はレジストリの
    /// distribution から決め、設定の `env` を最後に足す（H2）。
    Registry { env: BTreeMap<String, String> },
}

/// 足したエージェントを起動できない理由（UI が言葉にする・acp_client は i18n を持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// レジストリにこの id が無い（まだ取得していない・レジストリから消えた）。
    NotInRegistry { id: String },
    /// このマシン（OS / arch）で使える配布が無い。`platform` は手元のキー（分からなければ `None`）。
    NoDistribution { platform: Option<String> },
    /// npx が無い（node を入れると使える）。
    NeedsNode,
    /// uvx が無い（uv を入れると使える・necoder は勝手に入れない）。
    NeedsUv,
    /// binary の配布しか無いエージェントをリモートで起こそうとした（binary は手元に落として走らせる物）。
    BinaryOnRemote,
    /// binary をまだ落としていない（最初の起動で落とす・[`Agent::deploy_command`]）。
    NotDeployed,
    /// binary を置けなかった（落とせない・検証に落ちた・展開できない）。
    Deploy(DeployError),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::NotInRegistry { id } => {
                write!(formatter, "{id} が ACP レジストリに無い")
            }
            LaunchError::NoDistribution { platform } => write!(
                formatter,
                "このマシン（{}）向けの配布が無い",
                platform.as_deref().unwrap_or("?")
            ),
            LaunchError::NeedsNode => write!(formatter, "npx が無い（node が要る）"),
            LaunchError::NeedsUv => write!(formatter, "uvx が無い（uv が要る）"),
            LaunchError::BinaryOnRemote => {
                write!(formatter, "binary の配布はリモートでは起動できない")
            }
            LaunchError::NotDeployed => write!(formatter, "binary をまだ落としていない"),
            LaunchError::Deploy(error) => write!(formatter, "binary を置けない: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// 手元での起動の見込み（設定画面の状態と、起動前の判断に使う・PATH を見るだけで子プロセスは
/// 起こさない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPlan {
    /// 自分のコマンド。`found` = PATH（か絶対パス）で見つかる。
    Command { found: bool },
    /// レジストリの版を npx が初回に取って起動する。
    Npx { version: String },
    /// レジストリの版を uvx（uv）が初回に取って起動する。
    Uvx { version: String },
    /// レジストリの binary を置いて起動する。`deployed` = この版を置いてある（無ければ最初の起動で
    /// 落とす）。`verified` = sha256 を照合した / できる（`false` = レジストリに検証の値が無い）。
    Binary {
        version: String,
        deployed: bool,
        verified: bool,
    },
}

/// binary を置いた結果（UI が知らせる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployOutcome {
    /// 使う版。
    pub version: String,
    /// sha256 を照合したか（`false` = レジストリに検証の値が無い）。
    pub verified: bool,
    /// 新しい版（この値）を落とせず、手元に置いてある `version` で起こした。
    pub fallback_from: Option<String>,
}

/// 組み込みのエージェントのうち、レジストリの `id` の項目を持つ物（追加の画面で「組み込み」と出す・
/// レジストリから同じエージェントを二重に足させない）。
pub fn builtin_for_registry_id(id: &str) -> Option<&'static AgentKind> {
    AGENTS.iter().find(|agent| agent.registry_id == Some(id))
}

/// レジストリの項目と、`platform` で使う配布を引く。
pub(crate) fn registry_launch<'a>(
    id: &str,
    registry: Option<&'a Registry>,
    platform: Option<&str>,
) -> std::result::Result<(&'a RegistryAgent, Launch), LaunchError> {
    let entry = registry
        .and_then(|registry| registry.agent(id))
        .ok_or_else(|| LaunchError::NotInRegistry { id: id.to_string() })?;
    let launch = entry
        .launch_for(platform)
        .ok_or_else(|| LaunchError::NoDistribution {
            platform: platform.map(str::to_string),
        })?;
    Ok((entry, launch))
}

/// 置いた binary を起こすコマンド（レジストリの引数と env に、設定の env を最後に足す）。
fn binary_command(
    command: PathBuf,
    binary: &crate::registry::BinaryDistribution,
    settings_env: &BTreeMap<String, String>,
    cwd: PathBuf,
) -> AgentCommand {
    let mut resolved = AgentCommand::new(command, binary.args.clone(), cwd);
    resolved.env = binary.env.clone();
    resolved.env.extend(settings_env.clone());
    resolved
}

/// npx でレジストリの版を起こすコマンド（組み込みと同じく版は上限つき範囲・`-y` で聞かずに取る）。
fn npx_command(
    npx_path: PathBuf,
    npx: &crate::registry::NpxDistribution,
    settings_env: &BTreeMap<String, String>,
    cwd: PathBuf,
) -> AgentCommand {
    let mut args = vec!["-y".to_string(), bounded_npm_spec(&npx.package)];
    args.extend(npx.args.iter().cloned());
    let mut resolved = AgentCommand::new(npx_path, args, cwd);
    resolved.env = npx.env.clone();
    resolved.env.extend(settings_env.clone());
    resolved
}

/// uvx でレジストリの版を起こすコマンド（`uvx <package> <args>`・`pkg==1.2.3` も `pkg@1.2.3` も uvx が
/// そのまま受ける）。uv は necoder が入れない — 無ければ [`LaunchError::NeedsUv`] で案内する。
fn uvx_command(
    uvx_path: PathBuf,
    uvx: &crate::registry::UvxDistribution,
    settings_env: &BTreeMap<String, String>,
    cwd: PathBuf,
) -> AgentCommand {
    let mut args = vec![uvx.package.clone()];
    args.extend(uvx.args.iter().cloned());
    let mut resolved = AgentCommand::new(uvx_path, args, cwd);
    resolved.env = uvx.env.clone();
    resolved.env.extend(settings_env.clone());
    resolved
}

/// 道具（npx / uvx）が無い時の理由。
fn missing_tool(tool: &str) -> anyhow::Error {
    match tool {
        "npx" => LaunchError::NeedsNode.into(),
        "uvx" => LaunchError::NeedsUv.into(),
        _ => anyhow::anyhow!("{tool} が見つからない"),
    }
}

/// 設定側から受け取る 1 件（settings 層が `agent_servers` から写す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAgentSpec {
    /// `agent_servers` のキー。`disabled_agents` / `agent_config_defaults` も同じ id で引く。
    pub id: String,
    /// 一覧に出す名前（無い・空なら id）。
    pub name: Option<String>,
    pub launch: CustomLaunch,
}

/// 一覧に並ぶ足したエージェント。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAgent {
    pub id: String,
    /// 一覧・ピル・スレッドの `agent` に出る名前（組み込み・他の足した物と重ならない）。
    pub label: String,
    pub launch: CustomLaunch,
}

impl CustomAgent {
    /// 手元での起動の見込み（[`LaunchPlan`]）。起動できない時はその理由。
    /// 絶対パスのコマンドもここで当たる（PATH の各ディレクトリと join すると絶対パスはそのまま残る）。
    pub fn plan(
        &self,
        registry: Option<&Registry>,
    ) -> std::result::Result<LaunchPlan, LaunchError> {
        self.plan_for(registry, crate::registry::platform_key())
    }

    /// [`Self::plan`] の本体（プラットフォームを渡せる）。
    fn plan_for(
        &self,
        registry: Option<&Registry>,
        platform: Option<&str>,
    ) -> std::result::Result<LaunchPlan, LaunchError> {
        self.plan_in(
            registry,
            platform,
            crate::deploy::binary_root().as_deref(),
            &find_in_path,
        )
    }

    /// [`Self::plan`] の本体（プラットフォーム・binary の置き場・道具の探し方を渡せる）。
    fn plan_in(
        &self,
        registry: Option<&Registry>,
        platform: Option<&str>,
        binary_root: Option<&Path>,
        find_tool: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> std::result::Result<LaunchPlan, LaunchError> {
        match &self.launch {
            CustomLaunch::Command { command, .. } => Ok(LaunchPlan::Command {
                found: find_tool(command).is_some(),
            }),
            CustomLaunch::Registry { .. } => {
                let (entry, launch) = registry_launch(&self.id, registry, platform)?;
                match launch {
                    Launch::Npx(_) if find_tool("npx").is_some() => Ok(LaunchPlan::Npx {
                        version: entry.version.clone(),
                    }),
                    Launch::Npx(_) => Err(LaunchError::NeedsNode),
                    Launch::Uvx(_) if find_tool("uvx").is_some() => Ok(LaunchPlan::Uvx {
                        version: entry.version.clone(),
                    }),
                    Launch::Uvx(_) => Err(LaunchError::NeedsUv),
                    Launch::Binary {
                        platform,
                        distribution,
                    } => {
                        let installed = binary_root.and_then(|root| {
                            BinaryTarget {
                                id: &self.id,
                                version: &entry.version,
                                platform: &platform,
                                distribution: &distribution,
                            }
                            .installed(root)
                        });
                        Ok(LaunchPlan::Binary {
                            version: entry.version.clone(),
                            deployed: installed.is_some(),
                            verified: installed.map_or(distribution.sha256.is_some(), |deployed| {
                                deployed.verified
                            }),
                        })
                    }
                }
            }
        }
    }

    /// 手元で起こす前に binary を落として置く必要があるか（レジストリの binary で、この版をまだ
    /// 置いていない）。ファイルを見るだけ。
    pub fn needs_deploy(&self, registry: Option<&Registry>) -> bool {
        matches!(
            self.plan(registry),
            Ok(LaunchPlan::Binary {
                deployed: false,
                ..
            })
        )
    }

    /// binary を落として検証・展開して置き、起動コマンドを返す（**blocking・ネットワーク**。背景で呼ぶ）。
    /// 新しい版を**落とせなかった**時だけ、手元に置いてある一番新しい別の版で起こす（照合に落ちた・
    /// 展開できない時は代わりを使わない＝壊れた・食い違った物を黙って走らせない）。
    fn deploy_command(
        &self,
        cwd: PathBuf,
        registry: Option<&Registry>,
    ) -> std::result::Result<(AgentCommand, DeployOutcome), LaunchError> {
        let root = crate::deploy::binary_root().ok_or(LaunchError::Deploy(DeployError::NoPlace))?;
        self.deploy_command_in(&root, cwd, registry, |target| target.deploy(&root))
    }

    /// [`Self::deploy_command`] の本体（置き場と置き方を渡せる・テストはネットワークへ行かない）。
    fn deploy_command_in(
        &self,
        root: &Path,
        cwd: PathBuf,
        registry: Option<&Registry>,
        deploy: impl Fn(&BinaryTarget<'_>) -> std::result::Result<Deployed, DeployError>,
    ) -> std::result::Result<(AgentCommand, DeployOutcome), LaunchError> {
        let CustomLaunch::Registry { env } = &self.launch else {
            return Err(LaunchError::NotDeployed);
        };
        let (entry, launch) = registry_launch(&self.id, registry, crate::registry::platform_key())?;
        let Launch::Binary {
            platform,
            distribution,
        } = launch
        else {
            return Err(LaunchError::NotDeployed);
        };
        let target = BinaryTarget {
            id: &self.id,
            version: &entry.version,
            platform: &platform,
            distribution: &distribution,
        };
        let (deployed, fallback_from) = match deploy(&target) {
            Ok(deployed) => (deployed, None),
            Err(DeployError::Download { url, reason }) => {
                match crate::deploy::latest_other_installed(
                    root,
                    &self.id,
                    &platform,
                    &entry.version,
                ) {
                    Some(previous) => (previous, Some(entry.version.clone())),
                    None => return Err(LaunchError::Deploy(DeployError::Download { url, reason })),
                }
            }
            Err(error) => return Err(LaunchError::Deploy(error)),
        };
        let outcome = DeployOutcome {
            version: deployed.version.clone(),
            verified: deployed.verified,
            fallback_from,
        };
        Ok((
            binary_command(deployed.command, &distribution, env, cwd),
            outcome,
        ))
    }

    /// 起動コマンドを組む（ダウンロードはしない）。自分のコマンドは PATH から探し、見つからなければ
    /// 書いたまま渡す（ユーザーの指定を勝手に捨てない＝起動の失敗として原因が見える）。レジストリの
    /// 物は配布から組む。リモートは探索をリモートに任せ、binary は使わない（手元の配布の形なので）。
    fn command_on(
        &self,
        host: &dyn Host,
        cwd: PathBuf,
        registry: Option<&Registry>,
    ) -> Result<AgentCommand> {
        if host.is_remote() {
            let remote_cwd = cwd.clone();
            self.command_with(cwd, registry, None, &|tool| {
                find_on_remote(host, &remote_cwd, tool)
            })
        } else {
            self.command_with(cwd, registry, crate::registry::platform_key(), &|tool| {
                Ok(find_in_path(tool))
            })
        }
    }

    /// [`Self::command_on`] の本体。`platform` = binary を選ぶキー（`None` = リモート・binary を使わない）、
    /// `find_tool` = 道具（npx / uvx / 自分のコマンド）の探し方（手元は PATH・リモートは `command -v`）。
    fn command_with(
        &self,
        cwd: PathBuf,
        registry: Option<&Registry>,
        platform: Option<&str>,
        find_tool: &dyn Fn(&str) -> Result<Option<PathBuf>>,
    ) -> Result<AgentCommand> {
        let remote = platform.is_none();
        match &self.launch {
            CustomLaunch::Command { command, args, env } => {
                let path = if remote {
                    PathBuf::from(command)
                } else {
                    find_tool(command)?.unwrap_or_else(|| PathBuf::from(command))
                };
                let mut resolved = AgentCommand::new(path, args.clone(), cwd);
                resolved.env = env.clone();
                Ok(resolved)
            }
            CustomLaunch::Registry { env } => {
                let (entry, launch) = match registry_launch(&self.id, registry, platform) {
                    Ok(found) => found,
                    // binary しか無い物はリモートで起こせない（「このマシンの配布が無い」と言わない）。
                    Err(LaunchError::NoDistribution { .. })
                        if remote
                            && registry
                                .and_then(|registry| registry.agent(&self.id))
                                .is_some_and(|entry| !entry.distribution.binary.is_empty()) =>
                    {
                        return Err(LaunchError::BinaryOnRemote.into())
                    }
                    Err(error) => return Err(error.into()),
                };
                match launch {
                    Launch::Npx(npx) => {
                        let npx_path = find_tool("npx")?.ok_or_else(|| missing_tool("npx"))?;
                        Ok(npx_command(npx_path, &npx, env, cwd))
                    }
                    Launch::Uvx(uvx) => {
                        let uvx_path = find_tool("uvx")?.ok_or_else(|| missing_tool("uvx"))?;
                        Ok(uvx_command(uvx_path, &uvx, env, cwd))
                    }
                    Launch::Binary {
                        platform,
                        distribution,
                    } => {
                        // ここではダウンロードしない（UI スレッドからも呼ばれる）。まだ置いていなければ
                        // 呼び手が背景で [`Agent::deploy_command`] を使う。
                        let installed = crate::deploy::binary_root().and_then(|root| {
                            BinaryTarget {
                                id: &self.id,
                                version: &entry.version,
                                platform: &platform,
                                distribution: &distribution,
                            }
                            .installed(&root)
                        });
                        let deployed = installed.ok_or(LaunchError::NotDeployed)?;
                        Ok(binary_command(deployed.command, &distribution, env, cwd))
                    }
                }
            }
        }
    }
}

/// 選べるエージェント 1 件。組み込み（[`AGENTS`]）か、設定で足した物（[`CustomAgent`]）。
#[derive(Clone)]
pub enum Agent {
    Builtin(&'static AgentKind),
    Custom(Arc<CustomAgent>),
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Agent::Builtin(kind) => formatter.debug_tuple("Builtin").field(&kind.id).finish(),
            Agent::Custom(custom) => formatter.debug_tuple("Custom").field(custom).finish(),
        }
    }
}

impl PartialEq for Agent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Agent::Builtin(left), Agent::Builtin(right)) => left.id == right.id,
            (Agent::Custom(left), Agent::Custom(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for Agent {}

impl Agent {
    /// 設定のキー（`disabled_agents` / `agent_config_defaults` / `agent_servers` の id）。
    pub fn id(&self) -> &str {
        match self {
            Agent::Builtin(kind) => kind.id,
            Agent::Custom(custom) => &custom.id,
        }
    }

    /// 表示名（スレッドの `agent` に保存される綴り）。
    pub fn label(&self) -> &str {
        match self {
            Agent::Builtin(kind) => kind.label,
            Agent::Custom(custom) => &custom.label,
        }
    }

    pub fn builtin(&self) -> Option<&'static AgentKind> {
        match self {
            Agent::Builtin(kind) => Some(kind),
            Agent::Custom(_) => None,
        }
    }

    pub fn custom(&self) -> Option<&CustomAgent> {
        match self {
            Agent::Builtin(_) => None,
            Agent::Custom(custom) => Some(custom),
        }
    }

    /// ブランド表示 `(svg パス, モノグラム, 地の色)`。足した物は名前の頭文字と 1 色の地。
    pub fn brand(&self) -> (Option<&'static str>, String, u32) {
        match self {
            Agent::Builtin(kind) => {
                let (icon, monogram, color) = kind.brand();
                (icon, monogram.to_string(), color)
            }
            Agent::Custom(custom) => (None, monogram_for(&custom.label), CUSTOM_BRAND_COLOR),
        }
    }

    /// 起こす前に binary を落として置く必要があるか（手元・レジストリから足した binary で、この版を
    /// まだ置いていない）。ファイルを見るだけ。真なら呼び手は背景で [`Self::deploy_command`] を使う。
    pub fn needs_deploy(&self, host: &dyn Host, registry: Option<&Registry>) -> bool {
        match self {
            Agent::Custom(custom) if !host.is_remote() => custom.needs_deploy(registry),
            _ => false,
        }
    }

    /// binary を落として検証・展開して置き、起動コマンドを返す（**blocking・ネットワーク**。背景で呼ぶ）。
    /// 置く必要の無いエージェントには [`LaunchError::NotDeployed`]。
    pub fn deploy_command(
        &self,
        cwd: impl Into<PathBuf>,
        registry: Option<&Registry>,
    ) -> Result<(AgentCommand, DeployOutcome)> {
        match self {
            Agent::Custom(custom) => Ok(custom.deploy_command(cwd.into(), registry)?),
            Agent::Builtin(_) => Err(LaunchError::NotDeployed.into()),
        }
    }

    /// 起動コマンドを決める。組み込みは [`AgentKind::resolve_command_on`]（設定 → レジストリ →
    /// 組み込みカタログ）。足した物は自分の起動方法から組む（`settings` は読まない — 足した物は
    /// それ自体が同じ `agent_servers.<id>` から作られている）。戻り値の 3 態も組み込みと同じ。
    pub fn resolve_command_on(
        &self,
        host: &dyn Host,
        cwd: impl Into<PathBuf>,
        settings: Option<&AgentOverride>,
        registry: Option<&crate::registry::Registry>,
    ) -> Result<Option<AgentCommand>> {
        match self {
            Agent::Builtin(kind) => kind.resolve_command_on(host, cwd, settings, registry),
            Agent::Custom(custom) => custom.command_on(host, cwd.into(), registry).map(Some),
        }
    }
}

/// 名前の頭文字（語の頭を 2 つまで・大文字）。語が無ければ `?`。足したエージェントのモノグラム
/// （設定画面・スレッドのタブ・ピルで同じ物を出す）。
pub fn monogram_for(label: &str) -> String {
    let initials: String = label
        .split(|character: char| character.is_whitespace() || character == '-' || character == '_')
        .filter_map(|word| word.chars().find(|character| character.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if initials.is_empty() {
        "?".to_string()
    } else {
        initials
    }
}

/// 選べるエージェントの一覧（組み込み + 設定で足した物）。複製は安い（中身は `Arc`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCatalog {
    customs: Arc<Vec<Arc<CustomAgent>>>,
}

impl AgentCatalog {
    /// 設定の写しから一覧を作る。組み込みの id は上書き（新しいエージェントにしない）、表示名は
    /// 重ならないようにずらす。どうしても重なる（id まで同じ名前が既にある）物は一覧に入れない。
    ///
    /// 名前は `name` → レジストリの名前（レジストリから足した物）→ id の順。`registry` は名前を引く
    /// ためだけに使う（無くても一覧は作れる）。組み込みのエージェントのレジストリの項目
    /// （`claude-acp` 等）は足さない — 同じエージェントが別の起動で二重に並ぶのを避ける。
    pub fn new(specs: Vec<CustomAgentSpec>, registry: Option<&Registry>) -> Self {
        let mut taken: Vec<String> = AGENTS.iter().map(|agent| agent.label.to_string()).collect();
        let mut customs = Vec::new();
        for spec in specs {
            if spec.id.trim().is_empty() || AGENTS.iter().any(|agent| agent.id == spec.id) {
                continue;
            }
            let from_registry = matches!(spec.launch, CustomLaunch::Registry { .. });
            if from_registry && builtin_for_registry_id(&spec.id).is_some() {
                eprintln!(
                    "{} は組み込みのエージェントのレジストリの項目なので一覧に足さない",
                    spec.id
                );
                continue;
            }
            let registry_name = registry
                .filter(|_| from_registry)
                .and_then(|registry| registry.agent(&spec.id))
                .map(|entry| entry.name.clone());
            let base = spec
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .or(registry_name)
                .unwrap_or_else(|| spec.id.clone());
            let label = if taken.contains(&base) {
                format!("{base} ({})", spec.id)
            } else {
                base
            };
            if taken.contains(&label) {
                eprintln!(
                    "エージェント {} の表示名 {label} が他と重なるので一覧に入れない",
                    spec.id
                );
                continue;
            }
            taken.push(label.clone());
            customs.push(Arc::new(CustomAgent {
                id: spec.id,
                label,
                launch: spec.launch,
            }));
        }
        Self {
            customs: Arc::new(customs),
        }
    }

    /// 足したエージェント（設定の順）。
    pub fn customs(&self) -> &[Arc<CustomAgent>] {
        &self.customs
    }

    /// 全部（組み込みが先・続けて足した物）。
    pub fn agents(&self) -> Vec<Agent> {
        AGENTS
            .iter()
            .map(Agent::Builtin)
            .chain(self.customs.iter().cloned().map(Agent::Custom))
            .collect()
    }

    /// 全部の表示名（[`Self::agents`] と同じ並び）。
    pub fn labels(&self) -> Vec<String> {
        self.agents()
            .iter()
            .map(|agent| agent.label().to_string())
            .collect()
    }

    /// 表示名から引く（組み込み → 足した物の表示名 → 足した物の id の順）。
    pub fn by_label(&self, label: &str) -> Option<Agent> {
        if let Some(kind) = AgentKind::by_label(label) {
            return Some(Agent::Builtin(kind));
        }
        self.customs
            .iter()
            .find(|custom| custom.label == label)
            .or_else(|| self.customs.iter().find(|custom| custom.id == label))
            .cloned()
            .map(Agent::Custom)
    }

    /// 設定の id から引く。
    pub fn by_id(&self, id: &str) -> Option<Agent> {
        if let Some(kind) = AGENTS.iter().find(|agent| agent.id == id) {
            return Some(Agent::Builtin(kind));
        }
        self.customs
            .iter()
            .find(|custom| custom.id == id)
            .cloned()
            .map(Agent::Custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use host::LocalHost;

    fn command(id: &str, name: Option<&str>, command: &str) -> CustomAgentSpec {
        CustomAgentSpec {
            id: id.to_string(),
            name: name.map(str::to_string),
            launch: CustomLaunch::Command {
                command: command.to_string(),
                args: vec!["--acp".to_string()],
                env: [("DSH_MODEL".to_string(), "fast".to_string())]
                    .into_iter()
                    .collect(),
            },
        }
    }

    fn from_registry(id: &str, name: Option<&str>) -> CustomAgentSpec {
        CustomAgentSpec {
            id: id.to_string(),
            name: name.map(str::to_string),
            launch: CustomLaunch::Registry {
                env: [("PI_MODEL".to_string(), "glm".to_string())]
                    .into_iter()
                    .collect(),
            },
        }
    }

    /// レジストリの見本（npx の pi・binary だけの amp・uvx だけの fast-agent・組み込みの claude）。
    fn sample_registry() -> Registry {
        crate::registry::parse(
            r#"{"version":"1.0.0","agents":[
              {"id":"pi-acp","name":"pi ACP","version":"0.0.34","description":"pi",
               "distribution":{"npx":{"package":"pi-acp@0.0.34","args":["--acp"],
                                      "env":{"FROM_REGISTRY":"1"}}}},
              {"id":"amp-acp","name":"Amp","version":"0.9.0","description":"amp",
               "distribution":{"binary":{"darwin-aarch64":{"archive":"https://example.invalid/a.tar.gz",
                                                           "cmd":"./amp-acp"}}}},
              {"id":"fast-agent","name":"fast-agent","version":"0.10.1","description":"fast",
               "distribution":{"uvx":{"package":"fast-agent-acp==0.10.1"}}},
              {"id":"claude-acp","name":"Claude Agent","version":"99.0.0","description":"",
               "distribution":{"npx":{"package":"@agentclientprotocol/claude-agent-acp@99.0.0"}}}
            ]}"#,
        )
        .expect("見本を読める")
    }

    /// H1: `agent_servers` の新しい id は一覧に並ぶ。名前が無ければ id、組み込みの id は上書き
    /// （一覧を増やさない）、組み込みと同じ名前はずらす。
    #[test]
    fn custom_agents_join_the_list() {
        let catalog = AgentCatalog::new(
            vec![
                command("dsh", Some("DeepSeek Harness"), "dsh-acp"),
                command("pi", None, "pi-acp"),
                command("codex", Some("別の Codex"), "codex-acp"),
                command("my-codex", Some("Codex"), "codex-acp"),
                command("blank", Some("  "), "blank-acp"),
            ],
            None,
        );
        let labels = catalog.labels();
        assert_eq!(&labels[..AGENTS.len()], crate::AGENT_LABELS);
        assert_eq!(
            &labels[AGENTS.len()..],
            ["DeepSeek Harness", "pi", "Codex (my-codex)", "blank"]
        );
        assert_eq!(
            catalog
                .by_label("Codex")
                .map(|agent| agent.id().to_string()),
            Some("codex".to_string()),
            "組み込みの上書きは組み込みのまま"
        );
        let dsh = catalog.by_label("DeepSeek Harness").expect("名前で引ける");
        assert_eq!(dsh.id(), "dsh");
        assert!(dsh.builtin().is_none());
        // 名前を後から足した設定でも、id で覚えていたスレッドは相手を見失わない。
        assert_eq!(catalog.by_label("dsh"), Some(dsh.clone()));
        assert_eq!(catalog.by_id("dsh"), Some(dsh));
        assert_eq!(
            catalog
                .by_id("claude")
                .map(|agent| agent.label().to_string()),
            Some("Claude Code".to_string())
        );
        assert!(catalog.by_label("知らない").is_none());
    }

    /// H2: レジストリから足した物の名前は `name` → レジストリの名前 → id。組み込みのエージェントの
    /// レジストリの項目（claude-acp）は二重に並べない。
    #[test]
    fn registry_agents_are_named_from_the_registry() {
        let registry = sample_registry();
        let catalog = AgentCatalog::new(
            vec![
                from_registry("pi-acp", None),
                from_registry("amp-acp", Some("Amp (仕事用)")),
                from_registry("unknown-acp", None),
                from_registry("claude-acp", None),
            ],
            Some(&registry),
        );
        let labels: Vec<&str> = catalog
            .customs()
            .iter()
            .map(|custom| custom.label.as_str())
            .collect();
        assert_eq!(labels, vec!["pi ACP", "Amp (仕事用)", "unknown-acp"]);
        assert!(builtin_for_registry_id("claude-acp").is_some());
        assert!(builtin_for_registry_id("pi-acp").is_none());
    }

    #[test]
    fn a_custom_agent_gets_a_monogram_from_its_name() {
        let catalog = AgentCatalog::new(
            vec![command("dsh", Some("DeepSeek Harness"), "dsh-acp")],
            None,
        );
        let dsh = catalog.by_id("dsh").expect("在る");
        assert_eq!(dsh.brand(), (None, "DH".to_string(), CUSTOM_BRAND_COLOR));
        assert_eq!(monogram_for("pi-acp"), "PA");
        assert_eq!(monogram_for("amp"), "A");
        assert_eq!(monogram_for("  "), "?");
    }

    /// 起動の組み立て: 自分のコマンドをそのまま・引数と環境変数も書いたとおり。
    /// PATH に無い名前も捨てずにそのまま渡す（起動の失敗として原因が見える）。
    #[test]
    fn a_custom_command_is_launched_as_written() {
        let catalog = AgentCatalog::new(
            vec![command(
                "dsh",
                Some("DeepSeek Harness"),
                "definitely-not-a-real-agent-xyz",
            )],
            None,
        );
        let agent = catalog.by_id("dsh").expect("在る");
        let resolved = agent
            .resolve_command_on(LocalHost::shared().as_ref(), "/tmp", None, None)
            .expect("自分のコマンドは必ず組める")
            .expect("組める");
        assert!(resolved.path.ends_with("definitely-not-a-real-agent-xyz"));
        assert_eq!(resolved.args, vec!["--acp".to_string()]);
        assert_eq!(
            resolved.env.get("DSH_MODEL").map(String::as_str),
            Some("fast")
        );
        assert_eq!(resolved.cwd, PathBuf::from("/tmp"));
        assert_eq!(
            agent.custom().expect("足した物").plan(None),
            Ok(LaunchPlan::Command { found: false })
        );
    }

    /// H2: レジストリから足した npx の物は、レジストリの版（上限つき範囲）と引数で npx を起こし、
    /// レジストリの env に設定の env を足す。レジストリに無い・このマシンの配布が無い物は理由を返す。
    #[test]
    fn a_registry_agent_is_launched_from_its_distribution() {
        let registry = sample_registry();
        let catalog = AgentCatalog::new(
            vec![
                from_registry("pi-acp", None),
                from_registry("unknown-acp", None),
            ],
            Some(&registry),
        );
        let pi = catalog.by_id("pi-acp").expect("在る");
        let plan = pi.custom().expect("足した物").plan(Some(&registry));
        if find_in_path("npx").is_some() {
            assert_eq!(
                plan,
                Ok(LaunchPlan::Npx {
                    version: "0.0.34".to_string()
                })
            );
            let resolved = pi
                .resolve_command_on(LocalHost::shared().as_ref(), "/tmp", None, Some(&registry))
                .expect("組める")
                .expect("組める");
            assert_eq!(
                resolved.args,
                vec![
                    "-y".to_string(),
                    "pi-acp@0.0.0 - 0.0.34".to_string(),
                    "--acp".to_string()
                ]
            );
            assert_eq!(
                resolved.env.get("FROM_REGISTRY").map(String::as_str),
                Some("1")
            );
            assert_eq!(
                resolved.env.get("PI_MODEL").map(String::as_str),
                Some("glm")
            );
        } else {
            assert_eq!(plan, Err(LaunchError::NeedsNode));
        }
        let unknown = catalog.by_id("unknown-acp").expect("在る");
        assert_eq!(
            unknown.custom().expect("足した物").plan(Some(&registry)),
            Err(LaunchError::NotInRegistry {
                id: "unknown-acp".to_string()
            })
        );
        let Err(error) =
            unknown.resolve_command_on(LocalHost::shared().as_ref(), "/tmp", None, Some(&registry))
        else {
            panic!("レジストリに無い物は組めない");
        };
        assert_eq!(
            error.downcast_ref::<LaunchError>(),
            Some(&LaunchError::NotInRegistry {
                id: "unknown-acp".to_string()
            }),
            "理由は型のまま UI へ渡る（UI が言葉にする）"
        );
        // このマシン向けの binary が無い物（x86_64 の Linux から見た amp）は使えない。
        let amp = CustomAgent {
            id: "amp-acp".to_string(),
            label: "Amp".to_string(),
            launch: CustomLaunch::Registry {
                env: BTreeMap::new(),
            },
        };
        assert_eq!(
            amp.plan_for(Some(&registry), Some("linux-x86_64")),
            Err(LaunchError::NoDistribution {
                platform: Some("linux-x86_64".to_string())
            })
        );
    }

    /// H2-b: binary の物は、置いていなければ「最初の起動で落とす」見込み（検証の値の有無つき）。置く時は
    /// レジストリの引数と env に設定の env を足して起こす。新しい版を**落とせなかった**時だけ手元の古い版で
    /// 起こし、照合に落ちた時は代わりを使わない。
    #[test]
    fn a_registry_binary_is_deployed_before_it_starts() {
        let Some(platform) = crate::registry::platform_key() else {
            return; // レジストリのキーの無いプラットフォームでは binary を選ばない
        };
        let registry = crate::registry::parse(&format!(
            r#"{{"version":"1.0.0","agents":[
              {{"id":"amp-acp","name":"Amp","version":"0.9.0","description":"amp",
                "distribution":{{"binary":{{"{platform}":{{
                  "archive":"https://example.invalid/amp-acp","cmd":"./amp-acp",
                  "args":["acp"],"env":{{"FROM_REGISTRY":"1"}},"sha256":"{}"}}}}}}}}
            ]}}"#,
            "0".repeat(64)
        ))
        .expect("見本を読める");
        let amp = CustomAgent {
            id: "amp-acp".to_string(),
            label: "Amp".to_string(),
            launch: CustomLaunch::Registry {
                env: [("AMP_API_KEY".to_string(), "k".to_string())]
                    .into_iter()
                    .collect(),
            },
        };
        let root = std::env::temp_dir().join(format!(
            "necoder-custom-binary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        assert_eq!(
            amp.plan_in(Some(&registry), Some(platform), Some(&root), &find_in_path),
            Ok(LaunchPlan::Binary {
                version: "0.9.0".to_string(),
                deployed: false,
                verified: true,
            }),
            "まだ置いていない・sha256 で照合できる"
        );

        // 置けたら、置いたコマンドにレジストリの引数と env・設定の env を付けて起こす。
        let placed = root.join("placed-amp-acp");
        std::fs::create_dir_all(&root).expect("置き場を作れる");
        std::fs::write(
            &placed,
            "#!/bin/sh
",
        )
        .expect("書ける");
        let (command, outcome) = amp
            .deploy_command_in(&root, PathBuf::from("/tmp"), Some(&registry), |target| {
                assert_eq!(target.version, "0.9.0");
                assert_eq!(target.platform, platform);
                Ok(Deployed {
                    command: placed.clone(),
                    verified: true,
                    version: "0.9.0".to_string(),
                })
            })
            .expect("置ける");
        assert_eq!(command.path, placed);
        assert_eq!(command.args, vec!["acp".to_string()]);
        assert_eq!(
            command.env.get("FROM_REGISTRY").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            command.env.get("AMP_API_KEY").map(String::as_str),
            Some("k")
        );
        assert_eq!(
            outcome,
            DeployOutcome {
                version: "0.9.0".to_string(),
                verified: true,
                fallback_from: None,
            }
        );

        // 新しい版を落とせない時は、手元に置いてある古い版で起こす。
        let older = crate::registry::BinaryDistribution {
            archive: "https://example.invalid/amp-acp".to_string(),
            cmd: "./amp-acp".to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
            sha256: None,
        };
        BinaryTarget {
            id: "amp-acp",
            version: "0.8.0",
            platform,
            distribution: &older,
        }
        .deploy_with(&root, |_url: &str, destination: &Path| {
            std::fs::write(
                destination,
                "#!/bin/sh
",
            )
            .map_err(|error| DeployError::Io(error.to_string()))
        })
        .expect("古い版を置ける");
        let offline = |_: &BinaryTarget<'_>| {
            Err(DeployError::Download {
                url: "https://example.invalid/amp-acp".to_string(),
                reason: "offline".to_string(),
            })
        };
        let (command, outcome) = amp
            .deploy_command_in(&root, PathBuf::from("/tmp"), Some(&registry), offline)
            .expect("手元の版で起こす");
        assert!(command.path.starts_with(root.join("amp-acp/0.8.0")));
        assert_eq!(outcome.version, "0.8.0");
        assert_eq!(outcome.fallback_from.as_deref(), Some("0.9.0"));
        assert!(!outcome.verified, "古い版は検証の値が無かった");

        // 照合に落ちた時は代わりを使わない（食い違った物を黙って走らせない）。
        let mismatch = |_: &BinaryTarget<'_>| {
            Err(DeployError::ChecksumMismatch {
                expected: "a".to_string(),
                actual: "b".to_string(),
            })
        };
        match amp.deploy_command_in(&root, PathBuf::from("/tmp"), Some(&registry), mismatch) {
            Err(LaunchError::Deploy(DeployError::ChecksumMismatch { .. })) => {}
            Err(other) => panic!("照合の失敗のまま返す: {other:?}"),
            Ok(_) => panic!("照合の失敗のまま返す（代わりを使わない）"),
        }
        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("テストの置き場を消せない: {error}");
        }
    }

    /// H2-c: uvx の物は、uv（uvx）がある機械では `uvx <package> <args>` で起こし、レジストリの env に
    /// 設定の env を足す。無い機械では「uv が要る」を返す（necoder は勝手に入れない）。リモートでは
    /// binary を使わない（binary しか無い物は「リモートでは起動できない」）。
    #[test]
    fn a_uvx_agent_starts_with_uv_and_asks_for_it_otherwise() {
        let registry = crate::registry::parse(
            r#"{"version":"1.0.0","agents":[
              {"id":"fast-agent","name":"fast-agent","version":"0.10.1","description":"",
               "distribution":{"uvx":{"package":"fast-agent-acp==0.10.1","args":["-x"],
                                      "env":{"FAST_AGENT_MODEL":"codexplan"}}}},
              {"id":"amp-acp","name":"Amp","version":"0.9.0","description":"",
               "distribution":{"binary":{"linux-x86_64":{"archive":"https://example.invalid/a.tar.gz",
                                                         "cmd":"./amp-acp"}}}}
            ]}"#,
        )
        .expect("見本を読める");
        let fast = CustomAgent {
            id: "fast-agent".to_string(),
            label: "fast-agent".to_string(),
            launch: CustomLaunch::Registry {
                env: [(
                    "OPENAI_BASE_URL".to_string(),
                    "https://example.invalid/v1".to_string(),
                )]
                .into_iter()
                .collect(),
            },
        };
        let with_uv = |tool: &str| -> Option<PathBuf> {
            (tool == "uvx").then(|| PathBuf::from("/opt/uv/bin/uvx"))
        };
        let without_uv = |_tool: &str| -> Option<PathBuf> { None };
        assert_eq!(
            fast.plan_in(Some(&registry), Some("linux-x86_64"), None, &with_uv),
            Ok(LaunchPlan::Uvx {
                version: "0.10.1".to_string()
            })
        );
        assert_eq!(
            fast.plan_in(Some(&registry), Some("linux-x86_64"), None, &without_uv),
            Err(LaunchError::NeedsUv),
            "uv が無ければ案内する（入れない）"
        );
        let command = fast
            .command_with(
                PathBuf::from("/tmp"),
                Some(&registry),
                Some("linux-x86_64"),
                &|tool| Ok(with_uv(tool)),
            )
            .expect("uv があれば組める");
        assert_eq!(command.path, PathBuf::from("/opt/uv/bin/uvx"));
        assert_eq!(
            command.args,
            vec!["fast-agent-acp==0.10.1".to_string(), "-x".to_string()]
        );
        assert_eq!(
            command.env.get("FAST_AGENT_MODEL").map(String::as_str),
            Some("codexplan")
        );
        assert_eq!(
            command.env.get("OPENAI_BASE_URL").map(String::as_str),
            Some("https://example.invalid/v1")
        );
        let Err(error) = fast.command_with(
            PathBuf::from("/tmp"),
            Some(&registry),
            Some("linux-x86_64"),
            &|tool| Ok(without_uv(tool)),
        ) else {
            panic!("uv が無ければ組めない");
        };
        assert_eq!(
            error.downcast_ref::<LaunchError>(),
            Some(&LaunchError::NeedsUv)
        );

        // リモート（platform = None）では binary を使わない。
        let amp = CustomAgent {
            id: "amp-acp".to_string(),
            label: "Amp".to_string(),
            launch: CustomLaunch::Registry {
                env: BTreeMap::new(),
            },
        };
        let Err(error) =
            amp.command_with(PathBuf::from("/tmp"), Some(&registry), None, &|_| Ok(None))
        else {
            panic!("binary だけの物はリモートで組めない");
        };
        assert_eq!(
            error.downcast_ref::<LaunchError>(),
            Some(&LaunchError::BinaryOnRemote)
        );
    }

    /// 起動の本番の確かめ: 偽のエージェント（python の ACP サーバ）を「自分のコマンド」として足し、
    /// `initialize` → `session/new` まで通す（プロンプトは送らない）。
    #[test]
    fn a_custom_agent_opens_a_session() {
        let Some(python) = find_in_path("python3") else {
            eprintln!("python3 が PATH に無いためスキップ");
            return;
        };
        let script = r#"
import json, sys
sys.stdin.reconfigure(encoding='utf-8')
sys.stdout.reconfigure(encoding='utf-8')
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    rid = msg.get("id")
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": 1}}), flush=True)
    elif method == "session/new":
        print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "custom-1"}}), flush=True)
"#;
        let catalog = AgentCatalog::new(
            vec![CustomAgentSpec {
                id: "fake".to_string(),
                name: Some("Fake Harness".to_string()),
                launch: CustomLaunch::Command {
                    command: python.to_string_lossy().into_owned(),
                    args: vec!["-c".to_string(), script.to_string()],
                    env: BTreeMap::new(),
                },
            }],
            None,
        );
        let agent = catalog.by_label("Fake Harness").expect("在る");
        let cwd = std::env::temp_dir();
        let resolved = agent
            .resolve_command_on(LocalHost::shared().as_ref(), cwd, None, None)
            .expect("解決できる")
            .expect("組める");
        let opened = futures::executor::block_on(crate::probe_session(
            &resolved,
            // 期限は時間切れで落ちないための上限で、通れば着いた時点で返る。Windows の CI では python の
            // 起動が並行するテストと重なって 10 秒を超え、2 回落ちた（2026-10-02・10-06）。
            std::time::Duration::from_secs(60),
        ));
        assert!(opened, "initialize → session/new まで通る");
    }
}
