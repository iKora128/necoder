//! connections — **接続**（どのモデルの API に、誰の契約でつなぐか）をエージェントへ渡す（issue #38 H3）。
//!
//! 宛先を「エージェント × 接続」の 2 軸にした時の接続の側。宛先（形式とベース URL）は settings.json の
//! `connections`（**user 層だけ**・`settings_core::USER_ONLY_KEYS`）、API キーは OS のキーチェーン
//! （[`secrets`]）に置く。**necoder 自身はこのキーで API を呼ばない** — エージェントを起こす時に渡すだけ
//! （issue #38 §5-1）。モデルは今までどおりエージェントの広告から選ぶ（necoder は綴りを持たない）。
//!
//! 渡し方はエージェントごとに違う（[`Harness`]）:
//! - **Claude Code**（claude-agent-acp）: `agentCapabilities.providers` を広告していれば、`session/new` /
//!   `session/load` の前に `providers/set`（`providerId: "main"`・キーは `Authorization` ヘッダ）を送る。
//!   アダプタはこの値を Claude Code の env と設定の両方へ入れる（リポジトリの `.claude/settings.json` の
//!   env でも上書きできない）。広告しない古いアダプタは `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` で
//!   起こし直す。Anthropic 互換の口だけ（アダプタが `openai` を受けない）
//! - **DeepSeek Harness**（`dsh-acp`）: DeepSeek の経路（Anthropic Messages の口）に `DSH_PROVIDER` /
//!   `DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` で渡す。dsh は `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` を読まず
//!   （他社の口は dsh の profile の設定）、`providers/set` も持たないので、DeepSeek API の接続だけ
//! - **OpenCode**: ひな形が OpenCode の組み込みのプロバイダ（models.dev の id）に当たる時だけ、その
//!   プロバイダの環境変数でキーを渡し、`OPENCODE_CONFIG_CONTENT`（リポジトリの `opencode.json` より強い）
//!   の `enabled_providers` でそのプロバイダだけにする＝別の接続のモデルを出さない。宛先は OpenCode の
//!   既定の口なので、ベース URL を変えた接続は渡さない（変えた宛先を守れない）
//!
//! それ以外のエージェントは渡し方が分からないので渡さない（Pi は別 Task の調べを待つ）。**リモート
//! （SSH 先で起こすエージェント）にも渡さない** — キーを手元の外へ出さない（H3 の範囲外・呼び手が止める）。
//! このモジュールは UI も設定のスキーマも知らない（`mcp` / `preset` と同じ流儀）。

pub mod secrets;

use crate::{Agent, CustomLaunch};
use acp::schema::v1;
use agent_client_protocol as acp;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// API の形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    /// Anthropic Messages API 互換（Claude Code が話す形）。
    Anthropic,
    /// OpenAI Chat Completions 互換。
    OpenAi,
}

/// Anthropic 互換の口に、キーをどのヘッダで載せるか（各社の Claude Code の手順に合わせる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnthropicAuth {
    /// `Authorization: Bearer <key>`（Claude Code の `ANTHROPIC_AUTH_TOKEN`）。ほとんどの会社の手順。
    Bearer,
    /// `x-api-key: <key>`（Claude Code の `ANTHROPIC_API_KEY`・Kimi Code の手順）。`providers/set` では
    /// アダプタが `Authorization: Bearer acp-proxy` を必ず付けるので、同じキーを両方のヘッダに載せる。
    ApiKey,
}

/// OpenCode の組み込みのプロバイダ（models.dev の id と、キーを読む環境変数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenCodeProvider {
    pub id: &'static str,
    pub key_env: &'static str,
}

/// 接続のひな形（設定の「接続を追加」で選ぶ・ベース URL と形式を埋める）。
///
/// URL とキーの載せ方は各社の Claude Code / OpenAI SDK 向けの手順のページ（`docs`）で 2026-10-02 に
/// 確かめた物。変わったら直すのはこの表だけ（settings.json には選んだ時の URL が残る）。同じ会社でも
/// 従量課金とコーディングプランで口とキーが別の物（Kimi・Qwen）は別のひな形にする（混ぜると 401）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// settings.json の `connections.<id>.preset` に書く綴り。
    pub id: &'static str,
    /// 一覧に出す名前（各社の製品名のまま・訳さない）。`None` = 汎用のひな形（UI が i18n で名付ける）。
    pub name: Option<&'static str>,
    /// Anthropic 互換の口（Claude Code の `ANTHROPIC_BASE_URL` に入れる形・`/v1/messages` は足される側）。
    pub anthropic: Option<&'static str>,
    /// OpenAI 互換の口（各社の手順の `base_url` のまま）。
    pub openai: Option<&'static str>,
    pub anthropic_auth: AnthropicAuth,
    /// キーが要るか（手元の Ollama・汎用の互換 API は空でもよい）。
    pub needs_key: bool,
    /// キーの無い接続でエージェントに渡すトークン（Claude Code は何かが無いと保存済みのログインへ戻る）。
    pub placeholder_token: Option<&'static str>,
    pub opencode: Option<OpenCodeProvider>,
    /// 各社の手順のページ（設定の面の案内）。
    pub docs: &'static str,
}

