//! lsp — rust-analyzer との最小 LSP クライアント（M7）。GPUI 非依存。
//!
//! 移植根拠は `docs/research/porting-git-terminal-lsp.md`。JSON-RPC 封筒は自前、型は必要分だけ手書き
//! （lsp-types のバージョン差異を避ける）。transport は `std::process` + **読取スレッド**（blocking）+
//! `futures` channel で上位（GPUI 前景）へ橋渡しする（acp_client / terminal と同じ「背景→channel→pump」）。
//!
//! - **診断**（`textDocument/publishDiagnostics`）は通知＝capability 不要で受信。gutter は**行番号だけ**で
//!   置けるので UTF-16 変換は不要（範囲内の character は使わない）。
//! - **補完/hover/定義**は要求で、位置は UTF-16 code unit（[`Position`]）。上位が byte↔UTF-16 を変換して渡す。

use anyhow::{Context as _, Result};
#[cfg(test)]
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use host::{CommandSpec, Host, HostProcess, LocalHost};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

const JSONRPC: &str = "2.0";

/// Executable, arguments and protocol language id for one language server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageServerSpec {
    pub language_id: &'static str,
    pub command: PathBuf,
    pub args: Vec<&'static str>,
}

/// Resolve a language server from a file extension. Remote hosts resolve executable names
/// remotely; local hosts also search GUI-launch fallback directories.
pub fn language_server_for(path: &Path, remote: bool) -> Option<LanguageServerSpec> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let server = |language_id, command, args: &[&'static str]| LanguageServerSpec {
        language_id,
        command,
        args: args.to_vec(),
    };
    let executable = |binary: &str| {
        if remote {
            Some(PathBuf::from(binary))
        } else {
            find_executable(binary)
        }
    };
    match extension.as_str() {
        "rs" => {
            if remote {
                Some(server("rust", PathBuf::from("rust-analyzer"), &[]))
            } else {
                rust_analyzer_path().map(|command| server("rust", command, &[]))
            }
        }
        "ts" | "tsx" | "mts" | "cts" => executable("typescript-language-server")
            .map(|command| server("typescript", command, &["--stdio"])),
        "js" | "jsx" | "mjs" | "cjs" => executable("typescript-language-server")
            .map(|command| server("javascript", command, &["--stdio"])),
        "py" | "pyi" => executable("pyright-langserver")
            .map(|command| server("python", command, &["--stdio"]))
            .or_else(|| executable("pylsp").map(|command| server("python", command, &[]))),
        "go" => executable("gopls").map(|command| server("go", command, &[])),
        "c" | "h" => executable("clangd").map(|command| server("c", command, &[])),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => {
            executable("clangd").map(|command| server("cpp", command, &[]))
        }
        "lua" => executable("lua-language-server").map(|command| server("lua", command, &[])),
        "zig" => executable("zls").map(|command| server("zig", command, &[])),
        _ => None,
    }
}

fn find_executable(binary: &str) -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for suffix in [
            ".local/bin",
            ".volta/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".cargo/bin",
            ".deno/bin",
        ] {
            dirs.push(home.join(suffix));
        }
    }
    for base in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(base));
    }
    dirs.into_iter()
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

fn rust_analyzer_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SHIRUSHI_RUST_ANALYZER") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = &home {
        let toolchains = home.join(".rustup/toolchains");
        if let Ok(entries) = std::fs::read_dir(toolchains) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin/rust-analyzer");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        let proxy = home.join(".cargo/bin/rust-analyzer");
        if proxy.exists() {
            return Some(proxy);
        }
    }
    Some(PathBuf::from("rust-analyzer"))
}

/// サーバからの通知（method, params）。`publishDiagnostics` 等が流れる。
pub type ServerNotification = (String, Value);

type ResponseResult = Result<Value, RpcError>;
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<ResponseResult>>>>;
type ProcessStdin = Arc<Mutex<Box<dyn Write + Send>>>;

/// JSON-RPC エラー。
#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// Mutex ロック（poison は into_inner で回収＝panic しない）。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// 1 プロジェクトにつき 1 つの言語サーバ接続。
pub struct LspClient {
    stdin: ProcessStdin,
    next_id: AtomicI64,
    pending: Pending,
    client_process_id: Option<u32>,
    _process: HostProcess,
}

