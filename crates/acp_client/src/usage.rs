//! usage — 使用量とレート制限（O11）。エージェントが送ってくる値を、UI 非依存の型へ写す。
//!
//! **necoder は利用上限を自分で問い合わせない**（資格情報に触れない・非公開の API を叩かない）。
//! ここへ来るのは、エージェントが ACP で自分から送ってきた値だけ:
//!
//! - **Claude**（claude-agent-acp 0.81.1）: Claude Code が `rate_limit_event` を出すと、adapter は
//!   `usage_update` の `_meta["_claude/rateLimit"]` に SDK の `SDKRateLimitInfo` をそのまま載せる
//!   （`dist/acp-agent.js` 4620-4632）。形は SDK 0.3.280 の `sdk.d.ts`（5423-5447）:
//!   `status`（`allowed` / `allowed_warning` / `rejected`）・`resetsAt`（unix 秒）・`rateLimitType`
//!   （`five_hour` / `seven_day` / `seven_day_opus` …）・`utilization`（**0〜1 の割合**。Claude Code 本体が
//!   ヘッダ値を 0〜1 に丸めて持つ）。公開の型には無い `unifiedWindows`（5 時間・週・追加枠込みの週の
//!   全窓 `{utilization, resetsAt}`）も同じ物に載る。Claude Code は窓の使用率の整数 % が変わるたびに
//!   これを出し直す。
//! - **ターンの終わり**: `PromptResponse._meta.quota`（claude-agent-acp 6862-6897 / codex-acp 1.13.1 の
//!   `buildQuotaMeta`）。`token_count` とモデル別の `model_usage[].token_count`（camelCase のカウンタ）。
//! - **Codex**: ACP には rate limit を出さない（`/status` の文だけ）。`codex app-server` の
//!   `account/rateLimits/read` の結果を [`rate_limits_from_codex`] で読む（[`crate::codex_limits`]）。

use serde_json::Value;

/// レート制限の窓（どの期間の上限か）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LimitWindow {
    /// 5 時間枠（Claude `five_hour` / Codex の 300 分の窓）。
    FiveHour,
    /// 週枠（Claude `seven_day` / Codex の 10080 分の窓）。
    Weekly,
    /// モデル別の週枠・追加の使用枠など、名前の付いた窓（Claude の `rateLimitType` の綴りのまま）。
    Named(String),
    /// 5 時間・週以外の長さの窓（Codex の `windowDurationMins`・分）。
    Minutes(u64),
}

impl LimitWindow {
    /// Claude の `rateLimitType`（`unifiedWindows` のキーも同じ綴り）から。
    pub fn from_claude(kind: &str) -> LimitWindow {
        match kind {
            "five_hour" => LimitWindow::FiveHour,
            "seven_day" => LimitWindow::Weekly,
            other => LimitWindow::Named(other.to_string()),
        }
    }

    /// 窓の長さ（分）から（Codex）。300 分 = 5 時間枠、10080 分 = 週枠。
    pub fn from_minutes(minutes: u64) -> LimitWindow {
        match minutes {
            300 => LimitWindow::FiveHour,
            10_080 => LimitWindow::Weekly,
            other => LimitWindow::Minutes(other),
        }
    }
}

/// 全体の状態（Claude の `status`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStatus {
    /// 使える。
    Allowed,
    /// 使えるが上限が近い（Claude が警告を出した）。
    Warning,
    /// 上限に達して止められている。
    Rejected,
}

/// 1 つの窓の値。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowUsage {
    pub window: LimitWindow,
    /// 使った割合（0〜100）。エージェントが言わなければ `None`。
    pub used_percent: Option<f64>,
    /// 窓がリセットされる時刻（unix 秒）。
    pub resets_at: Option<i64>,
}

/// エージェントが知らせてきたレート制限（1 回分）。
///
/// 1 回の知らせに全部の窓が載るとは限らない（Claude は代表の窓だけのことがある）ので、
/// 受け手は**窓ごとに**差し替える。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RateLimits {
    /// 全体の状態。Codex の app-server には無い（`None`）。
    pub status: Option<LimitStatus>,
    /// 届いた窓（同じ窓は 1 つだけ）。
    pub windows: Vec<WindowUsage>,
}

/// 1 ターンに使ったトークン（`PromptResponse._meta.quota`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TurnTokens {
    /// 入力（キャッシュの読み書きを除く）。
    pub input: u64,
    pub output: u64,
    /// キャッシュから読んだ入力。
    pub cached_read: u64,
    /// キャッシュへ書いた入力（Claude だけが数える）。
    pub cached_write: u64,
    /// 合計（エージェントが数えた値。無ければ内訳の和）。
    pub total: u64,
}

