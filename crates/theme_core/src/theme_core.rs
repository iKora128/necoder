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

    /// Agent パネルの手順（⏺ のツール行）の面。`None` = 塗らない（チャット面 bg1 のまま）。
    pub chat_step_bg: Option<Hsla>,
    /// Agent パネルのエージェント回答の面。`None` = 塗らない（チャット面 bg1 のまま）。
    pub chat_agent_bg: Option<Hsla>,
    /// Agent パネルのユーザープロンプトの面。`None` = bg2。
    pub chat_user_bg: Option<Hsla>,

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
            chat_step_bg: None,
            chat_agent_bg: None,
            chat_user_bg: None,
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
            chat_step_bg: None,
            chat_agent_bg: None,
            chat_user_bg: None,
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
    chat_step_bg: Option<String>,
    chat_agent_bg: Option<String>,
    chat_user_bg: Option<String>,
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
        override_optional_color(&mut theme.chat_step_bg, self.chat_step_bg);
        override_optional_color(&mut theme.chat_agent_bg, self.chat_agent_bg);
        override_optional_color(&mut theme.chat_user_bg, self.chat_user_bg);
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

/// 既定で塗らない（`None`）トークン用。`value` が有効な hex なら `field` を `Some` で上書き。
fn override_optional_color(field: &mut Option<Hsla>, value: Option<String>) {
    if let Some(color) = value.as_deref().and_then(parse_hex) {
        *field = Some(color);
    }
}

/// `#rrggbb` / `#rrggbbaa`（`#` 省略可）を [`Hsla`] に。不正は `None`。
pub fn parse_hex(value: &str) -> Option<Hsla> {
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

// ── 自分のテーマとして保存（themes/ へ JSON 1 枚を書く）──

/// テーマを「トークン上書き JSON」の本文へ書き出す。全トークンを書く（元にしたテーマが後で変わっても、
/// 保存した色は動かない）。チャット用トークンは塗る時だけ書く（書かない＝塗らない）。
pub fn theme_json(theme: &Theme) -> Result<String> {
    let mut root = serde_json::Map::new();
    root.insert("name".into(), theme.name.to_string().into());
    let appearance = match theme.appearance {
        Appearance::Dark => "dark",
        Appearance::Light => "light",
    };
    root.insert("appearance".into(), appearance.into());
    let mut syntax = serde_json::Map::new();
    for token in ThemeToken::ALL {
        let color = match token {
            ThemeToken::ChatStepBg => theme.chat_step_bg,
            ThemeToken::ChatAgentBg => theme.chat_agent_bg,
            ThemeToken::ChatUserBg => theme.chat_user_bg,
            token => Some(token.effective(theme)),
        };
        let Some(color) = color else {
            continue;
        };
        let value = serde_json::Value::String(hex_string(color));
        match token.key().strip_prefix("syntax.") {
            Some(key) => syntax.insert(key.to_string(), value),
            None => root.insert(token.key().to_string(), value),
        };
    }
    root.insert("syntax".into(), syntax.into());
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .context("テーマ JSON を書き出せない")?;
    Ok(text + "\n")
}

/// 名前が既存のテーマ（組み込みの id・表示名・themes/ のファイル名）と重なるか。大文字と小文字は区別しない。
/// [`resolve`] は名前とファイル名のどちらでも引くので、重なると別のテーマを開いてしまう。
pub fn theme_name_taken(name: &str, themes_dir: Option<&Path>) -> bool {
    let name = name.trim();
    let same = |other: &str| other.trim().eq_ignore_ascii_case(name);
    available_themes(themes_dir)
        .iter()
        .any(|(display, source)| {
            same(display)
                || match source {
                    ThemeSource::BuiltIn(id) => same(id),
                    ThemeSource::User(path) => path
                        .file_stem()
                        .is_some_and(|stem| same(&stem.to_string_lossy())),
                }
        })
}

/// テーマを `themes_dir` へ新しいファイルとして書き、その場所を返す。ファイル名は名前から作り、
/// 同じ名前のファイルが既にあれば `-2` から番号を付ける（他のテーマのファイルを上書きしない）。
pub fn save_new_user_theme(themes_dir: &Path, theme: &Theme) -> Result<PathBuf> {
    std::fs::create_dir_all(themes_dir)
        .with_context(|| format!("テーマのフォルダを作れない: {}", themes_dir.display()))?;
    let stem = theme_file_stem(&theme.name);
    let mut path = themes_dir.join(format!("{stem}.json"));
    let mut number = 2;
    while path.exists() {
        path = themes_dir.join(format!("{stem}-{number}.json"));
        number += 1;
    }
    write_user_theme(&path, theme)?;
    Ok(path)
}

/// テーマを `path` へ書く（自分のテーマを同じ名前で保存し直す時もここ）。
pub fn write_user_theme(path: &Path, theme: &Theme) -> Result<()> {
    std::fs::write(path, theme_json(theme)?)
        .with_context(|| format!("テーマ JSON を書けない: {}", path.display()))
}

/// テーマ名からファイル名（拡張子なし）を作る。どの OS でも使えない文字は `-` にし、
/// Windows の予約名（CON など）と空の名前は避ける。日本語はそのまま使う。
fn theme_file_stem(name: &str) -> String {
    let replaced: String = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '-'
            } else {
                character
            }
        })
        .collect();
    let stem =
        replaced.trim_matches(|character: char| character == '.' || character.is_whitespace());
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if stem.is_empty() {
        "theme".to_string()
    } else if RESERVED
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
    {
        format!("{stem}-theme")
    } else {
        stem.to_string()
    }
}

