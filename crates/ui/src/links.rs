//! links — 文章の中の「パスらしきトークン」と URL を見つける（GPUI 非依存・純テキスト）。
//!
//! 消費側は 2 つ。**同じ規則で拾い、リンクにするかは各 view が決める**:
//! - `terminal_view`: cargo/rustc/grep の `path:line(:col)`（行番号つきだけをリンクにする）
//! - `agent_panel`: エージェント返答の本文（プロジェクト索引で**実在解決できたものだけ**をリンクにする）
//!
//! ## なぜ「形」だけで決めないか（2026-09-16・実スレッド 2,734 ターンの実測）
//!
//! 実際のエージェント出力から `名前.拡張子` らしきトークンを全部拾うと 3,939 件あり、そのうち
//! **実在するパスは 421 件だけ**だった。残りは `0.5` / `1.5` / `v0.1.17` / `Apache-2.0` /
//! `crates.io` / `//127.0.0.1:8000` のようなバージョン番号・ドメイン・URL の断片で、
//! これを全部下線付きリンクにすると本文が読めなくなる。
//!
//! そこでこの層は**形の判定だけ**を持ち（`looks_like_path`）、実在確認は呼び出し側に委ねる。
//! 形の段階で落とすのは「後ろが数字だけの拡張子」（= バージョン番号）と「英字を1つも含まない」もの。

use std::ops::Range;

/// リンクの行き先。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LinkTarget {
    /// `http(s)://…`。OS 既定のブラウザで開く。
    Url(String),
    /// パス。`line`/`column` は**1 始まり**（表示のまま持ち、0 始まりへの変換は開く側）。
    Path {
        path: String,
        line: Option<u32>,
        column: Option<u32>,
    },
}

/// 本文中の 1 リンク。`range` は**テキストの byte 範囲**（`StyledText` の highlight と同じ単位）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TextLink {
    pub range: Range<usize>,
    pub target: LinkTarget,
}

/// パスを構成しうる文字（ASCII のみ = 1 byte。日本語に隣接した時はそこで切れる）。
fn is_path_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '/' | '~' | '-' | '+')
}

/// URL の終端になる文字（閉じ括弧・引用符・日本語の句読点や鉤括弧）。
fn ends_url(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '"' | '\'' | '`' | '<' | '>' | ')' | ']' | '}' | '、' | '。' | '，' | '．'
                | '「' | '」' | '『' | '』' | '（' | '）' | '　'
        )
}

/// 末尾の句読点はリンクに含めない（`settings.json.` の `.` や `…rs:42,` の `,`）。
fn trim_trailing_punctuation(token: &str) -> &str {
    token.trim_end_matches(['.', ',', ';', ':', '!', '?'])
}

/// ディレクトリを含まない裸のトークンを「ファイル名らしいか」で絞る。
///
/// 通す: `agent_panel.rs` / `Cargo.toml` / `.gitignore` / `docs/ROADMAP.md`（`/` があれば無条件）
/// 落とす: `0.5` / `1.5` / `v0.1.17` / `Apache-2.0`（拡張子が数字だけ）/ `README`（拡張子なし）
fn looks_like_path(token: &str) -> bool {
    if token.is_empty() || !token.chars().any(|character| character.is_ascii_alphabetic()) {
        return false;
    }
    if token.contains('/') {
        return true;
    }
    let Some((_stem, extension)) = token.rsplit_once('.') else {
        return false;
    };
    // 拡張子は「英字で始まり英数字だけ」。`1.5s` / `v0.1.17` / `Apache-2.0` のような
    // バージョン番号・秒数をここで落とす（代償: `.7z` のような数字始まりの拡張子は拾えない）。
    let starts_with_letter = extension
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic());
    if !starts_with_letter
        || !extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return false;
    }
    // ここまで来れば `名前.拡張子` の形。`.gitignore` / `.env` のようなドットファイルも通す。
    true
}

