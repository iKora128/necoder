//! registry — ACP の**公開エージェントレジストリ**を読む（版のハードコードをやめるための層）。
//!
//! `const AGENTS`（[`crate::AGENTS`]）にパッケージ版を焼き込むと、エージェントを 1 つ上げるたびに
//! necoder のリリースが要る。実際 codex-acp は 1.1.14 のまま止まり、その間に upstream は 1.8 まで
//! 進んでいた（2026-09-02 実測）。レジストリはこの依存を切る。
//!
//! 取得先は ACP プロジェクト自身が出している**ベンダー中立の公開 CDN 成果物**:
//! `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`。
//! 特定エディタのものではないので necoder がそのまま読める（2026-09-02 時点で 39 エージェント）。
//!
//! **設計の出所**: レジストリを持つ・ディスクキャッシュを先に読む・ネットワークは間隔を空ける、
//! という構成は Zed の外部エージェント実装を**設計として比較参照**した。実装は公開スキーマと
//! 公開 URL からこの crate の型に合わせて独立に組み立てている（境界は
//! `docs/research/acp-agent-registry-notes.md`）。
//!
//! ネットワークは `curl` に委ねる＝依存ゼロ（`workspace::updater` と同じ流儀）。

use anyhow::{Context as _, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

/// 取得先。`NECODER_ACP_REGISTRY_URL` で差し替え可（テスト・社内ミラー用）。
pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// 再取得の下限間隔。これより新しいキャッシュはネットワークを見ない。
const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// 取得のタイムアウト（秒）。起動や設定画面を待たせないための上限。
const FETCH_TIMEOUT_SECONDS: &str = "30";

/// レジストリ 1 件ぶんのエージェント。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub distribution: Distribution,
    /// ブランドアイコンの URL（レジストリが配る svg）。necoder は在庫の svg を優先する。
    pub icon: Option<String>,
}

/// 起動方法。`binary` と `npx` の両方を持つ項目があるので、**解決時に選ぶ**（[`RegistryAgent::launch`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distribution {
    /// プラットフォーム別のネイティブバイナリ（`darwin-aarch64` 等がキー）。
    pub binary: BTreeMap<String, BinaryDistribution>,
    /// npm パッケージ経由（node/npx が要る）。
    pub npx: Option<NpxDistribution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryDistribution {
    /// 書庫の URL（`.tar.gz` / `.zip` 等）。
    pub archive: String,
    /// 展開後に起動するコマンド（`./amp-acp` のような相対パス）。
    pub cmd: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// 書庫の sha256（レジストリが載せていれば検証に使う）。
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpxDistribution {
    /// `pkg@1.2.3` 形式。npx へ渡す前に [`crate::bounded_npm_spec`] で上限範囲へ直す。
    pub package: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// 解決済みの起動方法。**この crate は npx 経路だけを起動できる**。
/// binary 経路は「在ることは分かるが、まだ配備実装が無い」を型で表す（黙って落とさない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Npx(NpxDistribution),
    /// このプラットフォーム向けのネイティブバイナリが在るが、配備（DL/検証/展開）は未実装。
    BinaryNotSupportedYet(BinaryDistribution),
}

/// レジストリ全体。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    /// レジストリ自身のスキーマ版（`1.0.0`）。
    pub version: String,
    pub agents: Vec<RegistryAgent>,
}

impl Registry {
    pub fn agent(&self, id: &str) -> Option<&RegistryAgent> {
        self.agents.iter().find(|agent| agent.id == id)
    }
}

impl RegistryAgent {
    /// このマシンでの起動方法を決める。**ネイティブバイナリが在ればそちらが優先**
    /// （node に依存しないため）。無ければ npx。どちらも無ければ `None`。
    pub fn launch(&self) -> Option<Launch> {
        if let Some(platform) = platform_key() {
            if let Some(binary) = self.distribution.binary.get(platform) {
                return Some(Launch::BinaryNotSupportedYet(binary.clone()));
            }
        }
        self.distribution.npx.clone().map(Launch::Npx)
    }

    /// 起動に node/npx が要るか（設定画面の説明用）。
    pub fn needs_node(&self) -> bool {
        matches!(self.launch(), Some(Launch::Npx(_)))
    }
}

