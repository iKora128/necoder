//! キー割り当ての画面（O27・D23 / H09）のための純ロジック。既定の keymap とユーザーの keymap.json を
//! 重ねた「いま効いている割り当て」を、アクションごとの行にする。画面で割り当てを変える・戻すと、
//! ユーザーの keymap（[`KeymapSection`] の列）だけを書き換える（既定は触らない）。
//!
//! 重ね方は gpui と同じ: 同じコンテキストの同じキーはユーザーが勝ち、[`NO_ACTION`] はそのキーを外す。
//! コンテキストは**文字列の一致**で束ねる（述語の包含までは解かない）。

use crate::{KeymapSection, NO_ACTION};
use std::collections::BTreeMap;

/// gpui のキー 1 つを keymap の書き方にする。修飾キーは `cmd` → `ctrl` → `alt` → `shift` → `fn` の順
/// （既定の keymap の多くと同じ `cmd-shift-p`）。プラットフォームの修飾キー（mac の ⌘・Windows の Win・
/// Linux の Super）は `cmd` と書く（gpui はどれでも読む）。
pub fn keystroke_text(keystroke: &gpui::Keystroke) -> String {
    let modifiers = &keystroke.modifiers;
    let mut text = String::new();
    for (on, name) in [
        (modifiers.platform, "cmd-"),
        (modifiers.control, "ctrl-"),
        (modifiers.alt, "alt-"),
        (modifiers.shift, "shift-"),
        (modifiers.function, "fn-"),
    ] {
        if on {
            text.push_str(name);
        }
    }
    text.push_str(&keystroke.key);
    text
}

/// キーの書き方をそろえる（`shift-alt-up` と `alt-shift-up` は同じキー・比べる時に使う）。
/// gpui が読めない物はそのまま（一致しないだけ）。
pub fn canonical(keystrokes: &str) -> String {
    keystrokes
        .split_whitespace()
        .map(|chord| match gpui::Keystroke::parse(chord) {
            Ok(keystroke) => keystroke_text(&keystroke),
            Err(_) => chord.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 1 つのコンテキストの 1 つのアクションに、いま効いているキー（画面の 1 行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionBinding {
    /// コンテキスト述語（`""` = 全域）。
    pub context: String,
    /// アクション名（`workspace::FileFinder` など）。
    pub action: String,
    /// いま効いているキー（既定のうちユーザーが外していない物と、ユーザーが足した物・キー順）。
    /// 空 = 割り当てが無い（ユーザーが全部外した既定のアクション）。
    pub keys: Vec<String>,
    /// ユーザーの keymap がこの行を変えている（足した・外した）。画面は「戻す」を出す。
    pub customized: bool,
}

fn section<'a>(sections: &'a [KeymapSection], context: &str) -> Option<&'a KeymapSection> {
    sections.iter().find(|section| section.context == context)
}

/// 既定の `context` で `action` に割り当てたキー（書き方は既定のまま）。
fn default_keys(defaults: &[KeymapSection], context: &str, action: &str) -> Vec<String> {
    section(defaults, context)
        .into_iter()
        .flat_map(|section| section.bindings.iter())
        .filter(|(_, bound)| bound.as_str() == action)
        .map(|(key, _)| key.clone())
        .collect()
}

/// `keys` に `key` と同じキー（書き方の違いは問わない）があるか。
fn contains_key(keys: &[String], key: &str) -> bool {
    let key = canonical(key);
    keys.iter().any(|own| canonical(own) == key)
}

/// 既定とユーザーの keymap を重ね、アクションごとの行にする。並びは既定のセクション順（ユーザーにしか
/// 無いコンテキストはその後）、セクションの中はアクション名の順。[`NO_ACTION`] は行にしない。
pub fn effective_bindings(
    defaults: &[KeymapSection],
    user: &[KeymapSection],
) -> Vec<ActionBinding> {
    let mut contexts: Vec<&str> = Vec::new();
    for section in defaults.iter().chain(user) {
        if !contexts.contains(&section.context.as_str()) {
            contexts.push(section.context.as_str());
        }
    }
    let mut rows = Vec::new();
    for context in contexts {
        let base = section(defaults, context);
        let overlay = section(user, context);
        // 同じキー（書き方の違いは問わない）はユーザーが勝つ。値 = (見せる書き方, アクション)。
        let mut merged: BTreeMap<String, (&str, &str)> = BTreeMap::new();
        for (key, action) in base
            .into_iter()
            .chain(overlay)
            .flat_map(|section| &section.bindings)
        {
            merged.insert(canonical(key), (key.as_str(), action.as_str()));
        }
        // アクション → キー。既定にあってユーザーが全部外した物も行に残す（割り当て直せるように）。
        let mut actions: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for action in base
            .into_iter()
            .flat_map(|section| section.bindings.values())
        {
            if action != NO_ACTION {
                actions.entry(action).or_default();
            }
        }
        for (key, action) in merged.into_values() {
            if action != NO_ACTION {
                actions.entry(action).or_default().push(key.to_string());
            }
        }
        for (action, keys) in actions {
            let own_defaults = default_keys(defaults, context, action);
            let customized = overlay.is_some_and(|overlay| {
                overlay
                    .bindings
                    .iter()
                    .any(|(key, bound)| bound == action || contains_key(&own_defaults, key))
            });
            rows.push(ActionBinding {
                context: context.to_string(),
                action: action.to_string(),
                keys,
                customized,
            });
        }
    }
    rows
}

/// ユーザーの `context` のセクション（無ければ末尾に作る）。
fn user_section<'a>(user: &'a mut Vec<KeymapSection>, context: &str) -> &'a mut KeymapSection {
    let position = match user.iter().position(|section| section.context == context) {
        Some(position) => position,
        None => {
            user.push(KeymapSection {
                context: context.to_string(),
                bindings: BTreeMap::new(),
            });
            user.len() - 1
        }
    };
    &mut user[position]
}

