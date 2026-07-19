//! todos — Todo ボードの真実 `.shirushi/todos.md` の読み書き（M12-10）。
//!
//! settings と同じ「**ファイルが真実**・UI/CLI/AI は全部ただの書き手」方式:
//! - markdown のチェックボックス（`- [ ]` / `- [x]`）+ 日付見出し（`# 2026-07-17`）
//! - UI のチェッククリックも AI の完了報告も**同じファイル書き換え**に落ちる
//! - 反映は watch（M10）任せ＝どの書き手が変えても板が追従する
//!
//! パースは行指向で**行番号を保持**し、トグルは該当行の `[ ]`↔`[x]` だけを書き換える
//! （それ以外の内容・空行・コメントは 1 バイトも動かさない）。

use anyhow::{Context as _, Result};
use host::Host;
use std::path::{Path, PathBuf};

/// 板の 1 項目。`line` は 0-based 行番号（トグル書換の宛先）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub line: usize,
    pub text: String,
    pub done: bool,
    /// 直近の見出し（`# ...`）。日付運用を想定するが中身は自由。
    pub section: Option<String>,
}

/// 板ファイルの場所（プロジェクト直下 `.shirushi/todos.md`）。
pub fn todos_path(root: &Path) -> PathBuf {
    root.join(".shirushi").join("todos.md")
}

/// markdown テキストから項目を抜く（`- [ ]` / `- [x]`。インデント許容・大文字 X 許容）。
pub fn parse_todos(text: &str) -> Vec<TodoItem> {
    let mut items = Vec::new();
    let mut section: Option<String> = None;
    for (line, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if let Some(heading) = trimmed.strip_prefix('#') {
            section = Some(heading.trim_start_matches('#').trim().to_string());
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("- [") else {
            continue;
        };
        let Some((mark, body)) = rest.split_once(']') else {
            continue;
        };
        let done = match mark {
            " " | "" => false,
            "x" | "X" => true,
            _ => continue, // "- [?]" 等は項目扱いしない
        };
        items.push(TodoItem { line, text: body.trim().to_string(), done, section: section.clone() });
    }
    items
}

/// `line` 行のチェックを反転した全文と新しい done を返す。行が項目でなければ None。
pub fn toggle_todo_line(text: &str, line: usize) -> Option<(String, bool)> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    let target = lines.get(line)?;
    let (new_line, now_done) = if let Some(position) = target.find("- [ ]") {
        (format!("{}- [x]{}", &target[..position], &target[position + 5..]), true)
    } else if let Some(position) = target.find("- [x]").or_else(|| target.find("- [X]")) {
        (format!("{}- [ ]{}", &target[..position], &target[position + 5..]), false)
    } else {
        return None;
    };
    let owned = new_line;
    lines[line] = &owned;
    Some((lines.join("\n"), now_done))
}

/// 板を読む（無ければ空）。
pub fn read_todos_on(host: &dyn Host, root: &Path) -> Vec<TodoItem> {
    let path = todos_path(root);
    let Ok(content) = host.read_file(&path) else {
        return Vec::new();
    };
    parse_todos(&String::from_utf8_lossy(&content.bytes))
}

/// `line` 行のチェックを反転してファイルへ書き戻す。新しい done を返す。
pub fn toggle_todo_on(host: &dyn Host, root: &Path, line: usize) -> Result<bool> {
    let path = todos_path(root);
    let content = host.read_file(&path).context("todos.md を読めません")?;
    let text = String::from_utf8_lossy(&content.bytes).to_string();
    let (new_text, now_done) =
        toggle_todo_line(&text, line).context("その行はチェックボックスではありません")?;
    host.write_file(&path, new_text.as_bytes(), host::WriteCondition::Any)
        .context("todos.md を書けません")?;
    Ok(now_done)
}

/// 今日の見出しの下に項目を追記する（見出し・ファイルが無ければ作る）。
pub fn add_todos_on(host: &dyn Host, root: &Path, texts: &[String], today: &str) -> Result<()> {
    let path = todos_path(root);
    let current = host
        .read_file(&path)
        .map(|content| String::from_utf8_lossy(&content.bytes).to_string())
        .unwrap_or_default();
    let new_text = append_todos(&current, texts, today);
    host.write_file(&path, new_text.as_bytes(), host::WriteCondition::Any)
        .context("todos.md を書けません")?;
    Ok(())
}

