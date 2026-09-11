//! pdf_view — PDF ファイルのタブ表示（FEATURES §2「Markdown/画像プレビュー」の PDF 側）。
//!
//! Pane/Item 多態化（ARCHITECTURE §3）の第3の具体型。**自前の PDF レンダラは積まない** —
//! OS が持つビューア（macOS = WKWebView 内の PDFKit / Windows = WebView2 の PDF ビューア）に
//! `file://` を渡して描かせる。necoder 側はタブの器とフォーカスの受け皿だけを持つ。
//!
//! なぜ WebView かというと、`crates/webview_view` に「ネイティブ子ビューを GPUI の矩形へ載せ、
//! 非表示中は破棄してメモリを返す」層が HTML プレビュー用に既に在るため。PDF はその層の
//! 2 番目の利用者で、拡張子判定と器以外は新規コードが要らない（pdfium 同梱は idle メモリ予算と
//! 配布サイズに効くので採らない）。
//!
//! remote (SSH) の PDF はネイティブ WebView が開けない（OS 子ビューはローカルパスしか読まない）ので、
//! Host 経由で読んだバイト列をキャッシュへ落として**そのローカル複製**を見せる。複製はタブが持ち、
//! タブを閉じた時（`Drop`）に消す。ディスク側が変わったら再取得して同じ複製を書き換える（`set_bytes`）。
//!
//! 制約（意図的な非対応。フォールバックカードを出して黙って失敗しない）:
//! - Linux: `webview_view::is_supported()` が false（WebKitGTK を引かない判断・DECISIONS）
//!
//! 色は識別に集約（UI-SPEC）: 器は bg1、フォールバック文言は fg2 のみ。

use crate::workspace::*;
use webview_view::WebViewView;

/// PDF としてタブ表示する拡張子か。非対応なら None＝テキストとして開く既存経路へ。
pub(crate) fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// remote の PDF をローカルへ落とした複製。**タブが所有し、drop で消す**（残すとキャッシュが
/// 際限なく太り、消し忘れた PDF が次の起動でも読める＝意図しない持ち出しになる）。
struct CachedRemotePdf {
    path: PathBuf,
}

impl CachedRemotePdf {
    /// `<cache>/remote-pdf/<pid>-<連番>-<元のファイル名>` へ書く。ファイル名は拡張子を保つ
    /// （WebView は拡張子と Content-Type で PDF と判断するため `.pdf` を落とせない）。
    fn write(source: &Path, bytes: &[u8]) -> Option<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SERIAL: AtomicU64 = AtomicU64::new(0);

        let directory = paths::cache_dir()?.join("remote-pdf");
        std::fs::create_dir_all(&directory).ok()?;
        let name = source
            .file_name()
            .unwrap_or_else(|| "document.pdf".as_ref());
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "{}-{serial}-{}",
            std::process::id(),
            name.to_string_lossy()
        ));
        std::fs::write(&path, bytes).ok()?;
        Some(Self { path })
    }

    /// 同じ複製を上書きする（ビューアが開いている URL を変えずに中身だけ差し替える）。
    fn rewrite(&self, bytes: &[u8]) -> bool {
        std::fs::write(&self.path, bytes).is_ok()
    }
}

impl Drop for CachedRemotePdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path); // 消せなくても落とさない（次回起動時に pid で判別可能）
    }
}

/// PDF タブ。OS のビューアを載せた [`WebViewView`] を 1 枚抱えるだけの器。
/// 編集・保存・LSP・hot exit は一切関与しない（画像タブと同じ扱い）。
pub(crate) struct PdfView {
    path: PathBuf,
    /// OS ビューアの実体。載せられない環境（Linux）では None でフォールバック表示。
    viewer: Option<Entity<WebViewView>>,
    /// remote の PDF を見せているときのローカル複製（local なら None）。
    cached: Option<CachedRemotePdf>,
    theme: Theme,
    focus_handle: FocusHandle,
}

