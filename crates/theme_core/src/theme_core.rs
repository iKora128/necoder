//! theme_core — Shirushi の「きせかえ」土台。
//!
//! UI-SPEC §1 のデザイントークンを型に写したもの。ここが「テーマの全インターフェース」
//! （UI-SPEC §1.1「きせかえ契約」）であり、ユーザー定義テーマ = この各トークンを上書きする
//! JSON 1 枚（読み込みは M3）。
//!
//! 独立した 2 軸を分けて扱う（UI-SPEC §1.1 / §1.2）:
//! - **テーマ**: 面と文字の配色（[`Theme`]）。dark / light を内蔵。
//! - **プロジェクト色 / スレッド色**: 識別のための巡回パレット（[`project_color`] / [`thread_color`]）。
//!   どのテーマでも独立に流れる。
//!
//! 依存方向（ARCHITECTURE §1）: foundation 層。gpui（外部）にのみ依存し、上位 crate から参照される。

use anyhow::{Context as _, Result};
use gpui::{Hsla, Rgba, SharedString, rgb};
use std::path::PathBuf;

/// 面が明るいか暗いか。既定テーマの選択や syn-* の前提に使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// UI-SPEC §1.1 のシンタックストークン（syn-*）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    pub keyword: Hsla,     // syn-kw
    pub function: Hsla,    // syn-fn
    pub type_: Hsla,       // syn-type
    pub string: Hsla,      // syn-str
    pub number: Hsla,      // syn-num
    pub comment: Hsla,     // syn-cmt（italic 表示は描画側の責務）
    pub macro_: Hsla,      // syn-mac
    pub punctuation: Hsla, // syn-punct
}

/// UI-SPEC §1.1 のデザイントークン一式（1 テーマ分）。フィールドは token 表と 1:1。
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: SharedString,
    pub appearance: Appearance,

    pub bg0: Hsla, // 窓クローム・ドック
    pub bg1: Hsla, // エディタ面・チャット面
    pub bg2: Hsla, // カード・パレット・入力箱
    pub bg3: Hsla, // hover・中立の選択面

    pub fg0: Hsla, // 主文字
    pub fg1: Hsla, // 副文字
    pub fg2: Hsla, // ミュート・行番号

    pub border: Hsla, // 全罫線（1px）

    pub ok: Hsla,   // 診断・git（成功 / 追加）
    pub warn: Hsla, // 診断・git（警告 / 変更）
    pub err: Hsla,  // 診断・git（エラー）

    pub syntax: SyntaxColors,
}

/// 組み込み dark テーマの名前。
pub const DARK_THEME_NAME: &str = "shirushi-dark";
/// 組み込み light テーマの名前。
pub const LIGHT_THEME_NAME: &str = "shirushi-light";

impl Theme {
    /// dark テーマ（既定）。値は UI-SPEC §1.1 の dark 列。
    pub fn dark() -> Theme {
        Theme {
            name: DARK_THEME_NAME.into(),
            appearance: Appearance::Dark,
            bg0: hex(0x16181e),
            bg1: hex(0x1b1e25),
            bg2: hex(0x232733),
            bg3: hex(0x2b3040),
            fg0: hex(0xd7dae3),
            fg1: hex(0x9aa1b2),
            fg2: hex(0x646b7a),
            border: hex(0x262b36),
            ok: hex(0x7bc96f),
            warn: hex(0xe5c07b),
            err: hex(0xe06c75),
            syntax: SyntaxColors {
                keyword: hex(0xc678dd),
                function: hex(0x61afef),
                type_: hex(0xe5c07b),
                string: hex(0x98c379),
                number: hex(0xd19a66),
                comment: hex(0x5f6672),
                macro_: hex(0x56b6c2),
                punctuation: hex(0xabb2bf),
            },
        }
    }

    /// light テーマ。値は UI-SPEC §1.1 の light 列。
    /// M2 時点では値を用意するだけ（テーマセレクタ / ライブプレビューは M3）。
    pub fn light() -> Theme {
        Theme {
            name: LIGHT_THEME_NAME.into(),
            appearance: Appearance::Light,
            bg0: hex(0xf0f1f4),
            bg1: hex(0xffffff),
            bg2: hex(0xffffff),
            bg3: hex(0xe7eaf0),
            fg0: hex(0x333845),
            fg1: hex(0x5c6370),
            fg2: hex(0x9aa1b2),
            border: hex(0xd8dce3),
            ok: hex(0x3e9d32),
            warn: hex(0xbf8803),
            err: hex(0xe45649),
            syntax: SyntaxColors {
                keyword: hex(0xa626a4),
                function: hex(0x4078f2),
                type_: hex(0xc18401),
                string: hex(0x50a14f),
                number: hex(0x986801),
                comment: hex(0xa3a6ae),
                macro_: hex(0x0184bc),
                punctuation: hex(0x383a42),
            },
        }
    }

