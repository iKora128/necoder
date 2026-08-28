//! image_view — 画像ファイルのタブ表示（FEATURES §2「Markdown/画像プレビュー」の画像側）。
//!
//! Pane/Item 多態化（ARCHITECTURE §3）の第2の具体型。バイト列を `gpui::Image` として保持し
//! `img()` で描く（デコードは gpui の asset 系が非同期に行う）ため、local / remote どちらの
//! Host から読んだファイルでも同じ経路で表示できる。編集・保存・LSP は一切関与しない。
//!
//! 色は識別に集約（UI-SPEC）: 背景は bg1・キャプションは fg2 のみで、装飾は足さない。

use crate::workspace::*;
use gpui::{img, ObjectFit};

/// 画像としてタブ表示する拡張子か（gpui がデコードできる形式 = image crate + resvg）。
pub(crate) fn is_image_path(path: &Path) -> bool {
    image_format_for(path).is_some()
}

/// 拡張子 → gpui の [`gpui::ImageFormat`]。非対応拡張子は None（テキストとして開く既存経路へ）。
fn image_format_for(path: &Path) -> Option<gpui::ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => gpui::ImageFormat::Png,
        "jpg" | "jpeg" => gpui::ImageFormat::Jpeg,
        "gif" => gpui::ImageFormat::Gif,
        "webp" => gpui::ImageFormat::Webp,
        "bmp" => gpui::ImageFormat::Bmp,
        "ico" => gpui::ImageFormat::Ico,
        "tif" | "tiff" => gpui::ImageFormat::Tiff,
        "svg" => gpui::ImageFormat::Svg,
        _ => return None,
    })
}

/// バイト数の人間向け表記（キャプション用）。
fn human_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub(crate) struct ImageView {
    image: Arc<gpui::Image>,
    /// ヘッダだけ読んだ寸法（svg・読めない場合は None → キャプションはサイズのみ）。
    dimensions: Option<(u32, u32)>,
    byte_len: usize,
    theme: Theme,
    focus_handle: FocusHandle,
}

impl ImageView {
    pub(crate) fn new(path: &Path, bytes: Vec<u8>, theme: Theme, cx: &mut Context<Self>) -> Self {
        // is_image_path で入ってくるので必ず Some だが、防御的に Png を既定にする。
        let format = image_format_for(path).unwrap_or(gpui::ImageFormat::Png);
        let (dimensions, byte_len) = Self::probe(format, &bytes);
        Self {
            image: Arc::new(gpui::Image::from_bytes(format, bytes)),
            dimensions,
            byte_len,
            theme,
            focus_handle: cx.focus_handle(),
        }
    }

    /// 寸法をヘッダから読む（フルデコードしない）。svg はラスタ寸法を持たないので None。
    fn probe(format: gpui::ImageFormat, bytes: &[u8]) -> (Option<(u32, u32)>, usize) {
        let dimensions = if format == gpui::ImageFormat::Svg {
            None
        } else {
            image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()
                .and_then(|reader| reader.into_dimensions().ok())
        };
        (dimensions, bytes.len())
    }

    /// ディスク上の内容が変わった（watcher 経由）。バイト列を差し替えると gpui::Image の id
    /// （= バイト列の hash）が変わり、asset キャッシュが新テクスチャとして読み直す。
    pub(crate) fn set_bytes(&mut self, path: &Path, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let format = image_format_for(path).unwrap_or(gpui::ImageFormat::Png);
        let (dimensions, byte_len) = Self::probe(format, &bytes);
        self.image = Arc::new(gpui::Image::from_bytes(format, bytes));
        self.dimensions = dimensions;
        self.byte_len = byte_len;
        cx.notify();
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }
}

impl Focusable for ImageView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let caption: SharedString = match self.dimensions {
            Some((width, height)) => {
                format!("{width} × {height} · {}", human_size(self.byte_len)).into()
            }
            None => human_size(self.byte_len).into(),
        };
        let fallback_color = theme.fg2;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(16.))
                    .child(
                        // サイズ未指定 = 実寸（aspect 維持）を max でペインに収める。拡大はしない。
                        img(self.image.clone())
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::ScaleDown)
                            .with_fallback(move || {
                                div()
                                    .text_color(fallback_color)
                                    .text_size(px(12.))
                                    .child(i18n::t!("image.load_failed"))
                                    .into_any_element()
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .pb(px(8.))
                    .text_size(px(11.))
                    .text_color(theme.fg2)
                    .child(caption),
            )
    }
}
