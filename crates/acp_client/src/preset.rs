//! preset — **セッションの作り方のプリセット**（`session/new` / `session/load` の `_meta`）。
//!
//! Chat モード（`docs/CHAT.md` §5）は新しいチャットエンジンを持たない。エージェントは Editor の
//! スレッドと同じ `claude-agent-acp` で、違うのは**セッションの作り方だけ**: システムプロンプトを
//! 差し替え、持たせる道具を絞り、user / project / local の設定を持ち込ませない。Claude Code が
//! 「ガンガン実装する」のはモデルの性格ではなく、長いシステムプロンプトと Bash などの道具を全部
//! 持っていることの帰結なので、この 3 つを変えれば会話のアシスタントとして振る舞う
//! （DECISIONS 決定ログ 2026-09-18 の実験）。
//!
//! `_meta` の形は **`claude-agent-acp` 固有**で ACP の標準ではない（アダプタの `createSession` が
//! `params._meta.systemPrompt` と `params._meta.claudeCode.options` を読む。`loadSession` も同じ
//! 関数へ入るので、**再開時にも同じものを載せないと設定が抜ける**）。他のエージェントは知らない
//! キーを無視するだけなので害は無いが、効きもしない — だから Chat は当面 Claude 限定。
//!
//! この crate は「何を載せるか」の器だけを持つ。プロンプトの文面や道具の一覧を決めるのは呼び手
//! （`agent_panel`）で、ここは UI も設定スキーマも知らない（`mcp` と同じ流儀）。

use acp::schema::v1;
use agent_client_protocol as acp;
use std::collections::BTreeMap;

/// セッションに載せる `_meta` の中身。どの欄も `None` なら「エージェントの既定のまま」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionPreset {
    /// システムプロンプト。**文字列で渡すとアダプタの既定（`claude_code` プリセット）を丸ごと置換**する。
    pub system_prompt: Option<String>,
    /// 使える組み込みツールの一覧（`Read` / `Write` …）。空の `Vec` は「組み込みツール無し」。
    ///
    /// SDK の `tools` は**使用可能なツールの限定**で、`allowedTools`（自動承認の指定）とは別物。
    /// MCP のツールはここでは絞れない — 呼び手が `mcp_servers` を空にし、`setting_sources` も
    /// 閉じて初めて「持たせた道具だけ」になる。
    pub tools: Option<Vec<String>>,
    /// 読み込む設定の出所（`user` / `project` / `local`）。アダプタの既定は 3 つ全部で、ユーザーの
    /// `~/.claude` の追加指示・MCP・hooks がセッションへ入ってくる。空の `Vec` で全部閉じる。
    pub setting_sources: Option<Vec<String>>,
    /// `mcpServers` で渡した MCP サーバ**だけ**を使う（SDK の `strictMcpConfig`）。プロジェクトの
    /// `.mcp.json`・ユーザー設定・プラグインの MCP を無視させる。
    pub strict_mcp: bool,
    /// エージェントのプロセスへ足す環境変数。`_meta` では閉じられない持ち込みを止めるのに使う —
    /// **claude.ai アカウントのコネクタ**（Figma・Google Calendar 等）は `setting_sources` を空に
    /// しても `strict_mcp` を立てても自動で読み込まれ、実測で 170 個超のツール定義が最初のターンから
    /// 文脈を 11 万トークン食った（2026-09-19 の通し確認）。止める口は
    /// `ENABLE_CLAUDEAI_MCP_SERVERS=false` だけ。
    pub env: BTreeMap<String, String>,
}