    /// 名前から組み込みテーマを引く。未知の名前は `None`。
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            DARK_THEME_NAME => Some(Theme::dark()),
            LIGHT_THEME_NAME => Some(Theme::light()),
            _ => None,
        }
    }

    /// [`ThemeSource`] からテーマを読み込む。
    /// ユーザーテーマ（トークン上書き JSON）の読み込みは M3 で実装するため、
    /// 現状は未実装であることを明示的にエラーで返す（黙って握り潰さない）。
    pub fn load(source: &ThemeSource) -> Result<Theme> {
        match source {
            ThemeSource::BuiltIn(name) => {
                Theme::builtin(name).with_context(|| format!("組み込みテーマが存在しない: {name}"))
            }
            ThemeSource::User(path) => anyhow::bail!(
                "ユーザーテーマ JSON の読み込みは未実装（M3 予定）: {}",
                path.display()
            ),
        }
    }

    /// フォルダアイコンの塗り（UI-SPEC §1.1 固定色）。
    /// ベース `#7d9bd8` を**アクセント非依存**で現在の bg3 に 55% 重ねる（mock: `color-mix in srgb`）。
    /// bg3 はテーマの面なので、テーマを跨いでフォルダ色が馴染む。
    pub fn folder_icon(&self) -> Hsla {
        mix_srgb(rgb(FOLDER_ICON_BASE), self.bg3.to_rgb(), 0.55).into()
    }
}

