//! inspector — （macOS 専用）Web Inspector を **necoder の窓の中に付けない**ための口。
//!
//! WKWebView の Web Inspector（wry の `open_devtools` = 私的メソッド `-[WKWebView _inspector]` の
//! `show`）は、アプリの設定 `inspectorStartsAttached`（既定 = 付ける）に従って WebView の横に
//! **付けた**状態で開く。付けた時の WebKit（`WebInspectorUIProxyMac.mm` の `platformAttach` /
//! `inspectedViewFrameDidChange`）は、WKWebView を**親ビューの幅いっぱい**に広げ、Inspector のビューを
//! その親の下端（左右に付けた時は親の高さいっぱい）に差し込む。necoder の WKWebView の親は GPUI の窓の
//! content view（窓全体）なので、左のレール・エクスプローラ・右のパネル・下のステータスバーがネイティブの
//! ビューの下に隠れ、GPUI からは何も押せなくなる（2026-09-27 本人報告「全画面になって何も戻れない」）。
//! 付けたまま閉じても `platformDetach` が WKWebView を親の幅いっぱいに置き直すので、閉じた後も広がったまま残る。
//!
//! ここでは次の 4 つだけを持つ（判断は `WebViewView` の側）:
//!
//! - [`supports_devtools`]: この WebKit が wry の devtools の口と `detach` を持つか（無ければ使わない）
//! - [`detach`]: 見えている Inspector を**別の窓**へ出す（`_WKInspector` の `detach`。付いていなければ
//!   WebKit の側で何もしない）。見えている時に 1 度呼べば、WebKit がこのアプリの設定を「窓で開く」に
//!   書き換えるので、次からは初めから別の窓で開く
//! - [`FrameWatch`]: WKWebView の枠が変わったら知らせる（`NSViewFrameDidChangeNotification`・公開の通知。
//!   WebKit 自身も、付けた Inspector を並べ直すのにこの通知を見ている）。WebKit が付けた・付けたまま閉じた
//!   ことはこれで知り、枠を GPUI の置き場所へ戻す
//! - [`holds_first_responder`]: OS のキーボードフォーカスがページにあるか（Esc をページの物として残す判定）
//!
//! 私的 API は wry が既に使っている `_inspector`（`_WKInspector`）の範囲に留める。`_WKInspector` の
//! `detach` は WebKit の SPI（`_WKInspector.h`）で、Inspector の画面にある「別のウインドウに切り離す」
//! ボタンと同じ WebKit の処理（`WebInspectorUIProxy::detach`）へ行く。知らないセレクタへ送ると
//! ObjC の例外で落ちるので、呼ぶ前に `respondsToSelector:` で確かめる。

use objc2::rc::Retained;
use objc2::runtime::{NSObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass as _, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSView, NSViewFrameDidChangeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use objc2_web_kit::WKWebView;

/// `web_view` の `_WKInspector`（無い・`selector` を持たない時は `None`）。
fn inspector_with(web_view: &WKWebView, selector: Sel) -> Option<Retained<NSObject>> {
    if !web_view.respondsToSelector(sel!(_inspector)) {
        return None;
    }
    // SAFETY: `_inspector` は wry の open / close / is_devtools_open が呼んでいるのと同じ私的メソッドで、
    // 引数を取らず `_WKInspector *`（NSObject の子）を返す。持っていることは上で確かめた。
    let inspector: Option<Retained<NSObject>> = unsafe { msg_send![web_view, _inspector] };
    inspector.filter(|inspector| inspector.respondsToSelector(selector))
}

/// wry の devtools の口（`_inspector` の `show` / `close` / `isVisible`）と [`detach`] を、この WebKit が
/// 持つか。wry は確かめずに送り、無いセレクタへ送ると ObjC の例外で落ちる。`is_devtools_open` は描画の
/// たびに呼ぶので、WebView を作った時に 1 度確かめ、持たなければ Inspector を使わない（`</>` は何もしない）。
/// `detach` が無いと付いた Inspector を窓へ出せない（necoder の UI を覆う）ので、それも条件に入れる。
pub(crate) fn supports_devtools(web_view: &WKWebView) -> bool {
    inspector_with(web_view, sel!(isVisible)).is_some_and(|inspector| {
        [sel!(show), sel!(close), sel!(detach)]
            .into_iter()
            .all(|selector| inspector.respondsToSelector(selector))
    })
}

/// 見えている Inspector を別の窓へ出す（付いていなければ WebKit の側で何もしない＝何度呼んでもよい）。
/// **見えている時だけ呼ぶこと**: WebKit は「開きかけ」（`show` を頼んでまだ見えていない）の間に
/// `detach` を受けると、付いていなくても WKWebView の枠を親の幅いっぱいに置き直す。
/// `detach` が無い（将来の WebKit で消えた）時は `false` ＝ 呼び側が閉じる（窓の上に残さない）。
pub(crate) fn detach(web_view: &WKWebView) -> bool {
    let Some(inspector) = inspector_with(web_view, sel!(detach)) else {
        return false;
    };
    // SAFETY: `detach` は引数も返り値も無い（`_WKInspector.h`）。持っていることは上で確かめた。
    unsafe {
        let () = msg_send![&inspector, detach];
    }
    true
}

/// OS のキーボードフォーカス（窓の first responder）が `web_view` か、その中のビューにあるか。
pub(crate) fn holds_first_responder(web_view: &WKWebView) -> bool {
    let Some(window) = web_view.window() else {
        return false;
    };
    let Some(responder) = window.firstResponder() else {
        return false;
    };
    responder
        .downcast_ref::<NSView>()
        .is_some_and(|view| view.isDescendantOf(web_view))
}

pub(crate) struct FrameObserverIvars {
    on_change: Box<dyn Fn()>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = FrameObserverIvars]
    pub(crate) struct FrameObserver;

    unsafe impl NSObjectProtocol for FrameObserver {}

    impl FrameObserver {
        #[unsafe(method(necoderWebViewFrameDidChange:))]
        fn frame_did_change(&self, _notification: &NSNotification) {
            (self.ivars().on_change)();
        }
    }
);

/// WKWebView の枠が変わるたびに `on_change` を呼ぶ見張り。落とすと外れる（WebView と同じ寿命で持つ）。
///
/// `on_change` は枠を変えた呼び出しの**中で**同期に呼ばれる（WebKit が Inspector を付けている最中を含む）。
/// そこで WebKit や GPUI を触ると入れ子になるので、知らせを積むだけにすること。
pub(crate) struct FrameWatch {
    observer: Retained<FrameObserver>,
}

impl FrameWatch {
    /// 主スレッド以外では `None`（付けない）。
    pub(crate) fn install(web_view: &WKWebView, on_change: Box<dyn Fn()>) -> Option<FrameWatch> {
        let main_thread = MainThreadMarker::new()?;
        let observer = main_thread
            .alloc::<FrameObserver>()
            .set_ivars(FrameObserverIvars { on_change });
        let observer: Retained<FrameObserver> = unsafe { msg_send![super(observer), init] };
        // NSView の `postsFrameChangedNotifications` は既定で YES（WebKit もこれを当てにしている）。
        // SAFETY: observer は `necoderWebViewFrameDidChange:`（NSNotification を 1 つ取る）を持つ。
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &observer,
                sel!(necoderWebViewFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(web_view),
            );
        }
        Some(FrameWatch { observer })
    }
}

impl Drop for FrameWatch {
    fn drop(&mut self) {
        // SAFETY: 付けた時の observer をそのまま外す。
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(&self.observer) };
    }
}
