//! ランダム操作ファズ — editor_core の不変条件を乱打で検証する（ベータ前の信頼性担保・M13）。
//!
//! 方式（Zed が rope/buffer で使う randomized test と同型・依存ゼロ）:
//! - 決定的 PRNG（SplitMix64）で編集列を生成 = 失敗はメッセージ中のシードで必ず単発再現できる
//! - 厳密系 [`strict_ops_match_reference_model`]: insert/delete/undo/redo を素朴な参照実装
//!   （String + 状態スタック）と毎手突合（テキスト・選択・undo/redo の可否まで）
//! - 全部系 [`all_ops_hold_global_invariants`]: 全公開操作を乱打し、大域不変条件
//!   （選択が常に範囲内 + char 境界・snapshot 整合・座標変換の往復・undo 全巻き戻し）を検証
//!
//! 再現・強化: `NECODER_FUZZ_SEED=<n>`（そのシードだけ 1 回）/ `NECODER_FUZZ_ITERS=<n>`（回数増）。

use editor_core::{Buffer, Selection};
use std::ops::Range;

// ── 決定的 PRNG（SplitMix64・依存ゼロ） ──

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// ランダムテキストの部品。\r は入れない（LineType::LF の行再構成不変条件を素直に保つため。
/// CRLF の individual な挙動は editor_core 本体の unit test が見る）。
const FRAGMENTS: &[&str] = &[
    "a", "b", "Z", "0", "_", " ", " ", "\n", "\n", "é", "日", "本語", "🦀", "𝄞", "()", "{", "\"",
    "'", "//", "。", "fn ", "let x",
];

fn random_text(rng: &mut Rng, max_pieces: usize) -> String {
    let count = rng.below(max_pieces + 1);
    (0..count)
        .map(|_| FRAGMENTS[rng.below(FRAGMENTS.len())])
        .collect()
}

/// model の char 境界一覧（0 と len を含む）。
fn boundaries(model: &str) -> Vec<usize> {
    model
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(model.len()))
        .collect()
}

