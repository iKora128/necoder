//! 外観 › テーマの「色を個別に変える」: 選択中のテーマの色をトークンごとに上書きする（settings.json の
//! `theme_overrides.<テーマ名>`）。テーマファイルは増やさず、上書きしていない色はテーマの値のまま。

use crate::{forget_theme_overrides, set_theme_override, SettingsView};
use gpui::{
    canvas, div, linear_color_stop, linear_gradient, prelude::*, px, svg, Bounds, Context,
    DispatchPhase, Div, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, SharedString, Window,
};
use settings_core::Settings;
use std::cell::Cell;
use std::rc::Rc;
use theme_core::ThemeToken;

/// 彩度・明度の四角の大きさ。
const SQUARE_WIDTH: f32 = 220.;
const SQUARE_HEIGHT: f32 = 132.;
/// 色相のバーの高さ（幅は四角と同じ）。
const HUE_HEIGHT: f32 = 12.;
/// ピッカー右列（見本と hex 入力）の幅。
const SIDE_WIDTH: f32 = 112.;
/// hex 入力に打てる桁（`#rrggbb` / `#rrggbbaa`）。
const HEX_DIGITS: [usize; 2] = [6, 8];

/// 外観設定で並べるまとまり（この順に並べ、まとまりの中は [`ThemeToken::ALL`] の順）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenGroup {
    Chat,
    Surface,
    Text,
    Status,
    Syntax,
}

const GROUPS: [TokenGroup; 5] = [
    TokenGroup::Chat,
    TokenGroup::Surface,
    TokenGroup::Text,
    TokenGroup::Status,
    TokenGroup::Syntax,
];

fn token_group(token: ThemeToken) -> TokenGroup {
    match token {
        ThemeToken::ChatUserBg | ThemeToken::ChatStepBg | ThemeToken::ChatAgentBg => {
            TokenGroup::Chat
        }
        ThemeToken::Bg0
        | ThemeToken::Bg1
        | ThemeToken::Bg2
        | ThemeToken::Bg3
        | ThemeToken::Border => TokenGroup::Surface,
        ThemeToken::Fg0 | ThemeToken::Fg1 | ThemeToken::Fg2 => TokenGroup::Text,
        ThemeToken::Ok | ThemeToken::Warn | ThemeToken::Err => TokenGroup::Status,
        ThemeToken::SyntaxKeyword
        | ThemeToken::SyntaxFunction
        | ThemeToken::SyntaxType
        | ThemeToken::SyntaxString
        | ThemeToken::SyntaxNumber
        | ThemeToken::SyntaxComment
        | ThemeToken::SyntaxMacro
        | ThemeToken::SyntaxPunctuation => TokenGroup::Syntax,
    }
}

/// まとまりの見出し（i18n）。
fn group_label(group: TokenGroup) -> String {
    match group {
        TokenGroup::Chat => i18n::t!("settings.theme_group_chat"),
        TokenGroup::Surface => i18n::t!("settings.theme_group_surface"),
        TokenGroup::Text => i18n::t!("settings.theme_group_text"),
        TokenGroup::Status => i18n::t!("settings.theme_group_status"),
        TokenGroup::Syntax => i18n::t!("settings.theme_group_syntax"),
    }
}

