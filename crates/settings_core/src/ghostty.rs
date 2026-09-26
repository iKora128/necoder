//! Ghostty の設定からターミナルの設定を取り込む（O45・H14）。
//!
//! Ghostty の設定（`key = value`・`#` はコメント）のうち、necoder のターミナルに同じ意味の設定がある物
//! だけを拾う:
//! - 書体（`font-family` の最初の 1 つ・代わりの書体の並びは持ってこない）
//! - 大きさ（`font-size`）
//! - カーソル（`cursor-style`・`block_hollow` は `block`）
//! - 配色（`theme = 名前` ならテーマのファイル・`light:A,dark:B` なら暗い方。テーマが無く
//!   `palette` / `background` / `foreground` を書いていればその設定ファイル自体）。ターミナルの配色を読む側は
//!   6 桁の 16 進しか読めないので、それが 1 つも無い物（X11 の色の名前だけ等）は持ってこない
//!
//! スクロールバック（Ghostty はバイト数）・キー・窓の設定は意味が違うので持ってこない。

use std::path::{Path, PathBuf};

/// 取り込める物（見つからなかった項目は `None`）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GhosttyTerminal {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    /// `terminal_cursor` の値（`block` / `bar` / `underline`）。
    pub cursor: Option<&'static str>,
    /// `terminal_color_scheme` に置くファイル。
    pub color_scheme: Option<PathBuf>,
}

impl GhosttyTerminal {
    pub fn is_empty(&self) -> bool {
        *self == GhosttyTerminal::default()
    }
}

/// Ghostty の設定ファイルの置き場（読む順）。XDG（`$XDG_CONFIG_HOME` か `~/.config`）の `ghostty/` と、
/// macOS のアプリの置き場。名前は新しい `config.ghostty` と古い `config`。
pub fn config_candidates() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(xdg) => directories.push(PathBuf::from(xdg).join("ghostty")),
        None => {
            if let Some(home) = paths::home_dir() {
                directories.push(home.join(".config").join("ghostty"));
            }
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = paths::home_dir() {
            directories.push(
                home.join("Library")
                    .join("Application Support")
                    .join("com.mitchellh.ghostty"),
            );
        }
    }
    directories
        .into_iter()
        .flat_map(|directory| [directory.join("config.ghostty"), directory.join("config")])
        .collect()
}

/// テーマの名前を探す場所（ユーザーのテーマ → アプリに入っているテーマ）。
fn theme_directories(config: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(parent) = config.parent() {
        directories.push(parent.join("themes"));
    }
    if cfg!(target_os = "macos") {
        directories.push(PathBuf::from(
            "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
        ));
    } else {
        directories.push(PathBuf::from("/usr/share/ghostty/themes"));
    }
    directories
}

/// 最初に見つかった設定ファイルを読んで、取り込める物を返す（無ければ `None`）。
pub fn read() -> Option<(PathBuf, GhosttyTerminal)> {
    let config = config_candidates()
        .into_iter()
        .find(|path| path.is_file())?;
    let text = std::fs::read_to_string(&config).ok()?;
    let imported = parse(&text, &config, &theme_directories(&config));
    Some((config, imported))
}

/// 設定の中身から拾う（`config` = そのファイルの場所・`theme_directories` = テーマを探す場所）。
pub fn parse(text: &str, config: &Path, theme_directories: &[PathBuf]) -> GhosttyTerminal {
    let mut imported = GhosttyTerminal::default();
    let mut theme = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim();
        match key {
            // 最初の 1 つが本の書体（後の行は代わりの書体）。空は並びの取り消し（次の行から数え直す）。
            "font-family" => {
                if value.is_empty() {
                    imported.font_family = None;
                } else if imported.font_family.is_none() {
                    imported.font_family = Some(value.to_string());
                }
            }
            "font-size" => {
                if let Ok(size) = value.parse::<f32>() {
                    if size.is_finite() && size > 0.0 {
                        imported.font_size = Some(size);
                    }
                }
            }
            "cursor-style" => {
                imported.cursor = match value {
                    "block" | "block_hollow" => Some("block"),
                    "bar" => Some("bar"),
                    "underline" => Some("underline"),
                    _ => imported.cursor,
                };
            }
            "theme" if !value.is_empty() => theme = Some(value.to_string()),
            _ => {}
        }
    }
    // テーマを優先する（16 色がそろっている。設定に書いた色はたいてい一部の上書き）。テーマが無ければ、
    // Ghostty のテーマと設定は同じ書式なので、色を書いた設定ファイル自体を配色として読める。
    imported.color_scheme = theme
        .and_then(|theme| resolve_theme(&theme, theme_directories))
        .filter(|path| std::fs::read_to_string(path).is_ok_and(|text| has_readable_colors(&text)))
        .or_else(|| has_readable_colors(text).then(|| config.to_path_buf()));
    imported
}

