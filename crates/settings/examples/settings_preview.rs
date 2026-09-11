//! 設定ホームを実 GPUI で 1 枚に描いて PNG に落とす開発用サンプル（UI 変更の目視検証用）。
//!
//! `remote_preview.rs` が「スマホ連携」セクションだけを隔離して撮るのに対し、こちらは**画面全体**を
//! 撮る。セクションを足した時（MCP サーバ等）にレイアウト崩れ・色・文字を目で確かめるため。
//! **本物のユーザー設定は読まない**（`NECODER_SETTINGS_PREVIEW_JSON` に撮影用の settings.json を
//! 渡す。未指定なら組み込み既定＝初回オンボーディング画面）。MCP サーバの一覧だけは
//! **このマシンの実際の発見結果**が出る（`~/.codex/config.toml` 等を読むため）。
//!
//! 既知の限界: この offscreen 経路では**文字がラスタライズされない**（枠・色・配置だけが写る）。
//! 文言の確認は実アプリの目視が要る。
//!
//! ```sh
//! NECODER_SETTINGS_PREVIEW_PNG=/tmp/settings.png \
//!   NECODER_SETTINGS_PREVIEW_JSON=/tmp/preview-settings.json \
//!   cargo run -p settings --features remote-preview,gpui_platform/runtime_shaders \
//!     --example settings_preview
//! ```
use gpui::{prelude::*, *};

fn main() {
    let output =
        std::env::var("NECODER_SETTINGS_PREVIEW_PNG").expect("出力 PNG のパス（環境変数）");
    // 画面は縦に長いので、既定でも主要セクションが 1 枚に収まる高さで開く。
    let height: f32 = std::env::var("NECODER_SETTINGS_PREVIEW_HEIGHT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1400.);
    gpui_platform::application().run(move |cx| {
        cx.text_system()
            .add_fonts(vec![
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/IBMPlexSansJP-Regular.ttf"
                )),
                std::borrow::Cow::Borrowed(include_bytes!(
                    "../../../assets/fonts/IBMPlexSansJP-SemiBold.ttf"
                )),
            ])
            .expect("preview fonts");
        i18n::set_locale(&std::env::var("NECODER_LOCALE").unwrap_or_else(|_| "ja".to_string()));
        let user_path = std::env::var("NECODER_SETTINGS_PREVIEW_JSON")
            .ok()
            .map(std::path::PathBuf::from);
        settings::init(user_path, None, cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(760.), px(height)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        settings::SettingsView::new(
                            theme_core::Theme::dark(),
                            rgb(0x61afef).into(),
                            cx,
                        )
                    })
                },
            )
            .expect("preview window");
        cx.activate(true);
        cx.spawn(async move |cx| {
            // 起動直後は「確認中…」なので、CLI 探索と MCP の読み込みが落ち着くのを待ってから撮る。
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            cx.update(|cx| {
                handle
                    .update(cx, |_, window, _| {
                        window
                            .render_to_image()
                            .expect("render")
                            .save(&output)
                            .expect("save")
                    })
                    .expect("window");
                cx.quit();
            });
        })
        .detach();
    });
}
