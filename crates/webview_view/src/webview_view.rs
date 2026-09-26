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
//!
//! 入口は 3 つ: ローカル HTML（[`WebViewView::local_file`]・`file://`）/ エージェントが書いた HTML を
//! 閉じ込める artifact（[`WebViewView::sandboxed`]・[`sandbox`]）/ 開発サーバを見る Web タブ
//! （[`WebViewView::localhost`]・[`localhost`]）。移動の線引きと IPC の有無は入口ごとに決まり、
//! 混ぜない（artifact は IPC 無し・Web タブは最上位が localhost から出ず、IPC は Design Mode の
//! 知らせだけ＝受け手が [`design::accept_message`] で検める）。

pub mod design;
pub mod localhost;
#[cfg(target_os = "macos")]
mod main_frame;
pub mod sandbox;
pub mod snapshot;

use futures::channel::mpsc;
use futures::StreamExt as _;
use gpui::{
    canvas, div, prelude::*, px, App, Bounds, Context, EventEmitter, IntoElement, Pixels, Task,
    Window,
};
use std::path::{Path, PathBuf};
use std::time::Duration;
use theme_core::Theme;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    Rect, WebView, WebViewBuilder,
};

/// Web タブ（[`WebViewView::localhost`]）の WebView から GPUI 側へ届く知らせ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebViewEvent {
    /// 最上位の文書の読み込みが始まった（`finished = false`）/ 終わった。`url` は WebView が今見ている URL。
    PageLoad { url: String, finished: bool },
    /// 文書のタイトルが変わった（空文字もそのまま届く）。
    TitleChanged(String),
    /// ページからの IPC（`window.ipc.postMessage`）。**中身は検証していない** —
    /// 受け手が送り手・状態・nonce・形・大きさを確かめてから使う（[`design::accept_message`]）。
    /// `sender` は送り手の文書の URL（wry が添える。埋め込まれた iframe から来たらその iframe の URL）。
    /// IPC を付けた Web タブ（[`WebViewView::enable_ipc`]）だけが出す。
    Message { sender: String, body: String },
}

/// wry のコールバック（主スレッド・`'static`）から GPUI の Entity へ渡す中継。
/// WebView を持たない OS（Linux）では積む側が居ない。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
enum NativeEvent {
    PageLoad { url: String, finished: bool },
    Title(String),
    LoadFailed(String),
    Message { sender: String, body: String },
}

/// Web タブだけが持つ状態。WebView を持たない OS（Linux）では生成に使う欄が読まれない。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
struct WebState {
    /// wry のコールバックが積む口。受け側は `_pump` が Entity へ流す。
    events: mpsc::UnboundedSender<NativeEvent>,
    _pump: Task<()>,
    /// ページのズーム（WKWebView の pageZoom / WebView2 の ZoomFactor）。破棄→再生成でも保つ。
    zoom: f64,
    title: Option<String>,
    loading: bool,
    /// 最上位の読み込みの失敗（サーバが起動していない等）。この間は WebView を隠して理由を出す。
    failed: Option<String>,
    /// 文書の頭で毎回走らせるスクリプト（最上位の文書のみ）。生成時にしか渡せないので覚えておく。
    initialization_scripts: Vec<String>,
    /// ページ → necoder の IPC を受けるか（生成時にしか付けられない）。
    ipc: bool,
}

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
    /// 閉じ込めて見せる時の配信の根（[`sandbox`]）。`None` は従来どおり `file://` で開く。
    sandbox_root: Option<PathBuf>,
    /// Web タブ（[`Self::localhost`]）の状態。`None` はファイルを見るプレビュー。
    web: Option<WebState>,
    /// 非表示化で仕掛ける破棄タイマー。再表示（set_active(true)）で drop ＝キャンセル。
    _evict_task: Option<Task<()>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    webview: Option<WebView>,
    /// Web タブの移動判定を最上位だけに掛ける delegate（[`main_frame`]）。WebView と同じ寿命。
    #[cfg(target_os = "macos")]
    main_frame_delegate: Option<objc2::rc::Retained<main_frame::MainFrameOnlyDelegate>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    last_bounds: Option<Bounds<Pixels>>,
}

