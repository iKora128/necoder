//! 書体の設定（O27）— UI の文字と、コードの文字（エディタ・差分・パス・ターミナルの既定）。
//!
//! 使う側は名前を直書きせず [`ui_font`] / [`code_font`] で引く。workspace が settings.json
//! （`ui_font_family` / `code_font_family`）から [`FontFamilies`] を作って global に置き、変わったら
//! 置き直す。global が無ければ同梱の既定（IBM Plex Sans JP / Guguru Sans Code）。
//! 入っていない書体を書いた時は、GPUI が OS の代わりの書体で描く（崩れはしない）。

use gpui::{App, Global, SharedString};

/// UI の書体の既定（同梱・SIL OFL）。
pub const DEFAULT_UI_FONT: &str = "IBM Plex Sans JP";
/// コードの書体の既定（同梱・Google Sans Code + IBM Plex Sans JP の等幅・SIL OFL）。
pub const DEFAULT_CODE_FONT: &str = "Guguru Sans Code";

/// 書体の設定（workspace が設定から作る global）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFamilies {
    pub ui: SharedString,
    pub code: SharedString,
}

impl Global for FontFamilies {}

impl Default for FontFamilies {
    fn default() -> Self {
        Self {
            ui: SharedString::new_static(DEFAULT_UI_FONT),
            code: SharedString::new_static(DEFAULT_CODE_FONT),
        }
    }
}

impl FontFamilies {
    /// 設定の値から作る（空白だけ・空は既定）。
    pub fn new(ui: &str, code: &str) -> Self {
        let pick = |value: &str, fallback: &'static str| match value.trim() {
            "" => SharedString::new_static(fallback),
            family => SharedString::from(family.to_string()),
        };
        Self {
            ui: pick(ui, DEFAULT_UI_FONT),
            code: pick(code, DEFAULT_CODE_FONT),
        }
    }
}

/// UI の書体。
pub fn ui_font(cx: &App) -> SharedString {
    cx.try_global::<FontFamilies>()
        .map(|fonts| fonts.ui.clone())
        .unwrap_or_else(|| SharedString::new_static(DEFAULT_UI_FONT))
}

/// コードの書体（等幅）。
pub fn code_font(cx: &App) -> SharedString {
    cx.try_global::<FontFamilies>()
        .map(|fonts| fonts.code.clone())
        .unwrap_or_else(|| SharedString::new_static(DEFAULT_CODE_FONT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_keep_the_bundled_fonts() {
        assert_eq!(FontFamilies::new("", "  "), FontFamilies::default());
        let chosen = FontFamilies::new(" Inter ", "JetBrains Mono");
        assert_eq!(chosen.ui.as_ref(), "Inter");
        assert_eq!(chosen.code.as_ref(), "JetBrains Mono");
    }
}
