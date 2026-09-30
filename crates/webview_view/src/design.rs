//! design — Web タブの **Design Mode**（ページの要素を選んで、その HTML・計算済みスタイル・切り抜きを
//! エージェントへの入力に添える）のうち、OS にも GPUI にも依らない部分。
//!
//! - [`picker_script`]: Web タブの最上位の文書に文書の頭で入れておくピッカー（`design_picker.js`）。
//!   普段は何もせず、[`start_script`] で nonce を渡した間だけホバー枠とクリックを受ける。
//! - [`accept_message`]: ページ → necoder の IPC を受け入れてよいか。**Design 中**かつ **nonce が一致**し、
//!   形と大きさが決まりどおりのものだけ通す（それ以外は全部捨てる）。IPC の口は生成時にしか付けられず
//!   Web タブの WebView に常に付いているので、ここが唯一の関所になる。
//! - [`format_for_prompt`]: 受け取った要素を composer に差し込む文章（Markdown）にする。
//!
//! 受け取った値はページ（= 利用者の開発サーバ）が作ったものなので、ここでも URL の query を落とし、
//! secret らしい値を伏せ直す（ピッカー側でも同じことをしている＝二重）。

use regex::Regex;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// ピッカー本体（`design_picker.js`）。出典は同ファイルの冒頭（stablyai/orca・MIT）。
const PICKER_SOURCE: &str = include_str!("design_picker.js");

/// debug ビルドだけに足す検証用の口（`__necoderDesignDebug`）。本番のページには出さない。
#[cfg(debug_assertions)]
const DEBUG_HOOKS: &str = r#"
  Object.defineProperty(window, '__necoderDesignDebug', {
    value: Object.freeze({
      active: function () { return !!state; },
      hover: function (selector) {
        var element = document.querySelector(selector);
        if (!state || !element) { return false; }
        highlight(element);
        return true;
      },
      pick: function (selector, multi) {
        var element = document.querySelector(selector);
        if (!state || !element) { return false; }
        highlight(element);
        pick(element, !!multi);
        return true;
      }
    }),
    writable: false,
    configurable: false,
    enumerable: false
  });
"#;

/// Web タブに入れるピッカーのスクリプト。
pub fn picker_script() -> String {
    #[cfg(debug_assertions)]
    {
        PICKER_SOURCE.replace("/* necoder:debug-hooks */", DEBUG_HOOKS)
    }
    #[cfg(not(debug_assertions))]
    {
        PICKER_SOURCE.to_string()
    }
}

/// Design を始めるスクリプト（nonce は 16 進だけなので埋め込んでも崩れない）。
pub fn start_script(nonce: &str) -> String {
    format!("window.__necoderDesign ? window.__necoderDesign.start({nonce:?}) : false")
}

pub const STOP_SCRIPT: &str = "window.__necoderDesign ? window.__necoderDesign.stop() : false";
pub const RESUME_SCRIPT: &str = "window.__necoderDesign ? window.__necoderDesign.resume() : false";

/// Design を始めるたびに作る使い捨ての合言葉（32 桁の 16 進）。ページの script は知らないので、
/// Design 中にページが勝手に `postMessage` しても通らない。前の Design の遅れた知らせも通らない。
pub fn new_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u128(now);
    first.write_u64(serial);
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u64(first.finish());
    second.write_u64(serial.rotate_left(17));
    format!("{:016x}{:016x}", first.finish(), second.finish())
}

// ── 受け取る形と大きさの上限 ──

/// IPC 1 通の上限。これを超えたら中身を見ずに捨てる。
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// HTML の抜粋の上限（文字数）。
pub const MAX_HTML_CHARS: usize = 8 * 1024;
/// 要素のテキストの上限（文字数）。
pub const MAX_TEXT_CHARS: usize = 2 * 1024;
const MAX_SHORT_CHARS: usize = 1024;
const MAX_NEARBY_ENTRIES: usize = 8;
const MAX_ATTRIBUTES: usize = 24;
const MAX_COMPONENTS: usize = 12;

