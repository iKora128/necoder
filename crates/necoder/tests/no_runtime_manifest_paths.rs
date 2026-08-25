//! **ビルド時のパスを実行時に開いていないか**を機械で止める（2026-08-25 追加）。
//!
//! ## なぜ要るか
//!
//! v0.1.0 と v0.1.1 の配布物（`.dmg` / `.zip`）は **他人の PC で起動しなかった**。原因は
//! `crates/agent_panel` のマスコット読み込みがこう書かれていたこと:
//!
//! ```ignore
//! let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mascot")).join(file);
//! let mut source = image::open(&path)...
//! ```
//!
//! `env!("CARGO_MANIFEST_DIR")` は **ビルド時のパスを文字列として焼き込むだけ**なので、
//! 実行時に開こうとするとビルドした機械以外では必ず失敗する:
//!
//! ```text
//! mascot asset D:\a\necoder\necoder\crates\agent_panel/assets/mascot\idle.png:
//! 指定されたパスが見つかりません。 (os error 3)
//! ```
//!
//! （`D:\a\necoder\necoder` は GitHub Actions ランナーのパス）
//!
//! **このバグは開発機では原理的に再現しない。** そのパスが手元には実在するからで、
//! 何度起動しても気づけない。CI にも GUI 起動テストは無い。だから**人間の目では防げず、
//! 機械で止めるしかない**。実際、リリースを 2 回すり抜けた。
//!
//! ## 何を許すか
//!
//! - `include_bytes!` / `include_str!` の中 — **コンパイル時に埋め込む**ので実行環境に依存しない。
//!   フォント（`necoder/src/main.rs` の `load_fonts`）とアイコン（同 `Assets`）は元からこの形
//! - 下の `ALLOWED` に列挙した箇所 — テストの中か、無くても壊れないと確認済みのもの
//!
//! それ以外は落とす。**新しく足すなら理由を書いて `ALLOWED` に載せること**（＝人間の判断を強制する）。

use std::path::{Path, PathBuf};

/// 実行時パスとして `CARGO_MANIFEST_DIR` を使ってよいと判断済みの箇所。
///
/// `(リポジトリ相対パス, 件数, 理由)`。**件数まで固定する**のは、同じファイルに
/// 新しい用例が紛れ込んだときに気づけるようにするため。
const ALLOWED: &[(&str, usize, &str)] = &[
    (
        "crates/host/src/host.rs",
        1,
        "find_local_remote_server の開発用フォールバック。exe の隣を先に見て、\
         無ければ `.is_file().then_some()` で None を返すだけ＝配布物では黙って無視される",
    ),
    (
        "crates/lang/src/lsp.rs",
        1,
        "#[cfg(test)] の中。実 rust-analyzer との結合テストで repo ルートを求めている",
    ),
    (
        "crates/project/src/project.rs",
        1,
        "#[cfg(test)] の中。この repo 自身のコミットで git blame を検証している",
    ),
];

/// `crates/necoder` → リポジトリルート。ここはテストなので `CARGO_MANIFEST_DIR` を使ってよい。
fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/necoder の 2 つ上がリポジトリルート")
        .to_path_buf()
}

/// `crates/*/src` 以下の `.rs` を集める。
fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let crates = root.join("crates");
    let entries = std::fs::read_dir(&crates).expect("crates/ を読めない");
    for entry in entries.flatten() {
        let source_dir = entry.path().join("src");
        if source_dir.is_dir() {
            collect_rust_files(&source_dir, &mut found);
        }
    }
    found.sort();
    found
}

