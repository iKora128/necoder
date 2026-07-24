//! 開発用: hot_exit テーブルの中身を出力する（`cargo run -p storage --example dump_hot_exit -- <db>`）。
fn main() {
    let path = std::env::args().nth(1).expect("使い方: dump_hot_exit <db path>");
    let storage = storage::Storage::open(std::path::Path::new(&path)).expect("DB を開けない");
    let rows = storage.load_hot_exit_all().expect("読めない");
    println!("{} 行", rows.len());
    for (scope, path, content) in rows {
        let head: String = content.chars().take(60).collect();
        println!("[{scope}] {} :: {head}", path.display());
    }
}