impl PdfView {
    /// `bytes` は remote の時だけ使う（ローカル複製を作る材料）。local はビューアが元のファイルを
    /// 直接読むので捨てる — タブが開いている間ずっとバイト列を抱え込まないため。
    pub(crate) fn new(
        path: &Path,
        remote: bool,
        bytes: Vec<u8>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Self {
        let cached = remote
            .then(|| CachedRemotePdf::write(path, &bytes))
            .flatten();
        // remote で複製を作れなかった（キャッシュに書けない）＝ビューアを載せない。
        // フォールバックカードが出るので、白紙のビューアを見せるより理由が伝わる。
        let source = match (remote, &cached) {
            (false, _) => Some(path.to_path_buf()),
            (true, Some(cached)) => Some(cached.path.clone()),
            (true, None) => None,
        };
        let viewer = source
            .filter(|_| webview_view::is_supported())
            .map(|source| {
                let viewer_theme = theme.clone();
                cx.new(move |_| WebViewView::local_file(source, viewer_theme))
            });
        Self {
            path: path.to_path_buf(),
            viewer,
            cached,
            theme,
            focus_handle: cx.focus_handle(),
        }
    }

    /// remote の PDF を見せているタブか（外部変更時に再取得が要るかの判定）。
    pub(crate) fn is_remote_backed(&self) -> bool {
        self.cached.is_some()
    }

    /// ディスクの新しい内容で remote の複製を差し替えて再読込する（`project_watcher` から）。
    pub(crate) fn set_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(cached) = &self.cached else {
            return;
        };
        if cached.rewrite(bytes) {
            self.reload(cx);
        }
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_theme(theme));
        }
    }

    /// 親レイアウト上の可視性をネイティブ子ビューへ同期する（HTML プレビューと同じ規律）。
    /// GPUI の描画木から外れても OS 子ビューは残るため、**タブが隠れたら必ず false を渡す**。
    pub(crate) fn set_surface_active(&mut self, active: bool, focus: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, cx| viewer.set_active(active, focus, cx));
        }
    }

    /// OS のキーボードフォーカスを PDF ビューアに渡す / GPUI へ返す（表示状態は変えない）。
    /// 返し忘れると隠れた WebView がキー入力と IME を吸い続ける（webview_view のモジュール注記）。
    pub(crate) fn set_key_focus(&mut self, owns: bool, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_key_focus(owns));
        }
    }

    /// 設定 `html_preview_evict_minutes` を中継する（`0` = 自動破棄しない）。
    /// 非表示 WebView の回収弁は HTML プレビューと共用＝PDF だけ別予算を持たせない。
    pub(crate) fn set_evict_minutes(&mut self, minutes: u64, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.set_evict_minutes(minutes));
        }
    }

    /// ディスク上の PDF が差し替わった時の再読込（project_watcher から）。
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = &self.viewer {
            viewer.update(cx, |viewer, _| viewer.reload());
        }
    }

    /// ネイティブ WebView を載せられるタブか（可視性同期の対象判定）。
    pub(crate) fn has_native_viewer(&self) -> bool {
        self.viewer.is_some()
    }
}

impl Focusable for PdfView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PdfView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let container = div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(self.theme.bg1);
        let Some(viewer) = self.viewer.clone() else {
            // Linux（WebView 非対応）か、remote 複製をキャッシュに書けなかった場合。
            // 開けない理由と、代わりの手段（既定アプリ）を必ず出す。
            return container
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .text_color(self.theme.fg2)
                .child(SharedString::from(i18n::t!("pdf.unsupported")))
                .child(
                    div()
                        .text_size(px(11.))
                        .child(SharedString::from(i18n::t!("pdf.unsupported_hint"))),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .child(self.path.to_string_lossy().to_string()),
                );
        };
        container.child(viewer.cached(StyleRefinement::default().size_full()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// remote の PDF はローカル複製を見せ、**タブを閉じたら複製が消える**こと。
    /// 残すとキャッシュが太り、リモートから持ち出した中身が手元に居座る。
    #[test]
    fn remote_pdf_is_cached_locally_and_removed_on_drop() {
        let bytes = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n%%EOF\n";
        let cached =
            CachedRemotePdf::write(Path::new("/srv/papers/spec.pdf"), bytes).expect("複製を書ける");
        let path = cached.path.clone();
        assert!(path.exists(), "複製が置かれる");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "中身がそのまま");
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("pdf"),
            "拡張子を保つ（WebView が PDF と判断する手がかり）"
        );

        assert!(
            cached.rewrite(b"%PDF-1.7\nnew\n"),
            "同じ複製を書き換えられる"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7\nnew\n");

        drop(cached);
        assert!(!path.exists(), "タブを閉じたら複製は消える");
    }

    #[test]
    fn pdf_extension_is_matched_case_insensitively() {
        assert!(is_pdf_path(Path::new("/tmp/a.pdf")));
        assert!(is_pdf_path(Path::new("/tmp/A.PDF")));
        assert!(!is_pdf_path(Path::new("/tmp/a.pdfx")));
        assert!(!is_pdf_path(Path::new("/tmp/pdf")));
        assert!(!is_pdf_path(Path::new("/tmp/a.md")));
    }
}