/// 板に 1 項目を今日の見出し下へ追記する（ボードの ＋ / インライン入力から・Todo 追加）。
/// 日付はそのマシンの `date +%F`（remote ならそのマシンの今日＝作業場所の時間が真実）。
/// 空文字は追加しない。反映は watch 任せ（既存の書き手と同じ経路）。
pub fn add_todo_on(host: &dyn Host, root: &Path, text: &str) -> Result<()> {
    let text = text.trim();
    anyhow::ensure!(!text.is_empty(), "空の項目は追加しない");
    let output = host
        .run_command(&host::CommandSpec::new("date", root).args(["+%F"]))
        .context("日付の取得に失敗")?;
    let today = String::from_utf8_lossy(&output.stdout).trim().to_string();
    anyhow::ensure!(!today.is_empty(), "日付が空");
    add_todos_on(host, root, &[text.to_string()], &today)
}

/// 追記の pure 部分: `# today` 見出しがあればその節の末尾へ、無ければ**先頭**に節を作って足す
/// （日付は新しい順＝上に積む方が板として読みやすい）。
pub fn append_todos(current: &str, texts: &[String], today: &str) -> String {
    let entry_lines: Vec<String> =
        texts.iter().map(|text| format!("- [ ] {}", text.trim())).collect();
    let heading = format!("# {today}");
    let mut lines: Vec<String> = if current.is_empty() {
        Vec::new()
    } else {
        current.split('\n').map(str::to_string).collect()
    };
    // 既存の今日見出しを探す。
    let heading_index = lines.iter().position(|line| line.trim() == heading);
    match heading_index {
        Some(index) => {
            // 節の終わり（次の見出し or EOF）を探し、その直前の非空行の後ろへ挿す。
            let mut insert_at = lines.len();
            for (offset, line) in lines.iter().enumerate().skip(index + 1) {
                if line.trim_start().starts_with('#') {
                    insert_at = offset;
                    break;
                }
            }
            while insert_at > index + 1 && lines[insert_at - 1].trim().is_empty() {
                insert_at -= 1;
            }
            for (offset, entry) in entry_lines.into_iter().enumerate() {
                lines.insert(insert_at + offset, entry);
            }
        }
        None => {
            let mut block = vec![heading];
            block.extend(entry_lines);
            block.push(String::new());
            // 先頭に積む（既存の内容の前）。
            block.extend(lines);
            lines = block;
        }
    }
    let joined = lines.join("\n");
    if joined.ends_with('\n') { joined } else { format!("{joined}\n") }
}

/// ✨今日の計画を生成して板へ追記する（件数を返す）。日付はそのマシンの `date +%F`
/// （remote ならそのマシンの今日＝作業場所の時間が真実）。
pub fn daily_plan_on(host: &dyn Host, root: &Path) -> Result<usize> {
    let items = draft_daily_plan_on(host, root)?;
    let output = host
        .run_command(&host::CommandSpec::new("date", root).args(["+%F"]))
        .context("日付の取得に失敗")?;
    let today = String::from_utf8_lossy(&output.stdout).trim().to_string();
    anyhow::ensure!(!today.is_empty(), "日付が空");
    let count = items.len();
    add_todos_on(host, root, &items, &today)?;
    Ok(count)
}

