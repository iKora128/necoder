//! sandbox — **エージェントが生成した HTML を閉じ込めて見せる**ための部品（`docs/CHAT.md` §3.3 / ROADMAP M16 C3）。
//!
//! プロジェクトの HTML プレビューは「自分のファイルを見る」用途なので `file://` で開く。だが
//! Chat の artifact は**エージェントが書いた JavaScript** で、`file://` で開くとページの中から
//! 手元のファイルを読めてしまう。だから artifact は専用のスキームで配信し、4 つを同時に閉じる
//! （custom protocol を張るだけでは隔離にならない）:
//!
//! 1. **配信できる範囲** — そのチャットの `artifacts/` の中だけ（`..` もシンボリックリンクも外へ出られない）
//! 2. **外部通信** — CSP で `connect-src 'none'`。スクリプトと書体は決まった CDN からだけ
//! 3. **画面遷移** — このスキームの外へは移動させず、http(s) のリンクは既定のブラウザへ渡す
//! 4. **ネイティブ機能** — IPC ハンドラを付けない。wry はページに `window.ipc` の殻を常に置くが、
//!    ネイティブ側の受け口（script message handler）は `ipc_handler` を渡した時だけ登録される＝
//!    `postMessage` しても届く先が無い
//!
//! 実物の WKWebView での確認（2026-09-19・`NECODER_WEBVIEW_PROBE_LOG`）: https への fetch / `file://` /
//! 親フォルダ / エンコードした `..` / XHR / WebSocket / 外部画像はすべて遮断された。
//!
//! ここは OS も wry も知らない純粋なロジックだけを置き、どの OS の CI でも検証できるようにする。

use std::path::{Component, Path, PathBuf};

/// artifact を配信するスキーム。ページの URL は `necoder-artifact://localhost/<相対パス>`
/// （Windows の WebView2 では wry が `http://necoder-artifact.localhost/…` へ読み替える）。
pub const SCHEME: &str = "necoder-artifact";

/// artifact に課す Content-Security-Policy。
///
/// - `connect-src 'none'`: fetch / XHR / WebSocket を全部止める（読んだ物を外へ送れない）
/// - `img-src` / `media-src` は自分自身と `data:` / `blob:` だけ（外部画像はビーコンになる）
/// - スクリプト・スタイル・書体は、単体 HTML が実用になる最小限の CDN だけ許す（チャートや
///   Tailwind を使う artifact が普通にあるため。Chat 用プロンプトにも同じ一覧を書いてある）
/// - `form-action` / `base-uri` / `frame-src` は閉じる
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
script-src 'unsafe-inline' 'unsafe-eval' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; \
style-src 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com https://fonts.googleapis.com; \
font-src data: https://fonts.gstatic.com https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; \
img-src 'self' data: blob:; media-src 'self' data: blob:; \
connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

/// `root` の中の `file` を指す URL。`file` が `root` の外なら `None`。
pub fn url_for(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let mut url = url::Url::parse(&format!("{SCHEME}://localhost/")).ok()?;
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop_if_empty();
        for component in relative.components() {
            match component {
                Component::Normal(name) => {
                    segments.push(&name.to_string_lossy());
                }
                _ => return None,
            }
        }
    }
    Some(url.into())
}

/// 配信の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

/// リクエストの URL パス（`/a/b.html`・パーセントエンコード済み）を `root` の中のファイルへ解決して読む。
/// 外へ出るパスは 404 ではなく **403**（「無い」と「見せない」を混ぜない）。
pub fn serve(root: &Path, request_path: &str) -> Served {
    let Some(path) = resolve(root, request_path) else {
        return Served {
            status: 403,
            content_type: "text/plain; charset=utf-8",
            body: b"forbidden".to_vec(),
        };
    };
    match std::fs::read(&path) {
        Ok(body) => Served {
            status: 200,
            content_type: content_type(&path),
            body,
        },
        Err(_) => Served {
            status: 404,
            content_type: "text/plain; charset=utf-8",
            body: b"not found".to_vec(),
        },
    }
}

/// URL パスを `root` の中の実パスへ。`..`・絶対パス・外を指すシンボリックリンクは `None`。
pub fn resolve(root: &Path, request_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(request_path.split(['?', '#']).next().unwrap_or(""))?;
    let mut path = root.to_path_buf();
    for segment in decoded.split('/').filter(|segment| !segment.is_empty()) {
        // デコード後に `..` や区切り文字が現れる形（`%2e%2e`・`%2f`）もここで落ちる。
        if segment == "." || segment == ".." || segment.contains(['\\', '\0']) {
            return None;
        }
        path.push(segment);
    }
    // 字面が中に収まっていても、途中のシンボリックリンクで外へ出られる。実体で比べる。
    let real_root = std::fs::canonicalize(root).ok()?;
    match std::fs::canonicalize(&path) {
        Ok(real) => real.starts_with(&real_root).then_some(real),
        // まだ無いファイルは 404 にしたいので字面のまま返す（読む時に失敗する）。
        Err(_) => Some(path),
    }
}

