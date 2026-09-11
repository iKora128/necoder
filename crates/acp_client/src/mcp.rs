//! mcp — ACP セッションへ渡す **MCP サーバ一覧**の解決層（`session/new` / `session/load` の `mcpServers`）。
//!
//! ACP では「どの MCP サーバへ繋ぐか」を決めるのは**クライアント**（＝necoder）で、エージェント
//! 側の設定ファイル（`~/.codex/config.toml` 等）はセッションに現れない。necoder が渡さない限り
//! エージェントからは 1 つも見えない — 「Codex CLI に登録したのに necoder のスレッドからは
//! `not installed or connected` になる」の原因はここが空だったこと（2026-09-10 の実測）。
//!
//! **真実は necoder の `settings.json` の `mcp_servers`**。ただし他ツール（Codex CLI / Claude Code /
//! Cursor）へ既に登録済みのサーバを**発見**して同じ一覧に載せる（登録し直しを強いない）。
//! 発見したものは**既定 off** — 他人の設定ファイルを根拠に子プロセスを起こしたり、課金される
//! リモートサーバへ繋いだりしない。有効化は明示（設定画面のトグル＝`mcp_servers.<name>.enabled`）だけ。
//!
//! この crate は設定スキーマ（`settings_core`）を知らない。呼び手が [`McpServerOverride`] へ
//! 写して渡す（`AgentOverride` と同じ流儀＝依存方向を増やさない）。

use acp::schema::v1;
use agent_client_protocol as acp;
use std::collections::BTreeMap;
use std::path::Path;

/// この定義がどこから来たか（設定画面の出所表示と、既定 on/off の判断に使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum McpSource {
    /// necoder の `settings.json`（`mcp_servers.<name>` に伝送方式まで書いてある）。
    Necoder,
    /// `~/.codex/config.toml` の `[mcp_servers.<name>]`。
    Codex,
    /// `~/.claude.json` の `mcpServers`。
    ClaudeCode,
    /// `~/.cursor/mcp.json` の `mcpServers`。
    Cursor,
}

impl McpSource {
    /// 出所の識別子（UI の i18n キー組み立てと、テストの照合に使う短い語）。
    pub fn id(self) -> &'static str {
        match self {
            McpSource::Necoder => "necoder",
            McpSource::Codex => "codex",
            McpSource::ClaudeCode => "claude",
            McpSource::Cursor => "cursor",
        }
    }

    /// necoder 自身の設定に書かれたものか（＝既定で有効にしてよいか）。
    pub fn is_own(self) -> bool {
        matches!(self, McpSource::Necoder)
    }
}

/// MCP サーバへの繋ぎ方（ACP の `McpServer` に 1:1 で対応する necoder 側の写し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransport {
    /// 子プロセスを起こして stdio で話す。**全エージェントが必ず対応する**方式。
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// HTTP（streamable）。エージェントが `mcpCapabilities.http` を広告した時だけ渡せる。
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
    /// SSE。エージェントが `mcpCapabilities.sse` を広告した時だけ渡せる。
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl McpTransport {
    /// 設定画面の副題に出す 1 行（`stdio: npx …` / `http: https://…`）。
    pub fn summary(&self) -> String {
        match self {
            McpTransport::Stdio { command, args, .. } => {
                if args.is_empty() {
                    format!("stdio: {command}")
                } else {
                    format!("stdio: {command} {}", args.join(" "))
                }
            }
            McpTransport::Http { url, .. } => format!("http: {url}"),
            McpTransport::Sse { url, .. } => format!("sse: {url}"),
        }
    }

    /// 伝送方式の名前（スキップ理由の説明に使う）。
    pub fn kind(&self) -> &'static str {
        match self {
            McpTransport::Stdio { .. } => "stdio",
            McpTransport::Http { .. } => "http",
            McpTransport::Sse { .. } => "sse",
        }
    }
}