/// ✨今日の計画: `claude -p` に「ROADMAP 断片 + git status + 板の未消化」を渡して
/// 今日やるべき 3〜5 項目の下書きをもらう（ai_commit_message と同型・M12-10）。
pub fn draft_daily_plan_on(host: &dyn Host, root: &Path) -> Result<Vec<String>> {
    // 引用符 / $ / バッククォートを含めない（sh -c の二重引用符に素で埋めるため）。
    // 「ファイルは開かず」= agentic なファイル探索をさせない（速度と出力純度の両方に効く）。
    let instruction = "stdin の素材だけから、今日やるべき開発タスクを3〜5個、\
        日本語の短い命令文（各60字以内・句点なし）で出力して。1行1タスク。\
        ファイルやリポジトリは開かない。前置き・注記・番号・チェックボックス記法・\
        見出し・説明は一切書かず、タスク本文の行だけを出力して。";
    // ROADMAP 冒頭 + git status + 板の未消化を素材として流す（無いものは無視される）。
    let script = format!(
        "{{ echo '--- git status ---'; git status --short | head -40; \
           echo '--- ROADMAP（先頭200行）---'; head -200 docs/ROADMAP.md 2>/dev/null; \
           echo '--- 板の未消化 ---'; grep -n -- '- \\[ \\]' .shirushi/todos.md 2>/dev/null | head -20; \
        }} | claude -p \"{instruction}\""
    );
    let output = host
        .run_command(&host::CommandSpec::new("sh", root).args(["-c", script.as_str()]))
        .context("今日の計画の実行に失敗（claude CLI 未導入？）")?;
    anyhow::ensure!(output.success(), "生成に失敗");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let items = parse_plan_lines(&stdout);
    anyhow::ensure!(!items.is_empty(), "生成結果が空");
    Ok(items)
}

/// `claude -p` の出力からタスク行だけ拾う（指示に反して混ざる前置き・注記への防御）。
/// 「。」で終わる説明文・※注記・見出し・60 字超は捨てる。
fn parse_plan_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|line| line.trim().trim_start_matches("- ").trim_start_matches("[ ]").trim())
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("--")
                && !line.starts_with('※')
                && !line.starts_with('#')
                && !line.ends_with('。')
                && line.chars().count() <= 60
        })
        .map(str::to_string)
        .take(6)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = "# 2026-07-17\n- [ ] レビュー対応\n- [x] 朝会\n\n# 2026-07-16\n- [ ] 積み残し\n";

    #[test]
    fn parse_reads_items_with_sections_and_lines() {
        let items = parse_todos(BOARD);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].text, "レビュー対応");
        assert_eq!(items[0].line, 1);
        assert!(!items[0].done);
        assert_eq!(items[0].section.as_deref(), Some("2026-07-17"));
        assert!(items[1].done);
        assert_eq!(items[2].section.as_deref(), Some("2026-07-16"));
    }

    #[test]
    fn toggle_flips_only_target_line() {
        // 未チェック → チェック。
        let (text, done) = toggle_todo_line(BOARD, 1).unwrap();
        assert!(done);
        assert!(text.contains("- [x] レビュー対応"));
        // 他の行は不変。
        assert!(text.contains("- [x] 朝会"));
        assert!(text.contains("- [ ] 積み残し"));
        // チェック → 未チェック（往復）。
        let (text2, done2) = toggle_todo_line(&text, 1).unwrap();
        assert!(!done2);
        assert_eq!(text2, BOARD);
        // 見出し行は None。
        assert!(toggle_todo_line(BOARD, 0).is_none());
    }

    #[test]
    fn plan_lines_survive_noisy_output() {
        // 実測した「前置き + タスク + ※注記」混在出力（2026-07-17）への防御。
        let noisy = "実ファイルを確認しました。素材が空だったためコードの実状から起こします。\n\
            \n\
            PR #42 のレビュー指摘に対応する\n\
            sample.rs を Cargo プロジェクト化して cargo test を通す\n\
            parse_port のベンチマーク計測の下準備を進める\n\
            \n\
            ※ 出所不明のエントリは含めていません。";
        let items = parse_plan_lines(noisy);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "PR #42 のレビュー指摘に対応する");
        assert!(items.iter().all(|line| !line.contains('※')));
    }

    #[test]
    fn append_creates_heading_or_extends_section() {
        // 見出しが無い → 先頭に節を作る。
        let empty = append_todos("", &["タスクA".into()], "2026-07-17");
        assert!(empty.starts_with("# 2026-07-17\n- [ ] タスクA\n"));
        // 既存の今日見出し → 節の末尾に足す（次の日付の前）。
        let appended = append_todos(BOARD, &["新タスク".into()], "2026-07-17");
        let plan_position = appended.find("- [ ] 新タスク").unwrap();
        let next_heading = appended.find("# 2026-07-16").unwrap();
        assert!(plan_position < next_heading);
        // 昨日の節は動かない。
        assert!(appended.contains("# 2026-07-16\n- [ ] 積み残し"));
    }
}
