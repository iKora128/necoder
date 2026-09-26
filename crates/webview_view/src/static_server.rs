//! static_server — **内蔵の配信**: HTML ファイルを Web タブで開くための、std だけの小さな静的配信。
//!
//! Web タブ（と Design Mode）は `http(s)://localhost` のページしか開かない。開発サーバ（`npm run dev`）の
//! 無いただの HTML でも同じように見て選べるよう、necoder の中で読み取り専用の HTTP を 1 つ立てる。
//!
//! - bind は `127.0.0.1` のランダムなポートだけ。URL は `http://127.0.0.1:<port>/<token>/<相対パス>` で、
//!   token は配るフォルダごとに作る推測できない 128 bit（[`random_token`]）。token が無い・知らない物は 404
//! - **Host ヘッダが `127.0.0.1:<port>` と完全一致しなければ 403**（DNS rebinding: 攻撃者のドメインを
//!   127.0.0.1 へ向けても、ブラウザが送る Host はそのドメインのまま）
//! - GET / HEAD だけ（他は 405）。配るのは登録したフォルダの中だけ: `..`・`%2e%2e`・シンボリックリンクで
//!   外へ出るパスは 403（実体を canonicalize して根の下か確かめる＝[`crate::sandbox::resolve`] と同じ書き方）。
//!   `.` で始まる名前（`.env`・`.git`）も配らない。フォルダの一覧は出さない（`index.html` だけ）
//! - `Cache-Control: no-store`・`X-Content-Type-Options: nosniff`。CORS のヘッダは付けない
//!   （他の origin のページからは中身を読めない）
//! - 必要な時だけ動く: 最初の [`StaticMount`] で起き、最後の 1 つが drop されたら止まる。止める時は
//!   accept のスレッドも接続のスレッドも join する（スレッドを残さない・常駐しない）
//! - ページのルート相対の参照（`/assets/app.js`）は、token 付きのページから来た時（Referer）だけ同じ token の
//!   下へ転送する（ビルド済みの `dist/index.html` がそのまま動く）。token を知らない相手には 404 のまま
//!
//! ファイルの変更で再読み込みするのは Workspace 側（既存のファイル監視 → Web タブの reload）。ページに
//! live reload のスクリプトは差し込まない（配る中身はディスクのまま）。

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// 1 本の接続で読み書きを待つ上限（止まった相手が接続のスレッドを握り続けないように）。
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// 同時に受ける接続の上限（WebView は 1 つの host に 6 本ほど張る）。超えた分は 503 で閉じる。
const MAX_CONNECTIONS: usize = 32;
/// リクエストの頭（リクエスト行とヘッダ）の上限。
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// 使う Web タブがある間だけ動く、アプリで 1 つの口（GPUI の Global として持つ）。
/// 自分では配信を持ち続けない — 生きている [`StaticMount`] が [`StaticServer`] を握り、最後の 1 つが
/// drop されると止まる。次の [`Self::mount`] でまた起きる（ポートと token は変わる）。
#[derive(Default)]
pub struct StaticHosting {
    server: Weak<StaticServer>,
}

impl gpui::Global for StaticHosting {}

impl StaticHosting {
    /// `root`（フォルダ）を配る口を作る。配信が止まっていれば起こす。
    pub fn mount(&mut self, root: &Path) -> io::Result<StaticMount> {
        let server = match self.server.upgrade() {
            Some(server) => server,
            None => {
                let server = StaticServer::start()?;
                self.server = Arc::downgrade(&server);
                server
            }
        };
        server.mount(root)
    }

    /// 配信が動いているか（使う Web タブが 1 つ以上あるか）。
    pub fn is_running(&self) -> bool {
        self.server.strong_count() > 0
    }
}

