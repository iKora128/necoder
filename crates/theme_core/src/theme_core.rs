//! theme_core — necoder の「きせかえ」土台。
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
use gpui::{rgb, Hsla, Rgba, SharedString};
use serde::Deserialize;
use std::path::{Path, PathBuf};

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
pub const DARK_THEME_NAME: &str = "necoder-dark";
/// 組み込み light テーマの名前。
pub const LIGHT_THEME_NAME: &str = "necoder-light";

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

    /// 名前から組み込みテーマを引く。dark / light の 2 種に加え、同梱テーマ（[`embedded_themes`]）も
    /// id（例: `solarized-dark`）と表示名（例: `Solarized Dark`）のどちらでも引ける。未知の名前は `None`。
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            DARK_THEME_NAME => Some(Theme::dark()),
            LIGHT_THEME_NAME => Some(Theme::light()),
            _ => embedded_themes()
                .iter()
                .find(|(id, theme)| *id == name || theme.name.as_ref() == name)
                .map(|(_, theme)| theme.clone()),
        }
    }

    /// [`ThemeSource`] からテーマを読み込む。
    /// ユーザーテーマは「トークン上書き JSON」1 枚（欠けたキーは `appearance` に応じた組み込みへフォールバック）。
    pub fn load(source: &ThemeSource) -> Result<Theme> {
        match source {
            ThemeSource::BuiltIn(name) => {
                Theme::builtin(name).with_context(|| format!("組み込みテーマが存在しない: {name}"))
            }
            ThemeSource::User(path) => load_user_theme(path),
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

// ── 同梱テーマ（バイナリ埋め込み JSON）──

/// 同梱テーマの一覧（id, JSON 本文）。形式はユーザーテーマと**同一**の「トークン上書き JSON」＝
/// 同梱ファイルがそのまま自作テーマの実例になる。id は settings.json の theme 名としても使える固定名。
/// パレット由来（値の採用のみ・コード移植ではない）: Solarized = Ethan Schoonover（MIT）/
/// Gruvbox = morhetz（MIT）/ Catppuccin Mocha = Catppuccin project（MIT）/ High Contrast は自作。
const EMBEDDED_THEME_JSONS: &[(&str, &str)] = &[
    (
        "solarized-dark",
        include_str!("../themes/solarized-dark.json"),
    ),
    (
        "solarized-light",
        include_str!("../themes/solarized-light.json"),
    ),
    ("gruvbox-dark", include_str!("../themes/gruvbox-dark.json")),
    (
        "catppuccin-mocha",
        include_str!("../themes/catppuccin-mocha.json"),
    ),
    (
        "high-contrast-dark",
        include_str!("../themes/high-contrast-dark.json"),
    ),
];

/// 同梱テーマを一度だけパースして返す。JSON はビルド時に埋め込まれており、壊れは
/// `embedded_themes_parse` テストが検知する（実行時は該当テーマだけ落として続行）。
fn embedded_themes() -> &'static [(&'static str, Theme)] {
    static CELL: std::sync::OnceLock<Vec<(&'static str, Theme)>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| {
        EMBEDDED_THEME_JSONS
            .iter()
            .filter_map(|(id, json)| match parse_theme_json(json) {
                Ok(theme) => Some((*id, theme)),
                Err(error) => {
                    eprintln!("同梱テーマが壊れている（スキップ）: {id}: {error:#}");
                    None
                }
            })
            .collect()
    })
}

// ── ユーザーテーマ JSON（トークン上書き）──

/// ユーザーテーマ JSON を読み、`appearance`（dark/light）を土台に指定トークンを上書きして返す。
/// 欠けたキーは土台のまま＝「上書き JSON」（UI-SPEC §1.1「きせかえ契約」）。
fn load_user_theme(path: &Path) -> Result<Theme> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("テーマ JSON を読めない: {}", path.display()))?;
    parse_theme_json(&text).with_context(|| format!("テーマ JSON の解析に失敗: {}", path.display()))
}

/// テーマ JSON 本文をパースする（同梱テーマ・ユーザーテーマ共通）。
fn parse_theme_json(text: &str) -> Result<Theme> {
    let overrides: ThemeOverrides =
        serde_json::from_str(text).context("テーマ JSON の形式が不正")?;
    Ok(overrides.into_theme())
}

