//! web_preview_view — localhost の開発サーバを見る **Web タブ**（`TabContent::Web`・Pane/Item の第 4 の具体型）。
//!
//! 中身は `webview_view` の Web タブ（[`WebViewView::localhost`]）で、読み込める範囲の線引き
//! （ループバックの http(s) だけ・外への移動と新しい窓は既定のブラウザへ）はそちらが持つ。
//! この器が持つのは、エディタのパンくずの位置に置くツールバー（戻る / 進む / 再読込 / URL /
//! ズーム / ビューポート幅 / DevTools / 既定のブラウザで開く）と、ビューポート幅を絞った時に
//! WebView の矩形を中央へ寄せることだけ。汎用ブラウザにはしない — 任意の URL を打つ欄は置かない
//! （URL は表示だけ。開く入口はパレット「プレビュー: localhost を開く…」と、エージェントの
//! transcript・Markdown プレビューのリンク = `Workspace::open_url`）。
//!
//! 色は識別に集約（UI-SPEC §1.3）: ツールバーは bg1・fg2/fg0 と中立の選択面 bg3 だけで、識別色は使わない。
//!
//! **HTML ファイル**も同じ Web タブで開ける（内蔵の配信・`webview_view::static_server`）。その時は
//! [`StaticSource`] が配っているフォルダの口を握り（このタブが生きている間だけ配信が動く）、鍵は
//! ファイルの `file://` URL（[`static_tab_key`]）、ツールバーの URL 欄は token 付きの URL ではなく
//! フォルダからの相対パスを出す。
//!
//! **Design Mode**（⌘⇧D・ツールバーの Design）: ページの要素を選び、その HTML・計算済みスタイル・
//! 切り抜き（PNG）をアクティブなスレッドの composer へ添える（送信はしない）。ピッカーは生成時に
//! 文書の頭へ入れておき（`webview_view::design`）、Design の間だけ nonce を渡して動かす。ページ →
//! necoder の IPC は Web タブにだけ付き、Design 中かつ nonce が一致した物だけを受ける。
//!
//! 切り抜きは非同期（台帳 R05）。宛先（session・パネル・スレッド）は**選んだ時点で**固定し
//! （`PickStarted` → [`WebPreviewView::set_pick_target`]）、Design とページの世代も選んだ時の値を持って
//! 完了時に照合する。古くなった選択（Design を抜けた・始め直した・再読込）は添えずに捨て、新しい
//! Design には触らない。複数選択は撮り終えた順ではなく選んだ順に知らせる。

use crate::workspace::*;
use std::collections::VecDeque;
use webview_view::{design, localhost, snapshot, static_server, WebViewEvent, WebViewView};

/// Design Mode の知らせ（Workspace が選んだ時点のスレッドの composer へ添える）。
pub(crate) enum WebPreviewEvent {
    /// 要素が選ばれた（切り抜きはこれから）。受け手は**この時点の**宛先を
    /// [`WebPreviewView::set_pick_target`] で固定する — 完了時に引き直すと、撮っている間に
    /// スレッドや Task を切り替えた時に別の会話へ付いてしまう。
    PickStarted { pick: u64 },
    /// 撮り終えた要素。**選んだ順に**届く（切り抜きの完了が前後しても並べ直してから出す）。
    ElementPicked {
        capture: Box<design::ElementCapture>,
        /// 要素の切り抜き（撮れなかった時は `None` ＝ テキストだけ添える）。
        png: Option<Vec<u8>>,
        /// `PickStarted` で固定した宛先（受け手が入れなかった時は `None`）。
        target: Option<DesignTarget>,
        /// この選択で Design が終わった（Shift+クリックで続ける時は `false`）。
        finished: bool,
    },
    /// 撮っている間に Design が終わった・始め直された・ページが読み込み直された＝その選択は古い。
    /// 添えずに捨てたことを知らせる（新しい Design には触らない）。
    PickDropped { reason: PickDropReason },
}

/// 選んだ要素を添えずに捨てた理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickDropReason {
    /// Design を抜けた（Esc・⌘⇧D・始め直し）。
    DesignEnded,
    /// ページが読み込み直された（切り抜きが選んだ要素の物とは限らない）。
    PageChanged,
}

/// Design 中の状態（Design 中だけ `Some`）。
struct DesignSession {
    /// この Design の合言葉。ページからの知らせはこれが一致した物だけ受ける。
    nonce: String,
    /// Design を始めるたびに増える世代。選んだ時と完了時で違えば、その選択は古い。
    generation: u64,
}

/// 撮っている途中の選択（選んだ順に並ぶ）。
struct PendingPick {
    /// この view の中で選んだ順の番号。
    serial: u64,
    /// 選んだ時の Design の世代とページの世代（完了時に今の値と照合する）。
    design_generation: u64,
    page_generation: u64,
    capture: Box<design::ElementCapture>,
    multi: bool,
    /// 選んだ時点の宛先（Workspace が `PickStarted` を受けて入れる）。
    target: Option<DesignTarget>,
    /// 切り抜きの結果（撮っている間は `None`）。
    result: Option<Result<Vec<u8>, String>>,
}

/// 選択が今の Design・今のページの物でなくなったか（なら捨てる理由）。
fn staleness(pick: &PendingPick, design: Option<u64>, page: u64) -> Option<PickDropReason> {
    if pick.page_generation != page {
        Some(PickDropReason::PageChanged)
    } else if design != Some(pick.design_generation) {
        Some(PickDropReason::DesignEnded)
    } else {
        None
    }
}

