//! paths — 設定・データ・状態ファイルの置き場を決める**唯一の場所**（WINDOWS-PORT.md §D1）。
//!
//! 依存ゼロ。ARCHITECTURE §1 の層構造では `i18n` / `theme_core` と同じ最下層に置く。
//!
//! ## なぜこの crate が要るか
//!
//! 以前は `~/Library/Application Support/necoder/...` が 9 ファイルに散らばり、`HOME` の直読みが
//! 19 箇所あった。これは Windows 移植の障害である前に **Linux での現行バグ**で、Linux で起動すると
//! macOS のディレクトリ名（`Library/Application Support`）を Linux 上に作ってしまう。
//!
//! ## 鉄則（WINDOWS-PORT.md §D8）
//!
//! **macOS の戻り値は 1 文字も変えない。** 既存ユーザーの設定・DB・スレッド履歴が引っ越すのは事故。
//! そのため mac の全パスを unit test で固定してある。
//!
//! ## テスト可能性の設計（重要）
//!
//! 分岐の核は `*_on` 系の内部関数で、**`Platform` と環境変数取得関数を引数で受け取る**。
//! ＝ **Windows や Linux の上でも macOS のパス生成をテストできる。**
//!
//! `#[cfg(target_os = "macos")]` でテストを切る書き方をすると、mac の期待値が mac 上でしか
//! 検証されず、「**mac を壊していないこと**」を Windows/Linux の CI が保証できなくなる。
//! W フェーズの変更は共有コードに入る以上、そこを機械で止められる形にしておく。
//!
//! ## 差し替え口
//!
//! `NECODER_HOME` を 1 つ置けば config/data/state/cache の全部が根から差し替わる
//! （テストが実ユーザーのデータを触らないため）。個別の互換変数
//! （`NECODER_LOG_DIR` / `NECODER_CRASH_DIR` / `NECODER_SHELL_PATH_CACHE` / `NECODER_GUI_SOCK`）
//! も従来どおり効く。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// アプリのディレクトリ名。macOS の `Library/Application Support/necoder` の末尾もこれ。
const APP: &str = "necoder";

/// 旧ブランド名（DECISIONS §8 の改名・2026-08-22）。`brand_migration` の引っ越し元。
const LEGACY_APP: &str = "Shirushi";

/// パスの決め方が変わるプラットフォームの区別。
///
/// `target_os` そのものではなく「パスの流儀」を表す。Linux 以外の unix（BSD 等）は
/// XDG に倣うので [`Platform::Linux`] に寄せる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    /// 実行中のプラットフォーム。
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// 実環境の環境変数を読む。テストはこれと同じ形の関数を差し替える。
fn real_var(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

// ---------------------------------------------------------------------------
// 公開 API（呼び出し側はここだけ使う）
// ---------------------------------------------------------------------------

/// ホームディレクトリ。
///
/// macOS/Linux は `HOME`。Windows は `USERPROFILE` →（無ければ）`HOMEDRIVE`+`HOMEPATH` →
/// 最後に `HOME`（Git Bash / MSYS が設定する）の順で見る。
pub fn home_dir() -> Option<PathBuf> {
    home_on(Platform::current(), &real_var)
}

/// 人が編集するもの（settings.json / keymap.json / テーマ）の置き場。
///
/// Windows で Roaming を使う理由: ドメイン環境でサーバへ同期され、別マシンでも設定が付いてくる。
pub fn config_dir() -> Option<PathBuf> {
    config_dir_on(Platform::current(), &real_var)
}

/// 機械が読み書きするもの（necoder.db / blobs）の置き場。
///
/// Windows で Local を使う理由: **数百 MB になりうる DB と blob を Roaming に置くとログオンが死ぬ**
/// （ドメイン環境でプロファイル同期に載る）。「人が編集する設定 = Roaming / 機械が書くもの = Local」
/// が Windows の作法。
pub fn data_dir() -> Option<PathBuf> {
    data_dir_on(Platform::current(), &real_var)
}

/// 状態（state.json / logs / crashes）の置き場。Windows では data と同じ Local。
pub fn state_dir() -> Option<PathBuf> {
    state_dir_on(Platform::current(), &real_var)
}

/// 捨ててよいキャッシュの置き場。
pub fn cache_dir() -> Option<PathBuf> {
    cache_dir_on(Platform::current(), &real_var)
}

/// 制御 IPC の口。
///
/// - macOS: `~/.necoder/gui.sock`（`SUN_LEN` ~104B に収まる短いパス）
/// - Linux: `$XDG_RUNTIME_DIR/necoder/gui.sock`（無ければ mac と同じ `~/.necoder/gui.sock`）
/// - Windows: `\\.\pipe\necoder-gui-<user>`（**名前付きパイプ**。TCP loopback は他ユーザーから
///   叩けてしまい「守るべき操作の単一経路」が崩れるので採らない・§D2）
pub fn runtime_socket() -> Option<PathBuf> {
    runtime_socket_on(Platform::current(), &real_var)
}

/// ユーザー設定 `settings.json`。
pub fn settings_file() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.json"))
}

