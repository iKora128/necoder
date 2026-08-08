//! 編集コアのマイクロベンチ（M13・性能予算の CI ガード）。
//! criterion を入れず Instant 直測（依存ゼロ・release で回す）。
//! 予算超過で exit 1 = CI が fail する。予算は「Zed 比 ~80%」目標の**編集コア分**の近似。
//!
//! 実行: `cargo run --release -p editor_core --example bench_editor`

use editor_core::{Buffer, Selection};
use std::time::Instant;

/// (名前, 実測 µs, 予算 µs)。
struct Row(&'static str, u128, u128);

fn main() {
    let mut rows = Vec::new();

    // 10k 行のバッファへの 1 文字挿入（キー入力 1 回分のコア処理）。
    let base: String = (0..10_000)
        .map(|i| format!("line {i} — こんにちは world\n"))
        .collect();
    let mut buffer = Buffer::from_str(&base);
    let middle = base.len() / 2;
    buffer.set_selections(vec![Selection::cursor(middle)]);
    let started = Instant::now();
    for _ in 0..100 {
        buffer.insert("x");
    }
    rows.push(Row(
        "insert×100 (10k行)",
        started.elapsed().as_micros() / 100,
        1_000,
    ));

    // undo/redo 100 回。
    let started = Instant::now();
    for _ in 0..100 {
        buffer.undo();
    }
    for _ in 0..100 {
        buffer.redo();
    }
    rows.push(Row(
        "undo+redo×100",
        started.elapsed().as_micros() / 200,
        1_000,
    ));

    // snapshot（描画毎に呼ばれる想定の複製コスト）。
    let started = Instant::now();
    for _ in 0..20 {
        let snapshot = buffer.snapshot();
        std::hint::black_box(snapshot.line_count());
    }
    rows.push(Row(
        "snapshot×20 (10k行)",
        started.elapsed().as_micros() / 20,
        5_000,
    ));

    // byte→point 変換 1000 回（gutter/wrap の座標計算）。
    let snapshot = buffer.snapshot();
    let started = Instant::now();
    for index in 0..1_000usize {
        let byte = (index * 37) % snapshot.len_bytes();
        std::hint::black_box(snapshot.byte_to_point(snapshot.clip_offset(byte)));
    }
    rows.push(Row(
        "byte_to_point×1000",
        started.elapsed().as_micros() / 1_000,
        500,
    ));

    let mut failed = false;
    println!("editor_core bench（予算 = CI ガード・µs/op）");
    for Row(name, actual, budget) in &rows {
        let mark = if actual > budget {
            failed = true;
            "FAIL"
        } else {
            "ok"
        };
        println!("  {mark:4} {name:24} {actual:>8} µs (予算 {budget} µs)");
    }
    if failed {
        eprintln!("性能予算を超過（CLAUDE.md: Zed 比 ~80% 目標のコア分）");
        std::process::exit(1);
    }
}