impl Preset {
    /// 形式ごとの既定の口。
    pub fn base_url(&self, protocol: Protocol) -> Option<&'static str> {
        match protocol {
            Protocol::Anthropic => self.anthropic,
            Protocol::OpenAi => self.openai,
        }
    }

    /// このひな形で選べる形式（Anthropic 互換が先）。
    pub fn protocols(&self) -> Vec<Protocol> {
        [Protocol::Anthropic, Protocol::OpenAi]
            .into_iter()
            .filter(|protocol| self.base_url(*protocol).is_some())
            .collect()
    }
}

/// 汎用の OpenAI 互換（宛先は人が書く）。
pub const OPENAI_COMPATIBLE: &str = "openai-compatible";
/// 汎用の Anthropic 互換（宛先は人が書く）。
pub const ANTHROPIC_COMPATIBLE: &str = "anthropic-compatible";
/// DeepSeek API のひな形（DeepSeek Harness に渡せる唯一の接続）。
pub const DEEPSEEK: &str = "deepseek";

/// ひな形の一覧（設定の「接続を追加」の並び順）。
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "zai-coding-plan",
        name: Some("GLM Coding Plan (Z.ai)"),
        anthropic: Some("https://api.z.ai/api/anthropic"),
        openai: Some("https://api.z.ai/api/coding/paas/v4"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "zai-coding-plan",
            key_env: "ZHIPU_API_KEY",
        }),
        docs: "https://docs.z.ai/devpack/tool/claude",
    },
    Preset {
        id: "kimi-code",
        name: Some("Kimi Code"),
        anthropic: Some("https://api.kimi.ai/coding/"),
        openai: Some("https://api.kimi.ai/coding/v1"),
        anthropic_auth: AnthropicAuth::ApiKey,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "kimi-code-plan-global",
            key_env: "KIMI_API_KEY",
        }),
        docs: "https://www.kimi.com/code/docs/third-party-tools/claude-code.html",
    },
    Preset {
        id: "moonshot",
        name: Some("Kimi API (Moonshot)"),
        anthropic: Some("https://api.moonshot.ai/anthropic"),
        openai: Some("https://api.moonshot.ai/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "moonshotai",
            key_env: "MOONSHOT_API_KEY",
        }),
        docs: "https://platform.kimi.ai/docs/guide/claude-code-kimi",
    },
    Preset {
        id: "minimax",
        name: Some("MiniMax"),
        anthropic: Some("https://api.minimax.io/anthropic"),
        openai: Some("https://api.minimax.io/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "minimax",
            key_env: "MINIMAX_API_KEY",
        }),
        docs: "https://platform.minimax.io/docs/coding-plan/claude-code",
    },
    Preset {
        id: "qwen-coding-plan",
        name: Some("Qwen Coding Plan (Alibaba Cloud)"),
        anthropic: Some("https://coding-intl.dashscope.aliyuncs.com/apps/anthropic"),
        openai: Some("https://coding-intl.dashscope.aliyuncs.com/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "alibaba-coding-plan",
            key_env: "ALIBABA_CODING_PLAN_API_KEY",
        }),
        docs: "https://www.alibabacloud.com/help/en/model-studio/coding-plan",
    },
    Preset {
        id: "qwen",
        name: Some("Qwen API (Alibaba Cloud Model Studio)"),
        anthropic: Some("https://dashscope-intl.aliyuncs.com/apps/anthropic"),
        openai: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "alibaba",
            key_env: "DASHSCOPE_API_KEY",
        }),
        docs: "https://www.alibabacloud.com/help/en/model-studio/claude-code",
    },
    Preset {
        id: "mimo",
        name: Some("Xiaomi MiMo"),
        anthropic: Some("https://api.xiaomimimo.com/anthropic"),
        openai: Some("https://api.xiaomimimo.com/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "xiaomi",
            key_env: "XIAOMI_API_KEY",
        }),
        docs: "https://mimo.mi.com/docs/en-US/tokenplan/integration/claudecode",
    },
    Preset {
        id: DEEPSEEK,
        name: Some("DeepSeek API"),
        anthropic: Some("https://api.deepseek.com/anthropic"),
        openai: Some("https://api.deepseek.com"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "deepseek",
            key_env: "DEEPSEEK_API_KEY",
        }),
        docs: "https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code",
    },
    Preset {
        id: "openrouter",
        name: Some("OpenRouter"),
        anthropic: Some("https://openrouter.ai/api"),
        openai: Some("https://openrouter.ai/api/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: true,
        placeholder_token: None,
        opencode: Some(OpenCodeProvider {
            id: "openrouter",
            key_env: "OPENROUTER_API_KEY",
        }),
        docs: "https://openrouter.ai/docs/cookbook/coding-agents/claude-code-integration",
    },
    Preset {
        id: "ollama",
        name: Some("Ollama"),
        anthropic: Some("http://localhost:11434"),
        openai: Some("http://localhost:11434/v1"),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: false,
        placeholder_token: Some("ollama"),
        opencode: None,
        docs: "https://docs.ollama.com/integrations/claude-code",
    },
    Preset {
        id: OPENAI_COMPATIBLE,
        name: None,
        anthropic: None,
        openai: Some(""),
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: false,
        placeholder_token: Some("necoder"),
        opencode: None,
        docs: "",
    },
    Preset {
        id: ANTHROPIC_COMPATIBLE,
        name: None,
        anthropic: Some(""),
        openai: None,
        anthropic_auth: AnthropicAuth::Bearer,
        needs_key: false,
        placeholder_token: Some("necoder"),
        opencode: None,
        docs: "",
    },
];

