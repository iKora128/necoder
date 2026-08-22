//! brand_migration — 旧ブランド Shirushi 時代のデータを necoder の置き場へ引き取る。
//!
//! 2026-08-22 の改名（DECISIONS §8）に伴う一度きりの後方互換。スレッド履歴・チェックポイント
//! blob・窓の復元状態・ユーザー keymap・シェル PATH キャッシュは全部
//! `~/Library/Application Support/Shirushi/` にあり、改名でそのまま参照できなくなる＝
//! ユーザーから見れば「全部消えた」に等しい。だから消さずに引っ越す。
//!
//! **`main()` の一番最初に呼ぶこと** — `logging` が `…/necoder/logs/` を先に作ってしまうと
//! 「新しい置き場はまだ無い」という素朴な判定が崩れる。後から動いても壊れない作りにはして
//! あるが（下記の「上書きしない」方針）、順序でも守る。
//!
//! 方針は **上書きしない引っ越し**: 新しい置き場に既に在るものには触れず、旧側にしか無いものだけ
//! 移す。二重起動や途中失敗で新旧が混ざっても、新しい側が常に勝つ。
//!
//! `~/.shirushi/gui.sock`（GUI 単一 writer の IPC・DECISIONS §8）は移さない — 起動ごとに
//! 作り直される揮発物で、古い socket を持ち込む方が害になる。

use std::path::{Path, PathBuf};

/// 旧ブランドのアプリ支援ディレクトリ（`~/Library/Application Support/Shirushi`）。
fn legacy_support_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/Shirushi"))
}

/// 現行のアプリ支援ディレクトリ（`~/Library/Application Support/necoder`）。
fn support_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/necoder"))
}

/// 旧ファイル名 → 新ファイル名。DB は `shirushi.db` で作られていたので `-wal` / `-shm` ごと連れて行く
/// （3 つの相対関係を保ったまま移せば SQLite 側は素直に開く）。それ以外は名前を変えない。
fn renamed(file_name: &str) -> String {
    match file_name.strip_prefix("shirushi.db") {
        Some(suffix) => format!("necoder.db{suffix}"),
        None => file_name.to_string(),
    }
}

/// 旧置き場が残っていれば necoder 側へ引き取る。新規インストールでは何もしない。
pub fn migrate_legacy_brand_data() {
    let (Some(legacy), Some(current)) = (legacy_support_dir(), support_dir()) else {
        return;
    };
    if !legacy.is_dir() {
        return;
    }
    if let Err(error) = std::fs::create_dir_all(&current) {
        eprintln!(
            "旧 Shirushi データの引き取りを中止（{} を作れない）: {error}",
            current.display()
        );
        return;
    }
    move_entries(&legacy, &current);
}

/// 旧ディレクトリの中身を新ディレクトリへ移し、全部移し切れたときだけ旧ディレクトリを畳む。
/// 新側に既に在るものには触らない（新しい側が常に勝つ）。
fn move_entries(legacy: &Path, current: &Path) {
    let entries = match std::fs::read_dir(legacy) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "旧 Shirushi データの引き取りを中止（{} を読めない）: {error}",
                legacy.display()
            );
            return;
        }
    };

    let mut moved = 0usize;
    let mut left_behind = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("旧 Shirushi データの一件を読み飛ばした: {error}");
                left_behind += 1;
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(renamed) else {
            eprintln!(
                "旧 Shirushi データに UTF-8 でない名前があったので残した: {}",
                entry.path().display()
            );
            left_behind += 1;
            continue;
        };
        let destination = current.join(&name);
        // 新しい側に在るものが常に勝つ。旧側は消さずに残して、後から手で確認できるようにする。
        if destination.exists() {
            left_behind += 1;
            continue;
        }
        if let Err(error) = std::fs::rename(entry.path(), &destination) {
            eprintln!(
                "旧 Shirushi データを移せなかった {} → {}: {error}",
                entry.path().display(),
                destination.display()
            );
            left_behind += 1;
            continue;
        }
        moved += 1;
    }

    if moved > 0 {
        eprintln!(
            "旧 Shirushi のデータ {moved} 件を {} へ引き取った（改名 2026-08-22・DECISIONS §8）",
            current.display()
        );
    }
    // 空になったときだけ旧ディレクトリを畳む。取りこぼしがあるうちは消さない（握り潰さない）。
    if left_behind == 0 {
        if let Err(error) = std::fs::remove_dir(legacy) {
            eprintln!(
                "旧 {} を畳めなかった（中身は移送済み）: {error}",
                legacy.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_sidecars_follow_the_rename() {
        assert_eq!(renamed("shirushi.db"), "necoder.db");
        assert_eq!(renamed("shirushi.db-wal"), "necoder.db-wal");
        assert_eq!(renamed("shirushi.db-shm"), "necoder.db-shm");
        assert_eq!(renamed("state.json"), "state.json");
        assert_eq!(renamed("blobs"), "blobs");
    }

    #[test]
    fn moves_only_what_the_new_side_lacks() {
        let base = std::env::temp_dir().join(format!("necoder-mig-{}", std::process::id()));
        let legacy = base.join("Shirushi");
        let current = base.join("necoder");
        std::fs::create_dir_all(&legacy).expect("旧ディレクトリを作れる");
        std::fs::create_dir_all(&current).expect("新ディレクトリを作れる");
        std::fs::write(legacy.join("shirushi.db"), "old-db").expect("旧 DB を置ける");
        std::fs::write(legacy.join("state.json"), "old-state").expect("旧 state を置ける");
        std::fs::write(current.join("state.json"), "new-state").expect("新 state を置ける");

        move_entries(&legacy, &current);

        // 旧側にしか無い DB は新名で引き取られる。
        assert_eq!(
            std::fs::read_to_string(current.join("necoder.db")).expect("新 DB が在る"),
            "old-db"
        );
        // 新側に既に在るものは上書きしない。旧側にも残す。
        assert_eq!(
            std::fs::read_to_string(current.join("state.json")).expect("新 state が在る"),
            "new-state"
        );
        assert!(legacy.join("state.json").exists(), "衝突した旧側は残る");

        std::fs::remove_dir_all(&base).expect("後片付けできる");
    }
}