/// 配信の本体（1 ポート・複数のフォルダ）。[`StaticMount`] が握っている間だけ生きる。
pub struct StaticServer {
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

/// accept と接続のスレッドが見る状態。
struct Shared {
    port: u16,
    /// token → 配るフォルダ（canonicalize 済み）。
    mounts: Mutex<HashMap<String, PathBuf>>,
    stopping: AtomicBool,
    /// 開いている接続。止める時に shutdown して、接続のスレッドをすぐ終わらせてから join する。
    connections: Mutex<Vec<Connection>>,
    next_connection: AtomicU64,
}

struct Connection {
    id: u64,
    stream: TcpStream,
    thread: Option<JoinHandle<()>>,
}

/// Mutex が毒されていても中身は使える（持ち主のスレッドが panic しただけで、表は壊れていない）。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl StaticServer {
    /// `127.0.0.1` のランダムなポートで配信を始める。
    pub fn start() -> io::Result<Arc<StaticServer>> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            port,
            mounts: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            connections: Mutex::new(Vec::new()),
            next_connection: AtomicU64::new(0),
        });
        let accept_shared = shared.clone();
        let accept = std::thread::Builder::new()
            .name("necoder-static-server".to_string())
            .spawn(move || accept_loop(listener, accept_shared))?;
        Ok(Arc::new(StaticServer {
            shared,
            accept: Some(accept),
        }))
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// `root`（フォルダ）を新しい token の下に配る。token は mount ごとに作る（同じフォルダでも別）。
    pub fn mount(self: &Arc<Self>, root: &Path) -> io::Result<StaticMount> {
        let root = std::fs::canonicalize(root)?;
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a folder: {}", root.display()),
            ));
        }
        let token = random_token()?;
        lock(&self.shared.mounts).insert(token.clone(), root.clone());
        Ok(StaticMount {
            server: self.clone(),
            token,
            root,
        })
    }
}

impl Drop for StaticServer {
    /// 止める: accept を起こして抜けさせ（listener が閉じる＝ポートが閉じる）、開いている接続を
    /// shutdown してから、全部のスレッドを join する。
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            // accept は blocking なので、自分へ 1 本繋いで起こす。繋げない時に join すると戻らないので、
            // その時だけは待たずに手放す（次の接続が来た時に抜ける）。
            let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.shared.port));
            match TcpStream::connect_timeout(&address, Duration::from_secs(1)) {
                Ok(_wake) => {
                    if accept.join().is_err() {
                        eprintln!("static_server: accept のスレッドが panic していた");
                    }
                }
                Err(error) => {
                    eprintln!("static_server: accept を起こせない（待たずに手放す）: {error}");
                }
            }
        }
        let connections: Vec<Connection> = lock(&self.shared.connections).drain(..).collect();
        for connection in &connections {
            // 相手が既に閉じていれば失敗するが、止める目的はもう果たされている。
            connection.stream.shutdown(Shutdown::Both).ok();
        }
        for mut connection in connections {
            if let Some(thread) = connection.thread.take() {
                if thread.join().is_err() {
                    eprintln!(
                        "static_server: 接続 {} のスレッドが panic していた",
                        connection.id
                    );
                }
            }
        }
    }
}

/// 1 つのフォルダを配る口。drop すると token を外す（最後の 1 つなら配信も止まる）。
pub struct StaticMount {
    server: Arc<StaticServer>,
    token: String,
    root: PathBuf,
}

impl StaticMount {
    /// 配るフォルダ（canonicalize 済み）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 配信の根の URL（`http://127.0.0.1:<port>/<token>/`）。
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/", self.server.port(), self.token)
    }

    /// `file`（このフォルダの中）を指す URL。外・無いファイルなら `None`。
    pub fn url_for(&self, file: &Path) -> Option<String> {
        let file = std::fs::canonicalize(file).ok()?;
        let relative = file.strip_prefix(&self.root).ok()?;
        let mut url = url::Url::parse(&self.base_url()).ok()?;
        {
            let mut segments = url.path_segments_mut().ok()?;
            segments.pop_if_empty();
            for component in relative.components() {
                match component {
                    std::path::Component::Normal(name) => {
                        segments.push(&name.to_string_lossy());
                    }
                    _ => return None,
                }
            }
        }
        Some(url.into())
    }

    /// URL がこの口の物なら、指しているファイルのフォルダからの相対パス（`about.html` / `docs/a.html`）。
    /// 表示に使う（ツールバーに token 付きの URL を出さない）。
    pub fn relative_path_of(&self, url: &str) -> Option<String> {
        let rest = url.strip_prefix(&self.base_url())?;
        let path = rest.split(['?', '#']).next().unwrap_or_default();
        Some(percent_decode(path).unwrap_or_else(|| path.to_string()))
    }
}

