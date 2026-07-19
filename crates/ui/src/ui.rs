//! ui — 再利用 UI 部品。中核は [`Picker`]（1 個のファジーリストを全モーダルで使い回す）。
//!
//! ARCHITECTURE §4: コマンドパレット・ファイルファインダ・プロジェクトスイッチャーは全てこの Picker
//! に載せる。Picker は項目 [`PickerItem`] のリストを持ち、確定/中止を [`PickerEvent`] で通知する
//! （ホスト側が id を解釈する）。色は UI-SPEC §1.3 の許可位置のみ（選択面 = accent-dim）。

use gpui::{
    Animation, AnimationExt, AnyView, App, BoxShadow, Context, EventEmitter, FocusHandle, Focusable,
    Hsla, IntoElement, KeyDownEvent, MouseButton, Render, SharedString, Window, div, ease_out_quint,
    hsla, prelude::*, px,
};
use std::time::Duration;
use theme_core::Theme;

/// エクスプローラからチャット composer へファイルをドラッグする際のペイロード兼ゴースト。
/// `path` は @メンション用の表示文字列（プロジェクト相対が望ましい）。
#[derive(Clone)]
pub struct DraggedFile {
    pub path: SharedString,
    pub theme: Theme,
}

impl Render for DraggedFile {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(self.theme.bg2)
            .border_1()
            .border_color(self.theme.border)
            .text_size(px(12.))
            .text_color(self.theme.fg0)
            .child(format!("@{}", self.path))
    }
}

/// ホバー時に "すっと" 出る簡易ツールチップ。gpui は 500ms 遅延の後に tooltip view を作るが
/// **アニメーションはしない**ので、ここで出現時に一度だけ opacity 0→1 の fade-in を掛ける
/// （oneshot なので settle 後は再描画しない＝idle 0% を壊さない）。
/// 使い方: `div().id("x").tooltip(ui::Tooltip::text("説明", theme))`。
pub struct Tooltip {
    text: SharedString,
    theme: Theme,
}

impl Tooltip {
    /// `.tooltip(...)` にそのまま渡せる builder（`Fn(&mut Window, &mut App) -> AnyView`）を返す。
    pub fn text(
        text: impl Into<SharedString>,
        theme: Theme,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_window, cx| {
            let tooltip = Tooltip { text: text.clone(), theme: theme.clone() };
            cx.new(|_cx| tooltip).into()
        }
    }

    /// 開発用プレビュー: tooltip の中身（fade 無しの静的ボックス）を返す。ヘッドレス撮影で見た目を確認する。
    pub fn preview(text: impl Into<SharedString>, theme: &Theme) -> impl IntoElement {
        tooltip_box(text.into(), theme)
    }
}

/// tooltip の見た目（角丸・bg2・罫線・浮遊影・テキスト）。Render と preview で共有する。
fn tooltip_box(text: SharedString, theme: &Theme) -> impl IntoElement {
    div()
        .px(px(8.))
        .py(px(4.))
        .rounded(px(6.))
        .bg(theme.bg2)
        .border_1()
        .border_color(theme.border)
        .text_size(px(11.5))
        .text_color(theme.fg0)
        // 浮遊感の影（暗色 UI 用にやや濃いめ）。
        .shadow(vec![BoxShadow::new(px(0.), px(4.), hsla(0., 0., 0., 0.36)).blur_radius(px(12.))])
        .child(text)
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 影の分だけ余白を持たせて浮いて見せる。出現時に一度だけ fade-in（gpui は tooltip を
        // アニメしないので "すっと出る" をここで足す。oneshot＝settle 後は再描画しない）。
        div()
            .p(px(4.))
            .child(tooltip_box(self.text.clone(), &self.theme))
            .with_animation(
                "tooltip-fade",
                Animation::new(Duration::from_millis(110)).with_easing(ease_out_quint()),
                |element, delta| element.opacity(delta),
            )
    }
}