impl LspClient {
    /// サーバ（例: rust-analyzer / typescript-language-server --stdio）を `root` で起動する。
    /// `args` はサーバ固有の起動引数（多くは空。stdio 系は `["--stdio"]`）。通知の受信チャネルも返す。
    pub fn new(
        server: &Path,
        args: &[&str],
        root: &Path,
    ) -> Result<(LspClient, mpsc::UnboundedReceiver<ServerNotification>)> {
        Self::new_on(LocalHost::shared(), server, args, root)
    }

    /// 指定 host 上で LSP を起動する。remote host は ControlMaster の別 session を使う。
    pub fn new_on(
        host: Arc<dyn Host>,
        server: &Path,
        args: &[&str],
        root: &Path,
    ) -> Result<(LspClient, mpsc::UnboundedReceiver<ServerNotification>)> {
        // LSP の processId は server と同じ OS 上の client PID。remote server へ local PID を渡すと
        // 無関係な remote process を監視し得るため null にする。
        let client_process_id = (!host.is_remote()).then(std::process::id);
        let spec = CommandSpec::new(server.to_string_lossy(), root).args(args.iter().copied());
        let mut process = host
            .spawn_process(&spec)
            .with_context(|| format!("言語サーバの起動に失敗: {}", server.display()))?;
        let stdin = process.take_stdin()?;
        let stdout = process.take_stdout()?;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (notification_tx, notification_rx) = mpsc::unbounded();

        let reader_pending = pending.clone();
        let reader_stdin = stdin.clone();
        std::thread::spawn(move || {
            reader_loop(stdout, reader_pending, notification_tx, reader_stdin)
        });

        let client = LspClient {
            stdin,
            next_id: AtomicI64::new(1),
            pending,
            client_process_id,
            _process: process,
        };
        Ok((client, notification_rx))
    }