impl Drop for StaticMount {
    fn drop(&mut self) {
        lock(&self.server.shared.mounts).remove(&self.token);
    }
}

/// ファイルの `file://` URL（内蔵の配信で開いた Web タブの鍵に使う: ポートと token は起動ごとに
/// 変わるので、鍵にはファイルを使う）。相対パスは `None`。
pub fn file_url(file: &Path) -> Option<String> {
    url::Url::from_file_path(file).ok().map(String::from)
}

/// `file://` URL が指すファイル（[`file_url`] の逆）。
pub fn file_of_url(text: &str) -> Option<PathBuf> {
    if !text.starts_with("file://") {
        return None;
    }
    url::Url::parse(text).ok()?.to_file_path().ok()
}

/// 推測できない 128 bit の token（16 進 32 桁）。
///
/// unix は `/dev/urandom`（host crate の askpass の token と同じ）。それ以外は std の `RandomState`
/// から作る — その鍵は OS の乱数で種をまいた SipHash の鍵で、外からは出力を予測できない。
fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    #[cfg(unix)]
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    #[cfg(not(unix))]
    {
        use std::hash::{BuildHasher, Hasher};
        for (index, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_usize(index);
            chunk.copy_from_slice(&hasher.finish().to_le_bytes());
        }
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stopping.load(Ordering::SeqCst) {
            break;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                // 手元の資源切れ（開けるファイル数など）。空回りしないよう少し待つ。
                eprintln!("static_server: 接続を受けられない: {error}");
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        spawn_connection(stream, &shared);
    }
}

/// 接続 1 本につきスレッド 1 本（`Connection: close`・リクエスト 1 つで閉じる）。
fn spawn_connection(stream: TcpStream, shared: &Arc<Shared>) {
    let mut connections = lock(&shared.connections);
    connections.retain(|connection| {
        connection
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    });
    if connections.len() >= MAX_CONNECTIONS {
        drop(connections);
        let mut stream = stream;
        write_status(&mut stream, 503, "Service Unavailable", false).ok();
        return;
    }
    let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    let kept = match stream.try_clone() {
        Ok(kept) => kept,
        Err(error) => {
            eprintln!("static_server: 接続を控えられない（受けずに閉じる）: {error}");
            return;
        }
    };
    let thread_shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("necoder-static-connection".to_string())
        .spawn(move || {
            let mut stream = stream;
            if let Err(error) = handle_connection(&mut stream, &thread_shared) {
                // 相手が途中で閉じた等。配信は続ける。
                if cfg!(debug_assertions) {
                    eprintln!("static_server: 接続 {id}: {error}");
                }
            }
            stream.shutdown(Shutdown::Both).ok();
        });
    match spawned {
        Ok(thread) => connections.push(Connection {
            id,
            stream: kept,
            thread: Some(thread),
        }),
        Err(error) => eprintln!("static_server: 接続のスレッドを作れない: {error}"),
    }
}

/// リクエストの頭から読んだ物（使う欄だけ）。
#[derive(Debug, Default, PartialEq, Eq)]
struct Request {
    method: String,
    target: String,
    host: Option<String>,
    /// Host ヘッダが 2 つ以上あった（食い違いで検査をすり抜けさせない）。
    duplicate_host: bool,
    referer: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<Option<Request>> {
    let mut reader = BufReader::new(Read::by_ref(stream).take(MAX_HEAD_BYTES as u64));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(None);
    }
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Ok(None);
    };
    if !version.starts_with("HTTP/1.") {
        return Ok(None);
    }
    let mut request = Request {
        method: method.to_string(),
        target: target.to_string(),
        ..Request::default()
    };
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            // 頭が上限を超えた・途中で切れた。
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => {
                if request.host.is_some() {
                    request.duplicate_host = true;
                }
                request.host = Some(value);
            }
            "referer" => request.referer = Some(value),
            _ => {}
        }
    }
    Ok(Some(request))
}