/// Web タブの鍵（`EditorTab.path`）。タブ列は path で同一判定・永続化するので、URL（正規形）を
/// そのまま入れる。ファイルの鍵は絶対パス（`/…` / `C:\…`）なので `http://` で始まる鍵と衝突しない。
/// 保存形式（窓セッションの `open_files`）は変えない — 古い版は URL の行を「無いファイル」として飛ばす。
pub(crate) fn web_tab_key(url: &str) -> PathBuf {
    PathBuf::from(url)
}

/// 鍵が Web タブの物ならその URL（許可範囲の正規形）。ファイルの鍵・範囲外の URL は `None`。
pub(crate) fn web_tab_url(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if !(text.starts_with("http://") || text.starts_with("https://")) {
        return None;
    }
    localhost::normalize(text)
}

/// 内蔵の配信で開いた HTML の Web タブの鍵: そのファイルの `file://` URL。ファイルの鍵（絶対パス）とも
/// localhost の Web タブの鍵（`http://`）とも衝突せず、窓セッションの `open_files` にそのまま載る
/// （配信のポートと token は起動ごとに変わるので、鍵にはファイルを使う）。
pub(crate) fn static_tab_key(file: &Path) -> Option<PathBuf> {
    static_server::file_url(file).map(PathBuf::from)
}

/// 鍵が内蔵の配信の Web タブの物なら、その HTML ファイル。
pub(crate) fn static_tab_file(path: &Path) -> Option<PathBuf> {
    static_server::file_of_url(path.to_str()?)
}

/// 内蔵の配信で開いた HTML（localhost の開発サーバなら無い）。
struct StaticSource {
    /// 開いた HTML ファイル（開いた時の綴り）。
    file: PathBuf,
    /// 配っているフォルダの口。このタブが生きている間だけ握る（全部閉じたら配信が止まる）。
    mount: static_server::StaticMount,
}

/// ビューポート幅（WebView の矩形の幅を絞って中央に置く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Viewport {
    /// ペインの幅いっぱい。
    Full,
    /// 指定の幅（CSS px）。ペインより狭い時だけ効く。
    Width(u32),
}

/// ツールバーに並べるビューポート幅（スマホ / タブレット / ノート PC の代表値）。
pub(crate) const VIEWPORT_WIDTHS: [u32; 3] = [375, 768, 1280];

/// タブ名に出す文字数の上限（ページのタイトルは長いことがある）。
const TAB_LABEL_MAX_CHARS: usize = 32;

fn truncate_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_string();
    }
    let mut truncated: String = label.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

/// ズームの段（ブラウザの ⌘+ / ⌘- と同じ刻み）。
const ZOOM_STEPS: [f64; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// 今のズームから `direction`（+1 / -1）へ 1 段動かした値。端では止まる。
fn next_zoom(current: f64, direction: i32) -> f64 {
    if direction > 0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current + 0.001)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current - 0.001)
            .unwrap_or(ZOOM_STEPS[0])
    }
}

/// Web タブ。OS の WebView を載せた [`WebViewView`] を 1 枚抱え、上にツールバーを置く。
/// 編集・保存・LSP・hot exit は関与しない（画像 / PDF タブと同じ表示専用タブ）。
pub(crate) struct WebPreviewView {
    /// 開いた時の URL（localhost はタブの鍵と同じ正規形・内蔵の配信は token 付きの URL）。ページの中で
    /// 移動しても変えない。いま見ている URL は [`Self::current_url`]。
    url: String,
    /// OS の WebView。載せられない環境（Linux）では None でフォールバック表示。
    viewer: Option<Entity<WebViewView>>,
    /// 内蔵の配信で開いた HTML（localhost の開発サーバなら `None`）。
    source: Option<StaticSource>,
    viewport: Viewport,
    design: Option<DesignSession>,
    /// 最後に始めた Design の世代。
    design_generation: u64,
    /// 最上位の文書の読み込みが始まるたびに増える世代（再読込・ページの移動）。
    page_generation: u64,
    /// 撮っている途中の選択（選んだ順）。
    picks: VecDeque<PendingPick>,
    /// 次の選択の番号。
    next_pick: u64,
    /// テスト用: 切り抜きを始めずに完了の口を預かる（完了の順と時機をテストが決める）。
    #[cfg(test)]
    held_snapshots: Option<Rc<std::cell::RefCell<Vec<snapshot::SnapshotDone>>>>,
    /// テスト用: 読み込み直しを頼まれた回数（テスト窓には WebView が無く、読み込みは起きない）。
    #[cfg(test)]
    reload_requests: usize,
    theme: Theme,
    focus_handle: FocusHandle,
    _viewer_subscriptions: Vec<Subscription>,
}

impl EventEmitter<WebPreviewEvent> for WebPreviewView {}

impl WebPreviewView {
    pub(crate) fn new(url: &str, theme: Theme, cx: &mut Context<Self>) -> Self {
        Self::with_source(url, None, theme, cx)
    }

    /// 内蔵の配信で HTML ファイルを開く Web タブ。`mount` はそのファイルのフォルダを配る口で、
    /// このタブが閉じるまで握る。`url` は `mount.url_for(&file)`（呼び側が確かめてから渡す）。
    pub(crate) fn new_static(
        file: PathBuf,
        mount: static_server::StaticMount,
        url: &str,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_source(url, Some(StaticSource { file, mount }), theme, cx)
    }

