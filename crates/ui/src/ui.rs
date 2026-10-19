//! ui — 再利用 UI 部品。中核は [`Picker`]（1 個のファジーリストを全モーダルで使い回す）。
//!
//! ARCHITECTURE §4: コマンドパレット・ファイルファインダ・プロジェクトスイッチャーは全てこの Picker
//! に載せる。Picker は項目 [`PickerItem`] のリストを持ち、確定/中止を [`PickerEvent`] で通知する
//! （ホスト側が id を解釈する）。色は UI-SPEC §1.3 の許可位置のみ（選択面 = accent-dim）。

/// 書体の設定（UI とコード・O27）。名前は直書きせずここから引く。
mod density;
pub mod emoji;
pub mod fonts;
/// 本文中のパス・URL 検出（`agent_panel` の transcript と `terminal_view` が共有する）。
pub mod links;
pub use density::{row_height, row_padding, RowDensity};
pub use fonts::{code_font, ui_font, FontFamilies};

use gpui::{
    div, ease_out_quint, hsla, prelude::*, px, svg, Animation, AnimationExt, AnyView, App,
    BoxShadow, Context, EventEmitter, FocusHandle, Focusable, Hsla, IntoElement, KeyDownEvent,
    MouseButton, Render, ScrollHandle, SharedString, Window,
};
use std::path::PathBuf;
use std::time::Duration;
use theme_core::Theme;

/// エクスプローラの行をドラッグする際のペイロード兼ゴースト。行き先は 3 通り:
/// チャット composer（@メンション参照）と、エクスプローラ内のフォルダ/背景（Finder 風の移動）と、
/// ターミナル（引用したパスを貼り付ける）。
/// `path` は @メンション用の表示文字列（プロジェクト相対が望ましい）・`source` は移動用の絶対パス。
#[derive(Clone)]
pub struct DraggedFile {
    pub path: SharedString,
    pub source: PathBuf,
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
            let tooltip = Tooltip {
                text: text.clone(),
                theme: theme.clone(),
            };
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
        .shadow(vec![
            BoxShadow::new(px(0.), px(4.), hsla(0., 0., 0., 0.36)).blur_radius(px(12.))
        ])
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
    /// 並びの加点（あいまい一致のスコアに足す・既定 0）。⌘P の「最近開いたファイルを上へ」
    /// 「無視されたファイルは後ろへ」に使う（D19）。一致しない項目を出す力は無い。
    pub boost: i32,
    /// 名前に一致しない時、`detail`（パス等）でも探す。名前の一致より下に並べる
    /// （最近使った項目の「フォルダの途中の名前で探す」用・既定 false）。
    pub match_detail: bool,
}