/// 返す物。
#[derive(Debug, PartialEq, Eq)]
enum Reply {
    File(PathBuf),
    Redirect(String),
    Status(u16, &'static str),
}

fn handle_connection(stream: &mut TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let Some(request) = read_request(stream)? else {
        return write_status(stream, 400, "Bad Request", false);
    };
    let head_only = request.method == "HEAD";
    let mounts = lock(&shared.mounts).clone();
    match route(shared.port, &mounts, &request) {
        Reply::File(path) => write_file(stream, &path, head_only),
        Reply::Redirect(location) => write_redirect(stream, &location),
        Reply::Status(code, reason) => write_status(stream, code, reason, head_only),
    }
}

/// リクエストをどう返すか（ソケットを使わない純粋な判断・テストの主対象）。
fn route(port: u16, mounts: &HashMap<String, PathBuf>, request: &Request) -> Reply {
    if request.method != "GET" && request.method != "HEAD" {
        return Reply::Status(405, "Method Not Allowed");
    }
    // DNS rebinding: 名前を 127.0.0.1 へ向けても、Host はその名前のまま届く。完全一致だけ通す。
    let expected_host = format!("127.0.0.1:{port}");
    if request.duplicate_host || request.host.as_deref() != Some(expected_host.as_str()) {
        return Reply::Status(403, "Forbidden");
    }
    // origin-form（`/…`）だけ。`http://…` の absolute-form や `*` は受けない。
    if !request.target.starts_with('/') {
        return Reply::Status(400, "Bad Request");
    }
    let (path, query) = match request.target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (request.target.as_str(), None),
    };
    let without_slash = &path[1..];
    let (token, rest) = match without_slash.split_once('/') {
        Some((token, rest)) => (token, Some(rest)),
        None => (without_slash, None),
    };
    let Some(root) = mounts.get(token) else {
        // ルート相対の参照（`/assets/app.js`）: token 付きのページから来た時だけ、その token の下へ。
        return match referer_token(port, mounts, request.referer.as_deref()) {
            Some(token) => Reply::Redirect(with_query(&format!("/{token}{path}"), query)),
            None => Reply::Status(404, "Not Found"),
        };
    };
    // `/<token>` → `/<token>/`（相対の参照がフォルダの中を指すように）。
    let Some(rest) = rest else {
        return Reply::Redirect(with_query(&format!("/{token}/"), query));
    };
    match resolve(root, rest) {
        Resolved::File(file) => Reply::File(file),
        Resolved::Directory => {
            if rest.is_empty() || rest.ends_with('/') {
                match resolve(root, &format!("{rest}index.html")) {
                    Resolved::File(file) => Reply::File(file),
                    Resolved::Forbidden => Reply::Status(403, "Forbidden"),
                    Resolved::Directory | Resolved::NotFound => Reply::Status(404, "Not Found"),
                    Resolved::BadRequest => Reply::Status(400, "Bad Request"),
                }
            } else {
                Reply::Redirect(with_query(&format!("{path}/"), query))
            }
        }
        Resolved::Forbidden => Reply::Status(403, "Forbidden"),
        Resolved::NotFound => Reply::Status(404, "Not Found"),
        Resolved::BadRequest => Reply::Status(400, "Bad Request"),
    }
}

fn with_query(path: &str, query: Option<&str>) -> String {
    match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    }
}

/// Referer が自分の配るページ（`http://127.0.0.1:<port>/<token>/…`）なら、その token。
fn referer_token<'a>(
    port: u16,
    mounts: &'a HashMap<String, PathBuf>,
    referer: Option<&str>,
) -> Option<&'a str> {
    let rest = referer?.strip_prefix(&format!("http://127.0.0.1:{port}/"))?;
    let token = rest.split(['/', '?', '#']).next()?;
    mounts.get_key_value(token).map(|(token, _)| token.as_str())
}