impl EventEmitter<WebViewEvent> for WebViewView {}

impl WebViewView {
    pub fn local_file(path: impl Into<PathBuf>, theme: Theme) -> Self {
        let path = path.into();
        let (url, error) = match file_url(&path) {
            Ok(url) => (Some(url), None),
            Err(error) => (None, Some(error)),
        };
        Self::with_source(path, url, error, theme)
    }

    fn with_source(
        path: PathBuf,
        url: Option<String>,
        error: Option<String>,
        theme: Theme,
    ) -> Self {
        Self {
            path,
            url,
            theme,
            active: false,
            focus_when_ready: false,
            key_focus: false,
            error,
            evict_after: default_evict_after(),
            sandbox_root: None,
            web: None,
            _evict_task: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            webview: None,
            #[cfg(target_os = "macos")]
            main_frame_delegate: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            last_bounds: None,
        }
    }

    /// 開発サーバを見る **Web タブ**（[`localhost`]）。読み込めるのはループバックの http(s) だけで、
    /// 最上位の文書がそこから出る移動と新しい窓はキャンセルして既定のブラウザへ渡す。
    /// `url` が範囲外ならエラー表示になる（黙って別の URL に倒さない）。
    pub fn localhost(url: &str, theme: Theme, cx: &mut Context<Self>) -> Self {
        let normalized = localhost::normalize(url);
        let error = normalized
            .is_none()
            .then(|| i18n::t!("webview.outside_localhost").to_string());
        let mut view = Self::with_source(PathBuf::from(url), normalized, error, theme);
        let (events, mut receiver) = mpsc::unbounded::<NativeEvent>();
        let pump = cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                if this
                    .update(cx, |view, cx| view.on_native_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        view.web = Some(WebState {
            events,
            _pump: pump,
            zoom: 1.0,
            title: None,
            loading: false,
            failed: None,
            initialization_scripts: Vec::new(),
            ipc: false,
        });
        view
    }

    /// Web タブの文書の頭で毎回走らせるスクリプトを足す（最上位の文書のみ・Design Mode のピッカー）。
    /// WebView の生成時にしか渡せないので、表示前（遅延生成の前）に呼ぶ。ファイルのプレビューと
    /// artifact には入れない（呼んでも無視）。
    pub fn add_initialization_script(&mut self, script: impl Into<String>) {
        if let Some(web) = self.web.as_mut() {
            web.initialization_scripts.push(script.into());
        }
    }

    /// Web タブでページ → necoder の IPC を受ける（[`WebViewEvent::Message`]）。生成時にしか付けられない
    /// ので表示前に呼ぶ。**Web タブだけ**が持つ口で、ファイルのプレビューと artifact には付けない
    /// （呼んでも無視・artifact の隔離は `sandbox` の注記のとおり IPC 無しのまま）。
    pub fn enable_ipc(&mut self) {
        if let Some(web) = self.web.as_mut() {
            web.ipc = true;
        }
    }

    pub fn is_web(&self) -> bool {
        self.web.is_some()
    }

    /// Web タブがいま見ている URL（最上位の文書・読み込みのたびに追従）。
    pub fn current_url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Web タブの文書のタイトル（空なら `None`）。
    pub fn page_title(&self) -> Option<&str> {
        self.web.as_ref().and_then(|web| web.title.as_deref())
    }

    pub fn is_loading(&self) -> bool {
        self.web.as_ref().is_some_and(|web| web.loading)
    }

    /// 最上位の読み込みの失敗の説明（失敗していなければ `None`）。
    pub fn load_failure(&self) -> Option<&str> {
        self.web.as_ref().and_then(|web| web.failed.as_deref())
    }

    pub fn zoom(&self) -> f64 {
        self.web.as_ref().map_or(1.0, |web| web.zoom)
    }

    /// ページのズームを変える（WebView がまだ無ければ生成時に効く）。
    pub fn set_zoom(&mut self, zoom: f64) {
        let Some(web) = self.web.as_mut() else {
            return;
        };
        web.zoom = zoom;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.zoom(zoom) {
                eprintln!("webview_view: ズームを変えられない: {error}");
            }
        }
    }

