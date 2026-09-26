//! ターミナルの見た目の設定（O25）— 文字の大きさ・フォント・scrollback・カーソルの形。
//!
//! terminal_view は設定の crate を知らない（依存の向き）。workspace が settings.json から
//! [`TerminalAppearance`] を作って global に置き、変わったら置き直す。開いている端末は
//! global を見張っていて、その場で描き直す（scrollback とカーソルは term に入れ直す）。
//! global が無ければ既定（設定を持つ前と同じ見た目）。

use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use gpui::{App, Global, SharedString};

/// 文字の大きさの既定（pt）。エディタより少し小さい。
pub const DEFAULT_FONT_SIZE: f32 = 12.5;
/// 既定のフォント（同梱のコードの書体・等幅）。workspace はコードの書体の設定をここへ渡すので、
/// これが効くのは設定が何も無い時だけ。
pub const DEFAULT_FONT_FAMILY: &str = ui::fonts::DEFAULT_CODE_FONT;
/// 遡れる行数の既定。
pub const DEFAULT_SCROLLBACK: usize = 10_000;
/// 文字の大きさの範囲（設定画面の ± と、手で書いた値の丸め）。
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 32.0;
/// scrollback の上限（1 行ごとにセルを持つので、大きすぎるとメモリを食う）。
pub const MAX_SCROLLBACK: usize = 100_000;
/// 行の高さ ÷ 文字の大きさ（既定の 12.5pt → 17px と同じ比）。
const LINE_HEIGHT_RATIO: f32 = 17.0 / DEFAULT_FONT_SIZE;

/// カーソルの形の既定。アプリ（vim・シェルの設定）が DECSCUSR で指定すればそちらが勝つ。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCursor {
    #[default]
    Block,
    Bar,
    Underline,
}

impl TerminalCursor {
    /// 設定の値（`block` / `bar` / `underline`）から。知らない値は既定。
    pub fn from_setting(value: &str) -> Self {
        match value {
            "bar" => Self::Bar,
            "underline" => Self::Underline,
            _ => Self::Block,
        }
    }

    fn shape(self) -> CursorShape {
        match self {
            Self::Block => CursorShape::Block,
            Self::Bar => CursorShape::Beam,
            Self::Underline => CursorShape::Underline,
        }
    }
}

/// 取り込んだ配色（O25・Ghostty / Windows Terminal / iTerm2 の配色ファイルから workspace が読む）。
/// `None` の色は既定のまま（ANSI は VSCode 系の 16 色・文字と背景はアプリのテーマ）。カーソルの色は
/// 取り込まない（キャレットはプロジェクトの識別色・UI-SPEC）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalColors {
    /// ANSI 0〜15（normal 8 + bright 8）。
    pub palette: [Option<(u8, u8, u8)>; 16],
    pub foreground: Option<(u8, u8, u8)>,
    pub background: Option<(u8, u8, u8)>,
}

/// ターミナルの見た目（workspace が設定から作る global）。
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalAppearance {
    pub font_size: f32,
    pub font_family: SharedString,
    pub scrollback: usize,
    pub cursor: TerminalCursor,
    /// 取り込んだ配色（無ければ既定）。
    pub colors: TerminalColors,
}

impl Global for TerminalAppearance {}

impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            font_family: SharedString::new_static(DEFAULT_FONT_FAMILY),
            scrollback: DEFAULT_SCROLLBACK,
            cursor: TerminalCursor::default(),
            colors: TerminalColors::default(),
        }
    }
}

impl TerminalAppearance {
    /// 設定の値から作る。範囲の外は端へ丸め、空のフォントは既定にする（手で書いた値でも崩れない）。
    pub fn new(font_size: f32, font_family: &str, scrollback: usize, cursor: &str) -> Self {
        let font_size = if font_size.is_finite() {
            font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
        } else {
            DEFAULT_FONT_SIZE
        };
        let font_family = match font_family.trim() {
            "" => SharedString::new_static(DEFAULT_FONT_FAMILY),
            family => SharedString::from(family.to_string()),
        };
        Self {
            font_size,
            font_family,
            scrollback: scrollback.min(MAX_SCROLLBACK),
            cursor: TerminalCursor::from_setting(cursor),
            colors: TerminalColors::default(),
        }
    }

    /// 取り込んだ配色を載せる。
    pub fn with_colors(mut self, colors: TerminalColors) -> Self {
        self.colors = colors;
        self
    }

    /// 行の高さ（px・整数に丸める＝行の境目がにじまない）。
    pub fn line_height(&self) -> f32 {
        (self.font_size * LINE_HEIGHT_RATIO).round()
    }

    /// いまの見た目（global が無ければ既定）。
    pub fn current(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }

    /// term に入れる形（scrollback とカーソルの既定）。
    pub(crate) fn cursor_style(&self) -> CursorStyle {
        CursorStyle {
            shape: self.cursor.shape(),
            blinking: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_looks_like_before() {
        let appearance = TerminalAppearance::default();
        assert_eq!(appearance.font_size, 12.5);
        assert_eq!(appearance.line_height(), 17.0, "設定を持つ前の行の高さ");
        assert_eq!(appearance.font_family.as_ref(), "Guguru Sans Code");
        assert_eq!(appearance.scrollback, 10_000);
        assert_eq!(appearance.cursor, TerminalCursor::Block);
    }

    #[test]
    fn written_values_are_kept_in_range() {
        let appearance = TerminalAppearance::new(200.0, "  ", 5_000_000, "bar");
        assert_eq!(appearance.font_size, MAX_FONT_SIZE);
        assert_eq!(
            appearance.font_family.as_ref(),
            DEFAULT_FONT_FAMILY,
            "空は既定"
        );
        assert_eq!(appearance.scrollback, MAX_SCROLLBACK);
        assert_eq!(appearance.cursor, TerminalCursor::Bar);
        assert_eq!(
            TerminalAppearance::new(1.0, "Menlo", 0, "?").font_size,
            MIN_FONT_SIZE
        );
        assert_eq!(
            TerminalAppearance::new(f32::NAN, "Menlo", 0, "underline").font_size,
            DEFAULT_FONT_SIZE
        );
        assert_eq!(TerminalAppearance::new(16.0, "", 0, "").line_height(), 22.0);
    }
}
