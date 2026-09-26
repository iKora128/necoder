//! ターミナルの配色ファイルを読む（O25・C11）。`terminal_color_scheme`（settings.json）に置いたパスの
//! ファイルから、ANSI 16 色と文字 / 背景の色を取り出して [`terminal_view::TerminalColors`] にする。
//!
//! 読める書式は 3 つで、拡張子ではなく中身で見分ける:
//! - **iTerm2 の `.itermcolors`**（plist の XML）: `Ansi 0 Color` 〜 `Ansi 15 Color`・`Background Color`・
//!   `Foreground Color`（成分は 0〜1 の実数）
//! - **Windows Terminal の scheme**（`{` で始まる JSON）: `black` 〜 `brightWhite`（紫は `purple` /
//!   `magenta`）・`background`・`foreground`。settings.json ごと渡されたら `schemes` の先頭
//! - **Ghostty のテーマ**（`key = value`）: `palette = 0=#1d1f21`・`background`・`foreground`
//!
//! 読むのは色だけ。カーソルの色は取り込まない（キャレットはプロジェクトの識別色・UI-SPEC）。

use crate::workspace::*;
use terminal_view::TerminalColors;

type Rgb = (u8, u8, u8);

/// Windows Terminal の scheme のキー（ANSI の番号順）。紫は `purple` が正式で、`magenta` も受ける。
const WINDOWS_TERMINAL_KEYS: [(&str, &str); 16] = [
    ("black", "black"),
    ("red", "red"),
    ("green", "green"),
    ("yellow", "yellow"),
    ("blue", "blue"),
    ("purple", "magenta"),
    ("cyan", "cyan"),
    ("white", "white"),
    ("brightBlack", "brightBlack"),
    ("brightRed", "brightRed"),
    ("brightGreen", "brightGreen"),
    ("brightYellow", "brightYellow"),
    ("brightBlue", "brightBlue"),
    ("brightPurple", "brightMagenta"),
    ("brightCyan", "brightCyan"),
    ("brightWhite", "brightWhite"),
];

/// 配色ファイルの中身を読む。色が 1 つも読めなければエラー（書式違いに黙って既定へ戻らない）。
pub(crate) fn parse_color_scheme(text: &str) -> anyhow::Result<TerminalColors> {
    let head = text.trim_start();
    let colors = if head.starts_with("<?xml") || head.starts_with("<plist") {
        parse_itermcolors(text)
    } else if head.starts_with('{') {
        parse_windows_terminal(text)?
    } else {
        parse_ghostty(text)
    };
    if colors == TerminalColors::default() {
        anyhow::bail!("配色が 1 つも読めない（Ghostty / Windows Terminal / iTerm2 の配色ファイルか確かめてください）");
    }
    Ok(colors)
}

/// 設定のパス（空 = 既定）から読む。読めない・書式違いは既定にして理由をログへ（端末は開ける）。
pub(crate) fn load_color_scheme(path: &str) -> TerminalColors {
    let path = path.trim();
    if path.is_empty() {
        return TerminalColors::default();
    }
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => paths::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    };
    let loaded = std::fs::read_to_string(&expanded)
        .map_err(anyhow::Error::from)
        .and_then(|text| parse_color_scheme(&text));
    match loaded {
        Ok(colors) => colors,
        Err(error) => {
            eprintln!(
                "ターミナルの配色を読めない（既定のまま）: {}: {error:#}",
                expanded.display()
            );
            TerminalColors::default()
        }
    }
}

/// `#1d1f21` / `1d1f21`（6 桁の 16 進）。
fn hex_color(value: &str) -> Option<Rgb> {
    let hex = value.trim().trim_matches('"').trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
    Some((channel(0..2)?, channel(2..4)?, channel(4..6)?))
}

fn parse_ghostty(text: &str) -> TerminalColors {
    let mut colors = TerminalColors::default();
    for line in text.lines() {
        let line = line.trim();
        // `#` で始まる行はコメント（色の値の `#` は `=` の右にしか来ない）。
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "palette" => {
                let Some((index, color)) = value.split_once('=') else {
                    continue;
                };
                let index = index
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|index| *index < 16);
                if let (Some(index), Some(color)) = (index, hex_color(color)) {
                    colors.palette[index] = Some(color);
                }
            }
            "background" => colors.background = hex_color(value).or(colors.background),
            "foreground" => colors.foreground = hex_color(value).or(colors.foreground),
            _ => {}
        }
    }
    colors
}

fn parse_windows_terminal(text: &str) -> anyhow::Result<TerminalColors> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    // settings.json ごと渡されたら `schemes` の先頭を読む。
    let scheme = match value.get("schemes").and_then(serde_json::Value::as_array) {
        Some(schemes) => schemes
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("schemes が空"))?,
        None => value,
    };
    let color = |key: &str| {
        scheme
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(hex_color)
    };
    let mut colors = TerminalColors::default();
    for (index, (key, alternative)) in WINDOWS_TERMINAL_KEYS.iter().enumerate() {
        colors.palette[index] = color(key).or_else(|| color(alternative));
    }
    colors.background = color("background");
    colors.foreground = color("foreground");
    Ok(colors)
}

