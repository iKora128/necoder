//! main_frame — （macOS 専用）Web タブの移動判定を**最上位の文書にだけ**掛ける。
//!
//! wry の `with_navigation_handler` は URL しか渡さないので、WKWebView では iframe の中の移動にも
//! 同じ判定が掛かる。Web タブの線引きは「最上位の文書が localhost から出ない」であって、ページに
//! 埋め込まれた外部の iframe（地図・決済フォーム・認証の隠し iframe など）まで止める理由は無い。
//! むしろ止めると「外部の iframe が 1 枚あるページを開くたびに既定のブラウザのタブが増える」
//! （判定が外部 = ブラウザで開く、なので）。WebView2 の `NavigationStarting` は元から最上位だけに
//! 発火するので、macOS もそれに揃える。
//!
//! やり方: wry が張った navigation delegate を包む delegate を差し込み、
//! `decidePolicyForNavigationAction` のうち iframe 宛て（`targetFrame.isMainFrame == NO`）だけを
//! その場で許可し、それ以外の呼び出しは全部 wry の delegate へそのまま渡す（ページ読み込みの通知・
//! スクリプト注入・ダウンロードは wry の実装が動く）。WKWebView の `navigationDelegate` は weak なので、
//! この delegate は `WebViewView` が WebView と一緒に持つ。
//!
//! ついでに、wry が拾わない「最上位の読み込みの失敗」（開発サーバが起動していない等）を
//! `webView:didFailProvisionalNavigation:withError:` で受けて知らせる。WKWebView は失敗しても
//! 何も描かない（白いまま）ので、理由を GPUI 側で出すための合図になる。

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass as _, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSError, NSObjectProtocol};
use objc2_web_kit::{
    WKDownload, WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKWebView,
};

pub(crate) struct MainFrameOnlyIvars {
    /// wry が張った delegate。iframe の移動判定以外は全部ここへ渡す。
    inner: Retained<ProtocolObject<dyn WKNavigationDelegate>>,
    /// 最上位の読み込みが失敗した時に呼ぶ（引数は OS の説明文）。
    on_load_failed: Box<dyn Fn(String)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MainFrameOnlyIvars]
    pub(crate) struct MainFrameOnlyDelegate;

    unsafe impl NSObjectProtocol for MainFrameOnlyDelegate {}

    unsafe impl WKNavigationDelegate for MainFrameOnlyDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy_for_action(
            &self,
            web_view: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // `targetFrame` が無い = 新しい窓の要求。最上位と同じ扱いで wry（= Web タブの判定）へ。
            let main_frame =
                unsafe { action.targetFrame() }.is_none_or(|frame| unsafe { frame.isMainFrame() });
            if main_frame {
                unsafe {
                    self.ivars()
                        .inner
                        .webView_decidePolicyForNavigationAction_decisionHandler(
                            web_view, action, handler,
                        )
                };
            } else {
                handler.call((WKNavigationActionPolicy::Allow,));
            }
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_policy_for_response(
            &self,
            web_view: &WKWebView,
            response: &WKNavigationResponse,
            handler: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            unsafe {
                self.ivars()
                    .inner
                    .webView_decidePolicyForNavigationResponse_decisionHandler(
                        web_view, response, handler,
                    )
            };
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit_navigation(&self, web_view: &WKWebView, navigation: &WKNavigation) {
            unsafe {
                self.ivars()
                    .inner
                    .webView_didCommitNavigation(web_view, Some(navigation))
            };
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish_navigation(&self, web_view: &WKWebView, navigation: &WKNavigation) {
            unsafe {
                self.ivars()
                    .inner
                    .webView_didFinishNavigation(web_view, Some(navigation))
            };
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional_navigation(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            if is_expected_interruption(&error.domain().to_string(), error.code()) {
                return;
            }
            (self.ivars().on_load_failed)(error.localizedDescription().to_string());
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn navigation_action_did_become_download(
            &self,
            web_view: &WKWebView,
            action: &WKNavigationAction,
            download: &WKDownload,
        ) {
            unsafe {
                self.ivars()
                    .inner
                    .webView_navigationAction_didBecomeDownload(web_view, action, download)
            };
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn navigation_response_did_become_download(
            &self,
            web_view: &WKWebView,
            response: &WKNavigationResponse,
            download: &WKDownload,
        ) {
            unsafe {
                self.ivars()
                    .inner
                    .webView_navigationResponse_didBecomeDownload(web_view, response, download)
            };
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_content_process_did_terminate(&self, web_view: &WKWebView) {
            unsafe {
                self.ivars()
                    .inner
                    .webViewWebContentProcessDidTerminate(web_view)
            };
        }
    }
);

/// 失敗ではない中断か: 取消（`NSURLErrorCancelled` = -999・次の移動が始まった等）と、
/// 方針による中断（WebKit の `FrameLoadInterruptedByPolicyChange` = 102・Web タブが外部への
/// 移動を止めた時にも来る）。
fn is_expected_interruption(domain: &str, code: isize) -> bool {
    (domain == "NSURLErrorDomain" && code == -999) || (domain == "WebKitErrorDomain" && code == 102)
}

/// wry が張った delegate を包んで差し替える。戻り値は WebView と同じ寿命で持つこと
/// （`navigationDelegate` は weak。手放した瞬間に wry の delegate ごと外れる）。
/// 主スレッド以外・delegate がまだ無い（wry の生成が済んでいない）時は `None`（差し替えない）。
pub(crate) fn install(
    web_view: &WKWebView,
    on_load_failed: Box<dyn Fn(String)>,
) -> Option<Retained<MainFrameOnlyDelegate>> {
    let main_thread = MainThreadMarker::new()?;
    let inner = unsafe { web_view.navigationDelegate() }?;
    let delegate = main_thread
        .alloc::<MainFrameOnlyDelegate>()
        .set_ivars(MainFrameOnlyIvars {
            inner,
            on_load_failed,
        });
    let delegate: Retained<MainFrameOnlyDelegate> = unsafe { msg_send![super(delegate), init] };
    unsafe { web_view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    Some(delegate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellations_are_not_reported_as_failures() {
        assert!(is_expected_interruption("NSURLErrorDomain", -999));
        assert!(is_expected_interruption("WebKitErrorDomain", 102));
        // 接続できない（サーバ未起動）・時間切れは知らせる。
        assert!(!is_expected_interruption("NSURLErrorDomain", -1004));
        assert!(!is_expected_interruption("NSURLErrorDomain", -1001));
    }
}
