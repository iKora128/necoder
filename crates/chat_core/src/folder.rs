//! folder — **チャットのフォルダ**の名前と寿命（`docs/CHAT.md` §2）。
//!
//! 成果物はユーザーのものなので、Finder で探せる場所に普通のファイルとして置く:
//! `<書類>/necoder/<YYYY-MM-DD 最初のメッセージの先頭>/artifacts/<成果物>`。
//!
//! 決まりごとは 3 つ:
//!
//! 1. **名前は作成後に変えない。** Claude Code はセッションを cwd のパス文字列で引くので、
//!    スレッドの改名に追従させると `session/load` が前回の会話を見つけられなくなる。だから
//!    LLM の自動命名（1 ターン後に決まる）を待たず、最初のメッセージの先頭から決める。
//! 2. **作るのは最初の送信時。** cwd が要るのはセッション開始時で、開いただけでは要らない。
//! 3. **空のフォルダは残さない。** 相談だけで終わるチャットが大半なので、残すと書類が空フォルダで
//!    埋まる。再開時に無ければ同じパスで作り直す（パス文字列が同じなら `session/load` は通る）。

use crate::date::Date;
use std::path::{Path, PathBuf};

/// 成果物を置くサブフォルダ。この中だけが artifact（`docs/CHAT.md` §2.3）。
///
/// フォルダ直下には成果物ではないもの（会話の書き出し・作業用ファイル）も置くので、直下に
/// 成果物を置くと「どれが artifact か」を拡張子で推測することになる。表示の隔離で配信してよい
/// 範囲もここに絞る。
pub const ARTIFACTS_DIR: &str = "artifacts";

/// フォルダ名に使う最初のメッセージの文字数。
const HEAD_CHARS: usize = 24;

/// 書類フォルダ。Windows は Known Folder を引く（OneDrive へのリダイレクトがあるので
/// `%USERPROFILE%\Documents` を決め打ちしない）。
pub fn documents_dir() -> Option<PathBuf> {
    paths::documents_dir(windows_known_documents())
}

/// チャットのフォルダを並べる親。設定 `chat.directory` があればそれ（`~/` は展開する）、
/// 無ければ `<書類>/necoder`。
///
/// `<書類>/necoder` が既に git リポジトリなら `<書類>/necoder chats` に倒す — 自分の necoder の
/// クローンを書類に置いている人の作業ツリーへ、チャットのフォルダを撒かない。
pub fn chats_root(configured: Option<&str>) -> Option<PathBuf> {
    if let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(expand_home(configured));
    }
    let documents = documents_dir()?;
    let root = documents.join("necoder");
    if root.join(".git").exists() {
        return Some(documents.join("necoder chats"));
    }
    Some(root)
}

fn expand_home(path: &str) -> PathBuf {
    let rest = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"));
    match (rest, paths::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ if path == "~" => paths::home_dir().unwrap_or_else(|| PathBuf::from(path)),
        _ => PathBuf::from(path),
    }
}

/// フォルダ名 = `YYYY-MM-DD <最初のメッセージの先頭 24 文字>`。LLM を呼ばずに決まる。
///
/// どの OS でも作れる名前にする（書類フォルダは同期されて別の OS から見えることがある）:
/// `/ \ : * ? " < > |` と制御文字は空白にし、空白の連続は 1 つに畳み、末尾の空白とピリオドを
/// 落とす（Windows は末尾のそれを黙って削るので、同じ名前のつもりが別物になる）。
/// 先頭が必ず日付なので、Windows の予約名（`CON` など）とは衝突しない。
/// 使える文字が 1 つも残らなければ時刻 `HHMM` を使う。
pub fn folder_name(date: Date, hour: u32, minute: u32, first_message: &str) -> String {
    let first_line = first_message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let cleaned: String = first_line
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let head: String = collapsed.chars().take(HEAD_CHARS).collect();
    let head = head.trim_end_matches([' ', '.']).trim_start_matches('.');
    if head.is_empty() {
        format!("{date} {hour:02}{minute:02}")
    } else {
        format!("{date} {head}")
    }
}