/// 空になったセクションを落とす（`{ "context": "Editor", "bindings": {} }` を残さない）。
fn drop_empty_sections(user: &mut Vec<KeymapSection>) {
    user.retain(|section| !section.bindings.is_empty());
}

/// `context` の `action` を `keystroke` 1 つにする。ユーザーが前に足したこのアクションのキーは消し、
/// 既定のキーは [`NO_ACTION`] で外す（別のアクションへ割り当て直してあるキーはそのまま）。
/// `keystroke` が既定で別のアクションのキーなら、そのアクションからは外れる（画面が先に確かめる）。
pub fn rebind(
    user: &mut Vec<KeymapSection>,
    defaults: &[KeymapSection],
    context: &str,
    action: &str,
    keystroke: &str,
) {
    let own_defaults = default_keys(defaults, context, action);
    let wanted = canonical(keystroke);
    let section = user_section(user, context);
    // このアクションに前に足したキーと、同じキーの別の書き方の行は消す（1 つのキーに 2 行を作らない）。
    section
        .bindings
        .retain(|key, bound| bound != action && canonical(key) != wanted);
    for key in own_defaults {
        let own = canonical(&key);
        let already = section
            .bindings
            .keys()
            .any(|existing| canonical(existing) == own);
        if own != wanted && !already {
            section.bindings.insert(key, NO_ACTION.to_string());
        }
    }
    section
        .bindings
        .insert(keystroke.to_string(), action.to_string());
    drop_empty_sections(user);
}

/// `context` の `action` を既定に戻す: ユーザーが足したこのアクションのキーを消し、外していた既定の
/// キーを戻す（そのキーを別のアクションへ割り当て直していれば、それは残す）。
pub fn reset(
    user: &mut Vec<KeymapSection>,
    defaults: &[KeymapSection],
    context: &str,
    action: &str,
) {
    let own_defaults = default_keys(defaults, context, action);
    if let Some(section) = user.iter_mut().find(|section| section.context == context) {
        section.bindings.retain(|key, bound| {
            bound != action && !(bound == NO_ACTION && contains_key(&own_defaults, key))
        });
    }
    drop_empty_sections(user);
}