    fn with_source(
        url: &str,
        source: Option<StaticSource>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Self {
        let viewer = webview_view::is_supported().then(|| {
            let viewer_theme = theme.clone();
            let url = url.to_string();
            cx.new(move |cx| {
                let mut viewer = WebViewView::localhost(&url, viewer_theme, cx);
                // Design Mode のピッカーと IPC は WebView の生成時にしか付けられない。普段は何もしない。
                viewer.add_initialization_script(design::picker_script());
                viewer.enable_ipc();
                viewer
            })
        });
        let mut subscriptions = Vec::new();
        if let Some(viewer) = &viewer {
            // 読み込み・タイトル・失敗でツールバーとタブ名を描き直す。
            subscriptions.push(cx.observe(viewer, |_, _, cx| cx.notify()));
            subscriptions.push(cx.subscribe(viewer, Self::on_viewer_event));
        }
        Self {
            url: url.to_string(),
            viewer,
            source,
            viewport: Viewport::Full,
            design: None,
            design_generation: 0,
            page_generation: 0,
            picks: VecDeque::new(),
            next_pick: 0,
            #[cfg(test)]
            held_snapshots: None,
            #[cfg(test)]
            reload_requests: 0,
            theme,
            focus_handle: cx.focus_handle(),
            _viewer_subscriptions: subscriptions,
        }
    }

    fn on_viewer_event(
        &mut self,
        _viewer: Entity<WebViewView>,
        event: &WebViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            WebViewEvent::PageLoad {
                finished: false, ..
            } => self.page_started(cx),
            WebViewEvent::PageLoad { .. } | WebViewEvent::TitleChanged(_) => cx.notify(),
            WebViewEvent::Message { sender, body } => self.on_design_message(sender, body, cx),
        }
    }

    /// 最上位の文書の読み込みが始まった。読み込み直した文書にはピッカーの状態が無い＝ Design は
    /// 終わり、撮っている途中の選択は古くなる（切り抜きが新しいページの物になりうる）。
    fn page_started(&mut self, cx: &mut Context<Self>) {
        self.page_generation += 1;
        self.design = None;
        self.drop_stale_picks(cx);
        cx.notify();
    }

    /// テスト用: WebView 無しで Design 中の状態にする（テスト窓はネイティブの WebView を作れない）。
    #[cfg(test)]
    pub(crate) fn force_design_for_test(&mut self, cx: &mut Context<Self>) {
        self.begin_design_session(design::new_nonce(), cx);
    }

    /// テスト用: 以後の切り抜きを始めずに、完了の口（呼ぶと撮り終えた扱い）を返す列へ預ける。
    #[cfg(test)]
    pub(crate) fn hold_snapshots_for_test(
        &mut self,
    ) -> Rc<std::cell::RefCell<Vec<snapshot::SnapshotDone>>> {
        self.held_snapshots
            .get_or_insert_with(|| Rc::new(std::cell::RefCell::new(Vec::new())))
            .clone()
    }

    /// テスト用: ページで要素が選ばれた（nonce の検めを通った後と同じ道）。
    #[cfg(test)]
    pub(crate) fn pick_for_test(
        &mut self,
        capture: design::ElementCapture,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        self.accept_pick(Box::new(capture), multi, cx);
    }

    /// テスト用: 最上位の文書の読み込みが始まった（再読込）。
    #[cfg(test)]
    pub(crate) fn page_started_for_test(&mut self, cx: &mut Context<Self>) {
        self.page_started(cx);
    }

    /// テスト用: 読み込み直しを頼まれた回数。
    #[cfg(test)]
    pub(crate) fn reload_requests(&self) -> usize {
        self.reload_requests
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn is_designing(&self) -> bool {
        self.design.is_some()
    }

    /// Design の開始 / 終了（⌘⇧D・ツールバー）。始められなかった（WebView がまだ無い）時は `false`。
    pub(crate) fn toggle_design(&mut self, cx: &mut Context<Self>) -> bool {
        if self.design.is_some() {
            self.stop_design(cx);
            return true;
        }
        self.start_design(cx)
    }

    fn start_design(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(viewer) = self.viewer.clone() else {
            return false;
        };
        // 読み込めていない（開発サーバが起動していない等）ページには選ぶ物が無い。
        if viewer.read(cx).load_failure().is_some() {
            return false;
        }
        let nonce = design::new_nonce();
        if !viewer
            .read(cx)
            .evaluate_script(&design::start_script(&nonce))
        {
            return false;
        }
        self.begin_design_session(nonce, cx);
        // Esc をページの中で受けられるよう、キーを WebView へ渡す。
        viewer.update(cx, |viewer, _| viewer.set_key_focus(true));
        cx.notify();
        true
    }

    /// 新しい世代の Design を始める（前の Design で撮っている途中の選択は古くなる）。
    fn begin_design_session(&mut self, nonce: String, cx: &mut Context<Self>) {
        self.design_generation += 1;
        self.design = Some(DesignSession {
            nonce,
            generation: self.design_generation,
        });
        self.drop_stale_picks(cx);
    }

    pub(crate) fn stop_design(&mut self, cx: &mut Context<Self>) {
        if self.design.take().is_none() {
            return;
        }
        if let Some(viewer) = &self.viewer {
            viewer.read(cx).evaluate_script(design::STOP_SCRIPT);
        }
        self.drop_stale_picks(cx);
        cx.notify();
    }

    /// ページからの知らせ。送り手が localhost の文書・Design 中・nonce 一致・形と大きさが決まりどおりの
    /// 物だけ受ける。
    fn on_design_message(&mut self, sender: &str, body: &str, cx: &mut Context<Self>) {
        let nonce = self.design.as_ref().map(|session| session.nonce.as_str());
        match design::accept_message(nonce, sender, body) {
            Ok(design::DesignMessage::Cancel) => {
                // ページの中で Esc。ピッカーは自分で片付けている。
                self.design = None;
                self.drop_stale_picks(cx);
                cx.notify();
            }
            Ok(design::DesignMessage::Pick { multi, capture }) => {
                self.accept_pick(capture, multi, cx)
            }
            Err(rejected) => {
                // 捨てるだけ（ページが偽の知らせを投げても何も起きない）。理由は開発中だけ出す。
                if cfg!(debug_assertions) {
                    eprintln!("design: ページからの知らせを捨てた: {rejected:?}");
                }
            }
        }
    }

    /// 要素が選ばれた: 宛先を固定してもらい（`PickStarted`）、切り抜きを始める。
    /// Design とページの世代は**この時点の**値を持っておき、撮り終えた時に照合する。
    fn accept_pick(
        &mut self,
        capture: Box<design::ElementCapture>,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.design.as_ref() else {
            return;
        };
        let serial = self.next_pick;
        self.next_pick += 1;
        let rect = capture.element.rect;
        let viewport_width = capture.page.viewport_width;
        self.picks.push_back(PendingPick {
            serial,
            design_generation: session.generation,
            page_generation: self.page_generation,
            capture,
            multi,
            target: None,
            result: None,
        });
        cx.emit(WebPreviewEvent::PickStarted { pick: serial });

        let (sender, receiver) = futures::channel::oneshot::channel();
        let started = self.begin_snapshot(
            &rect,
            viewport_width,
            Box::new(move |result| {
                // 受け手（この view）が先に消えていれば渡す先が無いだけ。
                sender.send(result).ok();
            }),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = match started {
                Ok(()) => receiver
                    .await
                    .unwrap_or_else(|_| Err("the snapshot was dropped".to_string())),
                Err(error) => Err(error),
            };
            #[cfg(debug_assertions)]
            if let Some(delay) = debug_capture_delay() {
                cx.background_executor().timer(delay).await;
            }
            // タブが閉じられて view が消えていれば届け先が無い（開き直したタブには触らない）。
            this.update(cx, |this, cx| this.complete_pick(serial, result, cx))
                .ok();
        })
        .detach();
    }