/// トークン上書き JSON（全フィールド任意）。色は `#rrggbb` / `#rrggbbaa`。
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThemeOverrides {
    name: Option<String>,
    appearance: Option<String>,
    bg0: Option<String>,
    bg1: Option<String>,
    bg2: Option<String>,
    bg3: Option<String>,
    fg0: Option<String>,
    fg1: Option<String>,
    fg2: Option<String>,
    border: Option<String>,
    ok: Option<String>,
    warn: Option<String>,
    err: Option<String>,
    syntax: Option<SyntaxOverrides>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SyntaxOverrides {
    keyword: Option<String>,
    function: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    string: Option<String>,
    number: Option<String>,
    comment: Option<String>,
    #[serde(rename = "macro")]
    macro_: Option<String>,
    punctuation: Option<String>,
}

impl ThemeOverrides {
    fn into_theme(self) -> Theme {
        // 土台は appearance で選ぶ（既定 dark）。
        let mut theme = match self.appearance.as_deref() {
            Some("light") => Theme::light(),
            _ => Theme::dark(),
        };
        if let Some(name) = self.name {
            theme.name = name.into();
        }
        override_color(&mut theme.bg0, self.bg0);
        override_color(&mut theme.bg1, self.bg1);
        override_color(&mut theme.bg2, self.bg2);
        override_color(&mut theme.bg3, self.bg3);
        override_color(&mut theme.fg0, self.fg0);
        override_color(&mut theme.fg1, self.fg1);
        override_color(&mut theme.fg2, self.fg2);
        override_color(&mut theme.border, self.border);
        override_color(&mut theme.ok, self.ok);
        override_color(&mut theme.warn, self.warn);
        override_color(&mut theme.err, self.err);
        if let Some(syntax) = self.syntax {
            override_color(&mut theme.syntax.keyword, syntax.keyword);
            override_color(&mut theme.syntax.function, syntax.function);
            override_color(&mut theme.syntax.type_, syntax.type_);
            override_color(&mut theme.syntax.string, syntax.string);
            override_color(&mut theme.syntax.number, syntax.number);
            override_color(&mut theme.syntax.comment, syntax.comment);
            override_color(&mut theme.syntax.macro_, syntax.macro_);
            override_color(&mut theme.syntax.punctuation, syntax.punctuation);
        }
        theme
    }
}

/// `value` が有効な hex なら `field` を上書き（不正・欠損は土台のまま）。
fn override_color(field: &mut Hsla, value: Option<String>) {
    if let Some(color) = value.as_deref().and_then(parse_hex) {
        *field = color;
    }
}

/// `#rrggbb` / `#rrggbbaa`（`#` 省略可）を [`Hsla`] に。不正は `None`。
fn parse_hex(value: &str) -> Option<Hsla> {
    let body = value.trim().trim_start_matches('#');
    match body.len() {
        6 => u32::from_str_radix(body, 16)
            .ok()
            .map(|rgb_value| rgb(rgb_value).into()),
        8 => u32::from_str_radix(body, 16).ok().map(|rgba_value| {
            let [red, green, blue, alpha] = rgba_value.to_be_bytes();
            Rgba {
                r: red as f32 / 255.0,
                g: green as f32 / 255.0,
                b: blue as f32 / 255.0,
                a: alpha as f32 / 255.0,
            }
            .into()
        }),
        _ => None,
    }
}

/// 選択肢に出すテーマ一覧（組み込み 2 種 + 同梱テーマ + `themes_dir` 直下の `*.json`）。
/// 各要素は (表示名, 出所)。表示名はユーザーテーマなら JSON の `name`、無ければファイル名。
pub fn available_themes(themes_dir: Option<&Path>) -> Vec<(SharedString, ThemeSource)> {
    let mut themes = vec![
        (
            SharedString::from("necoder Dark"),
            ThemeSource::BuiltIn(DARK_THEME_NAME),
        ),
        (
            SharedString::from("necoder Light"),
            ThemeSource::BuiltIn(LIGHT_THEME_NAME),
        ),
    ];
    themes.extend(
        embedded_themes()
            .iter()
            .map(|(id, theme)| (theme.name.clone(), ThemeSource::BuiltIn(id))),
    );
    if let Some(dir) = themes_dir {
        if let Ok(read) = std::fs::read_dir(dir) {
            let mut users: Vec<(SharedString, ThemeSource)> = Vec::new();
            for entry in read.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                    continue;
                }
                let display = load_user_theme(&path)
                    .map(|theme| theme.name.to_string())
                    .ok()
                    .or_else(|| {
                        path.file_stem()
                            .map(|stem| stem.to_string_lossy().to_string())
                    })
                    .unwrap_or_else(|| path.display().to_string());
                users.push((SharedString::from(display), ThemeSource::User(path)));
            }
            users.sort_by(|left, right| left.0.cmp(&right.0));
            themes.extend(users);
        }
    }
    themes
}