impl TurnTokens {
    fn add(self, other: TurnTokens) -> TurnTokens {
        TurnTokens {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cached_read: self.cached_read.saturating_add(other.cached_read),
            cached_write: self.cached_write.saturating_add(other.cached_write),
            total: self.total.saturating_add(other.total),
        }
    }
}

/// `_meta["_claude/rateLimit"]`（SDK の `SDKRateLimitInfo`）を読む。状態も窓も無ければ `None`。
///
/// 窓は `unifiedWindows`（全窓）を先に読み、代表の窓（`rateLimitType` + `utilization` + `resetsAt`）を
/// 上に重ねる。代表の窓は `status` が `allowed` の間は `utilization` を持たないことがあり、その時は
/// `unifiedWindows` の値を残す。`rejected` で使用率が無い代表の窓は、上限に達した窓なので 100% とみなす。
pub fn rate_limits_from_claude(info: &Value) -> Option<RateLimits> {
    let object = info.as_object()?;
    let status = match object.get("status").and_then(Value::as_str) {
        Some("allowed") => Some(LimitStatus::Allowed),
        Some("allowed_warning") => Some(LimitStatus::Warning),
        Some("rejected") => Some(LimitStatus::Rejected),
        _ => None,
    };
    let mut windows: Vec<WindowUsage> = Vec::new();
    if let Some(unified) = object.get("unifiedWindows").and_then(Value::as_object) {
        for (kind, reading) in unified {
            let used_percent = reading
                .get("utilization")
                .and_then(Value::as_f64)
                .and_then(percent_from_utilization);
            let resets_at = reading.get("resetsAt").and_then(unix_seconds);
            if used_percent.is_none() && resets_at.is_none() {
                continue;
            }
            merge_window(
                &mut windows,
                WindowUsage {
                    window: LimitWindow::from_claude(kind),
                    used_percent,
                    resets_at,
                },
            );
        }
    }
    if let Some(kind) = object.get("rateLimitType").and_then(Value::as_str) {
        let mut used_percent = object
            .get("utilization")
            .and_then(Value::as_f64)
            .and_then(percent_from_utilization);
        if used_percent.is_none() && status == Some(LimitStatus::Rejected) {
            used_percent = Some(100.0);
        }
        let resets_at = object.get("resetsAt").and_then(unix_seconds);
        if used_percent.is_some() || resets_at.is_some() {
            merge_window(
                &mut windows,
                WindowUsage {
                    window: LimitWindow::from_claude(kind),
                    used_percent,
                    resets_at,
                },
            );
        }
    }
    if status.is_none() && windows.is_empty() {
        return None;
    }
    Some(RateLimits { status, windows })
}

/// `codex app-server` の `account/rateLimits/read` の結果（`GetAccountRateLimitsResponse`）を読む。
///
/// 使うのは後方互換の単一バケット `rateLimits` の `primary` / `secondary` だけ（codex 0.144.6 の
/// `generate-json-schema` で確かめた形: `usedPercent` は 0〜100 の整数、`resetsAt` は unix 秒、
/// `windowDurationMins` は窓の長さ）。長さが無い窓は種類が分からないので `primary` / `secondary` の名前で持つ。
pub fn rate_limits_from_codex(result: &Value) -> Option<RateLimits> {
    let snapshot = result.get("rateLimits")?.as_object()?;
    let mut windows = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(reading) = snapshot.get(key).and_then(Value::as_object) else {
            continue;
        };
        let used_percent = reading
            .get("usedPercent")
            .and_then(Value::as_f64)
            .filter(|percent| percent.is_finite())
            .map(|percent| percent.clamp(0.0, 100.0));
        let resets_at = reading.get("resetsAt").and_then(unix_seconds);
        if used_percent.is_none() && resets_at.is_none() {
            continue;
        }
        let window = match reading.get("windowDurationMins").and_then(Value::as_u64) {
            Some(minutes) => LimitWindow::from_minutes(minutes),
            None => LimitWindow::Named(key.to_string()),
        };
        merge_window(
            &mut windows,
            WindowUsage {
                window,
                used_percent,
                resets_at,
            },
        );
    }
    if windows.is_empty() {
        return None;
    }
    Some(RateLimits {
        status: None,
        windows,
    })
}