// ── 外観設定からの個別上書き（settings.json の theme_overrides）──

/// 外観設定で個別に上書きできる色トークン。[`ThemeToken::key`] は settings.json の
/// `theme_overrides.<テーマ名>` のキー（構文色は `syntax.keyword` のようにドットで区切る）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeToken {
    Bg0,
    Bg1,
    Bg2,
    Bg3,
    Fg0,
    Fg1,
    Fg2,
    Border,
    Ok,
    Warn,
    Err,
    SyntaxKeyword,
    SyntaxFunction,
    SyntaxType,
    SyntaxString,
    SyntaxNumber,
    SyntaxComment,
    SyntaxMacro,
    SyntaxPunctuation,
    ChatStepBg,
    ChatAgentBg,
    ChatUserBg,
}

impl ThemeToken {
    /// 上書きできるトークンのすべて（外観設定は、まとまりの中をこの順で並べる）。
    pub const ALL: [ThemeToken; 22] = [
        ThemeToken::Bg0,
        ThemeToken::Bg1,
        ThemeToken::Bg2,
        ThemeToken::Bg3,
        ThemeToken::Border,
        ThemeToken::Fg0,
        ThemeToken::Fg1,
        ThemeToken::Fg2,
        ThemeToken::Ok,
        ThemeToken::Warn,
        ThemeToken::Err,
        ThemeToken::SyntaxKeyword,
        ThemeToken::SyntaxFunction,
        ThemeToken::SyntaxType,
        ThemeToken::SyntaxString,
        ThemeToken::SyntaxNumber,
        ThemeToken::SyntaxComment,
        ThemeToken::SyntaxMacro,
        ThemeToken::SyntaxPunctuation,
        ThemeToken::ChatUserBg,
        ThemeToken::ChatStepBg,
        ThemeToken::ChatAgentBg,
    ];