    pub fn can_go_back(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            return webview.can_go_back().unwrap_or(false);
        }
        false
    }

    pub fn can_go_forward(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            return webview.can_go_forward().unwrap_or(false);
        }
        false
    }

    pub fn go_back(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.go_back() {
                eprintln!("webview_view: 戻れない: {error}");
            }
        }
    }

    pub fn go_forward(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.go_forward() {
                eprintln!("webview_view: 進めない: {error}");
            }
        }
    }

    /// Web Inspector（DevTools）を開く。WebView がまだ無ければ何もしない。
    pub fn open_devtools(&self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            webview.open_devtools();
        }
    }

    /// ページの中でスクリプトを走らせる（結果は捨てる）。WebView がまだ無ければ `false`。
    pub fn evaluate_script(&self, script: &str) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            return match webview.evaluate_script(script) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("webview_view: スクリプトを走らせられない: {error}");
                    false
                }
            };
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = script;
        false
    }

    /// Design Mode: ビューポート基準の CSS px の矩形（`rect`）を切り抜いて PNG で返す。
    /// `done` は撮り終えた時に主スレッドで 1 回だけ呼ばれる。WebView がまだ無い・矩形が見えている範囲の
    /// 外・撮り始められない時は `Err` をすぐ返す（その時 `done` は呼ばれない）。
    pub fn capture_element(
        &self,
        rect: &design::Rect,
        viewport_css_width: f64,
        done: snapshot::SnapshotDone,
    ) -> Result<(), String> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let (Some(webview), Some(bounds)) = (&self.webview, self.last_bounds) else {
                return Err("the web view is not shown".to_string());
            };
            let view_width = f64::from(f32::from(bounds.size.width));
            let view_height = f64::from(f32::from(bounds.size.height));
            let view_rect =
                snapshot::to_view_rect(rect, viewport_css_width, view_width, view_height)
                    .ok_or_else(|| "the element is outside the visible area".to_string())?;
            snapshot::capture(webview, view_rect, view_width, done)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (rect, viewport_css_width, done);
            Err("web views are not supported here".to_string())
        }
    }

    /// 開発用: 見えている範囲をまるごと撮る（Design の検証で、ページの上のホバー枠を見るため）。
    #[cfg(debug_assertions)]
    pub fn capture_visible(&self, done: snapshot::SnapshotDone) -> Result<(), String> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let (Some(webview), Some(bounds)) = (&self.webview, self.last_bounds) else {
                return Err("the web view is not shown".to_string());
            };
            let view_width = f64::from(f32::from(bounds.size.width));
            let view_height = f64::from(f32::from(bounds.size.height));
            let whole = snapshot::ViewRect {
                x: 0.0,
                y: 0.0,
                width: view_width,
                height: view_height,
            };
            snapshot::capture(webview, whole, view_width, done)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = done;
            Err("web views are not supported here".to_string())
        }
    }

    /// ページの中でスクリプトを走らせ、結果（JSON 文字列）を `callback` へ渡す。
    /// `callback` は主スレッドで呼ばれる。WebView がまだ無ければ `false`（呼ばれない）。
    pub fn evaluate_script_with_callback(
        &self,
        script: &str,
        callback: impl Fn(String) + Send + 'static,
    ) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            return match webview.evaluate_script_with_callback(script, callback) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("webview_view: スクリプトを走らせられない: {error}");
                    false
                }
            };
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = (script, callback);
        false
    }

    /// wry のコールバックから届いた知らせを状態へ反映し、購読者へ流す。
    fn on_native_event(&mut self, event: NativeEvent, cx: &mut Context<Self>) {
        let Some(web) = self.web.as_mut() else {
            return;
        };
        match event {
            NativeEvent::PageLoad { url, finished } => {
                web.loading = !finished;
                if !finished {
                    web.failed = None;
                }
                // 再生成（15 分の破棄の後）で戻る先を、最後に見ていた localhost の画面にする。
                if localhost::is_allowed(&url) {
                    self.url = Some(url.clone());
                }
                cx.emit(WebViewEvent::PageLoad { url, finished });
            }
            NativeEvent::Title(title) => {
                web.title = (!title.trim().is_empty()).then(|| title.clone());
                cx.emit(WebViewEvent::TitleChanged(title));
            }
            NativeEvent::LoadFailed(description) => {
                web.loading = false;
                web.failed = Some(description);
                // WKWebView は失敗しても白いままなので、隠して理由を GPUI 側に出す。
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                if let Some(webview) = &self.webview {
                    if let Err(error) = webview.set_visible(false) {
                        eprintln!("webview_view: 隠せない: {error}");
                    }
                }
                self.set_key_focus(false);
            }
            NativeEvent::Message { sender, body } => {
                cx.emit(WebViewEvent::Message { sender, body })
            }
        }
        cx.notify();
    }

    /// エージェントが生成した HTML を**閉じ込めて**見せる（[`sandbox`]）。配信するのは `root` の中だけで、
    /// ページは外部と通信できず、外へ移動できず、necoder を呼ぶ口も持たない。
    /// `path` が `root` の外ならエラー表示になる（黙って `file://` に倒さない）。
    pub fn sandboxed(root: impl Into<PathBuf>, path: impl Into<PathBuf>, theme: Theme) -> Self {
        let (root, path) = (root.into(), path.into());
        let url = sandbox::url_for(&root, &path);
        let error = url
            .is_none()
            .then(|| i18n::t!("webview.outside_sandbox").to_string());
        let mut view = Self::local_file(path, theme);
        view.url = url;
        view.error = error;
        view.sandbox_root = Some(root);
        view
    }

    pub fn is_sandboxed(&self) -> bool {
        self.sandbox_root.is_some()
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
        // 「前面の UI を閉じると表示に戻ります」の一行を出し入れする（描画は cached なので知らせる）。
        cx.notify();
        // 表示に戻ったら破棄タイマーを解除、非表示になったら仕掛ける（タブ切替の速い行き来で
        // 破棄→即再生成の白フレームを出さないため、キャンセルは必ず先）。
        self._evict_task = None;
        if !active {
            self.schedule_evict(cx);
            // 隠す前にフォーカスを GPUI 側へ返す（隠れた WebView が first responder のまま
            // キー入力を吸うのを防ぐ）。
            self.set_key_focus(false);
        }
        // 読み込みに失敗した Web タブは、表示中でも WebView を隠したまま理由を出す（render）。
        let visible = active && self.load_failure().is_none();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(webview) = &self.webview {
            if let Err(error) = webview.set_visible(visible) {
                self.error = Some(error.to_string());
            }
        }
        if visible {
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
            // 包んでいた delegate は WebView の後に手放す（先に外すと wry の delegate が外れる）。
            #[cfg(target_os = "macos")]
            {
                self.main_frame_delegate = None;
            }
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
        // 読み込みに失敗した Web タブは、失敗したページではなく元の URL を読み直す
        // （WKWebView の reload は「最後に成功した文書」をやり直すため）。
        if let Some(web) = self.web.as_mut() {
            if web.failed.take().is_some() {
                web.loading = true;
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                if let (Some(webview), Some(url)) = (&self.webview, self.url.as_deref()) {
                    if let Err(error) = webview.load_url(url) {
                        self.error = Some(error.to_string());
                    }
                    if let Err(error) = webview.set_visible(self.active) {
                        self.error = Some(error.to_string());
                    }
                }
                return;
            }
        }
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

    /// 表示対象として同期されているか（＝ネイティブ子ビューを出してよい状態か）。
    /// 「オーバーレイ中は隠す」判断が効いているかを外から観測する点。
    pub fn is_active(&self) -> bool {
        self.active
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
            let mut builder = WebViewBuilder::new();
            if let Some(root) = self.sandbox_root.clone() {
                builder = sandboxed_builder(builder, root);
            } else if let Some(web) = &self.web {
                builder = localhost_builder(builder, web);
            }
            match builder
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
                    if let Some(web) = &self.web {
                        self.after_web_build(&webview, web.zoom, web.events.clone());
                    }
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

impl WebViewView {
    /// Web タブの WebView を作った直後の仕上げ: 移動判定を最上位だけにする（macOS）・ズームを戻す。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn after_web_build(
        &mut self,
        webview: &WebView,
        zoom: f64,
        events: mpsc::UnboundedSender<NativeEvent>,
    ) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS as _;
            let wk_web_view = webview.webview();
            self.main_frame_delegate = main_frame::install(
                &wk_web_view,
                Box::new(move |description| {
                    // 受け側（この view）が先に消えていれば届け先は無い。それで困る物は無い。
                    events
                        .unbounded_send(NativeEvent::LoadFailed(description))
                        .ok();
                }),
            );
            if self.main_frame_delegate.is_none() {
                eprintln!("webview_view: 移動判定を最上位だけに絞れない（delegate が無い）");
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = events;
        if (zoom - 1.0).abs() > f64::EPSILON {
            if let Err(error) = webview.zoom(zoom) {
                eprintln!("webview_view: ズームを戻せない: {error}");
            }
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
            let headline = if self.web.is_some() {
                i18n::t!("webview.web_failed")
            } else {
                i18n::t!("webview.failed")
            };
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .bg(self.theme.bg1)
                .text_color(self.theme.fg2)
                .child(headline)
                .child(div().max_w(px(640.)).text_size(px(11.)).child(error))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(self.theme.fg2)
                        .child(self.path.to_string_lossy().to_string()),
                )
                .into_any_element();
        }

        // Web タブの読み込み失敗（開発サーバが起動していない等）。WKWebView は白いままなので、
        // WebView を隠して（`on_native_event`）理由と戻し方をここに出す。
        if let Some(failure) = self.load_failure() {
            let target = self
                .url
                .as_deref()
                .map(localhost::display_label)
                .unwrap_or_default();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .bg(self.theme.bg1)
                .text_color(self.theme.fg2)
                .child(
                    div()
                        .text_color(self.theme.fg1)
                        .child(i18n::t!("webview.unreachable", "target" => target)),
                )
                .child(
                    div()
                        .max_w(px(640.))
                        .text_size(px(11.))
                        .child(failure.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .child(i18n::t!("webview.unreachable_hint")),
                )
                .into_any_element();
        }

        let view = cx.entity();
        // 描画木に居るのに非表示 ＝ 手前のオーバーレイに場所を譲っている間（`Workspace::
        // overlay_hides_native_view`）。タブが非選択なら描画木にも居ないので、ここは
        // 「オーバーレイ中」だけ。黙って bg1 の空面にすると「プレビューが消えた」に見えるため、
        // 戻し方を 1 行書く。
        let hidden_by_overlay = !self.active;
        div()
            .size_full()
            .relative()
            .bg(self.theme.bg1)
            .when(hidden_by_overlay, |element| {
                element.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.))
                        .text_color(self.theme.fg2)
                        .child(i18n::t!("webview.hidden_by_overlay")),
                )
            })
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

/// 閉じ込め用の設定を足す: 専用スキームでの配信・外への移動の禁止・新しい窓の禁止。
/// **IPC ハンドラは付けない**（ページから necoder を呼ぶ口を作らない）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn sandboxed_builder(builder: WebViewBuilder<'_>, root: PathBuf) -> WebViewBuilder<'_> {
    use std::borrow::Cow;
    use wry::http::{header, Response};
    // 開発用の覗き窓: 隔離の検証ページは結果を外へ出す手段を持たない（それが隔離）ので、
    // `document.title` に書いた結果を necoder 側でファイルへ落とす。debug ビルドで
    // `NECODER_WEBVIEW_PROBE_LOG` を置いた時だけ。
    #[cfg(debug_assertions)]
    let builder = match std::env::var_os("NECODER_WEBVIEW_PROBE_LOG") {
        Some(log) => builder.with_document_title_changed_handler(move |title| {
            use std::io::Write as _;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log);
            if let Ok(mut file) = file {
                if let Err(error) = writeln!(file, "{title}") {
                    eprintln!("WEBVIEW_PROBE: 書けない: {error}");
                }
            }
        }),
        None => builder,
    };
    builder
        .with_custom_protocol(sandbox::SCHEME.to_string(), move |_id, request| {
            let served = sandbox::serve(&root, request.uri().path());
            let body: Cow<'static, [u8]> = Cow::Owned(served.body);
            let response = Response::builder()
                .status(served.status)
                .header(header::CONTENT_TYPE, served.content_type)
                .header("Content-Security-Policy", sandbox::CONTENT_SECURITY_POLICY)
                .header("X-Content-Type-Options", "nosniff")
                .header(header::CACHE_CONTROL, "no-store")
                .body(body.clone());
            // ヘッダは全部定数なので組み立ては失敗しない。万一の時も本文だけは返す。
            response.unwrap_or_else(|_| Response::new(body))
        })
        .with_navigation_handler(|url| {
            if sandbox::is_internal(&url) {
                return true;
            }
            sandbox::open_in_browser(&url);
            false
        })
        .with_new_window_req_handler(|url, _features| {
            sandbox::open_in_browser(&url);
            wry::NewWindowResponse::Deny
        })
}