/// `path:line:column` を分解する（markdown リンクの dest にも使う）。
fn split_position(token: &str) -> (&str, Option<u32>, Option<u32>) {
    let mut parts = token.rsplitn(3, ':');
    let (last, middle, head) = (parts.next(), parts.next(), parts.next());
    match (head, middle, last) {
        // `path:line:column`
        (Some(path), Some(line), Some(column))
            if !line.is_empty()
                && line.chars().all(|character| character.is_ascii_digit())
                && !column.is_empty()
                && column.chars().all(|character| character.is_ascii_digit()) =>
        {
            (path, line.parse().ok(), column.parse().ok())
        }
        // `path:line`
        (None, Some(path), Some(line))
            if !line.is_empty() && line.chars().all(|character| character.is_ascii_digit()) =>
        {
            (path, line.parse().ok(), None)
        }
        _ => (token, None, None),
    }
}

/// markdown リンクの行き先（`[text](dest)` の dest）を解釈する。
/// `http(s)` は URL、それ以外は `path(:line(:col))` として扱う（`mailto:` 等は対象外 = None）。
pub fn parse_target(destination: &str) -> Option<LinkTarget> {
    let destination = destination.trim();
    if destination.is_empty() {
        return None;
    }
    if destination.starts_with("http://") || destination.starts_with("https://") {
        return Some(LinkTarget::Url(destination.to_string()));
    }
    let destination = destination.strip_prefix("file://").unwrap_or(destination);
    if destination.starts_with('#') || destination.contains("://") {
        return None; // 見出しアンカー・未対応スキーム
    }
    // `mailto:` 等のスキームを弾く。`path:42`（行番号）と Windows のドライブ文字 `C:/…` は通す。
    if let Some((scheme, rest)) = destination.split_once(':') {
        let scheme_like = scheme.len() >= 2
            && scheme
                .chars()
                .all(|character| character.is_ascii_alphabetic());
        let looks_positional = rest
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit());
        if scheme_like && !looks_positional && !rest.starts_with('/') {
            return None;
        }
    }
    let (path, line, column) = split_position(destination);
    if path.is_empty() {
        return None;
    }
    Some(LinkTarget::Path {
        path: path.to_string(),
        line,
        column,
    })
}

