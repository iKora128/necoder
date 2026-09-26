//! snapshot — Design Mode で選んだ要素の**切り抜き**（PNG）。
//!
//! - macOS: WKWebView の `takeSnapshotWithConfiguration` に矩形を渡す（WebView が自分の描画を返すので
//!   画面収録の権限は要らない）。返った `NSImage` を PNG にする。
//! - Windows: WebView2 の `CapturePreview` で見えている範囲を PNG で撮り、[`crop_png`] で切り抜く。
//!
//! 座標: ピッカーは要素の矩形をビューポート基準の **CSS px** で返す。ページのズームやビューポート幅の
//! 違いは「WebView の幅（論理 px）÷ ビューポートの CSS 幅」の倍率 1 つで吸収する（[`to_view_rect`]）。
//! 撮れるのは見えている範囲だけなので、そこへ切り詰める。

use crate::design;

/// WebView の座標（論理 px・左上原点）の矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 撮った全体の画素数に対して、切り抜く大きさの下限（1 px 未満は撮れない）。
const MIN_EDGE: f64 = 1.0;

/// ビューポート基準の CSS px の矩形を WebView の座標へ。見えている範囲（`view_width` × `view_height`）に
/// 切り詰め、何も残らなければ `None`。
pub fn to_view_rect(
    rect: &design::Rect,
    viewport_css_width: f64,
    view_width: f64,
    view_height: f64,
) -> Option<ViewRect> {
    if !(viewport_css_width > 0.0 && view_width > 0.0 && view_height > 0.0) {
        return None;
    }
    let scale = view_width / viewport_css_width;
    let left = (rect.x * scale).clamp(0.0, view_width);
    let top = (rect.y * scale).clamp(0.0, view_height);
    let right = ((rect.x + rect.width) * scale).clamp(0.0, view_width);
    let bottom = ((rect.y + rect.height) * scale).clamp(0.0, view_height);
    let (width, height) = (right - left, bottom - top);
    (width >= MIN_EDGE && height >= MIN_EDGE).then_some(ViewRect {
        x: left,
        y: top,
        width,
        height,
    })
}

/// 見えている範囲を撮った PNG（`view_width` 論理 px 分の幅）から `rect` を切り抜いて PNG で返す。
/// 撮った画像の画素数 ÷ `view_width` を倍率にする（Retina / 125% などの表示倍率を問わない）。
pub fn crop_png(png: &[u8], rect: ViewRect, view_width: f64) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    if view_width <= 0.0 {
        return Err("view width is zero".to_string());
    }
    let scale = f64::from(image.width()) / view_width;
    let clamp_to = |value: f64, limit: u32| (value.max(0.0).round() as u32).min(limit);
    let left = clamp_to(rect.x * scale, image.width());
    let top = clamp_to(rect.y * scale, image.height());
    let right = clamp_to((rect.x + rect.width) * scale, image.width());
    let bottom = clamp_to((rect.y + rect.height) * scale, image.height());
    if right <= left || bottom <= top {
        return Err("the element is outside the visible area".to_string());
    }
    let cropped = image.crop_imm(left, top, right - left, bottom - top);
    let mut encoded = Vec::new();
    cropped
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .map_err(|error| error.to_string())?;
    Ok(encoded)
}

/// 切り抜きの結果を 1 回だけ受ける口（主スレッドで呼ばれる）。
pub type SnapshotDone = Box<dyn FnOnce(Result<Vec<u8>, String>)>;

/// macOS: WKWebView に矩形を渡して撮る。撮り終えたら `done` を呼ぶ。
#[cfg(target_os = "macos")]
pub(crate) fn capture(
    webview: &wry::WebView,
    rect: ViewRect,
    _view_width: f64,
    done: SnapshotDone,
) -> Result<(), String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSImage;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::NSError;
    use objc2_web_kit::WKSnapshotConfiguration;
    use wry::WebViewExtMacOS as _;

    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| "snapshot must start on the main thread".to_string())?;
    let web_view = webview.webview();
    let configuration = unsafe { WKSnapshotConfiguration::new(main_thread) };
    unsafe {
        configuration.setRect(CGRect::new(
            CGPoint::new(rect.x, rect.y),
            CGSize::new(rect.width, rect.height),
        ));
        // ピッカーが消した枠が描き直された後を撮る（枠を写さない）。
        configuration.setAfterScreenUpdates(true);
    }
    // ObjC の block は何度でも呼べる形（Fn）なので、1 回だけ渡す口を中に持たせる。
    let pending = std::cell::RefCell::new(Some(done));
    let handler = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(done) = pending.borrow_mut().take() else {
            return;
        };
        let result = match unsafe { image.as_ref() } {
            Some(image) => png_from_image(image),
            None => Err(unsafe { error.as_ref() }
                .map(|error| error.localizedDescription().to_string())
                .unwrap_or_else(|| "snapshot failed".to_string())),
        };
        done(result);
    });
    unsafe {
        web_view.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &handler)
    };
    Ok(())
}