#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    File(PathBuf),
    Directory,
    /// 根の外へ出る・隠しファイル（`.` で始まる名前）・読めない形の名前。
    Forbidden,
    NotFound,
    /// パーセントエンコードが壊れている。
    BadRequest,
}

/// URL のパス（token の後・パーセントエンコード済み）を `root` の中の実パスへ。
fn resolve(root: &Path, encoded: &str) -> Resolved {
    let Some(decoded) = percent_decode(encoded) else {
        return Resolved::BadRequest;
    };
    let mut path = root.to_path_buf();
    for segment in decoded.split('/').filter(|segment| !segment.is_empty()) {
        // デコード後に `..` や区切り文字が現れる形（`%2e%2e`・`%2f`・`%5c`）もここで落ちる。
        // `.` で始まる名前（`.env`・`.git`）は配らない。`:` は Windows のドライブ / ADS。
        if segment.starts_with('.') || segment.contains(['\\', '\0', ':']) {
            return Resolved::Forbidden;
        }
        path.push(segment);
    }
    // 字面が中に収まっていても、途中のシンボリックリンクで外へ出られる。実体で比べる。
    let real = match std::fs::canonicalize(&path) {
        Ok(real) => real,
        Err(_) => return Resolved::NotFound,
    };
    if !real.starts_with(root) {
        return Resolved::Forbidden;
    }
    if real.is_dir() {
        Resolved::Directory
    } else {
        Resolved::File(real)
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
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "txt" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// 全部の応答に付けるヘッダ。
fn common_headers(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n"
    )
}

fn write_status(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    head_only: bool,
) -> io::Result<()> {
    let body = format!("{code} {reason}\n");
    write!(stream, "HTTP/1.1 {code} {reason}\r\n")?;
    if code == 405 {
        write!(stream, "Allow: GET, HEAD\r\n")?;
    }
    write!(
        stream,
        "Content-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    common_headers(stream)?;
    write!(stream, "\r\n")?;
    if !head_only {
        stream.write_all(body.as_bytes())?;
    }
    stream.flush()
}

/// 転送（本文は無いので GET / HEAD で同じ）。
fn write_redirect(stream: &mut TcpStream, location: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n"
    )?;
    common_headers(stream)?;
    write!(stream, "\r\n")?;
    stream.flush()
}

fn write_file(stream: &mut TcpStream, path: &Path, head_only: bool) -> io::Result<()> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return write_status(stream, 404, "Not Found", head_only),
    };
    let length = file.metadata()?.len();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {length}\r\n",
        content_type(path)
    )?;
    common_headers(stream)?;
    write!(stream, "\r\n")?;
    if !head_only {
        // 読んでいる間に伸びても、名乗った長さだけ送る。
        io::copy(&mut Read::by_ref(&mut file).take(length), stream)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        base: PathBuf,
        site: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let base = std::env::temp_dir().join(format!(
                "necoder_static_server_{tag}_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            let site = base.join("site");
            std::fs::create_dir_all(site.join("docs")).expect("フォルダを作れる");
            std::fs::create_dir_all(site.join(".git")).expect("フォルダを作れる");
            std::fs::create_dir_all(site.join("assets")).expect("フォルダを作れる");
            std::fs::write(site.join("index.html"), "<h1>index</h1>").expect("書ける");
            std::fs::write(site.join("docs/index.html"), "<h1>docs</h1>").expect("書ける");
            std::fs::write(site.join("図 1.html"), "<h1>図</h1>").expect("書ける");
            std::fs::write(site.join("assets/app.js"), "console.log(1)").expect("書ける");
            std::fs::write(site.join(".env"), "TOKEN=secret").expect("書ける");
            std::fs::write(site.join(".git/config"), "[core]").expect("書ける");
            std::fs::write(base.join("secret.txt"), "secret").expect("書ける");
            Fixture { base, site }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.base) {
                eprintln!("一時フォルダを消せない: {error}");
            }
        }
    }

    /// 生の HTTP/1.1 で 1 回聞いて、(状態コード, ヘッダ, 本文) を返す。
    fn fetch(
        port: u16,
        method: &str,
        target: &str,
        host: Option<&str>,
        extra: &str,
    ) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("繋がる");
        let host_line = host
            .map(|host| format!("Host: {host}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "{method} {target} HTTP/1.1\r\n{host_line}{extra}\r\n"
        )
        .expect("送れる");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("読める");
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("頭と本文の区切りがある");
        let head = String::from_utf8_lossy(&response[..split]).to_string();
        let body = response[split + 4..].to_vec();
        let code = head
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .expect("状態コード");
        (code, head, body)
    }

    fn get(port: u16, target: &str) -> (u16, String, Vec<u8>) {
        fetch(port, "GET", target, Some(&format!("127.0.0.1:{port}")), "")
    }

    fn token_of(mount: &StaticMount) -> String {
        mount
            .base_url()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .expect("token")
            .to_string()
    }

    #[test]
    fn files_in_the_folder_are_served_with_their_type_and_no_cache() {
        let fixture = Fixture::new("serve");
        let server = StaticServer::start().expect("始まる");
        let mount = server.mount(&fixture.site).expect("配れる");
        let port = server.port();
        let token = token_of(&mount);

        let url = mount
            .url_for(&fixture.site.join("index.html"))
            .expect("中のファイル");
        assert_eq!(url, format!("http://127.0.0.1:{port}/{token}/index.html"));
        let (code, head, body) = get(port, &format!("/{token}/index.html"));
        assert_eq!(code, 200);
        assert_eq!(body, b"<h1>index</h1>");
        assert!(
            head.contains("Content-Type: text/html; charset=utf-8"),
            "{head}"
        );
        assert!(head.contains("Cache-Control: no-store"), "{head}");
        assert!(head.contains("X-Content-Type-Options: nosniff"), "{head}");
        assert!(
            !head.contains("Access-Control"),
            "他の origin には読ませない: {head}"
        );

        // エスケープした名前も往復する。
        let escaped = mount.url_for(&fixture.site.join("図 1.html")).expect("url");
        let path = escaped
            .strip_prefix(&format!("http://127.0.0.1:{port}"))
            .expect("同じ配信");
        assert_eq!(get(port, path).0, 200);
        assert_eq!(
            mount.relative_path_of(&escaped).as_deref(),
            Some("図 1.html")
        );
        // フォルダは index.html。末尾の `/` が無ければ付け直す。
        assert_eq!(get(port, &format!("/{token}/")).2, b"<h1>index</h1>");
        assert_eq!(get(port, &format!("/{token}/docs/")).2, b"<h1>docs</h1>");
        let (code, head, _) = get(port, &format!("/{token}/docs"));
        assert_eq!(code, 302);
        assert!(
            head.contains(&format!("Location: /{token}/docs/")),
            "{head}"
        );
        let (code, head, _) = get(port, &format!("/{token}"));
        assert_eq!(code, 302);
        assert!(head.contains(&format!("Location: /{token}/")), "{head}");
        // HEAD は本文を送らない。GET / HEAD 以外は断る。
        let (code, head, body) = fetch(
            port,
            "HEAD",
            &format!("/{token}/index.html"),
            Some(&format!("127.0.0.1:{port}")),
            "",
        );
        assert_eq!((code, body.len()), (200, 0));
        assert!(head.contains("Content-Length: 14"), "{head}");
        let (code, head, _) = fetch(
            port,
            "POST",
            &format!("/{token}/index.html"),
            Some(&format!("127.0.0.1:{port}")),
            "Content-Length: 0\r\n",
        );
        assert_eq!(code, 405);
        assert!(head.contains("Allow: GET, HEAD"), "{head}");
    }

    #[test]
    fn content_types_follow_the_extension() {
        for (name, expected) in [
            ("a.html", "text/html; charset=utf-8"),
            ("a.HTM", "text/html; charset=utf-8"),
            ("a.css", "text/css; charset=utf-8"),
            ("a.js", "text/javascript; charset=utf-8"),
            ("a.mjs", "text/javascript; charset=utf-8"),
            ("a.json", "application/json; charset=utf-8"),
            ("a.svg", "image/svg+xml"),
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.webp", "image/webp"),
            ("a.woff2", "font/woff2"),
            ("a.wasm", "application/wasm"),
            ("a.ico", "image/x-icon"),
            ("a.mp4", "video/mp4"),
            ("a", "application/octet-stream"),
            ("a.unknown", "application/octet-stream"),
        ] {
            assert_eq!(content_type(Path::new(name)), expected, "{name}");
        }
    }

    /// 外へ出るパス（`..`・エンコードした `..`・区切りの混入）と隠しファイルは 403。
    #[test]
    fn nothing_outside_the_folder_or_hidden_is_reachable() {
        let fixture = Fixture::new("escape");
        let server = StaticServer::start().expect("始まる");
        let mount = server.mount(&fixture.site).expect("配れる");
        let port = server.port();
        let token = token_of(&mount);
        for path in [
            "../secret.txt",
            "%2e%2e/secret.txt",
            "%2E%2E/secret.txt",
            "..%2fsecret.txt",
            "docs/%2e%2e/%2e%2e/secret.txt",
            "docs%2f..%2f..%2fsecret.txt",
            "..%5csecret.txt",
            ".env",
            "%2eenv",
            ".git/config",
            "docs/../.env",
        ] {
            let (code, _, body) = get(port, &format!("/{token}/{path}"));
            assert_eq!(code, 403, "{path}");
            assert!(
                !body.starts_with(b"secret") && !body.starts_with(b"TOKEN"),
                "{path}"
            );
        }
        // 壊れたエスケープは 400、無いファイルは 404。
        assert_eq!(get(port, &format!("/{token}/bad%zz")).0, 400);
        assert_eq!(get(port, &format!("/{token}/missing.html")).0, 404);
        // absolute-form は受けない。
        assert_eq!(
            get(port, &format!("http://127.0.0.1:{port}/{token}/index.html")).0,
            400
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_does_not_lead_out_of_the_folder() {
        let fixture = Fixture::new("symlink");
        std::os::unix::fs::symlink(
            fixture.base.join("secret.txt"),
            fixture.site.join("leak.txt"),
        )
        .expect("symlink");
        std::os::unix::fs::symlink(&fixture.base, fixture.site.join("up")).expect("symlink");
        std::os::unix::fs::symlink(
            fixture.site.join("index.html"),
            fixture.site.join("home.html"),
        )
        .expect("symlink");
        let server = StaticServer::start().expect("始まる");
        let mount = server.mount(&fixture.site).expect("配れる");
        let port = server.port();
        let token = token_of(&mount);
        assert_eq!(get(port, &format!("/{token}/leak.txt")).0, 403);
        assert_eq!(get(port, &format!("/{token}/up/secret.txt")).0, 403);
        // 中を指すリンクは配る。
        assert_eq!(
            get(port, &format!("/{token}/home.html")).2,
            b"<h1>index</h1>"
        );
    }

    /// Host が `127.0.0.1:<port>` と完全一致しなければ断る（DNS rebinding）。
    #[test]
    fn requests_for_another_host_are_refused() {
        let fixture = Fixture::new("host");
        let server = StaticServer::start().expect("始まる");
        let mount = server.mount(&fixture.site).expect("配れる");
        let port = server.port();
        let target = format!("/{}/index.html", token_of(&mount));
        for host in [
            Some("evil.example".to_string()),
            Some(format!("evil.example:{port}")),
            Some(format!("localhost:{port}")),
            Some("127.0.0.1".to_string()),
            Some(format!("127.0.0.1:{}", port.wrapping_add(1))),
            Some(format!("127.0.0.1:{port}.evil.example")),
            None,
        ] {
            let (code, _, _) = fetch(port, "GET", &target, host.as_deref(), "");
            assert_eq!(code, 403, "{host:?}");
        }
        // Host を 2 つ送って検査の食い違いを狙う物も断る。
        let (code, _, _) = fetch(
            port,
            "GET",
            &target,
            Some(&format!("127.0.0.1:{port}")),
            "Host: evil.example\r\n",
        );
        assert_eq!(code, 403);
    }

    /// token が無い・知らない物は 404。token 付きのページから来たルート相対の参照だけ転送する。
    #[test]
    fn requests_without_a_known_token_find_nothing() {
        let fixture = Fixture::new("token");
        let server = StaticServer::start().expect("始まる");
        let mount = server.mount(&fixture.site).expect("配れる");
        let port = server.port();
        let token = token_of(&mount);
        for target in [
            "/",
            "/index.html",
            "/0123456789abcdef0123456789abcdef/index.html",
        ] {
            assert_eq!(get(port, target).0, 404, "{target}");
        }
        let from_page = format!("Referer: http://127.0.0.1:{port}/{token}/index.html\r\n");
        let (code, head, _) = fetch(
            port,
            "GET",
            "/assets/app.js?v=2",
            Some(&format!("127.0.0.1:{port}")),
            &from_page,
        );
        assert_eq!(code, 302);
        assert!(
            head.contains(&format!("Location: /{token}/assets/app.js?v=2")),
            "{head}"
        );
        // 他所のページ・知らない token の Referer では転送しない。
        for referer in [
            "Referer: https://evil.example/index.html\r\n".to_string(),
            format!("Referer: http://127.0.0.1:{port}/0123456789abcdef/index.html\r\n"),
            format!("Referer: http://localhost:{port}/{token}/index.html\r\n"),
        ] {
            let (code, _, _) = fetch(
                port,
                "GET",
                "/assets/app.js",
                Some(&format!("127.0.0.1:{port}")),
                &referer,
            );
            assert_eq!(code, 404, "{referer}");
        }
    }

    /// 口を外した token は届かなくなり、最後の口を外すと配信が止まってポートが閉じる。
    #[test]
    fn dropping_the_last_mount_stops_the_server_and_closes_the_port() {
        let fixture = Fixture::new("stop");
        let mut hosting = StaticHosting::default();
        assert!(!hosting.is_running(), "使うまで起きない");
        let first = hosting.mount(&fixture.site).expect("配れる");
        let second = hosting.mount(&fixture.site.join("docs")).expect("配れる");
        assert!(hosting.is_running());
        let port = first.server.port();
        assert_eq!(second.server.port(), port, "1 つの配信を共有する");
        let first_token = token_of(&first);
        let second_token = token_of(&second);
        assert_ne!(first_token, second_token, "口ごとに token が違う");
        assert_eq!(first_token.len(), 32);

        drop(first);
        assert_eq!(get(port, &format!("/{first_token}/index.html")).0, 404);
        assert_eq!(get(port, &format!("/{second_token}/index.html")).0, 200);

        // 開いたままの接続があっても止まる（待たせたまま残さない）。
        let idle = TcpStream::connect(("127.0.0.1", port)).expect("繋がる");
        drop(second);
        assert!(!hosting.is_running());
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "止めたらポートが閉じる"
        );
        drop(idle);

        // また使えば別のポートで起きる。
        let again = hosting.mount(&fixture.site).expect("また配れる");
        assert!(hosting.is_running());
        assert_eq!(
            get(again.server.port(), &format!("/{}/", token_of(&again))).0,
            200
        );
    }

    #[test]
    fn only_folders_can_be_mounted_and_urls_stay_inside() {
        let fixture = Fixture::new("mount");
        let server = StaticServer::start().expect("始まる");
        assert!(server.mount(&fixture.site.join("index.html")).is_err());
        assert!(server.mount(&fixture.base.join("missing")).is_err());
        let mount = server.mount(&fixture.site).expect("配れる");
        assert_eq!(mount.url_for(&fixture.base.join("secret.txt")), None);
        assert_eq!(mount.url_for(&fixture.site.join("missing.html")), None);
        assert_eq!(
            mount.relative_path_of("http://example.com/index.html"),
            None
        );
    }
}
