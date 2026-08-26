//! keymap_core — JSON keymap をコンテキスト述語つきで gpui の [`KeyBinding`] に変換する。
//!
//! 形式（Zed 互換の縮小版）:
//! ```json
//! [
//!   { "context": "Editor", "bindings": { "cmd-s": "editor::Save", "enter": "editor::Newline" } }
//! ]
//! ```
//! アクションは名前（`namespace::Name`）で参照し、実行時に [`App::build_action`] で解決する
//! （＝この crate は具体アクション型に依存しない）。未登録アクション・不正述語・不正キーは
//! **その項目だけ skip** して警告し、残りは活かす（部分ロード）。

use anyhow::{Context as _, Result};
use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::rc::Rc;

/// keymap の 1 セクション（1 つのコンテキスト述語に対する束）。
///
/// `Serialize` は非 mac の既定 keymap を mac 版から機械変換して書き戻すために要る（§D4）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeymapSection {
    /// コンテキスト述語（例 `"Editor"`, `"Workspace && !menu"`）。空なら全域。
    #[serde(default)]
    pub context: String,
    /// キーストローク → アクション名。
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
}

/// JSON をセクション列にパースする（構造の検証のみ・アクション解決はしない）。
pub fn parse(json: &str) -> Result<Vec<KeymapSection>> {
    serde_json::from_str(json).context("keymap JSON の解析に失敗")
}

/// JSON keymap を [`KeyBinding`] 列へ変換する。壊れた項目は skip して警告（部分ロード）。
pub fn load_bindings(json: &str, cx: &App) -> Result<Vec<KeyBinding>> {
    let sections = parse(json)?;
    let mapper = cx.keyboard_mapper().clone();
    let mut bindings = Vec::new();

    for section in sections {
        let predicate: Option<Rc<KeyBindingContextPredicate>> = if section.context.trim().is_empty()
        {
            None
        } else {
            match KeyBindingContextPredicate::parse(&section.context) {
                Ok(predicate) => Some(Rc::new(predicate)),
                Err(error) => {
                    eprintln!(
                        "keymap: context `{}` の述語が不正: {error}",
                        section.context
                    );
                    continue;
                }
            }
        };

        for (keystrokes, action_name) in &section.bindings {
            let action = match cx.build_action(action_name, None) {
                Ok(action) => action,
                Err(error) => {
                    eprintln!("keymap: アクション `{action_name}` を解決できない: {error}");
                    continue;
                }
            };
            match KeyBinding::load(
                keystrokes,
                action,
                predicate.clone(),
                false,
                None,
                mapper.as_ref(),
            ) {
                Ok(binding) => bindings.push(binding),
                Err(error) => eprintln!("keymap: キー `{keystrokes}` が不正: {error}"),
            }
        }
    }
    Ok(bindings)
}

/// アクション名からキーストローク表記を逆引きする（コマンドパレットのキー併記・M13）。
/// 複数バインドがあれば最初のセクション順で 1 つ。
pub fn key_for_action(sections: &[KeymapSection], action: &str) -> Option<String> {
    for section in sections {
        for (keystrokes, bound_action) in &section.bindings {
            if bound_action == action {
                return Some(keystrokes.clone());
            }
        }
    }
    None
}

/// **mac 表記のキーストローク 1 つ**を、このプラットフォームの表示用ラベルにする。
///
/// UI 側は既定 keymap と同じ `cmd-` 表記で書けばよく、`⌘O` / `Ctrl+O` の出し分けは
/// ここが持つ。記号を直書きすると Windows / Linux に mac の記号が出る
/// （2026-08-22・ウェルカム画面で実際に発生した）。
///
/// `cmd-shift-p` → mac: `⌘⇧P` / Windows: `Ctrl+Shift+P`
pub fn keystroke_label(mac_keystroke: &str) -> String {
    keystroke_label_for(KeymapPlatform::current(), mac_keystroke)
}

pub fn keystroke_label_for(platform: KeymapPlatform, mac_keystroke: &str) -> String {
    match platform {
        KeymapPlatform::MacOs => pretty_keystroke(mac_keystroke),
        KeymapPlatform::Windows => {
            // 既定 keymap と同じ変換規則を通す（表示と実際のキーがずれないため）
            let converted = to_non_mac_keystroke(mac_keystroke)
                .unwrap_or_else(|| mac_keystroke.replace("cmd-", "ctrl-"));
            windows_keystroke(&converted)
        }
    }
}