/// Picker の 1 項目。`id` はホストが解釈する（ファイル index・プロジェクト index 等）。
pub struct PickerItem {
    pub id: usize,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// 行頭の●（プロジェクト色・M12-12）。None なら出さない。
    pub accent: Option<Hsla>,
    /// 右端の小ドット列（実行中スレッド色・M12-12 の「どこで何が走っているか」）。
    pub dots: Vec<Hsla>,
}

impl PickerItem {
    pub fn new(id: usize, label: impl Into<SharedString>) -> Self {
        Self { id, label: label.into(), detail: None, accent: None, dots: Vec::new() }
    }

    pub fn with_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_accent(mut self, color: Hsla) -> Self {
        self.accent = Some(color);
        self
    }

    pub fn with_dots(mut self, dots: Vec<Hsla>) -> Self {
        self.dots = dots;
        self
    }
}

/// Picker からホストへの通知。id は [`PickerItem::id`]（ホストが解釈）。
pub enum PickerEvent {
    /// 選択がハイライトされた（矢印/入力で移動）。テーマセレクタのライブプレビュー等に使う。
    Highlighted(usize),
    Confirmed(usize),
    Dismissed,
}

/// ファジーリストのモーダル。
pub struct Picker {
    placeholder: SharedString,
    query: String,
    items: Vec<PickerItem>,
    filtered: Vec<usize>,
    selected: usize,
    focus_handle: FocusHandle,
    theme: Theme,
    accent: Hsla,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Picker {
    pub fn new(
        placeholder: impl Into<SharedString>,
        items: Vec<PickerItem>,
        theme: Theme,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            placeholder: placeholder.into(),
            query: String::new(),
            items,
            filtered,
            selected: 0,
            focus_handle: cx.focus_handle(),
            theme,
            accent,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// テーマを差し替える（ライブプレビュー中に Picker 自身も追従させる）。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// 項目を差し替える（背景で集めた行を後から流し込む・M12-12 ⌘O ダッシュボード）。
    /// 現在のクエリで再フィルタし、選択位置は範囲内へクランプする。
    pub fn set_items(&mut self, items: Vec<PickerItem>, cx: &mut Context<Self>) {
        let selected = self.selected;
        self.items = items;
        self.refilter();
        self.selected = selected.min(self.filtered.len().saturating_sub(1));
        cx.notify();
    }

    /// クエリを直接セットして再フィルタ（開発プローブ / プログラム操作用）。
    pub fn set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        self.query = query.into();
        self.refilter();
        cx.notify();
    }

    /// 現在の選択を確定する（Enter と同じ。開発プローブ / プログラム操作用）。
    pub fn confirm_selected(&mut self, cx: &mut Context<Self>) {
        self.confirm(cx);
    }

    /// 現在ハイライト中の項目 id をホストへ通知（ライブプレビュー用）。
    fn emit_highlight(&mut self, cx: &mut Context<Self>) {
        if let Some(&item_index) = self.filtered.get(self.selected) {
            cx.emit(PickerEvent::Highlighted(self.items[item_index].id));
        }
    }

    fn refilter(&mut self) {
        let mut scored: Vec<(usize, i32)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| fuzzy_score(&self.query, &item.label).map(|score| (index, score)))
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        self.filtered = scored.into_iter().map(|(index, _)| index).collect();
        self.selected = 0;
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.emit_highlight(cx);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(&item_index) = self.filtered.get(self.selected) {
            cx.emit(PickerEvent::Confirmed(self.items[item_index].id));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.emit(PickerEvent::Dismissed),
            "enter" => self.confirm(cx),
            "up" => self.move_selection(-1, cx),
            "down" => self.move_selection(1, cx),
            "backspace" => {
                self.query.pop();
                self.refilter();
                self.emit_highlight(cx);
                cx.notify();
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !text.chars().any(char::is_control) {
                        self.query.push_str(text);
                        self.refilter();
                        self.emit_highlight(cx);
                        cx.notify();
                    }
                }
            }
        }
    }
}