    /// 要求を送る。応答は返した receiver に届く（reader スレッドが相関して解決）。
    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<ResponseResult> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        let message = json!({ "jsonrpc": JSONRPC, "id": id, "method": method, "params": params });
        if let Err(error) = self.write_message(&message) {
            if let Some(sender) = lock(&self.pending).remove(&id) {
                let _ = sender.send(Err(RpcError {
                    code: -1,
                    message: format!("送信失敗: {error}"),
                }));
            }
        }
        receiver
    }

    /// 通知を送る（応答なし）。
    pub fn notify(&self, method: &str, params: Value) {
        let message = json!({ "jsonrpc": JSONRPC, "method": method, "params": params });
        if let Err(error) = self.write_message(&message) {
            eprintln!("LSP 通知の送信に失敗 ({method}): {error}");
        }
    }

    fn write_message(&self, message: &Value) -> std::io::Result<()> {
        let body = serde_json::to_string(message)?;
        let mut stdin = lock(&self.stdin);
        write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
        stdin.flush()
    }

    /// initialize 要求だけ送って receiver を返す（GPUI 用: `&self` を await 跨ぎで持たない）。
    /// 応答が来たら [`Self::initialized`] を呼ぶこと。
    pub fn initialize_request(&self, root: &Path) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "initialize",
            initialize_params(root, self.client_process_id),
        )
    }

    /// initialized 通知（initialize 応答後に 1 度だけ送る）。
    pub fn initialized(&self) {
        self.notify("initialized", json!({}));
    }

    /// initialize → capabilities 受領 → initialized 通知、まで（同期 await 版。テスト/単体用）。
    pub async fn initialize(&self, root: &Path) -> Result<Value> {
        let outcome = self
            .request(
                "initialize",
                initialize_params(root, self.client_process_id),
            )
            .await
            .context("initialize の応答なし")?;
        let result =
            outcome.map_err(|error| anyhow::anyhow!("initialize エラー: {}", error.message))?;
        self.notify("initialized", json!({}));
        Ok(result)
    }

    /// ファイルを開いたことを通知（これをしないと ra は診断/補完を返さない）。
    pub fn did_open(&self, path: &Path, language_id: &str, version: i32, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": path_to_uri(path), "languageId": language_id, "version": version, "text": text
            } }),
        );
    }

    /// ファイルを閉じたことを通知（タブを閉じたら送る＝サーバの追跡から外す）。
    pub fn did_close(&self, path: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": path_to_uri(path) } }),
        );
    }

    /// 変更通知（v1 は FULL テキスト）。
    pub fn did_change(&self, path: &Path, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": version },
                "contentChanges": [ { "text": text } ]
            }),
        );
    }

    /// 増分 didChange（M11-8）。サーバが Incremental sync を広告している時だけ使うこと。
    #[allow(clippy::too_many_arguments)]
    pub fn did_change_incremental(
        &self,
        path: &Path,
        version: i32,
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        text: &str,
    ) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": version },
                "contentChanges": [ {
                    "range": {
                        "start": { "line": start_line, "character": start_character },
                        "end": { "line": end_line, "character": end_character }
                    },
                    "text": text
                } ]
            }),
        );
    }

    /// 補完要求（位置は UTF-16）。結果 Value は `CompletionResponse`（Array or List）。
    pub fn completion(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character }
            }),
        )
    }

    /// ホバー要求。
    pub fn hover(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character }
            }),
        )
    }

    /// ドキュメント整形要求（⌥⇧F / 保存時フォーマット・M11）。結果は TextEdit[]。
    pub fn formatting(&self, path: &Path, tab_size: u32) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "options": { "tabSize": tab_size, "insertSpaces": true }
            }),
        )
    }

    /// rename 要求（F2・M11）。結果は WorkspaceEdit（changes / documentChanges）。
    pub fn rename(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        new_name: &str,
    ) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/rename",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character },
                "newName": new_name
            }),
        )
    }

    /// 参照検索要求（⇧F12・M11）。結果は Location[]。
    pub fn references(&self, path: &Path, line: u32, character: u32) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true }
            }),
        )
    }

    /// code actions 要求（⌘.・M11）。`diagnostics` は該当位置の診断（LSP 生 JSON）を渡す。
    pub fn code_actions(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        diagnostics: serde_json::Value,
    ) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character }
                },
                "context": { "diagnostics": diagnostics }
            }),
        )
    }

    /// codeAction/resolve（edit が遅延解決のアクション用・M11）。
    pub fn resolve_code_action(&self, action: serde_json::Value) -> oneshot::Receiver<ResponseResult> {
        self.request("codeAction/resolve", action)
    }

    /// ワークスペースシンボル要求（⌘T・M11）。結果は SymbolInformation[] / WorkspaceSymbol[]。
    pub fn workspace_symbols(&self, query: &str) -> oneshot::Receiver<ResponseResult> {
        self.request("workspace/symbol", json!({ "query": query }))
    }

    /// 定義ジャンプ要求。結果は Location / Location[] / LocationLink[]。
    pub fn definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> oneshot::Receiver<ResponseResult> {
        self.request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "position": { "line": line, "character": character }
            }),
        )
    }
}

// ── 読取スレッド（Content-Length フレーム → 相関/通知） ──

fn reader_loop(
    stdout: Box<dyn Read + Send>,
    pending: Pending,
    notification_tx: mpsc::UnboundedSender<ServerNotification>,
    stdin: ProcessStdin,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        // ヘッダを `\r\n\r\n` まで読む（Content-Length を拾う）。
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return, // EOF / エラー = サーバ終了
                Ok(_) => {}
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        if content_length == 0 {
            continue;
        }
        let mut body = vec![0u8; content_length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        dispatch(value, &pending, &notification_tx, &stdin);
    }
}

