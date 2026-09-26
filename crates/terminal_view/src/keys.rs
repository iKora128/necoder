//! キー入力 → PTY へ送るバイト列。
//!
//! 符号化は 2 系統:
//! - **従来（xterm 互換）**: 制御文字（⌃A = 0x01 …）・Alt / Option の `ESC` 前置（Meta）・
//!   修飾付きの矢印 / Home / End / F1〜F4 は `CSI 1;<mod>X`、Insert / Delete / PageUp / PageDown /
//!   F5〜F12 は `CSI N;<mod>~`。修飾なしの矢印と Home / End は APP_CURSOR で `SS3` に変わる。
//! - **kitty keyboard protocol**: アプリが `CSI > flags u` でモードを push した時だけ。
//!   `CSI code;mods u` で送り、⇧Enter・⌃ / ⌥ 付きの文字・Esc の曖昧さが解ける。
//!   alacritty_terminal はモードのスタックを持つがエンコーダを持たないので、仕様
//!   <https://sw.kovidgoyal.net/kitty/keyboard-protocol/> から独立に組んでいる。
//!
//! 送る値の修飾は xterm と kitty で共通の `1 + (⇧=1 | ⌥=2 | ⌃=4)`。⌘（Windows キー）付きは
//! アプリのショートカットなので端末へは送らない。
//!
//! ⇧Enter は kitty なしでは `ESC CR`（Claude Code の `/terminal-setup` が Alacritty 向けに
//! 設定するのと同じ列 = 改行として受け取られる）、kitty の DISAMBIGUATE では `CSI 13;2u`。
//!
//! **テンキーの APP_KEYPAD（DECKPAM）は扱えない**: GPUI の `Keystroke` はテンキーと本体の
//! 数字・Enter を区別しない（どちらも `"1"` / `"enter"`）ので、テンキーだけを `SS3` に変える
//! 手がかりが無い。本体キーの挙動は APP_KEYPAD に依らないので、ここでは何も変えない。

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// 修飾のビット。送る値は `1 + ビット和`。
const SHIFT: u8 = 1;
const ALT: u8 = 2;
const CONTROL: u8 = 4;

const ESC: u8 = 0x1b;

/// 文字を生まないキー。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NamedKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    /// F1〜F12。
    Function(u8),
}

impl NamedKey {
    fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "enter" => Self::Enter,
            "tab" => Self::Tab,
            "backspace" => Self::Backspace,
            "escape" => Self::Escape,
            "up" => Self::Up,
            "down" => Self::Down,
            "right" => Self::Right,
            "left" => Self::Left,
            "home" => Self::Home,
            "end" => Self::End,
            "insert" => Self::Insert,
            "delete" => Self::Delete,
            "pageup" => Self::PageUp,
            "pagedown" => Self::PageDown,
            _ => {
                let number = key.strip_prefix('f')?.parse::<u8>().ok()?;
                if !(1..=12).contains(&number) {
                    return None;
                }
                Self::Function(number)
            }
        })
    }
}

/// 名前付きキーの「番号 + 終端文字」。`Letter` は `CSI 1;<mod>X`（修飾なしは `CSI X` / `SS3 X`）、
/// `Tilde` は `CSI N;<mod>~`。
#[derive(Clone, Copy)]
enum Functional {
    Letter(u8),
    Tilde(u8),
}

fn functional(key: NamedKey, kitty: bool) -> Option<Functional> {
    Some(match key {
        NamedKey::Up => Functional::Letter(b'A'),
        NamedKey::Down => Functional::Letter(b'B'),
        NamedKey::Right => Functional::Letter(b'C'),
        NamedKey::Left => Functional::Letter(b'D'),
        NamedKey::Home => Functional::Letter(b'H'),
        NamedKey::End => Functional::Letter(b'F'),
        NamedKey::Insert => Functional::Tilde(2),
        NamedKey::Delete => Functional::Tilde(3),
        NamedKey::PageUp => Functional::Tilde(5),
        NamedKey::PageDown => Functional::Tilde(6),
        NamedKey::Function(1) => Functional::Letter(b'P'),
        NamedKey::Function(2) => Functional::Letter(b'Q'),
        // kitty は `CSI 1;<mod>R` がカーソル位置の報告と衝突するので F3 だけ `13~` にする。
        NamedKey::Function(3) if kitty => Functional::Tilde(13),
        NamedKey::Function(3) => Functional::Letter(b'R'),
        NamedKey::Function(4) => Functional::Letter(b'S'),
        NamedKey::Function(number) => Functional::Tilde(match number {
            5 => 15,
            6 => 17,
            7 => 18,
            8 => 19,
            9 => 20,
            10 => 21,
            11 => 23,
            _ => 24,
        }),
        NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace | NamedKey::Escape => return None,
    })
}