impl Focusable for Picker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Picker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent;
        let query_display: SharedString = if self.query.is_empty() {
            self.placeholder.clone()
        } else {
            self.query.clone().into()
        };
        let query_color = if self.query.is_empty() { theme.fg2 } else { theme.fg0 };

        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(120.))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            // モーダル外（背景）クリックで閉じる（ESC と同じ Dismissed）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event, _window, cx| cx.emit(PickerEvent::Dismissed)),
            )
            .child(
                div()
                    .w(px(560.))
                    .flex()
                    .flex_col()
                    .bg(theme.bg2)
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    // モーダル箱の中のクリックは背景へ伝播させない（閉じない）。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| cx.stop_propagation()),
                    )
                    // 入力行
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(theme.border)
                            .text_color(query_color)
                            .child(query_display),
                    )
                    // リスト（最大 50 件）
                    .child(
                        div().flex().flex_col().p_1().max_h(px(340.)).overflow_hidden().children(
                            self.filtered.iter().take(50).enumerate().map(|(row, &item_index)| {
                                let item = &self.items[item_index];
                                let is_selected = row == self.selected;
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(5.))
                                    .cursor_pointer()
                                    // マウスクリックで選択＋確定（キーボード ↑↓/Enter に加えて・全 Picker 共通）。
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            cx.stop_propagation(); // 背景/箱の handler へ伝播させない
                                            this.selected = row;
                                            this.confirm(cx);
                                        }),
                                    )
                                    .hover(|style| style.bg(theme.bg1))
                                    .text_size(px(12.5))
                                    .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                                    .when(is_selected, |element| element.bg(accent.alpha(0.16)))
                                    // 行頭●（プロジェクト色・M12-12）。
                                    .when_some(item.accent, |element, color| {
                                        element.child(
                                            div().size(px(7.)).rounded(px(3.5)).flex_none().bg(color),
                                        )
                                    })
                                    .child(item.label.clone())
                                    .when_some(item.detail.clone(), |element, detail| {
                                        element.child(
                                            div()
                                                .ml_auto()
                                                .text_color(theme.fg2)
                                                .text_size(px(11.))
                                                .child(detail),
                                        )
                                    })
                                    // 右端の実行中スレッドドット列（M12-12）。
                                    .when(!item.dots.is_empty(), |element| {
                                        element.child(
                                            div().flex().items_center().gap(px(3.)).flex_none().children(
                                                item.dots.iter().map(|color| {
                                                    div()
                                                        .size(px(6.))
                                                        .rounded(px(3.))
                                                        .flex_none()
                                                        .bg(*color)
                                                }),
                                            ),
                                        )
                                    })
                            }),
                        ),
                    ),
            )
    }
}

/// ベンチ専用の公開ラッパ（examples/bench_fuzzy が ⌘P の refilter 負荷を実測する用）。
#[doc(hidden)]
pub fn fuzzy_score_for_bench(query: &str, text: &str) -> Option<i32> {
    fuzzy_score(query, text)
}

/// 素朴なサブシーケンス fuzzy スコア。query の各文字が text に順に現れれば `Some(score)`。
/// 連続一致・先頭寄りを加点。大文字小文字は無視。空 query は全一致（スコア 0）。
fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text_chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0i32;
    let mut cursor = 0usize;
    let mut last_match: Option<usize> = None;
    for query_char in query.to_lowercase().chars() {
        let mut matched = None;
        while cursor < text_chars.len() {
            if text_chars[cursor] == query_char {
                matched = Some(cursor);
                cursor += 1;
                break;
            }
            cursor += 1;
        }
        let position = matched?;
        if last_match == Some(position.wrapping_sub(1)) {
            score += 6; // 連続一致ボーナス
        }
        score -= position as i32 / 4; // 先頭寄りを微加点
        last_match = Some(position);
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_subsequence() {
        assert!(fuzzy_score("mn", "main.rs").is_some());
        assert!(fuzzy_score("xyz", "main.rs").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn contiguous_scores_higher_than_scattered() {
        let contiguous = fuzzy_score("main", "main.rs").unwrap();
        let scattered = fuzzy_score("main", "m-a-i-n.rs").unwrap();
        assert!(contiguous > scattered, "連続一致が散在より高スコア");
    }
}