impl PickerItem {
    pub fn new(id: usize, label: impl Into<SharedString>) -> Self {
        Self {
            id,
            label: label.into(),
            detail: None,
            accent: None,
            dots: Vec::new(),
            boost: 0,
            match_detail: false,
        }
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

    pub fn with_boost(mut self, boost: i32) -> Self {
        self.boost = boost;
        self
    }

    /// 名前に一致しない時、`detail` でも探す（[`Self::match_detail`]）。
    pub fn with_match_detail(mut self) -> Self {
        self.match_detail = true;
        self
    }
}

/// Picker からホストへの通知。id は [`PickerItem::id`]（ホストが解釈）。
pub enum PickerEvent {
    /// 選択がハイライトされた（矢印/入力で移動）。テーマセレクタのライブプレビュー等に使う。
    Highlighted(usize),
    Confirmed(usize),
    /// ⌘⏎ の確定（VSCode の Open Recent 互換 = 「新しいウィンドウで開く」等の副意味論）。
    /// 対応しないモードのホストは通常の Confirmed と同じに扱ってよい。
    ConfirmedSecondary(usize),
    Dismissed,
}

/// ファジーリストのモーダル。
pub struct Picker {
    query_action: Option<(usize, SharedString)>,
    /// 入力に 1 件も一致しない時だけ出す行（⌘P の「無視されたファイルも探す」・D19）。
    /// 一致が 1 件でもあれば出さない（普段のリストを汚さない）。
    fallback_action: Option<(usize, SharedString)>,
    placeholder: SharedString,
    query: String,
    items: Vec<PickerItem>,
    filtered: Vec<usize>,
    selected: usize,
    focus_handle: FocusHandle,
    theme: Theme,
    accent: Hsla,
    /// 入力行に置く 1 行入力（ホストが作る IME の正しい入力欄）。None なら Picker 自身がキーを受けて
    /// 文字を描く（テスト・入力欄を渡さないホスト）。入力欄の中身はホストが [`Self::set_query`] で写す。
    input: Option<AnyView>,
    /// 入力欄が空か（プレースホルダを出すか）。変換中の未確定の文字だけでも空ではない。
    input_empty: bool,
    /// リストのスクロール（↑↓で選んだ行を見える所へ寄せる）。
    scroll_handle: ScrollHandle,
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
        // 空のクエリでも加点（⌘P の最近開いたファイル）で並べる。加点が無ければ元の並びのまま。
        let filtered = rank_items("", &items);
        Self {
            query_action: None,
            fallback_action: None,
            placeholder: placeholder.into(),
            query: String::new(),
            items,
            filtered,
            selected: 0,
            focus_handle: cx.focus_handle(),
            theme,
            accent,
            input: None,
            input_empty: true,
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// 入力行に 1 行入力を置く（[`Self::input`]）。キー（↑↓・⏎・esc）はホストが入力欄の束より
    /// 先に受けて [`Self::select_previous`] などへ回す。
    pub fn set_input(&mut self, input: AnyView, cx: &mut Context<Self>) {
        self.input = Some(input);
        cx.notify();
    }

    /// 入力欄が空か（プレースホルダの表示）をホストから写す。
    pub fn set_input_empty(&mut self, empty: bool, cx: &mut Context<Self>) {
        if self.input_empty != empty {
            self.input_empty = empty;
            cx.notify();
        }
    }

    /// 選択を 1 つ上へ（↑）。
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    /// 選択を 1 つ下へ（↓）。
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    /// ⌘⏎ の確定（[`PickerEvent::ConfirmedSecondary`]）。
    pub fn confirm_selected_secondary(&mut self, cx: &mut Context<Self>) {
        self.confirm_secondary(cx);
    }

    /// 閉じる（esc と同じ [`PickerEvent::Dismissed`]）。
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(PickerEvent::Dismissed);
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// 入力した名前で作成する行。既存候補を選ぶ操作と同じ Picker の中で完結する。
    pub fn set_query_action(
        &mut self,
        id: usize,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.query_action = Some((id, label.into()));
        self.refilter();
        cx.notify();
    }

    /// 入力に何も一致しない時だけ出す行を設定する（`None` で消す）。確定すると `id` を通知する。
    pub fn set_fallback_action(
        &mut self,
        action: Option<(usize, SharedString)>,
        cx: &mut Context<Self>,
    ) {
        self.fallback_action = action;
        cx.notify();
    }

    /// 一致なしの行が今出ているか（入力があり、項目が 1 件も一致しない）。
    pub fn fallback_visible(&self) -> bool {
        self.fallback_action.is_some() && !self.query.trim().is_empty() && self.filtered.is_empty()
    }

    /// 項目を後ろに足す（背景で追加で集めた行・⌘P の 2 回目）。現在のクエリで並べ直し、
    /// 先頭の一致を選ぶ。
    pub fn append_items(&mut self, items: Vec<PickerItem>, cx: &mut Context<Self>) {
        self.items.extend(items);
        self.refilter();
        self.emit_highlight(cx);
        cx.notify();
    }

    /// いま一致している項目の id（並び順）。テスト・プログラム操作用。
    pub fn matched_ids(&self) -> Vec<usize> {
        self.filtered
            .iter()
            .filter_map(|&index| self.items.get(index).map(|item| item.id))
            .collect()
    }

    /// テーマを差し替える（ライブプレビュー中に Picker 自身も追従させる）。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// 項目を差し替える（背景で集めた行を後から流し込む・M12-12 ⌘O ダッシュボード）。
    /// 現在のクエリで再フィルタし、選んでいた行と同じ id の行を選び直す（間に行が差し込まれても
    /// 選んでいた物から動かない）。その id が消えていれば、選択位置を範囲内へクランプする。
    pub fn set_items(&mut self, items: Vec<PickerItem>, cx: &mut Context<Self>) {
        let selected = self.selected;
        let selected_id = self.selected_id();
        self.items = items;
        self.refilter();
        self.selected = selected_id
            .and_then(|id| {
                self.filtered
                    .iter()
                    .position(|&index| self.items[index].id == id)
            })
            .unwrap_or_else(|| selected.min(self.filtered.len().saturating_sub(1)));
        cx.notify();
    }

    /// いま選んでいる行の id（一致が無ければ None）。ホストが選択に合わせて別の場所を描く用
    /// （⌘O で選んでいるプロジェクトをレールで光らせる）。
    pub fn selected_id(&self) -> Option<usize> {
        self.filtered
            .get(self.selected)
            .and_then(|&index| self.items.get(index))
            .map(|item| item.id)
    }

    /// クエリを直接セットして再フィルタ（開発プローブ / プログラム操作用）。
    pub fn set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        self.query = query.into();
        self.refilter();
        self.emit_highlight(cx);
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
        if let Some((id, _)) = &self.query_action {
            self.items.retain(|item| item.id != *id);
        }
        self.filtered = rank_items(&self.query, &self.items);
        if let Some((id, label)) = &self.query_action {
            if !self.query.trim().is_empty() {
                self.filtered.push(self.items.len());
                self.items.push(PickerItem::new(
                    *id,
                    format!("{label}: {}", self.query.trim()),
                ));
            }
        }
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.scroll_handle.scroll_to_item(self.selected);
        self.emit_highlight(cx);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(&item_index) = self.filtered.get(self.selected) {
            cx.emit(PickerEvent::Confirmed(self.items[item_index].id));
        } else if self.fallback_visible() {
            if let Some((id, _)) = &self.fallback_action {
                cx.emit(PickerEvent::Confirmed(*id));
            }
        }
    }

