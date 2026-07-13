# 移植ノート — git status / gutter diff（M8）・統合ターミナル（M8）・LSP（M7）

Zed ソース（`zed/`）を読み下した移植ガイドの蒸留版。実装時はここ + 該当 zed ファイルを正とする。
出典行番号は調査時点のもの（多少ズレても近傍にある）。

## A. Git status + gutter diff（M8）

**結論: Zed は git2/gix を使わず `git` CLI を叩き、行 diff は `imara-diff`。** これを踏襲する。
依存追加は `imara-diff = "0.1.8"` の 1 つだけ（`crates/project/Cargo.toml`）。純 Rust。

- **status**: `git --no-optional-locks status --porcelain=v1 --untracked-files=all --no-renames -z`
  → NUL 区切り。各エントリ `XY`＋` `＋path。末尾 `/` の untracked ディレクトリは捨てる。
  畳み込み: `(?,?)=Untracked` / `U|AA|DD=Conflicted` / `A→Added` / `D→Deleted` / else `Modified`。
  型: `enum StatusKind { Added, Modified, Deleted, Untracked, Conflicted }`。返り値 `Vec<(PathBuf, StatusKind)>`（ルート相対）。
- **HEAD blob**: `git --no-optional-locks show HEAD:<relpath>` → 成功時 stdout が HEAD 版テキスト。無ければ新規ファイル＝全行 Added。
- **行 diff**: `imara_diff::diff(Algorithm::Histogram, &InternedInput::new(lines_with_terminator(head), lines_with_terminator(cur)), Sink)`。
  `Sink::process_change(before: Range<u32>, after: Range<u32>)`: before=HEAD行域・after=現在バッファ行域（両方0始まり半開）。
  `after.is_empty()→Removed` / `before.is_empty()→Added` / else `Modified`。**after がガター描画のキー**。
  CRLF は両側で `\r\n→\n` 正規化してから diff（さもないと全行 Modified）。
- **非同期**: `cx.background_executor().spawn(...)` で status/diff を回し、foreground で結果格納 + `cx.notify()`。
- **捨てる**: SumTree/Anchor 差分・staging/index・word diff・summary rollup・collab/remote・blame/stash・rename/copy・Oid。

Zed 出典: `git/src/status.rs`(:9 enum, :111 from_bytes, :435 parse) / `git/src/repository.rs`(:3465 status args, :815 HEAD blob) /
`buffer_diff/src/buffer_diff.rs`(:1167 compute_hunks, :1258 process_change, :2253 kind) / `Cargo.toml:621`(imara-diff)。

## B. 統合ターミナル（M8）

**依存**: `alacritty_terminal`（Zed fork `git=zed-industries/alacritty rev=4c12966…` ≈ 0.26.1-dev。crates.io 0.24/0.25 も API ほぼ同一）。
**最重要**: 自前で reader スレッドも parser も書かない。**`EventLoop::spawn()` が読取スレッド＋vte parser** で、PTY→parse→`Term`(FairMutex)更新→`EventListener::send_event(Wakeup)` まで全部やる。
`vte::ansi::Processor` は PTY 経路では不要。

- **crate 分割**（Zed 踏襲）: `terminal`（モデル: alacritty/PTY 所有・`Content` スナップショット生成）+ `terminal_view`（GPUI ビュー＋grid 描画 Element）。zed の `terminal/src/alacritty.rs` がほぼ丸ごとバックエンド。
- **setup**: `Term::new(Config, &dims, Listener)` → `Arc<FairMutex<Term>>`。`tty::new(&Options{shell:None, working_directory:Some(cwd), env{TERM=xterm-256color,COLORTERM=truecolor}}, WindowSize, window_id)`。
  `EventLoop::new(term.clone(), Listener, pty, drain_on_exit=true, ref_test=false)` → `Notifier(event_loop.channel())`＝入力書込 / `event_loop.spawn()`＝IO スレッド。