/// バラバラの材料（伝送方式の宣言・コマンド・URL）から [`McpTransport`] を決める。
///
/// necoder の `settings.json` も他ツールの設定も同じ形（`type`/`transport` は任意で、無ければ
/// `url` の有無で決まる）なので、**解釈は 1 箇所に置く** — 設定画面に出ている行と、他ツールから
/// 発見した行が違う規則で読まれると、同じ書き方が場所によって別の意味になる。
/// 材料が足りない（`command` も `url` も無い）場合は `None` ＝「有効/無効だけを決める行」。
pub fn transport_from_parts(
    declared: Option<&str>,
    command: Option<&str>,
    args: &[String],
    env: &BTreeMap<String, String>,
    url: Option<&str>,
    headers: &BTreeMap<String, String>,
) -> Option<McpTransport> {
    let stdio = || {
        command.map(|command| McpTransport::Stdio {
            command: command.to_string(),
            args: args.to_vec(),
            env: env.clone(),
        })
    };
    let over_url = |sse: bool| {
        url.map(|url| {
            let url = url.to_string();
            let headers = headers.clone();
            if sse {
                McpTransport::Sse { url, headers }
            } else {
                McpTransport::Http { url, headers }
            }
        })
    };
    match declared {
        Some("stdio") => stdio(),
        Some("sse") => over_url(true),
        // Cursor は `streamableHttp`、他は `http`。どれも同じ streamable HTTP を指す。
        Some("http" | "streamable-http" | "streamableHttp") => over_url(false),
        // 宣言が無い（または知らない綴り）なら url があれば http、無ければ stdio。
        _ => over_url(false).or_else(stdio),
    }
}

/// 解決済みの MCP サーバ 1 件。設定画面はこれを並べ、セッションはこれを ACP へ写して渡す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub source: McpSource,
    /// このセッションで実際にエージェントへ渡すか。
    pub enabled: bool,
    pub transport: McpTransport,
}

/// `settings.json` の `mcp_servers.<name>` から来る 1 件。
///
/// `transport` が `Some` なら **necoder 自身の定義**、`None` なら「他ツールで発見したサーバの
/// on/off だけを決める行」。この 2 態を型で分けることで、「伝送方式は他所・有効判断だけこちら」
/// という中途半端な定義を作らせない（`AgentServerSetting` の custom/registry と同じ考え方）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerOverride {
    /// 明示的な有効/無効。`None` = 出所ごとの既定（necoder 定義は on・発見は off）。
    pub enabled: Option<bool>,
    pub transport: Option<McpTransport>,
}

/// エージェントへ渡せなかった理由（UI に「黙って落ちた」を作らないための型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSkipReason {
    /// エージェントがこの伝送方式を広告していない（`mcpCapabilities`）。
    UnsupportedTransport {
        transport: &'static str,
        agent_supports_http: bool,
        agent_supports_sse: bool,
    },
    /// リモートセッションに local の stdio サーバは渡せない（コマンドは接続元のパス）。
    RemoteStdio,
    /// `${VAR}` が未設定で、ヘッダ/環境変数を組み立てられない。
    MissingEnv(String),
}

impl McpSkipReason {
    /// ログ・UI 用の 1 行（i18n は呼び手が持つ。ここは診断文の素材だけを返す）。
    pub fn describe(&self) -> String {
        match self {
            McpSkipReason::UnsupportedTransport {
                transport,
                agent_supports_http,
                agent_supports_sse,
            } => format!(
                "エージェントが {transport} を広告していません（http={agent_supports_http} / sse={agent_supports_sse}）"
            ),
            McpSkipReason::RemoteStdio => {
                "リモートセッションでは stdio サーバ（接続元のコマンド）を渡せません".to_string()
            }
            McpSkipReason::MissingEnv(name) => format!("環境変数 {name} が未設定です"),
        }
    }
}