/// 本文から URL と `path(:line(:col))` を拾う。範囲は byte・画面順（先頭から）。
pub fn find_links(text: &str) -> Vec<TextLink> {
    let mut links = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        let Some(character) = rest.chars().next() else {
            break;
        };
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let length = rest
                .char_indices()
                .find(|(_, character)| ends_url(*character))
                .map(|(offset, _)| offset)
                .unwrap_or(rest.len());
            let raw = &rest[..length];
            let url = trim_trailing_punctuation(raw);
            // スキームだけ（`https://`）は拾わない。
            if url.len() > "https://".len() {
                links.push(TextLink {
                    range: index..index + url.len(),
                    target: LinkTarget::Url(url.to_string()),
                });
                index += raw.len().max(1);
                continue;
            }
        }
        if !is_path_char(character) {
            index += character.len_utf8();
            continue;
        }
        let start = index;
        let mut end = index;
        while let Some(next) = text[end..].chars().next() {
            if is_path_char(next) {
                end += next.len_utf8();
            } else {
                break;
            }
        }
        index = end;
        let mut token_end = end;
        let mut line = None;
        let mut column = None;
        // `:数字` が続けば行番号（さらに `:数字` があれば桁）。
        if text[token_end..].starts_with(':') {
            let digits_start = token_end + 1;
            let digits_end = digits_start
                + text[digits_start..]
                    .bytes()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
            if digits_end > digits_start {
                line = text[digits_start..digits_end].parse().ok();
                token_end = digits_end;
                if text[token_end..].starts_with(':') {
                    let column_start = token_end + 1;
                    let column_end = column_start
                        + text[column_start..]
                            .bytes()
                            .take_while(|byte| byte.is_ascii_digit())
                            .count();
                    if column_end > column_start {
                        column = text[column_start..column_end].parse().ok();
                        token_end = column_end;
                    }
                }
                index = token_end;
            }
        }
        let path = trim_trailing_punctuation(&text[start..end]);
        if path.is_empty() || !looks_like_path(path) {
            continue;
        }
        // 末尾の句読点を落としたぶんだけ範囲も縮める（行番号が付いていれば範囲は行番号まで）。
        let range_end = if line.is_some() {
            token_end
        } else {
            start + path.len()
        };
        links.push(TextLink {
            range: start..range_end,
            target: LinkTarget::Path {
                path: path.to_string(),
                line,
                column,
            },
        });
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
        find_links(text)
            .into_iter()
            .filter_map(|link| match link.target {
                LinkTarget::Path { path, line, column } => Some((path, line, column)),
                LinkTarget::Url(_) => None,
            })
            .collect()
    }

    #[test]
    fn cargo_and_grep_shapes_are_found_with_line_numbers() {
        // cargo/rustc（`--> path:line:col`）。範囲は `:col` まで含める（クリック域を広く）。
        let cargo = "  --> src/main.rs:10:5";
        let links = find_links(cargo);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            LinkTarget::Path {
                path: "src/main.rs".into(),
                line: Some(10),
                column: Some(5),
            }
        );
        assert_eq!(&cargo[links[0].range.clone()], "src/main.rs:10:5");

        assert_eq!(paths("lib.rs:42 に一致")[0].1, Some(42));
        assert_eq!(paths("/tmp/a.log:7")[0].0, "/tmp/a.log");
        assert_eq!(paths("~/notes/todo.md:3")[0].0, "~/notes/todo.md");
    }

    #[test]
    fn version_numbers_and_times_are_not_paths() {
        // 実スレッドの誤爆上位（2026-09-16 の実測）。形の段階で落ちること。
        for noise in [
            "12:30 に会議",
            "error:42",
            "0.5 倍",
            "1.5s かかった",
            "v0.1.17 をリリース",
            "Apache-2.0",
            "agent-client-protocol-1.3.0",
        ] {
            assert!(paths(noise).is_empty(), "誤爆した: {noise}");
        }
    }

    #[test]
    fn japanese_sentences_and_backticks_cut_tokens_cleanly() {
        assert_eq!(paths("`docs/JOURNAL.md` を見て")[0].0, "docs/JOURNAL.md");
        assert_eq!(paths("crates/agent_panel/src/agent_panel.rs を直した")[0].0,
            "crates/agent_panel/src/agent_panel.rs");
        // 英文の文末ピリオドはリンクに含めない。
        let link = &find_links("edit settings.json.")[0];
        assert_eq!(&"edit settings.json."[link.range.clone()], "settings.json");
        // ドットファイル。
        assert_eq!(paths("`.gitignore` に足した")[0].0, ".gitignore");
    }

    #[test]
    fn urls_are_found_and_do_not_leave_path_fragments() {
        let text = "詳細は https://github.com/zed-industries/zed/issues/55470 を参照。";
        let links = find_links(text);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target,
            LinkTarget::Url("https://github.com/zed-industries/zed/issues/55470".into())
        );
        // ポート付き localhost も URL 1 本として拾い、`//127.0.0.1:8000` の断片を作らない。
        let local = find_links("http://127.0.0.1:8000/mock を開く");
        assert_eq!(local.len(), 1);
        assert_eq!(
            local[0].target,
            LinkTarget::Url("http://127.0.0.1:8000/mock".into())
        );
        // 括弧で閉じた URL は括弧を含めない。
        let wrapped = find_links("(https://necoder.com)");
        assert_eq!(
            wrapped[0].target,
            LinkTarget::Url("https://necoder.com".into())
        );
    }

    #[test]
    fn markdown_destinations_are_parsed() {
        // 実測で最多だった Claude の引用形式。
        assert_eq!(
            parse_target("/Users/me/work/crates/acp_client/src/acp_client.rs:361"),
            Some(LinkTarget::Path {
                path: "/Users/me/work/crates/acp_client/src/acp_client.rs".into(),
                line: Some(361),
                column: None,
            })
        );
        assert_eq!(
            parse_target("https://example.com/x"),
            Some(LinkTarget::Url("https://example.com/x".into()))
        );
        assert_eq!(parse_target("#見出し"), None);
        assert_eq!(parse_target("mailto:a@example.com"), None);
        assert_eq!(
            parse_target("file:///tmp/a.log"),
            Some(LinkTarget::Path {
                path: "/tmp/a.log".into(),
                line: None,
                column: None,
            })
        );
    }
}