/// `PromptResponse._meta` の `quota` からこのターンのトークンを読む。
///
/// モデル別の行（`model_usage`）があればその合計を取る — Claude では `token_count` がメインの
/// ループだけなのに対し、`model_usage` はサブエージェント・圧縮まで数える会計用の値（adapter の
/// `turnQuotaMeta` のコメント）。行が無ければ `token_count`。Codex はどちらも「ターンの最後の
/// 要求」の分（`thread/tokenUsage/updated` の `last`）で、複数回モデルを呼んだターンは少なめに出る。
pub fn turn_tokens_from_quota(meta: &serde_json::Map<String, Value>) -> Option<TurnTokens> {
    let quota = meta.get("quota")?;
    let per_model = quota
        .get("model_usage")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("token_count"))
        .filter_map(token_count_from)
        .reduce(TurnTokens::add);
    per_model.or_else(|| quota.get("token_count").and_then(token_count_from))
}

/// `token_count` 1 つ（`{totalTokens, inputTokens, cachedInputTokens, cachedWriteTokens?,
/// outputTokens, reasoningOutputTokens}`）。全部 0 なら「使っていない」として `None`。
fn token_count_from(value: &Value) -> Option<TurnTokens> {
    let object = value.as_object()?;
    let count = |key: &str| object.get(key).and_then(token_count).unwrap_or(0);
    let mut tokens = TurnTokens {
        input: count("inputTokens"),
        output: count("outputTokens"),
        cached_read: count("cachedInputTokens"),
        cached_write: count("cachedWriteTokens"),
        total: count("totalTokens"),
    };
    if tokens.total == 0 {
        tokens.total = tokens
            .input
            .saturating_add(tokens.output)
            .saturating_add(tokens.cached_read)
            .saturating_add(tokens.cached_write);
    }
    (tokens.total > 0).then_some(tokens)
}

/// トークン数 1 つ。整数でも、整数値の浮動小数（JS の number）でも受ける。
fn token_count(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|count| count.is_finite() && *count >= 0.0)
            .map(|count| count.round() as u64)
    })
}

/// unix 秒 1 つ。整数でも浮動小数でも受ける。
fn unix_seconds(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|seconds| seconds.is_finite())
            .map(|seconds| seconds.round() as i64)
    })
}

/// Claude の `utilization`（0〜1 の割合）を 0〜100 の % へ。1 を超える値は、すでに % で来たものとして
/// そのまま扱う（Claude Code は今は 0〜1 に丸めているが、同じ SDK の `/usage` の API は 0〜100 で
/// 返すので、綴りが揃った時に 4200% と出さないため）。
fn percent_from_utilization(utilization: f64) -> Option<f64> {
    if !utilization.is_finite() || utilization < 0.0 {
        return None;
    }
    let percent = if utilization <= 1.0 {
        utilization * 100.0
    } else {
        utilization
    };
    Some(percent.min(100.0))
}