    /// 切り抜きを始める。`done` は撮り終えた時に 1 回だけ呼ばれる（始められなければ `Err` をすぐ返す）。
    fn begin_snapshot(
        &self,
        rect: &design::Rect,
        viewport_width: f64,
        done: snapshot::SnapshotDone,
        cx: &App,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(held) = &self.held_snapshots {
            held.borrow_mut().push(done);
            return Ok(());
        }
        let Some(viewer) = &self.viewer else {
            return Err("web views are not supported here".to_string());
        };
        viewer.read(cx).capture_element(rect, viewport_width, done)
    }

    /// 宛先を固定する（Workspace が `PickStarted` を受けた時＝選んだ時点で呼ぶ）。捨てた選択なら何もしない。
    pub(crate) fn set_pick_target(&mut self, pick: u64, target: Option<DesignTarget>) {
        if let Some(pending) = self.picks.iter_mut().find(|pending| pending.serial == pick) {
            pending.target = target;
        }
    }

    /// 切り抜きが届いた。捨てた選択（Design を抜けた等）の物なら宛てが無いので何もしない。
    fn complete_pick(
        &mut self,
        serial: u64,
        result: Result<Vec<u8>, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(pick) = self.picks.iter_mut().find(|pick| pick.serial == serial) else {
            return;
        };
        pick.result = Some(result);
        self.flush_picks(cx);
    }

    /// 撮り終えた選択を、先頭から**選んだ順に**知らせる（先の選択が撮り終わるまで後の選択は待つ）。
    fn flush_picks(&mut self, cx: &mut Context<Self>) {
        let mut resume = false;
        while self.picks.front().is_some_and(|pick| pick.result.is_some()) {
            let Some(pick) = self.picks.pop_front() else {
                break;
            };
            // 完了時に照合する: Design もページも選んだ時のままか。
            let current = self.design.as_ref().map(|session| session.generation);
            if let Some(reason) = staleness(&pick, current, self.page_generation) {
                cx.emit(WebPreviewEvent::PickDropped { reason });
                resume = false;
                continue;
            }
            let png = match pick.result {
                Some(Ok(png)) => Some(png),
                Some(Err(error)) => {
                    eprintln!("design: 要素を切り抜けない: {error}");
                    None
                }
                None => None,
            };
            let mut capture = pick.capture;
            // 内蔵の配信のページは、token 付きの URL（起動ごとに変わり、AI には意味が無い）でなく
            // ファイルのパスで渡す（エージェントが直すのはそのファイル）。
            if let Some(file) = self.served_file(&capture.page.url) {
                capture.page.url = file.display().to_string();
            }
            #[cfg(debug_assertions)]
            debug_dump_capture(&capture, png.as_deref());
            let finished = !pick.multi;
            cx.emit(WebPreviewEvent::ElementPicked {
                capture,
                png,
                target: pick.target,
                finished,
            });
            if finished {
                // この選択で Design を終える（後に残った選択は古くなって捨てられる）。
                self.stop_design(cx);
            }
            resume = !finished;
        }
        // ⇧ で続ける途中で、撮っている途中の選択も無ければ次の選択を受ける。
        if resume && self.picks.is_empty() && self.design.is_some() {
            if let Some(viewer) = &self.viewer {
                viewer.read(cx).evaluate_script(design::RESUME_SCRIPT);
            }
        }
        cx.notify();
    }

    /// 今の Design・今のページの物でなくなった選択を捨てて知らせる。完了を待たない — 撮り終わらない
    /// 切り抜きが後の選択を塞がないように。後から届いた完了は宛てが無いので捨てられる。
    fn drop_stale_picks(&mut self, cx: &mut Context<Self>) {
        let current = self.design.as_ref().map(|session| session.generation);
        let page = self.page_generation;
        let mut kept = VecDeque::new();
        for pick in self.picks.drain(..) {
            match staleness(&pick, current, page) {
                None => kept.push_back(pick),
                Some(reason) => cx.emit(WebPreviewEvent::PickDropped { reason }),
            }
        }
        self.picks = kept;
    }