fn parse_itermcolors(text: &str) -> TerminalColors {
    let mut colors = TerminalColors::default();
    for index in 0..16 {
        colors.palette[index] = itermcolors_entry(text, &format!("Ansi {index} Color"));
    }
    colors.background = itermcolors_entry(text, "Background Color");
    colors.foreground = itermcolors_entry(text, "Foreground Color");
    colors
}

/// `<key>{name}</key>` の直後の `<dict>…</dict>` から `Red / Green / Blue Component`（0〜1）を読む。
fn itermcolors_entry(text: &str, name: &str) -> Option<Rgb> {
    let key = format!("<key>{name}</key>");
    let after = &text[text.find(&key)? + key.len()..];
    let body = &after[after.find("<dict>")?..];
    let body = &body[..body.find("</dict>")?];
    let component = |component: &str| -> Option<u8> {
        let key = format!("<key>{component} Component</key>");
        let rest = &body[body.find(&key)? + key.len()..];
        let open = rest.find('>')? + 1;
        let close = rest.find("</")?;
        let value: f32 = rest.get(open..close)?.trim().parse().ok()?;
        Some((value.clamp(0.0, 1.0) * 255.0).round() as u8)
    };
    Some((component("Red")?, component("Green")?, component("Blue")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostty_themes_are_read() {
        let colors = parse_color_scheme(
            "# Tomorrow Night\npalette = 0=#1d1f21\npalette = 9=cc6666\npalette = 200=#ffffff\nbackground = 1d1f21\nforeground = #c5c8c6\ncursor-color = #ffffff\n",
        )
        .unwrap();
        assert_eq!(colors.palette[0], Some((0x1d, 0x1f, 0x21)));
        assert_eq!(colors.palette[9], Some((0xcc, 0x66, 0x66)));
        assert_eq!(colors.palette[1], None, "書いていない番号は既定");
        assert_eq!(colors.background, Some((0x1d, 0x1f, 0x21)));
        assert_eq!(colors.foreground, Some((0xc5, 0xc8, 0xc6)));
    }

    #[test]
    fn windows_terminal_schemes_are_read() {
        let scheme = r##"{ "name": "Campbell", "black": "#0C0C0C", "purple": "#881798",
            "brightMagenta": "#B4009E", "background": "#0C0C0C", "foreground": "#CCCCCC" }"##;
        let colors = parse_color_scheme(scheme).unwrap();
        assert_eq!(colors.palette[0], Some((0x0c, 0x0c, 0x0c)));
        assert_eq!(colors.palette[5], Some((0x88, 0x17, 0x98)), "紫は purple");
        assert_eq!(
            colors.palette[13],
            Some((0xb4, 0x00, 0x9e)),
            "magenta でも読む"
        );
        assert_eq!(colors.foreground, Some((0xcc, 0xcc, 0xcc)));
        let settings = format!(r#"{{ "profiles": {{}}, "schemes": [{scheme}] }}"#);
        assert_eq!(
            parse_color_scheme(&settings).unwrap(),
            colors,
            "settings.json ごとでも"
        );
    }

    #[test]
    fn itermcolors_are_read() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>Ansi 1 Color</key>
	<dict>
		<key>Alpha Component</key>
		<real>1</real>
		<key>Blue Component</key>
		<real>0.4</real>
		<key>Color Space</key>
		<string>sRGB</string>
		<key>Green Component</key>
		<real>0.4</real>
		<key>Red Component</key>
		<real>0.8</real>
	</dict>
	<key>Background Color</key>
	<dict>
		<key>Blue Component</key>
		<real>0</real>
		<key>Green Component</key>
		<real>0</real>
		<key>Red Component</key>
		<integer>1</integer>
	</dict>
</dict>
</plist>"#;
        let colors = parse_color_scheme(plist).unwrap();
        assert_eq!(colors.palette[1], Some((204, 102, 102)));
        assert_eq!(colors.background, Some((255, 0, 0)));
        assert_eq!(colors.palette[0], None);
    }

    #[test]
    fn files_without_colors_are_refused() {
        assert!(parse_color_scheme("font-size = 14\n").is_err());
        assert!(parse_color_scheme("{ \"name\": \"x\" }").is_err());
        assert!(parse_color_scheme("{ broken").is_err());
        assert_eq!(load_color_scheme(""), TerminalColors::default());
        assert_eq!(
            load_color_scheme("/definitely/not/here.itermcolors"),
            TerminalColors::default(),
            "読めなければ既定"
        );
    }
}
