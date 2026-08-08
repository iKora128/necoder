//! lang — tree-sitter による構文ハイライト（M7）。GPUI 非依存・テスト可能。
//!
//! ROADMAP M7「tree-sitter ハイライト（Rust から。theme の syn-* トークンに接続）」。
//! 出力は**非重複・start 昇順**の [`HighlightSpan`] 列。色は持たず [`HighlightKind`] だけを返し、
//! 描画側（editor_view）が theme_core の syn-* にマップする（= この crate は theme を知らない）。

use anyhow::{Context as _, Result};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;
use std::path::Path;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

/// LSP クライアント（rust-analyzer）。M7。
pub mod lsp;

/// syn-* トークンに対応する種別（UI-SPEC §1.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Macro,
    Punctuation,
    /// Markdown の見出し本文。
    Heading,
    /// Markdown のリンク文字列・URL。
    Link,
    /// Markdown の太字。
    Strong,
    /// Markdown の斜体。
    Emphasis,
    /// Markdown のインラインコード／コードブロック。
    Code,
}

/// 構文解析に対応する言語。エディタ・Explorer・ACP が同じ判定表を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageId {
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Go,
    Json,
    Yaml,
    Toml,
    Html,
    Css,
    Markdown,
    Bash,
    C,
    Cpp,
}

impl LanguageId {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::JavaScript => "JavaScript",
            Self::TypeScript | Self::Tsx => "TypeScript",
            Self::Python => "Python",
            Self::Go => "Go",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Markdown => "Markdown",
            Self::Bash => "Shell",
            Self::C => "C",
            Self::Cpp => "C++",
        }
    }

    /// ACP のコードフェンス名や拡張子として使う正規形。
    pub const fn canonical_id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Python => "python",
            Self::Go => "go",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Html => "html",
            Self::Css => "css",
            Self::Markdown => "markdown",
            Self::Bash => "bash",
            Self::C => "c",
            Self::Cpp => "cpp",
        }
    }

    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.trim_start_matches('.').to_ascii_lowercase();
        Some(match extension.as_str() {
            "rs" => Self::Rust,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "py" | "pyw" => Self::Python,
            "go" => Self::Go,
            "json" | "jsonc" => Self::Json,
            "yml" | "yaml" => Self::Yaml,
            "toml" => Self::Toml,
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            "md" | "markdown" | "mdown" | "mkd" | "mkdn" => Self::Markdown,
            "sh" | "bash" | "zsh" => Self::Bash,
            "c" | "h" => Self::C,
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Self::Cpp,
            _ => return None,
        })
    }

    /// Markdown の info string を含む、人が書く言語名を正規化する。
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim_start_matches('.')
            .to_ascii_lowercase();
        Some(match name.as_str() {
            "rust" | "rs" => Self::Rust,
            "javascript" | "js" | "jsx" | "node" | "nodejs" => Self::JavaScript,
            "typescript" | "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "python" | "py" => Self::Python,
            "go" | "golang" => Self::Go,
            "json" | "jsonc" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            "markdown" | "md" | "mdown" | "mkd" | "mkdn" => Self::Markdown,
            "bash" | "sh" | "shell" | "zsh" | "console" => Self::Bash,
            "c" => Self::C,
            "cpp" | "c++" | "cc" | "cxx" => Self::Cpp,
            _ => return Self::from_extension(&name),
        })
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let file_name = path.file_name()?.to_str()?;
        match file_name.to_ascii_lowercase().as_str() {
            "cargo.lock" => return Some(Self::Toml),
            ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".profile" => {
                return Some(Self::Bash)
            }
            _ => {}
        }
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }
}

pub fn language_for_path(path: &Path) -> Option<LanguageId> {
    LanguageId::from_path(path)
}

pub fn language_for_name(name: &str) -> Option<LanguageId> {
    LanguageId::from_path(Path::new(name)).or_else(|| LanguageId::from_name(name))
}

/// byte レンジ + 種別。非重複・start 昇順で返る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// tree-sitter の capture 名。`configure` の順が [`HighlightEvent`] の index になる。
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "operator",
    "property",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "text.title",
    "text.literal",
    "text.uri",
    "text.reference",
    "text.emphasis",
    "text.strong",
];

fn kind_for_name(name: &str) -> Option<HighlightKind> {
    if let Some(kind) = match name {
        "text.title" => Some(HighlightKind::Heading),
        "text.literal" => Some(HighlightKind::Code),
        "text.uri" | "text.reference" => Some(HighlightKind::Link),
        "text.emphasis" => Some(HighlightKind::Emphasis),
        "text.strong" => Some(HighlightKind::Strong),
        _ => None,
    } {
        return Some(kind);
    }
    let base = name.split('.').next().unwrap_or(name);
    Some(match base {
        "keyword" => HighlightKind::Keyword,
        "function" if name.contains("macro") => HighlightKind::Macro,
        "function" => HighlightKind::Function,
        "type" | "constructor" => HighlightKind::Type,
        "string" | "escape" => HighlightKind::String,
        "constant" => HighlightKind::Number,
        "comment" => HighlightKind::Comment,
        "attribute" => HighlightKind::Macro,
        "operator" | "punctuation" => HighlightKind::Punctuation,
        _ => return None, // variable / property / label 等は既定色
    })
}

