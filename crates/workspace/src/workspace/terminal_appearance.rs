//! ターミナルの見た目（O25）を settings.json から terminal_view へ渡す。
//!
//! terminal_view は設定の crate を知らない（依存の向き）。ここで設定を
//! [`terminal_view::TerminalAppearance`] に写して global に置き、設定が変わったら置き直す。
//! 開いている端末は global を見張っていて、その場で描き直す。

use crate::workspace::*;

/// 設定からターミナルの見た目を作る（範囲の外の値は terminal_view が丸める）。
pub(crate) fn terminal_appearance_from(
    settings: &settings::Settings,
) -> terminal_view::TerminalAppearance {
    terminal_view::TerminalAppearance::new(
        settings.terminal_font_size,
        &settings.terminal_font_family,
        usize::try_from(settings.terminal_scrollback).unwrap_or(usize::MAX),
        &settings.terminal_cursor,
    )
}

/// 起動時に 1 回だけ繋ぐ（main から呼ぶ）: 今の設定を置き、設定が変わるたびに置き直す。
pub fn install_terminal_appearance(cx: &mut App) {
    refresh_terminal_appearance(cx);
    cx.observe_global::<settings::SettingsGlobal>(refresh_terminal_appearance)
        .detach();
}

/// 設定から作り直す。前と同じなら置かない（ほかの設定の変更で端末を起こさない）。
fn refresh_terminal_appearance(cx: &mut App) {
    let Some(next) = cx
        .try_global::<settings::SettingsGlobal>()
        .map(|global| terminal_appearance_from(global.settings()))
    else {
        return;
    };
    if cx.try_global::<terminal_view::TerminalAppearance>() == Some(&next) {
        return;
    }
    cx.set_global(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_become_the_terminal_appearance() {
        let settings = settings::Settings {
            terminal_font_size: 15.0,
            terminal_font_family: "Menlo".to_string(),
            terminal_scrollback: 2_000,
            terminal_cursor: "underline".to_string(),
            ..settings::Settings::default()
        };
        let appearance = terminal_appearance_from(&settings);
        assert_eq!(appearance.font_size, 15.0);
        assert_eq!(appearance.font_family.as_ref(), "Menlo");
        assert_eq!(appearance.scrollback, 2_000);
        assert_eq!(appearance.cursor, terminal_view::TerminalCursor::Underline);
        assert_eq!(
            terminal_appearance_from(&settings::Settings::default()),
            terminal_view::TerminalAppearance::default(),
            "既定の設定は設定を持つ前の見た目"
        );
    }
}
