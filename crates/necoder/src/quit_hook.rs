//! macOS: Dock の「終了」・AppleScript の `quit` を ⌘Q と同じ確認（O4）へ回す。
//!
//! これらは Quit アクションを通らず、AppKit が 'quit' Apple Event を受けて
//! `-[NSApplication terminate:]` を直接呼ぶ。GPUI のアプリデリゲートは
//! `applicationShouldTerminate:` を実装していないので、デリゲートが作られる前（`App::run` の前）に
//! そのクラスへメソッドを 1 つ足し、**外から来た 'quit' だけ**を取り消して
//! `workspace::request_quit` へ回す（確認が要らなければそちらが即終了する）。
//!
//! 取り消さないもの:
//! - necoder 自身の `cx.quit()`（確認済みの終了・撮影の終了）— Apple Event の処理中ではない。
//! - ログアウト・再起動・システム終了（'quit' に理由 `why?` が付く）— OS の手続きを necoder が止めない。
//!
//! 足せなかった時（GPUI 側のクラス名が変わった等）は何もしない＝従来どおり確認なしで終了する。

use futures::channel::mpsc::UnboundedSender;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};
use std::sync::OnceLock;

/// `request_quit` への送り口（main の前景ループが受ける）。
static QUIT_REQUESTS: OnceLock<UnboundedSender<()>> = OnceLock::new();

/// `NSApplicationTerminateReply`。
const TERMINATE_CANCEL: usize = 0;
const TERMINATE_NOW: usize = 1;

/// Apple Event の 4 文字コード（'aevt' / 'quit' / 'why?'）。
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const QUIT_APPLICATION: u32 = u32::from_be_bytes(*b"quit");
const QUIT_REASON: u32 = u32::from_be_bytes(*b"why?");

pub fn install(requests: UnboundedSender<()>) {
    if QUIT_REQUESTS.set(requests).is_err() {
        return;
    }
    let Some(delegate_class) = AnyClass::get(c"GPUIApplicationDelegate") else {
        eprintln!("アプリのデリゲートが見つからない（Dock の「終了」は確認なしのまま）");
        return;
    };
    let should_terminate: unsafe extern "C-unwind" fn(
        *mut AnyObject,
        Sel,
        *mut AnyObject,
    ) -> usize = application_should_terminate;
    // SAFETY: Objective-C の IMP は引数の型を消した関数ポインタとして登録する決まり。実際の呼び出しは
    // 型文字列 `Q@:@`（NSUInteger を返し、self・_cmd・sender を取る）どおりに行われ、上の型と一致する。
    let implementation: Imp = unsafe { std::mem::transmute(should_terminate) };
    // SAFETY: GPUI が起動時（ctor）に登録済みのクラスへ、まだ実装していないメソッドを 1 つ足すだけ。
    // デリゲートのインスタンスは `App::run` の中で作られるので、この時点ではまだ誰も呼んでいない。
    let added = unsafe {
        ffi::class_addMethod(
            (delegate_class as *const AnyClass).cast_mut(),
            sel!(applicationShouldTerminate:),
            implementation,
            c"Q@:@".as_ptr(),
        )
    };
    if !added.as_bool() {
        eprintln!("applicationShouldTerminate: を足せない（Dock の「終了」は確認なしのまま）");
    }
}

/// `-[NSApplicationDelegate applicationShouldTerminate:]`。
unsafe extern "C-unwind" fn application_should_terminate(
    _this: *mut AnyObject,
    _command: Sel,
    _sender: *mut AnyObject,
) -> usize {
    if !is_quit_request_from_outside() {
        return TERMINATE_NOW;
    }
    match QUIT_REQUESTS.get() {
        Some(requests) if requests.unbounded_send(()).is_ok() => TERMINATE_CANCEL,
        _ => TERMINATE_NOW,
    }
}

/// いま処理中の Apple Event が、理由の付かない 'quit'（Dock の「終了」・`osascript -e 'quit app …'`）か。
fn is_quit_request_from_outside() -> bool {
    // SAFETY: いずれも AppKit の公開メソッドを宣言どおりの型で呼ぶだけ（4 文字コードは UInt32）。
    // 返ってくる記述子はこの呼び出しの間だけ使い、保持しない。
    unsafe {
        let manager: *mut AnyObject =
            msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
        if manager.is_null() {
            return false;
        }
        let event: *mut AnyObject = msg_send![manager, currentAppleEvent];
        if event.is_null() {
            return false;
        }
        let event_class: u32 = msg_send![event, eventClass];
        let event_id: u32 = msg_send![event, eventID];
        if event_class != CORE_EVENT_CLASS || event_id != QUIT_APPLICATION {
            return false;
        }
        let reason: *mut AnyObject = msg_send![event, attributeDescriptorForKeyword: QUIT_REASON];
        reason.is_null()
    }
}