/// 設定の theme 名を [`Theme`] に解決する。組み込み名 → `themes_dir` のユーザーテーマ
/// （`name` 一致 or ファイル名 stem 一致）→ dark、の順でフォールバック（起動時に使う）。
pub fn resolve(name: &str, themes_dir: Option<&Path>) -> Theme {
    if let Some(theme) = Theme::builtin(name) {
        return theme;
    }
    if let Some(dir) = themes_dir {
        if let Ok(read) = std::fs::read_dir(dir) {
            for entry in read.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                    continue;
                }
                let stem_match = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy() == name)
                    .unwrap_or(false);
                if let Ok(theme) = load_user_theme(&path) {
                    if stem_match || theme.name.as_ref() == name {
                        return theme;
                    }
                }
            }
        }
    }
    Theme::dark()
}

/// プロジェクトの識別（レール項目・ピル左縁 等に流れる）。UI-SPEC §1.2 / §2。
/// 色の優先順は `.necoder/settings.json` の `color` > 手動選択 > パレット巡回（解決は M3）。
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectIdentity {
    pub color: Hsla,
    pub icon: IconSource,
}

/// レール項目に出す図像の出所。優先順は `.necoder/settings.json` の `icon` > プロジェクト名の頭文字（UI-SPEC §2）。
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

/// プロジェクト色パレット（自動巡回）: 原色寄りの 5 色 red → green → blue → orange → violet。
/// M13 で titlebar/statusbar 淡塗り（Peacock）に合わせ低彩度→原色へ（面で映える。線より面が主用途）。
pub const PROJECT_COLOR_HEXES: [u32; 5] = [0xef4444, 0x22c55e, 0x3b82f6, 0xf97316, 0xa855f7];
/// スレッド色パレット（自動巡回）。プロジェクト色と独立。
pub const THREAD_COLOR_HEXES: [u32; 3] = [0x61afef, 0xe5c07b, 0xc678dd];

/// 色ピッカーが提示するアイデンティティ色（UI-SPEC §1.2）。M13 で**原色寄り**へ差し替え（塗り面で映える）。
/// 先頭 5 つは [`PROJECT_COLOR_HEXES`]（自動巡回色）と一致 = 巡回色もピッカーから選べる。
/// トレードオフ: 原色化でスレッド色（`#61afef`/`#e5c07b`/`#c678dd`）と近づく——2px 線でなく面で使う前提の判断。
/// 任意 hex 入力はこの外の色も許すエスケープハッチ（UI-SPEC §1.2）。
pub const IDENTITY_PALETTE_HEXES: [u32; 10] = [
    0xef4444, 0x22c55e, 0x3b82f6, 0xf97316,
    0xa855f7, // 巡回 5 色（red/green/blue/orange/violet）
    0x06b6d4, // cyan
    0xec4899, // pink
    0xeab308, // yellow
    0x14b8a6, // teal
    0xd946ef, // fuchsia
];

/// `index` 番目のプロジェクト色（パレットを巡回）。色優先順の最下段（自動巡回）に当たる。
pub fn project_color(index: usize) -> Hsla {
    hex(PROJECT_COLOR_HEXES[index % PROJECT_COLOR_HEXES.len()])
}

