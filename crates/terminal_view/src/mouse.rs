//! マウスの報告（アプリが `CSI ? 1000 / 1002 / 1003 h` で有効にした時・vim / tmux / TUI）。
//!
//! 報告の単位（どの操作を送るか）:
//! - MOUSE_REPORT_CLICK（1000）: 押す・離す・ホイール
//! - MOUSE_DRAG（1002）: 上に加えて、ボタンを押したままの移動
//! - MOUSE_MOTION（1003）: 上に加えて、ボタンを押していない移動も
//!
//! 符号化: SGR（1006・`CSI < b ; x ; y M` / 離す時は `m`）が有効ならそれ、UTF-8（1005）なら
//! 座標を UTF-8 の文字で、どちらも無ければ X10 形式（`CSI M` + 3 byte・座標は 223 まで）。
//! ボタンの値は 左 0 / 中 1 / 右 2 / 離す 3（X10 と UTF-8 のみ）/ ホイール上 64・下 65、移動は +32、
//! 修飾は ⇧ 4 / ⌥ 8 / ⌃ 16。座標は 1 始まり。
//!
//! ⇧ を押している間は報告せず、従来どおり端末の選択にする（一般的な端末の流儀）。

use alacritty_terminal::term::TermMode;
use gpui::Modifiers;

/// 報告するボタン。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReportButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// ボタンを押していない移動（MOUSE_MOTION）。
    NoButton,
}

impl ReportButton {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::NoButton => 3,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }

    pub(crate) fn from_gpui(button: gpui::MouseButton) -> Option<Self> {
        match button {
            gpui::MouseButton::Left => Some(Self::Left),
            gpui::MouseButton::Middle => Some(Self::Middle),
            gpui::MouseButton::Right => Some(Self::Right),
            gpui::MouseButton::Navigate(_) => None,
        }
    }
}

/// 報告する操作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReportAction {
    Press,
    Release,
    Motion,
}

/// アプリがマウスの報告を求めているか（⇧ を押している間は端末の選択を優先して報告しない）。
pub(crate) fn wants_report(mode: TermMode, modifiers: Modifiers) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !modifiers.shift
}

/// この移動を報告するか（1003 は常に、1002 はボタンを押している時だけ）。
pub(crate) fn wants_motion(mode: TermMode, button_held: bool) -> bool {
    mode.contains(TermMode::MOUSE_MOTION) || (button_held && mode.contains(TermMode::MOUSE_DRAG))
}

/// 1 つの報告のバイト列。`column` / `line` は 0 始まりの表示座標。X10 形式で座標が
/// 表せない（223 を超える）時は None（送らない）。
pub(crate) fn encode(
    button: ReportButton,
    action: ReportAction,
    column: usize,
    line: usize,
    modifiers: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mut code = button.code();
    if action == ReportAction::Motion {
        code += 32;
    }
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    let (x, y) = (column + 1, line + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let final_byte = if action == ReportAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{final_byte}").into_bytes());
    }
    // X10 / UTF-8 は離す時にどのボタンかを持たない（3 = 離した）。
    if action == ReportAction::Release {
        code = (code & !0b11) | 3;
    }
    let mut bytes = b"\x1b[M".to_vec();
    bytes.push(32 + code);
    if mode.contains(TermMode::UTF8_MOUSE) {
        for value in [x, y] {
            // UTF-8 の 2 byte で表せる所まで（1005 の約束）。
            let encoded = u32::try_from(32 + value)
                .ok()
                .filter(|code| *code <= 0x7ff)?;
            let character = char::from_u32(encoded)?;
            let mut buffer = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    } else {
        for value in [x, y] {
            bytes.push(u8::try_from(32 + value).ok()?);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(shift: bool, alt: bool, control: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            control,
            ..Modifiers::default()
        }
    }

    const NONE: Modifiers = Modifiers {
        control: false,
        alt: false,
        shift: false,
        platform: false,
        function: false,
    };

    fn sgr() -> TermMode {
        TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE
    }

    #[test]
    fn sgr_reports_press_release_motion_and_wheel() {
        let press = encode(ReportButton::Left, ReportAction::Press, 4, 2, NONE, sgr());
        assert_eq!(press.as_deref(), Some(&b"\x1b[<0;5;3M"[..]));
        // 離す時は同じボタンで終端が `m`。
        let release = encode(ReportButton::Left, ReportAction::Release, 4, 2, NONE, sgr());
        assert_eq!(release.as_deref(), Some(&b"\x1b[<0;5;3m"[..]));
        let drag = encode(ReportButton::Left, ReportAction::Motion, 5, 2, NONE, sgr());
        assert_eq!(drag.as_deref(), Some(&b"\x1b[<32;6;3M"[..]));
        let hover = encode(
            ReportButton::NoButton,
            ReportAction::Motion,
            0,
            0,
            NONE,
            sgr(),
        );
        assert_eq!(hover.as_deref(), Some(&b"\x1b[<35;1;1M"[..]));
        let wheel = encode(
            ReportButton::WheelDown,
            ReportAction::Press,
            0,
            9,
            NONE,
            sgr(),
        );
        assert_eq!(wheel.as_deref(), Some(&b"\x1b[<65;1;10M"[..]));
        // 修飾: ⌥ 8 + ⌃ 16。
        let right = encode(
            ReportButton::Right,
            ReportAction::Press,
            0,
            0,
            modifiers(false, true, true),
            sgr(),
        );
        assert_eq!(right.as_deref(), Some(&b"\x1b[<26;1;1M"[..]));
        // 大きい座標も SGR ならそのまま。
        let far = encode(ReportButton::Left, ReportAction::Press, 299, 0, NONE, sgr());
        assert_eq!(far.as_deref(), Some(&b"\x1b[<0;300;1M"[..]));
    }

    #[test]
    fn x10_and_utf8_encodings() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        let press = encode(ReportButton::Left, ReportAction::Press, 0, 0, NONE, mode);
        assert_eq!(press.as_deref(), Some(&[0x1b, b'[', b'M', 32, 33, 33][..]));
        // 離す時はボタンが 3 になる。
        let release = encode(ReportButton::Right, ReportAction::Release, 1, 1, NONE, mode);
        assert_eq!(
            release.as_deref(),
            Some(&[0x1b, b'[', b'M', 35, 34, 34][..])
        );
        // X10 は 223 列を超えると表せない（送らない）。
        assert!(encode(ReportButton::Left, ReportAction::Press, 223, 0, NONE, mode).is_none());
        // UTF-8（1005）なら座標を UTF-8 の文字で送れる。
        let utf8 = encode(
            ReportButton::Left,
            ReportAction::Press,
            299,
            0,
            NONE,
            mode | TermMode::UTF8_MOUSE,
        )
        .unwrap_or_default();
        assert_eq!(&utf8[..4], &[0x1b, b'[', b'M', 32]);
        assert_eq!(String::from_utf8_lossy(&utf8[4..]), "\u{14c}!");
    }

    #[test]
    fn shift_keeps_the_local_selection() {
        let mode = sgr();
        assert!(wants_report(mode, NONE));
        assert!(!wants_report(mode, modifiers(true, false, false)));
        assert!(!wants_report(TermMode::default(), NONE));
        // 1002 はボタンを押している移動だけ、1003 は全部。
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert!(wants_motion(drag, true));
        assert!(!wants_motion(drag, false));
        assert!(wants_motion(TermMode::MOUSE_MOTION, false));
        assert!(!wants_motion(sgr(), true));
    }
}