fn dispatch(
    value: Value,
    pending: &Pending,
    notification_tx: &mpsc::UnboundedSender<ServerNotification>,
    stdin: &ProcessStdin,
) {
    let id = value.get("id").and_then(Value::as_i64);
    let has_method = value.get("method").is_some();
    match (id, has_method) {
        // レスポンス（id あり・method 無し）→ pending を解決。
        (Some(id), false) => {
            if let Some(sender) = lock(pending).remove(&id) {
                let result = if let Some(error) = value.get("error") {
                    Err(serde_json::from_value(error.clone()).unwrap_or(RpcError {
                        code: 0,
                        message: "不明なエラー".to_string(),
                    }))
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = sender.send(result); // receiver が drop 済み = 呼び出し側が去った = 無視でよい
            }
        }
        // サーバ→クライアント要求（id あり・method あり）→ MethodNotFound を返す（ra を待たせない）。
        (Some(id), true) => {
            let reply = json!({
                "jsonrpc": JSONRPC, "id": id,
                "error": { "code": -32601, "message": "method not found" }
            });
            if let Ok(body) = serde_json::to_string(&reply) {
                let mut guard = lock(stdin);
                let _ = write!(guard, "Content-Length: {}\r\n\r\n{}", body.len(), body);
                let _ = guard.flush();
            }
        }
        // 通知（id 無し・method あり）→ 上位へ。
        (None, true) => {
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                let _ = notification_tx.unbounded_send((method.to_string(), params));
            }
        }
        _ => {}
    }
}

// ── 最小の型（診断のパース用。補完等の結果は Value のまま上位で解釈） ──

/// `textDocument/publishDiagnostics` の params。
#[derive(Debug, Clone, Deserialize)]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// 1 件の診断。gutter は行番号だけ使う（character は未使用）。
#[derive(Debug, Clone, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    #[serde(default)]
    pub severity: Option<u8>,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// LSP `TextEdit`。位置は UTF-16 code unit。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TextEdit {
    pub range: Range,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// 1 ファイル分の `WorkspaceEdit`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTextEdits {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
}

/// 定義ジャンプの着地点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionLocation {
    pub path: PathBuf,
    pub position: Position,
}

/// LSP Hover の contents（MarkupContent / MarkedString / MarkedString[]）をプレーン行に落とす。
pub fn parse_hover_lines(value: &Value) -> Vec<String> {
    fn push_text(text: &str, lines: &mut Vec<String>) {
        for line in text.lines() {
            if !line.trim_start().starts_with("```") {
                lines.push(line.to_string());
            }
        }
    }

    fn push_marked(item: &Value, lines: &mut Vec<String>) {
        if let Some(text) = item.as_str() {
            push_text(text, lines);
        } else if let Some(text) = item.get("value").and_then(Value::as_str) {
            push_text(text, lines);
        }
    }

    let mut lines = Vec::new();
    let Some(contents) = value.get("contents") else {
        return lines;
    };
    if let Some(array) = contents.as_array() {
        for item in array {
            push_marked(item, &mut lines);
        }
    } else {
        push_marked(contents, &mut lines);
    }

    let mut compact = Vec::new();
    for line in lines {
        if line.trim().is_empty()
            && compact
                .last()
                .map(|last: &String| last.trim().is_empty())
                .unwrap_or(true)
        {
            continue;
        }
        compact.push(line);
    }
    while compact
        .last()
        .map(|line| line.trim().is_empty())
        .unwrap_or(false)
    {
        compact.pop();
    }
    compact
}

/// LSP `TextEdit[]` を型付きの編集列へ変換する。不正な要素だけを読み飛ばす。
pub fn parse_text_edits(value: &Value) -> Vec<TextEdit> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|edit| serde_json::from_value(edit.clone()).ok())
        .collect()
}

/// `WorkspaceEdit` をファイル単位に畳む（`changes` / `documentChanges` 両対応）。
pub fn parse_workspace_edit(value: &Value) -> Vec<FileTextEdits> {
    let mut result = Vec::new();
    let mut push = |uri: &str, edits: &Value| {
        let Some(path) = uri_to_path(uri) else {
            return;
        };
        let edits = parse_text_edits(edits);
        if !edits.is_empty() {
            result.push(FileTextEdits { path, edits });
        }
    };

    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            push(uri, edits);
        }
    }
    if let Some(document_changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            let Some(uri) = change
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
            else {
                continue;
            };
            if let Some(edits) = change.get("edits") {
                push(uri, edits);
            }
        }
    }
    result
}

