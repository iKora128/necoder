//! custom — 設定で足したエージェント（H1）と、組み込みとまとめた「選べるエージェント」の一覧。
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

use crate::{find_in_path, AgentCommand, AgentKind, AgentOverride, AGENTS};
use anyhow::Result;
use host::Host;
use std::collections::BTreeMap;
use std::path::PathBuf;
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
    /// 自分のコマンドが手元で見つかるか（設定画面の説明用・PATH を見るだけで子プロセスは起こさない）。
    /// 絶対パスもここで当たる（PATH の各ディレクトリと join すると絶対パスはそのまま残る）。
    pub fn command_found(&self) -> bool {
        match &self.launch {
            CustomLaunch::Command { command, .. } => find_in_path(command).is_some(),
        }
    }

    /// 起動コマンドを組む。手元は PATH から探し、見つからなければ書いたまま渡す（ユーザーの指定を
    /// 勝手に捨てない＝起動の失敗として原因が見える）。リモートは探索をリモートに任せる。
    fn command_on(&self, host: &dyn Host, cwd: PathBuf) -> AgentCommand {
        match &self.launch {
            CustomLaunch::Command { command, args, env } => {
                let path = if host.is_remote() {
                    PathBuf::from(command)
                } else {
                    find_in_path(command).unwrap_or_else(|| PathBuf::from(command))
                };
                let mut resolved = AgentCommand::new(path, args.clone(), cwd);
                resolved.env = env.clone();
                resolved
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
            Agent::Custom(custom) => Ok(Some(custom.command_on(host, cwd.into()))),
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
    pub fn new(specs: Vec<CustomAgentSpec>) -> Self {
        let mut taken: Vec<String> = AGENTS.iter().map(|agent| agent.label.to_string()).collect();
        let mut customs = Vec::new();
        for spec in specs {
            if spec.id.trim().is_empty() || AGENTS.iter().any(|agent| agent.id == spec.id) {
                continue;
            }
            let base = spec
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(spec.id.as_str())
                .to_string();
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

    /// H1: `agent_servers` の新しい id は一覧に並ぶ。名前が無ければ id、組み込みの id は上書き
    /// （一覧を増やさない）、組み込みと同じ名前はずらす。
    #[test]
    fn custom_agents_join_the_list() {
        let catalog = AgentCatalog::new(vec![
            command("dsh", Some("DeepSeek Harness"), "dsh-acp"),
            command("pi", None, "pi-acp"),
            command("codex", Some("別の Codex"), "codex-acp"),
            command("my-codex", Some("Codex"), "codex-acp"),
            command("blank", Some("  "), "blank-acp"),
        ]);
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

    #[test]
    fn a_custom_agent_gets_a_monogram_from_its_name() {
        let catalog = AgentCatalog::new(vec![command("dsh", Some("DeepSeek Harness"), "dsh-acp")]);
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
        let catalog = AgentCatalog::new(vec![command(
            "dsh",
            Some("DeepSeek Harness"),
            "definitely-not-a-real-agent-xyz",
        )]);
        let agent = catalog.by_id("dsh").expect("在る");
        let resolved = agent
            .resolve_command_on(LocalHost::shared().as_ref(), "/tmp", None, None)
            .expect("手元は探索に失敗しない")
            .expect("足した物は必ず組める");
        assert!(resolved.path.ends_with("definitely-not-a-real-agent-xyz"));
        assert_eq!(resolved.args, vec!["--acp".to_string()]);
        assert_eq!(
            resolved.env.get("DSH_MODEL").map(String::as_str),
            Some("fast")
        );
        assert_eq!(resolved.cwd, PathBuf::from("/tmp"));
        assert!(!agent.custom().expect("足した物").command_found());
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
        let catalog = AgentCatalog::new(vec![CustomAgentSpec {
            id: "fake".to_string(),
            name: Some("Fake Harness".to_string()),
            launch: CustomLaunch::Command {
                command: python.to_string_lossy().into_owned(),
                args: vec!["-c".to_string(), script.to_string()],
                env: BTreeMap::new(),
            },
        }]);
        let agent = catalog.by_label("Fake Harness").expect("在る");
        let cwd = std::env::temp_dir();
        let resolved = agent
            .resolve_command_on(LocalHost::shared().as_ref(), cwd, None, None)
            .expect("解決できる")
            .expect("組める");
        let opened = futures::executor::block_on(crate::probe_session(
            &resolved,
            std::time::Duration::from_secs(10),
        ));
        assert!(opened, "initialize → session/new まで通る");
    }
}
