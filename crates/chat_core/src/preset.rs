//! preset — Chat のセッションの作り方（`docs/CHAT.md` §5）。
//!
//! Claude Code が「頼まれたら完全に実装しきる」のは、長いシステムプロンプトと Bash などの道具を
//! 全部持っていることの帰結で、モデルの性格ではない。プロンプトを差し替えて道具を絞ると、
//! 相談にはテキストで答え、見せる物を頼まれた時だけファイルを 1 つ書く（DECISIONS 2026-09-18）。
//!
//! 文面は英語で書く（返答の言語はユーザーに合わせさせる）。プロンプトは挙動の誘導であって
//! 防御ではない — 書ける場所を閉じるのは [`crate::policy`]。

use crate::date::Date;
use crate::folder::ARTIFACTS_DIR;
use acp_client::preset::SessionPreset;
use std::path::Path;

/// Chat に持たせる組み込みツール。**Bash は入れない**（入れた瞬間にコーディングエージェントへ戻る）。
/// `Glob` / `Grep` は渡されたフォルダの中を探すのに要る。
pub const TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Glob",
    "Grep",
    "WebSearch",
    "WebFetch",
];

/// Chat のセッションが必ず使う権限モード。ユーザーの既定が bypass でも引き継がない —
/// bypass ではエージェントが権限を聞いてこないので、[`crate::policy`] の裁定が素通りになる。
pub const PERMISSION_MODE: &str = "default";

/// このチャットのプリセット。`session/new` と `session/load` の両方に同じものを載せる。
pub fn session_preset(chat_dir: &Path, today: Date, custom_instructions: &str) -> SessionPreset {
    SessionPreset {
        system_prompt: Some(system_prompt(chat_dir, today, custom_instructions)),
        tools: Some(TOOLS.iter().map(|tool| tool.to_string()).collect()),
        // user / project / local の設定を持ち込まない（`~/.claude` の追加指示・MCP・hooks・権限ルール）。
        setting_sources: Some(Vec::new()),
        strict_mcp: true,
        // claude.ai アカウントのコネクタは上の 2 つでは止まらない（`SessionPreset::env` のコメント）。
        env: std::collections::BTreeMap::from([(
            "ENABLE_CLAUDEAI_MCP_SERVERS".to_string(),
            "false".to_string(),
        )]),
    }
}

/// Chat 用のシステムプロンプト。アダプタの既定（`claude_code` プリセット）を**丸ごと置換**するので、
/// 作業フォルダの場所や今日の日付のような、既定なら環境ブロックが教えていたこともここで教える。
pub fn system_prompt(chat_dir: &Path, today: Date, custom_instructions: &str) -> String {
    let chat_dir_text = chat_dir.display();
    let artifacts = chat_dir.join(ARTIFACTS_DIR);
    let artifacts_text = artifacts.display();
    let mut prompt = format!(
        "You are the assistant in the Chat mode of necoder, a desktop editor. Here you are a \
general-purpose conversational assistant, like a chat app. You are NOT acting as a coding agent.

# Conversation
- Answer questions, requests for advice, and discussions in the conversation itself. Do not create or edit files for them.
- Reply in the language the user writes in. Be direct and concise. Use Markdown where it helps.
- When the answer depends on current or external information, use WebSearch / WebFetch and cite the URLs you relied on.
- If a request is ambiguous about whether the user wants a file or just an answer, answer in the conversation and offer to make the file.

# Artifacts
An artifact is something the user wants to SEE or KEEP: a working UI, a diagram, a chart, a formatted document.
- Create one only when the user asks for such a deliverable.
- Write it as ONE self-contained file in `{artifacts_text}` (create the folder if needed). Use HTML with inline CSS and JavaScript, or Markdown. necoder shows the file next to the conversation automatically.
- HTML artifacts run sandboxed: the page cannot make network requests (fetch, XHR, WebSocket) and cannot load images from the network. Scripts, styles and fonts may be loaded only from cdnjs.cloudflare.com, cdn.jsdelivr.net, unpkg.com and Google Fonts. Embed everything else inline.
- No build step, no tests, no README, no multi-file projects, no helper files.
- To change an existing artifact, EDIT THE SAME FILE. Never write a second copy or a renamed version unless the user asks for one.
- After writing, say what you made in one or two sentences. Do not paste the file's contents into the conversation.

# Files from the user
- Lines of the form `@/absolute/path` at the top of a message are files or folders the user attached. Read them when they are relevant.
- When asked to change an attached file, edit that file in place. Do not copy it into your working folder.
- Do not read or write anywhere else unless the user asks. Writes outside your working folder and the attached files are refused.

# Environment
- Working folder: {chat_dir_text}
- Today's date: {today}
"
    );
    let custom_instructions = custom_instructions.trim();
    if !custom_instructions.is_empty() {
        prompt.push_str("\n# Instructions from the user (apply to every conversation)\n");
        prompt.push_str(custom_instructions);
        prompt.push('\n');
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const DAY: Date = Date {
        year: 2026,
        month: 9,
        day: 18,
    };

    #[test]
    fn the_prompt_names_the_folder_the_date_and_the_rules() {
        let chat_dir = PathBuf::from("/Users/test/Documents/necoder/2026-09-18 タイマー");
        let prompt = system_prompt(&chat_dir, DAY, "");
        // 期待値は**プロンプトが使うのと同じ組み立て**から作る。ここで区切りを直書きすると、
        // Windows（`\`）で落ちる（Windows CI で実際に踏んだ・2026-09-19）。
        let artifacts = chat_dir.join(ARTIFACTS_DIR).display().to_string();
        assert!(prompt.contains(&artifacts), "{artifacts}");
        assert!(prompt.contains("Today's date: 2026-09-18"));
        assert!(prompt.contains("EDIT THE SAME FILE"));
        assert!(prompt.contains("NOT acting as a coding agent"));
        assert!(!prompt.contains("Instructions from the user"));
    }

    #[test]
    fn custom_instructions_are_appended_only_when_present() {
        let chat_dir = PathBuf::from("/chat");
        let prompt = system_prompt(&chat_dir, DAY, "  敬語は使わない  ");
        assert!(prompt.ends_with(
            "# Instructions from the user (apply to every conversation)\n敬語は使わない\n"
        ));
        assert!(!system_prompt(&chat_dir, DAY, "   ").contains("Instructions from the user"));
    }

    /// 道具と設定の継承を**明示的に**閉じる（`None` は「エージェントの既定のまま」の意味になる）。
    #[test]
    fn the_preset_closes_the_shell_and_the_inherited_settings() {
        let preset = session_preset(Path::new("/chat"), DAY, "");
        let tools = preset.tools.expect("tools を明示する");
        assert!(!tools.iter().any(|tool| tool == "Bash"));
        assert!(tools.iter().any(|tool| tool == "WebSearch"));
        assert_eq!(preset.setting_sources, Some(Vec::new()));
        assert!(preset.strict_mcp);
        assert_eq!(
            preset
                .env
                .get("ENABLE_CLAUDEAI_MCP_SERVERS")
                .map(String::as_str),
            Some("false"),
            "claude.ai のコネクタは環境変数でしか止まらない"
        );
        assert!(preset.system_prompt.is_some());
    }
}
