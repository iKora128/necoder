//! OS 標準 WebView を GPUI の子ビューとして載せる薄い境界。
//!
//! アプリ UI やエディタは従来どおり GPUI で描画し、HTML プレビューを表示している間だけ
//! macOS の WKWebView / Windows の WebView2 を遅延生成する。Chromium 本体は同梱しない。

use gpui::{canvas, div, prelude::*, px, App, Bounds, Context, IntoElement, Pixels, Task, Window};
use std::path::{Path, PathBuf};
use std::time::Duration;
use theme_core::Theme;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    Rect, WebView, WebViewBuilder,
};

/// このビルドがネイティブ WebView を提供できるか。
pub const fn is_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// ローカル HTML ファイルを OS 標準 WebView で表示する GPUI view。
pub struct WebViewView {
    path: PathBuf,
    url: Option<String>,
    theme: Theme,
    active: bool,
    focus_when_ready: bool,
    error: Option<String>,
    /// 非表示のまま放置された WebView を自動破棄するまでの時間（`None` = 破棄しない・設定
    /// `html_preview_evict_minutes` 由来）。表示中は数十〜数百 MB を別プロセスで握るため、
    /// idle メモリ予算を守る回収弁。破棄後の再表示は初回表示と同じ遅延生成経路で復元する。
    evict_after: Option<Duration>,
    /// 非表示化で仕掛ける破棄タイマー。再表示（set_active(true)）で drop ＝キャンセル。
    _evict_task: Option<Task<()>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview: Option<WebView>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    last_bounds: Option<Bounds<Pixels>>,
}

impl WebViewView {
    pub fn local_file(path: impl Into<PathBuf>, theme: Theme) -> Self {
        let path = path.into();
        let (url, error) = match file_url(&path) {
            Ok(url) => (Some(url), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            path,
            url,
            theme,
            active: false,
            focus_when_ready: false,
            error,
            evict_after: default_evict_after(),
            _evict_task: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            last_bounds: None,
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// 設定 `html_preview_evict_minutes` を適用する（`0` = 自動破棄しない）。
    /// 非表示でタイマー作動中に変更しても仕掛け直しはしない（次の非表示から効く・十分）。
    pub fn set_evict_minutes(&mut self, minutes: u64) {
        if debug_evict_override().is_some() {
            return; // offscreen 検証中は env の短縮値を優先する
        }
        self.evict_after = match minutes {
            0 => None,
            minutes => Some(Duration::from_secs(minutes * 60)),
        };
    }

    /// 親タブが表示対象かを同期する。非表示への遷移は即座にネイティブ子ビューへ反映し、
    /// `evict_after` 経過まで非表示が続いたら WebView を破棄する（再表示で遅延再生成）。
    pub fn set_active(&mut self, active: bool, focus: bool, cx: &mut Context<Self>) {
        if self.active == active {
            if active && focus {
                self.focus();
            }
            return;
        }
        self.active = active;
        self.focus_when_ready = active && focus;
        // 表示に戻ったら破棄タイマーを解除、非表示になったら仕掛ける（タブ切替の速い行き来で
        // 破棄→即再生成の白フレームを出さないため、キャンセルは必ず先）。
        self._evict_task = None;
        if !active {
            self.schedule_evict(cx);
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.set_visible(active) {
                self.error = Some(error.to_string());
            }
            if active && focus {
                if let Err(error) = webview.focus() {
                    self.error = Some(error.to_string());
                }
                self.focus_when_ready = false;
            }
        }
    }

    /// 非表示が `evict_after` 続いたら WebView を破棄するタイマーを仕掛ける。
    fn schedule_evict(&mut self, cx: &mut Context<Self>) {
        let Some(evict_after) = self.evict_after else {
            return;
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if self.webview.is_none() {
            return;
        }
        self._evict_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(evict_after).await;
            let _ = this.update(cx, |view, _| view.evict_if_hidden());
        }));
    }

    /// タイマー着火時の破棄本体。表示に戻っていたら何もしない（防御。通常はタスク drop で着火しない）。
    fn evict_if_hidden(&mut self) {
        if self.active {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            // wry の Drop が removeFromSuperview まで行う＝ここで None にするだけで
            // ネイティブ子ビューと WebContent プロセスが解放される。
            self.webview = None;
            self.last_bounds = None;
        }
    }

    pub fn reload(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.reload() {
                self.error = Some(error.to_string());
            }
        }
    }