/// ターミナルの配色を読む側が読める色か: 6 桁の 16 進（`#` は有っても無くても）。
fn readable_color(value: &str) -> bool {
    let hex = value.trim().trim_matches('"').trim_start_matches('#');
    hex.len() == 6 && hex.chars().all(|character| character.is_ascii_hexdigit())
}

/// 配色の行（`palette = N=#rrggbb`・`background`・`foreground`）に、読める色が 1 つでもあるか。
fn has_readable_colors(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return false;
        }
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        match key.trim() {
            "palette" => value
                .split_once('=')
                .is_some_and(|(_, color)| readable_color(color)),
            "background" | "foreground" => readable_color(value),
            _ => false,
        }
    })
}

/// `theme` の値をファイルへ（`light:A,dark:B` は暗い方・パスはそのまま）。見つからなければ `None`。
fn resolve_theme(value: &str, theme_directories: &[PathBuf]) -> Option<PathBuf> {
    let name = if value.contains(':') && value.contains(',') {
        value
            .split(',')
            .find_map(|part| part.trim().strip_prefix("dark:"))
            .or_else(|| {
                value
                    .split(',')
                    .find_map(|part| part.trim().strip_prefix("light:"))
            })
            .unwrap_or(value)
            .trim()
    } else {
        value
    };
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    theme_directories
        .iter()
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("necoder_ghostty_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn terminal_settings_are_picked_from_a_ghostty_config() {
        let directory = scratch("plain");
        let config = directory.join("config");
        let text = "# my ghostty\nfont-family = \"JetBrains Mono\"\nfont-family = Symbols Nerd Font\nfont-size = 14.5\ncursor-style = block_hollow\nscrollback-limit = 10000000\nkeybind = cmd+t=new_tab\n";
        let imported = parse(text, &config, &[]);
        assert_eq!(
            imported.font_family.as_deref(),
            Some("JetBrains Mono"),
            "最初の 1 つ"
        );
        assert_eq!(imported.font_size, Some(14.5));
        assert_eq!(imported.cursor, Some("block"));
        assert_eq!(imported.color_scheme, None, "配色は書いていない");
        let reset = parse(
            "font-family = Menlo\nfont-family = \"\"\nfont-family = Iosevka\n",
            &config,
            &[],
        );
        assert_eq!(
            reset.font_family.as_deref(),
            Some("Iosevka"),
            "空で数え直す"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// 配色: 設定に直接書いてあればその設定ファイル、`theme = 名前` ならテーマのファイル（暗い方）。
    #[test]
    fn colors_come_from_the_config_itself_or_the_named_theme() {
        let directory = scratch("colors");
        let config = directory.join("config");
        let inline = parse("background = #1d1f21\npalette = 0=#000000\n", &config, &[]);
        assert_eq!(inline.color_scheme.as_deref(), Some(config.as_path()));

        let themes = directory.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("Dracula"), "background = #282a36\n").unwrap();
        let named = parse("theme = Dracula\n", &config, &[themes.clone()]);
        assert_eq!(named.color_scheme, Some(themes.join("Dracula")));
        let paired = parse(
            "theme = light:Missing,dark:Dracula\n",
            &config,
            &[themes.clone()],
        );
        assert_eq!(paired.color_scheme, Some(themes.join("Dracula")), "暗い方");
        let both = parse(
            "theme = Dracula\nbackground = #000000\n",
            &config,
            &[themes.clone()],
        );
        assert_eq!(
            both.color_scheme,
            Some(themes.join("Dracula")),
            "テーマが先"
        );
        let missing = parse("theme = Nowhere\n", &config, &[themes.clone()]);
        assert_eq!(missing.color_scheme, None, "見つからない名前は持ってこない");
        let fallback = parse(
            "theme = Nowhere\nbackground = #000000\n",
            &config,
            &[themes.clone()],
        );
        assert_eq!(
            fallback.color_scheme.as_deref(),
            Some(config.as_path()),
            "無ければ設定の色"
        );
        // X11 の色の名前だけの設定・テーマは読めないので持ってこない（16 進が 1 つあれば持ってくる）。
        assert_eq!(
            parse("background = black\nforeground = white\n", &config, &[]).color_scheme,
            None
        );
        assert_eq!(
            parse("background = black\npalette = 1=#cc6666\n", &config, &[])
                .color_scheme
                .as_deref(),
            Some(config.as_path())
        );
        std::fs::write(themes.join("Named"), "background = black\n").unwrap();
        assert_eq!(
            parse("theme = Named\n", &config, &[themes]).color_scheme,
            None,
            "名前だけのテーマ"
        );
        assert!(parse("# 空\n", &config, &[]).is_empty());
        let _ = std::fs::remove_dir_all(&directory);
    }
}
