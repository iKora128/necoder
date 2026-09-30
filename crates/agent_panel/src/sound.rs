//! 通知音の再生（完了 / 入力待ち）。
//!
//! 音は `assets/sounds/` の wav を**バイナリに埋め込む**。マスコット PNG と同じ理由で、
//! 実行時に `CARGO_MANIFEST_DIR` をパスとして読んではいけない（ビルド機のパスが焼き込まれ、
//! 配布物は他人の PC で鳴らない）。音源は `scripts/gen-chime.py` の合成＝権利は自前。
//!
//! 同梱の声は3つ（`nyaan` / `nya` / `mew`）× 場面2つ（[`Cue`]）。声は設定値で選ぶ
//! （`sound_done` / `sound_waiting`）ので、「完了は成猫・入力待ちは子猫」のような混ぜ方もできる。
//!
//! macOS の `afplay` はファイルパスしか受け取らないので、埋め込んだ wav は一時ディレクトリへ
//! 書き出してそのパスを渡す。**展開も再生も短命スレッドの中**でやる（UI スレッドは即戻る。
//! 子の終了まで待って刈り取るので zombie も残さない）。イベント駆動＝idle 予算に影響しない。
//!
//! Windows 対応（W フェーズ）では `rodio` へ差し替える予定（`docs/WINDOWS-PORT.md`）。
//! それまでは macOS 以外では**黙る**。

use std::path::PathBuf;

use settings_core::SOUND_VOICES;

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
    /// ターンが終わった（一声・`sound_done`）。
    Done,
    /// 承認・質問で止まっている（二声の呼びかけ・`sound_waiting`）。
    Waiting,
}

/// macOS のシステム音（設定値 `"system"`）。
const SYSTEM_SOUND: &str = "/System/Library/Sounds/Glass.aiff";

/// 設定値どおりに鳴らす。`choice` は `settings.sound_done` / `sound_waiting`:
/// 同梱の声（[`SOUND_VOICES`]）/ `"system"`（OS の音）/ `"off"` / 任意のファイルパス。
/// `volume_percent` は `settings.sound_volume`（0〜100・0 は鳴らさない）。
pub fn play(cue: Cue, choice: &str, volume_percent: u64) {
    // 再生手段があるのは今のところ macOS だけ。無い環境では一時ファイルも作らない。
    if !cfg!(target_os = "macos") {
        return;
    }
    let choice = choice.trim().to_string();
    if choice.is_empty() || choice == "off" {
        return;
    }
    let Some(volume) = afplay_volume(volume_percent) else {
        return;
    };
    // 展開（ファイル書き込み）まで含めてスレッドの中でやる＝UI スレッドを I/O で止めない。
    std::thread::spawn(move || {
        if let Some(path) = resolve(cue, &choice) {
            play_file(&path, &volume);
        }
    });
}

/// 設定の % を `afplay -v` の倍率へ（1 = 音源そのまま）。0 % は `None` = 鳴らさない。
/// 100 % を超える値は 100 % に抑える（音源より大きくはしない＝割れさせない）。
fn afplay_volume(percent: u64) -> Option<String> {
    match percent.min(100) {
        0 => None,
        100 => Some("1".to_string()),
        percent => Some(format!("{:.2}", percent as f64 / 100.0)),
    }
}

/// 設定値を再生するファイルへ解決する。鳴らさないときは `None`。
fn resolve(cue: Cue, choice: &str) -> Option<PathBuf> {
    match choice.trim() {
        "" | "off" => None,
        "system" => Some(PathBuf::from(SYSTEM_SOUND)),
        voice if SOUND_VOICES.contains(&voice) => materialize(voice, cue),
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

/// 声と場面に対応する埋め込みバイト列。同梱していない組み合わせは `None`。
fn bundled(voice: &str, cue: Cue) -> Option<&'static [u8]> {
    let bytes: &'static [u8] = match (voice, cue) {
        ("nya", Cue::Done) => sound_bytes!("nya-done.wav"),
        ("nya", Cue::Waiting) => sound_bytes!("nya-waiting.wav"),
        ("nyaan", Cue::Done) => sound_bytes!("nyaan-done.wav"),
        ("nyaan", Cue::Waiting) => sound_bytes!("nyaan-waiting.wav"),
        ("mew", Cue::Done) => sound_bytes!("mew-done.wav"),
        ("mew", Cue::Waiting) => sound_bytes!("mew-waiting.wav"),
        _ => return None,
    };
    Some(bytes)
}

