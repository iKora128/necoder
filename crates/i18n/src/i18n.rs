//! i18n — Shirushi の翻訳土台。
//!
//! ARCHITECTURE §6 / UI-SPEC §10 の規律を実装する: **UI 文字列は全て [`t!`] 経由**、
//! キーは `領域.キー`、`locales/ja.yml` と `en.yml` を同梱、OS ロケールで自動選択。
//!
//! 実装ノート: rust-i18n はマクロが `crate::` スコープに閉じており、レイヤ化した多 crate
//! 構成（ui / editor_view / workspace / agent_panel が各々 `t!` を使う）に噛み合わない。
//! そこで「`t!` 境界を守ればライブラリは差し替え可能」（ARCHITECTURE §6）に従い、
//! ワークスペース内から `i18n::t!` で一様に呼べる薄い実装にした。高度な複数形などが要れば
//! この crate 内で fluent-rs 等へ差し替える（呼び出し側は不変）。

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// 現在ロケールに無いキーはここへフォールバックする。
pub const FALLBACK_LOCALE: &str = "en";

/// 同梱ロケール（`領域.キー` → 文字列 に平坦化済み）。YAML はコンパイル時に埋め込む。
static LOCALES: LazyLock<HashMap<&'static str, HashMap<String, String>>> = LazyLock::new(|| {
    let mut all = HashMap::new();
    all.insert(
        "ja",
        parse_or_empty(include_str!("../../../locales/ja.yml")),
    );
    all.insert(
        "en",
        parse_or_empty(include_str!("../../../locales/en.yml")),
    );
    all
});

/// 実行時に選択中のロケール。既定は [`FALLBACK_LOCALE`]。起動時に [`init_from_os_locale`] で上書き。
static CURRENT_LOCALE: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(FALLBACK_LOCALE.to_string()));

/// キーを現在ロケールで翻訳する。無ければ en、それも無ければキー自身を返す（画面が空にならない）。
pub fn translate(key: &str) -> String {
    let locale = current_locale();
    lookup(&locale, key)
        .or_else(|| lookup(FALLBACK_LOCALE, key))
        .unwrap_or_else(|| key.to_string())
}

/// 現在ロケールを設定する（settings やテストから）。
pub fn set_locale(locale: &str) {
    if let Ok(mut guard) = CURRENT_LOCALE.write() {
        *guard = locale.to_string();
    }
}

/// 現在ロケールのコード（例 `"ja"`）。
pub fn locale() -> String {
    current_locale()
}

/// 同梱している全ロケールのコード（ソート済み）。
pub fn available_locales() -> Vec<&'static str> {
    let mut locales: Vec<&'static str> = LOCALES.keys().copied().collect();
    locales.sort_unstable();
    locales
}

/// OS のロケール環境変数から現在ロケールを決める（`LC_ALL` → `LC_MESSAGES` → `LANG`）。
/// 同梱していないロケールなら [`FALLBACK_LOCALE`]。起動時に一度呼ぶ。
pub fn init_from_os_locale() {
    set_locale(detect_os_locale());
}

fn detect_os_locale() -> &'static str {
    let available = available_locales();
    for variable in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(variable) {
            // 例: "ja_JP.UTF-8" → "ja"
            let code = value.split(['_', '.', '-']).next().unwrap_or("");
            if let Some(found) = available.iter().find(|locale| **locale == code) {
                return found;
            }
        }
    }
    FALLBACK_LOCALE
}

fn current_locale() -> String {
    CURRENT_LOCALE
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| FALLBACK_LOCALE.to_string())
}

fn lookup(locale: &str, key: &str) -> Option<String> {
    LOCALES.get(locale).and_then(|map| map.get(key)).cloned()
}

fn parse_or_empty(text: &str) -> HashMap<String, String> {
    match parse_locale(text) {
        Ok(map) => map,
        Err(error) => {
            // 埋め込み YAML の破損は parity/parse テストで検出する。実行時は空マップに退避し、
            // t! はキー名を返す（画面は壊れるが落ちない）。黙って握り潰さず標準エラーに残す。
            eprintln!("i18n: ロケール YAML の解析に失敗（キー名で代替）: {error}");
            HashMap::new()
        }
    }
}

