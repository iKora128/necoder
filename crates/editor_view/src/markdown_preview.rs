//! markdown_preview — `.md` の整形プレビュー（source ⇄ rendered トグルの rendered 側・⌘⇧V）。
//!
//! パースは共有 [`markdown`] crate（[model] 層・GPUI 非依存）、描画（Block→GPUI）だけをここに置く。
//! `agent_panel` の transcript レンダラ（`render_streaming_markdown`）と**視覚語彙を揃える**が、あちらは
//! 選択リージョン機構（M13）と結合するためコード共有はせず、閲覧特化の軽量版を自作する（見た目トークン
//! だけ踏襲＝自作コードの再利用でライセンス上の含みは無い）。表・画像・引用装飾は後続。
//!
//! 設計原則: 色は識別に集約する（UI-SPEC）。見出しの階層は**サイズと太さだけ**で示し、色や罫線の装飾は
//! 足さない。色が付くのは syn-* トークン（コードブロック）・インラインコード/リンクのみ。

use gpui::{
    div, img, prelude::*, px, AnyElement, FontStyle, FontWeight, HighlightStyle, ImageSource,
    ObjectFit, ScrollHandle, SharedString, StrikethroughStyle, StyledText, UnderlineStyle,
};
use std::ops::Range;
use std::path::{Path, PathBuf};
use theme_core::Theme;

/// 見出しレベル→文字サイズ（閲覧用に chat より大きめ・階層は size と weight だけで示す）。
fn heading_size(level: u8) -> f32 {
    match level {
        1 => 24.0,
        2 => 19.0,
        3 => 16.5,
        4 => 15.0,
        _ => 14.0,
    }
}

/// ブロック列を縦スクロールの読み物として組む。`scroll` は呼び出し側（EditorView）が保持する
/// 別系統のスクロール位置（EditorElement の縦スクロールとは独立）。
/// `base_dir` は相対パス画像の解決基準（= `.md` の親ディレクトリ。無題バッファは None）。
pub(crate) fn render_preview(
    blocks: &[markdown::Block],
    theme: &Theme,
    font_size: f32,
    scroll: &ScrollHandle,
    base_dir: Option<&Path>,
) -> AnyElement {
    let prose = font_size + 1.0; // 本文は code より僅かに大きく（読み物として）
    div()
        .id("markdown-preview")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll)
        .px(px(32.))
        .py(px(22.))
        .child(
            // 行長を抑えて可読性を上げる（左寄せ・センタリングはしない）。
            div()
                .max_w(px(860.))
                .flex()
                .flex_col()
                .gap(px(11.))
                .text_size(px(prose))
                .line_height(px(prose * 1.65))
                .text_color(theme.fg0)
                .children(
                    blocks
                        .iter()
                        .cloned()
                        .map(|block| render_block(block, theme, font_size, base_dir)),
                ),
        )
        .into_any_element()
}

/// 1 ブロックを GPUI 要素へ。`render_markdown`（agent_panel）と同じトークンを使う。
fn render_block(
    block: markdown::Block,
    theme: &Theme,
    font_size: f32,
    base_dir: Option<&Path>,
) -> AnyElement {
    match block {
        markdown::Block::Heading { level, text, spans } => div()
            .when(level <= 2, |element| element.pt(px(6.)))
            .text_size(px(heading_size(level)))
            .font_weight(if level == 1 {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            })
            .text_color(theme.fg0)
            .child(styled_text(text, md_highlights(theme, &spans)))
            .into_any_element(),
        markdown::Block::Paragraph { text, spans } => div()
            .child(styled_text(text, md_highlights(theme, &spans)))
            .into_any_element(),
        markdown::Block::Code { lang, text } => {
            let language = lang.as_deref().and_then(lang::LanguageId::from_name);
            let highlights = code_highlights(theme, language, &text);
            div()
                .flex()
                .flex_col()
                .my(px(2.))
                .rounded(px(6.))
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .px(px(11.))
                .py(px(9.))
                .font_family("Guguru Sans Code")
                .text_size(px(font_size))
                .line_height(px(font_size * 1.5))
                .text_color(theme.fg0)
                .child(styled_text(text, highlights))
                .into_any_element()
        }
        markdown::Block::ListItem {
            depth,
            marker,
            text,
            spans,
        } => {
            let bullet: SharedString = match marker {
                markdown::ListMarker::Bullet => "•".into(),
                markdown::ListMarker::Ordered(number) => format!("{number}.").into(),
                markdown::ListMarker::Task(true) => "☑".into(),
                markdown::ListMarker::Task(false) => "☐".into(),
            };
            div()
                .flex()
                .gap(px(8.))
                .pl(px(2.0 + depth as f32 * 20.0))
                .child(div().flex_none().text_color(theme.fg2).child(bullet))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(styled_text(text, md_highlights(theme, &spans))),
                )
                .into_any_element()
        }
        markdown::Block::Rule => div()
            .my(px(6.))
            .h(px(1.))
            .bg(theme.border)
            .into_any_element(),
        markdown::Block::Image { source, alt } => render_image(source, alt, theme, base_dir),
    }
}

