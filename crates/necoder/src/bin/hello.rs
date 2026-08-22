//! 切り分け用の最小テキストウィンドウ（necoder と同じビルドの gpui を使う）。
//! `cargo run -p necoder --bin hello` で「Hello world! 日本語も」が出れば gpui の文字描画は生きている。

use gpui::{div, prelude::*, px, rgb, size, App, Bounds, WindowBounds, WindowOptions};
use gpui_platform::application;

struct Hello;

impl gpui::Render for Hello {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .bg(rgb(0x2e7d32))
            .justify_center()
            .items_center()
            .text_color(rgb(0xffffff))
            .text_xl()
            .child("Hello world! 日本語も")
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.0), px(300.0)), cx);
        let _ = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Hello),
        );
        cx.activate(true);
    });
}