/// 設定（`mcp_servers`）と発見結果を突き合わせて、一覧を確定する。
///
/// - necoder 定義（`transport` あり）は同名の発見結果を**置き換える**（真実は necoder 側）
/// - 発見したサーバは `enabled` を明示された時だけ on
/// - 並び順は名前順（設定画面の行が入れ替わらない）
pub fn resolve(
    overrides: &BTreeMap<String, McpServerOverride>,
    discovered: Vec<McpServerConfig>,
) -> Vec<McpServerConfig> {
    let mut resolved: BTreeMap<String, McpServerConfig> = BTreeMap::new();
    for server in discovered {
        // 同名が複数の出所に居る場合は先に見つけた方を残す（`discover_in` の順＝Codex → Claude → Cursor）。
        resolved.entry(server.name.clone()).or_insert(server);
    }
    for (name, setting) in overrides {
        match &setting.transport {
            Some(transport) => {
                resolved.insert(
                    name.clone(),
                    McpServerConfig {
                        name: name.clone(),
                        source: McpSource::Necoder,
                        enabled: setting.enabled.unwrap_or(true),
                        transport: transport.clone(),
                    },
                );
            }
            // 伝送方式を持たない行は「発見済みサーバの on/off」。該当が無ければ黙って無視する
            // （他マシンの settings.json をそのまま持ってきても壊れない）。
            None => {
                if let Some(server) = resolved.get_mut(name) {
                    if let Some(enabled) = setting.enabled {
                        server.enabled = enabled;
                    }
                }
            }
        }
    }
    resolved.into_values().collect()
}

/// 有効なサーバを ACP の型へ写す。渡せなかったものは `(名前, 理由)` で返す（黙って落とさない）。
///
/// `remote` はセッションのホストがリモートか。リモートでは stdio を落とす — コマンドは接続元
/// （このマシン）のパスで、リモートのエージェントが起動しても別物か存在しないかのどちらかになる。
pub fn to_acp_servers(
    servers: &[McpServerConfig],
    capabilities: &v1::McpCapabilities,
    remote: bool,
) -> (Vec<v1::McpServer>, Vec<(String, McpSkipReason)>) {
    let mut passed = Vec::new();
    let mut skipped = Vec::new();
    for server in servers.iter().filter(|server| server.enabled) {
        match to_acp_server(server, capabilities, remote) {
            Ok(acp_server) => passed.push(acp_server),
            Err(reason) => skipped.push((server.name.clone(), reason)),
        }
    }
    (passed, skipped)
}

fn to_acp_server(
    server: &McpServerConfig,
    capabilities: &v1::McpCapabilities,
    remote: bool,
) -> Result<v1::McpServer, McpSkipReason> {
    let unsupported = |transport: &'static str| McpSkipReason::UnsupportedTransport {
        transport,
        agent_supports_http: capabilities.http,
        agent_supports_sse: capabilities.sse,
    };
    match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            if remote {
                return Err(McpSkipReason::RemoteStdio);
            }
            let env = expand_pairs(env)?
                .into_iter()
                .map(|(name, value)| v1::EnvVariable::new(name, value))
                .collect();
            Ok(v1::McpServer::Stdio(
                v1::McpServerStdio::new(server.name.clone(), expand(command)?)
                    .args(expand_all(args)?)
                    .env(env),
            ))
        }
        McpTransport::Http { url, headers } => {
            if !capabilities.http {
                return Err(unsupported("http"));
            }
            let headers = expand_pairs(headers)?
                .into_iter()
                .map(|(name, value)| v1::HttpHeader::new(name, value))
                .collect();
            Ok(v1::McpServer::Http(
                v1::McpServerHttp::new(server.name.clone(), expand(url)?).headers(headers),
            ))
        }
        McpTransport::Sse { url, headers } => {
            if !capabilities.sse {
                return Err(unsupported("sse"));
            }
            let headers = expand_pairs(headers)?
                .into_iter()
                .map(|(name, value)| v1::HttpHeader::new(name, value))
                .collect();
            Ok(v1::McpServer::Sse(
                v1::McpServerSse::new(server.name.clone(), expand(url)?).headers(headers),
            ))
        }
    }
}

