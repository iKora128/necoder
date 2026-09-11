//! OS 標準 WebView を GPUI の子ビューとして載せる薄い境界。
//!
//! アプリ UI やエディタは従来どおり GPUI で描画し、HTML プレビューを表示している間だけ
//! macOS の WKWebView / Windows の WebView2 を遅延生成する。Chromium 本体は同梱しない。
//!
//! ネイティブ子ビューはキーボードフォーカス（macOS の first responder / Windows のフォーカス HWND）を
//! **OS レベルで**奪う。GPUI 側にこれを取り返す仕組みは無い（`makeFirstResponder` を呼ぶのは窓を
//! 作る瞬間の 1 回だけ）ので、返すのは necoder の責任＝`set_key_focus(false)` を呼ぶ経路を持つ側が
//! 常に正しく呼ばないと、キーの宛先は WebView に残り続ける。返し忘れた時の見え方:
//!
//! - IME（日本語）は first responder の入力コンテキストに属するので、変換が丸ごと WebView 側へ行く
//!   ＝ GPUI の composer にはキャレットが見えているのに 1 文字も入らない
//! - 一方 IME が食わない生のキーは responder chain を上って GPUI に届く＝**ショートカットだけ効く**
//!
//! 返す合図は 3 つ: ①隠す / 破棄する（`set_active(false)`・`evict_if_hidden`）②GPUI 側でマウスダウンが
//! 起きた（＝クリックが WebView の外に落ちた）③GPUI のフォーカスが別の面へ動いた。②③は
//! `Workspace::release_native_key_focus` が呼ぶ。

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

/// ネイティブ子ビューの生成を止める針（既定 OFF）。
///
/// gpui の TestWindow は raw window handle を持たず、`build_as_child` が panic する
/// （`Test Windows are not backed by a real platform window`）。フォーカス受け渡しの配線を
/// gpui テストから通すために、生成だけを黙らせる。`TerminalDock::ensure_active_test` と同じ流儀。
static NATIVE_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn disable_native_webviews_for_tests() {
    NATIVE_DISABLED.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn native_disabled() -> bool {
    NATIVE_DISABLED.load(std::sync::atomic::Ordering::SeqCst)
}

/// ローカル HTML ファイルを OS 標準 WebView で表示する GPUI view。
pub struct WebViewView {
    path: PathBuf,
    url: Option<String>,
    theme: Theme,
    active: bool,
    focus_when_ready: bool,
    /// 最後に OS へ要求したキーボードフォーカスの持ち主（true = この WebView）。
    /// **判断の記録であって OS の真実ではない**。ページ内クリックで first responder は
    /// AppKit が黙って移すので、この値を見て OS への言い直しを省いてはいけない。
    key_focus: bool,
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
            key_focus: false,
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
                self.set_key_focus(true);
            }
            return;
        }
        self.active = active;
        // 表示に戻ったら破棄タイマーを解除、非表示になったら仕掛ける（タブ切替の速い行き来で
        // 破棄→即再生成の白フレームを出さないため、キャンセルは必ず先）。
        self._evict_task = None;
        if !active {
            self.schedule_evict(cx);
            // 隠す前にフォーカスを GPUI 側へ返す（隠れた WebView が first responder のまま
            // キー入力を吸うのを防ぐ）。
            self.set_key_focus(false);
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.set_visible(active) {
                self.error = Some(error.to_string());
            }
        }
        if active {
            self.set_key_focus(focus);
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
            // 念のため破棄直前にもフォーカスを返す（隠した時点で返しているが、Drop 後に
            // first responder が消えたビューを指す状態を作らない防御）。
            if let Some(webview) = &self.webview {
                Self::return_focus_to_parent(webview);
            }
            // wry の Drop が removeFromSuperview まで行う＝ここで None にするだけで
            // ネイティブ子ビューと WebContent プロセスが解放される。
            self.webview = None;
            self.last_bounds = None;
        }
    }

    /// キーボードフォーカスをネイティブ子ビューから GPUI のウィンドウへ返す。
    /// macOS では親 NSView を first responder に戻し、Windows では親 HWND に SetFocus する。
    /// 失敗はプレビューの表示エラーにせず標準エラーへ出すだけに留める（フォーカス移動は付随処理）。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn return_focus_to_parent(webview: &WebView) {
        if let Err(error) = webview.focus_parent() {
            eprintln!("webview_view: フォーカスを親ウィンドウへ返せませんでした: {error}");
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
        self.set_key_focus(true);
    }

    /// OS のキーボードフォーカス（macOS の first responder / Windows のフォーカス HWND）を、
    /// **表示状態は変えずに**渡す / 返す。`false` は「以後のキーは GPUI のもの」。
    ///
    /// 要求は毎回 OS へ言い直す（`key_focus` を見て省かない）。ページ内クリックで WebView が
    /// first responder になるのは AppKit が勝手にやることで、こちらからは観測できない＝手元の
    /// 「持っているつもり」はいつでも OS に裏切られるため。`makeFirstResponder` は同じ responder
    /// なら AppKit が即 YES を返す no-op なので、言い直しは安い。
    pub fn set_key_focus(&mut self, owns: bool) {
        let owns = owns && self.active; // 非表示のビューにキーは渡さない
        self.key_focus = owns;
        self.focus_when_ready = owns; // 遅延生成前なら、生成時にこれが効く
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            let result = if owns {
                webview.focus()
            } else {
                Self::return_focus_to_parent(webview);
                Ok(())
            };
            if let Err(error) = result {
                self.error = Some(error.to_string());
            }
            self.focus_when_ready = false;
        }
    }

    /// この WebView がキーボードフォーカスを持つ想定か（要求の記録。OS の真実ではない）。
    pub fn wants_key_focus(&self) -> bool {
        self.key_focus
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn sync_native_view(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.active || native_disabled() {
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
                // 右クリック →「要素を検証」で Web Inspector を開けるようにする（HTML を書く用途の
                // プレビューなので常時 ON。release で効かせるには wry の `devtools` feature が要る）。
                .with_devtools(true)
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

    /// 非表示のプレビューにキーボードフォーカスを渡さない（隠れた WebView が入力を吸う事故の逆）。
    #[test]
    fn hidden_preview_never_takes_key_focus() {
        let mut view = WebViewView::local_file(std::env::temp_dir().join("a.html"), Theme::dark());
        view.set_key_focus(true);
        assert!(!view.focus_when_ready, "非表示なら渡す予約もしない");
        assert!(!view.wants_key_focus());
    }

    /// 遅延生成前でも「渡す / 返す」の意思は覚え、返す側で予約を消す
    /// （生成直後に古い予約でフォーカスを奪い返さない）。
    #[test]
    fn key_focus_request_is_remembered_before_the_webview_exists() {
        let mut view = WebViewView::local_file(std::env::temp_dir().join("a.html"), Theme::dark());
        view.active = true;
        view.set_key_focus(true);
        assert!(view.focus_when_ready, "表示中なら生成時に渡す");
        assert!(view.wants_key_focus());
        view.set_key_focus(false);
        assert!(!view.focus_when_ready, "返したら予約も消える");
        assert!(!view.wants_key_focus());
    }

    #[test]
    fn local_html_path_becomes_file_url() {
        let path = std::env::temp_dir().join("necoder preview/index.html");
        let url = file_url(&path).expect("絶対パスは URL 化できる");
        assert!(url.starts_with("file://"));
        assert!(url.ends_with("necoder%20preview/index.html"));
    }
}
