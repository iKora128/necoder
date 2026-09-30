//! OS が先に取るショートカット（O27・H26）。keymap に書いても necoder まで届かない（か、OS の機能と
//! ぶつかる）キーを、⌘K ⌘S の一覧で知らせるための表。
//!
//! 書き方は keymap と同じ（`cmd-shift-3`・[`crate::user_keymap::canonical`] の並び）。`cmd` は
//! プラットフォームのキー（mac = ⌘・Windows = ⊞・Linux = Super）。表はよく知られた**既定の**物だけで、
//! OS の設定で外せる物も含む（外していれば届くので、画面は「届かないことがある」と言う）。
//! 名前（`spotlight` など）は呼び手が `key.os.<名前>` で i18n を引く。

use crate::user_keymap::canonical;
use crate::KeymapPlatform;

/// macOS（システム設定 › キーボード › キーボードショートカット の既定）。
const MAC: &[(&str, &str)] = &[
    ("cmd-space", "spotlight"),
    ("cmd-alt-space", "spotlight"),
    ("ctrl-space", "input_source"),
    ("ctrl-alt-space", "input_source"),
    ("cmd-ctrl-space", "character_viewer"),
    ("cmd-tab", "app_switcher"),
    ("cmd-shift-tab", "app_switcher"),
    ("cmd-`", "window_cycle"),
    ("cmd-shift-3", "screenshot"),
    ("cmd-shift-4", "screenshot"),
    ("cmd-shift-5", "screenshot"),
    ("cmd-ctrl-shift-3", "screenshot"),
    ("cmd-ctrl-shift-4", "screenshot"),
    ("ctrl-up", "mission_control"),
    ("ctrl-down", "mission_control"),
    ("ctrl-left", "spaces"),
    ("ctrl-right", "spaces"),
    ("cmd-alt-escape", "force_quit"),
    ("cmd-ctrl-q", "lock_screen"),
    ("cmd-alt-d", "dock"),
];

/// Windows。⊞ の組み合わせ（`cmd-…`）は全部 OS の物として別に扱う。Alt+F4 は載せない（necoder は
/// OS と同じ意味＝終了に使っている）。
const WINDOWS: &[(&str, &str)] = &[
    ("alt-tab", "app_switcher"),
    ("alt-shift-tab", "app_switcher"),
    ("ctrl-alt-delete", "secure_attention"),
    ("ctrl-shift-escape", "task_manager"),
    ("ctrl-escape", "start_menu"),
    ("alt-space", "window_menu"),
];

/// Linux（GNOME の既定・KDE も多くは同じ）。Super の組み合わせ（`cmd-…`）は全部 OS の物として別に扱う。
const LINUX: &[(&str, &str)] = &[
    ("alt-tab", "app_switcher"),
    ("alt-shift-tab", "app_switcher"),
    ("ctrl-alt-t", "terminal_launcher"),
    ("ctrl-alt-delete", "log_out"),
    ("ctrl-alt-up", "workspaces"),
    ("ctrl-alt-down", "workspaces"),
    ("ctrl-alt-left", "workspaces"),
    ("ctrl-alt-right", "workspaces"),
    ("alt-f2", "run_command"),
];

/// OS の区別（keymap の表記は Windows と Linux で共通だが、OS が取るキーは違う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    MacOs,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else {
            Os::Linux
        }
    }

    /// この OS の keymap の表記。
    pub fn keymap_platform(self) -> KeymapPlatform {
        match self {
            Os::MacOs => KeymapPlatform::MacOs,
            Os::Windows | Os::Linux => KeymapPlatform::Windows,
        }
    }
}

/// `keys`（keymap の書き方・`cmd-k cmd-s` のような続けて押すキーも可）のどれかを OS が先に取るなら、
/// その OS の機能の名前。続けて押すキーは、どの 1 打でも取られたら届かない。
pub fn os_shortcut(os: Os, keys: &str) -> Option<&'static str> {
    let table = match os {
        Os::MacOs => MAC,
        Os::Windows => WINDOWS,
        Os::Linux => LINUX,
    };
    canonical(keys).split_whitespace().find_map(|stroke| {
        if let Some((_, name)) = table.iter().find(|(reserved, _)| *reserved == stroke) {
            return Some(*name);
        }
        // Windows の ⊞・Linux の Super の組み合わせは OS（シェル）の物。
        match os {
            Os::Windows if stroke.starts_with("cmd-") => Some("windows_key"),
            Os::Linux if stroke.starts_with("cmd-") => Some("super_key"),
            _ => None,
        }
    })
}

/// 表にある名前の全部（i18n のキーがそろっているかを確かめる用）。
pub fn names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = MAC
        .iter()
        .chain(WINDOWS)
        .chain(LINUX)
        .map(|(_, name)| *name)
        .chain(["windows_key", "super_key"])
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_written_the_canonical_way() {
        for (keys, _) in MAC.iter().chain(WINDOWS).chain(LINUX) {
            assert_eq!(canonical(keys), *keys, "比べる時の書き方にそろえておく");
        }
    }

    #[test]
    fn reserved_keys_are_found_in_any_spelling_and_in_sequences() {
        assert_eq!(os_shortcut(Os::MacOs, "cmd-space"), Some("spotlight"));
        assert_eq!(os_shortcut(Os::MacOs, "shift-cmd-3"), Some("screenshot"));
        assert_eq!(
            os_shortcut(Os::MacOs, "cmd-k ctrl-space"),
            Some("input_source")
        );
        assert_eq!(os_shortcut(Os::MacOs, "cmd-shift-p"), None);
        assert_eq!(os_shortcut(Os::Windows, "win-d"), Some("windows_key"));
        assert_eq!(os_shortcut(Os::Windows, "alt-tab"), Some("app_switcher"));
        assert_eq!(os_shortcut(Os::Windows, "ctrl-shift-p"), None);
        assert_eq!(
            os_shortcut(Os::Linux, "ctrl-alt-t"),
            Some("terminal_launcher")
        );
        assert_eq!(os_shortcut(Os::Linux, "super-a"), Some("super_key"));
    }

    /// 既定の keymap のうち OS とぶつかる物（本人の判断待ち・画面は ⚠ で知らせる）。増えたらここで気付く。
    #[test]
    fn default_keymaps_collide_with_the_os_only_where_known() {
        for (os, expected) in [
            (
                Os::MacOs,
                vec![
                    ("ctrl-space", "input_source"),
                    ("cmd-shift-3", "screenshot"),
                ],
            ),
            (Os::Windows, vec![]),
            // 複数カーソル（VSCode の Windows と同じ）とスレッド送り。GNOME はワークスペースの切替。
            (
                Os::Linux,
                vec![
                    ("ctrl-alt-down", "workspaces"),
                    ("ctrl-alt-left", "workspaces"),
                    ("ctrl-alt-right", "workspaces"),
                    ("ctrl-alt-up", "workspaces"),
                ],
            ),
        ] {
            let sections = crate::parse(&crate::default_keymap_json(os.keymap_platform()))
                .expect("既定の keymap");
            let mut found: Vec<(String, &str)> = sections
                .iter()
                .flat_map(|section| section.bindings.keys())
                .filter_map(|keys| os_shortcut(os, keys).map(|name| (keys.clone(), name)))
                .collect();
            found.sort();
            found.dedup();
            let mut expected: Vec<(String, &str)> = expected
                .into_iter()
                .map(|(keys, name)| (keys.to_string(), name))
                .collect();
            expected.sort();
            assert_eq!(found, expected, "{os:?}");
        }
    }
}