/// `root/name` を作って返す。同名が既にあれば ` 2`・` 3` … を付ける。
///
/// 「無ければ作る」を `create_dir` の成否で判定する（存在確認してから作ると、同時に開いた
/// 2 つのチャットが同じフォルダを掴む）。
pub fn reserve(root: &Path, name: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    for attempt in 1..=999 {
        let candidate = if attempt == 1 {
            root.join(name)
        } else {
            root.join(format!("{name} {attempt}"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("同名のフォルダが多すぎます: {name}"),
    ))
}

/// 再開用: 前回のフォルダが（空で片付けられて）無ければ、**同じパスで**作り直す。
pub fn ensure(chat_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(chat_dir)
}

pub fn artifacts_dir(chat_dir: &Path) -> PathBuf {
    chat_dir.join(ARTIFACTS_DIR)
}

/// `path` がどれかのチャットの `artifacts/` の中なら、その `artifacts/` を返す
/// （`<root>/<チャットのフォルダ>/artifacts/…`）。表示の隔離で配信の根に使う。
pub fn artifacts_root_of(chats_root: &Path, path: &Path) -> Option<PathBuf> {
    let mut components = path.strip_prefix(chats_root).ok()?.components();
    let chat_folder = components.next()?;
    let artifacts = components.next()?;
    // `artifacts/` そのものではなく、その中のファイルであること。
    components.next()?;
    (artifacts.as_os_str() == ARTIFACTS_DIR)
        .then(|| chats_root.join(chat_folder).join(ARTIFACTS_DIR))
}

/// 中身の無いチャットのフォルダを消す。消したら `true`。
///
/// 「空」= `artifacts/` が空か無い、かつ他のファイルも無い。Finder が覗いただけで作る
/// `.DS_Store` は中身に数えない。**ファイルが 1 つでもあれば何もしない**（ユーザーの成果物を
/// 消す経路をここに作らない。`remove_dir` は空でないと失敗するので、判定を誤っても消えない）。
pub fn remove_if_empty(chat_dir: &Path) -> bool {
    let artifacts = artifacts_dir(chat_dir);
    if artifacts.is_dir() && !remove_dir_ignoring_finder_litter(&artifacts) {
        return false;
    }
    remove_dir_ignoring_finder_litter(chat_dir)
}

fn remove_dir_ignoring_finder_litter(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let names: Vec<_> = entries.flatten().map(|entry| entry.file_name()).collect();
    if names.iter().any(|name| name != ".DS_Store") {
        return false;
    }
    if !names.is_empty() && std::fs::remove_file(dir.join(".DS_Store")).is_err() {
        return false;
    }
    std::fs::remove_dir(dir).is_ok()
}

/// このチャットの成果物（`artifacts/` 直下のファイル）。新しい順。
pub fn list_artifacts(chat_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(artifacts_dir(chat_dir)) else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                    entry.path(),
                )
            })
        })
        .collect();
    files.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    files.into_iter().map(|(_, path)| path).collect()
}

/// necoder がその場でプレビューできる形式か（単体で完結する HTML と Markdown）。
pub fn is_previewable(path: &Path) -> bool {
    matches!(preview_kind(path), Some(_))
}

/// プレビューの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Html,
    Markdown,
}

pub fn preview_kind(path: &Path) -> Option<PreviewKind> {
    let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => Some(PreviewKind::Html),
        "md" | "markdown" => Some(PreviewKind::Markdown),
        _ => None,
    }
}

#[cfg(windows)]
fn windows_known_documents() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    // FOLDERID_Documents = {FDD39AD0-238F-46AF-ADB4-6C85480369C7}。定数は windows-sys の版で
    // 置き場が動くので、ABI が固定であることを利用してここで定義する（control_transport と同じ流儀）。
    const FOLDERID_DOCUMENTS: GUID = GUID {
        data1: 0xFDD3_9AD0,
        data2: 0x238F,
        data3: 0x46AF,
        data4: [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7],
    };

    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: 出力ポインタは有効。成功時に返る文字列は NUL 終端の UTF-16 で、呼び手が
    // `CoTaskMemFree` で解放する決まり（失敗時も null かもしれないポインタを解放してよい）。
    let path = unsafe {
        let status = SHGetKnownFolderPath(&FOLDERID_DOCUMENTS, 0, std::ptr::null_mut(), &mut raw);
        let path = if status >= 0 && !raw.is_null() {
            let mut length = 0;
            while *raw.add(length) != 0 {
                length += 1;
            }
            let wide = std::slice::from_raw_parts(raw, length);
            Some(PathBuf::from(std::ffi::OsString::from_wide(wide)))
        } else {
            None
        };
        CoTaskMemFree(raw.cast());
        path
    };
    path
}