/// キーストロークをこのプラットフォームの慣例表記へ（WINDOWS-PORT.md §D4）。
///
/// mac は記号（`⌘⇧P`）、Windows / Linux は綴り（`Ctrl+Shift+P`）。パレット・メニューの
/// キー併記はすべてここを通るので、**1 箇所直せば全域に波及する**。
pub fn pretty_keystroke_for(platform: KeymapPlatform, keystrokes: &str) -> String {
    match platform {
        KeymapPlatform::MacOs => pretty_keystroke(keystrokes),
        KeymapPlatform::Windows => windows_keystroke(keystrokes),
    }
}

/// Windows / Linux の慣例表記（`ctrl-shift-p` → `Ctrl+Shift+P`）。
fn windows_keystroke(keystrokes: &str) -> String {
    keystrokes
        .split_whitespace()
        .map(|chord| {
            let mut parts: Vec<String> = Vec::new();
            let mut key = "";
            for part in chord.split('-') {
                match part {
                    // Windows では ⌘ に相当するものが無い。keymap に cmd- が残っていたら
                    // ユーザーが自分で書いた行なので、そのまま Win キーとして見せる。
                    "cmd" => parts.push("Win".to_string()),
                    "ctrl" => parts.push("Ctrl".to_string()),
                    "alt" => parts.push("Alt".to_string()),
                    "shift" => parts.push("Shift".to_string()),
                    other => key = other,
                }
            }
            let key = match key {
                "enter" => "Enter".to_string(),
                "escape" => "Esc".to_string(),
                "space" => "Space".to_string(),
                "backspace" => "Backspace".to_string(),
                "delete" => "Delete".to_string(),
                "tab" => "Tab".to_string(),
                "left" => "Left".to_string(),
                "right" => "Right".to_string(),
                "up" => "Up".to_string(),
                "down" => "Down".to_string(),
                "home" => "Home".to_string(),
                "end" => "End".to_string(),
                other if other.len() == 1 => other.to_uppercase(),
                other => {
                    if other.starts_with('f') && other[1..].chars().all(|c| c.is_ascii_digit()) {
                        other.to_uppercase()
                    } else {
                        other.to_string()
                    }
                }
            };
            parts.push(key);
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// キーストロークを macOS 慣例の記号表記へ（`cmd-shift-p` → `⌘⇧P`、`cmd-k cmd-t` → `⌘K ⌘T`）。
pub fn pretty_keystroke(keystrokes: &str) -> String {
    keystrokes
        .split_whitespace()
        .map(|chord| {
            let mut out = String::new();
            let mut key = "";
            for part in chord.split('-') {
                match part {
                    "cmd" => out.push('⌘'),
                    "ctrl" => out.push('⌃'),
                    "alt" => out.push('⌥'),
                    "shift" => out.push('⇧'),
                    "fn" => out.push_str("fn"),
                    other => key = other,
                }
            }
            let key = match key {
                "enter" => "⏎".to_string(),
                "escape" => "esc".to_string(),
                "space" => "Space".to_string(),
                "backspace" => "⌫".to_string(),
                "tab" => "Tab".to_string(),
                "left" => "←".to_string(),
                "right" => "→".to_string(),
                "up" => "↑".to_string(),
                "down" => "↓".to_string(),
                other if other.len() == 1 => other.to_uppercase(),
                other => {
                    // f2 → F2、それ以外（{ } - 等の記号）はそのまま。
                    if other.starts_with('f') && other[1..].chars().all(|c| c.is_ascii_digit()) {
                        other.to_uppercase()
                    } else {
                        other.to_string()
                    }
                }
            };
            format!("{out}{key}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 組み込み既定 keymap（macOS・Zed 互換ベース）。編集アクションは `editor` 名前空間、終了は `necoder`。
pub const DEFAULT_KEYMAP_JSON: &str = r#"[
  {
    "context": "Editor",
    "bindings": {
      "backspace": "editor::Backspace",
      "delete": "editor::Delete",
      "enter": "editor::Newline",
      "shift-enter": "editor::InsertNewline",
      "left": "editor::MoveLeft",
      "right": "editor::MoveRight",
      "up": "editor::MoveUp",
      "down": "editor::MoveDown",
      "home": "editor::MoveToLineStart",
      "end": "editor::MoveToLineEnd",
      "cmd-left": "editor::MoveToLineStart",
      "cmd-right": "editor::MoveToLineEnd",
      "ctrl-a": "editor::MoveToLineStart",
      "ctrl-e": "editor::MoveToLineEnd",
      "shift-left": "editor::SelectLeft",
      "shift-right": "editor::SelectRight",
      "shift-up": "editor::SelectUp",
      "shift-down": "editor::SelectDown",
      "cmd-a": "editor::SelectAll",
      "cmd-c": "editor::Copy",
      "cmd-x": "editor::Cut",
      "cmd-v": "editor::Paste",
      "cmd-z": "editor::Undo",
      "cmd-shift-z": "editor::Redo",
      "cmd-s": "workspace::SaveActive",
      "alt-shift-f": "workspace::Format",
      "cmd-enter": "agent::SubmitPrompt",
      "f12": "workspace::GoToDefinition",
      "f2": "workspace::Rename",
      "cmd-.": "workspace::CodeActions",
      "shift-f12": "workspace::FindReferences",
      "cmd-shift-o": "workspace::OutlineSymbols",
      "cmd-t": "workspace::WorkspaceSymbols",
      "f8": "workspace::NextDiagnostic",
      "shift-f8": "workspace::PrevDiagnostic",
      "f7": "workspace::NextHunk",
      "shift-f7": "workspace::PrevHunk",
      "ctrl-space": "workspace::TriggerCompletion",
      "cmd-k cmd-i": "workspace::ShowHover",
      "alt-left": "editor::MoveWordLeft",
      "alt-right": "editor::MoveWordRight",
      "shift-alt-left": "editor::SelectWordLeft",
      "shift-alt-right": "editor::SelectWordRight",
      "alt-backspace": "editor::DeleteWordBackward",
      "cmd-up": "editor::MoveToStart",
      "cmd-down": "editor::MoveToEnd",
      "alt-up": "editor::MoveLineUp",
      "alt-down": "editor::MoveLineDown",
      "shift-alt-up": "editor::DuplicateLineUp",
      "shift-alt-down": "editor::DuplicateLineDown",
      "cmd-shift-k": "editor::DeleteLine",
      "cmd-/": "editor::ToggleComment",
      "tab": "editor::TabIndent",
      "shift-tab": "editor::Outdent",
      "cmd-]": "editor::Indent",
      "cmd-[": "editor::Outdent",
      "cmd-d": "editor::SelectNext",
      "alt-z": "editor::ToggleSoftWrap",
      "cmd-shift-v": "editor::ToggleRenderedMarkdown",
      "ctrl-g": "workspace::GoToLine",
      "alt-cmd-up": "editor::AddCursorAbove",
      "alt-cmd-down": "editor::AddCursorBelow",
      "escape": "editor::Cancel"
    }
  },
  {
    "context": "AgentPanel",
    "bindings": {
      "cmd-w": "agent::CloseActiveThread",
      "cmd-a": "editor::SelectAll",
      "cmd-c": "editor::Copy"
    }
  },
  {
    "context": "FleetControl",
    "bindings": {
      "enter": "workspace::ControlNext"
    }
  },
  {
    "bindings": {
      "cmd-p": "workspace::FileFinder",
      "cmd-shift-p": "workspace::CommandPalette",
      "cmd-k cmd-s": "workspace::ShortcutSheet",
      "cmd-o": "workspace::ProjectSwitcher",
      "cmd-f": "workspace::BufferSearch",
      "cmd-alt-f": "workspace::BufferReplace",
      "cmd-shift-f": "workspace::ProjectSearch",
      "cmd-shift-t": "workspace::RestoreClosedTab",
      "cmd-k cmd-t": "workspace::ThemeSelector",
      "cmd-k cmd-c": "workspace::ProjectColor",
      "cmd-j": "workspace::ToggleTerminal",
      "cmd-i": "workspace::InlineEdit",
      "cmd-\\": "workspace::SplitRight",
      "ctrl-shift-g": "workspace::ToggleGitPanel",
      "cmd-w": "workspace::CloseTab",
      "cmd-}": "workspace::SelectNextTab",
      "cmd-{": "workspace::SelectPrevTab",
      "cmd-shift-a": "workspace::NewThread",
      "cmd-shift-enter": "workspace::ToggleAgentFullScreen",
      "cmd-shift-h": "workspace::ThreadHistory",
      "cmd-alt-right": "workspace::SelectNextThread",
      "cmd-alt-left": "workspace::SelectPrevThread",
      "ctrl-tab": "workspace::SelectNextThread",
      "ctrl-shift-tab": "workspace::SelectPrevThread",
      "cmd-shift-n": "workspace::NewWindow",
      "cmd-1": "workspace::ActivateProject1",
      "cmd-2": "workspace::ActivateProject2",
      "cmd-3": "workspace::ActivateProject3",
      "cmd-4": "workspace::ActivateProject4",
      "cmd-5": "workspace::ActivateProject5",
      "cmd-6": "workspace::ActivateProject6",
      "cmd-7": "workspace::ActivateProject7",
      "cmd-8": "workspace::ActivateProject8",
      "cmd-9": "workspace::ActivateProject9",
      "ctrl-cmd-down": "workspace::NextProject",
      "ctrl-cmd-up": "workspace::PrevProject",
      "ctrl--": "workspace::NavigateBack",
      "ctrl-shift--": "workspace::NavigateForward",
      "cmd-,": "workspace::OpenSettings",
      "cmd-m": "workspace::Minimize",
      "cmd-h": "workspace::Hide",
      "cmd-alt-h": "workspace::HideOthers",
      "cmd-q": "necoder::Quit"
    }
  }
]"#;

/// 既定 keymap と記号表記を出し分けるプラットフォーム（WINDOWS-PORT.md §D4）。
///
/// Linux は Windows 側を共有する（`ctrl-` 系・VSCode 準拠）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapPlatform {
    MacOs,
    /// Windows と Linux。
    Windows,
}

impl KeymapPlatform {
    pub fn current() -> KeymapPlatform {
        if cfg!(target_os = "macos") {
            KeymapPlatform::MacOs
        } else {
            KeymapPlatform::Windows
        }
    }
}

/// mac 固有すぎて非 mac では**落とす**バインド。
///
/// - `ctrl-a` / `ctrl-e`: emacs 風の行頭・行末。**`ctrl-a` は SelectAll と衝突する**うえ、
///   Windows の作法でもない。`home` / `end` が既に同じ動作を持っているので失うものは無い
/// - `cmd-left` / `cmd-right`: 同上（Windows の `ctrl-left/right` は「単語単位」を意味する）
/// - `cmd-h` / `cmd-alt-h`: アプリの Hide / Hide Others は **macOS だけの概念**
/// - `cmd-m`: Minimize。Windows はタイトルバーの担当で、`ctrl-m` は別の意味を持つ
const NON_MAC_DROPPED: &[&str] = &[
    "ctrl-a",
    "ctrl-e",
    "cmd-left",
    "cmd-right",
    "cmd-h",
    "cmd-alt-h",
    "cmd-m",
];

/// 機械変換（`cmd-` → `ctrl-`）では正しくならないものを明示的に置き換える。**VSCode 準拠**。
///
/// ここに無いものは `cmd-` → `ctrl-` の単純変換で足りる。
const NON_MAC_REPLACEMENTS: &[(&str, &str)] = &[
    // 単語単位の移動・選択・削除は Windows では ctrl 側（mac は alt 側）
    ("alt-left", "ctrl-left"),
    ("alt-right", "ctrl-right"),
    ("shift-alt-left", "ctrl-shift-left"),
    ("shift-alt-right", "ctrl-shift-right"),
    ("alt-backspace", "ctrl-backspace"),
    // 文書の先頭・末尾
    ("cmd-up", "ctrl-home"),
    ("cmd-down", "ctrl-end"),
    // 複数カーソル（VSCode Windows は ctrl-alt-up/down）
    ("alt-cmd-up", "ctrl-alt-up"),
    ("alt-cmd-down", "ctrl-alt-down"),
    // 置換は VSCode Windows では ctrl-h（単純変換だと ctrl-alt-f になってしまう）
    ("cmd-alt-f", "ctrl-h"),
    // 終了は Windows の作法
    ("cmd-q", "alt-f4"),
    // プロジェクト送り。単純変換だと ctrl-alt-up/down（複数カーソル）と衝突するのでずらす。
    // VSCode に相当機能が無く前例が引けないため、これは necoder 独自の割り当て。
    ("ctrl-cmd-down", "ctrl-alt-shift-down"),
    ("ctrl-cmd-up", "ctrl-alt-shift-up"),
];

/// 非 mac にだけ足すバインド（mac 側には対応物が無い、その OS の定番）。
const NON_MAC_ADDITIONS: &[(&str, &str, &str)] = &[
    // Windows の Redo は伝統的に ctrl-y。ctrl-shift-z（機械変換の結果）と併存させる。
    ("Editor", "ctrl-y", "editor::Redo"),
];

/// 1 つのキーストロークを非 mac 向けへ変換する。落とす場合は `None`。
fn to_non_mac_keystroke(keystroke: &str) -> Option<String> {
    if NON_MAC_DROPPED.contains(&keystroke) {
        return None;
    }
    if let Some((_, replacement)) = NON_MAC_REPLACEMENTS
        .iter()
        .find(|(mac, _)| *mac == keystroke)
    {
        return Some((*replacement).to_string());
    }
    Some(keystroke.replace("cmd-", "ctrl-"))
}

/// このプラットフォームの既定 keymap（JSON）。
///
/// **mac 版（[`DEFAULT_KEYMAP_JSON`]）が唯一の正**で、非 mac 版はそこから機械変換で作る。
/// 2 本の JSON を手で並べると必ず片方だけ直されて腐るため、差分は上の 3 つの表にだけ持つ。
///
/// 衝突（2 つの mac バインドが同じ非 mac キーへ落ちて片方が黙って消える）は
/// unit test が止める。
pub fn default_keymap_json(platform: KeymapPlatform) -> String {
    if platform == KeymapPlatform::MacOs {
        return DEFAULT_KEYMAP_JSON.to_string();
    }
    let mut sections = parse(DEFAULT_KEYMAP_JSON).unwrap_or_default();
    for section in &mut sections {
        let mut translated = BTreeMap::new();
        for (keystroke, action) in &section.bindings {
            if let Some(converted) = to_non_mac_keystroke(keystroke) {
                translated.insert(converted, action.clone());
            }
        }
        for (context, keystroke, action) in NON_MAC_ADDITIONS {
            if section.context == *context {
                translated.insert((*keystroke).to_string(), (*action).to_string());
            }
        }
        section.bindings = translated;
    }
    serde_json::to_string_pretty(&sections).unwrap_or_else(|_| DEFAULT_KEYMAP_JSON.to_string())
}

#[cfg(test)]
mod windows_keymap_tests {
    use super::*;

    fn windows_sections() -> Vec<KeymapSection> {
        parse(&default_keymap_json(KeymapPlatform::Windows))
            .expect("非 mac の既定 keymap がパースできる")
    }

    /// **一番大事なテスト**: 2 つの mac バインドが同じ Windows キーへ落ちて
    /// 片方が黙って消えていないか。BTreeMap は後勝ちで上書きするので、
    /// これが無いと「なぜか効かないキーがある」という形で表に出る。
    #[test]
    fn no_two_mac_bindings_collapse_onto_one_windows_key() {
        for section in parse(DEFAULT_KEYMAP_JSON).expect("mac の既定 keymap") {
            let mut seen: BTreeMap<String, String> = BTreeMap::new();
            for (keystroke, action) in &section.bindings {
                let Some(converted) = to_non_mac_keystroke(keystroke) else {
                    continue;
                };
                if let Some(previous) = seen.insert(converted.clone(), keystroke.clone()) {
                    panic!(
                        "context `{}` で `{previous}` と `{keystroke}` が同じ `{converted}` に落ちる\
                         （片方が黙って消える）。NON_MAC_REPLACEMENTS で振り分けること。\
                         アクション: {action}",
                        section.context
                    );
                }
            }
        }
    }

    /// **意図して**非 mac から落とすアクション。macOS だけの概念なので代替キーも用意しない。
    const INTENTIONALLY_UNBOUND_ON_NON_MAC: &[&str] = &[
        "workspace::Hide",       // アプリを隠す = macOS の概念
        "workspace::HideOthers", // 同上
        "workspace::Minimize",   // Windows はタイトルバーの担当
    ];

    /// 落としたバインドのアクションが、**意図せず**到達できなくなっていないか。
    ///
    /// 例: `ctrl-a`（mac の emacs 風 行頭）を落としても `home` が同じ動作を持つので失われない。
    /// 一方 `workspace::Hide` は代替を用意せず意図的に落とす＝上の allowlist に載せる。
    #[test]
    fn dropping_mac_only_bindings_loses_no_action() {
        let mac = parse(DEFAULT_KEYMAP_JSON).expect("mac の既定 keymap");
        let windows = windows_sections();
        for (index, section) in mac.iter().enumerate() {
            let reachable: std::collections::BTreeSet<&String> =
                windows[index].bindings.values().collect();
            for action in section.bindings.values() {
                if INTENTIONALLY_UNBOUND_ON_NON_MAC.contains(&action.as_str()) {
                    assert!(
                        !reachable.contains(action),
                        "`{action}` は macOS 専用として落とすはずが非 mac に残っている"
                    );
                    continue;
                }
                assert!(
                    reachable.contains(action),
                    "`{action}` が非 mac から到達できなくなっている（context `{}`）。\
                     意図的なら INTENTIONALLY_UNBOUND_ON_NON_MAC に足すこと",
                    section.context
                );
            }
        }
    }

    #[test]
    fn windows_defaults_follow_vscode_conventions() {
        let sections = windows_sections();
        let editor = &sections[0].bindings;
        // ctrl-a は SelectAll。mac の emacs 風 ctrl-a（行頭）に奪われていないこと
        assert_eq!(editor.get("ctrl-a").map(String::as_str), Some("editor::SelectAll"));
        assert_eq!(editor.get("ctrl-s").map(String::as_str), Some("workspace::SaveActive"));
        // 単語単位は ctrl 側（mac の alt 側から移した）
        assert_eq!(editor.get("ctrl-left").map(String::as_str), Some("editor::MoveWordLeft"));
        // 文書の先頭・末尾
        assert_eq!(editor.get("ctrl-home").map(String::as_str), Some("editor::MoveToStart"));
        // Windows の Redo は ctrl-y も効く
        assert_eq!(editor.get("ctrl-y").map(String::as_str), Some("editor::Redo"));
        assert_eq!(editor.get("ctrl-shift-z").map(String::as_str), Some("editor::Redo"));
        // 行頭・行末は home/end が持っている（cmd-left/right を落としても失われない）
        assert_eq!(editor.get("home").map(String::as_str), Some("editor::MoveToLineStart"));

        // パレット・ファインダ等は context 無しのグローバルセクションにある
        let global = sections
            .iter()
            .find(|section| section.context.is_empty())
            .expect("グローバルセクション");
        assert_eq!(
            global.bindings.get("ctrl-shift-p").map(String::as_str),
            Some("workspace::CommandPalette")
        );
        assert_eq!(
            global.bindings.get("ctrl-p").map(String::as_str),
            Some("workspace::FileFinder")
        );
        // 置換は VSCode Windows と同じ ctrl-h（機械変換だと ctrl-alt-f になってしまう）
        assert_eq!(
            global.bindings.get("ctrl-h").map(String::as_str),
            Some("workspace::BufferReplace")
        );
        // 終了は Windows の作法
        assert_eq!(
            global.bindings.get("alt-f4").map(String::as_str),
            Some("necoder::Quit")
        );
    }

    /// macOS 固有の概念は非 mac へ持ち込まない。
    #[test]
    fn macos_only_concepts_are_dropped() {
        let json = default_keymap_json(KeymapPlatform::Windows);
        assert!(!json.contains("workspace::Hide\""), "Hide は macOS だけの概念");
        assert!(!json.contains("workspace::HideOthers"), "HideOthers は macOS だけの概念");
        // cmd- が 1 つも残っていない
        assert!(!json.contains("\"cmd-"), "非 mac の既定に cmd- が残っている");
    }

    /// **mac 側は 1 文字も変わらない**（§D8）。
    #[test]
    fn macos_default_is_returned_verbatim() {
        assert_eq!(default_keymap_json(KeymapPlatform::MacOs), DEFAULT_KEYMAP_JSON);
    }

    #[test]
    fn windows_keystrokes_are_spelled_out() {
        assert_eq!(
            pretty_keystroke_for(KeymapPlatform::Windows, "ctrl-shift-p"),
            "Ctrl+Shift+P"
        );
        assert_eq!(pretty_keystroke_for(KeymapPlatform::Windows, "ctrl-s"), "Ctrl+S");
        assert_eq!(pretty_keystroke_for(KeymapPlatform::Windows, "alt-f4"), "Alt+F4");
        assert_eq!(pretty_keystroke_for(KeymapPlatform::Windows, "f2"), "F2");
        assert_eq!(
            pretty_keystroke_for(KeymapPlatform::Windows, "ctrl-k ctrl-i"),
            "Ctrl+K Ctrl+I"
        );
        // mac 側は従来どおり記号
        assert_eq!(pretty_keystroke_for(KeymapPlatform::MacOs, "cmd-shift-p"), "⌘⇧P");
    }

    /// UI から `cmd-` 表記で引いたラベルが、そのプラットフォームで**実際に押すキー**と一致すること。
    /// （ウェルカム画面が `⌘O` を直書きしていて Windows で mac 記号が出ていた・2026-08-22）
    #[test]
    fn keystroke_labels_match_what_the_user_actually_presses() {
        assert_eq!(keystroke_label_for(KeymapPlatform::MacOs, "cmd-o"), "⌘O");
        assert_eq!(keystroke_label_for(KeymapPlatform::Windows, "cmd-o"), "Ctrl+O");
        assert_eq!(
            keystroke_label_for(KeymapPlatform::Windows, "cmd-shift-p"),
            "Ctrl+Shift+P"
        );
        // 置き換え表を通るものはラベルもそちらに従う（表示と実キーがずれない）
        assert_eq!(keystroke_label_for(KeymapPlatform::Windows, "cmd-alt-f"), "Ctrl+H");
        assert_eq!(keystroke_label_for(KeymapPlatform::Windows, "cmd-q"), "Alt+F4");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_lookup_and_pretty_print() {
        let sections = parse(DEFAULT_KEYMAP_JSON).expect("既定 keymap がパースできる");
        // 逆引き（コマンドパレットのキー併記・M13）。
        let key = key_for_action(&sections, "workspace::FileFinder").expect("⌘P がある");
        assert_eq!(key, "cmd-p");
        assert!(key_for_action(&sections, "workspace::存在しない").is_none());
        // 記号表記。
        assert_eq!(pretty_keystroke("cmd-shift-p"), "⌘⇧P");
        assert_eq!(pretty_keystroke("cmd-k cmd-t"), "⌘K ⌘T");
        assert_eq!(pretty_keystroke("ctrl-shift-g"), "⌃⇧G");
        assert_eq!(pretty_keystroke("f2"), "F2");
        assert_eq!(pretty_keystroke("alt-enter"), "⌥⏎");
        assert_eq!(pretty_keystroke("cmd-}"), "⌘}");
    }

    #[test]
    fn parses_sections_and_bindings() {
        let sections = parse(DEFAULT_KEYMAP_JSON).expect("既定 keymap がパースできる");
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].context, "Editor");
        // ⌘S は保存時フォーマットのフックのため workspace 側（M11）。
        assert_eq!(
            sections[0].bindings.get("cmd-s").map(String::as_str),
            Some("workspace::SaveActive")
        );
        // 2 セクション目は AgentPanel（⌘W でアクティブスレッドを閉じる、
        // transcript にフォーカスがある時は ⌘A / ⌘C で出力を選択・コピーする）。
        assert_eq!(sections[1].context, "AgentPanel");
        assert_eq!(
            sections[1].bindings.get("cmd-w").map(String::as_str),
            Some("agent::CloseActiveThread")
        );
        assert_eq!(
            sections[1].bindings.get("cmd-a").map(String::as_str),
            Some("editor::SelectAll")
        );
        assert_eq!(
            sections[1].bindings.get("cmd-c").map(String::as_str),
            Some("editor::Copy")
        );
        // 3 セクション目は管制（⏎ = 要対応キューの先頭へ・P3）
        assert_eq!(sections[2].context, "FleetControl");
        assert_eq!(
            sections[2].bindings.get("enter").map(String::as_str),
            Some("workspace::ControlNext")
        );
        // 末尾は全域（context 空）+ Quit
        assert!(sections[3].context.is_empty());
        assert_eq!(
            sections[3].bindings.get("cmd-q").map(String::as_str),
            Some("necoder::Quit")
        );
    }

    #[test]
    fn missing_fields_default_gracefully() {
        // context 省略・bindings 省略でも壊れない
        let sections = parse(r#"[{ "bindings": { "esc": "app::Escape" } }, {}]"#).unwrap();
        assert_eq!(sections.len(), 2);
        assert!(sections[0].context.is_empty());
        assert!(sections[1].bindings.is_empty());
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse("not json").is_err());
    }
}