/// このマシンのレジストリ用プラットフォームキー（`darwin-aarch64` 等）。未知なら `None`。
///
/// レジストリの表記は `<os>-<arch>` で、arch は Rust の `target_arch` と同じ語彙
/// （`aarch64` / `x86_64`）。`macos` は `darwin` へ寄せる。
pub fn platform_key() -> Option<&'static str> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    // `&'static str` を返すため組み合わせを列挙する（format! だと所有権が要る）。
    Some(match (os, arch) {
        ("darwin", "aarch64") => "darwin-aarch64",
        ("darwin", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => return None,
    })
}

/// レジストリ JSON をパースする（IO 無し＝テスト可能）。
///
/// **1 件の不備で全体を捨てない** — `id` / `version` / 起動方法のどれかを欠く項目だけ黙って
/// 落とす。レジストリは他人が更新するので、知らないキーや壊れた項目が混ざる前提で読む。
pub fn parse(json: &str) -> Result<Registry> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("ACP レジストリの JSON を読めない")?;
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let agents = value
        .get("agents")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(parse_agent).collect())
        .unwrap_or_default();
    Ok(Registry { version, agents })
}

fn parse_agent(value: &serde_json::Value) -> Option<RegistryAgent> {
    let id = value.get("id")?.as_str()?.to_string();
    let version = value.get("version")?.as_str()?.to_string();
    let distribution = parse_distribution(value.get("distribution")?)?;
    Some(RegistryAgent {
        name: value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&id)
            .to_string(),
        description: value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        icon: value
            .get("icon")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        id,
        version,
        distribution,
    })
}

fn parse_distribution(value: &serde_json::Value) -> Option<Distribution> {
    let binary: BTreeMap<String, BinaryDistribution> = value
        .get("binary")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(platform, entry)| Some((platform.clone(), parse_binary(entry)?)))
                .collect()
        })
        .unwrap_or_default();
    let npx = value.get("npx").and_then(parse_npx);
    // どちらの起動方法も無い項目は使えない（`fast-agent` のように distribution が空のものが在る）。
    if binary.is_empty() && npx.is_none() {
        return None;
    }
    Some(Distribution { binary, npx })
}

fn parse_binary(value: &serde_json::Value) -> Option<BinaryDistribution> {
    Some(BinaryDistribution {
        archive: value.get("archive")?.as_str()?.to_string(),
        cmd: value.get("cmd")?.as_str()?.to_string(),
        args: string_array(value.get("args")),
        env: string_map(value.get("env")),
        sha256: value
            .get("sha256")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn parse_npx(value: &serde_json::Value) -> Option<NpxDistribution> {
    Some(NpxDistribution {
        package: value.get("package")?.as_str()?.to_string(),
        args: string_array(value.get("args")),
        env: string_map(value.get("env")),
    })
}

fn string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn string_map(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    value
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(key, item)| Some((key.clone(), item.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// キャッシュファイルの場所（`paths` が決める）。
pub fn cache_path() -> Option<PathBuf> {
    paths::acp_registry_cache()
}

/// ディスクキャッシュを読む。無ければ `None`（起動時はまずこれだけを見る＝ネットワークを待たない）。
pub fn load_cached() -> Option<Registry> {
    let path = cache_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    parse(&text).ok()
}

/// キャッシュが [`REFRESH_INTERVAL`] より古い（または無い）か。
pub fn is_stale() -> bool {
    let Some(path) = cache_path() else {
        return false; // 置き場が無い＝キャッシュできないので取りに行かない
    };
    let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
        return true; // 未取得
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age >= REFRESH_INTERVAL)
        .unwrap_or(true) // 時計が巻き戻った＝古い扱いにして取り直す
}

/// レジストリを取得してキャッシュへ書く（**背景で呼ぶこと**。blocking + ネットワーク）。
///
/// 書き込みは同ディレクトリの一時ファイル経由で置き換える。途中で落ちても、壊れた JSON が
/// キャッシュとして残らない（次回の `load_cached` が黙って空を返すのを防ぐ）。
pub fn fetch_and_cache() -> Result<Registry> {
    let url =
        std::env::var("NECODER_ACP_REGISTRY_URL").unwrap_or_else(|_| REGISTRY_URL.to_string());
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            FETCH_TIMEOUT_SECONDS,
            "-H",
            "User-Agent: necoder",
            &url,
        ])
        .output()
        .context("curl を実行できない")?;
    anyhow::ensure!(
        output.status.success(),
        "ACP レジストリを取得できない: {url}"
    );
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    // 先にパースを通す＝壊れた応答をキャッシュへ残さない。
    let registry = parse(&text)?;
    if let Some(path) = cache_path() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("レジストリのキャッシュ先を作れない")?;
        }
        let staging = path.with_extension("json.tmp");
        std::fs::write(&staging, &text).context("レジストリのキャッシュを書けない")?;
        std::fs::rename(&staging, &path).context("レジストリのキャッシュを置き換えられない")?;
    }
    Ok(registry)
}

/// 古ければ取り直す。取れなければ**キャッシュを返す**（ネットワークが無くても動く）。
pub fn refresh_if_stale() -> Option<Registry> {
    if is_stale() {
        if let Ok(registry) = fetch_and_cache() {
            return Some(registry);
        }
    }
    load_cached()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実物のレジストリと同じ形の最小 JSON（npx 形 / binary 形 / 壊れた項目）。
    const SAMPLE: &str = r#"{
      "version": "1.0.0",
      "agents": [
        {
          "id": "claude-acp", "name": "Claude Code", "version": "0.73.0",
          "description": "…",
          "distribution": { "npx": { "package": "@agentclientprotocol/claude-agent-acp@0.73.0" } },
          "icon": "https://example.invalid/claude.svg"
        },
        {
          "id": "amp-acp", "name": "Amp", "version": "0.9.0", "description": "…",
          "distribution": { "binary": {
            "darwin-aarch64": { "archive": "https://example.invalid/a.tar.gz", "cmd": "./amp-acp",
                                "sha256": "abc" },
            "linux-x86_64":   { "archive": "https://example.invalid/b.tar.gz", "cmd": "./amp-acp" }
          } }
        },
        { "id": "no-distribution", "version": "1.0.0", "distribution": {} },
        { "name": "id が無い", "version": "1.0.0", "distribution": { "npx": { "package": "x@1" } } }
      ]
    }"#;

    #[test]
    fn parses_both_distribution_shapes() {
        let registry = parse(SAMPLE).expect("パースできる");
        assert_eq!(registry.version, "1.0.0");
        // 起動方法の無い項目と id の無い項目は落ちる＝2 件だけ残る。
        assert_eq!(registry.agents.len(), 2);

        let claude = registry.agent("claude-acp").expect("claude-acp が在る");
        assert_eq!(claude.name, "Claude Code");
        assert!(claude.distribution.binary.is_empty());
        assert_eq!(
            claude
                .distribution
                .npx
                .as_ref()
                .map(|npx| npx.package.as_str()),
            Some("@agentclientprotocol/claude-agent-acp@0.73.0")
        );

        let amp = registry.agent("amp-acp").expect("amp-acp が在る");
        assert_eq!(amp.distribution.binary.len(), 2);
        assert_eq!(
            amp.distribution.binary["darwin-aarch64"].sha256.as_deref(),
            Some("abc")
        );
        // sha256 が無いプラットフォームも落とさない（検証を省くだけ）。
        assert!(amp.distribution.binary["linux-x86_64"].sha256.is_none());
    }

    #[test]
    fn npx_agent_needs_node_binary_agent_does_not() {
        let registry = parse(SAMPLE).expect("パースできる");
        assert!(registry.agent("claude-acp").expect("在る").needs_node());
        // binary が在るプラットフォームでは node を要求しない。
        if platform_key().is_some_and(|key| key == "darwin-aarch64" || key == "linux-x86_64") {
            assert!(!registry.agent("amp-acp").expect("在る").needs_node());
        }
    }

    #[test]
    fn broken_json_is_an_error_not_a_panic() {
        assert!(parse("{ not json").is_err());
        // agents が無い JSON は「空のレジストリ」＝エラーにしない（スキーマ拡張に強くする）。
        let empty = parse(r#"{"version":"1.0.0"}"#).expect("読める");
        assert!(empty.agents.is_empty());
    }

    #[test]
    fn platform_key_is_known_on_supported_targets() {
        // CI が回る 3 OS × 2 arch では必ず解決する。
        if matches!(std::env::consts::OS, "macos" | "linux" | "windows")
            && matches!(std::env::consts::ARCH, "aarch64" | "x86_64")
        {
            assert!(platform_key().is_some());
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// キャッシュ済みの**実物**レジストリを通しでパースする（形が変わったら気づくため）。
    ///
    /// 中身は他人が更新するので、**特定の id や件数は当てにしない**（当てにすると
    /// upstream がエージェントを 1 つ消しただけで CI が赤くなる）。見るのは
    /// 「読めること・1 件以上あること・どの項目も起動方法を持つこと」の 3 つだけ。
    /// キャッシュが無い環境（CI 初回など）は黙って skip する。
    #[test]
    fn cached_registry_still_parses() {
        let Some(path) = cache_path() else { return };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let registry = parse(&text).expect("実物のレジストリをパースできる");
        assert!(
            !registry.agents.is_empty(),
            "エージェントが 1 件も読めていない（スキーマが変わった可能性）"
        );
        for agent in &registry.agents {
            assert!(!agent.id.is_empty(), "id の無い項目が残っている");
            assert!(
                agent.distribution.npx.is_some() || !agent.distribution.binary.is_empty(),
                "{}: 起動方法の無い項目が落とされていない",
                agent.id
            );
        }
    }
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    #[test]
    #[ignore = "ネットワークへ出る（cdn.agentclientprotocol.com）"]
    fn fetches_and_caches_the_real_registry() {
        let registry = fetch_and_cache().expect("レジストリを取得できる");
        println!(
            "  取得: version={} / {} agents",
            registry.version,
            registry.agents.len()
        );
        assert!(!registry.agents.is_empty());
        // キャッシュが書けていて、読み戻せる。
        let cached = load_cached().expect("キャッシュを読み戻せる");
        assert_eq!(cached.agents.len(), registry.agents.len());
        // 取り立てなので stale ではない。
        assert!(!is_stale(), "取得直後に stale 判定されている");
        println!("  キャッシュ: {}", cache_path().expect("path").display());
    }
}