/// 未オープンのファイル内容へ `TextEdit` 群を適用する。
pub fn apply_text_edits_to_string(text: &str, edits: &[TextEdit]) -> String {
    let mut line_starts = vec![0usize];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index + 1);
        }
    }
    let to_byte = |position: Position| -> usize {
        let line = (position.line as usize).min(line_starts.len() - 1);
        let start = line_starts[line];
        let end = line_starts.get(line + 1).copied().unwrap_or(text.len());
        let slice = &text[start..end];
        let mut utf16 = 0usize;
        for (offset, character) in slice.char_indices() {
            if utf16 >= position.character as usize {
                return start + offset;
            }
            utf16 += character.len_utf16();
        }
        end
    };
    let mut byte_edits: Vec<_> = edits
        .iter()
        .map(|edit| {
            let start = to_byte(edit.range.start);
            let end = to_byte(edit.range.end).max(start);
            (start, end, edit.new_text.as_str())
        })
        .collect();
    byte_edits.sort_by_key(|(start, _, _)| *start);

    let mut result = text.to_string();
    for (start, end, new_text) in byte_edits.into_iter().rev() {
        result.replace_range(start..end, new_text);
    }
    result
}

/// 定義ジャンプ結果（Location / Location[] / LocationLink[]）の先頭を返す。
pub fn parse_definition(value: &Value) -> Option<DefinitionLocation> {
    let location = if value.is_array() {
        value.as_array()?.first()?
    } else if value.is_object() {
        value
    } else {
        return None;
    };
    let (uri, range) = if let Some(uri) = location.get("targetUri").and_then(Value::as_str) {
        let range = location
            .get("targetSelectionRange")
            .or_else(|| location.get("targetRange"))?;
        (uri, range)
    } else {
        (location.get("uri")?.as_str()?, location.get("range")?)
    };
    let position = serde_json::from_value(range.get("start")?.clone()).ok()?;
    Some(DefinitionLocation {
        path: uri_to_path(uri)?,
        position,
    })
}

/// 診断の重大度（1=Error 2=Warning 3=Information 4=Hint。既定は Error）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub fn from_lsp(code: Option<u8>) -> Severity {
        match code {
            Some(2) => Severity::Warning,
            Some(3) => Severity::Information,
            Some(4) => Severity::Hint,
            _ => Severity::Error,
        }
    }
}

/// 最小の `InitializeParams`（UTF-16 明示・補完/hover の contentFormat）。
fn initialize_params(root: &Path, client_process_id: Option<u32>) -> Value {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    json!({
        "processId": client_process_id,
        "rootUri": path_to_uri(root),
        "capabilities": {
            // UTF-16 を明示（正しさの肝）。受信専用の診断は capability 不要。
            "general": { "positionEncodings": ["utf-16"] },
            "textDocument": {
                "completion": { "completionItem": { "snippetSupport": false } },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                // ⌘. code actions（M11）: literal 対応が無いと ra はアクションを返さない。
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": { "valueSet": [
                            "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                            "refactor.rewrite", "source", "source.organizeImports"
                        ] }
                    },
                    "resolveSupport": { "properties": ["edit"] },
                    "dataSupport": true
                },
                "rename": {},
                "formatting": {},
                "references": {}
            },
            // rename/code actions は documentChanges 形式で返ってくることがある。
            "workspace": { "workspaceEdit": { "documentChanges": true } }
        },
        "workspaceFolders": [ { "uri": path_to_uri(root), "name": name } ],
    })
}

/// `file://…` URI → パス。LSP URI の percent encoding を正しく戻す。
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let uri = url::Url::parse(uri).ok()?;
    (uri.scheme() == "file")
        .then(|| uri.to_file_path().ok())
        .flatten()
}

