//! scan_links — 実際のエージェント出力にリンク検出を当てて、拾い方を目で確かめる道具。
//!
//! 形だけで判定すると誤爆する（`0.5` / `v0.1.17` / `Apache-2.0`）ため、規則を触った時は
//! **本物の transcript** に当てて確かめる。necoder の実スレッドは
//! `~/Library/Application Support/necoder/necoder.db` の `turns` にあるので、
//! 例えばこう取り出して食わせる:
//!
//! ```sh
//! sqlite3 ~/Library/Application\ Support/necoder/necoder.db \
//!   "select content from turns where role='agent'" > /tmp/agent_turns.txt
//! cargo run -p ui --example scan_links -- /tmp/agent_turns.txt
//! ```
//!
//! 第 2 引数にプロジェクトのルートを渡すと、拾ったパスが**実在するか**まで数える
//! （agent_panel は実在解決できたものだけをリンクにするため、その歩留まりが見える）。

use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(corpus) = arguments.next() else {
        eprintln!("使い方: scan_links <テキストファイル> [プロジェクトルート]");
        return;
    };
    let root = arguments.next();
    let text = match std::fs::read_to_string(&corpus) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("{corpus} を読めない: {error}");
            return;
        }
    };
    let mut urls = 0usize;
    let mut paths: BTreeMap<String, usize> = BTreeMap::new();
    let mut with_line = 0usize;
    for link in ui::links::find_links(&text) {
        match link.target {
            ui::links::LinkTarget::Url(_) => urls += 1,
            ui::links::LinkTarget::Path { path, line, .. } => {
                if line.is_some() {
                    with_line += 1;
                }
                *paths.entry(path).or_default() += 1;
            }
        }
    }
    let total: usize = paths.values().sum();
    println!("URL {urls} 件 / パス {total} 件（うち行番号つき {with_line}）・種類 {}", paths.len());
    if let Some(root) = root {
        let root = Path::new(&root);
        let (mut hit, mut miss) = (0usize, 0usize);
        let mut missed: Vec<(usize, String)> = Vec::new();
        for (path, count) in &paths {
            let candidate = if Path::new(path).is_absolute() {
                Path::new(path).to_path_buf()
            } else {
                root.join(path)
            };
            if candidate.exists() {
                hit += count;
            } else {
                miss += count;
                missed.push((*count, path.clone()));
            }
        }
        println!("ルート直下で実在 {hit} 件 / 実在しない {miss} 件");
        missed.sort_by(|left, right| right.0.cmp(&left.0));
        println!("--- 実在しなかった上位 30（誤爆かどうかを目で見る）---");
        for (count, path) in missed.into_iter().take(30) {
            println!("{count:5}  {path}");
        }
    }
}
