//! 開発用: checkpoint を実ファイルへ書き戻す（`cargo run -p storage --example restore_checkpoint -- <db> <id>`）。
fn main() {
    let mut args = std::env::args().skip(1);
    let db = args.next().expect("使い方: restore_checkpoint <db> <checkpoint id>");
    let id: i64 = args.next().and_then(|value| value.parse().ok()).expect("id が不正");
    let storage = storage::Storage::open(std::path::Path::new(&db)).expect("DB を開けない");
    let blobs = storage::default_blobs_dir().expect("blob dir");
    let files = storage.load_checkpoint(id, &blobs).expect("checkpoint を読めない");
    for (path, content) in files {
        match content {
            Some(content) => {
                std::fs::write(&path, content).expect("書き戻しに失敗");
                println!("復元: {}", path.display());
            }
            None => {
                let _ = std::fs::remove_file(&path);
                println!("削除（checkpoint 時点で存在しなかった）: {}", path.display());
            }
        }
    }
}