    /// 開いた時の URL。
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// 内蔵の配信で開いた HTML ファイル（localhost の開発サーバなら `None`）。
    pub(crate) fn static_file(&self) -> Option<&Path> {
        self.source.as_ref().map(|source| source.file.as_path())
    }

    /// 内蔵の配信で配っているフォルダ（ファイル監視のパスと比べる綴り: 実体と、開いた時の綴り）。
    pub(crate) fn static_roots(&self) -> Vec<PathBuf> {
        let Some(source) = &self.source else {
            return Vec::new();
        };
        let mut roots = vec![source.mount.root().to_path_buf()];
        if let Some(parent) = source.file.parent() {
            if parent != source.mount.root() {
                roots.push(parent.to_path_buf());
            }
        }
        roots
    }

    /// 短い表記: localhost は `localhost:5173/app`、内蔵の配信は `site/about.html`
    /// （配っているフォルダの名前 + 相対パス。token 付きの URL は出さない）。
    fn display_label(&self, url: &str) -> String {
        let Some(source) = &self.source else {
            return localhost::display_label(url);
        };
        let folder = source
            .mount
            .root()
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        match source.mount.relative_path_of(url) {
            Some(relative) if folder.is_empty() => relative,
            Some(relative) => format!("{folder}/{relative}"),
            None => localhost::display_label(url),
        }
    }

    /// 内蔵の配信のページ（URL）が指すディスクの上のファイル（localhost・範囲外なら `None`）。
    /// 綴りは開いた時のフォルダに合わせる（エクスプローラで見えている綴り・Windows の `\\?\` を出さない）。
    fn served_file(&self, url: &str) -> Option<PathBuf> {
        let source = self.source.as_ref()?;
        let relative = source.mount.relative_path_of(url)?;
        let folder = source.file.parent().unwrap_or(source.mount.root());
        Some(folder.join(relative))
    }

    /// URL 欄のツールチップ: localhost は URL、内蔵の配信はディスクの上のファイル。
    fn url_tooltip(&self, url: &str) -> String {
        let Some(source) = &self.source else {
            return url.to_string();
        };
        let file = self.served_file(url).unwrap_or_else(|| source.file.clone());
        i18n::t!("webtab.static_tip", "path" => file.display())
    }

    /// いま見ている URL（ページの中の移動に追従）。WebView が無ければ開いた時の URL。
    pub(crate) fn current_url(&self, cx: &App) -> String {
        self.viewer
            .as_ref()
            .and_then(|viewer| viewer.read(cx).current_url().map(str::to_string))
            .unwrap_or_else(|| self.url.clone())
    }