/// トークンの表示名（i18n）。
fn token_label(token: ThemeToken) -> String {
    match token {
        ThemeToken::Bg0 => i18n::t!("settings.theme_token_bg0"),
        ThemeToken::Bg1 => i18n::t!("settings.theme_token_bg1"),
        ThemeToken::Bg2 => i18n::t!("settings.theme_token_bg2"),
        ThemeToken::Bg3 => i18n::t!("settings.theme_token_bg3"),
        ThemeToken::Border => i18n::t!("settings.theme_token_border"),
        ThemeToken::Fg0 => i18n::t!("settings.theme_token_fg0"),
        ThemeToken::Fg1 => i18n::t!("settings.theme_token_fg1"),
        ThemeToken::Fg2 => i18n::t!("settings.theme_token_fg2"),
        ThemeToken::Ok => i18n::t!("settings.theme_token_ok"),
        ThemeToken::Warn => i18n::t!("settings.theme_token_warn"),
        ThemeToken::Err => i18n::t!("settings.theme_token_err"),
        ThemeToken::SyntaxKeyword => i18n::t!("settings.theme_token_syntax_keyword"),
        ThemeToken::SyntaxFunction => i18n::t!("settings.theme_token_syntax_function"),
        ThemeToken::SyntaxType => i18n::t!("settings.theme_token_syntax_type"),
        ThemeToken::SyntaxString => i18n::t!("settings.theme_token_syntax_string"),
        ThemeToken::SyntaxNumber => i18n::t!("settings.theme_token_syntax_number"),
        ThemeToken::SyntaxComment => i18n::t!("settings.theme_token_syntax_comment"),
        ThemeToken::SyntaxMacro => i18n::t!("settings.theme_token_syntax_macro"),
        ThemeToken::SyntaxPunctuation => i18n::t!("settings.theme_token_syntax_punctuation"),
        ThemeToken::ChatStepBg => i18n::t!("settings.theme_token_chat_step_bg"),
        ThemeToken::ChatAgentBg => i18n::t!("settings.theme_token_chat_agent_bg"),
        ThemeToken::ChatUserBg => i18n::t!("settings.theme_token_chat_user_bg"),
    }
}

/// 色を編んでいるトークン 1 つ分の状態（ビューのメモリ・永続化しない）。
pub(crate) struct ColorEditor {
    token: ThemeToken,
    /// 色相・彩度・明度（HSV・どれも 0〜1）。灰色でも色相を失わないよう、色からは外から決まった時だけ引く。
    hue: f32,
    saturation: f32,
    value: f32,
    /// 透明度。ピッカーは変えず、テーマの `#rrggbbaa` を保存し直しても落とさない。
    alpha: f32,
    /// hex 入力欄に打っている途中の桁（`#` を除く）。空の間は今の色を薄く出す。
    typed: String,
    /// このトークンだけテーマのデフォルトへ戻した時に出る色。見本の左半分に出し、押すと戻す。
    default: Hsla,
    focus: FocusHandle,
    square_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    hue_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    dragging: Option<DragTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragTarget {
    Square,
    Hue,
}

impl ColorEditor {
    fn new(token: ThemeToken, color: Hsla, default: Hsla, focus: FocusHandle) -> Self {
        let mut editor = ColorEditor {
            token,
            hue: 0.,
            saturation: 0.,
            value: 0.,
            alpha: 1.,
            typed: String::new(),
            default,
            focus,
            square_bounds: Rc::new(Cell::new(None)),
            hue_bounds: Rc::new(Cell::new(None)),
            dragging: None,
        };
        editor.show_color(color);
        editor
    }

    fn color(&self) -> Hsla {
        color_from_hsv(self.hue, self.saturation, self.value).alpha(self.alpha)
    }

    /// ピッカーを `color` に合わせ、打ちかけの hex を捨てる（hex の確定やリセットで色が外から決まった時）。
    fn show_color(&mut self, color: Hsla) {
        let (hue, saturation, value) = hsv_from_color(color);
        self.hue = hue;
        self.saturation = saturation;
        self.value = value;
        self.alpha = color.a;
        self.typed.clear();
    }

    /// 四角か色相バーの上の位置から色を決める（端の外は端に寄せる）。
    fn pick_at(&mut self, target: DragTarget, position: Point<Pixels>) {
        let bounds = match target {
            DragTarget::Square => self.square_bounds.get(),
            DragTarget::Hue => self.hue_bounds.get(),
        };
        let Some(bounds) = bounds else {
            return;
        };
        let horizontal = ((position.x - bounds.origin.x) / bounds.size.width).clamp(0., 1.);
        let vertical = ((position.y - bounds.origin.y) / bounds.size.height).clamp(0., 1.);
        match target {
            DragTarget::Square => {
                self.saturation = horizontal;
                self.value = 1. - vertical;
            }
            DragTarget::Hue => self.hue = horizontal,
        }
        self.typed.clear();
    }

