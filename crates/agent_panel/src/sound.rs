//! 通知音の再生（完了 / 入力待ち）。
//!
//! 音は `assets/sounds/` の wav を**バイナリに埋め込む**。マスコット PNG と同じ理由で、
//! 実行時に `CARGO_MANIFEST_DIR` をパスとして読んではいけない（ビルド機のパスが焼き込まれ、
//! 配布物は他人の PC で鳴らない）。音源は `scripts/gen-chime.py` の合成＝権利は自前。
//!
//! macOS の `afplay` はファイルパスしか受け取らないので、埋め込んだ wav は**初回だけ**
//! 一時ディレクトリへ書き出してそのパスを使い回す。再生は短命スレッドで `status()` まで
//! 待つ（zombie を残さない）ので UI スレッドは即戻る。イベント駆動＝idle 予算に影響しない。
//!
//! Windows 対応（W フェーズ）では `rodio` へ差し替える予定（`docs/WINDOWS-PORT.md`）。
//! それまでは macOS 以外では**黙る**。

use std::path::PathBuf;
use std::sync::OnceLock;

/// 埋め込んだ通知音（`assets/sounds/`）。
macro_rules! sound_bytes {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/sounds/",
            $name
        ))
    };
}

/// 鳴らす場面。同梱 wav と設定キーがこれで決まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// ターンが終わった（一声の「にゃー」・`sound_done`）。
    Done,
    /// 承認・質問で止まっている（呼びかけの「にゃにゃっ」・`sound_waiting`）。
    Waiting,
}

impl Cue {
    fn bundled(self) -> &'static [u8] {
        match self {
            Cue::Done => sound_bytes!("done.wav"),
            Cue::Waiting => sound_bytes!("waiting.wav"),
        }
    }

    /// 一時ディレクトリへ書き出すときの名前。プロセス間で共有して構わない（中身は不変）。
    fn cache_name(self) -> &'static str {
        match self {
            Cue::Done => "necoder-done.wav",
            Cue::Waiting => "necoder-waiting.wav",
        }
    }
}

/// macOS のシステム音（設定値 `"system"`）。
const SYSTEM_SOUND: &str = "/System/Library/Sounds/Glass.aiff";

/// 設定値どおりに鳴らす。`choice` は `settings.sound_done` / `sound_waiting`:
/// `"nya"`（同梱）/ `"system"`（OS の音）/ `"off"` / 任意のファイルパス。
pub fn play(cue: Cue, choice: &str) {
    // 再生手段があるのは今のところ macOS だけ。無い環境では一時ファイルも作らない。
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(path) = resolve(cue, choice) {
        spawn_player(path);
    }
}

/// 設定値を再生するファイルへ解決する。鳴らさないときは `None`。
fn resolve(cue: Cue, choice: &str) -> Option<PathBuf> {
    match choice.trim() {
        "" | "off" => None,
        "nya" => cached_bundle(cue),
        "system" => Some(PathBuf::from(SYSTEM_SOUND)),
        // それ以外は「自分の音を指した」とみなす（wav / aiff / mp3 — afplay が読めれば何でも）。
        custom => Some(expand_home(custom)),
    }
}

/// `~/` をホームに開く。開けなければそのまま返す（相対パスは呼び出し側の cwd 依存で構わない）。
fn expand_home(raw: &str) -> PathBuf {
    match raw
        .strip_prefix("~/")
        .and_then(|rest| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest)))
    {
        Some(path) => path,
        None => PathBuf::from(raw),
    }
}

/// 同梱 wav を一時ディレクトリへ展開したパス（プロセス内で 1 回だけ）。
fn cached_bundle(cue: Cue) -> Option<PathBuf> {
    static DONE: OnceLock<Option<PathBuf>> = OnceLock::new();
    static WAITING: OnceLock<Option<PathBuf>> = OnceLock::new();
    let slot = match cue {
        Cue::Done => &DONE,
        Cue::Waiting => &WAITING,
    };
    slot.get_or_init(|| materialize(cue)).clone()
}

/// 埋め込んだバイト列をファイルにする。既に同じ長さで置いてあれば書き直さない
/// （毎起動の書き込みを避ける）。書きかけを `afplay` に掴ませないよう別名で書いてから rename する。
fn materialize(cue: Cue) -> Option<PathBuf> {
    let bytes = cue.bundled();
    let path = std::env::temp_dir().join(cue.cache_name());
    let already_there = std::fs::metadata(&path)
        .map(|meta| meta.len() == bytes.len() as u64)
        .unwrap_or(false);
    if already_there {
        return Some(path);
    }
    let staging =
        std::env::temp_dir().join(format!("{}.{}.tmp", cue.cache_name(), std::process::id()));
    match std::fs::write(&staging, bytes).and_then(|()| std::fs::rename(&staging, &path)) {
        Ok(()) => Some(path),
        Err(error) => {
            eprintln!("通知音の展開に失敗: {error}");
            None
        }
    }
}

/// 短命スレッドで鳴らす。子の終了まで待って刈り取る（zombie を残さない）。
fn spawn_player(path: PathBuf) {
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            use std::process::{Command, Stdio};
            let result = Command::new("/usr/bin/afplay")
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Err(error) = result {
                eprintln!("通知音の再生に失敗: {error}");
            }
        }
        #[cfg(not(target_os = "macos"))]
        drop(path);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_and_empty_do_not_resolve_to_a_file() {
        assert_eq!(resolve(Cue::Done, "off"), None);
        assert_eq!(resolve(Cue::Waiting, "  "), None);
    }

    #[test]
    fn bundled_sounds_are_written_out_whole() {
        for cue in [Cue::Done, Cue::Waiting] {
            let path = resolve(cue, "nya").expect("同梱音が展開される");
            let written = std::fs::read(&path).expect("展開した音が読める");
            assert_eq!(written, cue.bundled(), "{cue:?} の中身が埋め込みと一致する");
            // wav であること（先頭 4 バイトが RIFF）。壊れたファイルを afplay に渡さない。
            assert_eq!(&written[..4], b"RIFF");
        }
    }

    #[test]
    fn a_custom_choice_is_taken_as_a_path() {
        assert_eq!(
            resolve(Cue::Done, "/tmp/my-cat.wav"),
            Some(PathBuf::from("/tmp/my-cat.wav"))
        );
        assert_eq!(
            resolve(Cue::Done, "system"),
            Some(PathBuf::from(SYSTEM_SOUND))
        );
    }

    #[test]
    fn a_leading_tilde_opens_to_the_home_directory() {
        let Some(home) = std::env::var_os("HOME") else {
            return; // HOME が無い環境ではそのまま返る（下の assert が意味を持たない）
        };
        assert_eq!(
            expand_home("~/sounds/cat.wav"),
            PathBuf::from(home).join("sounds/cat.wav")
        );
        assert_eq!(expand_home("~cat.wav"), PathBuf::from("~cat.wav"));
    }
}