/// 昇順・非重複（接触は可）の選択レンジを 1〜3 個作る。零幅も混ぜる。
fn random_ranges(rng: &mut Rng, model: &str) -> Vec<Range<usize>> {
    let bounds = boundaries(model);
    let count = 1 + rng.below(3);
    let mut picks: Vec<usize> = (0..count * 2)
        .map(|_| bounds[rng.below(bounds.len())])
        .collect();
    picks.sort_unstable();
    picks.dedup();
    // 隣り合わせでペアにする = 非重複が保証される。足りなければ零幅で埋める。
    let mut ranges = Vec::new();
    let mut iter = picks.into_iter();
    while let Some(start) = iter.next() {
        let end = iter.next().unwrap_or(start);
        ranges.push(start..end);
        if ranges.len() >= count {
            break;
        }
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
}

/// 昇順・相異なるキャレット位置（char 境界）を 1〜3 個作る。
fn random_cursors(rng: &mut Rng, model: &str) -> Vec<usize> {
    let bounds = boundaries(model);
    let count = 1 + rng.below(3);
    let mut picks: Vec<usize> = (0..count)
        .map(|_| bounds[rng.below(bounds.len())])
        .collect();
    picks.sort_unstable();
    picks.dedup();
    picks
}

/// 参照実装: 昇順・非重複レンジを同一テキストで置換する。
fn apply_ranges(model: &str, ranges: &[Range<usize>], new_text: &str) -> String {
    let mut out = String::with_capacity(model.len() + ranges.len() * new_text.len());
    let mut last = 0;
    for range in ranges {
        out.push_str(&model[last..range.start]);
        out.push_str(new_text);
        last = range.end;
    }
    out.push_str(&model[last..]);
    out
}

/// 参照実装: 置換後の各キャレット位置（editor_core の apply_forward と同じ規約 =
/// 各レンジの調整済み start + 挿入テキスト長）。
fn expected_cursors(ranges: &[Range<usize>], new_len: usize) -> Vec<Selection> {
    let mut delta: isize = 0;
    let mut cursors = Vec::with_capacity(ranges.len());
    for range in ranges {
        let start = (range.start as isize + delta) as usize;
        cursors.push(Selection::cursor(start + new_len));
        delta += new_len as isize - (range.end - range.start) as isize;
    }
    cursors
}

/// c の直前の char 開始位置（c==0 は 0）。
fn prev_char_start(model: &str, offset: usize) -> usize {
    model[..offset]
        .chars()
        .next_back()
        .map(|c| offset - c.len_utf8())
        .unwrap_or(0)
}

/// c の直後の char 終了位置（末尾は据え置き）。
fn next_char_end(model: &str, offset: usize) -> usize {
    model[offset..]
        .chars()
        .next()
        .map(|c| offset + c.len_utf8())
        .unwrap_or(offset)
}

fn iterations(default_count: usize) -> usize {
    std::env::var("NECODER_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_count)
}

fn seeds(default_count: usize, base: u64) -> Vec<u64> {
    if let Some(seed) = std::env::var("NECODER_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return vec![seed];
    }
    (0..iterations(default_count))
        .map(|index| base ^ (index as u64).wrapping_mul(0x9E37_79B9))
        .collect()
}

// ── 厳密系: insert / delete / undo / redo を参照実装と毎手突合 ──

#[test]
fn strict_ops_match_reference_model() {
    for seed in seeds(40, 0x5AFE_C0DE_2026_0001) {
        let mut rng = Rng(seed);
        let initial = random_text(&mut rng, 30);
        let mut buffer = Buffer::from_str(&initial);
        let mut model = initial.clone();

        // states[k] = k 個目の transaction 適用後のテキスト。cursor = 現在位置。
        // before/after[k] = transaction k の直前/直後の選択（undo/redo が戻すべきもの）。
        let mut states: Vec<String> = vec![initial.clone()];
        let mut before_selections: Vec<Vec<Selection>> = Vec::new();
        let mut after_selections: Vec<Vec<Selection>> = Vec::new();
        let mut cursor = 0usize;

        let steps = 150;
        for step in 0..steps {
            let context =
                || format!("seed={seed} step={step}（NECODER_FUZZ_SEED={seed} で単発再現）");
            match rng.below(6) {
                // insert（複数レンジ同時置換）
                0 | 1 => {
                    let ranges = random_ranges(&mut rng, &model);
                    let selections: Vec<Selection> = ranges
                        .iter()
                        .map(|r| Selection::new(r.start, r.end))
                        .collect();
                    buffer.set_selections(selections.clone());
                    let text = random_text(&mut rng, 6);
                    buffer.insert(&text);

                    model = apply_ranges(&model, &ranges, &text);
                    let after = expected_cursors(&ranges, text.len());
                    states.truncate(cursor + 1);
                    before_selections.truncate(cursor);
                    after_selections.truncate(cursor);
                    states.push(model.clone());
                    before_selections.push(selections);
                    after_selections.push(after.clone());
                    cursor += 1;

                    assert_eq!(
                        buffer.text(),
                        model,
                        "insert 後のテキスト不一致: {}",
                        context()
                    );
                    assert_eq!(
                        buffer.selections(),
                        &after[..],
                        "insert 後の選択不一致: {}",
                        context()
                    );
                }
                // delete_backward（キャレット群）
                2 => {
                    let cursors = random_cursors(&mut rng, &model);
                    buffer.set_selections(cursors.iter().map(|&c| Selection::cursor(c)).collect());
                    buffer.delete_backward();

                    let ranges: Vec<Range<usize>> = cursors
                        .iter()
                        .map(|&c| prev_char_start(&model, c)..c)
                        .collect();
                    model = apply_ranges(&model, &ranges, "");
                    let after = expected_cursors(&ranges, 0);
                    states.truncate(cursor + 1);
                    before_selections.truncate(cursor);
                    after_selections.truncate(cursor);
                    states.push(model.clone());
                    before_selections.push(cursors.iter().map(|&c| Selection::cursor(c)).collect());
                    after_selections.push(after.clone());
                    cursor += 1;

                    assert_eq!(
                        buffer.text(),
                        model,
                        "backspace 後のテキスト不一致: {}",
                        context()
                    );
                    assert_eq!(
                        buffer.selections(),
                        &after[..],
                        "backspace 後の選択不一致: {}",
                        context()
                    );
                }
                // delete_forward（キャレット群）
                3 => {
                    let cursors = random_cursors(&mut rng, &model);
                    buffer.set_selections(cursors.iter().map(|&c| Selection::cursor(c)).collect());
                    buffer.delete_forward();

                    let ranges: Vec<Range<usize>> = cursors
                        .iter()
                        .map(|&c| c..next_char_end(&model, c))
                        .collect();
                    model = apply_ranges(&model, &ranges, "");
                    let after = expected_cursors(&ranges, 0);
                    states.truncate(cursor + 1);
                    before_selections.truncate(cursor);
                    after_selections.truncate(cursor);
                    states.push(model.clone());
                    before_selections.push(cursors.iter().map(|&c| Selection::cursor(c)).collect());
                    after_selections.push(after.clone());
                    cursor += 1;

                    assert_eq!(
                        buffer.text(),
                        model,
                        "delete 後のテキスト不一致: {}",
                        context()
                    );
                    assert_eq!(
                        buffer.selections(),
                        &after[..],
                        "delete 後の選択不一致: {}",
                        context()
                    );
                }
                // undo
                4 => {
                    let result = buffer.undo();
                    if cursor > 0 {
                        assert!(result.is_some(), "undo が Some のはず: {}", context());
                        cursor -= 1;
                        model = states[cursor].clone();
                        assert_eq!(
                            buffer.text(),
                            model,
                            "undo 後のテキスト不一致: {}",
                            context()
                        );
                        assert_eq!(
                            buffer.selections(),
                            &before_selections[cursor][..],
                            "undo 後の選択不一致: {}",
                            context()
                        );
                    } else {
                        assert!(
                            result.is_none(),
                            "空履歴の undo は None のはず: {}",
                            context()
                        );
                    }
                }
                // redo
                _ => {
                    let result = buffer.redo();
                    if cursor < before_selections.len() {
                        assert!(result.is_some(), "redo が Some のはず: {}", context());
                        model = states[cursor + 1].clone();
                        assert_eq!(
                            buffer.text(),
                            model,
                            "redo 後のテキスト不一致: {}",
                            context()
                        );
                        assert_eq!(
                            buffer.selections(),
                            &after_selections[cursor][..],
                            "redo 後の選択不一致: {}",
                            context()
                        );
                        cursor += 1;
                    } else {
                        assert!(
                            result.is_none(),
                            "先端での redo は None のはず: {}",
                            context()
                        );
                    }
                }
            }
        }
    }
}

// ── 全部系: 全公開操作を乱打して大域不変条件を守り続けるか ──

/// 大域不変条件: 選択が常にバッファ範囲内 + char 境界・snapshot がバッファと一致・
/// 行再構成 = 全文・座標変換の往復・単語境界が範囲内。
fn assert_global_invariants(buffer: &Buffer, rng: &mut Rng, context: &dyn Fn() -> String) {
    let text = buffer.text();
    for (index, selection) in buffer.selections().iter().enumerate() {
        for (name, offset) in [("anchor", selection.anchor), ("head", selection.head)] {
            assert!(
                offset <= text.len(),
                "選択 {index} の {name}={offset} が len={} を超過: {}",
                text.len(),
                context()
            );
            assert!(
                text.is_char_boundary(offset),
                "選択 {index} の {name}={offset} が char 境界でない: {}",
                context()
            );
        }
    }

    let snapshot = buffer.snapshot();
    assert_eq!(
        snapshot.text(),
        text,
        "snapshot とバッファの不一致: {}",
        context()
    );
    assert_eq!(
        snapshot.len_bytes(),
        text.len(),
        "len_bytes 不一致: {}",
        context()
    );

    // 行の再構成 = 全文（\r を生成しない前提の LF 再結合）。
    let rebuilt: Vec<String> = (0..snapshot.line_count())
        .map(|row| snapshot.line_text(row))
        .collect();
    assert_eq!(
        rebuilt.join("\n"),
        text,
        "line_text の再構成が全文と不一致: {}",
        context()
    );

    // 座標変換の往復と char 境界系（ランダムな 3 点）。
    for _ in 0..3 {
        let raw = rng.below(text.len() + 8);
        let clipped = snapshot.clip_offset(raw);
        assert!(
            text.is_char_boundary(clipped),
            "clip_offset が境界でない: {}",
            context()
        );
        let point = snapshot.byte_to_point(clipped);
        assert_eq!(
            snapshot.point_to_byte(point),
            clipped,
            "byte↔point の往復が {clipped} で崩れた: {}",
            context()
        );
        let previous_word = snapshot.prev_word_boundary(clipped);
        let next_word = snapshot.next_word_boundary(clipped);
        assert!(
            previous_word <= clipped && text.is_char_boundary(previous_word),
            "prev_word_boundary 異常: {}",
            context()
        );
        assert!(
            next_word >= clipped && next_word <= text.len() && text.is_char_boundary(next_word),
            "next_word_boundary 異常: {}",
            context()
        );
    }
}

#[test]
fn all_ops_hold_global_invariants() {
    for seed in seeds(30, 0x5AFE_C0DE_2026_0002) {
        let mut rng = Rng(seed);
        let initial = random_text(&mut rng, 40);
        let mut buffer = Buffer::from_str(&initial);
        let steps = 120;

        for step in 0..steps {
            let op = if buffer.len_bytes() > 4000 {
                8
            } else {
                rng.below(18)
            };
            let context = || {
                format!("seed={seed} step={step} op={op}（NECODER_FUZZ_SEED={seed} で単発再現）")
            };
            match op {
                0 | 1 => {
                    let model = buffer.text();
                    let ranges = random_ranges(&mut rng, &model);
                    buffer.set_selections(
                        ranges
                            .iter()
                            .map(|r| Selection::new(r.start, r.end))
                            .collect(),
                    );
                    let text = random_text(&mut rng, 6);
                    buffer.insert(&text);
                }
                2 => {
                    buffer.delete_backward();
                }
                3 => {
                    buffer.delete_forward();
                }
                4 => {
                    buffer.delete_word_backward();
                }
                5 => {
                    buffer.move_lines(rng.chance(50));
                }
                6 => {
                    buffer.duplicate_lines(rng.chance(50));
                }
                7 => {
                    buffer.insert_newline_indented(4);
                }
                8 => {
                    buffer.delete_lines();
                }
                9 => {
                    buffer.toggle_comment("//");
                }
                10 => {
                    buffer.indent_lines(4);
                }
                11 => {
                    buffer.outdent_lines(4);
                }
                12 => {
                    buffer.select_next_occurrence();
                }
                13 => {
                    buffer.add_cursor_vertically(rng.chance(50));
                }
                14 => {
                    // わざと範囲外・非境界も混ぜる（clip の検証）。
                    let offset = rng.below(buffer.len_bytes() + 8);
                    buffer.add_cursor_at(offset);
                    if rng.chance(30) {
                        buffer.collapse_to_primary();
                    }
                }
                15 => {
                    // 生のランダム選択（非境界・範囲外込み）→ set_selections が clip する。
                    let len = buffer.len_bytes();
                    let count = 1 + rng.below(3);
                    let selections: Vec<Selection> = (0..count)
                        .map(|_| Selection::new(rng.below(len + 8), rng.below(len + 8)))
                        .collect();
                    buffer.set_selections(selections);
                }
                16 => {
                    // 生のランダム edit_batch（重複・非境界込み）→ 正規化が守る。
                    let len = buffer.len_bytes();
                    let count = 1 + rng.below(3);
                    let edits: Vec<(Range<usize>, String)> = (0..count)
                        .map(|_| {
                            let a = rng.below(len + 4);
                            let b = rng.below(len + 4);
                            (a.min(b)..a.max(b), random_text(&mut rng, 3))
                        })
                        .collect();
                    buffer.edit_batch(&edits);
                }
                _ => {
                    if rng.chance(50) {
                        buffer.undo();
                    } else {
                        buffer.redo();
                    }
                }
            }
            assert_global_invariants(&buffer, &mut rng, &context);
        }

        // 締め: undo 全巻き戻しで初期テキストへ、undo した回数だけ redo すれば最終テキストへ。
        // （ウォークが undo で終わっていると redo スタックに「未来」が残っているため、
        //   全 redo ではなく undo 回数ぶんだけ進める。）
        let final_text = buffer.text();
        let mut undo_count = 0;
        while buffer.undo().is_some() {
            undo_count += 1;
            assert!(
                undo_count <= steps + 4,
                "undo が {steps} 手を超えて止まらない: seed={seed}"
            );
        }
        assert_eq!(
            buffer.text(),
            initial,
            "undo 全巻き戻しが初期テキストへ戻らない: seed={seed}（NECODER_FUZZ_SEED={seed}）"
        );
        for _ in 0..undo_count {
            assert!(
                buffer.redo().is_some(),
                "undo した回数の redo が尽きた: seed={seed}"
            );
        }
        assert_eq!(
            buffer.text(),
            final_text,
            "redo で最終テキストへ戻らない: seed={seed}（NECODER_FUZZ_SEED={seed}）"
        );
    }
}