    /// 打った桁を足す（16 進だけ・8 桁まで・`#` は読み飛ばす）。
    fn type_digits(&mut self, text: &str) {
        for character in text.chars().filter(char::is_ascii_hexdigit) {
            if self.typed.len() >= HEX_DIGITS[1] {
                break;
            }
            self.typed.push(character.to_ascii_lowercase());
        }
    }

    /// 打った桁が `#rrggbb` か `#rrggbbaa` にそろっていれば、その色。
    fn typed_color(&self) -> Option<Hsla> {
        if HEX_DIGITS.contains(&self.typed.len()) {
            theme_core::parse_hex(&self.typed)
        } else {
            None
        }
    }
}

/// gpui の Hsla（HSL）を HSV（色相, 彩度, 明度）へ。
fn hsv_from_color(color: Hsla) -> (f32, f32, f32) {
    let value = color.l + color.s * color.l.min(1. - color.l);
    let saturation = if value <= 0. {
        0.
    } else {
        2. * (1. - color.l / value)
    };
    (color.h, saturation.clamp(0., 1.), value.clamp(0., 1.))
}

/// HSV（色相, 彩度, 明度）を不透明な Hsla へ。
fn color_from_hsv(hue: f32, saturation: f32, value: f32) -> Hsla {
    let lightness = value * (1. - saturation / 2.);
    let hsl_saturation = if lightness <= 0. || lightness >= 1. {
        0.
    } else {
        (value - lightness) / lightness.min(1. - lightness)
    };
    gpui::hsla(
        hue,
        hsl_saturation.clamp(0., 1.),
        lightness.clamp(0., 1.),
        1.,
    )
}

impl SettingsView {
    /// テーマのチップの下に置く「色を個別に変える」。開くと選択中のテーマの色をまとまりごとに並べる。
    pub(crate) fn theme_details(&self, settings: &Settings, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let open = self.theme_details_open;
        let overridden: Vec<String> = settings
            .theme_override_pairs(theme.name.as_ref())
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        let toggle = div()
            .id("theme-details-toggle")
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(11.5))
            .text_color(theme.fg1)
            .cursor_pointer()
            .hover(|style| style.text_color(theme.fg0))
            // ▸ と ▾ は字幅が違い、開け閉めで文字がずれる。幅の決まったアイコンにする。
            .child(
                svg()
                    .path(if open {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    })
                    .size(px(12.))
                    .flex_none()
                    .text_color(theme.fg2),
            )
            .child(SharedString::from(i18n::t!("settings.theme_details")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _window, cx| {
                    view.theme_details_open = !view.theme_details_open;
                    if !view.theme_details_open {
                        view.color_editor = None;
                    }
                    cx.notify();
                }),
            );
        let mut details = div().flex().flex_col().gap(px(8.)).child(toggle);
        if !open {
            return details;
        }
        for group in GROUPS {
            let mut rows = div().flex().flex_col().gap(px(2.)).child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(group_label(group))),
            );
            for token in ThemeToken::ALL
                .into_iter()
                .filter(|token| token_group(*token) == group)
            {
                let is_overridden = overridden.iter().any(|key| key == token.key());
                rows = rows.child(self.token_row(token, is_overridden, cx));
                if let Some(editor) = self
                    .color_editor
                    .as_ref()
                    .filter(|editor| editor.token == token)
                {
                    rows = rows.child(self.render_color_editor(editor, cx));
                }
            }
            details = details.child(rows);
        }
        let reset_all = div()
            .id("theme-reset-all")
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .when(overridden.is_empty(), |element| {
                element.text_color(theme.fg2)
            })
            .when(!overridden.is_empty(), |element| {
                element
                    .text_color(theme.fg1)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, _window, cx| view.reset_all_colors(cx)),
                    )
            })
            .child(SharedString::from(i18n::t!("settings.theme_reset_all")));
        details.child(div().flex().child(reset_all))
    }

    /// 1 トークン分の行: 色見本 + 名前 + hex + （上書きしていれば）「リセット」。押すとピッカーを開閉する。
    fn token_row(
        &self,
        token: ThemeToken,
        is_overridden: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let swatch_color = token.effective(&theme);
        let editing = self
            .color_editor
            .as_ref()
            .is_some_and(|editor| editor.token == token);
        div()
            .id(("theme-token", token as usize))
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(6.))
            .py(px(3.))
            .rounded(px(5.))
            .cursor_pointer()
            .when(editing, |element| element.bg(theme.bg3))
            .hover(|style| style.bg(theme.bg3))
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(swatch_color),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .child(SharedString::from(token_label(token))),
            )
            .child(
                div()
                    .w(px(70.))
                    .flex_none()
                    .text_size(px(11.))
                    .font_family(ui::code_font(cx))
                    .text_color(theme.fg2)
                    .child(SharedString::from(theme_core::hex_string(swatch_color))),
            )
            // リセットの欄は上書きしていない行でも空けておく（出し入れで hex の位置を動かさない）。
            .child(
                div()
                    .id(("theme-token-reset", token as usize))
                    .w(px(52.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .py(px(1.))
                    .rounded(px(4.))
                    .text_size(px(10.5))
                    .when(is_overridden, |element| {
                        element
                            .text_color(theme.fg1)
                            .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                            .child(SharedString::from(i18n::t!("settings.theme_reset_one")))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _, _window, cx| {
                                    cx.stop_propagation();
                                    view.reset_color(token, cx);
                                }),
                            )
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| view.toggle_color_editor(token, window, cx)),
            )
            .into_any_element()
    }

    /// トークンのピッカーを開く（同じ行をもう一度押すと閉じる）。hex 入力へフォーカスする。
    fn toggle_color_editor(
        &mut self,
        token: ThemeToken,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .color_editor
            .as_ref()
            .is_some_and(|editor| editor.token == token)
        {
            self.color_editor = None;
            cx.notify();
            return;
        }
        // このトークンだけ上書き前へ戻したテーマ。チャット用トークンの「塗らない」は、他の上書きを
        // 当てたチャット面の色として出るので、上書き前のテーマから直接は引かない。
        let mut restored = self.theme.clone();
        token.restore(&self.theme_base, &mut restored);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.color_editor = Some(ColorEditor::new(
            token,
            token.effective(&self.theme),
            token.effective(&restored),
            focus,
        ));
        cx.notify();
    }

    /// ピッカー本体（枠付きのカード）: 彩度・明度の四角 + 色相のバー + 見本（左 = テーマのデフォルト・
    /// 右 = 今の色）+ hex 入力 + 右上の ×。
    fn render_color_editor(
        &self,
        editor: &ColorEditor,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let pure_hue = gpui::hsla(editor.hue, 1., 0.5, 1.);
        let white = gpui::hsla(0., 0., 1., 1.);
        let black = gpui::hsla(0., 0., 0., 1.);
        let square_bounds = editor.square_bounds.clone();
        let hue_bounds = editor.hue_bounds.clone();
        let marker_x = editor.saturation * SQUARE_WIDTH;
        let marker_y = (1. - editor.value) * SQUARE_HEIGHT;
        let hue_x = editor.hue * SQUARE_WIDTH;
        let dragging = editor.dragging.is_some();
        let view = cx.entity();
        // ドラッグ中だけ窓全体のマウス移動と離す操作を受ける。要素の on_mouse_move はその要素が
        // マウスの下にあると判定された時しか呼ばれず、設定ペインでは移動を受け取れなかった。
        let drag_listener = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if !dragging {
                    return;
                }
                let move_view = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                    if phase == DispatchPhase::Bubble {
                        move_view.update(cx, |view, cx| view.on_color_drag_move(event, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        view.update(cx, |view, cx| view.finish_color_drag(cx));
                    }
                });
            },
        )
        .absolute()
        .size_0();
        let square = div()
            .id("theme-color-square")
            .relative()
            .w(px(SQUARE_WIDTH))
            .h(px(SQUARE_HEIGHT))
            .rounded(px(4.))
            .overflow_hidden()
            .bg(pure_hue)
            .cursor_pointer()
            // 左 → 右で白から純色、上 → 下で透明から黒（HSV の彩度と明度）。
            .child(div().absolute().size_full().bg(linear_gradient(
                90.,
                linear_color_stop(white, 0.),
                linear_color_stop(white.alpha(0.), 1.),
            )))
            .child(div().absolute().size_full().bg(linear_gradient(
                180.,
                linear_color_stop(black.alpha(0.), 0.),
                linear_color_stop(black, 1.),
            )))
            .child(
                canvas(
                    move |bounds, _, _| square_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(marker_x - 5.))
                    .top(px(marker_y - 5.))
                    .size(px(10.))
                    .rounded(px(5.))
                    .border_2()
                    .border_color(white),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    view.start_color_drag(DragTarget::Square, event.position, cx);
                }),
            );
        // 色相は 6 区間の 2 色グラデーションをつなぐ（gpui の線形グラデーションは 2 色まで）。
        let mut hue_bar = div()
            .id("theme-color-hue")
            .relative()
            .flex()
            .w(px(SQUARE_WIDTH))
            .h(px(HUE_HEIGHT))
            .rounded(px(3.))
            .overflow_hidden()
            .cursor_pointer();
        for segment in 0..6 {
            let from = gpui::hsla(segment as f32 / 6., 1., 0.5, 1.);
            let to = gpui::hsla(((segment + 1) as f32 / 6.).min(0.9999), 1., 0.5, 1.);
            hue_bar = hue_bar.child(div().flex_1().h_full().bg(linear_gradient(
                90.,
                linear_color_stop(from, 0.),
                linear_color_stop(to, 1.),
            )));
        }
        let hue_bar = hue_bar
            .child(
                canvas(
                    move |bounds, _, _| hue_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(hue_x - 2.))
                    .top(px(-1.))
                    .w(px(4.))
                    .h(px(HUE_HEIGHT + 2.))
                    .rounded(px(2.))
                    .border_1()
                    .border_color(white),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    view.start_color_drag(DragTarget::Hue, event.position, cx);
                }),
            );
        // 打っていない間は今の色を薄く出す（打った 1 桁目から置き換わる）。
        let (hex_text, hex_color) = if editor.typed.is_empty() {
            let current = theme_core::hex_string(editor.color());
            (current.trim_start_matches('#').to_string(), theme.fg2)
        } else {
            (editor.typed.clone(), theme.fg0)
        };
        let hex_field = div()
            .id("theme-color-hex")
            .track_focus(&editor.focus)
            .on_key_down(cx.listener(Self::on_color_editor_key_down))
            .flex()
            .items_center()
            .w(px(SIDE_WIDTH))
            .h(px(26.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(theme.bg0)
            .border_1()
            .border_color(theme.border)
            .font_family(ui::code_font(cx))
            .text_size(px(12.))
            .child(div().text_color(theme.fg2).child("#"))
            .when(!editor.typed.is_empty(), |field| {
                field.child(
                    div()
                        .text_color(hex_color)
                        .child(SharedString::from(hex_text.clone())),
                )
            })
            .child(div().w(px(1.)).h(px(13.)).bg(self.accent))
            .when(editor.typed.is_empty(), |field| {
                field.child(
                    div()
                        .text_color(hex_color)
                        .child(SharedString::from(hex_text)),
                )
            });
        // 見本は 2 色に分ける: 左 = テーマのデフォルト（クリックで戻す）・右 = 今の色。
        let token = editor.token;
        let preview = div()
            .flex()
            .w(px(SIDE_WIDTH))
            .h(px(36.))
            .rounded(px(6.))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("theme-color-default")
                    .flex_1()
                    .h_full()
                    .bg(editor.default)
                    .cursor_pointer()
                    .tooltip(ui::Tooltip::text(
                        i18n::t!("settings.theme_color_default"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _window, cx| view.reset_color(token, cx)),
                    ),
            )
            .child(div().flex_1().h_full().bg(editor.color()));
        let close = div()
            .id("theme-color-close")
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .group("theme-color-close")
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3))
            .tooltip(ui::Tooltip::text(
                i18n::t!("settings.theme_color_close"),
                theme.clone(),
            ))
            // svg は親の文字色を引き継がない（自分の text_color が無いと描かれない）。
            .child(
                svg()
                    .path("icons/x.svg")
                    .size(px(12.))
                    .text_color(theme.fg2)
                    .group_hover("theme-color-close", |style| style.text_color(theme.fg0)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _window, cx| {
                    view.color_editor = None;
                    cx.notify();
                }),
            );
        div()
            .relative()
            .flex()
            // 幅が足りない時は、見本と hex 入力を四角の下へ折り返す。
            .flex_wrap()
            .gap_x(px(14.))
            .gap_y(px(10.))
            .ml(px(30.))
            .mt(px(4.))
            .mb(px(8.))
            .p(px(12.))
            .pr(px(36.))
            .rounded(px(8.))
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .child(drag_listener)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(square)
                    .child(hue_bar),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(preview)
                    .child(hex_field),
            )
            .child(close)
            .into_any_element()
    }

    fn start_color_drag(
        &mut self,
        target: DragTarget,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.color_editor.as_mut() {
            editor.dragging = Some(target);
            editor.pick_at(target, position);
            cx.notify();
        }
    }

    /// ドラッグ中の移動（窓全体で受ける＝四角の外へ出ても追う）。見本だけを動かし、
    /// 保存は離した時の 1 回にする（動かすたびに settings.json を書かない）。
    fn on_color_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(target) = self
            .color_editor
            .as_ref()
            .and_then(|editor| editor.dragging)
        else {
            return;
        };
        // 窓の外で離した後に届いた move は、離した扱いにする。
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_color_drag(cx);
            return;
        }
        if let Some(editor) = self.color_editor.as_mut() {
            editor.pick_at(target, event.position);
            cx.notify();
        }
    }

    /// ドラッグの終わり（窓のどこで離しても届く）。決まった色を保存する。
    fn finish_color_drag(&mut self, cx: &mut Context<Self>) {
        let dragging = self
            .color_editor
            .as_mut()
            .and_then(|editor| editor.dragging.take())
            .is_some();
        if dragging {
            self.commit_color(cx);
        }
    }

    /// hex 入力欄のキー: 16 進を 8 桁まで・⌘V で貼り付け・Enter で保存・Esc で閉じる。
    fn on_color_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.color_editor.as_mut() else {
            return;
        };
        let modifiers = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => self.color_editor = None,
            "enter" => {
                // 6 桁か 8 桁そろった時だけ保存する（3 桁の略記や打ちかけは受けない）。
                if let Some(color) = editor.typed_color() {
                    editor.show_color(color);
                    self.commit_color(cx);
                }
            }
            "backspace" => {
                editor.typed.pop();
            }
            "v" if modifiers.secondary() => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    editor.typed.clear();
                    editor.type_digits(&text);
                }
            }
            _ => {
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                // key_char の無い打鍵（テストの打鍵など）は、1 文字のキー名をそのまま文字として扱う。
                let text = match &event.keystroke.key_char {
                    Some(text) => text.clone(),
                    None if event.keystroke.key.chars().count() == 1 => event.keystroke.key.clone(),
                    None => return,
                };
                editor.type_digits(&text);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// ピッカーの色を `theme_overrides.<テーマ名>.<トークン>` へ保存する（全ビューへ即反映）。
    /// テーマのデフォルトと同じ色なら、上書きを消す（同じ色の「リセット」を残さない）。
    fn commit_color(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.color_editor.as_mut() else {
            return;
        };
        editor.typed.clear();
        let color = theme_core::hex_string(editor.color());
        let value = (color != theme_core::hex_string(editor.default)).then_some(color);
        let key = editor.token.key();
        let theme_name = self.theme.name.to_string();
        let result = set_theme_override(cx, &theme_name, key, value.as_deref());
        self.report_save(result, cx);
        cx.notify();
    }

    /// 1 トークンをテーマのデフォルトへ戻す。そのトークンのピッカーが開いていれば、デフォルトの色へ合わせる。
    fn reset_color(&mut self, token: ThemeToken, cx: &mut Context<Self>) {
        let theme_name = self.theme.name.to_string();
        let result = set_theme_override(cx, &theme_name, token.key(), None);
        self.report_save(result, cx);
        if let Some(editor) = self
            .color_editor
            .as_mut()
            .filter(|editor| editor.token == token)
        {
            let default = editor.default;
            editor.show_color(default);
        }
        cx.notify();
    }

    /// 選択中のテーマの色をすべてテーマのデフォルトへ戻す。
    fn reset_all_colors(&mut self, cx: &mut Context<Self>) {
        let theme_name = self.theme.name.to_string();
        let result = forget_theme_overrides(cx, &theme_name);
        self.report_save(result, cx);
        self.color_editor = None;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size, Modifiers};
    use theme_core::Theme;

    /// 外観ページで ChatStepBg のピッカーを開いた設定ビューを作る（user の設定は一時ファイル）。
    fn open_picker<'a>(
        cx: &'a mut gpui::TestAppContext,
        root: &std::path::Path,
    ) -> (
        gpui::Entity<SettingsView>,
        &'a mut gpui::VisualTestContext,
        std::path::PathBuf,
    ) {
        std::fs::remove_dir_all(root).ok();
        std::fs::create_dir_all(root).expect("作れる");
        let user = root.join("settings.json");
        std::fs::write(&user, r#"{ "onboarded": true }"#).expect("書ける");
        cx.update(|cx| crate::init(Some(user.clone()), None, cx));
        let (view, cx) = cx.add_window_view(|_window, cx| {
            let mut view = SettingsView::new(Theme::dark(), gpui::red(), cx);
            view.availability_pending = false;
            view.page = crate::SettingsPage::Appearance;
            view.theme_details_open = true;
            view
        });
        cx.simulate_resize(size(px(1400.), px(4000.)));
        view.update_in(cx, |view, window, cx| {
            view.toggle_color_editor(ThemeToken::ChatStepBg, window, cx)
        });
        cx.run_until_parked();
        (view, cx, user)
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "necoder-theme-colors-{name}-{}",
            std::process::id()
        ))
    }

    fn written_override(user: &std::path::Path) -> Option<String> {
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(user).expect("読める")).expect("JSON");
        written["theme_overrides"]["necoder-dark"]["chat_step_bg"]
            .as_str()
            .map(str::to_string)
    }

    #[gpui::test]
    fn dragging_in_the_square_moves_the_marker_and_saves_on_release(cx: &mut gpui::TestAppContext) {
        let root = temp_root("square");
        let (view, cx, user) = open_picker(cx, &root);
        let bounds = view
            .read_with(cx, |view, _| {
                view.color_editor
                    .as_ref()
                    .and_then(|editor| editor.square_bounds.get())
            })
            .expect("四角が描かれている");
        let start = bounds.origin + point(px(10.), px(10.));
        let end = bounds.origin + point(bounds.size.width - px(10.), bounds.size.height - px(10.));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
        let (saturation, value) = view.read_with(cx, |view, _| {
            let editor = view.color_editor.as_ref().expect("開いている");
            (editor.saturation, editor.value)
        });
        assert!(
            saturation > 0.9,
            "右へドラッグすると彩度が上がる: {saturation}"
        );
        assert!(value < 0.1, "下へドラッグすると明度が下がる: {value}");
        assert_eq!(written_override(&user), None, "ドラッグ中は保存しない");
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        assert!(written_override(&user).is_some(), "離したら保存する");
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn dragging_the_hue_bar_changes_the_hue(cx: &mut gpui::TestAppContext) {
        let root = temp_root("hue");
        let (view, cx, user) = open_picker(cx, &root);
        let bounds = view
            .read_with(cx, |view, _| {
                view.color_editor
                    .as_ref()
                    .and_then(|editor| editor.hue_bounds.get())
            })
            .expect("色相のバーが描かれている");
        let middle_y = bounds.size.height / 2.;
        cx.simulate_mouse_down(
            bounds.origin + point(px(2.), middle_y),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.simulate_mouse_move(
            bounds.origin + point(bounds.size.width / 2., middle_y),
            MouseButton::Left,
            Modifiers::none(),
        );
        let hue = view.read_with(cx, |view, _| {
            view.color_editor.as_ref().expect("開いている").hue
        });
        assert!(
            (hue - 0.5).abs() < 0.05,
            "真ん中までドラッグすると色相は半分: {hue}"
        );
        cx.simulate_mouse_up(
            bounds.origin + point(bounds.size.width / 2., middle_y),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert!(written_override(&user).is_some(), "離したら保存する");
        std::fs::remove_dir_all(&root).ok();
    }

    #[gpui::test]
    fn typing_a_hex_replaces_the_shown_color_and_saves_on_enter(cx: &mut gpui::TestAppContext) {
        let root = temp_root("typing");
        let (view, cx, user) = open_picker(cx, &root);
        cx.simulate_keystrokes("1 2 3 4 5 6 enter");
        assert_eq!(written_override(&user).as_deref(), Some("#123456"));
        // テーマのデフォルトと同じ色を打つと、上書きは消える。
        let default = view.read_with(cx, |view, _| {
            theme_core::hex_string(view.color_editor.as_ref().expect("開いている").default)
        });
        let digits: Vec<String> = default
            .trim_start_matches('#')
            .chars()
            .map(|character| character.to_string())
            .collect();
        cx.simulate_keystrokes(&format!("{} enter", digits.join(" ")));
        assert_eq!(written_override(&user), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hsv_round_trips_through_hsla() {
        for hex in [
            0x0f1a24_u32,
            0x1c1a14,
            0xff5252,
            0xffffff,
            0x000000,
            0x7d9bd8,
        ] {
            let color = theme_core::color_from_hex(hex);
            let (hue, saturation, value) = hsv_from_color(color);
            let back = color_from_hsv(hue, saturation, value);
            assert_eq!(theme_core::hex_from_color(back), hex, "{hex:06x}");
        }
    }

    #[test]
    fn hsv_corners_are_white_black_and_pure_hue() {
        assert_eq!(
            theme_core::hex_from_color(color_from_hsv(0.3, 0., 1.)),
            0xffffff
        );
        assert_eq!(
            theme_core::hex_from_color(color_from_hsv(0.3, 1., 0.)),
            0x000000
        );
        assert_eq!(
            theme_core::hex_from_color(color_from_hsv(0., 1., 1.)),
            0xff0000
        );
    }

    #[test]
    fn every_token_belongs_to_a_shown_group() {
        for token in ThemeToken::ALL {
            assert!(GROUPS.contains(&token_group(token)), "{token:?}");
        }
    }
}