    fn confirm_secondary(&mut self, cx: &mut Context<Self>) {
        if let Some(&item_index) = self.filtered.get(self.selected) {
            cx.emit(PickerEvent::ConfirmedSecondary(self.items[item_index].id));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // 入力欄があれば文字・編集キーは入力欄の物（ここで拾うと二重に入る）。↑↓⏎esc はホストが
        // 入力欄の束より先に受ける。
        if self.input.is_some() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => cx.emit(PickerEvent::Dismissed),
            // ⌘⏎ = 副確定（新しいウィンドウで開く等）。無修飾 ⏎ = 通常確定。
            "enter" if event.keystroke.modifiers.platform => self.confirm_secondary(cx),
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
        let query_color = if self.query.is_empty() {
            theme.fg2
        } else {
            theme.fg0
        };

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
                    // 入力行: 枠のある入力欄（検索アイコン + 入力欄）。ターミナル検索・設定の検索欄と
                    // 同じ形（bg1・角 6・フォーカス中の枠はプロジェクト色）。入力欄が無いホストは
                    // 打った文字を描くだけ。
                    .child(
                        div().p_2().border_b_1().border_color(theme.border).child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(px(6.))
                                .bg(theme.bg1)
                                .border_1()
                                .border_color(accent)
                                .cursor(gpui::CursorStyle::IBeam)
                                .child(
                                    svg()
                                        .path("icons/search.svg")
                                        .size(px(14.))
                                        .flex_none()
                                        .text_color(theme.fg2),
                                )
                                .map(|field| match self.input.clone() {
                                    Some(input) => field.child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .min_w_0()
                                            .h(px(20.))
                                            .overflow_hidden()
                                            // 入力欄はこの枠（高さ 20）いっぱい。親が高さを決めないと 0 に潰れる。
                                            .child(input)
                                            .when(self.input_empty, |text| {
                                                // 空の欄のキャレット（行頭・幅 2px）に重ならないよう、その右から。
                                                text.child(
                                                    div()
                                                        .absolute()
                                                        .inset_0()
                                                        .pl(px(4.))
                                                        .flex()
                                                        .items_center()
                                                        .overflow_hidden()
                                                        .text_color(theme.fg2)
                                                        .child(self.placeholder.clone()),
                                                )
                                            }),
                                    ),
                                    None => field.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_color(query_color)
                                            .child(query_display),
                                    ),
                                }),
                        ),
                    )
                    // リスト（最大 50 件・↑↓で選んだ行へスクロールを寄せる）
                    .child(
                        div()
                            .id("picker-list")
                            .flex()
                            .flex_col()
                            .p_1()
                            .max_h(px(340.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .children(self.filtered.iter().take(50).enumerate().map(
                                |(row, &item_index)| {
                                    let item = &self.items[item_index];
                                    let is_selected = row == self.selected;
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_2()
                                        // 行の詰め具合（O27・`density`）。
                                        .py(row_padding(cx, 4.))
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
                                                div()
                                                    .size(px(7.))
                                                    .rounded(px(3.5))
                                                    .flex_none()
                                                    .bg(color),
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
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(3.))
                                                    .flex_none()
                                                    .children(item.dots.iter().map(|color| {
                                                        div()
                                                            .size(px(6.))
                                                            .rounded(px(3.))
                                                            .flex_none()
                                                            .bg(*color)
                                                    })),
                                            )
                                        })
                                },
                            ))
                            // 一致なしの時だけの行（⌘P の「無視されたファイルも探す」）。唯一の行なので
                            // 選択面で出し、⏎ / クリックで確定する。
                            .when_some(
                                self.fallback_visible()
                                    .then(|| self.fallback_action.clone())
                                    .flatten(),
                                |list, (_, label)| {
                                    list.child(
                                        div()
                                            .id("picker-fallback")
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .px_2()
                                            .py_1()
                                            .rounded(px(5.))
                                            .cursor_pointer()
                                            .text_size(px(12.5))
                                            .text_color(theme.fg0)
                                            .bg(accent.alpha(0.16))
                                            .child(
                                                div().flex_none().text_color(theme.fg2).child("⌕"),
                                            )
                                            .child(label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _event, _window, cx| {
                                                    cx.stop_propagation();
                                                    this.confirm(cx);
                                                }),
                                            ),
                                    )
                                },
                            ),
                    ),
            )
    }
}