/// `${VAR}` を環境変数で置き換える。**未設定なら `Err`** — 展開できなかった文字列を
/// そのまま送ると `Authorization: Bearer ${TOKEN}` のような嘘のヘッダで繋ぎに行くことになる。
fn expand(value: &str) -> Result<String, McpSkipReason> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            // 閉じていない `${` はただの文字列として扱う（設定に書かれた通りに送る）。
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let name = &after[..end];
        let resolved =
            std::env::var(name).map_err(|_| McpSkipReason::MissingEnv(name.to_string()))?;
        out.push_str(&resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn expand_all(values: &[String]) -> Result<Vec<String>, McpSkipReason> {
    values.iter().map(|value| expand(value)).collect()
}

fn expand_pairs(pairs: &BTreeMap<String, String>) -> Result<Vec<(String, String)>, McpSkipReason> {
    pairs
        .iter()
        .map(|(name, value)| Ok((name.clone(), expand(value)?)))
        .collect()
}

// ── 他ツールの設定からの発見 ────────────────────────────────────────────────────

/// ホームディレクトリ配下の既知の設定から MCP サーバを発見する。
/// 読めないファイル・壊れたファイルは黙って飛ばす（他人の設定なので落とさない）。
pub fn discover() -> Vec<McpServerConfig> {
    paths::home_dir()
        .map(|home| discover_in(&home))
        .unwrap_or_default()
}

/// [`discover`] の本体（テストから任意のディレクトリを渡せるようにするため分けてある）。
pub fn discover_in(home: &Path) -> Vec<McpServerConfig> {
    let mut found = Vec::new();
    if let Ok(text) = std::fs::read_to_string(home.join(".codex").join("config.toml")) {
        found.extend(parse_codex_config(&text));
    }
    if let Ok(text) = std::fs::read_to_string(home.join(".claude.json")) {
        found.extend(parse_mcp_servers_json(&text, McpSource::ClaudeCode));
    }
    if let Ok(text) = std::fs::read_to_string(home.join(".cursor").join("mcp.json")) {
        found.extend(parse_mcp_servers_json(&text, McpSource::Cursor));
    }
    found
}

/// Codex CLI の `~/.codex/config.toml` から `[mcp_servers.<name>]` を読む。
///
/// 対応するキー: `command` / `args` / `env` / `url` / `headers`（`http_headers` も可）/
/// `bearer_token_env_var`（→ `Authorization: Bearer ${VAR}`）/ `enabled`。
/// `cwd` は ACP の `McpServerStdio` に対応する場所が無いので**読まない**（黙って別の意味にしない）。
pub fn parse_codex_config(text: &str) -> Vec<McpServerConfig> {
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    let Some(table) = value.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    table
        .iter()
        .filter_map(|(name, entry)| {
            // 向こうで off にしてあるものは一覧にも出さない（消したはずの登録が復活して見える方が困る）。
            if entry.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
                return None;
            }
            let transport = if let Some(url) = entry.get("url").and_then(toml::Value::as_str) {
                let mut headers = toml_string_map(entry.get("headers"));
                headers.extend(toml_string_map(entry.get("http_headers")));
                if let Some(variable) = entry
                    .get("bearer_token_env_var")
                    .and_then(toml::Value::as_str)
                {
                    headers
                        .entry("Authorization".to_string())
                        .or_insert_with(|| format!("Bearer ${{{variable}}}"));
                }
                McpTransport::Http {
                    url: url.to_string(),
                    headers,
                }
            } else {
                let command = entry.get("command").and_then(toml::Value::as_str)?;
                McpTransport::Stdio {
                    command: command.to_string(),
                    args: toml_string_array(entry.get("args")),
                    env: toml_string_map(entry.get("env")),
                }
            };
            Some(McpServerConfig {
                name: name.clone(),
                source: McpSource::Codex,
                enabled: false, // 発見したものは既定 off（有効化は necoder 側の明示だけ）
                transport,
            })
        })
        .collect()
}

/// Claude Code（`~/.claude.json`）/ Cursor（`~/.cursor/mcp.json`）の `mcpServers` を読む。
///
/// 伝送方式は `type` か `transport`（`stdio` / `http` / `sse`）。無ければ `url` の有無で決める。
/// `disabled: true` の項目は一覧に出さない（Codex の `enabled = false` と同じ扱い）。
pub fn parse_mcp_servers_json(text: &str, source: McpSource) -> Vec<McpServerConfig> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(table) = value.get("mcpServers").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    table
        .iter()
        .filter_map(|(name, entry)| {
            if entry.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
                return None;
            }
            let transport = transport_from_parts(
                entry
                    .get("type")
                    .or_else(|| entry.get("transport"))
                    .and_then(|v| v.as_str()),
                entry.get("command").and_then(|v| v.as_str()),
                &json_string_array(entry.get("args")),
                &json_string_map(entry.get("env")),
                entry.get("url").and_then(|v| v.as_str()),
                &json_string_map(entry.get("headers")),
            )?;
            Some(McpServerConfig {
                name: name.clone(),
                source,
                enabled: false,
                transport,
            })
        })
        .collect()
}