    pub fn focus(&mut self) {
        self.focus_when_ready = true;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.focus() {
                self.error = Some(error.to_string());
            } else {
                self.focus_when_ready = false;
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn sync_native_view(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active {
            return;
        }
        let Some(url) = self.url.as_deref() else {
            return;
        };
        let native_bounds = rect(bounds);
        if self.webview.is_none() {
            match WebViewBuilder::new()
                .with_url(url)
                .with_bounds(native_bounds)
                .with_visible(true)
                .with_back_forward_navigation_gestures(true)
                .build_as_child(window)
            {
                Ok(webview) => {
                    if self.focus_when_ready {
                        if let Err(error) = webview.focus() {
                            self.error = Some(error.to_string());
                        } else {
                            self.focus_when_ready = false;
                        }
                    }
                    self.webview = Some(webview);
                    self.last_bounds = Some(bounds);
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                }
            }
            return;
        }
        if self.last_bounds != Some(bounds) {
            if let Some(webview) = &self.webview {
                if let Err(error) = webview.set_bounds(native_bounds) {
                    self.error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            }
            self.last_bounds = Some(bounds);
        }
    }
}

impl Render for WebViewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fallback = self
            .error
            .clone()
            .or_else(|| (!is_supported()).then(|| i18n::t!("webview.unsupported")));
        if let Some(error) = fallback {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .bg(self.theme.bg1)
                .text_color(self.theme.fg2)
                .child(i18n::t!("webview.failed"))
                .child(div().max_w(px(640.)).text_size(px(11.)).child(error))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(self.theme.fg2)
                        .child(self.path.to_string_lossy().to_string()),
                )
                .into_any_element();
        }

        let view = cx.entity();
        div()
            .size_full()
            .bg(self.theme.bg1)
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, cx: &mut App| {
                        view.update(cx, |view, cx| {
                            #[cfg(any(target_os = "macos", target_os = "windows"))]
                            view.sync_native_view(bounds, window, cx);
                            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                            let _ = (view, bounds, window, cx);
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

/// 既定の破棄猶予（settings 適用前のフォールバック = settings_core の既定 15 分と同値）。
fn default_evict_after() -> Option<Duration> {
    debug_evict_override().or(Some(Duration::from_secs(15 * 60)))
}

/// 開発用: `NECODER_WEBVIEW_EVICT_MS=<ms>` で破棄猶予を短縮する（offscreen 検証。debug のみ）。
fn debug_evict_override() -> Option<Duration> {
    #[cfg(debug_assertions)]
    {
        return std::env::var("NECODER_WEBVIEW_EVICT_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .map(Duration::from_millis);
    }
    #[cfg(not(debug_assertions))]
    None
}

fn file_url(path: &Path) -> Result<String, String> {
    url::Url::from_file_path(path)
        .map(|url| url.into())
        .map_err(|_| format!("HTML パスを file URL に変換できません: {}", path.display()))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn rect(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        position: LogicalPosition::new(
            f64::from(f32::from(bounds.origin.x)),
            f64::from(f32::from(bounds.origin.y)),
        )
        .into(),
        size: LogicalSize::new(
            f64::from(f32::from(bounds.size.width).max(1.0)),
            f64::from(f32::from(bounds.size.height).max(1.0)),
        )
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_html_path_becomes_file_url() {
        let path = std::env::temp_dir().join("necoder preview/index.html");
        let url = file_url(&path).expect("絶対パスは URL 化できる");
        assert!(url.starts_with("file://"));
        assert!(url.ends_with("necoder%20preview/index.html"));
    }
}