impl SessionPreset {
    /// `session/new` / `session/load` に載せる `_meta`。載せるものが無ければ `None`
    /// （`_meta` キーごと省かれる＝既存のスレッドは 1 バイトも変わらない）。
    pub fn to_meta(&self) -> Option<v1::Meta> {
        let mut options = serde_json::Map::new();
        if let Some(tools) = &self.tools {
            options.insert("tools".to_string(), serde_json::json!(tools));
        }
        if let Some(setting_sources) = &self.setting_sources {
            options.insert(
                "settingSources".to_string(),
                serde_json::json!(setting_sources),
            );
        }
        if self.strict_mcp {
            options.insert("strictMcpConfig".to_string(), serde_json::json!(true));
        }

        let mut meta = v1::Meta::new();
        if let Some(system_prompt) = &self.system_prompt {
            meta.insert(
                "systemPrompt".to_string(),
                serde_json::Value::String(system_prompt.clone()),
            );
        }
        if !options.is_empty() {
            meta.insert(
                "claudeCode".to_string(),
                serde_json::json!({ "options": options }),
            );
        }
        (!meta.is_empty()).then_some(meta)
    }
}

/// 決まった MCP サーバ**だけ**を持たせるセッションにする（FLEET-V2 §5.8・Captain の席）。
/// 「他の MCP を持ち込ませない」やり方がエージェントごとに違うので、その違いをここに閉じ込める
/// （agent_panel の席と、実機プローブ `probe_captain_seat` が同じ関数を使う＝試したものが本番になる）。
///
/// - **Codex**: codex-acp は Codex 自身の設定（`~/.codex/config.toml`）の MCP サーバをそのまま立ち上げ、
///   ACP で渡したサーバは `mcp_servers` に丸ごと差し込む（同じキーに置いた「止める」指定は上書きで消える）。
///   だから ACP の `mcpServers` は空にし、`CODEX_CONFIG` で「持たせるサーバ」と「止めるサーバ
///   （`codex_servers` の名前・`enabled = false`）」を一緒に渡す。持たせるサーバは
///   `default_tools_approval_mode = "approve"`（席の道具は necoder がもう選んである）。
///   2026-09-25 に偽のサーバで確認: 塞ぐ前は起動され、塞いだ後は起動されず、necoder の道具は使えた。
/// - **Claude Code**: `mcpServers` で渡し、`strict_mcp`（`.mcp.json`・ユーザー設定・プラグインの MCP を無視）+
///   `setting_sources` を空（`~/.claude` の追加指示・hooks・**許可ルール**を持ち込まない＝Bash を黙って通す
///   ルールがあっても効かない）+ claude.ai のコネクタを止める環境変数。組み込みの道具は絞らない
///   （MCP の道具を読み込む `ToolSearch` まで消えうるので。書く道具と shell は権限の問いで断る）。
/// - その他: `mcpServers` で渡すだけ（持ち込みの止め方が分からない）。
pub fn restrict_mcp_to(
    preferences: &mut crate::SessionPreferences,
    agent_id: &str,
    servers: Vec<crate::mcp::McpServerConfig>,
    codex_servers: &[String],
) {
    match agent_id {
        "codex" => {
            let mut table = serde_json::Map::new();
            for name in codex_servers {
                table.insert(name.clone(), serde_json::json!({ "enabled": false }));
            }
            for server in &servers {
                if let crate::mcp::McpTransport::Stdio { command, args, env } = &server.transport {
                    table.insert(
                        server.name.clone(),
                        serde_json::json!({
                            "command": command,
                            "args": args,
                            "env": env,
                            "default_tools_approval_mode": "approve",
                        }),
                    );
                }
            }
            preferences.mcp_servers = Vec::new();
            preferences.preset.env.insert(
                "CODEX_CONFIG".to_string(),
                serde_json::json!({ "mcp_servers": table }).to_string(),
            );
        }
        "claude" => {
            preferences.mcp_servers = servers;
            preferences.preset.strict_mcp = true;
            preferences.preset.setting_sources = Some(Vec::new());
            preferences.preset.env.insert(
                "ENABLE_CLAUDEAI_MCP_SERVERS".to_string(),
                "false".to_string(),
            );
        }
        _ => preferences.mcp_servers = servers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_preset_sends_no_meta_at_all() {
        assert_eq!(SessionPreset::default().to_meta(), None);
    }

    #[test]
    fn the_meta_has_the_shape_the_claude_adapter_reads() {
        let preset = SessionPreset {
            system_prompt: Some("会話で答える".to_string()),
            tools: Some(vec!["Read".to_string(), "Write".to_string()]),
            setting_sources: Some(Vec::new()),
            strict_mcp: true,
            // 環境変数はプロセスに渡すもので、`_meta` には載らない。
            env: BTreeMap::from([(
                "ENABLE_CLAUDEAI_MCP_SERVERS".to_string(),
                "false".to_string(),
            )]),
        };
        let meta = serde_json::Value::Object(preset.to_meta().expect("meta"));
        assert_eq!(
            meta,
            serde_json::json!({
                "systemPrompt": "会話で答える",
                "claudeCode": { "options": {
                    "tools": ["Read", "Write"], "settingSources": [], "strictMcpConfig": true,
                } },
            })
        );
    }

    /// 空の一覧は「閉じる」という意味を持つので、`None`（＝既定のまま）と区別して載せる。
    #[test]
    fn an_empty_list_is_sent_rather_than_dropped() {
        let preset = SessionPreset {
            tools: Some(Vec::new()),
            ..SessionPreset::default()
        };
        let meta = serde_json::Value::Object(preset.to_meta().expect("meta"));
        assert_eq!(
            meta,
            serde_json::json!({ "claudeCode": { "options": { "tools": [] } } })
        );
    }

    fn necoder_server() -> crate::mcp::McpServerConfig {
        crate::mcp::McpServerConfig {
            name: "necoder".to_string(),
            source: crate::mcp::McpSource::Necoder,
            enabled: true,
            transport: crate::mcp::McpTransport::Stdio {
                command: "/Applications/necoder.app/Contents/MacOS/necoder".to_string(),
                args: vec!["mcp".to_string(), "--captain".to_string(), "/repo".to_string()],
                env: BTreeMap::new(),
            },
        }
    }

    /// Codex は Codex 自身の設定の MCP サーバを立ち上げるので、席では止めて necoder の分と一緒に
    /// `CODEX_CONFIG` で渡す（ACP の mcpServers に入れると「止める」指定が上書きで消える）。
    #[test]
    fn a_codex_seat_turns_off_its_own_servers_and_carries_necoder_in_codex_config() {
        let mut preferences = crate::SessionPreferences::default();
        restrict_mcp_to(
            &mut preferences,
            "codex",
            vec![necoder_server()],
            &["computer-use".to_string(), "node_repl".to_string()],
        );
        assert!(preferences.mcp_servers.is_empty());
        let config: serde_json::Value =
            serde_json::from_str(&preferences.preset.env["CODEX_CONFIG"]).unwrap();
        assert_eq!(config["mcp_servers"]["computer-use"]["enabled"], false);
        assert_eq!(config["mcp_servers"]["node_repl"]["enabled"], false);
        assert_eq!(config["mcp_servers"]["necoder"]["args"][1], "--captain");
        assert_eq!(config["mcp_servers"]["necoder"]["default_tools_approval_mode"], "approve");
    }

    /// Claude Code は mcpServers で渡し、他の MCP・ユーザー設定（許可ルール含む）・claude.ai のコネクタを閉じる。
    /// 組み込みの道具は絞らない（ToolSearch まで消えうる）。
    #[test]
    fn a_claude_seat_is_strict_about_mcp_and_settings() {
        let mut preferences = crate::SessionPreferences::default();
        restrict_mcp_to(&mut preferences, "claude", vec![necoder_server()], &["x".to_string()]);
        assert_eq!(preferences.mcp_servers, vec![necoder_server()]);
        assert!(preferences.preset.strict_mcp);
        assert_eq!(preferences.preset.setting_sources, Some(Vec::new()));
        assert_eq!(preferences.preset.tools, None, "組み込みの道具は絞らない");
        assert_eq!(preferences.preset.env["ENABLE_CLAUDEAI_MCP_SERVERS"], "false");
        assert!(!preferences.preset.env.contains_key("CODEX_CONFIG"));
    }
}
