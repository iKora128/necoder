//! `NECODER_QR_PREVIEW_JSON` のダミー QR を実 GPUI で描画する開発用サンプル。
use gpui::{prelude::*, *};

fn main() {
    let payload =
        std::fs::read(std::env::var("NECODER_QR_PREVIEW_JSON").expect("dummy QR payload path"))
            .expect("read dummy QR");
    let output = std::env::var("NECODER_QR_PREVIEW_PNG").expect("output PNG path");
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
        i18n::set_locale("ja");
        settings::init(None, None, cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(700.), px(760.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut view = settings::SettingsView::new(
                            theme_core::Theme::dark(),
                            rgb(0x61afef).into(),
                            cx,
                        );
                        view.remote_preview(&payload);
                        view
                    })
                },
            )
            .expect("preview window");
        cx.activate(true);
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
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