/// `NSImage` → PNG（TIFF 表現を経由してビットマップにし、PNG で書き出す）。
#[cfg(target_os = "macos")]
fn png_from_image(image: &objc2_app_kit::NSImage) -> Result<Vec<u8>, String> {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::NSDictionary;

    let tiff = image
        .TIFFRepresentation()
        .ok_or_else(|| "the snapshot has no bitmap".to_string())?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff)
        .ok_or_else(|| "the snapshot bitmap is unreadable".to_string())?;
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or_else(|| "the snapshot cannot be encoded as PNG".to_string())?;
    Ok(png.to_vec())
}

/// Windows: WebView2 の `CapturePreview` で見えている範囲を PNG で撮り、矩形で切り抜く。
#[cfg(target_os = "windows")]
pub(crate) fn capture(
    webview: &wry::WebView,
    rect: ViewRect,
    view_width: f64,
    done: SnapshotDone,
) -> Result<(), String> {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::System::Com::IStream;
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use wry::WebViewExtWindows as _;

    let stream: IStream = unsafe { SHCreateMemStream(None) }
        .ok_or_else(|| "cannot create a memory stream".to_string())?;
    let reader = stream.clone();
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let outcome = result
            .map_err(|error| error.to_string())
            .and_then(|()| read_stream(&reader))
            .and_then(|png| crop_png(&png, rect, view_width));
        done(outcome);
        Ok(())
    }));
    unsafe {
        webview.webview().CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
    }
    .map_err(|error| error.to_string())
}

/// メモリ上の IStream を頭から全部読む。
#[cfg(target_os = "windows")]
fn read_stream(stream: &windows::Win32::System::Com::IStream) -> Result<Vec<u8>, String> {
    use windows::Win32::System::Com::STREAM_SEEK_SET;

    unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        let status = unsafe {
            stream.Read(
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                Some(&mut read as *mut u32),
            )
        };
        if status.is_err() {
            return Err(windows_core::Error::from(status).to_string());
        }
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read as usize]);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_rects_scale_with_zoom_and_clip_to_the_visible_area() {
        let rect = design::Rect {
            x: 100.0,
            y: 50.0,
            width: 200.0,
            height: 40.0,
        };
        // 100% ズーム: CSS 幅 = WebView 幅。
        assert_eq!(
            to_view_rect(&rect, 800.0, 800.0, 600.0),
            Some(ViewRect {
                x: 100.0,
                y: 50.0,
                width: 200.0,
                height: 40.0
            })
        );
        // 200% ズーム: CSS 400 px が WebView 800 px に広がる。
        assert_eq!(
            to_view_rect(&rect, 400.0, 800.0, 600.0),
            Some(ViewRect {
                x: 200.0,
                y: 100.0,
                width: 400.0,
                height: 80.0
            })
        );
        // はみ出した分は見えている範囲へ切り詰める。
        let wide = design::Rect {
            x: 700.0,
            y: 580.0,
            width: 300.0,
            height: 100.0,
        };
        assert_eq!(
            to_view_rect(&wide, 800.0, 800.0, 600.0),
            Some(ViewRect {
                x: 700.0,
                y: 580.0,
                width: 100.0,
                height: 20.0
            })
        );
        // 画面の外・大きさ 0 は撮らない。
        let outside = design::Rect {
            x: 900.0,
            y: 10.0,
            width: 50.0,
            height: 50.0,
        };
        assert_eq!(to_view_rect(&outside, 800.0, 800.0, 600.0), None);
        assert_eq!(to_view_rect(&rect, 0.0, 800.0, 600.0), None);
    }

    #[test]
    fn crop_uses_the_captured_pixel_density() {
        // 論理 100 × 50 の WebView を 2 倍で撮った 200 × 100 の画像。左上 (40,20) から 20 × 10 だけ白い。
        let mut canvas = image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 0, 255]));
        for y in 20..30 {
            for x in 40..60 {
                canvas.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(canvas)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("PNG にできる");
        let rect = ViewRect {
            x: 20.0,
            y: 10.0,
            width: 10.0,
            height: 5.0,
        };
        let cropped = crop_png(&png, rect, 100.0).expect("切り抜ける");
        let decoded = image::load_from_memory(&cropped).expect("PNG として読める");
        assert_eq!((decoded.width(), decoded.height()), (20, 10));
        let rgba = decoded.to_rgba8();
        assert!(rgba.pixels().all(|pixel| pixel.0 == [255, 255, 255, 255]));
        // 画像の外だけを指す矩形は撮れない。
        let outside = ViewRect {
            x: 150.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        assert!(crop_png(&png, outside, 100.0).is_err());
    }
}