/// ユーザー keymap `keymap.json`。
pub fn keymap_file() -> Option<PathBuf> {
    Some(config_dir()?.join("keymap.json"))
}

/// Task ledger / スレッド履歴の DB。
pub fn db_file() -> Option<PathBuf> {
    Some(data_dir()?.join("necoder.db"))
}

/// チェックポイント blob の置き場。
pub fn blobs_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("blobs"))
}

/// 窓の復元状態 `state.json`。
pub fn state_file() -> Option<PathBuf> {
    Some(state_dir()?.join("state.json"))
}

/// 外部 ACP エージェント関連の置き場（レジストリのキャッシュ・将来の配備先）。
pub fn external_agents_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("external_agents"))
}

/// ACP 公開レジストリのキャッシュファイル。`NECODER_ACP_REGISTRY_CACHE` で差し替え可（テスト用）。
pub fn acp_registry_cache() -> Option<PathBuf> {
    if let Some(path) = real_var("NECODER_ACP_REGISTRY_CACHE") {
        return Some(PathBuf::from(path));
    }
    Some(
        external_agents_dir()?
            .join("registry")
            .join("registry.json"),
    )
}

/// ログの置き場。`NECODER_LOG_DIR` で差し替え可（互換）。
pub fn logs_dir() -> Option<PathBuf> {
    if let Some(path) = real_var("NECODER_LOG_DIR") {
        return Some(PathBuf::from(path));
    }
    Some(state_dir()?.join("logs"))
}

/// クラッシュログの置き場。`NECODER_CRASH_DIR` で差し替え可（互換）。
pub fn crashes_dir() -> Option<PathBuf> {
    if let Some(path) = real_var("NECODER_CRASH_DIR") {
        return Some(PathBuf::from(path));
    }
    Some(state_dir()?.join("crashes"))
}

/// login shell の PATH キャッシュ。`NECODER_SHELL_PATH_CACHE` で差し替え可（互換）。
///
/// **Windows では `None`**。Windows に login shell の PATH という概念が無く、環境変数 PATH が
/// そのまま子プロセスへ継承されるため、キャッシュする対象が存在しない（§D1）。
pub fn shell_path_cache() -> Option<PathBuf> {
    if let Some(path) = real_var("NECODER_SHELL_PATH_CACHE") {
        return Some(PathBuf::from(path));
    }
    shell_path_cache_on(Platform::current(), &real_var)
}

/// 実在するパスを「**比較に使える正規形**」にする。`std::fs::canonicalize` の置き換え。
///
/// ## なぜ std のままでは駄目か（Windows の実害）
///
/// Windows の `std::fs::canonicalize` は **verbatim 形式**（`\\?\C:\Users\…`）を返す。
/// 一方 `git rev-parse --show-toplevel` のような外部コマンドは通常形（`C:/Users/…`）を返す。
/// この 2 つは `Path` として**等しくない**（前者は `VerbatimDisk`、後者は `Disk` プレフィクス）。
///
/// つまり同じファイルが別物として扱われる ＝ **git status の色が付かない・同じファイルの
/// タブが二重に開く・エクスプローラの選択が合わない**。2026-08-22 に project の
/// git status テスト 2 本が実際にこれで落ちた。
///
/// なお区切り文字の違い（`/` と `\`）は問題にならない。Windows の `Path` は
/// どちらも区切りとして扱うのでコンポーネント比較で一致する。**verbatim プレフィクスだけ**が
/// 食い違いの原因なので、それを剥がして揃える。
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    // ここが唯一 `std::fs::canonicalize` を呼んでよい場所（他は全部この関数を経由する）。
    let canonical = std::fs::canonicalize(path)?;
    Ok(normalize_canonical(canonical))
}

