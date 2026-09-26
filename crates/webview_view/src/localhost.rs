//! localhost — 開発サーバ（`http://localhost:<port>` など）を **Web タブ**で見せるための線引き。
//!
//! Web タブは汎用ブラウザではない（FEATURES §9 の never「拡張 / API 向けの汎用 webview」に踏み込まない）。
//! 最上位の文書として読み込めるのは**このマシンのループバックの http(s)** だけで、それ以外への移動と
//! 新しい窓はキャンセルして既定のブラウザへ渡す。任意の URL を打てるアドレスバーも作らない
//! （入力を受けるのはパレットの「プレビュー: localhost を開く…」だけで、そこもこの範囲しか通さない）。
//!
//! host は**完全一致**で見る。部分一致（`ends_with("localhost")` 等）にすると、WebView2 が artifact を
//! 配信する内部 host `http://necoder-artifact.localhost/` や `localhost.example.com` が通ってしまう。
//! `*.localhost` も入れない — WKWebView ではサブドメインの名前解決が OS 任せでループバックの保証が
//! 無く、artifact の内部 host とも紛れるため。
//!
//! 判定は WHATWG の URL パーサ（`url` crate）を通してから行う。`http://localhost@evil.example/` の
//! ような userinfo の偽装や `http://0x7f.1/` のような表記揺れは、ブラウザと同じ解釈で host が決まる。
//!
//! ここは OS も wry も知らない純粋なロジックだけを置く（どの OS の CI でも検証できるように）。

use std::net::{Ipv4Addr, Ipv6Addr};
use url::{Host, Url};

/// 最上位の文書として Web タブに読み込んでよい URL か（ループバックの http(s)・ポートは任意）。
pub fn is_allowed(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| is_allowed_url(&url))
}

fn is_allowed_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && match url.host() {
            // パーサが小文字化済み（`LOCALHOST` も `localhost` になる）。末尾の `.` 付きは通さない。
            Some(Host::Domain(domain)) => domain == "localhost",
            Some(Host::Ipv4(address)) => address == Ipv4Addr::LOCALHOST,
            Some(Host::Ipv6(address)) => address == Ipv6Addr::LOCALHOST,
            None => false,
        }
}

/// 許可範囲の URL をタブの同一判定に使う正規形へ（`http://localhost:3000` → `http://localhost:3000/`）。
/// 範囲外なら `None`。
pub fn normalize(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    is_allowed_url(&parsed).then(|| parsed.to_string())
}

/// パレットに打たれた文字列を URL にする。受け付けるのは:
///
/// - ポート番号だけ（`3000` / `:3000`）→ `http://localhost:3000/`
/// - scheme を省いた localhost（`localhost:5173/app` / `127.0.0.1:8080` / `[::1]:4000`）→ `http://` を補う
/// - 許可範囲の完全な URL（`https://localhost:8443/`）
///
/// それ以外（外部のホスト・`file:` など）は `None`。
pub fn parse_input(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let port = input.strip_prefix(':').unwrap_or(input);
    if port.chars().all(|character| character.is_ascii_digit()) {
        let port: u16 = port.parse().ok()?;
        if port == 0 {
            return None;
        }
        return normalize(&format!("http://localhost:{port}/"));
    }
    if input.contains("://") {
        normalize(input)
    } else {
        normalize(&format!("http://{input}"))
    }
}

/// Web タブの中での移動をどう扱うか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    /// そのまま Web タブの中で読み込む。
    Allow,
    /// キャンセルして既定のブラウザで開く（外部の http(s)）。
    OpenInBrowser,
    /// キャンセルするだけ（`file:`・`javascript:`・アプリを起こす独自スキームなど）。
    Block,
}

/// 最上位の文書の移動の扱い。iframe の中の移動はここを通さない（`webview_view` の注記）。
pub fn navigation(url: &str) -> Navigation {
    // 空のページと srcdoc は文書の差し替えで、外へ出ていない。
    if url == "about:blank" || url == "about:srcdoc" {
        return Navigation::Allow;
    }
    let Ok(parsed) = Url::parse(url) else {
        return Navigation::Block;
    };
    if is_allowed_url(&parsed) {
        Navigation::Allow
    } else if matches!(parsed.scheme(), "http" | "https") {
        Navigation::OpenInBrowser
    } else {
        Navigation::Block
    }
}

/// 新しい窓（`target="_blank"`・`window.open`）の扱い。Web タブは窓を増やさないので、
/// localhost 宛てでも既定のブラウザへ渡す。http(s) 以外は開かない。
pub fn new_window(url: &str) -> Navigation {
    match Url::parse(url) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => Navigation::OpenInBrowser,
        _ => Navigation::Block,
    }
}

/// タブやツールバーに出す短い表記（`http://localhost:5173/app/` → `localhost:5173/app`）。
/// query と fragment は落とさない（見ている画面の区別に要る）。
pub fn display_label(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_string();
    };
    let host = parsed.host_str().unwrap_or_default();
    let mut label = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let path = parsed.path().trim_end_matches('/');
    label.push_str(path);
    if let Some(query) = parsed.query() {
        label.push('?');
        label.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        label.push('#');
        label.push_str(fragment);
    }
    label
}

