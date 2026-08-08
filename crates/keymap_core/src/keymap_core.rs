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
use serde::Deserialize;
use std::collections::BTreeMap;
use std::rc::Rc;

/// keymap の 1 セクション（1 つのコンテキスト述語に対する束）。
#[derive(Debug, Clone, Deserialize)]
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

/// 組み込み既定 keymap（macOS・Zed 互換ベース）。編集アクションは `editor` 名前空間、終了は `shirushi`。
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
      "ctrl--": "workspace::NavigateBack",
      "ctrl-shift--": "workspace::NavigateForward",
      "cmd-,": "workspace::OpenSettings",
      "cmd-m": "workspace::Minimize",
      "cmd-h": "workspace::Hide",
      "cmd-alt-h": "workspace::HideOthers",
      "cmd-q": "shirushi::Quit"
    }
  }
]"#;

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
            Some("shirushi::Quit")
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