fn toml_string_array(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn toml_string_map(value: Option<&toml::Value>) -> BTreeMap<String, String> {
    value
        .and_then(toml::Value::as_table)
        .map(|table| {
            table
                .iter()
                .filter_map(|(key, item)| Some((key.clone(), item.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn json_string_array(value: Option<&serde_json::Value>) -> Vec<String> {
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

fn json_string_map(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    value
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(key, item)| Some((key.clone(), item.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実物と同じ形の Codex 設定（stdio / URL / off にしてあるもの / 起動方法の無いもの）。
    const CODEX_CONFIG: &str = r#"
model = "gpt-5"

[mcp_servers.pencil]
command = "/opt/pencil/mcp-server"
args = ["--app", "cursor"]

[mcp_servers.higgsfield]
url = "https://mcp.higgsfield.ai/mcp"

[mcp_servers.private]
url = "https://mcp.example.invalid/mcp"
bearer_token_env_var = "EXAMPLE_TOKEN"

[mcp_servers.computer-use]
command = "./SkyComputerUseClient"
enabled = false

[mcp_servers.broken]
startup_timeout_sec = 120
"#;

    fn stdio(command: &str) -> McpTransport {
        McpTransport::Stdio {
            command: command.to_string(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    fn find<'a>(servers: &'a [McpServerConfig], name: &str) -> &'a McpServerConfig {
        servers
            .iter()
            .find(|server| server.name == name)
            .unwrap_or_else(|| panic!("{name} が一覧に無い"))
    }

    #[test]
    fn reads_codex_stdio_and_url_servers() {
        let servers = parse_codex_config(CODEX_CONFIG);
        // off にしてあるものと、起動方法の無いものは落ちる。
        assert_eq!(servers.len(), 3, "{servers:?}");
        assert!(
            servers.iter().all(|server| !server.enabled),
            "発見は既定 off"
        );
        assert!(servers
            .iter()
            .all(|server| server.source == McpSource::Codex));

        assert_eq!(
            find(&servers, "pencil").transport,
            McpTransport::Stdio {
                command: "/opt/pencil/mcp-server".to_string(),
                args: vec!["--app".to_string(), "cursor".to_string()],
                env: BTreeMap::new(),
            }
        );
        assert_eq!(
            find(&servers, "higgsfield").transport,
            McpTransport::Http {
                url: "https://mcp.higgsfield.ai/mcp".to_string(),
                headers: BTreeMap::new(),
            }
        );
        // bearer_token_env_var は Authorization ヘッダの `${VAR}` へ畳む（値は展開時に解決）。
        let McpTransport::Http { headers, .. } = &find(&servers, "private").transport else {
            panic!("http のはず");
        };
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer ${EXAMPLE_TOKEN}")
        );
    }

    #[test]
    fn reads_claude_and_cursor_json_servers() {
        let json = r#"{
          "mcpServers": {
            "aws": { "type": "stdio", "command": "npx", "args": ["-y", "aws-mcp"], "env": {"A": "1"} },
            "remote": { "type": "http", "url": "https://example.invalid/mcp", "headers": {"X-Key": "v"} },
            "stream": { "type": "sse", "url": "https://example.invalid/sse" },
            "guessed": { "url": "https://example.invalid/guess" },
            "off": { "command": "x", "disabled": true }
          }
        }"#;
        let servers = parse_mcp_servers_json(json, McpSource::ClaudeCode);
        assert_eq!(servers.len(), 4, "{servers:?}");
        assert_eq!(
            find(&servers, "aws").transport,
            McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "aws-mcp".to_string()],
                env: BTreeMap::from([("A".to_string(), "1".to_string())]),
            }
        );
        assert!(matches!(
            find(&servers, "stream").transport,
            McpTransport::Sse { .. }
        ));
        // type が無くても url があれば http（Claude Code / Cursor の既定と同じ解釈）。
        assert!(matches!(
            find(&servers, "guessed").transport,
            McpTransport::Http { .. }
        ));
    }

    #[test]
    fn transport_inference_is_the_same_rule_everywhere() {
        let none = BTreeMap::new();
        let infer = |declared: Option<&str>, command: Option<&str>, url: Option<&str>| {
            transport_from_parts(declared, command, &[], &none, url, &none)
        };
        // 宣言が無ければ url 優先 → 無ければ command。
        assert!(matches!(
            infer(None, Some("npx"), Some("https://example.invalid")),
            Some(McpTransport::Http { .. })
        ));
        assert!(matches!(
            infer(None, Some("npx"), None),
            Some(McpTransport::Stdio { .. })
        ));
        // 宣言が優先（url があっても stdio と書いてあれば stdio）。
        assert!(matches!(
            infer(Some("stdio"), Some("npx"), Some("https://example.invalid")),
            Some(McpTransport::Stdio { .. })
        ));
        assert!(matches!(
            infer(Some("sse"), None, Some("https://example.invalid")),
            Some(McpTransport::Sse { .. })
        ));
        assert!(matches!(
            infer(
                Some("streamableHttp"),
                None,
                Some("https://example.invalid")
            ),
            Some(McpTransport::Http { .. })
        ));
        // 宣言と材料が食い違う（stdio なのに command が無い）は None＝定義として採らない。
        assert!(infer(Some("stdio"), None, Some("https://example.invalid")).is_none());
        // 材料が何も無い行は「有効/無効だけを決める行」。
        assert!(infer(None, None, None).is_none());
    }

    #[test]
    fn broken_files_yield_nothing_instead_of_panicking() {
        assert!(parse_codex_config("[[[ not toml").is_empty());
        assert!(parse_mcp_servers_json("{ not json", McpSource::Cursor).is_empty());
        // MCP を持たない設定は空（エラーにしない）。
        assert!(parse_codex_config("model = \"gpt-5\"").is_empty());
        assert!(parse_mcp_servers_json(r#"{"projects":{}}"#, McpSource::ClaudeCode).is_empty());
    }

    #[test]
    fn discovered_servers_stay_off_until_settings_say_otherwise() {
        let discovered = parse_codex_config(CODEX_CONFIG);
        let resolved = resolve(&BTreeMap::new(), discovered.clone());
        assert!(
            resolved.iter().all(|server| !server.enabled),
            "設定に無い発見サーバが勝手に有効になっている"
        );

        let overrides = BTreeMap::from([(
            "higgsfield".to_string(),
            McpServerOverride {
                enabled: Some(true),
                transport: None,
            },
        )]);
        let resolved = resolve(&overrides, discovered);
        assert!(find(&resolved, "higgsfield").enabled);
        assert!(!find(&resolved, "pencil").enabled);
        // 出所は発見元のまま（necoder 定義に化けない）。
        assert_eq!(find(&resolved, "higgsfield").source, McpSource::Codex);
    }

    #[test]
    fn own_definition_replaces_the_discovered_one_and_defaults_to_on() {
        let overrides = BTreeMap::from([
            (
                "pencil".to_string(),
                McpServerOverride {
                    enabled: None,
                    transport: Some(stdio("/usr/local/bin/pencil")),
                },
            ),
            (
                "muted".to_string(),
                McpServerOverride {
                    enabled: Some(false),
                    transport: Some(stdio("/usr/local/bin/muted")),
                },
            ),
        ]);
        let resolved = resolve(&overrides, parse_codex_config(CODEX_CONFIG));
        let pencil = find(&resolved, "pencil");
        assert_eq!(pencil.source, McpSource::Necoder);
        assert!(pencil.enabled, "necoder 自身の定義は既定 on");
        assert_eq!(pencil.transport, stdio("/usr/local/bin/pencil"));
        assert!(!find(&resolved, "muted").enabled);
        // 該当の無い on/off 行は黙って無視する（別マシンの settings.json を持ってきても壊れない）。
        let orphan = BTreeMap::from([(
            "nowhere".to_string(),
            McpServerOverride {
                enabled: Some(true),
                transport: None,
            },
        )]);
        assert!(resolve(&orphan, Vec::new()).is_empty());
    }

    fn capabilities(http: bool, sse: bool) -> v1::McpCapabilities {
        v1::McpCapabilities::new().http(http).sse(sse)
    }

    fn server(name: &str, transport: McpTransport) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            source: McpSource::Necoder,
            enabled: true,
            transport,
        }
    }

    #[test]
    fn stdio_always_passes_but_http_needs_the_advertised_capability() {
        let servers = vec![
            server("local", stdio("/usr/local/bin/mcp")),
            server(
                "remote",
                McpTransport::Http {
                    url: "https://example.invalid/mcp".to_string(),
                    headers: BTreeMap::new(),
                },
            ),
        ];
        let (passed, skipped) = to_acp_servers(&servers, &capabilities(false, false), false);
        assert_eq!(passed.len(), 1);
        assert!(matches!(passed[0], v1::McpServer::Stdio(_)));
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, "remote");
        assert!(matches!(
            skipped[0].1,
            McpSkipReason::UnsupportedTransport { .. }
        ));

        let (passed, skipped) = to_acp_servers(&servers, &capabilities(true, false), false);
        assert_eq!(passed.len(), 2);
        assert!(skipped.is_empty());
    }

    #[test]
    fn disabled_servers_are_not_sent_at_all() {
        let mut off = server("local", stdio("/usr/local/bin/mcp"));
        off.enabled = false;
        let (passed, skipped) = to_acp_servers(&[off], &capabilities(true, true), false);
        assert!(passed.is_empty());
        assert!(skipped.is_empty(), "無効な行はスキップ理由にも出さない");
    }

    #[test]
    fn remote_sessions_drop_local_stdio_servers() {
        let servers = vec![server("local", stdio("/usr/local/bin/mcp"))];
        let (passed, skipped) = to_acp_servers(&servers, &capabilities(true, true), true);
        assert!(passed.is_empty());
        assert_eq!(skipped[0].1, McpSkipReason::RemoteStdio);
    }

    #[test]
    fn missing_environment_variables_skip_the_server_instead_of_sending_the_literal() {
        let name = "NECODER_TEST_MCP_TOKEN_MISSING";
        std::env::remove_var(name);
        let servers = vec![server(
            "private",
            McpTransport::Http {
                url: "https://example.invalid/mcp".to_string(),
                headers: BTreeMap::from([(
                    "Authorization".to_string(),
                    format!("Bearer ${{{name}}}"),
                )]),
            },
        )];
        let (passed, skipped) = to_acp_servers(&servers, &capabilities(true, true), false);
        assert!(passed.is_empty());
        assert_eq!(skipped[0].1, McpSkipReason::MissingEnv(name.to_string()));
    }

    #[test]
    fn expands_environment_variables_in_headers() {
        let name = "NECODER_TEST_MCP_TOKEN_PRESENT";
        std::env::set_var(name, "secret-value");
        let expanded = expand(&format!("Bearer ${{{name}}}!")).expect("展開できる");
        std::env::remove_var(name);
        assert_eq!(expanded, "Bearer secret-value!");
        // 閉じていない `${` はそのまま文字列として扱う（設定に書かれた通り）。
        assert_eq!(expand("${UNCLOSED").expect("落ちない"), "${UNCLOSED");
    }

    #[test]
    fn discovery_reads_every_known_config_and_prefers_the_first_source() {
        let dir = std::env::temp_dir().join(format!("necoder_mcp_discover_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".codex")).expect("作れる");
        std::fs::create_dir_all(dir.join(".cursor")).expect("作れる");
        std::fs::write(dir.join(".codex").join("config.toml"), CODEX_CONFIG).expect("書ける");
        std::fs::write(
            dir.join(".claude.json"),
            r#"{"mcpServers":{"aws":{"command":"npx","args":["-y","aws-mcp"]}}}"#,
        )
        .expect("書ける");
        std::fs::write(
            dir.join(".cursor").join("mcp.json"),
            r#"{"mcpServers":{"pencil":{"command":"/other/pencil"}}}"#,
        )
        .expect("書ける");

        let resolved = resolve(&BTreeMap::new(), discover_in(&dir));
        assert_eq!(resolved.len(), 4, "{resolved:?}");
        assert_eq!(find(&resolved, "aws").source, McpSource::ClaudeCode);
        // 同名は先に読んだ出所（Codex）を残す。
        assert_eq!(find(&resolved, "pencil").source, McpSource::Codex);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