/// キーストロークを PTY へ送るバイト列へ。送るものが無ければ `None`（⌘ 付き・未対応キー）。
///
/// `mode` は端末のモード（APP_CURSOR と kitty のフラグを見る）。`repeat` はキーの押しっぱなしによる
/// 自動反復（kitty の REPORT_EVENT_TYPES で `:2` を付ける）。
pub(crate) fn keystroke_to_bytes(
    keystroke: &Keystroke,
    mode: TermMode,
    repeat: bool,
) -> Option<Vec<u8>> {
    let modifiers = keystroke.modifiers;
    if modifiers.platform {
        return None;
    }
    let composed = composed_text(keystroke);
    let mut bits = 0;
    if modifiers.shift {
        bits |= SHIFT;
    }
    // Option / AltGr で記号を打った時は、その文字が答え（修飾として送らない）。
    if composed.is_none() {
        if modifiers.alt {
            bits |= ALT;
        }
        if modifiers.control {
            bits |= CONTROL;
        }
    }
    let named = NamedKey::from_key(&keystroke.key);
    let kitty =
        mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC);
    if kitty {
        if let Some(bytes) = kitty_bytes(keystroke, named, composed, bits, mode, repeat) {
            return Some(bytes);
        }
    }
    match named {
        Some(key) => Some(legacy_named(key, bits, mode)),
        None => legacy_text(keystroke, composed, bits),
    }
}

/// Option（mac）/ AltGr（Windows・ctrl+alt で届く）の層で ASCII の記号を打った時、その文字。
///
/// ドイツ配列の ⌥L = `@`、JIS の ⌥¥ = `\` のように、記号を打つのに Option が要る配列がある。
/// これを Meta（`ESC` 前置）にすると記号が打てなくなるので、**ASCII の記号が出来た時だけ**は
/// 文字をそのまま送る。⌥F = `ƒ` のような ASCII 外の文字になった時は Meta として扱う
/// （readline の単語移動など、端末で Option に期待されるのはこちら）。
fn composed_text(keystroke: &Keystroke) -> Option<&str> {
    if !keystroke.modifiers.alt {
        return None;
    }
    let text = keystroke.key_char.as_deref()?;
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() || !character.is_ascii_graphic() {
        return None;
    }
    // ⇧ で大文字になっただけ（Alt+Shift+F → "F"）は合成ではない。
    if keystroke.key.eq_ignore_ascii_case(text) {
        return None;
    }
    Some(text)
}

/// 従来（xterm）の名前付きキー。
fn legacy_named(key: NamedKey, bits: u8, mode: TermMode) -> Vec<u8> {
    let alt = bits & ALT != 0;
    let with_escape = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if alt {
            out.push(ESC);
        }
        out.extend_from_slice(bytes);
        out
    };
    match key {
        // ⇧Enter は ESC CR（Claude Code / Codex が改行として受け取る列）。⌥Enter も同じ。
        NamedKey::Enter if bits & (SHIFT | ALT) != 0 => vec![ESC, b'\r'],
        NamedKey::Enter => vec![b'\r'],
        NamedKey::Tab if bits & SHIFT != 0 => with_escape(b"\x1b[Z"),
        NamedKey::Tab => with_escape(b"\t"),
        NamedKey::Backspace if bits & CONTROL != 0 => with_escape(&[0x08]),
        NamedKey::Backspace => with_escape(&[0x7f]),
        NamedKey::Escape => with_escape(&[ESC]),
        _ => match functional(key, false) {
            Some(Functional::Letter(letter)) if bits == 0 => {
                // 修飾なし: 矢印と Home / End は APP_CURSOR で SS3。F1〜F4 は常に SS3。
                let cursor_key = !matches!(key, NamedKey::Function(_));
                if !cursor_key || mode.contains(TermMode::APP_CURSOR) {
                    vec![ESC, b'O', letter]
                } else {
                    vec![ESC, b'[', letter]
                }
            }
            Some(Functional::Letter(letter)) => {
                let mut out = format!("\x1b[1;{}", 1 + bits).into_bytes();
                out.push(letter);
                out
            }
            Some(Functional::Tilde(number)) if bits == 0 => format!("\x1b[{number}~").into_bytes(),
            Some(Functional::Tilde(number)) => format!("\x1b[{number};{}~", 1 + bits).into_bytes(),
            None => Vec::new(),
        },
    }
}