/// パス → `file://…` URI。
pub fn path_to_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|uri| uri.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_registry_maps_extensions_and_remote_commands() {
        let rust = language_server_for(Path::new("src/main.rs"), true).unwrap();
        assert_eq!(rust.language_id, "rust");
        assert_eq!(rust.command, PathBuf::from("rust-analyzer"));

        let typescript = language_server_for(Path::new("web/app.tsx"), true).unwrap();
        assert_eq!(typescript.language_id, "typescript");
        assert_eq!(typescript.args, vec!["--stdio"]);
        assert!(language_server_for(Path::new("README.md"), true).is_none());
    }

    #[test]
    fn hover_contents_are_normalized_to_plain_lines() {
        let hover = json!({
            "contents": [
                { "language": "rust", "value": "```rust\nfn main()\n```" },
                "\nextra\n\n"
            ]
        });
        assert_eq!(parse_hover_lines(&hover), vec!["fn main()", "", "extra"]);
        assert!(parse_hover_lines(&Value::Null).is_empty());
    }

    #[test]
    fn parse_definition_handles_location_shapes() {
        let location = json!({ "uri": "file:///x/lib.rs", "range": { "start": { "line": 10, "character": 4 }, "end": { "line": 10, "character": 9 } } });
        assert_eq!(
            parse_definition(&location),
            Some(DefinitionLocation {
                path: PathBuf::from("/x/lib.rs"),
                position: Position { line: 10, character: 4 },
            })
        );
        assert_eq!(parse_definition(&json!([location.clone()])), parse_definition(&location));

        let link = json!([{ "targetUri": "file:///y/m.rs", "targetSelectionRange": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 2 } } }]);
        assert_eq!(
            parse_definition(&link),
            Some(DefinitionLocation {
                path: PathBuf::from("/y/m.rs"),
                position: Position { line: 3, character: 0 },
            })
        );
        assert_eq!(parse_definition(&Value::Null), None);
    }

    #[test]
    fn apply_text_edits_handles_utf16_multiline_and_unsorted_input() {
        let parse = |value| parse_text_edits(&value);
        let edits = parse(json!([
            { "range": { "start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 2} }, "newText": "YY" }
        ]));
        assert_eq!(
            apply_text_edits_to_string("あxい\nsecond line\n", &edits),
            "あYYい\nsecond line\n"
        );

        let edits = parse(json!([
            { "range": { "start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 13} }, "newText": "3" },
            { "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3} }, "newText": "1" }
        ]));
        assert_eq!(apply_text_edits_to_string("one two three", &edits), "1 two 3");

        let edits = parse(json!([
            { "range": { "start": {"line": 0, "character": 1}, "end": {"line": 2, "character": 1} }, "newText": "" }
        ]));
        assert_eq!(apply_text_edits_to_string("aaa\nbbb\nccc", &edits), "acc");
    }

    #[test]
    fn parse_workspace_edit_supports_changes_and_document_changes() {
        let value = json!({
            "changes": {
                "file:///a.rs": [
                    { "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3} }, "newText": "new" }
                ]
            }
        });
        let parsed = parse_workspace_edit(&value);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, PathBuf::from("/a.rs"));
        assert_eq!(parsed[0].edits.len(), 1);

        let value = json!({
            "documentChanges": [
                { "textDocument": { "uri": "file:///b.rs", "version": 3 },
                  "edits": [ { "range": { "start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1} }, "newText": "x" } ] }
            ]
        });
        let parsed = parse_workspace_edit(&value);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, PathBuf::from("/b.rs"));
    }

    #[test]
    fn uri_roundtrip() {
        let path = Path::new("/Users/x/main.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///Users/x/main.rs");
        assert_eq!(uri_to_path(&uri), Some(path.to_path_buf()));

        let path_with_space = Path::new("/Users/x/project name/main.rs");
        let uri = path_to_uri(path_with_space);
        assert!(uri.contains("project%20name"));
        assert_eq!(uri_to_path(&uri), Some(path_with_space.to_path_buf()));
    }

    #[test]
    fn remote_initialize_does_not_advertise_local_pid() {
        let params = initialize_params(Path::new("/workspace"), None);
        assert!(params["processId"].is_null());
    }

    #[test]
    fn dispatch_resolves_response() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, _rx) = mpsc::unbounded();
        // ダミー stdin は作れないので、応答経路だけを検証する（stdin は使わない分岐）。
        let (sender, receiver) = oneshot::channel();
        lock(&pending).insert(7, sender);
        // stdin を要さないダミー（応答分岐は stdin を触らない）。
        let fake_stdin = make_fake_stdin();
        dispatch(
            json!({ "jsonrpc": "2.0", "id": 7, "result": { "ok": true } }),
            &pending,
            &tx,
            &fake_stdin,
        );
        let got = futures::executor::block_on(receiver).expect("応答が届く");
        let value = got.expect("result は Ok");
        assert_eq!(value["ok"].as_bool(), Some(true));
    }

    #[test]
    fn dispatch_forwards_notification() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded();
        let fake_stdin = make_fake_stdin();
        dispatch(
            json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": { "uri": "file:///a.rs", "diagnostics": [] } }),
            &pending,
            &tx,
            &fake_stdin,
        );
        let (method, params) = futures::executor::block_on(rx.next()).expect("通知が届く");
        assert_eq!(method, "textDocument/publishDiagnostics");
        let parsed: PublishDiagnosticsParams =
            serde_json::from_value(params).expect("パースできる");
        assert_eq!(parsed.uri, "file:///a.rs");
    }

    /// 実 rust-analyzer との initialize handshake（capabilities 受領）を検証する。
    /// 実サーバ + このリポジトリが要るので既定はスキップ。`cargo test -p lang -- --ignored` で実行。
    #[test]
    #[ignore]
    fn real_initialize_handshake() {
        let home = std::env::var("HOME").expect("HOME");
        let server = PathBuf::from(home).join(".cargo/bin/rust-analyzer");
        if !server.exists() {
            eprintln!("rust-analyzer が無いのでスキップ");
            return;
        }
        // crates/lang → リポジトリルート。
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repo root")
            .to_path_buf();
        let (client, _notifications) = LspClient::new(&server, &[], &root).expect("起動");
        let result = futures::executor::block_on(client.initialize(&root)).expect("initialize");
        assert!(result.get("capabilities").is_some(), "capabilities が返る");
        // 補完/hover/定義のプロバイダが広告されている。
        let capabilities = &result["capabilities"];
        assert!(
            capabilities.get("completionProvider").is_some(),
            "補完プロバイダ"
        );
        eprintln!("rust-analyzer capabilities OK");
    }

    /// 実 rust-analyzer が診断を出すか検証（`/tmp/lsp-test` にエラー入り Cargo プロジェクトが要る）。
    /// `cargo test -p lang -- --ignored real_diagnostics_flow --nocapture` で実行。
    #[test]
    #[ignore]
    fn real_diagnostics_flow() {
        let home = std::env::var("HOME").expect("HOME");
        let server = PathBuf::from(home).join(".cargo/bin/rust-analyzer");
        let root = Path::new("/tmp/lsp-test");
        if !server.exists() || !root.exists() {
            eprintln!("前提が無いのでスキップ（server or /tmp/lsp-test）");
            return;
        }
        let (client, mut rx) = LspClient::new(&server, &[], root).expect("起動");
        futures::executor::block_on(client.initialize(root)).expect("initialize");
        let main = root.join("src/main.rs");
        let text = std::fs::read_to_string(&main).expect("read main.rs");
        client.did_open(&main, "rust", 1, &text);
        // 通知を最大 400 件まで見て、非空の publishDiagnostics が来たら成功。
        for _ in 0..400 {
            let Some((method, params)) = futures::executor::block_on(rx.next()) else {
                break;
            };
            eprintln!("通知: {method}");
            if method == "textDocument/publishDiagnostics" {
                let parsed: PublishDiagnosticsParams =
                    serde_json::from_value(params).expect("パース");
                if !parsed.diagnostics.is_empty() {
                    eprintln!(
                        "★診断 {} 件・先頭: [{}行] {}",
                        parsed.diagnostics.len(),
                        parsed.diagnostics[0].range.start.line,
                        parsed.diagnostics[0].message
                    );
                    return;
                }
            }
        }
        panic!("診断が来なかった（rust-analyzer の warmup 不足 or 設定）");
    }

    /// テスト用のダミー stdin（応答/通知分岐では実際には書き込まれない）。
    fn make_fake_stdin() -> ProcessStdin {
        Arc::new(Mutex::new(Box::new(std::io::sink())))
    }
}