/// 受け取る計算済みスタイル（この 16 項目だけ）。
pub const STYLE_PROPERTIES: [&str; 16] = [
    "display",
    "position",
    "width",
    "height",
    "margin",
    "padding",
    "color",
    "background-color",
    "border",
    "border-radius",
    "font-family",
    "font-size",
    "font-weight",
    "line-height",
    "text-align",
    "z-index",
];

/// 名前がこれらしい属性（`data-api-key` など）の値は伏せる。
const SECRET_NAME_PATTERN: &str = r"(?i)(access_token|auth_token|api[_-]?key|apikey|client_secret|oauth_state|session[_-]?id|csrf|secret|passwd|password|token|x-amz-)";
/// 値の中のこれは伏せる: 「鍵=値」「鍵: 値」・`Bearer …`・AWS の署名・JWT らしい物。`type="password"` や
/// `class="password-field"` のような語そのものは伏せない（情報を失うだけで守る物が無い）。
const SECRET_VALUE_PATTERN: &str = r"(?i)((access_token|auth_token|api[_-]?key|apikey|client_secret|oauth_state|session[_-]?id|csrf[_-]?token|csrf|secret|password|passwd|token)\s*[=:])|(bearer\s+\S)|(x-amz-)|(eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})";

const REDACTED: &str = "[redacted]";