/// [`canonicalize`] に失敗したら元のパスをそのまま返す版（存在しないパスを扱う場所用）。
pub fn canonicalize_or_keep(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_canonical(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    match strip_verbatim_prefix(&path.to_string_lossy()) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

/// `\\?\C:\x` → `C:\x` / `\\?\UNC\server\share` → `\\server\share`。剥がすものが無ければ `None`。
///
/// 純粋な文字列操作にしてあるので **どのプラットフォームでもテストできる**
/// （Windows の挙動を mac の CI が守れる）。
fn strip_verbatim_prefix(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    text.strip_prefix(r"\\?\").map(str::to_string)
}

/// 旧ブランド（Shirushi）のアプリ支援ディレクトリ。`brand_migration` の引っ越し元。
///
/// **macOS 以外では `None`**。改名前（2026-08-22 以前）は macOS でしか配布していないので、
/// 他のプラットフォームには引っ越すべきデータが原理的に存在しない。
pub fn legacy_brand_dir() -> Option<PathBuf> {
    legacy_brand_dir_on(Platform::current(), &real_var)
}

// ---------------------------------------------------------------------------
// 内部（テストから Platform と環境変数を注入して呼ぶ）
// ---------------------------------------------------------------------------

/// `NECODER_HOME`（全体の差し替え口）。設定されていれば config/data/state/cache が全部ここになる。
fn necoder_home_on(var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    var("NECODER_HOME").map(PathBuf::from)
}

fn home_on(platform: Platform, var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match platform {
        Platform::MacOs | Platform::Linux => var("HOME").map(PathBuf::from),
        Platform::Windows => var("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                // ドメイン環境では USERPROFILE が無いことがある。
                let drive = var("HOMEDRIVE")?;
                let path = var("HOMEPATH")?;
                let mut joined = drive;
                joined.push(path);
                Some(PathBuf::from(joined))
            })
            // Git Bash / MSYS 経由で起動された場合はこれしか無いことがある。
            .or_else(|| var("HOME").map(PathBuf::from)),
    }
}

/// macOS のアプリ支援ディレクトリ。**この文字列は変えてはいけない**（§D8）。
fn macos_support_dir(app: &str, var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    Some(
        home_on(Platform::MacOs, var)?
            .join("Library/Application Support")
            .join(app),
    )
}

/// POSIX の意味での絶対パス判定（先頭が `/`）。
///
/// **`Path::is_absolute()` を使ってはいけない。** あれは「実行中 OS の規則」で判定するので、
/// Windows 上で Linux の挙動をテストすると `/custom/config` が「相対パス」と見なされて落ちる
/// （2026-08-22 に実際に踏んだ）。この crate は「実行中 OS ではなく対象 Platform の流儀で
/// パスを決める」ためにあるのだから、判定側も実行中 OS に依存させない。
fn is_posix_absolute(value: &OsString) -> bool {
    value.to_string_lossy().starts_with('/')
}

/// XDG のディレクトリ。`$<variable>` があればそれ、無ければ `~/<fallback>`。
fn xdg_dir(
    variable: &str,
    fallback: &str,
    var: &impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    let base = match var(variable) {
        // XDG 仕様: 相対パスが入っていたら「未設定」として扱う
        Some(value) if is_posix_absolute(&value) => PathBuf::from(value),
        _ => home_on(Platform::Linux, var)?.join(fallback),
    };
    Some(base.join(APP))
}

fn config_dir_on(platform: Platform, var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(root) = necoder_home_on(var) {
        return Some(root);
    }
    match platform {
        Platform::MacOs => macos_support_dir(APP, var),
        // Roaming（設定は別マシンにも付いてくるべき）
        Platform::Windows => windows_base(var, "APPDATA", "AppData/Roaming"),
        Platform::Linux => xdg_dir("XDG_CONFIG_HOME", ".config", var),
    }
}

fn data_dir_on(platform: Platform, var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(root) = necoder_home_on(var) {
        return Some(root);
    }
    match platform {
        Platform::MacOs => macos_support_dir(APP, var),
        // Local（DB と blob は数百 MB になりうる。Roaming に置くとログオンが死ぬ）
        Platform::Windows => windows_base(var, "LOCALAPPDATA", "AppData/Local"),
        Platform::Linux => xdg_dir("XDG_DATA_HOME", ".local/share", var),
    }
}

fn state_dir_on(platform: Platform, var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(root) = necoder_home_on(var) {
        return Some(root);
    }
    match platform {
        Platform::MacOs => macos_support_dir(APP, var),
        Platform::Windows => windows_base(var, "LOCALAPPDATA", "AppData/Local"),
        Platform::Linux => xdg_dir("XDG_STATE_HOME", ".local/state", var),
    }
}

fn cache_dir_on(platform: Platform, var: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(root) = necoder_home_on(var) {
        return Some(root);
    }
    match platform {
        Platform::MacOs => macos_support_dir(APP, var),
        Platform::Windows => windows_base(var, "LOCALAPPDATA", "AppData/Local"),
        Platform::Linux => xdg_dir("XDG_CACHE_HOME", ".cache", var),
    }
}

/// Windows の `%APPDATA%` / `%LOCALAPPDATA%`。環境変数が無ければホームからの既定位置で組む。
fn windows_base(
    var: &impl Fn(&str) -> Option<OsString>,
    variable: &str,
    fallback: &str,
) -> Option<PathBuf> {
    let base = match var(variable) {
        Some(value) => PathBuf::from(value),
        None => home_on(Platform::Windows, var)?.join(fallback),
    };
    Some(base.join(APP))
}

fn shell_path_cache_on(
    platform: Platform,
    var: &impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    match platform {
        // Windows に login shell PATH の概念が無い＝キャッシュする対象が無い
        Platform::Windows => None,
        Platform::MacOs => Some(macos_support_dir(APP, var)?.join("shell-path")),
        Platform::Linux => Some(cache_dir_on(platform, var)?.join("shell-path")),
    }
}

fn runtime_socket_on(
    platform: Platform,
    var: &impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    if let Some(path) = var("NECODER_GUI_SOCK") {
        return Some(PathBuf::from(path));
    }
    match platform {
        // mac の既存パス。変えない（§D8）
        Platform::MacOs => Some(home_on(platform, var)?.join(".necoder/gui.sock")),
        Platform::Linux => match var("XDG_RUNTIME_DIR") {
            Some(runtime) => Some(PathBuf::from(runtime).join(APP).join("gui.sock")),
            None => Some(home_on(platform, var)?.join(".necoder/gui.sock")),
        },
        Platform::Windows => {
            // 名前付きパイプ。ユーザー名を含めて他ユーザーのものと衝突させない。
            let user = var("USERNAME")
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "default".to_string());
            let sanitized: String = user
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                        character
                    } else {
                        '_'
                    }
                })
                .collect();
            Some(PathBuf::from(format!(r"\\.\pipe\necoder-gui-{sanitized}")))
        }
    }
}