- **Listener**: `struct L(UnboundedSender<AlacTermEvent>); impl EventListener { fn send_event(&self,e){ self.0.unbounded_send(e).ok(); } }`。
- **idle 0%**: 出力時のみ Wakeup→`events_rx.next().await` の foreground pump（`cx.spawn`）が `cx.emit(Wakeup)`→view の `cx.subscribe`/`cx.observe`→`cx.notify`。**タイマー無し**。カーソル blink は v1 で捨てる（静止ブロック）。
- **サイズ**: `TerminalBounds{cell_width,line_height,bounds}`。`num_lines=(h/line_height).next_up().floor()` / `num_columns=(w/cell_width).next_up().floor()`。`impl Dimensions`。
  cell 実寸は font: `advance(font_id,size,'m').width` / `line_height=font_px*1.3`。resize は `Msg::Resize(WindowSize)`（PTY winsize）＋`term.lock().resize(dims)`。行列/セル寸が変化した時だけ（SIGWINCH 抑制）。
- **描画**: foreground で `term.lock().renderable_content()` → `display_iter`(可視セル) を copy。Element paint: ①全面 bg quad ②非デフォ bg セルの quad ③各セル文字を `shape_line(c, size, &[run], Some(cell_width))`（等幅強制）で描画 ④ブロックカーソル=quad（focus 時 filled＋下地文字を bg 色で再描画 / 非focus は outline）。
  色: `Color::Named(n)`→ANSI16+fg/bg/cursor / `Spec(rgb)`→truecolor / `Indexed(i)`→256(16-231=6×6×6 cube `c==0?0:c*40+55`, 232-255=grayscale `i*10+8`)。`Flags::INVERSE`で fg/bg swap・`BOLD`/`ITALIC`・`WIDE_CHAR_SPACER`はskip。
- **入力（2 経路とも要る）**: ①印字文字/IME は GPUI `InputHandler::replace_text_in_range(text)`→`term.input(text.as_bytes())`→PTY。②特殊キーは `on_key_down`→`to_esc_str(&keystroke, Modes, option_as_meta)->Option<Cow<str>>`（`mappings/keys.rs` を丸ごと移植・依存少）。Some なら PTY 送信＋`stop_propagation`、None なら印字経路へ委譲。
  最低限: Enter=`\r`, S-Enter=`\n`, BS=`\x7f`, Tab=`\t`, Esc=`\x1b`, 矢印=`\x1b[A/B/C/D`（APP_CURSOR 時 `\x1bOA…`）, Ctrl-A..Z=`\x01..\x1a`。`Modes` は `renderable_content().mode` から（最低 APP_CURSOR）。
- **捨てる**: hyperlink 検出・task runner・検索・選択/コピー・mouse reporting・vi mode・scrollback UI・永続化・複数端末/分割・blink・min-contrast・IME pre-edit・editor::CursorLayout。

Zed 出典: `terminal/src/alacritty.rs`(丸ごと) / `terminal/src/terminal.rs`(:489 Content, :746 TerminalBounds, :1313 pump, :1603 resize) / `terminal_view/src/terminal_element.rs`(:372 layout_grid, :1324 paint, :1383 handle_input) / `terminal_view/src/mappings/keys.rs`(to_esc_str) / `Cargo.toml:513`。

## C. LSP: rust-analyzer（M7）

**依存**: crates.io `lsp-types = "0.97"`（型は全部これ・手書きしない）。ランタイムは smol + futures（GPUI executor と一致）。プロセスは `smol::process::Command`。
自前で書くのは **JSON-RPC 封筒のみ**（lsp-types は封筒を持たない）。

- **封筒**: `RequestId = untagged{Int(i32),Str}`。`Request{jsonrpc:"2.0",id,method,params}` / `Notification{jsonrpc,method,params}`。
  受信は 2 段: まず `NotificationOrRequest{id:Option, method, params:Option<Value>}`、失敗したら `AnyResponse{id, error:Option<RpcError>, result:Option<RawValue>}`（response は method 無し＝前者を弾く）。`RpcError{code:i64,message,data}`。