    /// settings.json の `theme_overrides.<テーマ名>` で使うキー。
    pub fn key(self) -> &'static str {
        match self {
            ThemeToken::Bg0 => "bg0",
            ThemeToken::Bg1 => "bg1",
            ThemeToken::Bg2 => "bg2",
            ThemeToken::Bg3 => "bg3",
            ThemeToken::Fg0 => "fg0",
            ThemeToken::Fg1 => "fg1",
            ThemeToken::Fg2 => "fg2",
            ThemeToken::Border => "border",
            ThemeToken::Ok => "ok",
            ThemeToken::Warn => "warn",
            ThemeToken::Err => "err",
            ThemeToken::SyntaxKeyword => "syntax.keyword",
            ThemeToken::SyntaxFunction => "syntax.function",
            ThemeToken::SyntaxType => "syntax.type",
            ThemeToken::SyntaxString => "syntax.string",
            ThemeToken::SyntaxNumber => "syntax.number",
            ThemeToken::SyntaxComment => "syntax.comment",
            ThemeToken::SyntaxMacro => "syntax.macro",
            ThemeToken::SyntaxPunctuation => "syntax.punctuation",
            ThemeToken::ChatStepBg => "chat_step_bg",
            ThemeToken::ChatAgentBg => "chat_agent_bg",
            ThemeToken::ChatUserBg => "chat_user_bg",
        }
    }

    /// [`ThemeToken::key`] の逆引き。知らないキーは `None`。
    pub fn from_key(key: &str) -> Option<ThemeToken> {
        ThemeToken::ALL.into_iter().find(|token| token.key() == key)
    }

    /// 画面に描かれる色。チャット用トークンが未設定なら、代わりに描かれる面の色を返す。
    pub fn effective(self, theme: &Theme) -> Hsla {
        match self {
            ThemeToken::Bg0 => theme.bg0,
            ThemeToken::Bg1 => theme.bg1,
            ThemeToken::Bg2 => theme.bg2,
            ThemeToken::Bg3 => theme.bg3,
            ThemeToken::Fg0 => theme.fg0,
            ThemeToken::Fg1 => theme.fg1,
            ThemeToken::Fg2 => theme.fg2,
            ThemeToken::Border => theme.border,
            ThemeToken::Ok => theme.ok,
            ThemeToken::Warn => theme.warn,
            ThemeToken::Err => theme.err,
            ThemeToken::SyntaxKeyword => theme.syntax.keyword,
            ThemeToken::SyntaxFunction => theme.syntax.function,
            ThemeToken::SyntaxType => theme.syntax.type_,
            ThemeToken::SyntaxString => theme.syntax.string,
            ThemeToken::SyntaxNumber => theme.syntax.number,
            ThemeToken::SyntaxComment => theme.syntax.comment,
            ThemeToken::SyntaxMacro => theme.syntax.macro_,
            ThemeToken::SyntaxPunctuation => theme.syntax.punctuation,
            ThemeToken::ChatStepBg => theme.chat_step_bg.unwrap_or(theme.bg1),
            ThemeToken::ChatAgentBg => theme.chat_agent_bg.unwrap_or(theme.bg1),
            ThemeToken::ChatUserBg => theme.chat_user_bg.unwrap_or(theme.bg2),
        }
    }

    /// `theme` のこのトークンを `color` にする（チャット用トークンは塗る側へ切り替わる）。
    pub fn set(self, theme: &mut Theme, color: Hsla) {
        match self {
            ThemeToken::Bg0 => theme.bg0 = color,
            ThemeToken::Bg1 => theme.bg1 = color,
            ThemeToken::Bg2 => theme.bg2 = color,
            ThemeToken::Bg3 => theme.bg3 = color,
            ThemeToken::Fg0 => theme.fg0 = color,
            ThemeToken::Fg1 => theme.fg1 = color,
            ThemeToken::Fg2 => theme.fg2 = color,
            ThemeToken::Border => theme.border = color,
            ThemeToken::Ok => theme.ok = color,
            ThemeToken::Warn => theme.warn = color,
            ThemeToken::Err => theme.err = color,
            ThemeToken::SyntaxKeyword => theme.syntax.keyword = color,
            ThemeToken::SyntaxFunction => theme.syntax.function = color,
            ThemeToken::SyntaxType => theme.syntax.type_ = color,
            ThemeToken::SyntaxString => theme.syntax.string = color,
            ThemeToken::SyntaxNumber => theme.syntax.number = color,
            ThemeToken::SyntaxComment => theme.syntax.comment = color,
            ThemeToken::SyntaxMacro => theme.syntax.macro_ = color,
            ThemeToken::SyntaxPunctuation => theme.syntax.punctuation = color,
            ThemeToken::ChatStepBg => theme.chat_step_bg = Some(color),
            ThemeToken::ChatAgentBg => theme.chat_agent_bg = Some(color),
            ThemeToken::ChatUserBg => theme.chat_user_bg = Some(color),
        }
    }

    /// `to` のこのトークンだけを `from` の値へ戻す（チャット用トークンは「塗らない」も含めて写す）。
    /// 上書き前のテーマから戻せば、このトークンを戻した時に画面へ出る色を [`Self::effective`] で引ける。
    pub fn restore(self, from: &Theme, to: &mut Theme) {
        match self {
            ThemeToken::ChatStepBg => to.chat_step_bg = from.chat_step_bg,
            ThemeToken::ChatAgentBg => to.chat_agent_bg = from.chat_agent_bg,
            ThemeToken::ChatUserBg => to.chat_user_bg = from.chat_user_bg,
            token => token.set(to, token.effective(from)),
        }
    }
}

/// settings.json の `theme_overrides.<テーマ名>`（キー → `#rrggbb`）をテーマへ当てる。
/// 知らないキーと読めない色は飛ばす（手編集の誤りで外観全体を壊さない）。
pub fn apply_overrides<'a>(
    theme: &mut Theme,
    overrides: impl IntoIterator<Item = (&'a str, &'a str)>,
) {
    for (key, value) in overrides {
        if let (Some(token), Some(color)) = (ThemeToken::from_key(key), parse_hex(value)) {
            token.set(theme, color);
        }
    }
}