/// 0xRRGGBB を Hsla へ（リモートのホスト別色を storage が u32 で持つため・M13 #3b）。
pub fn color_from_hex(value: u32) -> Hsla {
    hex(value)
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
        assert_eq!(project_color(0), h(0xef4444));
        assert_eq!(project_color(4), h(0xa855f7));
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

    /// 同梱テーマは全数パースでき、id / 表示名のどちらでも引けて、外観が宣言通りであること。
    /// JSON の壊れ（typo・欠け・未知キー）はこのテストがビルド時産物ごと検知する。
    #[test]
    fn embedded_themes_parse() {
        assert_eq!(embedded_themes().len(), EMBEDDED_THEME_JSONS.len());
        for (id, theme) in embedded_themes() {
            // id でも JSON の name でも同じテーマが引ける（settings.json にはどちらを書いてもよい）。
            assert_eq!(Theme::builtin(id).as_ref(), Some(theme), "id で引けない: {id}");
            assert_eq!(
                Theme::builtin(theme.name.as_ref()).as_ref(),
                Some(theme),
                "表示名で引けない: {id}"
            );
            // resolve（起動時の解決）も同じ経路に落ちる。
            assert_eq!(&resolve(id, None), theme);
        }
        assert_eq!(
            Theme::builtin("solarized-light").map(|theme| theme.appearance),
            Some(Appearance::Light)
        );
        assert_eq!(
            Theme::builtin("Gruvbox Dark").map(|theme| theme.appearance),
            Some(Appearance::Dark)
        );
        // 同梱テーマは全トークン明示（土台 dark/light の値が透けて残らない）を name で担保:
        // 表示名が JSON で上書きされている＝JSON が読まれている証左。
        for (_, theme) in embedded_themes() {
            assert_ne!(theme.name.as_ref(), DARK_THEME_NAME);
            assert_ne!(theme.name.as_ref(), LIGHT_THEME_NAME);
        }
    }

    #[test]
    fn load_resolves_builtins_and_reports_errors() {
        assert_eq!(Theme::builtin(DARK_THEME_NAME), Some(Theme::dark()));
        assert_eq!(Theme::builtin(LIGHT_THEME_NAME), Some(Theme::light()));
        assert_eq!(Theme::builtin("nope"), None);

        assert!(Theme::load(&ThemeSource::BuiltIn(DARK_THEME_NAME)).is_ok());
        assert!(Theme::load(&ThemeSource::BuiltIn("nope")).is_err());
        // 存在しないユーザーテーマ JSON は読み込みエラー
        assert!(Theme::load(&ThemeSource::User(PathBuf::from("/x/theme.json"))).is_err());
    }

    #[test]
    fn user_theme_overrides_base() {
        let dir = std::env::temp_dir().join(format!("necoder_theme_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("midnight.json");
        // light を土台に一部トークンだけ上書き。syntax.keyword は #rrggbbaa（alpha 付き）。
        std::fs::write(
            &path,
            r##"{ "name": "Midnight", "appearance": "light", "bg0": "#000000",
                 "syntax": { "keyword": "#ff0088ff" } }"##,
        )
        .unwrap();

        let theme = Theme::load(&ThemeSource::User(path.clone())).expect("読める");
        assert_eq!(theme.name.as_ref(), "Midnight");
        assert_eq!(theme.appearance, Appearance::Light); // 土台 = light
        assert_eq!(theme.bg0, h(0x000000)); // 上書きされた
        assert_eq!(theme.bg1, Theme::light().bg1); // 未指定 → 土台のまま
        assert_eq!(theme.syntax.keyword, h(0xff0088)); // alpha=ff → 不透明
        assert_eq!(theme.syntax.string, Theme::light().syntax.string); // 未指定 → 土台

        // 一覧に組み込み 2 種 + 同梱 5 種 + ユーザー 1 種が並ぶ（ユーザーは末尾）。
        let list = available_themes(Some(&dir));
        assert_eq!(list.len(), 2 + EMBEDDED_THEME_JSONS.len() + 1);
        assert_eq!(list[0].1, ThemeSource::BuiltIn(DARK_THEME_NAME));
        assert!(matches!(list.last().unwrap().1, ThemeSource::User(_)));
        // resolve は name 一致でユーザーテーマを引ける。
        assert_eq!(resolve("Midnight", Some(&dir)).bg0, h(0x000000));
        assert_eq!(
            resolve("necoder-light", Some(&dir)).appearance,
            Appearance::Light
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