/// ひな形を id で引く。知らない id（手で書いた・後の版で消えた）は `None`。
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// 接続 1 件（acp_client の言葉。settings 層が `connections.<id>` から写す）。キーは持たない。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Connection {
    /// `connections` のキー（キーチェーンの項目の名前にも使う）。
    pub id: String,
    /// 一覧・ピル・知らせに出す名前。
    pub name: String,
    /// ひな形の id（知らない id は汎用の互換 API として扱う）。
    pub preset: String,
    pub protocol: Protocol,
    pub base_url: String,
}

impl Connection {
    pub fn preset(&self) -> Option<&'static Preset> {
        preset(&self.preset)
    }

    /// キーが要るか（ひな形が要らないと言う物だけ要らない）。
    pub fn needs_key(&self) -> bool {
        self.preset().is_none_or(|preset| preset.needs_key)
    }

    /// 宛先がひな形の既定の口のままか（人がベース URL を書き換えていない）。末尾の `/` は見ない。
    pub fn uses_preset_url(&self) -> bool {
        let trim = |url: &str| url.trim().trim_end_matches('/').to_string();
        self.preset()
            .and_then(|preset| preset.base_url(self.protocol))
            .is_some_and(|url| !url.is_empty() && trim(url) == trim(&self.base_url))
    }

    /// 宛先を決める中身の指紋（id・ひな形・形式・ベース URL）。モデルの在庫・使用量の鍵が、接続を
    /// 書き換えた時に古い一覧と混ざらないように使う。
    pub fn routing_fingerprint(&self) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// 接続を渡す時に使う、ベース URL として正しい形か（`http://` か `https://` で始まり、ホストがある）。
/// 設定の面が保存の前に確かめる（アダプタの `providers/set` も同じ形を求める）。
pub fn is_valid_base_url(url: &str) -> bool {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !host.is_empty() && !host.starts_with(':') && !rest.contains(char::is_whitespace)
    })
}

/// 接続の id に使える形か（キーチェーンの項目の名前に入るので、英数字と `-` `_` `.` だけ）。
pub fn is_valid_connection_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

/// 接続を渡せるエージェント（渡し方が分かっている物だけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Harness {
    ClaudeCode,
    OpenCode,
    DeepSeekHarness,
}

/// 接続をそのエージェントへ渡せない理由（UI が言葉にする）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// このエージェントは別の形式の口しか使えない。
    Protocol { needed: Protocol },
    /// DeepSeek Harness は DeepSeek API の接続だけ（他社の口は dsh 自身の設定で選ぶ）。
    OnlyDeepSeek,
    /// OpenCode の組み込みのプロバイダに当たらないひな形（自分で書いた互換 API・Ollama）。
    NotAnOpenCodeProvider,
    /// ベース URL を書き換えた接続（OpenCode は自分の既定の口で繋ぐので、書き換えた宛先を守れない）。
    CustomUrlForOpenCode,
    /// キーがキーチェーンに無い。
    MissingKey,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Protocol { needed } => {
                write!(formatter, "{needed:?} 互換の口だけを受け付けるエージェント")
            }
            Refusal::OnlyDeepSeek => {
                write!(
                    formatter,
                    "DeepSeek Harness には DeepSeek API の接続だけを渡す"
                )
            }
            Refusal::NotAnOpenCodeProvider => {
                write!(formatter, "OpenCode の組み込みのプロバイダに当たらない接続")
            }
            Refusal::CustomUrlForOpenCode => {
                write!(
                    formatter,
                    "ベース URL を書き換えた接続は OpenCode に渡さない"
                )
            }
            Refusal::MissingKey => write!(formatter, "キーがキーチェーンに無い"),
        }
    }
}

