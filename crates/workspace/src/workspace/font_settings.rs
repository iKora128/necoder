//! 書体の設定（O27）を settings.json から `ui::FontFamilies` の global へ渡す。
//!
//! 使う側（エディタ・Agent パネル・差分・ターミナルの検索欄など）は `ui::ui_font` / `ui::code_font` で
//! 引くので、ここで global を置き直せば次の描画から効く。ターミナルの本文は `terminal_settings` が
//! 同じ設定（`code_font_family`）から見た目を作る。

use crate::workspace::*;

/// 設定から書体を作る（空は同梱の既定）。
pub(crate) fn font_families_from(settings: &settings::Settings) -> ui::FontFamilies {
    ui::FontFamilies::new(&settings.ui_font_family, &settings.code_font_family)
}

/// 起動時に 1 回だけ繋ぐ（main から呼ぶ）: 今の設定を置き、設定が変わるたびに置き直す。
pub fn install_font_settings(cx: &mut App) {
    refresh_font_settings(cx);
    cx.observe_global::<settings::SettingsGlobal>(refresh_font_settings)
        .detach();
}

/// 設定から作り直す。前と同じなら置かない（ほかの設定の変更で全部を描き直させない）。
fn refresh_font_settings(cx: &mut App) {
    let Some(next) = cx
        .try_global::<settings::SettingsGlobal>()
        .map(|global| font_families_from(global.settings()))
    else {
        return;
    };
    if cx.try_global::<ui::FontFamilies>() == Some(&next) {
        return;
    }
    cx.set_global(next);
    // 書体は描画のたびに読むので、開いている窓を描き直させる。
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_become_the_font_families() {
        assert_eq!(
            font_families_from(&settings::Settings::default()),
            ui::FontFamilies::default(),
            "既定は同梱の書体"
        );
        let settings = settings::Settings {
            ui_font_family: "Inter".to_string(),
            code_font_family: "JetBrains Mono".to_string(),
            ..settings::Settings::default()
        };
        let fonts = font_families_from(&settings);
        assert_eq!(fonts.ui.as_ref(), "Inter");
        assert_eq!(fonts.code.as_ref(), "JetBrains Mono");
        // ターミナルは書体を指定しなければコードの書体に揃う。
        assert_eq!(
            super::super::terminal_settings::terminal_appearance_from(&settings)
                .font_family
                .as_ref(),
            "JetBrains Mono"
        );
    }
}