fn legacy_brand_dir_on(
    platform: Platform,
    var: &impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    match platform {
        Platform::MacOs => macos_support_dir(LEGACY_APP, var),
        // 改名前は macOS でしか配布していない＝引っ越すデータが存在しない
        Platform::Windows | Platform::Linux => None,
    }
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 環境変数のふり。`&[(key, value)]` から引く。
    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(*value))
        }
    }

    const MAC: &[(&str, &str)] = &[("HOME", "/Users/test")];

    // -----------------------------------------------------------------------
    // macOS — 現行の戻り値を 1 文字も変えないための固定（§D8）
    //
    // これらは **Windows / Linux の CI でも実行される**。`#[cfg(target_os = "macos")]` で
    // 切ってしまうと「mac を壊していないこと」を他プラットフォームが保証できないため。
    //
    // 比較は文字列ではなく `PathBuf` で行う: `join` は実行中 OS の区切り文字を使うので、
    // Windows 上では `/Users/test\Library/Application Support` のように混ざる。`Path` の
    // 比較はコンポーネント単位で、Windows は `/` も `\` も区切りとして扱うため一致する。
    // -----------------------------------------------------------------------

    #[test]
    fn macos_config_paths_are_unchanged() {
        let var = env(MAC);
        let support = PathBuf::from("/Users/test/Library/Application Support/necoder");
        assert_eq!(config_dir_on(Platform::MacOs, &var), Some(support.clone()));
        assert_eq!(data_dir_on(Platform::MacOs, &var), Some(support.clone()));
        assert_eq!(state_dir_on(Platform::MacOs, &var), Some(support.clone()));
        assert_eq!(cache_dir_on(Platform::MacOs, &var), Some(support));
    }

    #[test]
    fn macos_every_file_path_is_unchanged() {
        let var = env(MAC);
        let support = PathBuf::from("/Users/test/Library/Application Support/necoder");
        // 置換前のコードが作っていたパスと 1 対 1 で対応する
        assert_eq!(
            config_dir_on(Platform::MacOs, &var).map(|dir| dir.join("settings.json")),
            Some(support.join("settings.json")),
            "settings_core.rs:261"
        );
        assert_eq!(
            config_dir_on(Platform::MacOs, &var).map(|dir| dir.join("keymap.json")),
            Some(support.join("keymap.json")),
            "main.rs のユーザー keymap"
        );
        assert_eq!(
            data_dir_on(Platform::MacOs, &var).map(|dir| dir.join("necoder.db")),
            Some(support.join("necoder.db")),
            "storage.rs:155"
        );
        assert_eq!(
            data_dir_on(Platform::MacOs, &var).map(|dir| dir.join("blobs")),
            Some(support.join("blobs")),
            "storage.rs:161"
        );
        assert_eq!(
            state_dir_on(Platform::MacOs, &var).map(|dir| dir.join("state.json")),
            Some(support.join("state.json")),
            "persistence.rs:40"
        );
        assert_eq!(
            state_dir_on(Platform::MacOs, &var).map(|dir| dir.join("logs")),
            Some(support.join("logs")),
            "logging.rs:27"
        );
        assert_eq!(
            state_dir_on(Platform::MacOs, &var).map(|dir| dir.join("crashes")),
            Some(support.join("crashes")),
            "crash.rs:29"
        );
        assert_eq!(
            shell_path_cache_on(Platform::MacOs, &var),
            Some(support.join("shell-path")),
            "shell_env.rs:50"
        );
    }

    #[test]
    fn macos_control_socket_is_unchanged() {
        let var = env(MAC);
        assert_eq!(
            runtime_socket_on(Platform::MacOs, &var),
            Some(PathBuf::from("/Users/test/.necoder/gui.sock")),
            "control_ipc.rs:31"
        );
    }

    #[test]
    fn macos_legacy_brand_dir_is_unchanged() {
        let var = env(MAC);
        assert_eq!(
            legacy_brand_dir_on(Platform::MacOs, &var),
            Some(PathBuf::from(
                "/Users/test/Library/Application Support/Shirushi"
            )),
            "brand_migration.rs:23"
        );
    }

    #[test]
    fn macos_without_home_yields_none() {
        let var = env(&[]);
        assert_eq!(config_dir_on(Platform::MacOs, &var), None);
        assert_eq!(runtime_socket_on(Platform::MacOs, &var), None);
    }

    // -----------------------------------------------------------------------
    // Windows
    // -----------------------------------------------------------------------

    const WINDOWS: &[(&str, &str)] = &[
        ("USERPROFILE", r"C:\Users\test"),
        ("APPDATA", r"C:\Users\test\AppData\Roaming"),
        ("LOCALAPPDATA", r"C:\Users\test\AppData\Local"),
        ("USERNAME", "test"),
    ];

    /// **Windows のパスは区切り文字を無視して比べる。**
    ///
    /// mac の期待値は `PathBuf` 同士で比較できる（Windows は `/` も `\` も区切りとして扱うので
    /// コンポーネント比較で一致する）。**だが逆は成り立たない** — mac / Linux は `\` を
    /// **ただの文字**として扱うので、`PathBuf::from(r"C:\a\b")` は 1 コンポーネントになり、
    /// `join` で組んだ `C:\a\b/necoder` と一致しない。
    ///
    /// 2026-08-24 に CI の test-macos で実際に落ちた。「mac の期待値を Windows で検証できる
    /// ようにする」配慮を入れたのに、**逆向きが抜けていた**。この crate が防ごうとしている
    /// バグ（実行中 OS の規則に引きずられる）の鏡写しなので、教訓としてここに残す。
    fn assert_windows_path(actual: Option<PathBuf>, expected: &str, note: &str) {
        let actual = actual.expect(note);
        let normalize = |text: &str| text.replace('\\', "/");
        assert_eq!(
            normalize(&actual.to_string_lossy()),
            normalize(expected),
            "{note}"
        );
    }

    #[test]
    fn windows_separates_roaming_and_local() {
        let var = env(WINDOWS);
        assert_windows_path(
            config_dir_on(Platform::Windows, &var),
            r"C:\Users\test\AppData\Roaming\necoder",
            "人が編集する設定は Roaming",
        );
        // DB と blob は数百 MB になりうる。Roaming に置くとログオンが死ぬ
        assert_windows_path(
            data_dir_on(Platform::Windows, &var),
            r"C:\Users\test\AppData\Local\necoder",
            "DB と blob は Local",
        );
        assert_windows_path(
            state_dir_on(Platform::Windows, &var),
            r"C:\Users\test\AppData\Local\necoder",
            "state も Local",
        );
    }

    #[test]
    fn windows_never_creates_macos_directories() {
        let var = env(WINDOWS);
        for dir in [
            config_dir_on(Platform::Windows, &var),
            data_dir_on(Platform::Windows, &var),
            state_dir_on(Platform::Windows, &var),
            cache_dir_on(Platform::Windows, &var),
        ] {
            let rendered = dir
                .expect("Windows のパスが決まらない")
                .to_string_lossy()
                .into_owned();
            assert!(
                !rendered.contains("Library"),
                "macOS のディレクトリ名が Windows に漏れている: {rendered}"
            );
        }
    }

    #[test]
    fn windows_uses_a_named_pipe_not_a_socket_file() {
        let var = env(WINDOWS);
        assert_eq!(
            runtime_socket_on(Platform::Windows, &var),
            Some(PathBuf::from(r"\\.\pipe\necoder-gui-test"))
        );
    }

    #[test]
    fn windows_pipe_name_sanitizes_domain_user() {
        // ドメイン環境の `DOMAIN\user` がそのままだとパイプ名の階層になってしまう
        let var = env(&[("USERPROFILE", r"C:\Users\test"), ("USERNAME", r"CORP\a b")]);
        assert_eq!(
            runtime_socket_on(Platform::Windows, &var),
            Some(PathBuf::from(r"\\.\pipe\necoder-gui-CORP_a_b"))
        );
    }

    #[test]
    fn windows_has_no_shell_path_cache() {
        let var = env(WINDOWS);
        assert_eq!(
            shell_path_cache_on(Platform::Windows, &var),
            None,
            "Windows に login shell PATH の概念は無い"
        );
    }

    #[test]
    fn windows_has_no_legacy_brand_dir() {
        let var = env(WINDOWS);
        assert_eq!(legacy_brand_dir_on(Platform::Windows, &var), None);
    }

    #[test]
    fn windows_falls_back_to_homedrive_and_homepath() {
        let var = env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\test")]);
        assert_windows_path(
            home_on(Platform::Windows, &var),
            r"C:\Users\test",
            "HOMEDRIVE + HOMEPATH でホームが組める",
        );
        // APPDATA が無い環境でもホームから組める
        assert_windows_path(
            config_dir_on(Platform::Windows, &var),
            r"C:\Users\test\AppData\Roaming\necoder",
            "APPDATA 不在でもホームから組める",
        );
    }

    // -----------------------------------------------------------------------
    // Linux — 「Linux で Library/Application Support を作る」現行バグの回帰テスト
    // -----------------------------------------------------------------------

    #[test]
    fn linux_follows_xdg_and_never_creates_macos_directories() {
        let var = env(&[("HOME", "/home/test")]);
        assert_eq!(
            config_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/home/test/.config/necoder"))
        );
        assert_eq!(
            data_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/home/test/.local/share/necoder"))
        );
        assert_eq!(
            state_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/home/test/.local/state/necoder"))
        );
        assert_eq!(
            cache_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/home/test/.cache/necoder"))
        );
    }

    #[test]
    fn linux_honors_xdg_variables() {
        let var = env(&[
            ("HOME", "/home/test"),
            ("XDG_CONFIG_HOME", "/custom/config"),
            ("XDG_DATA_HOME", "/custom/data"),
        ]);
        assert_eq!(
            config_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/custom/config/necoder"))
        );
        assert_eq!(
            data_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/custom/data/necoder"))
        );
    }

    #[test]
    fn linux_ignores_relative_xdg_variables() {
        // XDG 仕様: 相対パスは未設定として扱う
        let var = env(&[("HOME", "/home/test"), ("XDG_CONFIG_HOME", "relative/path")]);
        assert_eq!(
            config_dir_on(Platform::Linux, &var),
            Some(PathBuf::from("/home/test/.config/necoder"))
        );
    }

    #[test]
    fn linux_runtime_socket_prefers_xdg_runtime_dir() {
        let with_runtime = env(&[
            ("HOME", "/home/test"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]);
        assert_eq!(
            runtime_socket_on(Platform::Linux, &with_runtime),
            Some(PathBuf::from("/run/user/1000/necoder/gui.sock"))
        );
        let without_runtime = env(&[("HOME", "/home/test")]);
        assert_eq!(
            runtime_socket_on(Platform::Linux, &without_runtime),
            Some(PathBuf::from("/home/test/.necoder/gui.sock"))
        );
    }

    // -----------------------------------------------------------------------
    // NECODER_HOME（全体の差し替え口）
    // -----------------------------------------------------------------------

    #[test]
    fn necoder_home_overrides_every_platform() {
        for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            let var = env(&[
                ("NECODER_HOME", "/tmp/necoder-test"),
                ("HOME", "/Users/test"),
                ("USERPROFILE", r"C:\Users\test"),
            ]);
            let root = PathBuf::from("/tmp/necoder-test");
            assert_eq!(
                config_dir_on(platform, &var),
                Some(root.clone()),
                "{platform:?}"
            );
            assert_eq!(
                data_dir_on(platform, &var),
                Some(root.clone()),
                "{platform:?}"
            );
            assert_eq!(
                state_dir_on(platform, &var),
                Some(root.clone()),
                "{platform:?}"
            );
            assert_eq!(cache_dir_on(platform, &var), Some(root), "{platform:?}");
        }
    }

    #[test]
    fn necoder_gui_sock_overrides_every_platform() {
        for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            let var = env(&[
                ("NECODER_GUI_SOCK", "/tmp/custom.sock"),
                ("HOME", "/Users/test"),
            ]);
            assert_eq!(
                runtime_socket_on(platform, &var),
                Some(PathBuf::from("/tmp/custom.sock")),
                "{platform:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // canonicalize の verbatim 剥がし
    //
    // 文字列操作なので **どのプラットフォームでも実行される**。Windows の挙動を
    // mac / Linux の CI が守れる形にしてある。
    // -----------------------------------------------------------------------

    #[test]
    fn verbatim_prefix_is_stripped_so_paths_compare_equal_to_git_output() {
        // `paths::canonicalize` の戻り値 → 外部コマンドが返す形へ揃える
        assert_eq!(
            strip_verbatim_prefix(r"\\?\C:\Users\a\project"),
            Some(r"C:\Users\a\project".to_string())
        );
        // ネットワークパスは `\\server\share` へ戻す
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\server\share\project"),
            Some(r"\\server\share\project".to_string())
        );
    }

    #[test]
    fn ordinary_paths_are_left_alone() {
        assert_eq!(strip_verbatim_prefix(r"C:\Users\a"), None);
        assert_eq!(strip_verbatim_prefix("/Users/a"), None);
        assert_eq!(strip_verbatim_prefix(r"\\server\share"), None);
    }

    #[test]
    fn canonicalize_matches_a_path_built_the_way_git_reports_it() {
        // 実在パスで往復させる。Windows では verbatim が剥がれていないと
        // 「git が返す形」と一致しない＝git status の色が付かないバグになる。
        let temporary = std::env::temp_dir();
        let canonical = canonicalize(&temporary).expect("一時ディレクトリは実在する");
        let rendered = canonical.to_string_lossy();
        assert!(
            !rendered.starts_with(r"\\?\"),
            "verbatim プレフィクスが残っている: {rendered}"
        );
        // 区切り文字だけが違うパスは Path として一致する（＝正規化の対象外でよい）
        let with_forward_slashes = PathBuf::from(rendered.replace('\\', "/"));
        assert_eq!(canonical, with_forward_slashes);
    }

    #[test]
    fn platform_current_matches_the_compilation_target() {
        let expected = if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        };
        assert_eq!(Platform::current(), expected);
    }
}