#[cfg(not(windows))]
fn windows_known_documents() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Date = Date {
        year: 2026,
        month: 9,
        day: 18,
    };

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "necoder_chat_folder_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("一時ディレクトリ");
        dir
    }

    #[test]
    fn the_name_is_the_date_and_the_head_of_the_first_message() {
        assert_eq!(
            folder_name(
                DAY,
                14,
                32,
                "ポモドーロタイマー作って。作業 25 分・休憩 5 分で"
            ),
            "2026-09-18 ポモドーロタイマー作って。作業 25 分・休憩"
        );
        // 先頭の空行は飛ばし、2 行目以降は使わない。
        assert_eq!(
            folder_name(DAY, 14, 32, "\n\n  LP のコピー案  \n5 本ほしい"),
            "2026-09-18 LP のコピー案"
        );
    }

    #[test]
    fn characters_no_filesystem_accepts_are_dropped() {
        assert_eq!(
            folder_name(DAY, 9, 5, r#"a/b\c:d*e?f"g<h>i|j"#),
            "2026-09-18 a b c d e f g h i j"
        );
        // 制御文字とタブも空白扱いで、連続は 1 つに畳む。
        assert_eq!(folder_name(DAY, 9, 5, "a\t\u{7}  b"), "2026-09-18 a b");
    }

    /// Windows は末尾の空白とピリオドを黙って削る＝同じ名前のつもりが別物になるので、先に落とす。
    #[test]
    fn trailing_dots_and_spaces_are_trimmed() {
        assert_eq!(folder_name(DAY, 9, 5, "えっと... "), "2026-09-18 えっと");
        assert_eq!(folder_name(DAY, 9, 5, "..hidden"), "2026-09-18 hidden");
    }

    #[test]
    fn a_message_with_nothing_usable_falls_back_to_the_time() {
        assert_eq!(folder_name(DAY, 9, 5, "???"), "2026-09-18 0905");
        assert_eq!(folder_name(DAY, 23, 40, ""), "2026-09-18 2340");
    }

    #[test]
    fn reserving_the_same_name_twice_numbers_the_second() {
        let root = scratch("reserve");
        let first = reserve(&root, "2026-09-18 タイマー").expect("1 つ目");
        let second = reserve(&root, "2026-09-18 タイマー").expect("2 つ目");
        assert_eq!(first, root.join("2026-09-18 タイマー"));
        assert_eq!(second, root.join("2026-09-18 タイマー 2"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_empty_chat_folder_is_removed_but_one_with_files_is_kept() {
        let root = scratch("empty");
        let talk_only = reserve(&root, "talk").expect("作成");
        std::fs::create_dir_all(artifacts_dir(&talk_only)).expect("artifacts");
        // Finder が覗いた跡は中身に数えない。
        std::fs::write(talk_only.join(".DS_Store"), b"x").expect("litter");
        assert!(remove_if_empty(&talk_only));
        assert!(!talk_only.exists());

        let with_artifact = reserve(&root, "made").expect("作成");
        std::fs::create_dir_all(artifacts_dir(&with_artifact)).expect("artifacts");
        std::fs::write(artifacts_dir(&with_artifact).join("timer.html"), b"<p>").expect("成果物");
        assert!(!remove_if_empty(&with_artifact));
        assert!(artifacts_dir(&with_artifact).join("timer.html").exists());

        // 直下に成果物ではないファイル（会話の書き出し）があっても残す。
        let exported = reserve(&root, "exported").expect("作成");
        std::fs::write(exported.join("会話.md"), b"# log").expect("書き出し");
        assert!(!remove_if_empty(&exported));
        assert!(exported.join("会話.md").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn artifacts_are_listed_newest_first_and_skip_hidden_files() {
        let root = scratch("list");
        let chat = reserve(&root, "chat").expect("作成");
        let artifacts = artifacts_dir(&chat);
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        std::fs::write(artifacts.join("a.html"), b"a").expect("a");
        std::fs::write(artifacts.join(".DS_Store"), b"x").expect("litter");
        std::fs::create_dir_all(artifacts.join("sub")).expect("sub");
        let listed = list_artifacts(&chat);
        assert_eq!(listed, vec![artifacts.join("a.html")]);
        assert!(list_artifacts(&root.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_path_knows_which_chats_artifacts_it_belongs_to() {
        let root = Path::new("/docs/necoder");
        assert_eq!(
            artifacts_root_of(
                root,
                Path::new("/docs/necoder/2026-09-18 タイマー/artifacts/a.html")
            ),
            Some(PathBuf::from("/docs/necoder/2026-09-18 タイマー/artifacts"))
        );
        // フォルダ直下（会話の書き出し）・`artifacts/` そのもの・置き場の外は対象外。
        for path in [
            "/docs/necoder/2026-09-18 タイマー/会話.md",
            "/docs/necoder/2026-09-18 タイマー/artifacts",
            "/docs/necoder/artifacts/a.html",
            "/work/site/artifacts/a.html",
        ] {
            assert_eq!(artifacts_root_of(root, Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn only_standalone_html_and_markdown_preview() {
        assert_eq!(
            preview_kind(Path::new("/x/Timer.HTML")),
            Some(PreviewKind::Html)
        );
        assert_eq!(
            preview_kind(Path::new("notes.md")),
            Some(PreviewKind::Markdown)
        );
        assert_eq!(preview_kind(Path::new("app.tsx")), None);
        assert_eq!(preview_kind(Path::new("Makefile")), None);
    }

    #[test]
    fn a_configured_directory_wins_and_expands_the_home() {
        assert_eq!(
            chats_root(Some("/mnt/chats")),
            Some(PathBuf::from("/mnt/chats"))
        );
        if let Some(home) = paths::home_dir() {
            assert_eq!(chats_root(Some("~/Chats")), Some(home.join("Chats")));
        }
    }
}
