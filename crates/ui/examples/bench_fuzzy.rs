//! ⌘P の in-process fuzzy の実測（M13・terminal-stack-2026 §4「巨大 repo で fzf+rg に負けないか」）。
//! zed クローン（数万ファイル級）で「1 キー入力毎の全件再スコア」の実コストを測る。
//! 予算: 1 refilter < 16ms（1 フレーム以内なら体感ゼロ）。超過で exit 1。
//!
//! 実行: `cargo run --release -p ui --example bench_fuzzy -- [dir=./zed]`

use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "./zed".to_string());
    let started = Instant::now();
    let files: Vec<String> = walk(&std::path::PathBuf::from(&dir));
    let listed_ms = started.elapsed().as_millis();
    println!("列挙: {} ファイル in {listed_ms}ms（{dir}）", files.len());

    // 実 repo と、5 万件へ合成拡大した「巨大 repo 相当」の両方で全件スコア
    // （Picker::refilter 相当 = 1 キー入力毎の負荷）。
    let mut synthetic: Vec<String> = Vec::with_capacity(50_000);
    let mut generation = 0usize;
    while synthetic.len() < 50_000 {
        for file in &files {
            synthetic.push(if generation == 0 {
                file.clone()
            } else {
                format!("vendor{generation}/{file}")
            });
            if synthetic.len() >= 50_000 {
                break;
            }
        }
        generation += 1;
        if files.is_empty() {
            break;
        }
    }
    let queries = ["ed", "editor", "wrksp", "term_view", "gpui/src/win"];
    let mut failed = false;
    for (label, set) in [("実測", &files), ("50k 合成", &synthetic)] {
        println!("{label}（{} 件）:", set.len());
        for query in queries {
            let started = Instant::now();
            let mut matched = 0usize;
            for file in set {
                if ui::fuzzy_score_for_bench(query, file).is_some() {
                    matched += 1;
                }
            }
            let micros = started.elapsed().as_micros();
            let mark = if micros > 16_000 { failed = true; "FAIL" } else { "ok" };
            println!("  {mark:4} query={query:14} {matched:>6} 件一致  {micros:>7} µs / refilter");
        }
    }
    if failed {
        eprintln!("⌘P の refilter が 1 フレーム（16ms）を超過 — 子プロセス化（rg/fzf 方式）の検討ライン");
        std::process::exit(1);
    }
}

/// 素朴な再帰列挙（.git と target を除くだけ。ignore 処理はベンチ対象外の I/O なので簡略）。
fn walk(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.display().to_string());
            }
        }
    }
    out
}
