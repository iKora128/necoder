//! 注記の位置合わせ（R01）: 注記が指す行が、今の差分でも付けた時と同じ中身かを確かめる。GPUI 非依存。
//!
//! 注記は「パス + 側 + 行番号」に加えて、付けた時の該当行（抜粋）を持っている。行番号だけで置くと、
//! 上に行が挿入された・比較の基準を変えた・行の中身が書き換わった時に、**無関係な行へ黙って付く**。
//! そこで置く前に抜粋と今の中身を突き合わせ、
//!
//! - 合う（または、まだ中身を読めていなくて確かめられない）→ そのまま
//! - 合わないが、同じ中身の並びが 1 か所だけある → そこへ移す
//! - どちらでもない → 「位置が変わった注記」（ファイルの見出しの直後に、元の抜粋つきで出す）
//!
//! に分ける。

/// 注記の置き場所の判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotePosition {
    /// 付けた時の行のまま（中身が合った / まだ確かめられない）。
    Unchanged,
    /// 同じ中身の並びが別の場所に 1 か所だけあった（1 始まり・両端を含む）。
    Moved { start: u32, end: u32 },
    /// 付けた行の中身が変わり、同じ中身の並びも 1 か所に決まらない。
    Lost,
}

/// 注記の位置を判定する。
///
/// - `expected`: 注記の側の抜粋の行（新しい側なら追加行と文脈行、古い側なら削除行）
/// - `start..=end`: 注記が今指している行
/// - `lookup`: 今の中身の 1 行（読めていなければ `None`）
/// - `candidates`: 移し先を探す行（行番号の昇順。全文を読めていれば全文、まだなら diff の行だけ）
pub fn locate<'a>(
    expected: &[&str],
    start: u32,
    end: u32,
    lookup: impl Fn(u32) -> Option<&'a str>,
    candidates: &[(u32, &str)],
) -> NotePosition {
    if expected.is_empty() || end < start {
        return NotePosition::Unchanged;
    }
    // 抜粋が範囲の行数と揃っていれば全行を、揃っていなければ（範囲の途中に畳みがあった等）
    // 両端だけを突き合わせる。
    let span = (end - start + 1) as usize;
    let pairs: Vec<(u32, &str)> = if expected.len() == span {
        expected
            .iter()
            .enumerate()
            .map(|(offset, text)| (start + offset as u32, *text))
            .collect()
    } else {
        vec![(start, expected[0]), (end, expected[expected.len() - 1])]
    };
    let mismatch = pairs
        .iter()
        .any(|(line, text)| lookup(*line).is_some_and(|current| current != *text));
    if !mismatch {
        return NotePosition::Unchanged;
    }
    let mut found = None;
    for window in candidates.windows(expected.len()) {
        let consecutive = window.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1);
        let same = window
            .iter()
            .zip(expected)
            .all(|((_, current), text)| current == text);
        if consecutive && same {
            if found.is_some() {
                // 同じ中身が 2 か所以上ある = どちらか決められない。黙ってどちらかに付けない。
                return NotePosition::Lost;
            }
            found = Some((window[0].0, window[window.len() - 1].0));
        }
    }
    match found {
        Some((start, end)) => NotePosition::Moved { start, end },
        None => NotePosition::Lost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[&'static str]) -> Vec<(u32, &'static str)> {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| (index as u32 + 1, *line))
            .collect()
    }

    fn lookup<'a>(lines: &'a [(u32, &'a str)]) -> impl Fn(u32) -> Option<&'a str> {
        move |number| {
            lines
                .iter()
                .find(|(line, _)| *line == number)
                .map(|(_, text)| *text)
        }
    }

    #[test]
    fn matching_lines_stay_where_they_are() {
        let current = text(&["a", "b", "c", "d"]);
        assert_eq!(
            locate(&["b", "c"], 2, 3, lookup(&current), &current),
            NotePosition::Unchanged
        );
    }

    #[test]
    fn lines_inserted_above_move_the_note_down() {
        let current = text(&["new 1", "new 2", "a", "b", "c", "d"]);
        assert_eq!(
            locate(&["b", "c"], 2, 3, lookup(&current), &current),
            NotePosition::Moved { start: 4, end: 5 }
        );
    }

    #[test]
    fn rewritten_lines_lose_the_note_instead_of_attaching_it_elsewhere() {
        let current = text(&["a", "B!", "c", "d"]);
        assert_eq!(
            locate(&["b", "c"], 2, 3, lookup(&current), &current),
            NotePosition::Lost
        );
    }

    #[test]
    fn duplicated_content_is_not_guessed() {
        let current = text(&["x", "b", "c", "y", "b", "c"]);
        assert_eq!(
            locate(&["b", "c"], 1, 2, lookup(&current), &current),
            NotePosition::Lost,
            "同じ並びが 2 か所 = どちらにも付けない"
        );
    }

    #[test]
    fn unreadable_lines_and_empty_excerpts_are_left_alone() {
        let partial = vec![(40, "far away")];
        assert_eq!(
            locate(&["b"], 2, 2, lookup(&partial), &partial),
            NotePosition::Unchanged,
            "まだ中身を読めていない行は確かめられない（全文が届いたら確かめ直す）"
        );
        let current = text(&["a"]);
        assert_eq!(
            locate(&[], 1, 1, lookup(&current), &current),
            NotePosition::Unchanged
        );
    }

    #[test]
    fn moves_are_found_only_across_consecutive_lines() {
        // diff の行だけ（全文がまだ）: 行番号が飛んでいる所をまたいで一致させない。
        let hunks = vec![
            (1, "x"),
            (2, "y"),
            (3, "b"),
            (10, "c"),
            (20, "b"),
            (21, "c"),
        ];
        assert_eq!(
            locate(&["b", "c"], 1, 2, lookup(&hunks), &hunks),
            NotePosition::Moved { start: 20, end: 21 }
        );
    }
}