/// `keystroke` をいま使っている行（`except` の行は除く）。コンテキストは問わない（深いコンテキストの
/// 割り当ては浅い方を隠すので、どれも知らせる価値がある）。
pub fn conflicts<'a>(
    rows: &'a [ActionBinding],
    keystroke: &str,
    except: (&str, &str),
) -> Vec<&'a ActionBinding> {
    rows.iter()
        .filter(|row| (row.context.as_str(), row.action.as_str()) != except)
        .filter(|row| contains_key(&row.keys, keystroke))
        .collect()
}

/// ユーザーの keymap.json の中身を読む。空・空白だけは「何も無い」。
pub fn parse_user(json: &str) -> anyhow::Result<Vec<KeymapSection>> {
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    crate::parse(json)
}

/// ユーザーの keymap を keymap.json の中身にする（整形・末尾に改行）。
pub fn to_json(user: &[KeymapSection]) -> String {
    let mut json = serde_json::to_string_pretty(user).unwrap_or_else(|_| "[]".to_string());
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(json: &str) -> Vec<KeymapSection> {
        crate::parse(json).expect("keymap")
    }

    fn defaults() -> Vec<KeymapSection> {
        sections(
            r#"[
              { "context": "", "bindings": { "cmd-p": "workspace::FileFinder", "cmd-o": "workspace::Open" } },
              { "context": "Editor", "bindings": { "cmd-z": "editor::Undo", "ctrl-z": "editor::Undo", "cmd-s": "editor::Save" } }
            ]"#,
        )
    }

    fn row<'a>(rows: &'a [ActionBinding], context: &str, action: &str) -> &'a ActionBinding {
        rows.iter()
            .find(|row| row.context == context && row.action == action)
            .expect("行がある")
    }

    #[test]
    fn user_bindings_are_laid_over_the_defaults() {
        let defaults = defaults();
        let user = sections(
            r#"[
              { "context": "Editor", "bindings": { "ctrl-z": "zed::NoAction", "cmd-shift-s": "editor::Save" } },
              { "context": "Terminal", "bindings": { "cmd-t": "terminal::NewTab" } }
            ]"#,
        );
        let rows = effective_bindings(&defaults, &user);
        let undo = row(&rows, "Editor", "editor::Undo");
        assert_eq!(undo.keys, vec!["cmd-z".to_string()], "外したキーは消える");
        assert!(undo.customized);
        let save = row(&rows, "Editor", "editor::Save");
        assert_eq!(
            save.keys,
            vec!["cmd-s".to_string(), "cmd-shift-s".to_string()]
        );
        assert!(save.customized);
        assert!(!row(&rows, "", "workspace::FileFinder").customized);
        assert_eq!(
            rows.last().map(|row| row.context.as_str()),
            Some("Terminal"),
            "ユーザーにしか無いコンテキストは後ろ"
        );
        assert!(rows.iter().all(|row| row.action != NO_ACTION));
    }

    #[test]
    fn rebinding_replaces_the_keys_and_reset_brings_them_back() {
        let defaults = defaults();
        let mut user = Vec::new();
        rebind(&mut user, &defaults, "Editor", "editor::Undo", "cmd-u");
        let rows = effective_bindings(&defaults, &user);
        assert_eq!(
            row(&rows, "Editor", "editor::Undo").keys,
            vec!["cmd-u".to_string()]
        );
        // 2 回目は前に足したキーを置き換える（増やさない）。
        rebind(&mut user, &defaults, "Editor", "editor::Undo", "cmd-y");
        let rows = effective_bindings(&defaults, &user);
        assert_eq!(
            row(&rows, "Editor", "editor::Undo").keys,
            vec!["cmd-y".to_string()]
        );
        // 既定のキーを選び直すと、その既定のキーは外さない。
        rebind(&mut user, &defaults, "Editor", "editor::Undo", "cmd-z");
        let rows = effective_bindings(&defaults, &user);
        assert_eq!(
            row(&rows, "Editor", "editor::Undo").keys,
            vec!["cmd-z".to_string()]
        );

        reset(&mut user, &defaults, "Editor", "editor::Undo");
        assert!(user.is_empty(), "戻すと何も残らない: {user:?}");
        let rows = effective_bindings(&defaults, &user);
        let undo = row(&rows, "Editor", "editor::Undo");
        assert_eq!(undo.keys, vec!["cmd-z".to_string(), "ctrl-z".to_string()]);
        assert!(!undo.customized);
    }

    #[test]
    fn a_taken_key_is_reported_and_moves_on_rebind() {
        let defaults = defaults();
        let mut user = Vec::new();
        let rows = effective_bindings(&defaults, &user);
        let taken = conflicts(&rows, "cmd-p", ("Editor", "editor::Save"));
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].action, "workspace::FileFinder");
        assert!(conflicts(&rows, "cmd-p", ("", "workspace::FileFinder")).is_empty());

        // 同じコンテキストで別のアクションのキーを取ると、そちらから外れる。
        rebind(&mut user, &defaults, "", "workspace::Open", "cmd-p");
        let rows = effective_bindings(&defaults, &user);
        assert_eq!(
            row(&rows, "", "workspace::Open").keys,
            vec!["cmd-p".to_string()]
        );
        assert!(row(&rows, "", "workspace::FileFinder").keys.is_empty());
        // Open を戻すと cmd-o が戻り、cmd-p は FileFinder に戻る。
        reset(&mut user, &defaults, "", "workspace::Open");
        let rows = effective_bindings(&defaults, &user);
        assert_eq!(
            row(&rows, "", "workspace::FileFinder").keys,
            vec!["cmd-p".to_string()]
        );
        assert_eq!(
            row(&rows, "", "workspace::Open").keys,
            vec!["cmd-o".to_string()]
        );
    }

    /// 修飾キーの順が違うだけの書き方は同じキー（既定の `shift-alt-up` とユーザーの `alt-shift-up`）。
    #[test]
    fn spellings_of_the_same_key_are_the_same_key() {
        assert_eq!(canonical("shift-alt-up"), canonical("alt-shift-up"));
        assert_eq!(canonical("shift-cmd-p"), "cmd-shift-p");
        assert_eq!(canonical("cmd-k cmd-s"), "cmd-k cmd-s");
        let defaults = sections(
            r#"[{ "context": "Editor", "bindings": { "shift-alt-up": "editor::DuplicateLineUp" } }]"#,
        );
        let user = sections(
            r#"[{ "context": "Editor", "bindings": { "alt-shift-up": "editor::MoveLineUp" } }]"#,
        );
        let rows = effective_bindings(&defaults, &user);
        assert!(row(&rows, "Editor", "editor::DuplicateLineUp")
            .keys
            .is_empty());
        assert_eq!(
            row(&rows, "Editor", "editor::MoveLineUp").keys,
            vec!["alt-shift-up".to_string()]
        );
        assert_eq!(
            conflicts(&rows, "shift-alt-up", ("Editor", "other")).len(),
            1,
            "書き方が違っても当たる"
        );
        let mut user = user;
        rebind(
            &mut user,
            &defaults,
            "Editor",
            "editor::DuplicateLineUp",
            "shift-alt-up",
        );
        assert_eq!(
            user[0].bindings.len(),
            1,
            "同じキーに 2 行を作らない: {user:?}"
        );
    }

    #[test]
    fn user_files_round_trip() {
        assert!(parse_user("  \n").expect("空").is_empty());
        assert!(parse_user("{ broken").is_err());
        let mut user = Vec::new();
        rebind(
            &mut user,
            &defaults(),
            "Editor",
            "editor::Save",
            "cmd-shift-s",
        );
        let json = to_json(&user);
        assert_eq!(parse_user(&json).expect("読める").len(), 1);
        assert!(json.ends_with('\n'));
    }
}
