//! search — バッファ内 / プロジェクト横断のテキスト検索。GPUI 非依存・テスト可能。
//!
//! ARCHITECTURE §2 / ROADMAP M6。literal・regex（大文字小文字トグル）を統一して regex にコンパイルする。
//! 位置は全て byte offset。プロジェクト横断は「ファイルパス列 + クエリ」で各ファイルを走査する
//! （ファイル収集は `project::Worktree::all_files` の責務。ここは検索に集中）。

use anyhow::{Context as _, Result};
use host::{Host, TextSearchSpec};
use regex::{Regex, RegexBuilder};
use std::ops::Range;
use std::path::PathBuf;

/// 1 件のヒット。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// 0 始まりの行番号。
    pub line: usize,
    /// 行内の byte 列。
    pub column: usize,
    /// テキスト全体での byte レンジ。
    pub byte_range: Range<usize>,
    /// ヒットした行のテキスト（改行なし。プレビュー用）。
    pub line_text: String,
}

/// 1 ファイル分のヒット。
#[derive(Debug, Clone)]
pub struct FileMatch {
    pub path: PathBuf,
    pub matches: Vec<Match>,
}

/// コンパイル済み検索クエリ。
pub struct SearchQuery {
    regex: Regex,
    is_empty: bool,
    pattern: String,
    is_regex: bool,
    case_sensitive: bool,
}

impl SearchQuery {
    /// クエリを作る。`is_regex=false` なら literal（エスケープ）。`case_sensitive=false` で大小無視。
    pub fn new(pattern: &str, is_regex: bool, case_sensitive: bool) -> Result<SearchQuery> {
        let body = if is_regex {
            pattern.to_string()
        } else {
            regex::escape(pattern)
        };
        let regex = RegexBuilder::new(&body)
            .case_insensitive(!case_sensitive)
            .build()
            .context("検索パターンが不正")?;
        Ok(SearchQuery {
            regex,
            is_empty: pattern.is_empty(),
            pattern: pattern.to_string(),
            is_regex,
            case_sensitive,
        })
    }

    /// テキスト（1 バッファ分）を検索する。改行を跨がない。
    pub fn search_text(&self, text: &str) -> Vec<Match> {
        if self.is_empty {
            return Vec::new();
        }
        let mut matches = Vec::new();
        let mut line_start = 0usize;
        for (line_index, raw_line) in text.split_inclusive('\n').enumerate() {
            let line_content = raw_line
                .strip_suffix('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
                .unwrap_or(raw_line);
            for found in self.regex.find_iter(line_content) {
                matches.push(Match {
                    line: line_index,
                    column: found.start(),
                    byte_range: (line_start + found.start())..(line_start + found.end()),
                    line_text: line_content.to_string(),
                });
            }
            line_start += raw_line.len();
        }
        matches
    }

    /// 複数ファイルを横断検索する。UTF-8 で読めないファイル（バイナリ等）はスキップ。
    pub fn search_files(&self, files: &[PathBuf]) -> Vec<FileMatch> {
        self.search_files_on(host::LocalHost::shared().as_ref(), files)
    }

    /// 指定ホスト上の複数ファイルを横断検索する。UTF-8 で読めないファイルはスキップ。
    pub fn search_files_on(&self, host: &dyn Host, files: &[PathBuf]) -> Vec<FileMatch> {
        if self.is_empty {
            return Vec::new();
        }
        let mut results = Vec::new();
        for path in files {
            let Ok(content) = host.read_file(path) else {
                continue;
            };
            let Ok(text) = String::from_utf8(content.bytes) else {
                continue;
            };
            let matches = self.search_text(&text);
            if !matches.is_empty() {
                results.push(FileMatch {
                    path: path.clone(),
                    matches,
                });
            }
        }
        results
    }

    /// project 全体を host 側で一括検索する。remote ではファイル内容を local へ全転送しない。
    pub fn search_project_on(
        &self,
        host: &dyn Host,
        root: &std::path::Path,
        file_limit: usize,
        max_matches: usize,
    ) -> Vec<FileMatch> {
        self.try_search_project_on(host, root, file_limit, max_matches)
            .unwrap_or_default()
    }

    /// [`Self::search_project_on`] の失敗を保持する版。UI は remote disconnect を空結果と混同しない。
    pub fn try_search_project_on(
        &self,
        host: &dyn Host,
        root: &std::path::Path,
        file_limit: usize,
        max_matches: usize,
    ) -> Result<Vec<FileMatch>> {
        if self.is_empty {
            return Ok(Vec::new());
        }
        let spec = TextSearchSpec {
            pattern: self.pattern.clone(),
            is_regex: self.is_regex,
            case_sensitive: self.case_sensitive,
            max_matches,
        };
        let hits = host.search_project(root, &spec, file_limit)?;
        let mut results = Vec::<FileMatch>::new();
        for hit in hits {
            let item = Match {
                line: hit.line,
                column: hit.column,
                byte_range: hit.byte_start..hit.byte_end,
                line_text: hit.line_text,
            };
            match results.last_mut() {
                Some(file) if file.path == hit.path => file.matches.push(item),
                _ => results.push(FileMatch {
                    path: hit.path,
                    matches: vec![item],
                }),
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "fn main() {\n    // TODO: あとで直す\n    let todo = 1;\n}\n";

    #[test]
    fn literal_search_finds_lines_and_ranges() {
        let query = SearchQuery::new("TODO", false, true).unwrap();
        let matches = query.search_text(SAMPLE);
        assert_eq!(matches.len(), 1); // 大文字 TODO のみ（case_sensitive）
        assert_eq!(matches[0].line, 1);
        // byte_range がテキスト全体を指す
        assert_eq!(&SAMPLE[matches[0].byte_range.clone()], "TODO");
    }

    #[test]
    fn case_insensitive_finds_both() {
        let query = SearchQuery::new("todo", false, false).unwrap();
        let matches = query.search_text(SAMPLE);
        assert_eq!(matches.len(), 2); // "TODO" と "todo"
        assert_eq!(matches[0].line, 1);
        assert_eq!(matches[1].line, 2);
    }

    #[test]
    fn regex_search() {
        let query = SearchQuery::new(r"let \w+", true, true).unwrap();
        let matches = query.search_text(SAMPLE);
        assert_eq!(matches.len(), 1);
        assert_eq!(&SAMPLE[matches[0].byte_range.clone()], "let todo");
    }

    #[test]
    fn empty_pattern_yields_nothing() {
        let query = SearchQuery::new("", false, false).unwrap();
        assert!(query.search_text(SAMPLE).is_empty());
    }

    #[test]
    fn invalid_regex_errors() {
        assert!(SearchQuery::new("(unclosed", true, true).is_err());
    }

    #[test]
    fn multibyte_line_offsets_are_correct() {
        // 日本語を含む行でも byte_range が正しく元テキストを指す
        let text = "α = 1\nTODO を書く\n";
        let query = SearchQuery::new("TODO", false, true).unwrap();
        let matches = query.search_text(text);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 1);
        assert_eq!(&text[matches[0].byte_range.clone()], "TODO");
    }

    #[test]
    fn search_files_across_temp_files() {
        let dir = std::env::temp_dir().join(format!("shirushi_search_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        std::fs::write(&a, "// TODO one\nok\n").unwrap();
        std::fs::write(&b, "no match here\n").unwrap();

        let query = SearchQuery::new("TODO", false, true).unwrap();
        let results = query.search_files(&[a.clone(), b.clone()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, a);
        assert_eq!(results[0].matches.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