fn parse_locale(text: &str) -> Result<HashMap<String, String>, serde_yaml::Error> {
    let root: serde_yaml::Value = serde_yaml::from_str(text)?;
    let mut flattened = HashMap::new();
    flatten("", &root, &mut flattened);
    Ok(flattened)
}

/// ネストした YAML マップを `領域.キー` のドット結合に平坦化する。葉はスカラのみ扱う。
fn flatten(prefix: &str, value: &serde_yaml::Value, out: &mut HashMap<String, String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (raw_key, child) in map {
                let Some(segment) = raw_key.as_str() else {
                    continue;
                };
                let key = if prefix.is_empty() {
                    segment.to_string()
                } else {
                    format!("{prefix}.{segment}")
                };
                flatten(&key, child, out);
            }
        }
        serde_yaml::Value::String(text) => {
            out.insert(prefix.to_string(), text.clone());
        }
        serde_yaml::Value::Number(number) => {
            out.insert(prefix.to_string(), number.to_string());
        }
        serde_yaml::Value::Bool(flag) => {
            out.insert(prefix.to_string(), flag.to_string());
        }
        _ => {}
    }
}

/// UI 文字列を引く。`t!("領域.キー")` か、`%{名前}` を差し替える `t!("領域.キー", "名前" => 値)`。
///
/// ```
/// i18n::set_locale("en");
/// assert_eq!(i18n::t!("app.name"), "Shirushi");
/// ```
#[macro_export]
macro_rules! t {
    ($key:expr $(,)?) => {
        $crate::translate($key)
    };
    ($key:expr, $($name:literal => $value:expr),+ $(,)?) => {{
        let mut translated = $crate::translate($key);
        $(
            translated = translated.replace(
                ::std::concat!("%{", $name, "}"),
                &::std::string::ToString::to_string(&$value),
            );
        )+
        translated
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_locales_parse() {
        assert!(parse_locale(include_str!("../../../locales/ja.yml")).is_ok());
        assert!(parse_locale(include_str!("../../../locales/en.yml")).is_ok());
    }

    #[test]
    fn ja_and_en_have_identical_keys() {
        let ja =
            parse_locale(include_str!("../../../locales/ja.yml")).expect("ja.yml が解析できる");
        let en =
            parse_locale(include_str!("../../../locales/en.yml")).expect("en.yml が解析できる");
        let mut ja_keys: Vec<&String> = ja.keys().collect();
        let mut en_keys: Vec<&String> = en.keys().collect();
        ja_keys.sort();
        en_keys.sort();
        assert_eq!(
            ja_keys, en_keys,
            "ja.yml と en.yml のキー集合が不一致（どちらかに書き忘れ）"
        );
    }

    #[test]
    fn nested_keys_flatten_to_dotted() {
        let text = "explorer:\n  context:\n    open: ひらく\n";
        let map = parse_locale(text).expect("解析できる");
        assert_eq!(
            map.get("explorer.context.open").map(String::as_str),
            Some("ひらく")
        );
    }

    #[test]
    fn available_locales_lists_both_sorted() {
        assert_eq!(available_locales(), vec!["en", "ja"]);
    }

    // ロケールはグローバル状態なので、書き換える検証はこの 1 本に集約する（並列テストでの競合回避）。
    #[test]
    fn translate_and_macro_respect_locale_and_fallback() {
        set_locale("ja");
        assert_eq!(translate("app.name"), "しるし");

        set_locale("en");
        assert_eq!(translate("app.name"), "Shirushi");
        assert_eq!(t!("app.name"), "Shirushi");

        // 未知キーはキー自身を返す（画面が空にならない）
        assert_eq!(translate("nope.missing"), "nope.missing");

        // 補間アーム: 未知キー（=キー自身）内の %{a} を差し替え
        assert_eq!(t!("x %{a}", "a" => 1), "x 1");

        // 現在ロケールに無ければ en へフォールバック
        set_locale("zz");
        assert_eq!(translate("app.name"), "Shirushi");

        set_locale(FALLBACK_LOCALE);
    }
}