/// Web タブ用の設定を足す: 最上位の文書が localhost から出る移動と新しい窓はキャンセルして既定の
/// ブラウザへ（[`localhost`]）。読み込み・タイトル・（付けた時だけ）IPC は GPUI 側へ中継する。
/// iframe の中の移動はここを通さない（macOS は [`main_frame`]・WebView2 は元から最上位だけ）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn localhost_builder<'a>(builder: WebViewBuilder<'a>, web: &WebState) -> WebViewBuilder<'a> {
    // 受け側（view）が先に消えた後の送信は届け先が無いだけ（`.ok()`）。
    let page_events = web.events.clone();
    let title_events = web.events.clone();
    let mut builder = builder
        .with_navigation_handler(|url| match localhost::navigation(&url) {
            localhost::Navigation::Allow => true,
            localhost::Navigation::OpenInBrowser => {
                localhost::open_in_browser(&url);
                false
            }
            localhost::Navigation::Block => false,
        })
        .with_new_window_req_handler(|url, _features| {
            if localhost::new_window(&url) == localhost::Navigation::OpenInBrowser {
                localhost::open_in_browser(&url);
            }
            wry::NewWindowResponse::Deny
        })
        .with_on_page_load_handler(move |event, url| {
            let finished = matches!(event, wry::PageLoadEvent::Finished);
            page_events
                .unbounded_send(NativeEvent::PageLoad { url, finished })
                .ok();
        })
        .with_document_title_changed_handler(move |title| {
            title_events.unbounded_send(NativeEvent::Title(title)).ok();
        });
    for script in &web.initialization_scripts {
        builder = builder.with_initialization_script(script.as_str());
    }
    // IPC は Web タブにだけ付ける（artifact は `sandboxed_builder` で付けないまま）。中身の検証は受け手。
    if web.ipc {
        let message_events = web.events.clone();
        builder = builder.with_ipc_handler(move |request| {
            let sender = request.uri().to_string();
            let body = request.into_body();
            message_events
                .unbounded_send(NativeEvent::Message { sender, body })
                .ok();
        });
    }
    builder
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

    /// IPC の送り手は wry が `http::Uri` に通してから渡す。localhost の文書の URL はその形でも
    /// localhost と判定され（fragment は Uri が落とす）、外部の iframe は外部のまま。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn ipc_senders_keep_their_host_through_the_uri() {
        for (sender, allowed) in [
            ("http://127.0.0.1:56525/", true),
            ("http://localhost:5173/app?x=1#top", true),
            ("http://[::1]:4000/", true),
            ("https://ads.example/frame.html", false),
            ("http://necoder-artifact.localhost/timer.html", false),
        ] {
            let uri: wry::http::Uri = sender.parse().expect("wry が組める URL");
            assert_eq!(localhost::is_allowed(&uri.to_string()), allowed, "{sender}");
        }
    }

    #[test]
    fn local_html_path_becomes_file_url() {
        let path = std::env::temp_dir().join("necoder preview/index.html");
        let url = file_url(&path).expect("絶対パスは URL 化できる");
        assert!(url.starts_with("file://"));
        assert!(url.ends_with("necoder%20preview/index.html"));
    }
}