/// 埋め込んだバイト列をファイルにする。既に同じ長さで置いてあれば書き直さない
/// （鳴らすたびの書き込みを避ける）。書きかけを `afplay` に掴ませないよう別名で書いてから rename する。
fn materialize(voice: &str, cue: Cue) -> Option<PathBuf> {
    let bytes = bundled(voice, cue)?;
    let scene = match cue {
        Cue::Done => "done",
        Cue::Waiting => "waiting",
    };
    // 中身は不変なのでプロセス間で共有して構わない。
    let path = std::env::temp_dir().join(format!("necoder-{voice}-{scene}.wav"));
    let already_there = std::fs::metadata(&path)
        .map(|meta| meta.len() == bytes.len() as u64)
        .unwrap_or(false);
    if already_there {
        return Some(path);
    }
    let staging = std::env::temp_dir().join(format!(
        "necoder-{voice}-{scene}.{}.tmp",
        std::process::id()
    ));
    match std::fs::write(&staging, bytes).and_then(|()| std::fs::rename(&staging, &path)) {
        Ok(()) => Some(path),
        Err(error) => {
            eprintln!("通知音の展開に失敗: {error}");
            None
        }
    }
}

/// 子の終了まで待って刈り取る（zombie を残さない）。呼び出し元が短命スレッドの中。
fn play_file(path: &PathBuf, volume: &str) {
    #[cfg(target_os = "macos")]
    {
        use std::process::{Command, Stdio};
        let result = Command::new("/usr/bin/afplay")
            .args(["-v", volume])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = result {
            eprintln!("通知音の再生に失敗: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _unused = (path, volume);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_maps_to_an_afplay_factor_and_zero_is_silent() {
        assert_eq!(afplay_volume(0), None);
        assert_eq!(afplay_volume(100).as_deref(), Some("1"));
        assert_eq!(afplay_volume(40).as_deref(), Some("0.40"));
        assert_eq!(
            afplay_volume(250).as_deref(),
            Some("1"),
            "音源より大きくしない"
        );
    }

    #[test]
    fn off_and_empty_do_not_resolve_to_a_file() {
        assert_eq!(resolve(Cue::Done, "off"), None);
        assert_eq!(resolve(Cue::Waiting, "  "), None);
    }

    #[test]
    fn every_bundled_voice_has_both_scenes() {
        for voice in SOUND_VOICES {
            for cue in [Cue::Done, Cue::Waiting] {
                let path = resolve(cue, voice).expect("同梱音が展開される");
                let written = std::fs::read(&path).expect("展開した音が読める");
                let bundled = bundled(voice, cue).expect("声と場面の組み合わせが同梱されている");
                assert_eq!(written, bundled, "{voice} の {cue:?} が埋め込みと一致する");
                // wav であること（先頭 4 バイトが RIFF）。壊れたファイルを afplay に渡さない。
                assert_eq!(&written[..4], b"RIFF");
            }
        }
    }

    #[test]
    fn each_voice_is_a_different_sound() {
        // 選べるのに耳で同じ、を防ぐ（生成スクリプトの取り違えは中身でしか気づけない）。
        let dones: Vec<_> = SOUND_VOICES
            .iter()
            .filter_map(|voice| bundled(voice, Cue::Done))
            .collect();
        assert_eq!(dones.len(), SOUND_VOICES.len());
        for (index, sound) in dones.iter().enumerate() {
            assert!(
                !dones[index + 1..].contains(sound),
                "{} の完了音が他の声と同じ中身",
                SOUND_VOICES[index]
            );
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
