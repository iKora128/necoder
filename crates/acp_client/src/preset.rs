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
}