    /// タブ名: 文書のタイトルがあればそれ、無ければ `localhost:5173/app` の短い表記。
    /// タブ列の幅を 1 枚で食わないよう [`TAB_LABEL_MAX_CHARS`] 文字で切る。
    pub(crate) fn tab_label(&self, cx: &App) -> String {
        let label = self
            .viewer
            .as_ref()
            .and_then(|viewer| viewer.read(cx).page_title().map(str::to_string))
            .unwrap_or_else(|| self.display_label(&self.current_url(cx)));
        truncate_label(&label, TAB_LABEL_MAX_CHARS)
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_theme(theme));
        }
        cx.notify();
    }

    /// 親レイアウト上の可視性をネイティブ子ビューへ同期する（HTML プレビュー / PDF と同じ規律）。
    pub(crate) fn set_surface_active(&mut self, active: bool, focus: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| viewer.set_active(active, focus, cx));
        }
    }

    /// OS のキーボードフォーカスを WebView に渡す / GPUI へ返す（表示状態は変えない）。
    pub(crate) fn set_key_focus(&mut self, owns: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_key_focus(owns));
        }
    }

    /// 設定 `html_preview_evict_minutes` を中継する（非表示 WebView の回収弁は HTML / PDF と共用）。
    pub(crate) fn set_evict_minutes(&mut self, minutes: u64, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_evict_minutes(minutes));
        }
    }

    pub(crate) fn has_native_viewer(&self) -> bool {
        self.viewer.is_some()
    }

    /// WebView を出してよい状態として同期されているか（オーバーレイ中は false・テストの観測点）。
    #[cfg(test)]
    pub(crate) fn is_surface_active(&self, cx: &App) -> bool {
        self.viewer
            .as_ref()
            .is_some_and(|viewer| viewer.read(cx).is_active())
    }

    /// WebView がキーボードフォーカスを持つ想定か（要求の記録・テストの観測点）。
    #[cfg(test)]
    pub(crate) fn wants_key_focus(&self, cx: &App) -> bool {
        self.viewer
            .as_ref()
            .is_some_and(|viewer| viewer.read(cx).wants_key_focus())
    }

    pub(crate) fn set_viewport(&mut self, viewport: Viewport, cx: &mut Context<Self>) {
        if self.viewport != viewport {
            self.viewport = viewport;
            cx.notify();
        }
    }

    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.reload_requests += 1;
        }
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| {
                viewer.reload();
                cx.notify();
            });
        }
    }

    fn zoom(&self, cx: &App) -> f64 {
        self.viewer
            .as_ref()
            .map_or(1.0, |viewer| viewer.read(cx).zoom())
    }

    /// ズームを 1 段動かす（`direction` = +1 / -1）。0 で 100% に戻す。
    pub(crate) fn step_zoom(&mut self, direction: i32, cx: &mut Context<Self>) {
        let next = if direction == 0 {
            1.0
        } else {
            next_zoom(self.zoom(cx), direction)
        };
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| {
                viewer.set_zoom(next);
                cx.notify();
            });
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.go_back());
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.go_forward());
        }
    }

    fn open_devtools(&self, cx: &App) {
        if let Some(viewer) = &self.viewer {
            viewer.read(cx).open_devtools();
        }
    }

    /// いま見ている画面を既定のブラウザで開く。
    fn open_in_browser(&self, cx: &App) {
        localhost::open_in_browser(&self.current_url(cx));
    }

    /// 開発用: 読み込まれたページの中身をスクリプトで読み出して標準エラーへ出す
    /// （WKWebView の中身は offscreen の画像に写らないので、ページ側はこれで確かめる）。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_evaluate(&self, label: String, script: &str, cx: &App) {
        let Some(viewer) = &self.viewer else {
            eprintln!("WEB_PREVIEW_PROBE {label}: WebView が無い");
            return;
        };
        let ran = viewer
            .read(cx)
            .evaluate_script_with_callback(script, move |result| {
                eprintln!("WEB_PREVIEW_PROBE {label}: {result}");
            });
        if !ran {
            eprintln!("WEB_PREVIEW_PROBE: WebView がまだ作られていない（表示前）");
        }
    }

    /// 開発用: ピッカーの debug 口（`__necoderDesignDebug`）でホバー / 選択を起こす。
    /// 実際のクリックと同じ道（切り抜き → composer）を通る。debug ビルドのみ。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_design(&self, action: &str, selector: &str, cx: &App) {
        let Some(viewer) = &self.viewer else {
            return;
        };
        let selector = serde_json::Value::String(selector.to_string()).to_string();
        let script = match action {
            "hover" => format!("window.__necoderDesignDebug.hover({selector})"),
            "pick" => format!("window.__necoderDesignDebug.pick({selector}, false)"),
            _ => format!("window.__necoderDesignDebug.pick({selector}, true)"),
        };
        let label = format!("design {action} {selector}");
        viewer
            .read(cx)
            .evaluate_script_with_callback(&script, move |result| {
                eprintln!("WEB_PREVIEW_PROBE {label}: {result}");
            });
    }

    /// 開発用: 見えている範囲をまるごと撮って PNG に書く（ホバー枠がページの上に出ているかを見る）。
    #[cfg(debug_assertions)]
    pub(crate) fn debug_snapshot_view(&self, path: PathBuf, cx: &App) {
        let Some(viewer) = &self.viewer else {
            return;
        };
        let started = viewer
            .read(cx)
            .capture_visible(Box::new(move |result| match result {
                Ok(png) => match std::fs::write(&path, png) {
                    Ok(()) => eprintln!("WEB_PREVIEW_PROBE snapshot: {}", path.display()),
                    Err(error) => eprintln!("WEB_PREVIEW_PROBE snapshot: 書けない: {error}"),
                },
                Err(error) => eprintln!("WEB_PREVIEW_PROBE snapshot: 撮れない: {error}"),
            }));
        if let Err(error) = started {
            eprintln!("WEB_PREVIEW_PROBE snapshot: 撮り始められない: {error}");
        }
    }

    /// ツールバーの四角いアイコンボタン（19px・hover で bg2）。
    fn icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        enabled: bool,
        tip: String,
    ) -> Stateful<Div> {
        let theme = &self.theme;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(19.))
            .rounded(px(5.))
            .when(enabled, |button| {
                button.cursor_pointer().hover(|style| style.bg(theme.bg2))
            })
            .child(
                svg()
                    .path(icon)
                    .size(px(12.))
                    .flex_none()
                    .text_color(if enabled {
                        theme.fg1
                    } else {
                        theme.fg2.alpha(0.5)
                    }),
            )
            .tooltip(Tooltip::text(tip, theme.clone()))
    }

    /// 文字のチップ（ビューポート幅・ズーム率）。選択中は中立の面 bg3 + fg0。
    fn text_chip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        selected: bool,
    ) -> Stateful<Div> {
        let theme = &self.theme;
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .h(px(19.))
            .px(px(6.))
            .rounded(px(5.))
            .text_size(px(10.5))
            .cursor_pointer()
            .text_color(if selected { theme.fg0 } else { theme.fg2 })
            .when(selected, |chip| chip.bg(theme.bg3))
            .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
            .child(label)
    }

    fn separator(&self) -> Div {
        div()
            .flex_none()
            .w(px(1.))
            .h(px(12.))
            .mx(px(4.))
            .bg(self.theme.border)
    }

    /// エディタのパンくずの位置（26px）に置くツールバー。
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let (can_back, can_forward, loading) = self
            .viewer
            .as_ref()
            .map(|viewer| {
                let viewer = viewer.read(cx);
                (
                    viewer.can_go_back(),
                    viewer.can_go_forward(),
                    viewer.is_loading(),
                )
            })
            .unwrap_or((false, false, false));
        let current = self.current_url(cx);
        let zoom = self.zoom(cx);
        let designing = self.design.is_some();
        let mut viewport_chips = vec![self
            .text_chip(
                "web-viewport-full",
                i18n::t!("webtab.viewport_full"),
                self.viewport == Viewport::Full,
            )
            .tooltip(Tooltip::text(
                i18n::t!("webtab.viewport_full_tip"),
                theme.clone(),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    cx.stop_propagation();
                    this.set_viewport(Viewport::Full, cx);
                }),
            )];
        for width in VIEWPORT_WIDTHS {
            viewport_chips.push(
                self.text_chip(
                    ("web-viewport", width as usize),
                    width.to_string(),
                    self.viewport == Viewport::Width(width),
                )
                .tooltip(Tooltip::text(
                    i18n::t!("webtab.viewport_tip", "width" => width),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        cx.stop_propagation();
                        this.set_viewport(Viewport::Width(width), cx);
                    }),
                ),
            );
        }
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(BREADCRUMB_HEIGHT))
            .px(px(8.))
            .flex_none()
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg2)
            .child(
                self.icon_button(
                    "web-back",
                    "icons/arrow-left.svg",
                    can_back,
                    i18n::t!("webtab.back_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.go_back(cx);
                    }),
                ),
            )
            .child(
                self.icon_button(
                    "web-forward",
                    "icons/arrow-right.svg",
                    can_forward,
                    i18n::t!("webtab.forward_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.go_forward(cx);
                    }),
                ),
            )
            .child(
                self.icon_button(
                    "web-reload",
                    "icons/refresh-cw.svg",
                    true,
                    i18n::t!("webtab.reload_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.reload(cx);
                    }),
                ),
            )
            // URL は表示だけ（任意の URL を打つ欄にはしない）。Design 中は操作の案内に替える。
            .child(
                div()
                    .id("web-url")
                    .flex_1()
                    .min_w_0()
                    .ml(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(if designing { theme.fg0 } else { theme.fg1 })
                    .child(SharedString::from(if designing {
                        i18n::t!("design.hint")
                    } else if loading {
                        format!("{} …", self.display_label(&current))
                    } else {
                        self.display_label(&current)
                    }))
                    .tooltip(Tooltip::text(self.url_tooltip(&current), theme.clone())),
            )
            .child(
                self.text_chip("web-zoom-out", "−".to_string(), false)
                    .tooltip(Tooltip::text(
                        i18n::t!("webtab.zoom_out_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.step_zoom(-1, cx);
                        }),
                    ),
            )
            .child(
                self.text_chip(
                    "web-zoom-reset",
                    format!("{}%", (zoom * 100.0).round() as i64),
                    false,
                )
                .tooltip(Tooltip::text(
                    i18n::t!("webtab.zoom_reset_tip"),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.step_zoom(0, cx);
                    }),
                ),
            )
            .child(
                self.text_chip("web-zoom-in", "+".to_string(), false)
                    .tooltip(Tooltip::text(i18n::t!("webtab.zoom_in_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.step_zoom(1, cx);
                        }),
                    ),
            )
            .child(self.separator())
            .children(viewport_chips)
            .child(self.separator())
            .child(
                div()
                    .id("web-design")
                    .debug_selector(|| "web-design".to_string())
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .h(px(19.))
                    .px(px(6.))
                    .rounded(px(5.))
                    .text_size(px(10.5))
                    .cursor_pointer()
                    .text_color(if designing { theme.fg0 } else { theme.fg1 })
                    .when(designing, |chip| chip.bg(theme.bg3))
                    .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                    .child(
                        svg()
                            .path("icons/mouse-pointer-click.svg")
                            .size(px(12.))
                            .flex_none()
                            .text_color(if designing { theme.fg0 } else { theme.fg1 }),
                    )
                    .child(SharedString::from(i18n::t!("design.toggle")))
                    .tooltip(Tooltip::text(
                        format!(
                            "{}  {}",
                            i18n::t!("design.toggle_tip"),
                            keymap_core::keystroke_label("cmd-shift-d")
                        ),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _, window, cx| {
                            cx.stop_propagation();
                            // ⌘⇧D と同じ入口を通す（始められない時の案内もそちらが出す）。
                            window.dispatch_action(Box::new(ToggleDesignMode), cx);
                        }),
                    ),
            )
            .child(
                self.icon_button(
                    "web-devtools",
                    "icons/code-xml.svg",
                    self.viewer.is_some(),
                    i18n::t!("webtab.devtools_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.open_devtools(cx);
                    }),
                ),
            )
            .child(
                self.icon_button(
                    "web-open-browser",
                    "icons/external-link.svg",
                    true,
                    i18n::t!("webtab.open_browser_tip"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.open_in_browser(cx);
                    }),
                ),
            )
    }
}