fn collect_rust_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// 行コメントか（`//` `///` `//!` すべて）。
///
/// **コメントを除かないと自分の解説文で落ちる。** この検査を入れた当日、
/// 「以前は実行時のパスとして渡していた」と書いた doc コメントを自分で検出した。
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `env!("CARGO_MANIFEST_DIR")` のうち **埋め込みでないもの**の行番号を返す。
///
/// `include_bytes!(concat!(` と `env!(...)` は改行で分かれることがある
/// （`main.rs` の `load_fonts` がその形）ので、**直前数行も見る**。
fn runtime_uses(text: &str) -> Vec<usize> {
    const NEEDLE: &str = r#"env!("CARGO_MANIFEST_DIR")"#;
    const LOOKBEHIND: usize = 3;
    let lines: Vec<&str> = text.lines().collect();
    let mut hits = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(NEEDLE) || is_comment(line) {
            continue;
        }
        let window_start = index.saturating_sub(LOOKBEHIND);
        let embedded = lines[window_start..=index]
            .iter()
            .any(|candidate| candidate.contains("include_bytes!") || candidate.contains("include_str!"));
        if !embedded {
            hits.push(index + 1);
        }
    }
    hits
}

#[test]
fn build_time_paths_are_never_opened_at_runtime() {
    let root = repository_root();
    let mut violations = Vec::new();

    for file in source_files(&root) {
        let text = std::fs::read_to_string(&file).expect("ソースを読めない");
        let hits = runtime_uses(&text);
        if hits.is_empty() {
            continue;
        }
        let relative = file
            .strip_prefix(&root)
            .expect("リポジトリ内のはず")
            .to_string_lossy()
            .replace('\\', "/");
        match ALLOWED.iter().find(|(path, _, _)| *path == relative) {
            Some((_, allowed_count, reason)) if hits.len() == *allowed_count => {}
            Some((_, allowed_count, _)) => violations.push(format!(
                "{relative}: 許可は {allowed_count} 件だが {} 件ある（行 {hits:?}）。\
                 増えた分が本当に安全か確かめて、ALLOWED の件数と理由を更新すること",
                hits.len()
            )),
            None => violations.push(format!(
                "{relative}: 行 {hits:?} が `env!(\"CARGO_MANIFEST_DIR\")` を\
                 実行時のパスとして使っている疑い"
            )),
        }
    }

    assert!(
        violations.is_empty(),
        "ビルド時のパスを実行時に開いている箇所がある（配布物が他人の PC で起動しなくなる）:\n{}\n\n\
         資産を読むなら `include_bytes!` で**埋め込む**こと。\
         どうしても実行時パスが要るなら、無いときに壊れないことを確かめて\
         crates/necoder/tests/no_runtime_manifest_paths.rs の ALLOWED に理由付きで載せること。",
        violations.join("\n")
    );
}

/// 許可リストの理由が空でないこと（「とりあえず載せる」を防ぐ）。
#[test]
fn every_allowance_states_a_reason() {
    for (path, _, reason) in ALLOWED {
        assert!(
            reason.len() > 20,
            "{path} の許可理由が短すぎる: {reason:?}"
        );
    }
}

/// **検出器が本当に落ちるか**を合成テキストで確かめる。
///
/// 「絶対に落ちない検査」を置いてしまうのが一番まずい（あるつもりになる）ので、
/// 通る側と落ちる側の両方を固定する。
#[test]
fn the_detector_itself_catches_the_bug_it_is_meant_to_catch() {
    // 落ちるべき: v0.1.0 / v0.1.1 を壊した実物の形。
    let broken = r#"
        let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mascot")).join(file);
        let source = image::open(&path);
    "#;
    assert_eq!(runtime_uses(broken), vec![2], "実行時パスを見逃している");

    // 通るべき: 同じ行で埋め込む形（マスコットの修正後・アイコンの macro）。
    let inline = r#"
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mascot/", $name)).as_slice()
    "#;
    assert!(runtime_uses(inline).is_empty(), "埋め込みを誤検知している");

    // 通るべき: 改行で分かれる形（main.rs の load_fonts）。
    let wrapped = r#"
        Cow::Borrowed(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/IBMPlexSansJP-Regular.ttf"
        ))),
    "#;
    assert!(
        runtime_uses(wrapped).is_empty(),
        "改行を挟んだ include_bytes! を誤検知している"
    );

    // 通るべき: コメントの中（この検査自身の解説文がこれで落ちた）。
    let commented = r#"
        /// 以前は env!("CARGO_MANIFEST_DIR") を実行時のパスとして開いていた。
        // let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    "#;
    assert!(
        runtime_uses(commented).is_empty(),
        "コメントを誤検知している"
    );
}