/// 画像ブロック。ローカルは base_dir 基準で解決したパス、http(s) は URI をそのまま `img()` へ
/// （URI は既定 NullHttpClient だと読み込み失敗 → fallback で alt を出す＝安全に劣化）。
/// 解決できない相対パス（無題バッファ・remote の .md）も alt テキストへフォールバックする。
fn render_image(source: String, alt: String, theme: &Theme, base_dir: Option<&Path>) -> AnyElement {
    let image_source: Option<ImageSource> =
        if source.starts_with("http://") || source.starts_with("https://") {
            Some(source.clone().into())
        } else {
            let path = Path::new(&source);
            let resolved: Option<PathBuf> = if path.is_absolute() {
                Some(path.to_path_buf())
            } else {
                base_dir.map(|base| base.join(path))
            };
            resolved
                .filter(|path| path.is_file())
                .map(ImageSource::from)
        };
    let placeholder = {
        let text: SharedString = if alt.is_empty() {
            source.clone().into()
        } else {
            alt.clone().into()
        };
        let color = theme.fg2;
        move || {
            div()
                .flex()
                .gap(px(6.))
                .text_color(color)
                .child("🖼")
                .child(StyledText::new(text.clone()))
                .into_any_element()
        }
    };
    match image_source {
        Some(image_source) => div()
            .my(px(2.))
            .child(
                // サイズ未指定 = 画像の実寸（aspect_ratio 維持）を max_w で本文幅に収める。
                // 高さは幅×アスペクト比で決まるので上限を付けない（付けると要素箱と描画の比が
                // ずれてレターボックスの空白が出る）。縦長画像はスクロールで読む（Zed と同じ）。
                img(image_source)
                    .max_w_full()
                    .object_fit(ObjectFit::ScaleDown)
                    .with_fallback(placeholder),
            )
            .into_any_element(),
        None => placeholder(),
    }
}

/// インライン装飾（byte レンジ→HighlightStyle）。`streaming_md_highlights`（agent_panel）と同一の対応。
fn md_highlights(theme: &Theme, spans: &[markdown::Span]) -> Vec<(Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|span| {
            let style = match span.kind {
                markdown::SpanKind::Strong => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
                markdown::SpanKind::Emphasis => HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
                markdown::SpanKind::Strikethrough => HighlightStyle {
                    strikethrough: Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(theme.fg2),
                    }),
                    ..Default::default()
                },
                // インラインコードは HighlightStyle に font-family が無く mono 化できないため
                // 色（syn-mac）+ 薄背景で表現する（agent_panel と同じ割り切り）。
                markdown::SpanKind::Code => HighlightStyle {
                    color: Some(theme.syntax.macro_),
                    background_color: Some(theme.bg3),
                    ..Default::default()
                },
                markdown::SpanKind::Link => HighlightStyle {
                    color: Some(theme.syntax.function),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(theme.syntax.function),
                        wavy: false,
                    }),
                    ..Default::default()
                },
            };
            (span.range.clone(), style)
        })
        .collect()
}

/// コードブロックの tree-sitter ハイライト（`streaming_syntax_highlights` と同一の色対応）。
/// キャッシュは持たない（プレビュー中は点滅停止で idle 再描画が無い＝再構築は toggle/編集時のみ）。
fn code_highlights(
    theme: &Theme,
    language: Option<lang::LanguageId>,
    text: &str,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(language) = language else {
        return Vec::new();
    };
    let Some(highlighter) = lang::Highlighter::for_language(language) else {
        return Vec::new();
    };
    highlighter
        .highlight(text)
        .into_iter()
        .map(|span| {
            let color = match span.kind {
                lang::HighlightKind::Keyword | lang::HighlightKind::Heading => theme.syntax.keyword,
                lang::HighlightKind::Function | lang::HighlightKind::Link => theme.syntax.function,
                lang::HighlightKind::Type | lang::HighlightKind::Strong => theme.syntax.type_,
                lang::HighlightKind::String | lang::HighlightKind::Code => theme.syntax.string,
                lang::HighlightKind::Number => theme.syntax.number,
                lang::HighlightKind::Comment => theme.syntax.comment,
                lang::HighlightKind::Macro | lang::HighlightKind::Emphasis => theme.syntax.macro_,
                lang::HighlightKind::Punctuation => theme.syntax.punctuation,
            };
            (
                span.range,
                HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// 装飾付きテキスト要素。重なり（`***太字斜体***` は Strong と Emphasis が同レンジ、`**`コード`**` は
/// Strong が Code を内包）を [`gpui::combine_highlights`] で非重複・昇順の run へ畳んでから渡す。
/// これを怠ると `with_highlights` の前提（ソート済み非重複）が崩れ layout_line が split_at で abort する。
/// レンジ端点は [`markdown::parse`] が文字境界を保証済み（unit test）＝ snap 不要。
fn styled_text(text: String, highlights: Vec<(Range<usize>, HighlightStyle)>) -> AnyElement {
    let highlights = gpui::combine_highlights(highlights, std::iter::empty()).collect::<Vec<_>>();
    let mut styled = StyledText::new(SharedString::from(text));
    if !highlights.is_empty() {
        styled = styled.with_highlights(highlights);
    }
    styled.into_any_element()
}