/// 開発用: `NECODER_DESIGN_CAPTURE_DELAY_MS=<ms>` で切り抜きの完了を遅らせる（撮っている間に
/// スレッドを替える・再読込する、を実画面で確かめるため・台帳 R05）。debug ビルドのみ。
#[cfg(debug_assertions)]
fn debug_capture_delay() -> Option<std::time::Duration> {
    std::env::var("NECODER_DESIGN_CAPTURE_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
}

/// 開発用: `NECODER_DESIGN_CAPTURE_DIR=<dir>` を置くと、選んだ要素の切り抜き（PNG）と composer へ入れる
/// 文章を `element-<n>.png` / `element-<n>.md` に書き出す（WKWebView の中身は offscreen の画像に
/// 写らないので、切り抜きが正しいかはこれで目で確かめる）。debug ビルドのみ。
#[cfg(debug_assertions)]
fn debug_dump_capture(capture: &design::ElementCapture, png: Option<&[u8]>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let Some(directory) = std::env::var_os("NECODER_DESIGN_CAPTURE_DIR").map(PathBuf::from) else {
        return;
    };
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let base = directory.join(format!("element-{serial}"));
    if let Some(png) = png {
        if let Err(error) = std::fs::write(base.with_extension("png"), png) {
            eprintln!("DESIGN_CAPTURE: PNG を書けない: {error}");
        }
    }
    if let Err(error) = std::fs::write(
        base.with_extension("md"),
        design::format_for_prompt(capture),
    ) {
        eprintln!("DESIGN_CAPTURE: 文章を書けない: {error}");
    }
    eprintln!(
        "DESIGN_CAPTURE: {} ({} bytes png)",
        base.display(),
        png.map_or(0, <[u8]>::len)
    );
}

impl Focusable for WebPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WebPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let body = match self.viewer.clone() {
            // Linux（WebView 非対応）: 理由と代わりの手段を出す（黙って空にしない）。
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("webtab.unsupported")))
                .child(
                    div()
                        .text_size(px(10.))
                        .child(SharedString::from(self.url.clone())),
                )
                .into_any_element(),
            Some(viewer) => {
                let full = self.viewport == Viewport::Full;
                // 幅を絞った時は WebView の矩形を中央に置き、左右は bg0 の余白にする
                // （ネイティブの矩形はこの div の大きさで決まる＝webview_view の canvas）。
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .when(!full, |stage| stage.bg(theme.bg0))
                    .child(
                        div()
                            .h_full()
                            .map(|frame| match self.viewport {
                                Viewport::Full => frame.w_full(),
                                Viewport::Width(width) => frame
                                    .w(px(width as f32))
                                    .max_w_full()
                                    .border_x_1()
                                    .border_color(theme.border),
                            })
                            .child(viewer.cached(StyleRefinement::default().size_full())),
                    )
                    .into_any_element()
            }
        };
        div()
            .key_context("WebPreview")
            .track_focus(&self.focus_handle(cx))
            // Design 中に GPUI 側（ツールバー等）へキーが来ている時の Esc。ページの中の Esc は
            // ピッカーが受けて知らせてくる。
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "escape" && this.design.is_some() {
                    cx.stop_propagation();
                    this.stop_design(cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .child(self.render_toolbar(cx))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(selector: &str) -> design::ElementCapture {
        serde_json::from_value(serde_json::json!({
            "page": {"url": "http://localhost:5173/", "title": "App", "viewport_width": 800.0,
                     "viewport_height": 600.0, "device_pixel_ratio": 2.0},
            "element": {"tag": "div", "selector": selector, "path": selector, "text": "",
                        "nearby_text": [], "html": "<div></div>", "role": null,
                        "accessible_name": null, "attributes": [],
                        "rect": {"x": 0.0, "y": 0.0, "width": 10.0, "height": 10.0},
                        "styles": {}, "components": [], "source": null}
        }))
        .expect("形どおり")
    }

    /// 台帳 R05: 切り抜きが逆の順に撮り終わっても、要素と画像の組は崩れず、選んだ順に知らせる。
    /// ⇧ で続ける選択は Design を終えず、最後の選択で終わる。
    #[gpui::test]
    fn picks_keep_their_own_image_and_the_selection_order(cx: &mut gpui::TestAppContext) {
        ::webview_view::disable_native_webviews_for_tests();
        let view = cx.new(|cx| WebPreviewView::new("http://localhost:5173/", Theme::dark(), cx));
        let seen: Rc<std::cell::RefCell<Vec<(String, Option<Vec<u8>>, bool)>>> =
            Rc::new(std::cell::RefCell::new(Vec::new()));
        let _subscription = cx.update(|cx| {
            let seen = seen.clone();
            cx.subscribe(&view, move |_view, event: &WebPreviewEvent, _cx| {
                if let WebPreviewEvent::ElementPicked {
                    capture,
                    png,
                    finished,
                    ..
                } = event
                {
                    seen.borrow_mut().push((
                        capture.element.selector.clone(),
                        png.clone(),
                        *finished,
                    ));
                }
            })
        });
        let held = view.update(cx, |view, cx| {
            view.force_design_for_test(cx);
            let held = view.hold_snapshots_for_test();
            view.pick_for_test(element("#first"), true, cx);
            view.pick_for_test(element("#second"), true, cx);
            view.pick_for_test(element("#third"), false, cx);
            held
        });
        cx.run_until_parked();
        // 撮り終わる順は 3 → 1 → 2。画像の中身で組を見分ける。
        let third = held.borrow_mut().remove(2);
        third(Ok(vec![3]));
        cx.run_until_parked();
        assert!(seen.borrow().is_empty(), "先の選択が撮り終わるまで出さない");
        let first = held.borrow_mut().remove(0);
        first(Ok(vec![1]));
        cx.run_until_parked();
        let second = held.borrow_mut().remove(0);
        second(Ok(vec![2]));
        cx.run_until_parked();
        assert_eq!(
            *seen.borrow(),
            vec![
                ("#first".to_string(), Some(vec![1]), false),
                ("#second".to_string(), Some(vec![2]), false),
                ("#third".to_string(), Some(vec![3]), true),
            ]
        );
        assert!(!view.read_with(cx, |view, _cx| view.is_designing()));
    }

    #[test]
    fn web_tab_keys_round_trip_and_never_match_files() {
        let key = web_tab_key("http://localhost:5173/");
        assert_eq!(web_tab_url(&key).as_deref(), Some("http://localhost:5173/"));
        assert_eq!(web_tab_url(Path::new("/tmp/project/index.html")), None);
        assert_eq!(web_tab_url(Path::new("C:\\work\\index.html")), None);
        // 保存形式に紛れ込んだ範囲外の URL は Web タブとして開かない。
        assert_eq!(web_tab_url(Path::new("https://example.com/")), None);
    }

    #[test]
    fn long_titles_are_cut_for_the_tab() {
        assert_eq!(truncate_label("Vite App", 32), "Vite App");
        let long = "あ".repeat(40);
        let cut = truncate_label(&long, 32);
        assert_eq!(cut.chars().count(), 32);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn zoom_steps_like_a_browser_and_stop_at_the_ends() {
        assert_eq!(next_zoom(1.0, 1), 1.1);
        assert_eq!(next_zoom(1.0, -1), 0.9);
        assert_eq!(next_zoom(2.0, 1), 2.0);
        assert_eq!(next_zoom(0.5, -1), 0.5);
        // 段の間の値からは近い段へ寄る。
        assert_eq!(next_zoom(1.05, 1), 1.1);
        assert_eq!(next_zoom(1.05, -1), 1.0);
    }
}