impl std::error::Error for Refusal {}

/// DeepSeek Harness の ACP アダプタのコマンド名（`@openma/deepseek-harness-acp`）。
const DEEPSEEK_HARNESS_COMMAND: &str = "dsh-acp";

impl Harness {
    /// エージェントの種類から引く。組み込みは id、足したエージェントは起動コマンドの名前で当てる
    /// （DeepSeek Harness は H1 で `agent_servers` に自分で足すので、id は人が決める）。
    pub fn of(agent: &Agent) -> Option<Harness> {
        match agent {
            Agent::Builtin(kind) => match kind.id {
                "claude" => Some(Harness::ClaudeCode),
                "opencode" => Some(Harness::OpenCode),
                _ => None,
            },
            Agent::Custom(custom) => match &custom.launch {
                CustomLaunch::Command { command, .. } => {
                    let name = Path::new(command)
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or_default();
                    (name == DEEPSEEK_HARNESS_COMMAND).then_some(Harness::DeepSeekHarness)
                }
                CustomLaunch::Registry { .. } => None,
            },
        }
    }

    /// この接続を渡せるか（キーの有無はまだ見ない）。
    pub fn accepts(self, connection: &Connection) -> Result<(), Refusal> {
        match self {
            Harness::ClaudeCode => match connection.protocol {
                Protocol::Anthropic => Ok(()),
                Protocol::OpenAi => Err(Refusal::Protocol {
                    needed: Protocol::Anthropic,
                }),
            },
            // dsh の DeepSeek の経路は Anthropic Messages の口（`DEEPSEEK_BASE_URL`）で話す。
            Harness::DeepSeekHarness if connection.preset != DEEPSEEK => Err(Refusal::OnlyDeepSeek),
            Harness::DeepSeekHarness => match connection.protocol {
                Protocol::Anthropic => Ok(()),
                Protocol::OpenAi => Err(Refusal::Protocol {
                    needed: Protocol::Anthropic,
                }),
            },
            Harness::OpenCode => {
                if connection
                    .preset()
                    .and_then(|preset| preset.opencode)
                    .is_none()
                {
                    Err(Refusal::NotAnOpenCodeProvider)
                } else if !connection.uses_preset_url() {
                    Err(Refusal::CustomUrlForOpenCode)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// 起動に載せる接続を組む。`key` はキーチェーンから読んだ値（キーの要らない接続は `None` でよい）。
    pub fn launch(
        self,
        connection: &Connection,
        key: Option<&str>,
    ) -> Result<ConnectionLaunch, Refusal> {
        self.accepts(connection)?;
        let preset = connection.preset();
        let key = key.map(str::trim).filter(|key| !key.is_empty());
        let key = match (key, connection.needs_key()) {
            (Some(key), _) => key.to_string(),
            (None, true) => return Err(Refusal::MissingKey),
            (None, false) => preset
                .and_then(|preset| preset.placeholder_token)
                .unwrap_or("necoder")
                .to_string(),
        };
        let base_url = connection.base_url.trim().to_string();
        let mut launch = ConnectionLaunch {
            name: connection.name.clone(),
            provider: None,
            env: BTreeMap::new(),
        };
        match self {
            Harness::ClaudeCode => {
                let auth = preset
                    .map(|preset| preset.anthropic_auth)
                    .unwrap_or(AnthropicAuth::Bearer);
                // キーは `Authorization` に載せる（アダプタが付ける `Bearer acp-proxy` をこれで置き換える）。
                // `x-api-key` で受ける口には同じキーを両方に載せる（同じ宛先へ同じキーを送るだけ）。
                let mut headers =
                    BTreeMap::from([("Authorization".to_string(), format!("Bearer {key}"))]);
                if auth == AnthropicAuth::ApiKey {
                    headers.insert("x-api-key".to_string(), key.clone());
                }
                launch.provider = Some(ProviderRoute {
                    base_url: base_url.clone(),
                    headers,
                });
                // 広告しない古いアダプタへの代わり。アダプタの `providers/set` と同じく、他の宛先と資格情報を
                // 空にしてから宛先とキーを入れる（手元の `ANTHROPIC_API_KEY` を別の会社へ送らない）。
                for name in CLAUDE_ROUTING_ENV {
                    launch.env.insert(name.to_string(), String::new());
                }
                launch
                    .env
                    .insert("CLAUDE_CODE_USE_BEDROCK".to_string(), "0".to_string());
                launch
                    .env
                    .insert("CLAUDE_CODE_USE_VERTEX".to_string(), "0".to_string());
                launch
                    .env
                    .insert("ANTHROPIC_BASE_URL".to_string(), base_url);
                let key_variable = match auth {
                    AnthropicAuth::Bearer => "ANTHROPIC_AUTH_TOKEN",
                    AnthropicAuth::ApiKey => "ANTHROPIC_API_KEY",
                };
                launch.env.insert(key_variable.to_string(), key);
            }
            Harness::DeepSeekHarness => {
                // 起動の env は dsh の保存済みの資格情報より強い（dsh の README の優先順）。
                launch
                    .env
                    .insert("DSH_PROVIDER".to_string(), DEEPSEEK.to_string());
                launch.env.insert("DEEPSEEK_BASE_URL".to_string(), base_url);
                launch.env.insert("DEEPSEEK_API_KEY".to_string(), key);
            }
            Harness::OpenCode => {
                let Some(provider) = preset.and_then(|preset| preset.opencode) else {
                    return Err(Refusal::NotAnOpenCodeProvider);
                };
                launch.env.insert(provider.key_env.to_string(), key);
                launch.env.insert(
                    "OPENCODE_CONFIG_CONTENT".to_string(),
                    serde_json::json!({ "enabled_providers": [provider.id] }).to_string(),
                );
            }
        }
        Ok(launch)
    }
}

/// Claude Code の宛先と資格情報を決める env（接続を渡す時は一度ぜんぶ空にする・アダプタの
/// `createEnvForProvider` の resetRouting と同じ顔ぶれ）。
const CLAUDE_ROUTING_ENV: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "AWS_REGION",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// `providers/set` で渡す宛先（`providerId` は claude-agent-acp の唯一のプロバイダ `main`）。
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderRoute {
    pub base_url: String,
    /// キーを載せたヘッダ（呼ぶたびに全部を置き換える＝毎回全部送る）。
    pub headers: BTreeMap<String, String>,
}

/// claude-agent-acp が持つ唯一のプロバイダの id。
pub const PROVIDER_ID: &str = "main";

impl ProviderRoute {
    /// `providers/set` の params（ACP スキーマの型で組む＝綴りを手で書かない）。
    pub fn set_request(&self) -> v1::SetProviderRequest {
        v1::SetProviderRequest::new(
            PROVIDER_ID,
            v1::LlmProtocol::Anthropic,
            self.base_url.clone(),
        )
        .headers(
            self.headers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        )
    }
}

impl std::fmt::Debug for ProviderRoute {
    /// ヘッダの値（キー）は出さない（ログ・パニックの文に残さない）。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderRoute")
            .field("base_url", &self.base_url)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// 起動に載せる接続（`SessionPreferences::connection`）。キーを含むので `Debug` は名前だけ出す。
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionLaunch {
    /// 接続の名前（知らせに出す）。
    pub name: String,
    /// `providers/set` で渡す宛先（Claude Code）。`Some` なら、まず env 無しで起こし、エージェントが
    /// `agentCapabilities.providers` を広告していれば `session/new` / `session/load` の前に送る。広告
    /// しなければ `env` を足して起こし直す。
    pub provider: Option<ProviderRoute>,
    /// 起動時に足す環境変数（`provider` があれば、広告しなかった時の代わり）。
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Debug for ConnectionLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionLaunch")
            .field("name", &self.name)
            .field("provider", &self.provider)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// 接続を渡すセッションで、リポジトリの `.necoder/task.env` から**読まない**変数か。
///
/// task.env はリポジトリが持つファイルで、エージェントのプロセスへそのまま入る。接続のキーを渡す
/// プロセスでは、通信の行き先・通信路・読み込むコードを変える変数を読まない（読むとキーを横取りできる）:
/// 宛先と資格情報（`ANTHROPIC_*` / `OPENAI_*` / `OPENCODE_*` / `CLAUDE_CODE_*` と各社の鍵の変数）、
/// プロキシと証明書（`HTTPS_PROXY` / `NODE_EXTRA_CA_CERTS` …）、実行時に差し込むコード（`NODE_OPTIONS` /
/// `LD_PRELOAD` / `DYLD_*` …）。接続の変数は task.env より後に積むので、同じ名前は元から上書きされる。
pub fn guarded_from_task_env(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ANTHROPIC_",
        "OPENAI_",
        "OPENCODE_",
        "CLAUDE_CODE_",
        "DEEPSEEK_",
        "DSH_",
        "DYLD_",
    ];
    const NAMES: &[&str] = &[
        "CLAUDE_CONFIG_DIR",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "NODE_EXTRA_CA_CERTS",
        "NODE_TLS_REJECT_UNAUTHORIZED",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "NODE_OPTIONS",
        "NODE_PATH",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "BUN_OPTIONS",
    ];
    let upper = name.to_ascii_uppercase();
    PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
        || NAMES.contains(&upper.as_str())
        || PRESETS.iter().any(|preset| {
            preset
                .opencode
                .is_some_and(|provider| provider.key_env == upper)
        })
}

/// エージェントのログインの指紋（モデルの在庫の鍵・issue #38 H3）。ログインや設定を替えると変わる。
///
/// 見るのはログインと設定の**ファイルの更新時刻と大きさ**だけで、資格情報の中身は読まない（Claude Code の
/// `.claude.json` だけは、資格情報ではないアカウントの id を読む — macOS のログインはキーチェーンに
/// あってファイルが変わらないため）。`env` は起動に渡す env（`CLAUDE_CONFIG_DIR` / `CODEX_HOME` で
/// 置き場を替えたアカウントはその置き場を見る）。手元のファイルを見るので手元のエージェントにだけ使う。
pub fn login_fingerprint(agent_id: &str, env: &BTreeMap<String, String>) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    agent_id.hash(&mut hasher);
    for path in login_files(agent_id, env) {
        path.hash(&mut hasher);
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                metadata.len().hash(&mut hasher);
                metadata.modified().ok().hash(&mut hasher);
            }
            Err(_) => 0u8.hash(&mut hasher),
        }
    }
    if agent_id == "claude" {
        claude_account(env).hash(&mut hasher);
    }
    hasher.finish()
}

/// 置き場の変数（`env` に在れば）か、ホームの下の既定の置き場。
fn config_root(env: &BTreeMap<String, String>, variable: &str, default: &str) -> Option<PathBuf> {
    match env.get(variable).filter(|value| !value.is_empty()) {
        Some(value) => Some(PathBuf::from(value)),
        None => std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| paths::home_dir().map(|home| home.join(default))),
    }
}

/// ログインと、モデルの一覧を変え得る設定のファイル（無い物も並べる＝作られたら指紋が変わる）。
fn login_files(agent_id: &str, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let home = paths::home_dir();
    let under_home = |relative: &str| home.as_ref().map(|home| home.join(relative));
    let files: Vec<Option<PathBuf>> = match agent_id {
        "claude" => {
            let root = config_root(env, "CLAUDE_CONFIG_DIR", ".claude");
            vec![
                root.as_ref().map(|root| root.join(".credentials.json")),
                root.as_ref().map(|root| root.join("settings.json")),
            ]
        }
        "codex" => {
            let root = config_root(env, "CODEX_HOME", ".codex");
            vec![
                root.as_ref().map(|root| root.join("auth.json")),
                root.as_ref().map(|root| root.join("config.toml")),
            ]
        }
        "opencode" => {
            let data = config_root(env, "XDG_DATA_HOME", ".local/share");
            let config = config_root(env, "XDG_CONFIG_HOME", ".config");
            vec![
                data.as_ref().map(|root| root.join("opencode/auth.json")),
                config
                    .as_ref()
                    .map(|root| root.join("opencode/opencode.json")),
                config
                    .as_ref()
                    .map(|root| root.join("opencode/opencode.jsonc")),
            ]
        }
        "copilot" => vec![
            under_home(".copilot/config.json"),
            under_home(".config/gh/hosts.yml"),
        ],
        "qwen" => vec![under_home(".qwen/settings.json"), under_home(".qwen/.env")],
        "kimi" => vec![
            under_home(".kimi-code/config.toml"),
            under_home(".kimi-code/credentials.json"),
        ],
        "grok" => vec![
            under_home(".grok/config.toml"),
            under_home(".grok/credentials.json"),
        ],
        _ => Vec::new(),
    };
    files.into_iter().flatten().collect()
}

/// Claude Code がログインしているアカウント（`.claude.json` の `oauthAccount` の id だけ）。
/// `.claude.json` は起動のたびに書き換わる（起動回数など）ので、更新時刻ではなくアカウントの id を見る。
fn claude_account(env: &BTreeMap<String, String>) -> Option<(String, String)> {
    let path = match env
        .get("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
    {
        Some(root) => PathBuf::from(root).join(".claude.json"),
        None => paths::home_dir()?.join(".claude.json"),
    };
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let account = value.get("oauthAccount")?;
    let field = |name: &str| {
        account
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some((field("accountUuid"), field("organizationUuid")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(preset: &str, protocol: Protocol, base_url: &str) -> Connection {
        Connection {
            id: "c".into(),
            name: "接続".into(),
            preset: preset.into(),
            protocol,
            base_url: base_url.into(),
        }
    }

    fn glm() -> Connection {
        connection(
            "zai-coding-plan",
            Protocol::Anthropic,
            "https://api.z.ai/api/anthropic",
        )
    }

    /// Claude Code: `providers/set` の宛先とキーのヘッダ、広告しない時の env（他の宛先と資格情報を空に
    /// してから入れる＝手元の `ANTHROPIC_API_KEY` を別の会社へ送らない）。
    #[test]
    fn claude_code_gets_a_provider_route_and_an_env_fallback() {
        let launch = Harness::ClaudeCode
            .launch(&glm(), Some(" sk-test-key "))
            .expect("渡せる");
        let route = launch.provider.as_ref().expect("providers/set で渡す");
        assert_eq!(route.base_url, "https://api.z.ai/api/anthropic");
        assert_eq!(route.headers["Authorization"], "Bearer sk-test-key");
        let request = serde_json::to_value(route.set_request()).expect("JSON");
        assert_eq!(
            request,
            serde_json::json!({ "providerId": "main", "apiType": "anthropic",
                                "baseUrl": "https://api.z.ai/api/anthropic",
                                "headers": { "Authorization": "Bearer sk-test-key" } })
        );
        assert_eq!(
            launch.env["ANTHROPIC_BASE_URL"],
            "https://api.z.ai/api/anthropic"
        );
        assert_eq!(launch.env["ANTHROPIC_AUTH_TOKEN"], "sk-test-key");
        assert_eq!(launch.env["ANTHROPIC_API_KEY"], "", "手元のキーを空にする");
        assert_eq!(launch.env["CLAUDE_CODE_OAUTH_TOKEN"], "");
        assert_eq!(launch.env["CLAUDE_CODE_USE_BEDROCK"], "0");
        // Debug にキーは出ない。
        let debug = format!("{launch:?}");
        assert!(!debug.contains("sk-test-key"), "{debug}");
    }

    /// Kimi Code は `x-api-key` で受ける口: `providers/set` では同じキーを `Authorization` と両方に載せ、
    /// 広告しない時の env は `ANTHROPIC_API_KEY` で渡す（`ANTHROPIC_AUTH_TOKEN` は空にする）。
    #[test]
    fn an_api_key_endpoint_gets_the_key_in_both_headers() {
        let kimi = connection(
            "kimi-code",
            Protocol::Anthropic,
            "https://api.kimi.ai/coding/",
        );
        let launch = Harness::ClaudeCode
            .launch(&kimi, Some("kk"))
            .expect("渡せる");
        let route = launch.provider.as_ref().expect("providers/set");
        assert_eq!(route.headers["Authorization"], "Bearer kk");
        assert_eq!(route.headers["x-api-key"], "kk");
        assert_eq!(launch.env["ANTHROPIC_API_KEY"], "kk");
        assert_eq!(launch.env["ANTHROPIC_AUTH_TOKEN"], "");
    }

    #[test]
    fn each_harness_takes_only_the_protocol_it_speaks() {
        let openai = connection(DEEPSEEK, Protocol::OpenAi, "https://api.deepseek.com");
        assert_eq!(
            Harness::ClaudeCode.accepts(&openai),
            Err(Refusal::Protocol {
                needed: Protocol::Anthropic
            })
        );
        // DeepSeek Harness は DeepSeek API の Anthropic Messages の口だけ（dsh の DeepSeek の経路）。
        assert_eq!(
            Harness::DeepSeekHarness.accepts(&glm()),
            Err(Refusal::OnlyDeepSeek)
        );
        assert_eq!(
            Harness::DeepSeekHarness.accepts(&openai),
            Err(Refusal::Protocol {
                needed: Protocol::Anthropic
            })
        );
        let deepseek = connection(
            DEEPSEEK,
            Protocol::Anthropic,
            "https://api.deepseek.com/anthropic",
        );
        let launch = Harness::DeepSeekHarness
            .launch(&deepseek, Some("sk-ds"))
            .expect("渡せる");
        assert!(launch.provider.is_none(), "env で渡す");
        assert_eq!(launch.env["DSH_PROVIDER"], "deepseek");
        assert_eq!(
            launch.env["DEEPSEEK_BASE_URL"],
            "https://api.deepseek.com/anthropic"
        );
        assert_eq!(launch.env["DEEPSEEK_API_KEY"], "sk-ds");
        assert!(
            !launch.env.contains_key("OPENAI_BASE_URL"),
            "dsh は読まない"
        );
    }

    /// OpenCode: 組み込みのプロバイダの変数でキーを渡し、そのプロバイダだけを使わせる。ベース URL を
    /// 書き換えた接続・OpenCode のプロバイダに当たらない接続は渡さない。
    #[test]
    fn opencode_takes_presets_it_knows_with_their_own_url() {
        let launch = Harness::OpenCode
            .launch(&glm(), Some("zk"))
            .expect("渡せる");
        assert_eq!(launch.env["ZHIPU_API_KEY"], "zk");
        let config: serde_json::Value =
            serde_json::from_str(&launch.env["OPENCODE_CONFIG_CONTENT"]).expect("JSON");
        assert_eq!(
            config,
            serde_json::json!({ "enabled_providers": ["zai-coding-plan"] })
        );
        let edited = connection(
            "zai-coding-plan",
            Protocol::Anthropic,
            "https://open.bigmodel.cn/api/anthropic",
        );
        assert_eq!(
            Harness::OpenCode.accepts(&edited),
            Err(Refusal::CustomUrlForOpenCode)
        );
        let custom = connection(
            OPENAI_COMPATIBLE,
            Protocol::OpenAi,
            "https://llm.example/v1",
        );
        assert_eq!(
            Harness::OpenCode.accepts(&custom),
            Err(Refusal::NotAnOpenCodeProvider)
        );
    }

    /// キーの要る接続にキーが無ければ起こさない（黙って自分のログインで走らせない）。キーの要らない
    /// Ollama は、ログインを求められないように決まったトークンを渡す。
    #[test]
    fn a_missing_key_is_refused_unless_the_preset_needs_none() {
        assert_eq!(
            Harness::ClaudeCode.launch(&glm(), None),
            Err(Refusal::MissingKey)
        );
        assert_eq!(
            Harness::ClaudeCode.launch(&glm(), Some("  ")),
            Err(Refusal::MissingKey)
        );
        let ollama = connection("ollama", Protocol::Anthropic, "http://localhost:11434");
        let launch = Harness::ClaudeCode.launch(&ollama, None).expect("渡せる");
        assert_eq!(launch.env["ANTHROPIC_AUTH_TOKEN"], "ollama");
    }

    #[test]
    fn base_urls_and_ids_are_checked_before_saving() {
        for good in [
            "https://api.z.ai/api/anthropic",
            "http://localhost:11434",
            "http://127.0.0.1:1234/v1",
        ] {
            assert!(is_valid_base_url(good), "{good}");
        }
        for bad in [
            "",
            "api.z.ai",
            "ftp://x",
            "https://",
            "https://:80",
            "https://a b",
        ] {
            assert!(!is_valid_base_url(bad), "{bad}");
        }
        assert!(is_valid_connection_id("zai-coding-plan-2"));
        assert!(!is_valid_connection_id("a/b"));
        assert!(!is_valid_connection_id(""));
    }

    /// 接続のキーを渡すセッションでは、task.env から宛先・プロキシ・差し込むコードを読まない。
    #[test]
    fn task_env_cannot_reroute_a_connection() {
        for name in [
            "ANTHROPIC_BASE_URL",
            "anthropic_custom_headers",
            "OPENAI_BASE_URL",
            "OPENCODE_CONFIG",
            "HTTPS_PROXY",
            "https_proxy",
            "NODE_OPTIONS",
            "NODE_EXTRA_CA_CERTS",
            "DYLD_INSERT_LIBRARIES",
            "ZHIPU_API_KEY",
            "DEEPSEEK_API_KEY",
        ] {
            assert!(guarded_from_task_env(name), "{name}");
        }
        for name in ["CARGO_TARGET_DIR", "DATABASE_URL", "RUST_LOG", "PATH"] {
            assert!(!guarded_from_task_env(name), "{name}");
        }
    }

    /// ログインの指紋は、置き場の変数を替えると変わり、ファイルが変わると変わる。中身は読まない。
    #[test]
    fn the_login_fingerprint_follows_the_account_folder() {
        let dir = std::env::temp_dir().join(format!(
            "necoder_login_fingerprint_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let work = dir.join("work");
        let personal = dir.join("personal");
        std::fs::create_dir_all(&work).expect("mkdir");
        std::fs::create_dir_all(&personal).expect("mkdir");
        let env_for = |root: &Path| {
            BTreeMap::from([(
                "CODEX_HOME".to_string(),
                root.to_string_lossy().into_owned(),
            )])
        };
        let before = login_fingerprint("codex", &env_for(&work));
        assert_eq!(
            before,
            login_fingerprint("codex", &env_for(&work)),
            "同じなら同じ"
        );
        assert_ne!(before, login_fingerprint("codex", &env_for(&personal)));
        std::fs::write(work.join("auth.json"), "{}").expect("write");
        assert_ne!(
            before,
            login_fingerprint("codex", &env_for(&work)),
            "ログインし直すと変わる"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ひな形の表は形が揃っている（id は重ならない・口は http(s)・汎用の 2 つだけが空の口を持つ）。
    #[test]
    fn presets_are_well_formed() {
        let mut ids: Vec<&str> = PRESETS.iter().map(|preset| preset.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PRESETS.len());
        for preset in PRESETS {
            assert!(!preset.protocols().is_empty(), "{}", preset.id);
            let generic = preset.id == OPENAI_COMPATIBLE || preset.id == ANTHROPIC_COMPATIBLE;
            assert_eq!(preset.name.is_none(), generic, "{}", preset.id);
            for protocol in preset.protocols() {
                let url = preset.base_url(protocol).unwrap_or_default();
                assert_eq!(url.is_empty(), generic, "{}", preset.id);
                assert!(generic || is_valid_base_url(url), "{}: {url}", preset.id);
            }
            assert_eq!(
                preset.needs_key,
                preset.placeholder_token.is_none(),
                "{}",
                preset.id
            );
        }
    }
}