/// `%XX` を戻す。不正なエスケープと、UTF-8 として読めない結果は `None`。
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// 拡張子からの Content-Type。知らない物は `application/octet-stream`
/// （ブラウザに中身を推測させない。`X-Content-Type-Options: nosniff` と組で使う）。
pub fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "txt" | "md" | "csv" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// この URL への移動を WebView の中で許すか。許すのは artifact のスキームの中だけ。
pub fn is_internal(url: &str) -> bool {
    url == "about:blank"
        || url.starts_with(&format!("{SCHEME}://localhost/"))
        // WebView2 は custom scheme を `http(s)://<scheme>.localhost/` として見せる。
        || url.starts_with(&format!("http://{SCHEME}.localhost/"))
        || url.starts_with(&format!("https://{SCHEME}.localhost/"))
}

/// 中で開かせなかったリンクを既定のブラウザへ渡してよいか（`http` / `https` だけ。`file:` や
/// `javascript:`、アプリを起こす独自スキームは渡さない）。
pub fn opens_in_browser(url: &str) -> bool {
    !is_internal(url) && (url.starts_with("https://") || url.starts_with("http://"))
}

/// 既定のブラウザで開く。シェルを通さない（URL の中の `&` や `;` を解釈させない）。
pub fn open_in_browser(url: &str) {
    if !opens_in_browser(url) {
        return;
    }
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    if let Err(error) = command.arg(url).spawn() {
        eprintln!("リンクをブラウザで開けない: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        base: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let base = std::env::temp_dir().join(format!(
                "necoder_webview_sandbox_{tag}_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            let root = base.join("chat/artifacts");
            std::fs::create_dir_all(&root).expect("root");
            std::fs::write(root.join("timer.html"), "<p>timer</p>").expect("page");
            std::fs::write(base.join("chat/会話.md"), "# log").expect("export");
            std::fs::write(base.join("secret.txt"), "secret").expect("secret");
            Fixture { base, root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn a_file_inside_the_root_is_served_with_its_type() {
        let fixture = Fixture::new("serve");
        let served = serve(&fixture.root, "/timer.html");
        assert_eq!(served.status, 200);
        assert_eq!(served.content_type, "text/html; charset=utf-8");
        assert_eq!(served.body, b"<p>timer</p>");
        assert_eq!(serve(&fixture.root, "/missing.html").status, 404);
        // クエリとフラグメントは無視する。
        assert_eq!(serve(&fixture.root, "/timer.html?v=2#top").status, 200);
    }

    /// 同じフォルダの会話の書き出しも、その外のファイルも、どう書いても届かない。
    #[test]
    fn nothing_outside_the_root_is_reachable() {
        let fixture = Fixture::new("escape");
        for path in [
            "/../会話.md",
            "/../../secret.txt",
            "/%2e%2e/%2e%2e/secret.txt",
            "/..%2f..%2fsecret.txt",
            "/a/../../secret.txt",
            "/%00",
            "/bad%zzescape",
        ] {
            assert_eq!(serve(&fixture.root, path).status, 403, "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_does_not_lead_out_of_the_root() {
        let fixture = Fixture::new("symlink");
        std::os::unix::fs::symlink(
            fixture.base.join("secret.txt"),
            fixture.root.join("leak.txt"),
        )
        .expect("symlink");
        assert_eq!(serve(&fixture.root, "/leak.txt").status, 403);
    }

    #[test]
    fn urls_are_built_relative_to_the_root_and_escaped() {
        let root = Path::new("/docs/necoder/2026-09-18 タイマー/artifacts");
        assert_eq!(
            url_for(root, &root.join("timer.html")).as_deref(),
            Some("necoder-artifact://localhost/timer.html")
        );
        assert_eq!(
            url_for(root, &root.join("図 1.html")).as_deref(),
            Some("necoder-artifact://localhost/%E5%9B%B3%201.html")
        );
        assert_eq!(url_for(root, Path::new("/etc/passwd")), None);
    }

    #[test]
    fn the_built_url_resolves_back_to_the_same_file() {
        let fixture = Fixture::new("roundtrip");
        let file = fixture.root.join("図 1.html");
        std::fs::write(&file, "x").expect("file");
        let url = url_for(&fixture.root, &file).expect("url");
        let path = url
            .strip_prefix("necoder-artifact://localhost")
            .expect("prefix");
        assert_eq!(serve(&fixture.root, path).status, 200);
    }

    #[test]
    fn only_the_artifact_scheme_navigates_and_only_web_links_leave() {
        assert!(is_internal("necoder-artifact://localhost/timer.html"));
        assert!(is_internal("http://necoder-artifact.localhost/timer.html"));
        assert!(is_internal("about:blank"));
        for url in [
            "https://example.com/",
            "file:///etc/passwd",
            "necoder-artifact://evil.example/timer.html",
            "javascript:alert(1)",
        ] {
            assert!(!is_internal(url), "{url}");
        }
        assert!(opens_in_browser("https://example.com/a?b=1&c=2"));
        assert!(!opens_in_browser("file:///etc/passwd"));
        assert!(!opens_in_browser("javascript:alert(1)"));
        assert!(!opens_in_browser("vscode://open?x"));
    }

    #[test]
    fn the_policy_closes_the_network_but_keeps_inline_pages_working() {
        assert!(CONTENT_SECURITY_POLICY.contains("connect-src 'none'"));
        assert!(CONTENT_SECURITY_POLICY.contains("default-src 'none'"));
        assert!(CONTENT_SECURITY_POLICY.contains("script-src 'unsafe-inline'"));
        assert!(!CONTENT_SECURITY_POLICY.contains("img-src *"));
    }
}