/// 既定のブラウザで開く（http(s) だけ・シェルを通さない）。実体は artifact と同じ
/// [`crate::sandbox::open_in_browser`]（artifact の内部 host は渡さない判定もそちらが持つ）。
///
/// 開発用: debug ビルドで `NECODER_WEBVIEW_BROWSER_LOG=<file>` を置くと、ブラウザを起こさずに
/// そのファイルへ URL を追記する（隔離した検証で本人のブラウザにタブを増やさないため）。
pub fn open_in_browser(url: &str) {
    #[cfg(debug_assertions)]
    if let Some(log) = std::env::var_os("NECODER_WEBVIEW_BROWSER_LOG") {
        use std::io::Write as _;
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{url}") {
                    eprintln!("WEBVIEW_BROWSER_LOG: 書けない: {error}");
                }
            }
            Err(error) => eprintln!("WEBVIEW_BROWSER_LOG: 開けない: {error}"),
        }
        return;
    }
    crate::sandbox::open_in_browser(url);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_allowed_on_any_port() {
        for url in [
            "http://localhost:3000/",
            "http://localhost/",
            "https://localhost:8443/app?x=1#top",
            "http://127.0.0.1:5173/",
            "http://[::1]:4000/",
            // パーサが正規化する表記揺れはブラウザと同じ解釈で通る。
            "http://LOCALHOST:3000/",
            "http://0x7f.0.0.1:3000/",
            "http://127.1/",
            "http://[0:0:0:0:0:0:0:1]:8080/",
        ] {
            assert!(is_allowed(url), "{url}");
        }
    }

    #[test]
    fn everything_else_is_refused() {
        for url in [
            "https://example.com/",
            "http://localhost.example.com/",
            "http://example.com/localhost",
            "http://127.0.0.1.nip.io/",
            "http://127.0.0.2/",
            "http://[::ffff:127.0.0.1]/",
            "http://localhost./",
            "http://app.localhost:3000/",
            // userinfo の偽装: host は evil.example。
            "http://localhost@evil.example/",
            "http://127.0.0.1:3000@evil.example/",
            "http://evil.example#@localhost/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ws://localhost:3000/",
            "not a url",
        ] {
            assert!(!is_allowed(url), "{url}");
        }
    }

    /// WebView2 は artifact の配信に `http://necoder-artifact.localhost/` を使う。部分一致で
    /// 判定すると Web タブがここを開けてしまうので、完全一致でしか通さない。
    #[test]
    fn the_artifact_host_is_not_mistaken_for_localhost() {
        let spoof = format!("http://{}.localhost/timer.html", crate::sandbox::SCHEME);
        assert!(!is_allowed(&spoof));
        assert_eq!(navigation(&spoof), Navigation::OpenInBrowser);
        assert_eq!(normalize(&spoof), None);
        assert_eq!(
            parse_input(&format!("{}.localhost", crate::sandbox::SCHEME)),
            None
        );
    }

    #[test]
    fn user_input_becomes_a_loopback_url() {
        assert_eq!(
            parse_input("3000").as_deref(),
            Some("http://localhost:3000/")
        );
        assert_eq!(
            parse_input(" :5173 ").as_deref(),
            Some("http://localhost:5173/")
        );
        assert_eq!(
            parse_input("localhost:5173/app").as_deref(),
            Some("http://localhost:5173/app")
        );
        assert_eq!(
            parse_input("127.0.0.1:8080").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(
            parse_input("[::1]:4000/a?b=1").as_deref(),
            Some("http://[::1]:4000/a?b=1")
        );
        assert_eq!(
            parse_input("https://localhost:8443").as_deref(),
            Some("https://localhost:8443/")
        );
        for refused in [
            "",
            "0",
            "70000",
            "example.com",
            "https://example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "localhost.example.com:3000",
        ] {
            assert_eq!(parse_input(refused), None, "{refused}");
        }
    }

    #[test]
    fn navigation_keeps_the_top_document_on_localhost() {
        assert_eq!(navigation("http://localhost:3000/about"), Navigation::Allow);
        assert_eq!(navigation("about:blank"), Navigation::Allow);
        assert_eq!(
            navigation("https://github.com/login"),
            Navigation::OpenInBrowser
        );
        assert_eq!(navigation("file:///etc/passwd"), Navigation::Block);
        assert_eq!(navigation("vscode://open?x"), Navigation::Block);
        assert_eq!(navigation("javascript:alert(1)"), Navigation::Block);
        assert_eq!(navigation("data:text/html,hi"), Navigation::Block);
    }

    #[test]
    fn new_windows_always_leave_for_the_browser() {
        assert_eq!(
            new_window("http://localhost:3000/other"),
            Navigation::OpenInBrowser
        );
        assert_eq!(
            new_window("https://example.com/"),
            Navigation::OpenInBrowser
        );
        assert_eq!(new_window("file:///etc/passwd"), Navigation::Block);
        assert_eq!(new_window("about:blank"), Navigation::Block);
    }

    #[test]
    fn labels_drop_only_the_scheme_and_trailing_slash() {
        assert_eq!(display_label("http://localhost:5173/"), "localhost:5173");
        assert_eq!(
            display_label("http://localhost:5173/app/"),
            "localhost:5173/app"
        );
        assert_eq!(
            display_label("https://127.0.0.1:8443/a?b=1#c"),
            "127.0.0.1:8443/a?b=1#c"
        );
        assert_eq!(display_label("http://[::1]:4000/"), "[::1]:4000");
    }
}
