//! links — 文章の中の「パスらしきトークン」と URL を見つける（GPUI 非依存・純テキスト）。
//!
//! 消費側は 2 つ。**同じ規則で拾い、リンクにするかは各 view が決める**:
//! - `terminal_view`: cargo/rustc/grep の `path:line(:col)`（行番号つきだけをリンクにする）と URL
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
//!
//! ## スキーム無しのローカル URL（2026-09-26）
//!
//! 開発サーバは `localhost:3000` / `127.0.0.1:5173` / `[::1]:8080` のように**スキームを付けずに**
//! 待ち受け先を出すことが多い。これを `path:line` の形（`localhost` + 行 3000）と読むと、
//! パスらしくないので捨てられてリンクにならなかった。**ホストがローカルの形（`localhost` /
//! IPv4 / `[IPv6]`）で、ポートが付いているものだけ**を `http://` を補った URL として拾う。
//! ポートの無い `127.0.0.1` はバージョン番号と区別できないので拾わない。

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

/// `[` から始まる IPv6 リテラル（`[::1]` / `[fe80::1%en0]`）の byte 長。形が違えば None。
/// URL の終端文字に `]` が入っているので、ホスト部の括弧だけはここで先に読み切る。
fn ipv6_literal_length(text: &str) -> Option<usize> {
    let inner = text.strip_prefix('[')?;
    let close = inner.find(']')?;
    // `%` の後ろはゾーン ID（`en0` など英数字）。
    let (address, zone) = inner[..close]
        .split_once('%')
        .unwrap_or((&inner[..close], ""));
    let well_formed = address.contains(':')
        && address
            .chars()
            .all(|character| character.is_ascii_hexdigit() || matches!(character, ':' | '.'))
        && zone
            .chars()
            .all(|character| character.is_ascii_alphanumeric());
    well_formed.then_some(close + 2)
}

/// `http(s)://` から始まる URL の byte 長（末尾の句読点は後で落とす）。
fn url_length(text: &str) -> usize {
    let mut length = text.find("://").map_or(0, |offset| offset + "://".len());
    if let Some(literal) = ipv6_literal_length(&text[length..]) {
        length += literal;
    }
    length
        + text[length..]
            .char_indices()
            .find(|(_, character)| ends_url(*character))
            .map_or(text.len() - length, |(offset, _)| offset)
}

/// 10 進の IPv4（`127.0.0.1`）の byte 長。4 組で各 0〜255 でなければ None。
fn ipv4_length(text: &str) -> Option<usize> {
    let mut length = 0;
    for part in 0..4 {
        if part > 0 {
            if !text[length..].starts_with('.') {
                return None;
            }
            length += 1;
        }
        let digits = text[length..]
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 || digits > 3 || text[length..length + digits].parse::<u8>().is_err() {
            return None;
        }
        length += digits;
    }
    Some(length)
}

/// スキーム無しのローカル URL（`localhost:3000/path` / `127.0.0.1:5173` / `[::1]:8080`）の byte 長。
/// ポート必須（無いものはバージョン番号や単語と区別できない）。末尾の句読点は含めない。
fn local_url_length(text: &str) -> Option<usize> {
    let host = if text.starts_with("localhost") {
        "localhost".len()
    } else if text.starts_with('[') {
        ipv6_literal_length(text)?
    } else {
        ipv4_length(text)?
    };
    let rest = text[host..].strip_prefix(':')?;
    let port_digits = rest
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if port_digits == 0 || rest[..port_digits].parse::<u16>().is_err() {
        return None;
    }
    let mut length = host + 1 + port_digits;
    // ポートの直後が英数字なら別の語（`localhost:3000abc`）。
    let next = text[length..].chars().next();
    if next.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_') {
        return None;
    }
    if next == Some('/') {
        length += text[length..]
            .char_indices()
            .find(|(_, character)| ends_url(*character))
            .map_or(text.len() - length, |(offset, _)| offset);
    }
    Some(trim_trailing_punctuation(&text[..length]).len())
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

/// Windows のドライブ接頭辞（`C:\\…` / `C:/…`）の byte 長。無ければ 0。
/// 位置（`:line:column`）の解釈から外すために使う。
fn drive_prefix_length(token: &str) -> usize {
    let bytes = token.as_bytes();
    let looks_drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'\\' || bytes[2] == b'/');
    if looks_drive { 2 } else { 0 }
}

/// `path:line:column` を分解する（markdown リンクの dest にも使う）。
/// **ドライブ文字は先に切り離す** — `C:\Users\…\ROADMAP.md:157` を素で `rsplitn(3, ':')` に
/// 掛けると頭が `C` に食われて `path:line` の腕へ落ちず、行番号が消える（2026-09-18・Windows CI）。
fn split_position(token: &str) -> (&str, Option<u32>, Option<u32>) {
    let drive = drive_prefix_length(token);
    let (path, line, column) = split_position_after_drive(&token[drive..]);
    (&token[..drive + path.len()], line, column)
}