/// 1 言語分のハイライタ。クエリのコンパイルは 1 回（`new`）。
pub struct Highlighter {
    config: HighlightConfiguration,
}

impl Highlighter {
    /// 任意言語のハイライタを組む共通コンストラクタ。
    fn with_query(
        language: tree_sitter::Language,
        name: &'static str,
        highlights_query: &str,
        injections_query: &str,
        locals_query: &str,
    ) -> Result<Highlighter> {
        let mut config = HighlightConfiguration::new(
            language,
            name,
            highlights_query,
            injections_query,
            locals_query,
        )
        .with_context(|| format!("{name} ハイライトクエリのコンパイルに失敗"))?;
        config.configure(HIGHLIGHT_NAMES);
        Ok(Highlighter { config })
    }

    /// Rust 用ハイライタ。
    pub fn rust() -> Result<Highlighter> {
        Self::with_query(
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            "",
            "",
        )
    }

    /// 拡張子から対応ハイライタを選ぶ（M11 多言語）。クエリのコンパイル失敗は None（無ハイライトで開く）。
    pub fn for_extension(extension: &str) -> Option<Highlighter> {
        LanguageId::from_extension(extension).and_then(Self::for_language)
    }

    /// 共通言語 ID からハイライタを作る。Markdown は block/inline の 2 grammar が必要なため、
    /// 通常エディタの [`IncrementalHighlighter`] が担当する。
    pub fn for_language(language: LanguageId) -> Option<Highlighter> {
        let result = match language {
            LanguageId::Rust => Highlighter::rust(),
            LanguageId::JavaScript => Self::with_query(
                tree_sitter_javascript::LANGUAGE.into(),
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            // TS/TSX は JS のクエリ + TS 差分クエリを連結（tree-sitter-typescript の流儀）。
            LanguageId::TypeScript => Self::with_query(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
                &format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
                "",
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            LanguageId::Tsx => Self::with_query(
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                "tsx",
                &format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
                "",
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            LanguageId::Python => Self::with_query(
                tree_sitter_python::LANGUAGE.into(),
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            LanguageId::Go => Self::with_query(
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            LanguageId::Json => Self::with_query(
                tree_sitter_json::LANGUAGE.into(),
                "json",
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            LanguageId::Yaml => Self::with_query(
                tree_sitter_yaml::LANGUAGE.into(),
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            LanguageId::Toml => Self::with_query(
                tree_sitter_toml_ng::LANGUAGE.into(),
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            LanguageId::Html => Self::with_query(
                tree_sitter_html::LANGUAGE.into(),
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY,
                tree_sitter_html::INJECTIONS_QUERY,
                "",
            ),
            LanguageId::Css => Self::with_query(
                tree_sitter_css::LANGUAGE.into(),
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
                "",
            ),
            LanguageId::Bash => Self::with_query(
                tree_sitter_bash::LANGUAGE.into(),
                "bash",
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
                "",
            ),
            LanguageId::C => Self::with_query(
                tree_sitter_c::LANGUAGE.into(),
                "c",
                tree_sitter_c::HIGHLIGHT_QUERY,
                "",
                "",
            ),
            LanguageId::Cpp => Self::with_query(
                tree_sitter_cpp::LANGUAGE.into(),
                "cpp",
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                "",
                "",
            ),
            LanguageId::Markdown => return None,
        };
        match result {
            Ok(highlighter) => Some(highlighter),
            Err(error) => {
                eprintln!(
                    "{} ハイライタ初期化に失敗（無ハイライトで継続）: {error:#}",
                    language.canonical_id()
                );
                None
            }
        }
    }

    /// テキストをハイライトし、非重複・start 昇順の span 列を返す。
    pub fn highlight(&self, text: &str) -> Vec<HighlightSpan> {
        let mut highlighter = TsHighlighter::new();
        let events = match highlighter.highlight(&self.config, text.as_bytes(), None, |_| None) {
            Ok(events) => events,
            Err(_) => return Vec::new(),
        };

        let mut spans = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for event in events {
            let Ok(event) = event else { continue };
            match event {
                HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if start >= end {
                        continue;
                    }
                    if let Some(&index) = stack.last() {
                        if let Some(name) = HIGHLIGHT_NAMES.get(index) {
                            if let Some(kind) = kind_for_name(name) {
                                // 直前 span と隣接・同種なら連結
                                match spans.last_mut() {
                                    Some(HighlightSpan {
                                        range,
                                        kind: last_kind,
                                    }) if *last_kind == kind && range.end == start => {
                                        range.end = end;
                                    }
                                    _ => spans.push(HighlightSpan {
                                        range: start..end,
                                        kind,
                                    }),
                                }
                            }
                        }
                    }
                }
            }
        }
        spans
    }
}

/// `highlights`（非重複・start 昇順）のうち `[start, end)` と重なる部分を返す。
pub fn spans_in_range(highlights: &[HighlightSpan], start: usize, end: usize) -> &[HighlightSpan] {
    let first = highlights.partition_point(|span| span.range.end <= start);
    let last = highlights.partition_point(|span| span.range.start < end);
    &highlights[first..last]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_keywords_strings_comments() {
        let highlighter = Highlighter::rust().expect("Rust ハイライタが作れる");
        let source = "fn main() {\n    let x = \"hi\"; // c\n}\n";
        let spans = highlighter.highlight(source);
        assert!(!spans.is_empty(), "span が出る");

        // start 昇順・非重複
        for pair in spans.windows(2) {
            assert!(pair[0].range.start <= pair[1].range.start);
            assert!(pair[0].range.end <= pair[1].range.start, "非重複");
        }

        let kind_of = |needle: &str| {
            let at = source.find(needle).unwrap();
            spans
                .iter()
                .find(|span| span.range.start <= at && at < span.range.end)
                .map(|span| span.kind)
        };
        assert_eq!(kind_of("fn"), Some(HighlightKind::Keyword));
        assert_eq!(kind_of("let"), Some(HighlightKind::Keyword));
        assert_eq!(kind_of("\"hi\""), Some(HighlightKind::String));
        assert_eq!(kind_of("// c"), Some(HighlightKind::Comment));
    }

    #[test]
    fn for_extension_selects_rust_only() {
        assert!(Highlighter::for_extension("rs").is_some());
        assert!(Highlighter::for_extension("txt").is_none());
    }

    #[test]
    fn spans_in_range_filters() {
        let spans = vec![
            HighlightSpan {
                range: 0..3,
                kind: HighlightKind::Keyword,
            },
            HighlightSpan {
                range: 5..8,
                kind: HighlightKind::String,
            },
            HighlightSpan {
                range: 10..12,
                kind: HighlightKind::Comment,
            },
        ];
        let slice = spans_in_range(&spans, 4, 9);
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0].kind, HighlightKind::String);
    }

    #[test]
    fn language_registry_normalizes_paths_and_fence_names() {
        assert_eq!(LanguageId::from_name("rust"), Some(LanguageId::Rust));
        assert_eq!(LanguageId::from_name("c++"), Some(LanguageId::Cpp));
        assert_eq!(LanguageId::from_name("shell"), Some(LanguageId::Bash));
        assert_eq!(
            language_for_path(Path::new("docs/Guide.MD")),
            Some(LanguageId::Markdown)
        );
        assert_eq!(
            language_for_path(Path::new("config/settings.yaml")),
            Some(LanguageId::Yaml)
        );
        assert_eq!(
            language_for_path(Path::new("Cargo.lock")),
            Some(LanguageId::Toml)
        );
        assert_eq!(
            language_for_path(Path::new(".zshrc")),
            Some(LanguageId::Bash)
        );
    }
}

/// 単一 grammar の言語 → (言語, highlights クエリ)。Markdown は専用の block/inline 統合へ送る。
fn language_and_query(language_id: LanguageId) -> Option<(tree_sitter::Language, String)> {
    let pair: (tree_sitter::Language, String) = match language_id {
        LanguageId::Rust => (
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::JavaScript => (
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY.to_string(),
        ),
        LanguageId::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        LanguageId::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
        ),
        LanguageId::Python => (
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Go => (
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Json => (
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Yaml => (
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Toml => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Html => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Css => (
            tree_sitter_css::LANGUAGE.into(),
            tree_sitter_css::HIGHLIGHTS_QUERY.to_string(),
        ),
        LanguageId::Bash => (
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.to_string(),
        ),
        LanguageId::C => (
            tree_sitter_c::LANGUAGE.into(),
            tree_sitter_c::HIGHLIGHT_QUERY.to_string(),
        ),
        LanguageId::Cpp => (
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::HIGHLIGHT_QUERY.to_string(),
        ),
        LanguageId::Markdown => return None,
    };
    Some(pair)
}

struct StandardIncrementalHighlighter {
    parser: tree_sitter::Parser,
    query: tree_sitter::Query,
    capture_kinds: Vec<Option<HighlightKind>>,
    tree: Option<tree_sitter::Tree>,
}

impl StandardIncrementalHighlighter {
    fn new(language_id: LanguageId) -> Option<Self> {
        let (language, query_source) = language_and_query(language_id)?;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).ok()?;
        let query = match tree_sitter::Query::new(&language, &query_source) {
            Ok(query) => query,
            Err(error) => {
                eprintln!(
                    "{} のハイライトクエリが不正（無ハイライトで継続）: {error}",
                    language_id.canonical_id()
                );
                return None;
            }
        };
        let capture_kinds = query
            .capture_names()
            .iter()
            .map(|name| kind_for_name(name))
            .collect();
        Some(Self {
            parser,
            query,
            capture_kinds,
            tree: None,
        })
    }

    fn reparse_full(&mut self, text: &str) {
        self.tree = self.parser.parse(text, None);
    }

    fn apply_edit(&mut self, text: &str, start: usize, old: &str, new: &str) {
        let Some(tree) = self.tree.as_mut() else {
            self.reparse_full(text);
            return;
        };
        // start までのテキストは編集前後で同一 → 新テキストから start の Point を得る。
        let start_point = point_at(text, start);
        let edit = tree_sitter::InputEdit {
            start_byte: start,
            old_end_byte: start + old.len(),
            new_end_byte: start + new.len(),
            start_position: start_point,
            old_end_position: advance_point(start_point, old),
            new_end_position: advance_point(start_point, new),
        };
        tree.edit(&edit);
        self.tree = self.parser.parse(text, self.tree.as_ref());
    }

    fn spans(&self, text: &str, range: Range<usize>) -> Vec<HighlightSpan> {
        use tree_sitter::StreamingIterator as _;
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        let mut cursor = tree_sitter::QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut collected: Vec<HighlightSpan> = Vec::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), text.as_bytes());
        while let Some(found) = matches.next() {
            for capture in found.captures {
                let Some(kind) = self.capture_kinds[capture.index as usize] else {
                    continue;
                };
                let node_range = capture.node.byte_range();
                let start = node_range.start.max(range.start);
                let end = node_range.end.min(range.end);
                if start < end {
                    collected.push(HighlightSpan {
                        range: start..end,
                        kind,
                    });
                }
            }
        }
        normalize_spans(collected)
    }
}

/// Markdown grammar は block と inline に分かれているため、両方の Tree を同じ編集で更新する。
struct MarkdownTrees {
    block: tree_sitter::Tree,
    inline: Vec<tree_sitter::Tree>,
}

impl MarkdownTrees {
    fn edit(&mut self, edit: &tree_sitter::InputEdit) {
        self.block.edit(edit);
        for tree in &mut self.inline {
            tree.edit(edit);
        }
    }
}

struct MarkdownIncrementalHighlighter {
    parser: tree_sitter::Parser,
    block_language: tree_sitter::Language,
    inline_language: tree_sitter::Language,
    block_query: tree_sitter::Query,
    inline_query: tree_sitter::Query,
    block_capture_kinds: Vec<Option<HighlightKind>>,
    inline_capture_kinds: Vec<Option<HighlightKind>>,
    trees: Option<MarkdownTrees>,
    /// フェンス本文は各言語の Highlighter で一度だけ解析し、描画時は可視範囲を切るだけにする。
    fence_spans: Vec<HighlightSpan>,
    fence_highlighters: std::collections::HashMap<LanguageId, Highlighter>,
    fence_cache: std::collections::HashMap<FenceCacheKey, Vec<HighlightSpan>>,
    fence_cache_order: std::collections::VecDeque<FenceCacheKey>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FenceCacheKey {
    language: LanguageId,
    content_hash: u64,
    len: usize,
}

impl MarkdownIncrementalHighlighter {
    fn new() -> Option<Self> {
        let block_language: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
        let inline_language: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
        let block_query =
            tree_sitter::Query::new(&block_language, tree_sitter_md::HIGHLIGHT_QUERY_BLOCK).ok()?;
        let inline_query =
            tree_sitter::Query::new(&inline_language, tree_sitter_md::HIGHLIGHT_QUERY_INLINE)
                .ok()?;
        let block_capture_kinds = block_query
            .capture_names()
            .iter()
            .map(|name| kind_for_name(name))
            .collect();
        let inline_capture_kinds = inline_query
            .capture_names()
            .iter()
            .map(|name| kind_for_name(name))
            .collect();
        Some(Self {
            parser: tree_sitter::Parser::new(),
            block_language,
            inline_language,
            block_query,
            inline_query,
            block_capture_kinds,
            inline_capture_kinds,
            trees: None,
            fence_spans: Vec::new(),
            fence_highlighters: std::collections::HashMap::new(),
            fence_cache: std::collections::HashMap::new(),
            fence_cache_order: std::collections::VecDeque::new(),
        })
    }

    fn parse(&mut self, text: &str, old: Option<&MarkdownTrees>) -> Option<MarkdownTrees> {
        self.parser.set_included_ranges(&[]).ok()?;
        self.parser.set_language(&self.block_language).ok()?;
        let block = self.parser.parse(text, old.map(|trees| &trees.block))?;
        self.parser.set_language(&self.inline_language).ok()?;
        let mut inline = Vec::new();
        let mut cursor = block.walk();
        let mut index = 0usize;

        'outer: loop {
            let node = loop {
                let kind = cursor.node().kind();
                if kind == "inline" || kind == "pipe_table_cell" || !cursor.goto_first_child() {
                    while !cursor.goto_next_sibling() {
                        if !cursor.goto_parent() {
                            break 'outer;
                        }
                    }
                }
                let kind = cursor.node().kind();
                if kind == "inline" || kind == "pipe_table_cell" {
                    break cursor.node();
                }
            };

            let mut range = node.range();
            let mut ranges = Vec::new();
            if cursor.goto_first_child() {
                while cursor.goto_next_sibling() {
                    if !cursor.node().is_named() {
                        continue;
                    }
                    let child_range = cursor.node().range();
                    ranges.push(tree_sitter::Range {
                        start_byte: range.start_byte,
                        start_point: range.start_point,
                        end_byte: child_range.start_byte,
                        end_point: child_range.start_point,
                    });
                    range.start_byte = child_range.end_byte;
                    range.start_point = child_range.end_point;
                }
                cursor.goto_parent();
            }
            ranges.push(range);
            self.parser.set_included_ranges(&ranges).ok()?;
            let tree = self
                .parser
                .parse(text, old.and_then(|trees| trees.inline.get(index)))?;
            inline.push(tree);
            index += 1;
        }
        drop(cursor);
        Some(MarkdownTrees { block, inline })
    }

    fn reparse_full(&mut self, text: &str) {
        self.trees = self.parse(text, None);
        self.refresh_fence_spans(text);
    }

    fn apply_edit(&mut self, text: &str, start: usize, old: &str, new: &str) {
        let Some(mut previous) = self.trees.take() else {
            self.reparse_full(text);
            return;
        };
        let start_point = point_at(text, start);
        previous.edit(&tree_sitter::InputEdit {
            start_byte: start,
            old_end_byte: start + old.len(),
            new_end_byte: start + new.len(),
            start_position: start_point,
            old_end_position: advance_point(start_point, old),
            new_end_position: advance_point(start_point, new),
        });
        self.trees = self.parse(text, Some(&previous));
        self.refresh_fence_spans(text);
    }

    fn refresh_fence_spans(&mut self, text: &str) {
        self.fence_spans.clear();
        let Some(trees) = &self.trees else { return };
        collect_fenced_code(
            trees.block.root_node(),
            text,
            &mut self.fence_highlighters,
            &mut self.fence_cache,
            &mut self.fence_cache_order,
            &mut self.fence_spans,
        );
        self.fence_spans = normalize_spans(std::mem::take(&mut self.fence_spans));
    }

    fn spans(&self, text: &str, range: Range<usize>) -> Vec<HighlightSpan> {
        let Some(trees) = &self.trees else {
            return Vec::new();
        };
        let mut spans = Vec::new();
        collect_query_spans(
            &self.block_query,
            &self.block_capture_kinds,
            &trees.block,
            text,
            range.clone(),
            &mut spans,
        );
        let first_inline = trees
            .inline
            .partition_point(|tree| tree.root_node().byte_range().end <= range.start);
        for tree in trees.inline[first_inline..]
            .iter()
            .take_while(|tree| tree.root_node().byte_range().start < range.end)
        {
            collect_query_spans(
                &self.inline_query,
                &self.inline_capture_kinds,
                tree,
                text,
                range.clone(),
                &mut spans,
            );
        }
        for span in spans_in_range(&self.fence_spans, range.start, range.end) {
            let mut clipped = span.clone();
            clipped.range.start = clipped.range.start.max(range.start);
            clipped.range.end = clipped.range.end.min(range.end);
            if clipped.range.start < clipped.range.end {
                spans.push(clipped);
            }
        }
        normalize_spans(spans)
    }
}

fn collect_query_spans(
    query: &tree_sitter::Query,
    capture_kinds: &[Option<HighlightKind>],
    tree: &tree_sitter::Tree,
    text: &str,
    range: Range<usize>,
    spans: &mut Vec<HighlightSpan>,
) {
    use tree_sitter::StreamingIterator as _;
    let mut cursor = tree_sitter::QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(found) = matches.next() {
        for capture in found.captures {
            let Some(kind) = capture_kinds[capture.index as usize] else {
                continue;
            };
            let node_range = capture.node.byte_range();
            let start = node_range.start.max(range.start);
            let end = node_range.end.min(range.end);
            if start < end {
                spans.push(HighlightSpan {
                    range: start..end,
                    kind,
                });
            }
        }
    }
}

fn collect_fenced_code(
    node: tree_sitter::Node<'_>,
    text: &str,
    highlighters: &mut std::collections::HashMap<LanguageId, Highlighter>,
    cache: &mut std::collections::HashMap<FenceCacheKey, Vec<HighlightSpan>>,
    cache_order: &mut std::collections::VecDeque<FenceCacheKey>,
    spans: &mut Vec<HighlightSpan>,
) {
    if node.kind() == "fenced_code_block" {
        let content = descendant_of_kind(node, "code_fence_content");
        let language = descendant_of_kind(node, "language")
            .and_then(|node| node.utf8_text(text.as_bytes()).ok())
            .and_then(LanguageId::from_name);
        if let Some(content) = content {
            let content_range = content.byte_range();
            // 未指定・未対応でもコード本文として最低限区別する。
            spans.push(HighlightSpan {
                range: content_range.clone(),
                kind: HighlightKind::Code,
            });
            if let Some(language) = language.filter(|language| *language != LanguageId::Markdown) {
                if !highlighters.contains_key(&language) {
                    if let Some(highlighter) = Highlighter::for_language(language) {
                        highlighters.insert(language, highlighter);
                    }
                }
                if let Some(highlighter) = highlighters.get(&language) {
                    if let Ok(source) = content.utf8_text(text.as_bytes()) {
                        let mut hasher = DefaultHasher::new();
                        source.hash(&mut hasher);
                        let key = FenceCacheKey {
                            language,
                            content_hash: hasher.finish(),
                            len: source.len(),
                        };
                        let relative_spans = if let Some(cached) = cache.get(&key) {
                            cached.clone()
                        } else {
                            let highlighted = highlighter.highlight(source);
                            while cache_order.len() >= 256 {
                                if let Some(expired) = cache_order.pop_front() {
                                    cache.remove(&expired);
                                }
                            }
                            cache_order.push_back(key);
                            cache.insert(key, highlighted.clone());
                            highlighted
                        };
                        spans.extend(relative_spans.into_iter().map(|span| HighlightSpan {
                            range: content_range.start + span.range.start
                                ..content_range.start + span.range.end,
                            kind: span.kind,
                        }));
                    }
                }
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_fenced_code(child, text, highlighters, cache, cache_order, spans);
    }
}

fn descendant_of_kind<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
) -> Option<tree_sitter::Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .find_map(|child| descendant_of_kind(child, kind));
    found
}

fn highlight_priority(kind: HighlightKind) -> u8 {
    match kind {
        HighlightKind::Code => 0,
        HighlightKind::Heading => 1,
        HighlightKind::Emphasis => 2,
        HighlightKind::Strong | HighlightKind::Link => 3,
        HighlightKind::Keyword
        | HighlightKind::Function
        | HighlightKind::Type
        | HighlightKind::String
        | HighlightKind::Number
        | HighlightKind::Comment
        | HighlightKind::Macro => 4,
        HighlightKind::Punctuation => 5,
    }
}

/// 入れ子 capture を端点ごとの勝者へ分解し、非重複・昇順の不変条件に戻す。
fn normalize_spans(spans: Vec<HighlightSpan>) -> Vec<HighlightSpan> {
    let mut boundaries = Vec::with_capacity(spans.len() * 2);
    for span in &spans {
        if span.range.start < span.range.end {
            boundaries.push(span.range.start);
            boundaries.push(span.range.end);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut result: Vec<HighlightSpan> = Vec::new();
    for pair in boundaries.windows(2) {
        let start = pair[0];
        let end = pair[1];
        let Some(winner) = spans
            .iter()
            .filter(|span| span.range.start <= start && span.range.end >= end)
            .max_by_key(|span| (highlight_priority(span.kind), usize::MAX - span.range.len()))
        else {
            continue;
        };
        match result.last_mut() {
            Some(last) if last.kind == winner.kind && last.range.end == start => {
                last.range.end = end;
            }
            _ => result.push(HighlightSpan {
                range: start..end,
                kind: winner.kind,
            }),
        }
    }
    result
}

enum IncrementalEngine {
    Standard(StandardIncrementalHighlighter),
    Markdown(MarkdownIncrementalHighlighter),
}

/// 増分ハイライタ（M11-8）。Tree を保持し、編集は `Tree::edit` + 差分パース。
/// span 抽出は**可視 byte 範囲だけ** QueryCursor で走る。Markdown のコードフェンスも同じ
/// [`LanguageId`] レジストリで内側の言語へ委譲する。
pub struct IncrementalHighlighter {
    language: LanguageId,
    engine: IncrementalEngine,
}

impl IncrementalHighlighter {
    pub fn for_extension(extension: &str) -> Option<Self> {
        LanguageId::from_extension(extension).and_then(Self::for_language)
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        LanguageId::from_path(path).and_then(Self::for_language)
    }

    pub fn for_language(language: LanguageId) -> Option<Self> {
        let engine = if language == LanguageId::Markdown {
            IncrementalEngine::Markdown(MarkdownIncrementalHighlighter::new()?)
        } else {
            IncrementalEngine::Standard(StandardIncrementalHighlighter::new(language)?)
        };
        Some(Self { language, engine })
    }

    pub const fn language(&self) -> LanguageId {
        self.language
    }

    pub fn reparse_full(&mut self, text: &str) {
        match &mut self.engine {
            IncrementalEngine::Standard(highlighter) => highlighter.reparse_full(text),
            IncrementalEngine::Markdown(highlighter) => highlighter.reparse_full(text),
        }
    }

    pub fn apply_edit(&mut self, text: &str, start: usize, old: &str, new: &str) {
        match &mut self.engine {
            IncrementalEngine::Standard(highlighter) => {
                highlighter.apply_edit(text, start, old, new)
            }
            IncrementalEngine::Markdown(highlighter) => {
                highlighter.apply_edit(text, start, old, new)
            }
        }
    }

    pub fn spans(&self, text: &str, range: Range<usize>) -> Vec<HighlightSpan> {
        match &self.engine {
            IncrementalEngine::Standard(highlighter) => highlighter.spans(text, range),
            IncrementalEngine::Markdown(highlighter) => highlighter.spans(text, range),
        }
    }
}

/// テキスト内 byte offset の (row, column) Point。
fn point_at(text: &str, offset: usize) -> tree_sitter::Point {
    let prefix = &text[..offset.min(text.len())];
    let row = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let column = prefix
        .rfind('\n')
        .map(|last| offset - last - 1)
        .unwrap_or(offset);
    tree_sitter::Point::new(row, column)
}

/// Point から文字列 `s` の分だけ進めた Point。
fn advance_point(start: tree_sitter::Point, s: &str) -> tree_sitter::Point {
    let newlines = s.bytes().filter(|byte| *byte == b'\n').count();
    if newlines == 0 {
        tree_sitter::Point::new(start.row, start.column + s.len())
    } else {
        let tail = s.rfind('\n').map(|last| s.len() - last - 1).unwrap_or(0);
        tree_sitter::Point::new(start.row + newlines, tail)
    }
}

/// アウトラインの 1 項目（⌘⇧O・M11）。tree-sitter クエリ駆動 = LSP 不要で全対応言語に効く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub name: String,
    /// 種別の短いラベル（fn/struct/enum/trait/impl/mod/const）。
    pub kind: &'static str,
    /// 0 始まりの行番号。
    pub row: usize,
}

/// テキストのアウトラインを返す（対応言語のみ・現状 Rust）。定義順。
pub fn outline(text: &str, extension: &str) -> Vec<OutlineItem> {
    match extension {
        "rs" => outline_rust(text),
        _ => Vec::new(),
    }
}

fn outline_rust(text: &str) -> Vec<OutlineItem> {
    use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator as _};
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    // name capture の順に (クエリ, 種別) を並べる。impl は型名を名前として使う。
    const QUERY: &str = r#"
        (function_item name: (identifier) @fn)
        (struct_item name: (type_identifier) @struct)
        (enum_item name: (type_identifier) @enum)
        (trait_item name: (type_identifier) @trait)
        (impl_item type: (type_identifier) @impl)
        (mod_item name: (identifier) @mod)
        (const_item name: (identifier) @const)
        (static_item name: (identifier) @const)
    "#;
    let Ok(query) = Query::new(&language, QUERY) else {
        return Vec::new();
    };
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut items = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    while let Some(found) = matches.next() {
        for capture in found.captures {
            let node = capture.node;
            let Ok(name) = node.utf8_text(text.as_bytes()) else {
                continue;
            };
            let kind: &'static str = match names[capture.index as usize] {
                "fn" => "fn",
                "struct" => "struct",
                "enum" => "enum",
                "trait" => "trait",
                "impl" => "impl",
                "mod" => "mod",
                "const" => "const",
                _ => "•",
            };
            items.push(OutlineItem {
                name: name.to_string(),
                kind,
                row: node.start_position().row,
            });
        }
    }
    items.sort_by_key(|item| item.row);
    items
}

/// 拡張子 → 行コメント接頭辞（⌘/ トグル用の最小表。M11 の言語表に統合予定）。
/// ブロックコメントのみの言語（html/css/md）は v1 非対応 = None。
pub fn comment_prefix(extension: &str) -> Option<&'static str> {
    match extension {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "c" | "h" | "cpp" | "cc" | "cxx"
        | "hpp" | "go" | "java" | "swift" | "kt" | "json" | "jsonc" => Some("//"),
        "py" | "sh" | "zsh" | "bash" | "rb" | "yml" | "yaml" | "toml" => Some("#"),
        "sql" | "lua" => Some("--"),
        _ => None,
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::*;

    #[test]
    fn incremental_matches_full_reparse_after_edits() {
        let mut incremental = IncrementalHighlighter::for_extension("rs").unwrap();
        let v1 = "fn main() { let x = 1; }";
        incremental.reparse_full(v1);
        let full_spans = |text: &str| {
            let mut fresh = IncrementalHighlighter::for_extension("rs").unwrap();
            fresh.reparse_full(text);
            fresh.spans(text, 0..text.len())
        };
        assert_eq!(incremental.spans(v1, 0..v1.len()), full_spans(v1));

        // "1" → "\"hello\"" の単一編集（変更前座標 start=22, old="1"）
        let v2 = "fn main() { let x = \"hello\"; }";
        incremental.apply_edit(v2, 20, "1", "\"hello\"");
        assert_eq!(incremental.spans(v2, 0..v2.len()), full_spans(v2));

        // 改行を跨ぐ編集
        let v3 = "fn main() { let x = \"hello\";\n    let y = 2; }";
        incremental.apply_edit(v3, 28, "", "\n    let y = 2;");
        assert_eq!(incremental.spans(v3, 0..v3.len()), full_spans(v3));

        // 可視範囲クエリ = 範囲内だけ返る
        let ranged = incremental.spans(v3, 0..10);
        assert!(ranged.iter().all(|span| span.range.end <= 10));
        assert!(!ranged.is_empty());
    }

    #[test]
    fn markdown_combines_block_inline_and_fenced_rust() {
        let source = concat!(
            "# Beautiful title\n\n",
            "Text with **strong words**, *soft words*, and [a link](https://example.com).\n\n",
            "```rust\nfn main() { let value = \"hello\"; }\n```\n"
        );
        let mut highlighter = IncrementalHighlighter::for_extension("md").unwrap();
        highlighter.reparse_full(source);
        let spans = highlighter.spans(source, 0..source.len());
        let kind_of = |needle: &str| {
            let at = source.find(needle).unwrap();
            spans
                .iter()
                .find(|span| span.range.start <= at && at < span.range.end)
                .map(|span| span.kind)
        };
        assert_eq!(kind_of("Beautiful"), Some(HighlightKind::Heading));
        assert_eq!(kind_of("strong words"), Some(HighlightKind::Strong));
        assert_eq!(kind_of("soft words"), Some(HighlightKind::Emphasis));
        assert_eq!(kind_of("a link"), Some(HighlightKind::Link));
        assert_eq!(kind_of("fn main"), Some(HighlightKind::Keyword));
        assert_eq!(kind_of("\"hello\""), Some(HighlightKind::String));
        for pair in spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "span must not overlap"
            );
        }
    }

    #[test]
    fn markdown_incremental_edit_matches_a_fresh_parse() {
        let before = "# Title\n\nA **bold** value.\n";
        let after = "# Title\n\nA **strong** value.\n";
        let start = before.find("bold").unwrap();
        let mut incremental = IncrementalHighlighter::for_extension("markdown").unwrap();
        incremental.reparse_full(before);
        incremental.apply_edit(after, start, "bold", "strong");

        let mut fresh = IncrementalHighlighter::for_extension("md").unwrap();
        fresh.reparse_full(after);
        assert_eq!(
            incremental.spans(after, 0..after.len()),
            fresh.spans(after, 0..after.len())
        );
    }

    #[test]
    fn point_helpers_count_rows_and_columns() {
        let text = "ab\ncde\nf";
        assert_eq!(point_at(text, 0), tree_sitter::Point::new(0, 0));
        assert_eq!(point_at(text, 4), tree_sitter::Point::new(1, 1));
        assert_eq!(point_at(text, 8), tree_sitter::Point::new(2, 1)); // "f" の後
        assert_eq!(
            advance_point(tree_sitter::Point::new(1, 2), "xy"),
            tree_sitter::Point::new(1, 4)
        );
        assert_eq!(
            advance_point(tree_sitter::Point::new(1, 2), "x\nyz"),
            tree_sitter::Point::new(2, 2)
        );
    }
}

#[cfg(test)]
mod multilang_tests {
    use super::*;

    #[test]
    fn highlighters_compile_and_produce_spans_for_all_languages() {
        let samples: &[(&str, &str)] = &[
            ("rs", "fn main() { let x = 1; }"),
            ("js", "function f(a) { return a + 1; }"),
            ("ts", "function f(a: number): number { return a; }"),
            ("tsx", "const x = <div className=\"a\">hi</div>;"),
            ("py", "def f(a):\n    return a + 1\n"),
            ("go", "package main\nfunc main() { x := 1 }\n"),
            ("json", "{ \"key\": [1, 2, true] }"),
            ("yml", "key: value\nlist:\n  - 1\n"),
            ("toml", "[table]\nkey = \"value\"\n"),
            ("html", "<html><body class=\"x\">hi</body></html>"),
            ("css", ".cls { color: red; }"),
            ("sh", "#!/bin/sh\nname=world\necho \"hello $name\"\n"),
            ("c", "int main(void) { return 0; }"),
            (
                "cpp",
                "class Thing { public: int value() const { return 1; } };",
            ),
        ];
        for (extension, sample) in samples {
            let highlighter = Highlighter::for_extension(extension)
                .unwrap_or_else(|| panic!("{extension} のハイライタが作れない"));
            let spans = highlighter.highlight(sample);
            assert!(!spans.is_empty(), "{extension} の span が空");
            // 非重複・昇順の不変条件
            for pair in spans.windows(2) {
                assert!(
                    pair[0].range.end <= pair[1].range.start,
                    "{extension} で重複 span"
                );
            }
        }
    }

    #[test]
    fn yaml_values_receive_syntax_kinds() {
        let source = "name: \"shirushi\"\nenabled: true\ncount: 3\n";
        let highlighter = Highlighter::for_extension("yaml").unwrap();
        let spans = highlighter.highlight(source);
        let string_at = source.find("\"shirushi\"").unwrap();
        assert!(spans
            .iter()
            .any(|span| { span.kind == HighlightKind::String && span.range.contains(&string_at) }));
        assert!(!spans.is_empty());
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    #[test]
    fn rust_outline_lists_definitions_in_order() {
        let text = "mod helpers;\n\npub struct Thing { x: u32 }\n\nimpl Thing {\n    pub fn new() -> Self { Self { x: 0 } }\n}\n\nfn main() {}\n";
        let items = outline(text, "rs");
        let summary: Vec<(&str, &str, usize)> = items
            .iter()
            .map(|item| (item.kind, item.name.as_str(), item.row))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("mod", "helpers", 0),
                ("struct", "Thing", 2),
                ("impl", "Thing", 4),
                ("fn", "new", 5),
                ("fn", "main", 8),
            ]
        );
        // 非対応言語は空
        assert!(outline(text, "md").is_empty());
    }
}
