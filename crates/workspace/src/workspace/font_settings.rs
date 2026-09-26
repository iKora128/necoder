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
/// 行の詰め具合（`density`・O27）も同じ時に置く（どちらも描画のたびに読む見た目の設定）。
fn refresh_font_settings(cx: &mut App) {
    let Some((next, density)) = cx.try_global::<settings::SettingsGlobal>().map(|global| {
        let settings = global.settings();
        let density = match settings.density {
            settings::Density::Compact => ui::RowDensity::Compact,
            settings::Density::Cozy => ui::RowDensity::Cozy,
        };
        (font_families_from(settings), density)
    }) else {
        return;
    };
    let fonts_changed = cx.try_global::<ui::FontFamilies>() != Some(&next);
    let density_changed = cx.try_global::<ui::RowDensity>() != Some(&density);
    if !fonts_changed && !density_changed {
        return;
    }
    cx.set_global(next);
    cx.set_global(density);
    // 書体と行の高さは描画のたびに読むので、開いている窓を描き直させる。
    cx.refresh_windows();
}

impl Workspace {
    /// 書体のピッカーを開く（O27）。先頭 = 既定に戻す、続けて入っている書体（OS の一覧・名前順）。
    /// 選んだ書体は `key`（`ui_font_family` / `code_font_family` / `terminal_font_family`）へ書く。
    pub(crate) fn open_font_picker(
        &mut self,
        key: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fonts: Vec<String> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .collect();
        let current = {
            let settings = settings::get(cx);
            match key {
                "ui_font_family" => settings.ui_font_family.clone(),
                "code_font_family" => settings.code_font_family.clone(),
                "terminal_font_family" => settings.terminal_font_family.clone(),
                _ => String::new(),
            }
        };
        let mut items = vec![PickerItem::new(0, i18n::t!("settings.font_picker_default"))];
        items.extend(fonts.iter().enumerate().map(|(index, name)| {
            let item = PickerItem::new(index + 1, name.clone());
            if *name == current {
                item.with_accent(self.accent())
            } else {
                item
            }
        }));
        self.overlays.picker_fonts = fonts;
        self.overlays.picker_font_key = key;
        self.open_picker(
            PickerMode::Fonts,
            i18n::t!("settings.font_picker_placeholder"),
            items,
            window,
            cx,
        );
    }

    /// ピッカーで選んだ書体を設定へ書く（id 0 = 空 = 既定）。書けなければ知らせる。
    pub(crate) fn commit_font(&mut self, id: usize, cx: &mut Context<Self>) {
        let key = self.overlays.picker_font_key;
        if key.is_empty() {
            return;
        }
        let value = match id {
            0 => String::new(),
            index => match self.overlays.picker_fonts.get(index - 1) {
                Some(name) => name.clone(),
                None => return,
            },
        };
        let result = settings::set_user_value(cx, key, serde_json::Value::String(value));
        self.report_settings_save(result, cx);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// O27: 行の詰め具合は設定に付いてくる（cozy = 4px 高く・compact に戻せる）。
    #[gpui::test]
    fn row_density_follows_the_setting(cx: &mut gpui::TestAppContext) {
        let path =
            std::env::temp_dir().join(format!("necoder_density_{}.json", std::process::id()));
        std::fs::write(&path, r#"{"onboarded":true,"density":"cozy"}"#).unwrap();
        cx.update(|cx| {
            settings::init(Some(path.clone()), None, cx);
            install_font_settings(cx);
            assert_eq!(ui::row_height(cx, 23.), px(27.));
            assert_eq!(ui::row_padding(cx, 4.), px(6.));
        });
        // 監視は張った effect の終わりから効く（アプリは起動時に張る）。
        cx.update(|cx| {
            settings::set_user_value(cx, "density", serde_json::json!("compact")).unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(ui::row_height(cx, 23.), px(23.)));
        let _ = std::fs::remove_file(&path);
    }

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

    #[gpui::test]
    fn the_font_picker_writes_the_chosen_font(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("necoder_font_picker_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![root.clone()], Theme::dark(), None, cx)
        });
        // 設定の「選ぶ…」と同じ入口（窓の要る後処理で開く）。
        workspace.update_in(cx, |workspace, _window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.chrome.pending_font_picker = Some("code_font_family");
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.update_in(cx, |workspace, _window, cx| {
            assert!(workspace.overlays.picker.is_some(), "ピッカーが開く");
            assert!(workspace.overlays.picker_mode == PickerMode::Fonts);
            assert_eq!(workspace.overlays.picker_font_key, "code_font_family");
            // 一覧は OS 次第なので、選ぶ書体はここで決める。
            workspace.overlays.picker_fonts = vec!["Test Mono".to_string()];
            workspace.commit_font(1, cx);
            assert_eq!(settings::get(cx).code_font_family, "Test Mono");
            workspace.commit_font(0, cx);
            assert_eq!(settings::get(cx).code_font_family, "", "先頭 = 既定に戻す");
        });
        let _ = std::fs::remove_dir_all(&root);
    }
}