/// 同じ窓があれば、新しい方の**分かっている値だけ**で上書きする（無い値で消さない）。
fn merge_window(windows: &mut Vec<WindowUsage>, incoming: WindowUsage) {
    match windows
        .iter_mut()
        .find(|window| window.window == incoming.window)
    {
        Some(existing) => {
            if incoming.used_percent.is_some() {
                existing.used_percent = incoming.used_percent;
            }
            if incoming.resets_at.is_some() {
                existing.resets_at = incoming.resets_at;
            }
        }
        None => windows.push(incoming),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// claude-agent-acp が `rate_limit_event` から作る `_meta["_claude/rateLimit"]` の実際の形
    /// （SDK の `SDKRateLimitInfo` + 内部の `unifiedWindows`。Claude Code の `Sa()` が組む並び）。
    #[test]
    fn claude_rate_limit_meta_becomes_windows() {
        let info = json!({
            "status": "allowed_warning",
            "resetsAt": 1_790_000_000,
            "rateLimitType": "five_hour",
            "utilization": 0.92,
            "isUsingOverage": false,
            "surpassedThreshold": 0.9,
            "unifiedWindows": {
                "five_hour": {"utilization": 0.91, "resetsAt": 1_790_000_000},
                "seven_day": {"utilization": 0.18, "resetsAt": 1_790_400_000},
                "seven_day_overage_included": {"utilization": 0.05, "resetsAt": 1_790_400_000}
            }
        });
        let limits = rate_limits_from_claude(&info).expect("窓が読める");
        assert_eq!(limits.status, Some(LimitStatus::Warning));
        let find = |window: LimitWindow| {
            limits
                .windows
                .iter()
                .find(|reading| reading.window == window)
                .cloned()
        };
        let five_hour = find(LimitWindow::FiveHour).expect("5 時間枠");
        // 代表の窓（公開の欄）が unifiedWindows の同じ窓を上書きする。0〜1 は % へ。
        assert!((five_hour.used_percent.expect("使用率") - 92.0).abs() < 1e-9);
        assert_eq!(five_hour.resets_at, Some(1_790_000_000));
        let weekly = find(LimitWindow::Weekly).expect("週枠");
        assert!((weekly.used_percent.expect("使用率") - 18.0).abs() < 1e-9);
        assert_eq!(weekly.resets_at, Some(1_790_400_000));
        assert!(find(LimitWindow::Named("seven_day_overage_included".into())).is_some());
        assert_eq!(limits.windows.len(), 3, "同じ窓は 1 つだけ: {limits:?}");
    }

    /// `allowed` の間は代表の窓が使用率を持たないことがある。その時は unifiedWindows の値を残し、
    /// 公開の型だけ（unifiedWindows の無い古い Claude Code）でも読める。
    #[test]
    fn claude_rate_limit_keeps_known_values_and_works_without_unified_windows() {
        let info = json!({
            "status": "allowed",
            "resetsAt": 1_790_000_000,
            "rateLimitType": "five_hour",
            "unifiedWindows": {"five_hour": {"utilization": 0.42, "resetsAt": 1_790_000_000}}
        });
        let limits = rate_limits_from_claude(&info).expect("読める");
        assert_eq!(limits.status, Some(LimitStatus::Allowed));
        assert_eq!(limits.windows.len(), 1);
        assert!((limits.windows[0].used_percent.expect("使用率") - 42.0).abs() < 1e-9);

        let public_only = json!({
            "status": "allowed_warning",
            "resetsAt": 1_790_400_000,
            "rateLimitType": "seven_day_opus",
            "utilization": 0.8
        });
        let limits = rate_limits_from_claude(&public_only).expect("読める");
        assert_eq!(
            limits.windows,
            vec![WindowUsage {
                window: LimitWindow::Named("seven_day_opus".into()),
                used_percent: Some(80.0),
                resets_at: Some(1_790_400_000),
            }]
        );
    }

    /// 止められた（`rejected`）代表の窓は、使用率が無くても 100% とみなす。状態だけの知らせも捨てない
    /// （前の `rejected` を解くのに要る）。形の違う値は読まない。
    #[test]
    fn claude_rejection_and_status_only_updates() {
        let rejected = json!({
            "status": "rejected",
            "resetsAt": 1_790_000_000,
            "rateLimitType": "five_hour",
            "isUsingOverage": false
        });
        let limits = rate_limits_from_claude(&rejected).expect("読める");
        assert_eq!(limits.status, Some(LimitStatus::Rejected));
        assert_eq!(limits.windows[0].used_percent, Some(100.0));

        let status_only = rate_limits_from_claude(&json!({"status": "allowed"})).expect("読める");
        assert_eq!(status_only.status, Some(LimitStatus::Allowed));
        assert!(status_only.windows.is_empty());

        assert_eq!(rate_limits_from_claude(&json!("five_hour")), None);
        assert_eq!(rate_limits_from_claude(&json!({"foo": 1})), None);
        // 負や NaN 相当の使用率は捨てる。1 を超える値は % として扱う（4200% にしない）。
        let odd = json!({
            "rateLimitType": "five_hour",
            "utilization": 42,
            "unifiedWindows": {"seven_day": {"utilization": -1, "resetsAt": 1_790_400_000}}
        });
        let limits = rate_limits_from_claude(&odd).expect("読める");
        let five_hour = limits
            .windows
            .iter()
            .find(|reading| reading.window == LimitWindow::FiveHour)
            .expect("5 時間枠");
        assert_eq!(five_hour.used_percent, Some(42.0));
        let weekly = limits
            .windows
            .iter()
            .find(|reading| reading.window == LimitWindow::Weekly)
            .expect("週枠（リセット時刻だけ）");
        assert_eq!(weekly.used_percent, None);
    }

    /// 窓の分類: Claude は `rateLimitType` の綴り、Codex は窓の長さ（300 分 / 10080 分）。
    #[test]
    fn windows_are_classified_by_name_or_length() {
        assert_eq!(LimitWindow::from_claude("five_hour"), LimitWindow::FiveHour);
        assert_eq!(LimitWindow::from_claude("seven_day"), LimitWindow::Weekly);
        assert_eq!(
            LimitWindow::from_claude("seven_day_sonnet"),
            LimitWindow::Named("seven_day_sonnet".into())
        );
        assert_eq!(
            LimitWindow::from_claude("overage"),
            LimitWindow::Named("overage".into())
        );
        assert_eq!(LimitWindow::from_minutes(300), LimitWindow::FiveHour);
        assert_eq!(LimitWindow::from_minutes(10_080), LimitWindow::Weekly);
        assert_eq!(LimitWindow::from_minutes(60), LimitWindow::Minutes(60));
    }

    /// `codex app-server` の `account/rateLimits/read` の結果（生成したスキーマの形）。
    #[test]
    fn codex_rate_limits_response_becomes_windows() {
        let result = json!({
            "rateLimits": {
                "limitId": "codex",
                "limitName": null,
                "planType": "plus",
                "primary": {"usedPercent": 12, "windowDurationMins": 300, "resetsAt": 1_790_000_000},
                "secondary": {"usedPercent": 30, "windowDurationMins": 10080, "resetsAt": 1_790_400_000},
                "credits": {"hasCredits": false, "unlimited": false, "balance": null}
            },
            "rateLimitsByLimitId": null,
            "rateLimitResetCredits": null
        });
        let limits = rate_limits_from_codex(&result).expect("読める");
        assert_eq!(limits.status, None);
        assert_eq!(
            limits.windows,
            vec![
                WindowUsage {
                    window: LimitWindow::FiveHour,
                    used_percent: Some(12.0),
                    resets_at: Some(1_790_000_000),
                },
                WindowUsage {
                    window: LimitWindow::Weekly,
                    used_percent: Some(30.0),
                    resets_at: Some(1_790_400_000),
                },
            ]
        );
        // 長さの無い窓は種類を当て推量しない。窓が 1 つも無ければ None。
        let unknown =
            json!({"rateLimits": {"primary": {"usedPercent": 5, "windowDurationMins": null}}});
        assert_eq!(
            rate_limits_from_codex(&unknown).expect("読める").windows[0].window,
            LimitWindow::Named("primary".into())
        );
        assert_eq!(
            rate_limits_from_codex(&json!({"rateLimits": {"primary": null, "secondary": null}})),
            None
        );
    }

    /// Claude の `_meta.quota`: モデル別の行の合計（サブエージェントまで数える）を取る。
    #[test]
    fn claude_quota_sums_model_usage_rows() {
        let meta = json!({
            "quota": {
                "token_count": {
                    "totalTokens": 1_500, "inputTokens": 100, "cachedInputTokens": 1_000,
                    "cachedWriteTokens": 200, "outputTokens": 200, "reasoningOutputTokens": 0
                },
                "model_usage": [
                    {"model": "claude-opus-5[1m]", "token_count": {
                        "totalTokens": 1_500, "inputTokens": 100, "cachedInputTokens": 1_000,
                        "cachedWriteTokens": 200, "outputTokens": 200, "reasoningOutputTokens": 0}},
                    {"model": "claude-haiku-4-5", "token_count": {
                        "totalTokens": 300, "inputTokens": 250, "cachedInputTokens": 0,
                        "cachedWriteTokens": 0, "outputTokens": 50, "reasoningOutputTokens": 0}}
                ]
            }
        });
        let meta = meta.as_object().expect("object").clone();
        assert_eq!(
            turn_tokens_from_quota(&meta),
            Some(TurnTokens {
                input: 350,
                output: 250,
                cached_read: 1_000,
                cached_write: 200,
                total: 1_800,
            })
        );
    }

    /// Codex の `_meta.quota`（`cachedWriteTokens` が無い）。モデル別の行が無ければ `token_count`、
    /// 何も無い・全部 0 なら None。
    #[test]
    fn codex_quota_and_missing_quota() {
        let meta = json!({
            "quota": {
                "token_count": {
                    "totalTokens": 5_000, "inputTokens": 1_000, "cachedInputTokens": 3_500,
                    "outputTokens": 500, "reasoningOutputTokens": 120
                },
                "model_usage": []
            }
        });
        let meta = meta.as_object().expect("object").clone();
        assert_eq!(
            turn_tokens_from_quota(&meta),
            Some(TurnTokens {
                input: 1_000,
                output: 500,
                cached_read: 3_500,
                cached_write: 0,
                total: 5_000,
            })
        );
        let empty = json!({"quota": {"token_count": null, "model_usage": []}});
        assert_eq!(
            turn_tokens_from_quota(empty.as_object().expect("object")),
            None
        );
        let zero = json!({"quota": {"token_count": {"totalTokens": 0, "inputTokens": 0}}});
        assert_eq!(
            turn_tokens_from_quota(zero.as_object().expect("object")),
            None
        );
        let unrelated = json!({"claudeCode": {"terminal": true}});
        assert_eq!(
            turn_tokens_from_quota(unrelated.as_object().expect("object")),
            None
        );
    }
}