/// `detail` での一致（[`PickerItem::match_detail`]）を名前の一致より下へ落とす減点。
const DETAIL_MATCH_PENALTY: i32 = 1000;

/// クエリに一致する項目の添字を、良い順に並べて返す（Picker の refilter の本体）。
/// 順位 = あいまい一致のスコア + [`PickerItem::boost`]。同点は元の並び（安定ソート）。
/// 一致しない項目は加点があっても出さない。
pub fn rank_items(query: &str, items: &[PickerItem]) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            fuzzy_score(query, &item.label)
                .or_else(|| {
                    // 名前で外れた時だけパス等で探す。名前の一致より必ず下に並べる。
                    item.match_detail
                        .then_some(item.detail.as_ref())
                        .flatten()
                        .and_then(|detail| detail_score(query, detail))
                        .map(|score| score.saturating_sub(DETAIL_MATCH_PENALTY))
                })
                .map(|score| (index, score.saturating_add(item.boost)))
        })
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(index, _)| index).collect()
}

/// パス等（[`PickerItem::match_detail`]）での一致。打った語がそのまま含まれる時だけ当てる
/// （あいまい一致だと長いパスの飛び飛びの文字に当たり、`api` でほぼ全部が残る）。後ろ寄り
/// （フォルダ名に近い所）の一致ほど上。
fn detail_score(query: &str, detail: &str) -> Option<i32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let detail = detail.to_lowercase();
    let position = detail.rfind(&query)?;
    let distance_from_end = detail.len() - position - query.len();
    Some(-(distance_from_end as i32) / 4)
}