- **transport**: spawn=`Command.current_dir(root).stdin/out/err(piped).kill_on_drop(true).spawn()`。
  共有: `response_handlers: Arc<Mutex<HashMap<RequestId, FnOnce(Result<String,RpcError>)>>>` / `notification_handlers: HashMap<&'static str, FnMut(Option<RequestId>,Value,&mut AsyncApp)>` / `outbound_tx: smol::channel::Sender<String>`。
  - **writer** task: `outbound_rx.recv()` → `Content-Length: {len}\r\n\r\n{json}` を BufWriter へ書いて flush。len は UTF-8 バイト長。
  - **reader** task: `read_until(\n)` で `\r\n\r\n` までヘッダ収集→Content-Length 解析→`read_exact(len)`→`from_slice::<NotificationOrRequest>` 優先→ダメなら `AnyResponse`→`response_handlers.remove(&id)` を呼ぶ。
  - **request**: `id=next_id.fetch_add(1)`, oneshot 登録, `outbound_tx.try_send(json)`, `select!` で timeout。
  - **未処理 request（id あり）には `-32601 MethodNotFound` を返す**（ra を待たせない）。他の server→client request ハンドラは実装しない。
- **lifecycle**: `initialize`(req)→capabilities 保存→`initialized`(notif)→バッファ毎 `didOpen`→`didChange`…→終了時 `shutdown`(req,~5s)→`exit`(notif)→kill。
  最小 `InitializeParams`: `process_id, root_uri, capabilities{general:{position_encodings:[UTF16]}}, workspace_folders`。`initialization_options:None` で ra は動く。**didOpen しないと ra は補完/診断を返さない（必須）**。
  didChange は **FULL**（`content_changes=[{range:None,text:全文}]`・version は単調増加 i32）。ra は INCREMENTAL 広告でも FULL を受ける。
- **診断（#1 優先）**: `textDocument/publishDiagnostics` notification（capability 不要で受信可）。`PublishDiagnosticsParams{uri,diagnostics:Vec<Diagnostic>,version}`。
  `Diagnostic{range,severity:Option<1=ERR/2=WARN/3=INFO/4=HINT>,message,..}`。**空 vec は「この uri の診断を全消し」＝置換**。range→byte は §E。gutter 行＝`range.start.line`。ra の実診断は起動数秒後（cargo check 後）。
- **補完/hover/定義**: 発行前に capabilities を gate。
  - Completion `textDocument/completion` → `Option<CompletionResponse>`（`Array(Vec<CompletionItem>)` or `List(CompletionList)` 両対応）。挿入は `text_edit` 優先→`insert_text`→`label`。
  - Hover `textDocument/hover` → `Option<Hover{contents:Scalar|Array|Markup, range}>`。
  - Definition `textDocument/definition` → `Option<GotoDefinitionResponse>`（`Scalar(Location)|Array|Link(LocationLink)` 全対応）。`Location{uri,range}`。
- **§E 位置変換（最重要の罠）**: LSP `Position.character` は **行頭からの UTF-16 code unit 数**。ropey で:
  ```
  // Position→byte
  line=min(pos.line, len_lines-1); ls_char=line_to_char(line);
  target_u16=char_to_utf16_cu(ls_char)+pos.character; char=utf16_cu_to_char(target_u16); byte=char_to_byte(char)
  // byte→Position
  char=byte_to_char(byte); line=char_to_line(char);
  character=char_to_utf16_cu(char)-char_to_utf16_cu(line_to_char(line))
  ```
  naive な byte/char 換算は非BMP文字（絵文字・一部CJK）で全位置がズレる。行末 `\n` はその行に属する・`character` は行の UTF-16 長にクランプ。
- **threading/idle 0%**: reader は `stdout.read_exact().await`＝バイト到着時のみ起床（poll 無し）。response は oneshot→`cx.spawn` の `.await` 再開→`update+notify`。notification は reader→mpsc→foreground loop（`&mut AsyncApp`）で UI スレッド上ハンドラ→直接 `update`。
- **捨てる**: LspStore 全体・複数 server・remote/proto・LspCommand 抽象・server discovery/download・`default_initialize_params` の巨大 capability・incremental didChange・symbols/code action/format/rename/inlay/semantic tokens・二重 notification channel。

Zed 出典: `lsp/src/lsp.rs`(:212 封筒, :421 spawn, :710 writer, :1426 request, :1055 initialize) / `lsp/src/input_handler.rs`(:35 read_headers, :70 reader) /
`project/src/lsp_store.rs`(:874 診断登録, :8422 didChange, :11856 診断→entry) / `language/src/language.rs`(:1478 point_to_lsp) / `Cargo.toml:638`(lsp-types)。