/// 選ばれた要素（ピッカーが集めた物を検証・整えた後）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementCapture {
    pub page: PageInfo,
    pub element: ElementInfo,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInfo {
    /// query と fragment を落とした URL。
    pub url: String,
    pub title: String,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub device_pixel_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementInfo {
    pub tag: String,
    pub selector: String,
    /// 人が読む祖先の道筋（`main.card > button#start`）。
    pub path: String,
    pub text: String,
    pub nearby_text: Vec<String>,
    pub html: String,
    pub role: Option<String>,
    pub accessible_name: Option<String>,
    pub attributes: Vec<Attribute>,
    /// ビューポート基準の矩形（CSS px）。
    pub rect: Rect,
    /// 計算済みスタイル（[`STYLE_PROPERTIES`] の中だけ）。
    pub styles: BTreeMap<String, String>,
    /// React のコンポーネント名（外 → 内）。開発ビルドでなければ空。
    pub components: Vec<String>,
    pub source: Option<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 要素を書いたソースの位置（開発ビルドで取れた時だけ）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    /// どこから取ったか（`debugSource` / `debugStack` / `data-inspector` / `data-source`）。
    pub via: String,
}

/// ページからの知らせ（検証済み）。
#[derive(Debug, Clone, PartialEq)]
pub enum DesignMessage {
    /// 要素を 1 つ選んだ。`multi` = ⇧ クリック（続けて選ぶ）。
    Pick {
        multi: bool,
        capture: Box<ElementCapture>,
    },
    /// ページの中で Esc が押された。
    Cancel,
}

/// 捨てた理由（ログと unit test 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// Design 中ではない（開始前・終了後の遅れた知らせ・ページが勝手に送った物）。
    NotActive,
    TooLarge,
    Malformed,
    WrongNonce,
    UnknownType,
    /// その欄が上限を超えた。
    FieldTooLarge(&'static str),
    UnknownStyle(String),
    BadNumber,
    /// localhost の外の URL（Web タブの最上位の文書はそこから出ないので、あり得ない＝壊れている）。
    NotLocalhost,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMessage {
    #[serde(rename = "type")]
    kind: String,
    nonce: String,
    #[serde(default)]
    multi: bool,
    #[serde(default)]
    capture: Option<ElementCapture>,
}

/// IPC の本文を受け入れてよいか確かめる。`active_nonce` = 今の Design の合言葉（Design 中でなければ
/// `None`）。`sender` = 送り手の文書の URL（wry が添える物）— ページに埋め込まれた外部の iframe も
/// `window.webkit.messageHandlers` に届くので、localhost の文書から来た物だけを通す（合言葉は最上位の
/// 文書にしか渡していないので、これは二重の守り）。通った物は URL と secret を整え直して返す。
pub fn accept_message(
    active_nonce: Option<&str>,
    sender: &str,
    body: &str,
) -> Result<DesignMessage, Rejected> {
    let Some(active_nonce) = active_nonce else {
        return Err(Rejected::NotActive);
    };
    if !crate::localhost::is_allowed(sender) {
        return Err(Rejected::NotLocalhost);
    }
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(Rejected::TooLarge);
    }
    let raw: RawMessage = serde_json::from_str(body).map_err(|_| Rejected::Malformed)?;
    if raw.nonce != active_nonce {
        return Err(Rejected::WrongNonce);
    }
    match raw.kind.as_str() {
        "cancel" => Ok(DesignMessage::Cancel),
        "pick" => {
            let mut capture = raw.capture.ok_or(Rejected::Malformed)?;
            validate(&capture)?;
            sanitize(&mut capture);
            Ok(DesignMessage::Pick {
                multi: raw.multi,
                capture: Box::new(capture),
            })
        }
        _ => Err(Rejected::UnknownType),
    }
}

fn within(text: &str, limit: usize, field: &'static str) -> Result<(), Rejected> {
    if text.chars().count() > limit {
        return Err(Rejected::FieldTooLarge(field));
    }
    Ok(())
}

fn validate(capture: &ElementCapture) -> Result<(), Rejected> {
    let page = &capture.page;
    let element = &capture.element;
    if !crate::localhost::is_allowed(&page.url) {
        return Err(Rejected::NotLocalhost);
    }
    within(&page.url, 2048, "url")?;
    within(&page.title, MAX_SHORT_CHARS, "title")?;
    within(&element.tag, 64, "tag")?;
    within(&element.selector, MAX_SHORT_CHARS, "selector")?;
    within(&element.path, MAX_SHORT_CHARS, "path")?;
    within(&element.text, MAX_TEXT_CHARS, "text")?;
    within(&element.html, MAX_HTML_CHARS, "html")?;
    if element.nearby_text.len() > MAX_NEARBY_ENTRIES {
        return Err(Rejected::FieldTooLarge("nearby_text"));
    }
    for text in &element.nearby_text {
        within(text, 256, "nearby_text")?;
    }
    if let Some(role) = &element.role {
        within(role, 256, "role")?;
    }
    if let Some(name) = &element.accessible_name {
        within(name, 256, "accessible_name")?;
    }
    if element.attributes.len() > MAX_ATTRIBUTES {
        return Err(Rejected::FieldTooLarge("attributes"));
    }
    for attribute in &element.attributes {
        within(&attribute.name, 64, "attributes")?;
        within(&attribute.value, MAX_SHORT_CHARS, "attributes")?;
    }
    if element.styles.len() > STYLE_PROPERTIES.len() {
        return Err(Rejected::FieldTooLarge("styles"));
    }
    for (name, value) in &element.styles {
        if !STYLE_PROPERTIES.contains(&name.as_str()) {
            return Err(Rejected::UnknownStyle(name.clone()));
        }
        within(value, 256, "styles")?;
    }
    if element.components.len() > MAX_COMPONENTS {
        return Err(Rejected::FieldTooLarge("components"));
    }
    for name in &element.components {
        within(name, 128, "components")?;
    }
    if let Some(source) = &element.source {
        within(&source.file, MAX_SHORT_CHARS, "source")?;
        within(&source.via, 32, "source")?;
    }
    let numbers = [
        page.viewport_width,
        page.viewport_height,
        page.device_pixel_ratio,
        element.rect.x,
        element.rect.y,
        element.rect.width,
        element.rect.height,
    ];
    if numbers.iter().any(|number| !number.is_finite())
        || element.rect.width < 0.0
        || element.rect.height < 0.0
        || page.viewport_width <= 0.0
        || page.viewport_height <= 0.0
    {
        return Err(Rejected::BadNumber);
    }
    Ok(())
}

/// ピッカーが整えた後でも、もう一度 URL の query を落とし secret らしい値を伏せる（ページの script が
/// ピッカーを通さずに作った物でも、合言葉が漏れていれば届き得るため）。
fn sanitize(capture: &mut ElementCapture) {
    capture.page.url = strip_query(&capture.page.url);
    let element = &mut capture.element;
    for attribute in &mut element.attributes {
        let name = attribute.name.to_ascii_lowercase();
        // URL は query を落としてから見る（トークンは大抵 query に乗る。道筋まで伏せない）。
        if matches!(name.as_str(), "href" | "src" | "action") {
            attribute.value = strip_query(&attribute.value);
        }
        if secret_name(&name) || looks_secret(&attribute.value) {
            attribute.value = REDACTED.to_string();
        }
    }
    element.html = redact_html(&element.html);
    if let Some(source) = &element.source {
        if looks_secret(&source.file) {
            element.source = None;
        }
    }
}

/// URL（相対も）の `?` と `#` 以降を落とす。
pub fn strip_query(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or_default().to_string()
}

/// secret らしい値か（「鍵=値」・`Bearer …`・JWT など。語そのものは含めない）。
pub fn looks_secret(value: &str) -> bool {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    // 正規表現は定数なので失敗しない。万一の時は伏せる側に倒す。
    PATTERN
        .get_or_init(|| Regex::new(SECRET_VALUE_PATTERN).ok())
        .as_ref()
        .is_none_or(|pattern| pattern.is_match(value))
}

/// 名前が secret らしい属性か（`data-api-key` / `data-token` など）。
pub fn secret_name(name: &str) -> bool {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN
        .get_or_init(|| Regex::new(SECRET_NAME_PATTERN).ok())
        .as_ref()
        .is_none_or(|pattern| pattern.is_match(name))
}

/// HTML の抜粋の属性を伏せ直す: password 入力の `value`・secret らしい名前 / 値の属性・URL 属性の query。
/// 相手はブラウザが書き出した `outerHTML`（属性値は必ず `"` で囲まれ、中の `"` は `&quot;`）なので、
/// タグの中の `name="value"` を順に見れば足りる。
pub fn redact_html(html: &str) -> String {
    static TAG: OnceLock<Option<Regex>> = OnceLock::new();
    static ATTRIBUTE: OnceLock<Option<Regex>> = OnceLock::new();
    let (Some(tag), Some(attribute)) = (
        TAG.get_or_init(|| Regex::new(r"<[A-Za-z][^<>]*>").ok()),
        ATTRIBUTE.get_or_init(|| Regex::new(r#"([^\s"'<>/=]+)="([^"]*)""#).ok()),
    ) else {
        // 正規表現は定数なので失敗しない。万一の時は丸ごと伏せる（漏らす側に倒さない）。
        return REDACTED.to_string();
    };
    tag.replace_all(html, |tag_match: &regex::Captures| {
        let text = &tag_match[0];
        let lower = text.to_ascii_lowercase();
        let is_password = lower.starts_with("<input") && lower.contains(r#"type="password""#);
        attribute
            .replace_all(text, |pair: &regex::Captures| {
                let name = pair[1].to_ascii_lowercase();
                // URL は query を落としてから見る（sanitize と同じ順）。
                let value = if matches!(name.as_str(), "href" | "src" | "action") {
                    strip_query(&pair[2])
                } else {
                    pair[2].to_string()
                };
                if (is_password && name == "value") || secret_name(&name) || looks_secret(&value) {
                    format!(r#"{}="{REDACTED}""#, &pair[1])
                } else {
                    format!(r#"{}="{value}""#, &pair[1])
                }
            })
            .into_owned()
    })
    .into_owned()
}

// ── composer に差し込む文章 ──

/// composer に入れる HTML の抜粋の上限（受け取る上限より短くして、入力欄を埋め尽くさない）。
const PROMPT_HTML_CHARS: usize = 3000;

/// 要素の短い呼び名（チップとホバーに出す）: `<Timer> button#start "開始"`。
pub fn element_label(capture: &ElementCapture) -> String {
    let element = &capture.element;
    let last_part = element
        .path
        .rsplit(" > ")
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(&element.tag);
    let mut label = String::new();
    if let Some(component) = element.components.last() {
        label.push('<');
        label.push_str(component);
        label.push_str("> ");
    }
    label.push_str(last_part);
    let name = element
        .accessible_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&element.text);
    let name = truncate(&collapse(name), 40);
    if !name.is_empty() {
        label.push_str(" \"");
        label.push_str(&name);
        label.push('"');
    }
    label
}

/// 既定値で情報の無いスタイルは載せない（`auto` / `normal` / `none` の類）。
fn meaningful_style(name: &str, value: &str) -> bool {
    let value = value.trim();
    !(value.is_empty()
        || value == "auto"
        || value == "normal"
        || value == "none"
        || (name == "position" && value == "static")
        || (name == "background-color" && value == "rgba(0, 0, 0, 0)")
        || (name == "z-index" && value == "auto")
        || (name == "margin" && value == "0px")
        || (name == "padding" && value == "0px")
        || (name == "border-radius" && value == "0px")
        || (name == "text-align" && value == "start")
        || (name == "border" && value.starts_with("0px ")))
}

/// 要素を composer に差し込む文章にする（Markdown）。見出しと項目名は UI の言語に合わせる。
pub fn format_for_prompt(capture: &ElementCapture) -> String {
    let page = &capture.page;
    let element = &capture.element;
    let mut lines = vec![
        i18n::t!("design.prompt_heading", "label" => element_label(capture)),
        format!("- {}: {}", i18n::t!("design.prompt_page"), page.url),
        format!(
            "- {}: `{}`",
            i18n::t!("design.prompt_selector"),
            element.selector.replace('`', "'")
        ),
    ];
    if !element.path.is_empty() && element.path != element.selector {
        lines.push(format!(
            "- {}: {}",
            i18n::t!("design.prompt_path"),
            element.path
        ));
    }
    if let Some(source) = &element.source {
        let mut location = format!("{}:{}", source.file, source.line);
        if let Some(column) = source.column {
            location.push_str(&format!(":{column}"));
        }
        lines.push(format!(
            "- {}: {location}",
            i18n::t!("design.prompt_source")
        ));
    }
    if !element.components.is_empty() {
        let components: Vec<String> = element
            .components
            .iter()
            .map(|name| format!("<{name}>"))
            .collect();
        lines.push(format!(
            "- {}: {}",
            i18n::t!("design.prompt_components"),
            components.join(" ")
        ));
    }
    let mut accessibility = Vec::new();
    if let Some(role) = element.role.as_deref().filter(|role| !role.is_empty()) {
        accessibility.push(format!("role={role}"));
    }
    if let Some(name) = element
        .accessible_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        accessibility.push(format!("name=\"{}\"", truncate(&collapse(name), 120)));
    }
    for attribute in &element.attributes {
        if attribute.name.starts_with("aria-") && attribute.name != "aria-label" {
            accessibility.push(format!("{}=\"{}\"", attribute.name, attribute.value));
        }
    }
    if !accessibility.is_empty() {
        lines.push(format!(
            "- {}: {}",
            i18n::t!("design.prompt_accessibility"),
            accessibility.join(" ")
        ));
    }
    lines.push(format!(
        "- {}: {}",
        i18n::t!("design.prompt_bounds"),
        i18n::t!(
            "design.prompt_bounds_value",
            "x" => element.rect.x.round(),
            "y" => element.rect.y.round(),
            "width" => element.rect.width.round(),
            "height" => element.rect.height.round(),
            "viewport" => format!(
                "{}×{}",
                page.viewport_width.round(),
                page.viewport_height.round()
            )
        )
    ));
    let styles: Vec<String> = STYLE_PROPERTIES
        .iter()
        .filter_map(|name| {
            let value = element.styles.get(*name)?;
            meaningful_style(name, value).then(|| format!("{name}: {}", value.trim()))
        })
        .collect();
    if !styles.is_empty() {
        lines.push(format!(
            "- {}: {}",
            i18n::t!("design.prompt_styles"),
            styles.join("; ")
        ));
    }
    let text = truncate(&collapse(&element.text), 300);
    if !text.is_empty() {
        lines.push(format!("- {}: \"{text}\"", i18n::t!("design.prompt_text")));
    }
    let nearby: Vec<String> = element
        .nearby_text
        .iter()
        .map(|text| truncate(&collapse(text), 80))
        .filter(|text| !text.is_empty())
        .take(4)
        .map(|text| format!("\"{text}\""))
        .collect();
    if !nearby.is_empty() {
        lines.push(format!(
            "- {}: {}",
            i18n::t!("design.prompt_nearby"),
            nearby.join(" / ")
        ));
    }
    let html = truncate(element.html.trim(), PROMPT_HTML_CHARS);
    if !html.is_empty() {
        // 中身に ``` があっても柵が閉じないよう、中の最長より長い柵にする。
        let fence = "`".repeat(longest_backtick_run(&html).max(2) + 1);
        lines.push(format!("{fence}html\n{html}\n{fence}"));
    }
    lines.join("\n")
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for character in text.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "0123456789abcdef0123456789abcdef";
    /// IPC の送り手（wry が添える、送った文書の URL）。
    const SENDER: &str = "http://localhost:5173/app?token=abc";

    fn capture_json() -> serde_json::Value {
        serde_json::json!({
            "page": {
                "url": "http://localhost:5173/app?token=abc#top",
                "title": "Pomodoro",
                "viewport_width": 1280.0,
                "viewport_height": 800.0,
                "device_pixel_ratio": 2.0
            },
            "element": {
                "tag": "button",
                "selector": "main.card > button#start",
                "path": "main.card > button#start",
                "text": "Start",
                "nearby_text": ["25:00", "集中する時間です。"],
                "html": "<button id=\"start\" class=\"primary\" data-api-key=\"sk-live-123\" data-href=\"/x\">Start</button>",
                "role": null,
                "accessible_name": "タイマーを開始",
                "attributes": [
                    {"name": "id", "value": "start"},
                    {"name": "href", "value": "/next?session_id=42"},
                    {"name": "title", "value": "client_secret=xyz"}
                ],
                "rect": {"x": 312.4, "y": 208.0, "width": 96.0, "height": 40.0},
                "styles": {
                    "display": "inline-block",
                    "position": "static",
                    "color": "rgb(255, 255, 255)",
                    "background-color": "rgb(79, 70, 229)",
                    "border-radius": "8px",
                    "font-size": "15px",
                    "z-index": "auto"
                },
                "components": ["App", "Timer"],
                "source": {"file": "src/Timer.tsx", "line": 42, "column": 7, "via": "debugSource"}
            }
        })
    }

    fn message(kind: &str, nonce: &str) -> String {
        serde_json::json!({
            "type": kind,
            "nonce": nonce,
            "multi": false,
            "capture": capture_json()
        })
        .to_string()
    }

    #[test]
    fn a_pick_with_the_right_nonce_is_accepted_and_cleaned() {
        let accepted = accept_message(Some(NONCE), SENDER, &message("pick", NONCE)).expect("通る");
        let DesignMessage::Pick { multi, capture } = accepted else {
            panic!("pick のはず");
        };
        assert!(!multi);
        assert_eq!(
            capture.page.url, "http://localhost:5173/app",
            "query と fragment を落とす"
        );
        let href = capture
            .element
            .attributes
            .iter()
            .find(|attribute| attribute.name == "href")
            .map(|attribute| attribute.value.clone());
        assert_eq!(
            href.as_deref(),
            Some("/next"),
            "属性の URL も query を落とす"
        );
        let title = capture
            .element
            .attributes
            .iter()
            .find(|attribute| attribute.name == "title")
            .map(|attribute| attribute.value.clone());
        assert_eq!(title.as_deref(), Some(REDACTED), "secret らしい値は伏せる");
        assert!(!capture.element.html.contains("sk-live-123"));
        assert!(capture
            .element
            .html
            .contains(r#"data-api-key="[redacted]""#));
    }

    #[test]
    fn messages_outside_design_or_with_another_nonce_are_dropped() {
        assert_eq!(
            accept_message(None, SENDER, &message("pick", NONCE)),
            Err(Rejected::NotActive),
            "Design 中でなければ捨てる"
        );
        assert_eq!(
            accept_message(
                Some(NONCE),
                SENDER,
                &message("pick", "ffffffffffffffffffffffffffffffff")
            ),
            Err(Rejected::WrongNonce)
        );
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &message("steal", NONCE)),
            Err(Rejected::UnknownType)
        );
        assert_eq!(
            accept_message(Some(NONCE), SENDER, "not json"),
            Err(Rejected::Malformed)
        );
        // 埋め込まれた外部の iframe は合言葉を知らないが、知っていても通さない（送り手で弾く）。
        assert_eq!(
            accept_message(
                Some(NONCE),
                "https://ads.example/frame.html",
                &message("pick", NONCE)
            ),
            Err(Rejected::NotLocalhost)
        );
        let cancel = serde_json::json!({"type": "cancel", "nonce": NONCE}).to_string();
        assert_eq!(
            accept_message(Some(NONCE), "https://ads.example/frame.html", &cancel),
            Err(Rejected::NotLocalhost)
        );
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &cancel),
            Ok(DesignMessage::Cancel)
        );
    }

    #[test]
    fn oversized_or_misshapen_captures_are_dropped() {
        let huge = format!("\"{}\"", "x".repeat(MAX_MESSAGE_BYTES));
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &huge),
            Err(Rejected::TooLarge)
        );

        let mut value = capture_json();
        value["element"]["html"] =
            serde_json::json!("<p>".to_string() + &"a".repeat(MAX_HTML_CHARS));
        let body =
            serde_json::json!({"type": "pick", "nonce": NONCE, "capture": value}).to_string();
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &body),
            Err(Rejected::FieldTooLarge("html"))
        );

        let mut value = capture_json();
        value["element"]["text"] = serde_json::json!("あ".repeat(MAX_TEXT_CHARS + 1));
        let body =
            serde_json::json!({"type": "pick", "nonce": NONCE, "capture": value}).to_string();
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &body),
            Err(Rejected::FieldTooLarge("text"))
        );

        let mut value = capture_json();
        value["element"]["styles"]["cursor"] = serde_json::json!("pointer");
        let body =
            serde_json::json!({"type": "pick", "nonce": NONCE, "capture": value}).to_string();
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &body),
            Err(Rejected::UnknownStyle("cursor".to_string()))
        );

        let mut value = capture_json();
        value["element"]["unexpected"] = serde_json::json!(1);
        let body =
            serde_json::json!({"type": "pick", "nonce": NONCE, "capture": value}).to_string();
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &body),
            Err(Rejected::Malformed),
            "知らない欄は形が違う＝捨てる"
        );

        let mut value = capture_json();
        value["page"]["url"] = serde_json::json!("https://example.com/");
        let body =
            serde_json::json!({"type": "pick", "nonce": NONCE, "capture": value}).to_string();
        assert_eq!(
            accept_message(Some(NONCE), SENDER, &body),
            Err(Rejected::NotLocalhost)
        );
    }

    #[test]
    fn secrets_in_the_html_excerpt_are_redacted() {
        let html = r#"<form action="/login?next=/admin"><input type="password" name="pw" value="hunter2"><input name="q" value="ok"><a href="/a?session_id=1#x" data-token="bearer abc">a</a></form>"#;
        let redacted = redact_html(html);
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(redacted.contains(r#"value="[redacted]""#));
        assert!(redacted.contains(r#"value="ok""#), "普通の値は残す");
        assert!(
            redacted.contains(r#"type="password""#),
            "入力の種類は伏せない（語そのものは secret ではない）: {redacted}"
        );
        assert!(redacted.contains(r#"action="/login""#), "{redacted}");
        assert!(redacted.contains(r#"href="/a""#), "{redacted}");
        assert!(!redacted.contains("bearer abc"), "{redacted}");
        assert!(redacted.ends_with("</form>"), "中身と閉じタグは変えない");
    }

    #[test]
    fn only_secret_shaped_values_are_redacted() {
        for secret in [
            "api_key=sk-live-123",
            "client_secret: abc",
            "Bearer eyJhbGciOiJIUzI1NiJ9",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig",
            "https://s3.amazonaws.com/x?X-Amz-Signature=abc",
            "session_id=42",
        ] {
            assert!(looks_secret(secret), "{secret}");
        }
        for plain in [
            "password",
            "password-field",
            "current-password",
            "Sign in",
            "token-list",
        ] {
            assert!(!looks_secret(plain), "{plain}");
        }
        assert!(secret_name("data-api-key"));
        assert!(secret_name("data-csrf"));
        assert!(!secret_name("type"));
        assert!(!secret_name("aria-label"));
    }

    #[test]
    fn query_and_fragment_are_stripped() {
        assert_eq!(
            strip_query("http://localhost:3000/a/b?x=1&token=2#frag"),
            "http://localhost:3000/a/b"
        );
        assert_eq!(strip_query("/rel/path#x"), "/rel/path");
        assert_eq!(strip_query("plain"), "plain");
    }

    #[test]
    fn the_prompt_names_the_element_and_its_source() {
        let DesignMessage::Pick { capture, .. } =
            accept_message(Some(NONCE), SENDER, &message("pick", NONCE)).expect("通る")
        else {
            panic!("pick のはず");
        };
        let text = format_for_prompt(&capture);
        assert!(
            text.contains("<Timer> button#start \"タイマーを開始\""),
            "{text}"
        );
        assert!(text.contains("http://localhost:5173/app"), "{text}");
        assert!(!text.contains("token=abc"), "query は載せない: {text}");
        assert!(text.contains("`main.card > button#start`"), "{text}");
        assert!(text.contains("src/Timer.tsx:42:7"), "{text}");
        assert!(text.contains("<App> <Timer>"), "{text}");
        assert!(
            text.contains("background-color: rgb(79, 70, 229)"),
            "{text}"
        );
        assert!(
            !text.contains("position: static"),
            "既定値は載せない: {text}"
        );
        assert!(!text.contains("z-index"), "{text}");
        assert!(text.contains("x=312 y=208 96×40"), "{text}");
        assert!(text.contains("```html\n<button"), "{text}");
        assert!(!text.contains("sk-live-123"), "{text}");
    }

    #[test]
    fn html_with_fences_gets_a_longer_fence() {
        let mut capture: ElementCapture = serde_json::from_value(capture_json()).expect("形どおり");
        capture.element.html = "<pre>```rust\nfn main() {}\n```</pre>".to_string();
        let text = format_for_prompt(&capture);
        assert!(text.contains("````html\n<pre>"), "{text}");
    }

    #[test]
    fn nonces_are_hex_and_differ() {
        let first = new_nonce();
        let second = new_nonce();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(first, second);
        assert!(start_script(&first).contains(&format!("\"{first}\"")));
    }

    #[test]
    fn the_picker_script_has_no_debug_marker_left_in_debug_builds() {
        let script = picker_script();
        assert!(script.contains("__necoderDesign"));
        #[cfg(debug_assertions)]
        assert!(script.contains("__necoderDesignDebug"));
    }
}