/// ベンチ専用の公開ラッパ（examples/bench_fuzzy が ⌘P の refilter 負荷を実測する用）。
#[doc(hidden)]
pub fn fuzzy_score_for_bench(query: &str, text: &str) -> Option<i32> {
    fuzzy_score(query, text)
}

/// 素朴なサブシーケンス fuzzy スコア。query の各文字が text に順に現れれば `Some(score)`。
/// 連続一致・先頭寄りを加点。大文字小文字は無視。空 query は全一致（スコア 0）。
/// Picker（⌘P 等）と composer の `/` 補完が同じ並びになるよう共有する。
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
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

    fn items(labels: &[(&str, i32)]) -> Vec<PickerItem> {
        labels
            .iter()
            .enumerate()
            .map(|(id, (label, boost))| PickerItem::new(id, *label).with_boost(*boost))
            .collect()
    }

    /// ⌘P の並び（D19）: 加点（最近開いた = +・無視された = −）がスコアに足され、
    /// 空のクエリでは加点順（同点は元の並び）。一致しない項目は加点があっても出ない。
    #[test]
    fn rank_items_adds_boost_to_the_fuzzy_score() {
        let list = items(&[
            ("src/a.rs", 0),
            ("src/main.rs", 0),
            ("src/recent.rs", 10_000),
            ("build/ignored.rs", -10_000),
        ]);
        assert_eq!(rank_items("", &list), vec![2, 0, 1, 3]);
        assert_eq!(
            rank_items("rs", &list).first(),
            Some(&2),
            "最近開いたファイルが上"
        );
        assert_eq!(
            rank_items("rs", &list).last(),
            Some(&3),
            "無視されたファイルは最後"
        );
        assert_eq!(
            rank_items("main", &list),
            vec![1],
            "一致しないものは出さない"
        );
    }

    /// 最近使った項目（`match_detail`）: 名前で外れてもパスで当たる。名前の一致が必ず上。
    /// 印の無い項目（⌘P 等）はパスでは探さない。
    #[test]
    fn rank_items_matches_detail_only_when_asked() {
        let list = vec![
            PickerItem::new(0, "api")
                .with_detail("/Users/me/Work/necoder-api")
                .with_match_detail(),
            PickerItem::new(1, "necoder")
                .with_detail("/Users/me/Work/necoder")
                .with_match_detail(),
            PickerItem::new(2, "notes").with_detail("/Users/me/necoder-notes"),
        ];
        assert_eq!(
            rank_items("necoder", &list),
            vec![1, 0],
            "名前の一致が上・パスだけの一致はその下・印の無い項目は出ない"
        );
        assert_eq!(
            rank_items("work", &list),
            vec![1, 0],
            "フォルダの途中の名前でも当たる・フォルダ名に近い一致が上"
        );
        assert_eq!(
            rank_items("mwk", &list),
            Vec::<usize>::new(),
            "パスは飛び飛びの文字では当てない"
        );
    }

    #[test]
    fn contiguous_scores_higher_than_scattered() {
        let contiguous = fuzzy_score("main", "main.rs").unwrap();
        let scattered = fuzzy_score("main", "m-a-i-n.rs").unwrap();
        assert!(contiguous > scattered, "連続一致が散在より高スコア");
    }
}