/// ⌃ + キー → 制御文字（xterm の対応表）。対応が無ければ None。
fn control_byte(key: &str) -> Option<u8> {
    if key == "space" {
        return Some(0);
    }
    let mut characters = key.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    Some(match character {
        'a'..='z' => character as u8 - b'a' + 1,
        'A'..='Z' => character as u8 - b'A' + 1,
        '@' | '2' | ' ' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Meta（`ESC` 前置）で送る元の文字。⇧ 付きの英字は大文字。
fn meta_base(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.as_str();
    if key == "space" {
        return Some(" ".to_string());
    }
    let mut characters = key.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    Some(if keystroke.modifiers.shift {
        character.to_uppercase().collect()
    } else {
        character.to_string()
    })
}

/// 印字キーの確定文字（IME 確定などは `EntityInputHandler` 側の経路）。制御文字は送らない。
fn typed_text(keystroke: &Keystroke) -> Option<&str> {
    let text = keystroke.key_char.as_deref()?;
    (!text.is_empty() && !text.chars().any(char::is_control)).then_some(text)
}

/// 従来（xterm）の文字キー。
fn legacy_text(keystroke: &Keystroke, composed: Option<&str>, bits: u8) -> Option<Vec<u8>> {
    if let Some(text) = composed {
        return Some(text.as_bytes().to_vec());
    }
    let alt = bits & ALT != 0;
    if bits & CONTROL != 0 {
        let byte = control_byte(&keystroke.key)?;
        return Some(if alt { vec![ESC, byte] } else { vec![byte] });
    }
    if alt {
        let base = meta_base(keystroke)?;
        let mut out = vec![ESC];
        out.extend_from_slice(base.as_bytes());
        return Some(out);
    }
    typed_text(keystroke).map(|text| text.as_bytes().to_vec())
}

/// kitty の `CSI` 列を組む。`number` が 1 で他の欄が無い時は省く（`CSI A`）。
fn kitty_sequence(
    number: u32,
    alternate: Option<u32>,
    bits: u8,
    event_type: Option<u8>,
    text: Option<&str>,
    final_byte: u8,
) -> Vec<u8> {
    let mut out = String::from("\x1b[");
    let modifier_field = bits != 0 || event_type.is_some() || text.is_some();
    if number != 1 || alternate.is_some() || modifier_field || final_byte == b'u' {
        out.push_str(&number.to_string());
    }
    if let Some(alternate) = alternate {
        out.push(':');
        out.push_str(&alternate.to_string());
    }
    if modifier_field {
        out.push(';');
        out.push_str(&(1 + bits).to_string());
        if let Some(event_type) = event_type {
            out.push(':');
            out.push_str(&event_type.to_string());
        }
    }
    if let Some(text) = text {
        out.push(';');
        let codepoints: Vec<String> = text
            .chars()
            .map(|character| (character as u32).to_string())
            .collect();
        out.push_str(&codepoints.join(":"));
    }
    let mut bytes = out.into_bytes();
    bytes.push(final_byte);
    bytes
}

/// kitty keyboard protocol での符号化。従来の列でよいキーは None（呼び元が従来で送る）。
fn kitty_bytes(
    keystroke: &Keystroke,
    named: Option<NamedKey>,
    composed: Option<&str>,
    bits: u8,
    mode: TermMode,
    repeat: bool,
) -> Option<Vec<u8>> {
    let report_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let event_type = (repeat && mode.contains(TermMode::REPORT_EVENT_TYPES)).then_some(2);
    match named {
        Some(key) => {
            // DISAMBIGUATE だけの時、修飾なしの Enter / Tab / Backspace と機能キーは従来の列のまま
            // （アプリが落ちてモードが残っても `reset` を打てるように・仕様の例外）。Esc は常に CSI u。
            if !report_all && bits == 0 && key != NamedKey::Escape {
                return None;
            }
            let (number, final_byte) = match key {
                NamedKey::Escape => (27, b'u'),
                NamedKey::Enter => (13, b'u'),
                NamedKey::Tab => (9, b'u'),
                NamedKey::Backspace => (127, b'u'),
                _ => match functional(key, true)? {
                    Functional::Letter(letter) => (1, letter),
                    Functional::Tilde(number) => (u32::from(number), b'~'),
                },
            };
            Some(kitty_sequence(
                number, None, bits, event_type, None, final_byte,
            ))
        }
        None => {
            // DISAMBIGUATE だけの時、⌃ / ⌥ の無い文字（⇧ 付きを含む）はそのまま文字で送る。
            if !report_all && bits & (CONTROL | ALT) == 0 {
                return None;
            }
            if composed.is_some() && !report_all {
                return None;
            }
            let code = kitty_key_code(&keystroke.key)?;
            // ⇧ で変わった文字（REPORT_ALTERNATE_KEYS）。GPUI は記号の ⇧ を畳むので英字だけ。
            let alternate = (bits & SHIFT != 0 && mode.contains(TermMode::REPORT_ALTERNATE_KEYS))
                .then(|| char::from_u32(code))
                .flatten()
                .filter(char::is_ascii_lowercase)
                .map(|character| character.to_ascii_uppercase() as u32);
            // 文字を生むキーの文字（REPORT_ASSOCIATED_TEXT・REPORT_ALL の時だけ意味を持つ）。
            let text = (report_all && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT))
                .then(|| composed.or_else(|| typed_text(keystroke)))
                .flatten()
                .filter(|_| bits & (CONTROL | ALT) == 0);
            Some(kitty_sequence(
                code, alternate, bits, event_type, text, b'u',
            ))
        }
    }
}

/// kitty のキー番号 = ⇧ を外した文字の Unicode（英字は小文字）。
fn kitty_key_code(key: &str) -> Option<u32> {
    if key == "space" {
        return Some(' ' as u32);
    }
    let mut characters = key.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    Some(character.to_ascii_lowercase() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `"shift-enter"` のような表記に、実際に届く確定文字（key_char）を添えて組む。
    fn key(source: &str, key_char: Option<&str>) -> Keystroke {
        let mut keystroke = Keystroke::parse(source).expect("キー表記");
        keystroke.key_char = key_char.map(str::to_string);
        keystroke
    }

    fn send(source: &str, key_char: Option<&str>, mode: TermMode) -> Vec<u8> {
        keystroke_to_bytes(&key(source, key_char), mode, false).unwrap_or_default()
    }

    fn legacy(source: &str, key_char: Option<&str>) -> Vec<u8> {
        send(source, key_char, TermMode::default())
    }

    fn kitty(source: &str, key_char: Option<&str>) -> Vec<u8> {
        send(
            source,
            key_char,
            TermMode::default() | TermMode::DISAMBIGUATE_ESC_CODES,
        )
    }

    #[test]
    fn shift_enter_inserts_a_newline_for_agent_clis() {
        // Enter は従来どおり CR（IME のテストもこの前提）。
        assert_eq!(legacy("enter", None), b"\r");
        // kitty なし: ⇧Enter = ESC CR（Claude Code の /terminal-setup と同じ列）。
        assert_eq!(legacy("shift-enter", None), b"\x1b\r");
        assert_eq!(legacy("alt-enter", None), b"\x1b\r");
        assert_eq!(legacy("ctrl-enter", None), b"\r");
        // kitty の DISAMBIGUATE: ⇧Enter = CSI 13;2u、素の Enter は CR のまま。
        assert_eq!(kitty("shift-enter", None), b"\x1b[13;2u");
        assert_eq!(kitty("enter", None), b"\r");
    }

    #[test]
    fn kitty_disambiguates_escape_and_modified_text_keys() {
        assert_eq!(kitty("escape", None), b"\x1b[27u");
        assert_eq!(kitty("ctrl-c", None), b"\x1b[99;5u");
        assert_eq!(kitty("alt-a", Some("å")), b"\x1b[97;3u");
        assert_eq!(kitty("ctrl-shift-a", None), b"\x1b[97;6u");
        assert_eq!(kitty("shift-tab", None), b"\x1b[9;2u");
        assert_eq!(kitty("alt-backspace", None), b"\x1b[127;3u");
        // 修飾の無い文字・⇧ だけの文字は文字のまま。
        assert_eq!(kitty("a", Some("a")), b"a");
        assert_eq!(kitty("shift-a", Some("A")), b"A");
        assert_eq!(kitty("tab", None), b"\t");
        assert_eq!(kitty("backspace", None), b"\x7f");
        // 機能キー: 修飾なしは従来、修飾付きは CSI 1;<mod>X（F3 は 13~）。
        assert_eq!(kitty("up", None), b"\x1b[A");
        assert_eq!(kitty("ctrl-up", None), b"\x1b[1;5A");
        assert_eq!(kitty("shift-f3", None), b"\x1b[13;2~");
        // ⌘ は送らない。
        assert!(keystroke_to_bytes(&key("cmd-c", None), TermMode::default(), false).is_none());
    }

    #[test]
    fn kitty_report_all_keys_and_optional_fields() {
        let all = TermMode::default()
            | TermMode::DISAMBIGUATE_ESC_CODES
            | TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(send("a", Some("a"), all), b"\x1b[97u");
        assert_eq!(send("enter", None, all), b"\x1b[13u");
        assert_eq!(send("up", None, all), b"\x1b[A");
        // ⇧ で変わった文字（REPORT_ALTERNATE_KEYS）。
        assert_eq!(
            send("shift-a", Some("A"), all | TermMode::REPORT_ALTERNATE_KEYS),
            b"\x1b[97:65;2u"
        );
        // 文字（REPORT_ASSOCIATED_TEXT）。修飾欄は 1 を明示する。
        assert_eq!(
            send("a", Some("a"), all | TermMode::REPORT_ASSOCIATED_TEXT),
            b"\x1b[97;1;97u"
        );
        // 押しっぱなしの反復（REPORT_EVENT_TYPES）。
        let repeat = keystroke_to_bytes(
            &key("ctrl-a", None),
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES,
            true,
        );
        assert_eq!(repeat.as_deref(), Some(&b"\x1b[97;5:2u"[..]));
    }

    #[test]
    fn alt_sends_escape_prefix_unless_the_layout_composed_a_symbol() {
        assert_eq!(legacy("alt-f", Some("ƒ")), b"\x1bf");
        assert_eq!(legacy("alt-shift-f", Some("Ï")), b"\x1bF");
        assert_eq!(legacy("alt-.", Some("≥")), b"\x1b.");
        assert_eq!(legacy("alt-space", Some("\u{a0}")), b"\x1b ");
        assert_eq!(legacy("alt-backspace", None), b"\x1b\x7f");
        // ドイツ配列の ⌥L = @、JIS の ⌥¥ = \ は記号として送る（Meta にしない）。
        assert_eq!(legacy("alt-l", Some("@")), b"@");
        assert_eq!(legacy("alt-¥", Some("\\")), b"\\");
        // Windows の AltGr（ctrl+alt で届く）も同じ。
        assert_eq!(legacy("ctrl-alt-q", Some("@")), b"@");
    }

    #[test]
    fn modified_cursor_and_editing_keys_use_xterm_parameters() {
        assert_eq!(legacy("shift-up", None), b"\x1b[1;2A");
        assert_eq!(legacy("ctrl-left", None), b"\x1b[1;5D");
        assert_eq!(legacy("alt-right", None), b"\x1b[1;3C");
        assert_eq!(legacy("ctrl-shift-end", None), b"\x1b[1;6F");
        assert_eq!(legacy("home", None), b"\x1b[H");
        assert_eq!(legacy("shift-delete", None), b"\x1b[3;2~");
        assert_eq!(legacy("ctrl-pageup", None), b"\x1b[5;5~");
        assert_eq!(legacy("pagedown", None), b"\x1b[6~");
        assert_eq!(legacy("insert", None), b"\x1b[2~");
        assert_eq!(legacy("shift-tab", None), b"\x1b[Z");
        // APP_CURSOR: 修飾なしだけ SS3。修飾付きは CSI のまま。
        let app_cursor = TermMode::default() | TermMode::APP_CURSOR;
        assert_eq!(send("up", None, app_cursor), b"\x1bOA");
        assert_eq!(send("end", None, app_cursor), b"\x1bOF");
        assert_eq!(send("ctrl-up", None, app_cursor), b"\x1b[1;5A");
    }

    #[test]
    fn function_keys() {
        assert_eq!(legacy("f1", None), b"\x1bOP");
        assert_eq!(legacy("f4", None), b"\x1bOS");
        assert_eq!(legacy("f5", None), b"\x1b[15~");
        assert_eq!(legacy("f12", None), b"\x1b[24~");
        assert_eq!(legacy("shift-f1", None), b"\x1b[1;2P");
        assert_eq!(legacy("ctrl-f5", None), b"\x1b[15;5~");
        assert!(legacy("f13", None).is_empty());
    }

    #[test]
    fn control_characters() {
        assert_eq!(legacy("ctrl-a", None), [0x01]);
        assert_eq!(legacy("ctrl-shift-a", None), [0x01]);
        assert_eq!(legacy("ctrl-[", None), [0x1b]);
        assert_eq!(legacy("ctrl-space", None), [0x00]);
        assert_eq!(legacy("ctrl-/", None), [0x1f]);
        assert_eq!(legacy("ctrl-\\", None), [0x1c]);
        assert_eq!(legacy("ctrl-alt-a", None), [0x1b, 0x01]);
        assert_eq!(legacy("ctrl-backspace", None), [0x08]);
        assert_eq!(legacy("escape", None), [0x1b]);
        // 印字キーは確定文字をそのまま。
        assert_eq!(legacy("a", Some("a")), b"a");
        assert_eq!(legacy("shift-a", Some("A")), b"A");
        assert_eq!(legacy("space", Some(" ")), b" ");
    }
}