/// テーマの出所。組み込み or ユーザー JSON（ARCHITECTURE §3）。
#[derive(Clone, Debug, PartialEq)]
pub enum ThemeSource {
    BuiltIn(&'static str),
    User(PathBuf),
}

/// プロジェクトの識別（レール項目・ピル左縁 等に流れる）。UI-SPEC §1.2 / §2。
/// 色の優先順は `.shirushi/settings.json` の `color` > 手動選択 > パレット巡回（解決は M3）。
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectIdentity {
    pub color: Hsla,
    pub icon: IconSource,
}

/// レール項目に出す図像の出所。優先順は `.shirushi/settings.json` の `icon` > プロジェクト名の頭文字（UI-SPEC §2）。
#[derive(Clone, Debug, PartialEq)]
pub enum IconSource {
    Monogram(char), // プロジェクト名の頭文字（例: 印）
    Emoji(String),
    Image(PathBuf),
}

// ── テーマ非連動の固定色（UI-SPEC §1.1）。どのテーマでも同じ値になる ──

/// エディタ選択面のベース色（`#7d9bd8`）。実際の塗りは [`editor_selection`]。
pub const EDITOR_SELECTION_BASE: u32 = 0x7d9bd8;
/// フォルダアイコンのベース色（`#7d9bd8`）。bg3 と混ぜて使う（[`Theme::folder_icon`]）。
pub const FOLDER_ICON_BASE: u32 = 0x7d9bd8;
/// Claude バレット（⏺）のテラコッタ（`#d97757`）。バレット以外に使わない（UI-SPEC §1.3）。
pub const CLAUDE_BULLET: u32 = 0xd97757;

/// エディタの選択面。ベース `#7d9bd8` を alpha 0.28 で塗る（テーマ非依存）。
pub fn editor_selection() -> Hsla {
    hex(EDITOR_SELECTION_BASE).alpha(0.28)
}

/// Claude バレット色。
pub fn claude_bullet() -> Hsla {
    hex(CLAUDE_BULLET)
}

// ── アイデンティティ色の巡回パレット（UI-SPEC §1.2）。テーマと独立の 2 軸目 ──

/// プロジェクト色パレット（自動巡回）: indigo → teal → amber → rose → green。
pub const PROJECT_COLOR_HEXES: [u32; 5] = [0x7c8cf8, 0x34d3b6, 0xf0a24b, 0xef7d9b, 0x85c46c];
/// スレッド色パレット（自動巡回）。プロジェクト色と独立。
pub const THREAD_COLOR_HEXES: [u32; 3] = [0x61afef, 0xe5c07b, 0xc678dd];

/// `index` 番目のプロジェクト色（パレットを巡回）。色優先順の最下段（自動巡回）に当たる。
pub fn project_color(index: usize) -> Hsla {
    hex(PROJECT_COLOR_HEXES[index % PROJECT_COLOR_HEXES.len()])
}

/// `index` 番目のスレッド色（パレットを巡回）。
pub fn thread_color(index: usize) -> Hsla {
    hex(THREAD_COLOR_HEXES[index % THREAD_COLOR_HEXES.len()])
}

/// アクセント（プロジェクト色 / スレッド色）の薄面。UI-SPEC §1.2: 16% 透過。
/// パレット選択面・タイル選択輪郭のみに使う（許可リスト §1.3）。
pub fn accent_dim(accent: Hsla) -> Hsla {
    accent.alpha(0.16)
}

// ── 内部ヘルパ ──

/// RGB hex（`0xRRGGBB`）を不透明な [`Hsla`] に変換する。
fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// sRGB 空間での 2 色の線形補間（CSS `color-mix(in srgb, a `ratio`, b)` 相当）。
/// `ratio` は `a` の割合。結果は不透明。
fn mix_srgb(a: Rgba, b: Rgba, ratio: f32) -> Rgba {
    let inverse = 1.0 - ratio;
    Rgba {
        r: a.r * ratio + b.r * inverse,
        g: a.g * ratio + b.g * inverse,
        b: a.b * ratio + b.b * inverse,
        a: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストは実装ヘルパに依存せず、gpui の変換パスで期待値を作る。
    fn h(value: u32) -> Hsla {
        rgb(value).into()
    }

    #[test]
    fn dark_tokens_match_spec() {
        let theme = Theme::dark();
        assert_eq!(theme.appearance, Appearance::Dark);
        assert_eq!(theme.bg0, h(0x16181e));
        assert_eq!(theme.bg1, h(0x1b1e25));
        assert_eq!(theme.bg2, h(0x232733));
        assert_eq!(theme.bg3, h(0x2b3040));
        assert_eq!(theme.fg0, h(0xd7dae3));
        assert_eq!(theme.fg2, h(0x646b7a));
        assert_eq!(theme.border, h(0x262b36));
        assert_eq!(theme.err, h(0xe06c75));
        assert_eq!(theme.syntax.keyword, h(0xc678dd));
        assert_eq!(theme.syntax.macro_, h(0x56b6c2));
        assert_eq!(theme.syntax.punctuation, h(0xabb2bf));
    }

    #[test]
    fn light_tokens_match_spec() {
        let theme = Theme::light();
        assert_eq!(theme.appearance, Appearance::Light);
        assert_eq!(theme.bg0, h(0xf0f1f4));
        assert_eq!(theme.bg1, h(0xffffff));
        assert_eq!(theme.fg0, h(0x333845));
        assert_eq!(theme.syntax.keyword, h(0xa626a4));
        assert_eq!(theme.syntax.punctuation, h(0x383a42));
    }

    #[test]
    fn dark_and_light_surfaces_differ() {
        assert_ne!(Theme::dark().bg0, Theme::light().bg0);
        assert_ne!(Theme::dark().fg0, Theme::light().fg0);
    }

    #[test]
    fn project_palette_cycles() {
        assert_eq!(project_color(0), h(0x7c8cf8));
        assert_eq!(project_color(4), h(0x85c46c));
        // 5 個で 1 周
        assert_eq!(project_color(5), project_color(0));
        assert_eq!(project_color(11), project_color(1));
    }

    #[test]
    fn thread_palette_cycles() {
        assert_eq!(thread_color(0), h(0x61afef));
        assert_eq!(thread_color(2), h(0xc678dd));
        // 3 個で 1 周
        assert_eq!(thread_color(3), thread_color(0));
    }

    #[test]
    fn accent_dim_only_lowers_alpha() {
        let base = h(0x7c8cf8);
        let dim = accent_dim(base);
        assert!((dim.a - 0.16).abs() < 1e-6);
        // 色相・彩度・明度は据え置き（面だけ薄くする）
        assert_eq!((dim.h, dim.s, dim.l), (base.h, base.s, base.l));
    }

    #[test]
    fn editor_selection_is_28_percent() {
        assert!((editor_selection().a - 0.28).abs() < 1e-6);
    }

    #[test]
    fn folder_icon_mixes_between_base_and_bg3() {
        let dark = Theme::dark();
        let icon = dark.folder_icon().to_rgb();
        let base = rgb(FOLDER_ICON_BASE);
        let bg3 = dark.bg3.to_rgb();
        assert!((icon.a - 1.0).abs() < 1e-6);
        // 各チャネルがベースと bg3 の間に収まる（55% / 45% の混色）
        for (mixed, from, to) in [
            (icon.r, base.r, bg3.r),
            (icon.g, base.g, bg3.g),
            (icon.b, base.b, bg3.b),
        ] {
            let low = from.min(to);
            let high = from.max(to);
            assert!(mixed >= low - 1e-6 && mixed <= high + 1e-6);
        }
    }

    #[test]
    fn load_resolves_builtins_and_reports_errors() {
        assert_eq!(Theme::builtin(DARK_THEME_NAME), Some(Theme::dark()));
        assert_eq!(Theme::builtin(LIGHT_THEME_NAME), Some(Theme::light()));
        assert_eq!(Theme::builtin("nope"), None);

        assert!(Theme::load(&ThemeSource::BuiltIn(DARK_THEME_NAME)).is_ok());
        assert!(Theme::load(&ThemeSource::BuiltIn("nope")).is_err());
        // ユーザーテーマ JSON は M3 まで未実装 → エラー
        assert!(Theme::load(&ThemeSource::User(PathBuf::from("/x/theme.json"))).is_err());
    }
}