/// Hsla を `#rrggbb`（不透明）か `#rrggbbaa`（透明度あり）の文字列へ。settings.json へ書くときに使う。
pub fn hex_string(color: Hsla) -> String {
    let rgba = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (red, green, blue, alpha) = (
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b),
        channel(rgba.a),
    );
    if alpha == u8::MAX {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}")
    }
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

/// 0xRRGGBB を Hsla へ（プロジェクト色を storage が u32 で持つため）。
pub fn color_from_hex(value: u32) -> Hsla {
    hex(value)
}

/// Hsla を 0xRRGGBB へ（[`color_from_hex`] の逆・透明度は捨てる）。DB へ焼くときに使う。
pub fn hex_from_color(color: Hsla) -> u32 {
    let rgba = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b)
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
    fn hex_round_trips_through_hsla() {
        // パレット全色と黒/白/灰が u32 → Hsla → u32 で 1 bit も変わらない（DB に焼いて戻す経路）。
        for value in IDENTITY_PALETTE_HEXES
            .iter()
            .copied()
            .chain([0x000000, 0xffffff, 0x808080, 0xd97757])
        {
            assert_eq!(hex_from_color(color_from_hex(value)), value, "{value:06x}");
        }
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
            assert_eq!(
                Theme::builtin(id).as_ref(),
                Some(theme),
                "id で引けない: {id}"
            );
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
                 "chat_step_bg": "#112233",
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
        assert_eq!(theme.chat_step_bg, Some(h(0x112233))); // 指定 → その色で塗る
        assert_eq!(theme.chat_agent_bg, None); // 未指定 → 塗らない
        assert_eq!(Theme::dark().chat_step_bg, None); // 組み込みは塗らない＝従来の見た目

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

    #[test]
    fn token_keys_round_trip_and_are_unique() {
        let mut keys = std::collections::HashSet::new();
        for token in ThemeToken::ALL {
            assert_eq!(ThemeToken::from_key(token.key()), Some(token));
            assert!(keys.insert(token.key()), "キーが重複: {}", token.key());
        }
        assert_eq!(ThemeToken::from_key("nope"), None);
    }

    #[test]
    fn overrides_replace_only_listed_tokens() {
        let mut theme = Theme::dark();
        apply_overrides(
            &mut theme,
            [
                ("bg1", "#101010"),
                ("syntax.keyword", "#ff0000"),
                ("chat_user_bg", "#00ff00"),
                ("unknown", "#123456"), // 知らないキーは飛ばす
                ("fg0", "not-a-color"), // 読めない色は飛ばす
            ],
        );
        assert_eq!(theme.bg1, h(0x101010));
        assert_eq!(theme.syntax.keyword, h(0xff0000));
        assert_eq!(theme.chat_user_bg, Some(h(0x00ff00)));
        assert_eq!(theme.fg0, Theme::dark().fg0);
        assert_eq!(theme.bg2, Theme::dark().bg2);
    }

    #[test]
    fn chat_tokens_fall_back_to_the_surface_they_replace() {
        let theme = Theme::dark();
        assert_eq!(ThemeToken::ChatStepBg.effective(&theme), theme.bg1);
        assert_eq!(ThemeToken::ChatAgentBg.effective(&theme), theme.bg1);
        assert_eq!(ThemeToken::ChatUserBg.effective(&theme), theme.bg2);
        assert_eq!(hex_string(h(0x1b1e25)), "#1b1e25");
        assert_eq!(hex_string(h(0x1b1e25).alpha(0.5)), "#1b1e2580");
        assert_eq!(
            parse_hex("#1b1e2580").map(hex_string).as_deref(),
            Some("#1b1e2580")
        );
    }

    #[test]
    fn every_color_field_has_a_token() {
        // Theme に色のフィールドを足したら、下の分解がコンパイルで落ちる＝ThemeToken にも足すこと。
        let mut theme = Theme::dark();
        let mut assigned = std::collections::BTreeSet::new();
        for (index, token) in ThemeToken::ALL.into_iter().enumerate() {
            let color = (index as u32 + 1) * 0x0a0b0c;
            token.set(&mut theme, h(color));
            assigned.insert(color);
        }
        let Theme {
            name: _,
            appearance: _,
            bg0,
            bg1,
            bg2,
            bg3,
            fg0,
            fg1,
            fg2,
            border,
            ok,
            warn,
            err,
            chat_step_bg,
            chat_agent_bg,
            chat_user_bg,
            syntax:
                SyntaxColors {
                    keyword,
                    function,
                    type_,
                    string,
                    number,
                    comment,
                    macro_,
                    punctuation,
                },
        } = theme;
        let optional = [chat_step_bg, chat_agent_bg, chat_user_bg];
        assert!(
            optional.iter().all(Option::is_some),
            "チャット用トークンも塗る側へ変わる"
        );
        let fields: std::collections::BTreeSet<u32> = [
            bg0,
            bg1,
            bg2,
            bg3,
            fg0,
            fg1,
            fg2,
            border,
            ok,
            warn,
            err,
            keyword,
            function,
            type_,
            string,
            number,
            comment,
            macro_,
            punctuation,
        ]
        .into_iter()
        .chain(optional.into_iter().flatten())
        .map(hex_from_color)
        .collect();
        assert_eq!(
            fields, assigned,
            "どの色のフィールドも ThemeToken から変えられる"
        );
    }

    #[test]
    fn restore_copies_one_token_including_unpainted_chat_tokens() {
        let base = Theme::dark();
        let mut theme = Theme::dark();
        ThemeToken::Bg1.set(&mut theme, h(0x101010));
        ThemeToken::ChatStepBg.set(&mut theme, h(0x202020));
        ThemeToken::ChatStepBg.restore(&base, &mut theme);
        assert_eq!(theme.chat_step_bg, None, "塗らない状態へ戻る");
        // 戻した時に出る色は、上書きした bg1（チャット面）。
        assert_eq!(ThemeToken::ChatStepBg.effective(&theme), h(0x101010));
        ThemeToken::Bg1.restore(&base, &mut theme);
        assert_eq!(theme.bg1, base.bg1);
    }

    /// 保存した JSON を読み直すと、同じ名前・明暗・色のテーマに戻る（塗らないチャットの面は塗らないまま）。
    #[test]
    fn saved_theme_reads_back_as_the_same_theme() {
        let dir = std::env::temp_dir().join(format!("necoder_save_theme_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut theme = Theme::light();
        theme.name = "自分のテーマ #1".into();
        ThemeToken::Bg1.set(&mut theme, h(0x101418));
        ThemeToken::SyntaxKeyword.set(&mut theme, h(0xff0088).alpha(0.5));
        ThemeToken::ChatStepBg.set(&mut theme, h(0x0f1a24));

        let path = save_new_user_theme(&dir, &theme).expect("保存できる");
        assert_eq!(path, dir.join("自分のテーマ #1.json"));
        let loaded = Theme::load(&ThemeSource::User(path)).expect("読み直せる");
        assert_eq!(loaded.name, theme.name);
        assert_eq!(loaded.appearance, Appearance::Light);
        for token in ThemeToken::ALL {
            assert_eq!(
                hex_string(token.effective(&loaded)),
                hex_string(token.effective(&theme)),
                "{token:?}"
            );
        }
        assert_eq!(loaded.chat_agent_bg, None, "塗らない面は塗らないまま");
        assert!(loaded.chat_step_bg.is_some());

        // 同じ名前のファイルがあれば番号を付ける（前のファイルを上書きしない）。
        let second = save_new_user_theme(&dir, &theme).expect("保存できる");
        assert_eq!(second, dir.join("自分のテーマ #1-2.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_names_cannot_reuse_existing_names_or_file_names() {
        let dir = std::env::temp_dir().join(format!("necoder_theme_names_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("作れる");
        std::fs::write(dir.join("midnight-file.json"), r#"{ "name": "Midnight" }"#)
            .expect("書ける");
        for taken in [
            "necoder Dark",
            "necoder-dark",
            "solarized dark",
            "Solarized-Light",
            " Midnight ",
            "MIDNIGHT-FILE",
        ] {
            assert!(theme_name_taken(taken, Some(&dir)), "{taken}");
        }
        assert!(!theme_name_taken("Midnight 2", Some(&dir)));
        assert!(!theme_name_taken("自分のテーマ", None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_file_names_are_safe_on_every_os() {
        assert_eq!(theme_file_stem("My Theme"), "My Theme");
        assert_eq!(theme_file_stem("a/b:c*d?"), "a-b-c-d-");
        assert_eq!(theme_file_stem("  ..  "), "theme");
        assert_eq!(theme_file_stem("con"), "con-theme");
        assert_eq!(theme_file_stem("夜の海"), "夜の海");
    }
}