/// ドライブ接頭辞を除いた部分の分解。返す path は入力の接頭辞であること（上で繋ぎ直す）。
fn split_position_after_drive(token: &str) -> (&str, Option<u32>, Option<u32>) {
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
    if local_url_length(destination) == Some(destination.len()) {
        return Some(LinkTarget::Url(format!("http://{destination}")));
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
            let raw = &rest[..url_length(rest)];
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
        // スキーム無しのローカル URL は語の先頭でだけ見る（`mylocalhost:3000` の途中から拾わない）。
        let at_word_start = text[..index]
            .chars()
            .next_back()
            .is_none_or(|previous| !is_path_char(previous));
        if at_word_start {
            if let Some(length) = local_url_length(rest) {
                links.push(TextLink {
                    range: index..index + length,
                    target: LinkTarget::Url(format!("http://{}", &rest[..length])),
                });
                index += length;
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

    /// Windows の絶対パスでも行番号が拾えること（ドライブ文字を位置と取り違えない）。
    /// mac では再現しないので Windows CI でしか見つからなかった（2026-09-18）。
    #[test]
    fn a_drive_letter_is_not_mistaken_for_a_position() {
        assert_eq!(
            parse_target(r"C:\Users\daichi\docs\ROADMAP.md:157"),
            Some(LinkTarget::Path {
                path: r"C:\Users\daichi\docs\ROADMAP.md".into(),
                line: Some(157),
                column: None,
            })
        );
        assert_eq!(
            parse_target("C:/work/src/main.rs:10:5"),
            Some(LinkTarget::Path {
                path: "C:/work/src/main.rs".into(),
                line: Some(10),
                column: Some(5),
            })
        );
        // 位置が付いていないドライブ付きパスはそのまま。
        assert_eq!(
            parse_target(r"C:\work\Cargo.toml"),
            Some(LinkTarget::Path {
                path: r"C:\work\Cargo.toml".into(),
                line: None,
                column: None,
            })
        );
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

    /// (見えている文字列, 開く URL) の組。
    fn urls(text: &str) -> Vec<(String, String)> {
        find_links(text)
            .into_iter()
            .filter_map(|link| match link.target {
                LinkTarget::Url(url) => Some((text[link.range].to_string(), url)),
                LinkTarget::Path { .. } => None,
            })
            .collect()
    }

    fn url_pair(visible: &str, url: &str) -> Vec<(String, String)> {
        vec![(visible.to_string(), url.to_string())]
    }

    #[test]
    fn local_dev_server_addresses_become_urls() {
        // スキーム無しは `http://` を補う。見えている範囲（下線の位置）は元の文字列のまま。
        assert_eq!(
            urls("ready on localhost:3000"),
            url_pair("localhost:3000", "http://localhost:3000")
        );
        assert_eq!(
            urls("Listening on 127.0.0.1:5173."),
            url_pair("127.0.0.1:5173", "http://127.0.0.1:5173")
        );
        assert_eq!(
            urls("open [::1]:8080/app, then"),
            url_pair("[::1]:8080/app", "http://[::1]:8080/app")
        );
        assert_eq!(
            urls("  ➜  Local:   localhost:5173/"),
            url_pair("localhost:5173/", "http://localhost:5173/")
        );
        // path:line と読み違えて捨てない（`localhost` + 行 3000 のパスにしない）。
        assert!(paths("localhost:3000").is_empty());
        assert!(paths("127.0.0.1:5173").is_empty());
    }

    #[test]
    fn ipv6_hosts_are_not_cut_at_the_bracket() {
        // 以前は URL の終端文字 `]` で `http://[::1` に切れていた。
        assert_eq!(
            urls("http://[::1]:5173/ を開く"),
            url_pair("http://[::1]:5173/", "http://[::1]:5173/")
        );
        assert_eq!(
            urls("(http://[fe80::1%en0]:8000)"),
            url_pair("http://[fe80::1%en0]:8000", "http://[fe80::1%en0]:8000")
        );
        // ホスト以外の `]` は従来どおり終端（markdown の `[text](url)` の外側など）。
        assert_eq!(
            urls("[docs](https://necoder.com/x)"),
            url_pair("https://necoder.com/x", "https://necoder.com/x")
        );
    }

    #[test]
    fn local_addresses_need_a_port_and_a_word_boundary() {
        for noise in [
            "localhost",
            "127.0.0.1 と 10.0.0.1",
            "mylocalhost:3000",
            "localhost:99999",
            "localhost:3000abc",
            "999.0.0.1:80",
            "1.2.3:80",
        ] {
            assert!(urls(noise).is_empty(), "URL と誤認した: {noise}");
        }
        // 既存の path:line は壊れない。
        assert_eq!(paths("src/main.rs:10:5")[0].1, Some(10));
        assert_eq!(paths("localhost.rs:12")[0].0, "localhost.rs");
    }

    #[test]
    fn markdown_destinations_are_parsed() {
        assert_eq!(
            parse_target("localhost:3000"),
            Some(LinkTarget::Url("http://localhost:3000".into()))
        );
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
