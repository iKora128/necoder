# ARCHITECTURE — 実装設計図

目的: この文書 + [`UI-SPEC.md`](./UI-SPEC.md) + [`ROADMAP.md`](./ROADMAP.md) + [`../FEATURES.md`](../FEATURES.md) + `mock/index.html` だけで、
新しいエージェントセッションが**質問なしで**実装を進められる状態を保つ。乖離が出たらコードではなくまずこの文書群を直す。

## 1. 層構造と依存方向（鉄則）

```
[shell]      necoder(bin) ─ 結線・起動・メニュー
[shell]      workspace ─ レール / chrome / active ProjectSession の合成・event routing
[view]       editor_view / webview_view / explorer / git_ui / review_view / search_ui / agent_panel / terminal_view / settings / graph_view(M14)
[model]      editor_core / project / acp_client / search / lang / storage
[foundation] ui(部品+Registry) / theme_core / settings_core / keymap_core / i18n
[外部]       gpui(git rev固定) / agent-client-protocol(crates.io) / ropey / alacritty_terminal
```

依存の向き（Zed と同じ）。違反 import を見つけたら実装でなく設計を疑う:
- `editor_core` は **GPUI を知らない**（純データ + ロジック。テストが最速で回る層）
- view 層は `workspace` を知らない（workspace が view を「載せる」。逆はない）
- `ui` は `theme_core` / `i18n` のみに依存
- AI・Git・ターミナルを editor_core が import したら誤り（Zed の editor が vim/collab/agent を知らないのと同型）

`workspace` の具体的な ownership と source 境界は
[`REFACTOR-WORKSPACE.md`](./REFACTOR-WORKSPACE.md) を正とする。

## 2. crate 対応表（実装の出自・ライセンス境界）

| crate | 中身 | 出自 | 時期 |
|---|---|---|---|
| `necoder` (bin) | 結線・起動 | necoder 固有の独立実装 | M1 ✓ |
| `theme_core` | トークン構造体・dark/light・ProjectIdentity/ThreadColor | UI-SPEC §1 を型にした独立実装 | M2 |
| `i18n` | `t!` マクロ・ja/en YAML 同梱 | 独立実装（§6） | M2 ✓ |
| `editor_core` | Buffer(ropey)・Selection・Transaction/undo | `ropey` 上の独立実装。CRDT 不採用 | M2 |
| `editor_view` | 行仮想化描画・gutter・キャレット・IME | GPUI の公開 API / examples 上の独立実装 | M2 |
| `settings_core` / `settings` | 3層設定・監視・設定画面 | serde/YAML/GPUI 上の独立実装 | M3/M13 |
| `keymap_core` / `ui` | keymap・Button/List/Picker/Modal・Registry 群・`links`（本文中のパス/URL 検出。transcript とターミナルが共有） | GPUI の公開 API 上の独立実装 | M3 |
| `workspace` | レール・ドック・ペイン・タブ・statusbar・永続化 | necoder の `ProjectSession` モデルによる独立実装 | M3 |
| `project` / `explorer` / `search` | FS・worktree・Git・各ビュー | Rust 標準 API、`notify`、Git CLI、`imara-diff`、ripgrep 上の独立実装 | M3-M6 |
| `acp_client` / `agent_panel` | ACP セッション・transcript・composer | crates.io `agent-client-protocol` と necoder 固有 UI の独立実装 | M4 |
| `lang` | tree-sitter ハイライト・LSP クライアント | 公開 LSP 仕様と tree-sitter crates 上の独立実装 | M7 |
| `git_ui` / `terminal_view` | gutter diff / 統合ターミナル（⌘F バーの入力欄は `editor_view` の 1 行入力を借りる。`editor_view` は端末を知らない） | `imara-diff` / crates.io `alacritty_terminal` 上の独立実装 | M8 |
| `review_view` | 変更レビュー（worktree の全変更を 1 画面で・可変高リスト・ツリー・畳み）。構造化 diff は `project::review`（git CLI の unified diff を解析・読み込みは `ReviewCancel` で取り消せる）。`editor_view` は構文色の写像だけ借りる（コアは変更レビューを知らない） | GPUI の公開 API・Git CLI 上の独立実装（Orca の機能比較のみ） | parity O6 |
| `webview_view` | ローカル HTML プレビュー / artifact の隔離表示 / localhost の Web タブ | `wry` の child view API。macOS=WKWebView / Windows=WebView2（エンジン非同梱）。Web タブの移動判定を最上位だけに掛けるため、macOS は wry の navigation delegate を包む（`main_frame.rs`・objc2）。Design モードのピッカー（`design_picker.js`・初期化スクリプト）と IPC は Web タブの WebView にだけ付け、受けた知らせは `design.rs` が送り手・nonce・形・大きさで検め秘密を伏せる。要素の切り抜きは `snapshot.rs`（macOS=`takeSnapshotWithConfiguration` / Windows=`CapturePreview` + 切り抜き）。Web Inspector（Web タブの `</>`・右クリック）は macOS では WebView の中に付けない: WebKit が付けると WKWebView を窓全体の幅に広げて GPUI の UI を覆うので、WKWebView の `NSViewFrameDidChangeNotification` で付いたことを知り、wry が既に使う私的 `_inspector` の `detach` で別の窓へ出して枠を戻す（`inspector.rs`）。開閉の状態は `WebViewView` が持つ（WebView2 は閉じる口が無いので開くだけ）。HTML ファイルを Web タブで開く**内蔵の配信**は `static_server.rs`（std の `TcpListener` だけ・`127.0.0.1`・token 付きの URL・Host 検査・使う Web タブがある間だけ動く） | M14 |
| `graph_view` | worktree×commit の DAG・custom Element | Git CLI の出力を使う独立実装 | M14 |

Zed のソースは GPUI API の利用例や設計比較のために閲覧しているため、本プロジェクトを厳密な意味での
clean-room 実装とは呼ばない。一方、Zed の GPL アプリケーション crate からソースコードを複製、翻訳、
改変して取り込まないことをプロジェクト境界とする。比較で得た一般的な設計上の知見は、公開仕様と
permissive な依存ライブラリを使い、necoder の型と要件から独立に実装する。

直接利用する `gpui` / `gpui_platform` は Zed 側で Apache-2.0 と表示されている。ただし現在固定している
Zed revision の依存グラフには `ztracing` / `ztracing_macro` / `zlog`（GPL-3.0-or-later）が含まれる。
necoder はこれらと GPLv3/AGPLv3 §13 の互換規定に基づいて AGPL-3.0-or-later で組み合わせて配布する。
第三者 GPL 部分を含む間、配布バイナリ全体を任意の非 GPL/AGPL ライセンスへ再ライセンスできる、とは扱わない。
詳細は [`DECISIONS.md`](./DECISIONS.md) §5 と [`../THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md)。

## 3. コア型スケッチ（M2 の契約 — 変えるならここを先に変える）

```rust
// editor_core（GPUI 非依存）
pub struct Buffer {
    rope: ropey::Rope,
    selections: Vec<Selection>,
    history: History,          // Transaction 単位の undo/redo
    version: u64,
    file: Option<PathBuf>,     // None = 無題
    dirty: bool,
}
pub struct Selection { pub anchor: usize, pub head: usize } // byte offset・UTF-8 境界保証
pub struct Transaction { edits: Vec<Edit>, before: Vec<Selection>, after: Vec<Selection> }

impl Buffer {
    pub fn edit(&mut self, ranges: &[Range<usize>], text: &str) -> TransactionId;
    pub fn undo(&mut self) -> Option<TransactionId>;
    pub fn redo(&mut self) -> Option<TransactionId>;
    pub fn snapshot(&self) -> BufferSnapshot;   // 描画側はこれ「だけ」を読む（不変・行アクセス O(log n)）
    pub fn save(&mut self) -> anyhow::Result<()>;
}
```

- 位置追従アンカーは M2 では offset ベースの簡易版でよい（multibuffer を入れる時に anchor 化）
- ペインに載る物は `TabItem` trait（エディタ/画像/ディレクトリビュー/設定UI を同格に扱う。multibuffer 前提の抽象だけ先に切る）
- **Pane/Item の初版（M10・複数タブ）**: `TabItem` trait を最初から抽象化し切らず、まず `workspace` 内の
  具体型 `EditorTab { path, editor: Entity<EditorView>, _observation }` の `Vec` + `active_tab: usize` で始める
  （ペインは当面「主ペイン = 複数タブ」+「右分割 = 単一比較ビュー」）。多態化（画像/diff/設定 UI を同格に）が
  必要になった時点で `enum PaneItem { Editor(..), Diff(..), .. }` → `trait TabItem` へ育てる（multibuffer 本体は later）。
  **現在地（2026-09-26）**: `enum TabContent { Editor, Image, Pdf, Web }` の 4 具体型。Image / Pdf / Web は「編集も保存も
  LSP もしない表示専用タブ」で、trait 化はこの性質を持たない Item（diff / 設定 UI）が要求した時点で再検討する。
  Web（localhost の開発サーバ・`web_preview_view`）は鍵（`EditorTab.path`）に URL をそのまま入れる — ファイルの鍵は
  絶対パスなので衝突せず、窓セッションの `open_files` の形も変えずに永続化できる（`web_tab_url` が見分ける）。
  Pdf は自前レンダラを持たず、`webview_view`（HTML プレビュー用のネイティブ子ビュー層）に `file://` を渡して
  OS のビューア（macOS = WKWebView の PDFKit / Windows = WebView2）に描かせる。ネイティブ子ビューは GPUI の
  描画木を外れても OS 側に残るため、`Workspace::sync_native_view_visibility` が毎 render で可視性と
  キーボードフォーカスを同期する（HTML プレビューと共用の規律・回収弁も `html_preview_evict_minutes` を共有）。
  **OS 子ビューは同じ窓の GPUI 描画より常に手前**で層を挟めない（z 順で勝つ手が無い）ため、操作を受ける
  オーバーレイ（Picker / 検索バー / メニュー / モーダル / 補完…）が開いている間は `overlay_hides_native_view`
  が false を返して**ネイティブ側を隠す**。通知系（トースト・フラッシュ・紙吹雪）は数えない（2026-09-13）。
  remote (SSH) の PDF は OS 子ビューがローカルパスしか読めないので `<cache>/remote-pdf/` へ複製して見せる。
  複製はタブが所有し `Drop` で消す（リモートの中身を手元に残さない）。
  永続化は `ProjectSlot.open_files: Vec<PathBuf>` + `active_file`（プロジェクト単位でタブ列を復元）。
  非アクティブプロジェクトのタブは**遅延復元**（レール切替時に開く）。LSP は didOpen/didClose をタブ開閉に追従、
  didChange は編集タブごと（`lsp_sent_versions: HashMap<Path, u64>` で誤スキップを防ぐ）。

```rust
// theme_core — 「きせかえ」の2軸: テーマ（面の配色）× プロジェクト色（識別）
pub struct Theme { /* UI-SPEC §1 のトークン表と 1:1 のフィールド */ }
pub enum ThemeSource { BuiltIn(&'static str), User(PathBuf) } // themes/*.json = トークン上書き JSON
impl Theme { pub fn load(source) -> Result<Theme>; }          // 欠けたキーは built-in にフォールバック
pub struct ProjectIdentity { pub color: Hsla, pub icon: IconSource } // .necoder/settings.json > DB project_colors（手動・初回自動割当を焼く）> 未使用パレット色（workspace/project_colors.rs）
pub enum IconSource { Monogram(char), Emoji(String), Image(PathBuf) }
```
テーマセレクタ（ライブプレビュー付き）は M3 の Picker 基盤に載せる。VSCode/Zed テーマのインポートは
公開されたテーマ形式を解析する独立実装として later に扱う（Zed の `theme_importer` コードは取り込まない）。

## 4. 登録式境界（コアは機能を知らない）

VSCode の contribution points / Zed の初期化結線から学んだ形。**本体機能も最初の「拡張」としてこの口から登録する**:

- `CommandRegistry` → palette action の id / i18n key を一意な一覧から引く
- `PanelRegistry` → child Entity の typed event subscription を一箇所で結線する
- `StatusItemRegistry`（未導入。statusbar の拡張点が必要になった時点で追加）
- `KeymapContext` 述語（`"Editor && mode == full"` — gpui の KeyContext をそのまま使う）
- 将来の拡張 API（WASM）は同じ Registry へ別経路で流し込むだけ、が狙い（FEATURES 9 の ADR 対象）

## 5. ウィンドウモデル（2026-07-11 確定 → 2026-07-20 ownership 改訂）

- **1窓 = 複数 (project, branch/worktree) を持つレール**。`ProjectSessions` が
  `projects: Vec<ProjectSlot>` / `active` / `sessions: Vec<ProjectSession>` を同じ添字で所有する。
  通常切替は `active` だけを変え、既に開いた Editor Entity、dirty/undo、Agent session、Terminal process、
  watcher を破棄しない。未表示 session のタブだけ初回に遅延復元する。
- **ブランチ/worktree は既定でレールに開く**（`open_folder_in_rail`）。⎇ メニューの行クリック=in-place 切替、⧉=worktree をレールに、⎇ worktree セクション/⌘O worktree 行=レールに。**新窓は明示操作のみ**（レール右クリック→「新しいウィンドウで開く」・⌘⇧N）。旧「1窓=1worktree・新窓に開く」から転換（色による方向感覚を窓境界で切らないため）
- **レール項目の右クリック = コンテキストメニュー**（`render_rail_menu`）: 色スウォッチ＋「その他の色…」（フル hex ピッカー）／新しいウィンドウで開く／レールから外す／(worktree タブのみ) worktree を削除・worktree ごとブランチを削除。破壊的操作は**二段確認**（`RailMenuState.confirm`）。「削除」は3階層に分離 — 外す=表示のみ・worktree 削除=`git worktree remove`・ブランチ削除=worktree ごと `git branch -D`
- 同一リポジトリの別ブランチをレールに載せると identity 色が親と衝突する → `next_free_color` で未使用パレット色に倒し、同色スロット2枚を防ぐ
- titlebar ピル: プロジェクト名（クリック→⌘O スイッチャー）+ ⎇ ブランチ（クリック→branch/worktree メニュー）
- エージェントスレッドは (project, branch) に属する。titlebar beacon はアクティブ project 分、レールの静止ドットは他 project の Blocked/Done を担う（Working は herd/statusbar へ集約）

**Fleet モード（M14・UI-SPEC §11）**: 主単位は thread ではなく `TaskSpace`（1 task = 1 branch = 1 linked worktree = 1 `ProjectSession`）。各 Task cell は通常 View と同じ完全な `Entity<AgentPanel>` を埋め込み、同一 Task の複数 Agent は panel 内 thread として所有する。既定 `+ Task` は常に worktree を隔離し、main は protected `IntegrationSpace`。Task lifecycle / Agent runtime / Git health は別軸で、永続 event ledger、read-only merge preview、明示 integration gate を通す。詳細と実装来歴の境界は [FLEET-ARCHITECTURE.md](FLEET-ARCHITECTURE.md)。

### 5.1 Workspace shell の責務

`Workspace` の直接フィールドは `ProjectSessions`、theme/focus、chrome、overlays、notifications、
persistence、updater の 8 個。feature state は `ProjectSession` / `EditorArea` / child Entity に置く。

- `ProjectSession`: `EditorArea`、Explorer、Git、Search、Agent、Terminal、Todo、repository snapshot、watcher
- `EditorArea`: tabs/pane、LSP/diagnostics/completion、navigation、diff/inline edit、hot exit
- `CommandRegistry`: command palette の登録境界
- `PanelRegistry`: child → shell の typed event 登録境界
- root `Render`: chrome / rail / active session / overlay の合成。Host / FS / Git / DB は呼ばない

Window を必要とする child event は pending value として受け、effect-cycle 末尾で処理する。Render
中にファイルを開く、project を切り替える、Git 操作を開始する、といった状態変更は行わない。

Explorer / Git は feature crate が project model、interaction state、repository snapshot、typed event
契約を所有する一方、active `ProjectSlot` と rail / picker / notification / window を横断する実描画
callback は `workspace` の shell adapter が所有する。これを子 Render に移すと project state の複製か
巨大な往復 event protocol が必要になるためである。Search / Agent / Terminal / Settings / Todo など、
実際に child Entity から shell へ上がる通信は `PanelRegistry` の typed event を使う。Explorer / Git の
event enum は将来共通 Dock API へ adapter を移すための契約で、現在の操作は shell adapter 内で完結する。

## 6. i18n（2026-07-11 決定 — 言語パック内蔵）

- 方式: **薄い自作 `i18n` crate**（ロケール YAML を `include_str!` で埋め込み、`i18n::t!("tab.close")`）。
  当初 rust-i18n を予定したが、その `t!` はマクロが `crate::` スコープに閉じ、レイヤ化した多 crate 構成
  （ui / editor_view / workspace / agent_panel が各々 `t!`）に噛み合わない。**`t!` 境界は不変**（下の swap 方針）なので、
  workspace 内から `i18n::t!` で一様に呼べる自作実装にした（YAML パースは `serde_yaml`、ja/en parity テスト付き）
- **規律が本体**: UI 文字列は**初日から全て `t!` 経由**。ハードコード禁止（retrofit は地獄。Zed は i18n 無し＝後発が入れられない実例、VSCode は言語パック方式）
- キーは英語スネークケース。**`ja.yml` / `en.yml` を同梱**して出荷。OS ロケールで自動選択、settings で上書き
- **追加言語 = `locales/xx.yml` 1枚**（= 言語パック。later: 拡張として配布・コミュニティ翻訳）
- 複数形など高度要件が出たら fluent-rs へ移行（`t!` 境界を守っていればライブラリは差し替え可能）

## 7. 永続化（2026-07-16 更新: ローカル DB = Turso 採用）

**二本立て**: 「人が読む/編集するものはファイル」「機械が高頻度に読み書きするものは DB」。

- **ファイルが真実（DB に入れない）**: settings.json（user/project）・`.necoder/todos.md`（M12 Todo ボード — ファイルであること自体が要件）・keymap.json・テーマ JSON。git が真実のもの（status/diff/blame）も入れない。検索索引も持たない（regex 走査が正 — DECISIONS §8）
- **ローカル DB（`~/Library/Application Support/necoder/necoder.db`）**: [Turso](https://github.com/tursodatabase/turso)（SQLite の pure-Rust 再実装・MIT・async ネイティブ）を採用。用途は
  ①**hot exit**（dirty バッファ全文 + path/version/カーソル。WAL で kill -9 耐性）
  ②**スレッド永続化**（threads/turns テーブル。turn 毎 INSERT 追記 = JSON 全書き換えを避ける。ブラウズはページング）。追記の例外は composer の `!` で走らせたシェルの行（role `shell`・JSON 1 つ・#37）: 待機中なら走り出した時点で追記し、終わったら同じ行を `update_turn` で書き換える（途中で終了した行は復元で「中断」と読める）
  ③**使用量**（`turn_usage`・O11・2026-09-26: エージェントがターンの終わりに報告したトークンと、会話の累計コストの差分＝推定 USD を 1 ターン 1 行。Stats の日別集計と、再起動後に引き継いだ会話のコスト差分の基準に使う。旧「トークン台帳」`token_ledger` は `threads.tokens_used`＝文脈窓の使用量を並べるだけで累計ではなかったため削除。レート制限は保存しない＝エージェントが知らせた最後の値をメモリに持つだけ）
  ④**checkpoint のメタデータ**（turn→file→blob hash。blob 本体は content-addressed ファイル or DB — M12 着手時に比較）
- **隔離**: DB アクセスは薄い `storage` crate に閉じ込める（SQL を UI 層に漏らさない）。Turso はまだ若いので、問題が出たら rusqlite へ 1 crate の差し替えで退避できる面を保つ。書き込みは全て background executor（async API がそのまま「UI スレッドで塞がない」規律に合う）
- ⑥**変更レビューの注記**（`review_notes`・parity O7）: 1 注記 = 1 行（scope = TaskSpace id / 対象は JSON 1 列 + `target_kind` / 本文 / 状態 unsent・sent・resolved / sent_at）。対象を列に展開しないのは、Design Mode のページ要素など種類が増えても表を変えないため
- ⑤**窓セッション**（`window_sessions`・2026-09-03）: 1 窓 = 1 行（window_id / payload JSON = プロジェクト列 + 各プロジェクトの開タブ列 + アクティブ / closed_at）。各窓は自分の行だけを `WindowSessionWriter`（background の合流書き・順序保証）で更新し、起動時は生存中の全行を窓として復元（無ければ最後に閉じた 1 行）。ユーザーが窓を閉じたら `closed_at`（⌘Q では付けない）。⌘Q 直前は最新 payload を同期保存して background 書き込みの取りこぼしを防ぐ。旧 `state.json` は**廃止・互換読み込みも無し**
- **キャッシュ（捨ててよい・真実ではない）**: `external_agents/registry/registry.json` = ACP 公開レジストリの写し（`paths::acp_registry_cache`）。消えても組み込みカタログで動く

### 7.1 エージェントの版はどこから来るか（2026-09-02）

**解決順は「設定 → 公開レジストリ → 組み込みカタログ」**（`AgentKind::resolve_command`）。
`const AGENTS` に版を焼き込むだけだと、エージェントを 1 つ上げるたびに necoder のリリースが要る
（実際 codex-acp は 1.1.14 のまま止まり upstream は 1.8 まで進んでいた）。

- **設定** = `settings.json` の `agent_servers.<id>`。`{"type":"custom","command":…,"args":[],"env":{}}`
  で起動を丸ごと差し替え、`{"type":"registry","env":{}}` で env だけ足す。**command を持つのは custom だけ** —
  レジストリ管理のコマンドを半端に差し替えて版と食い違う状態を作らせない
- **レジストリ** = ACP プロジェクトの公開 CDN（ベンダー中立）。起動時はキャッシュだけ読み、
  ネットワークは起動 12 秒後に背景で後追い（1 時間スロットル）。**走行中のスレッドには適用しない**
- **組み込みカタログ** = `const AGENTS`。necoder が検証した既定値で、オフライン・未登録時の土台
- npm 指定は**完全一致ピンにしない**（`pkg@0.0.0 - X` の上限範囲）。npm の `min-release-age` 環境で
  公開直後の版が入らなくなるため。詳細と Zed 比較の境界は `docs/research/acp-agent-registry-notes.md`

**npm の ACP アダプタの管理導入（`acp_client::install`・2026-09-19）**: 組み込みのエージェントの ACP アダプタが npm の
固定版（`name@x.y.z`・レジストリの npx 配布で引数なし、無ければカタログの `package`）なら、最初の起動（送信）の時に
`<data>/external_agents/npm/<name@x.y.z>/` へ `npm install --save-exact` で置き（背景の blocking・180 秒で諦める）、
manifest の名前・版・`bin` を確かめてから rename で公開し、以後は `node <entry>` で直接起こす（`npm exec` の親を
持たない）。新しい版を置いたら同じパッケージの古い版を消す（1 版 260MB 級）。置けなければ従来の `npx -y` で起こす
（上の範囲指定のまま）。手元だけ — SSH・Windows・設定のコマンド・範囲指定の版は従来の経路。

**置き方の分担（parity 統合・2026-09-27）**: 起動の解決は 1 本（`acp_client::Agent::resolve_command_on`）で、置き場を
持つのは 2 つ。組み込みの npm アダプタ = `install`（`external_agents/npm/`・送信時に `run_session_on` が導入）、足した
エージェントの binary = `deploy`（`external_agents/binary/`・下の H2-b・送信時に背景で）。足したエージェントの npx / uvx は
置き場を持たず、その道具をそのまま起こす。置き場は別のフォルダで、片方の掃除がもう片方に触れない
（`install` の古い版の掃除は `npm/` の同じパッケージだけ、`deploy::remove_deployed` は `binary/<id>` の中だけ）。

**組み込みの 7 件の外のエージェント（H1・issue #38・2026-09-27）**: `agent_servers` のキーが組み込みの id
（`claude` / `codex` …）でなければ、上書きではなく**新しいエージェント**として一覧に並ぶ。
`{"name": "DeepSeek Harness", "command": "dsh-acp", "args": [], "env": {}}`（`type` は省略可・`command` が
あれば `custom`）。`name` が無ければ id を出す。組み込みの id では今までどおり上書きで、`name` は読まない。

- **一覧は設定の写し**: `settings::SettingsGlobal` が store を差し替えるたびに `acp_client::AgentCatalog` を
  作り直す（`settings::agent_catalog(cx)`）。acp_client は設定のスキーマを知らないので、写すのは settings
  （`custom_agent_specs`）。プロセス全体の可変な置き場は作らない（窓・テストごとの App が自分の一覧を持つ）
- **スレッドは相手を表示名で覚える**（DB の `threads.agent`）ので、表示名は組み込み・他の足した物と
  重ならないようにずらす（`名前 (id)`）。引く時は表示名 → id の順（名前を後から変えても前のスレッドが
  相手を見失わない）。`disabled_agents` / `agent_config_defaults` は id で引く
- 組み込みと足した物を 1 つの型で扱う入口は `acp_client::Agent`（`Builtin(&AgentKind)` / `Custom`）。
  oneshot（題名づけ）・再ログインの案内・レジストリの版の解決・ログインの確かめは組み込みだけ
- 起動は書いたコマンドをそのまま組む（PATH で探し、無ければ書いたまま渡す＝起動の失敗として原因が見える）。
  リモートは探索をリモートに任せる

**レジストリの全件から足す（H2・2026-09-27）**: 設定の「エージェントを追加」がレジストリの全件（キャッシュの写し）を
並べ、選ぶと `agent_servers.<レジストリの id>` に `{"type": "registry", "name": "<レジストリの名前>"}` を書く
（**新しいキーは作らない** — `type: registry` の意味は「起動はレジストリ・env だけ足す」のままで、キーが
組み込みでなければそのレジストリの項目を足す、になるだけ）。`name` を書いておくのは、スレッドが表示名で
相手を覚えるので、レジストリのキャッシュの有無で名前が変わらないようにするため。

- 起動の配布は **binary（このマシンの `<os>-<arch>` に完全一致）→ npx → uvx** の順で選ぶ
  （`RegistryAgent::launch_for`）。リモートは binary を使わない（手元の配布の形なので）。npx は `npx -y
  <pkg@0.0.0 - 版> <args>`、uvx は `uvx <package> <args>`（uv がある機械だけ。無ければ `LaunchError::NeedsUv`
  で「uv が要る」と案内し、**necoder は uv を入れない**）、binary は下の配備
- 組み込みのエージェントのレジストリの項目（`claude-acp` 等）は足させない（同じエージェントが別の起動で
  二重に並ぶ）。組み込みの起動（PATH / npx の版の解決）は変えない — binary / uvx の配備は足した物だけ
- 起動できない理由は `acp_client::LaunchError`（レジストリに無い・このマシンの配布が無い・node が要る…）で
  型のまま UI へ渡し、設定の行と transcript の失敗の文で言葉にする（acp_client は i18n を持たない）
- 設定の画面・追加の画面は**キャッシュを読むだけ**。取りに行くのは人が「取得する」「取り直す」を押した時と、
  既存の起動 12 秒後の背景の後追い（1 時間スロットル）だけ

**状態と版を読むだけの層（#38 H2 の見せ方・2026-10-02・`acp_client::readiness`）**: 設定 › AI エージェントの行の
「このホストで動くか（使える / 足りない物）」と「版（今の版とレジストリの版）」は、起動の解決と同じ順で**読むだけ**で決める
（起動の振る舞いは変えない・CLI を子プロセスで起こさない・資格情報の中身は読まない）。

- **事実**（`HostFacts`）と**見立て**（`assess`）を分ける。見立ては純関数で、このマシンでも SSH 先でも同じ
- このマシンの事実 = `local_facts`（PATH・Zed の npx キャッシュ・`install` の置き場の版・npx のキャッシュの版・`deploy` の
  印の版・ログインの跡のファイルと環境変数の名前）。読み取りの口は `install::installed_versions`・`deploy::deployed_versions`・
  `registry::split_npm_spec` に足した（どれも読むだけ）。ログインの跡の表は `AgentKind::login_traces` 1 本で、
  `configured_auth_state`（このマシン）と SSH 先の見立てが同じ表を見る
- SSH 先の事実 = `probe_remote`。workspace が渡した「この窓で開いている SSH のプロジェクトの接続とその根」
  （`settings::AgentHost`）で、`sh -lc` の読み取りだけのシェルを 1 回流す（`run_command_retry_safe`・
  `command -v` / `readlink -f` で辿った `package.json` / npx のキャッシュ / `test -e` / `printenv`）。新しく SSH を
  張らない。シェルは手元の `sh` と dash で本当に流すテストで固定している
- 「使わない」にできない理由（既定・Captain・使う最後の 1 つ）は `settings::agents_page::disable_refusal` 1 本で、
  スイッチと「外す」が書く前に確かめる

**binary の配備（H2-b・`acp_client::deploy`）— 外から落とした実行ファイルを走らせるので、ここを固定する**:

- **落とす元**: レジストリの JSON の `distribution.binary.<os>-<arch>.archive` の URL **だけ**。necoder は URL を
  組み立てない・書き換えない。`curl --proto =https --proto-redir =https`（リダイレクト先も https だけ）。
  https でない URL は落とさない。止まった転送（1 KB/s 未満が 60 秒）は諦める
- **置き場**: `<data>/external_agents/binary/<id>/<version>/<os>-<arch>/`（`deploy::binary_root`）。id・版・キーは
  英数字と `._-+` だけ（置き場の外を指す名前を通さない）。版ごとに別のフォルダ＝更新で走っている版を書き換えない。
  同じ版が置いてあれば落とし直さない（キャッシュ）。新しい版を置いたら 1 つ前の版だけ残して古い版を消す
- **検証**: レジストリに `sha256` があれば、落とした書庫の sha256（`sha2` crate・64 KB ずつ読む）と照合し、
  違えば展開せずに捨てる（代わりの版も使わない）。無い物は照合できない — 追加の画面・設定の行に「検証の値が
  ありません」、起動時に transcript へ 1 行、完了の印（`.necoder-deployed.json` の `verified`）に残す
- **展開**: 置き場の隣の一時フォルダ（`.staging-<pid>-<時刻>`）に落として、先に中身の名前を確かめる（絶対パス・
  ドライブ名・`..` を含む書庫は展開しない）→ OS の `tar`（macOS / Windows の bsdtar は zip も）、Linux の zip だけ
  `unzip` → 起動する `cmd` が置き場の中にあるか確かめて実行の権限を付ける → 印を書いて rename で公開。
  印の無いフォルダは「置いていない」。書庫でない実行ファイル（拡張子なし・`.exe`）は `cmd` の名前で置く
- **いつ落とすか**: 最初の起動（送信）の時に**背景で**（`Agent::needs_deploy` → `Agent::deploy_command`）。UI
  スレッドの解決では落とさない。先張りはしない（見ただけのタブで黙って落とさない）。落としている間は
  transcript に 1 行。新しい版を**落とせなかった**時だけ手元の一番新しい別の版で起こし、そう知らせる
- **外す**（設定の「外す」・2026-09-27）: `external_agents/binary/<id>` の中の **necoder が置いた物だけ**を消す
  （`deploy::remove_deployed`: 完了の印のある `<version>/<os>-<arch>` と途中の `.staging-*` / `.download-*`。
  `<id>` 自体が symlink なら何もしない・中の symlink はリンクだけ消えて先は触らない・知らないファイルは残す）。
  消す直前に settings.json を読み直し、同じ id がまだ書かれていれば消さない（`agent_servers` は user の層
  だけから読む・§7.7）
- 同じアプリの中で同時に初回が起きても落とすのは 1 回（プロセスの中の lock）。別のプロセスが先に同じ版を
  置いたら、その完成品を使う

### 7.2 MCP サーバはクライアントが渡す（2026-09-10）

**ACP では「どの MCP サーバへ繋ぐか」を決めるのはクライアント（＝necoder）**。エージェント側の
設定ファイル（`~/.codex/config.toml` 等）はセッションに現れず、`session/new` / `session/load` の
`mcpServers` に載せた分だけがエージェントから見える。この受け渡しが無かった間は「Codex CLI に
登録したのに necoder のスレッドからは `not installed or connected`」になっていた（実測 2026-09-10）。

解決は `acp_client::mcp`（1 本の解決結果を設定画面とセッションが共有する）:

- **真実は `settings.json` の `mcp_servers.<name>`**。`command`（stdio）か `url`（http/sse）を書けば
  necoder 自身の定義＝**既定 on**。`{"enabled": true}` だけの行は「発見済みサーバの on/off」
- **発見** = `~/.codex/config.toml` の `[mcp_servers.*]` / `~/.claude.json` / `~/.cursor/mcp.json` の
  `mcpServers`。登録し直しを強いないための読み取りで、**既定 off**（他人の設定を根拠に子プロセスを
  起こしたり課金されるリモートサーバへ繋いだりしない・DECISIONS の該当項）
- 伝送方式の解釈は `transport_from_parts` の 1 箇所だけ（`type`/`transport` 明示 → 無ければ `url` の
  有無）。necoder の設定と他ツールの設定が**同じ書き方で同じ意味**になる
- **渡せなかったものは黙って落とさない** — 広告の無い伝送方式（`mcpCapabilities.http` / `.sse`）、
  リモートセッションの stdio（コマンドは接続元のパス）、未設定の `${VAR}` は `AgentEvent::Notice`
  として transcript に 1 行出す。`${VAR}` を展開できないまま送ると `Bearer ${TOKEN}` という嘘の
  ヘッダで繋ぎに行くことになるので、**そのサーバごと渡さない**
- 反映は**次に開くスレッド**から（走行中のセッションは繋ぎ直さない）

### 7.3 セッションは送信を待たずに張る（2026-09-10）

**ACP はセッションが開くまでモデル・思考量・権限モードを広告しない。** `session/new` の応答（と
直後の通知）に載って初めて一覧が分かるので、「初回送信でセッションを立てる」遅延起動のままだと、
composer 下のピルは最初の送信まで空で押せない（0.1.14 の実害）。実測でも `claude-agent-acp` は
**prompt を 1 通も送らない状態**で Model 5 種 / ThoughtLevel 6 種 / Mode 5 種を広告する
（`cargo run -p acp_client --example probe_advertisement`）。

そこで **先張り（prewarm）**: `AgentPanel::schedule_prewarm` が、宛先が決まった時・復元直後・
タブ切替・新規タブ・agent 切替で、**アクティブなスレッド 1 本だけ**のセッションを 500ms 後に立てる。

- **1 枚だけ**なのは necoder が「スレッド 1 本 = エージェントのプロセス 1 本」だから。Zed は
  `AgentConnectionStore` で **agent ごとに 1 接続を共有**するので画面を開いた分だけ繋いでよいが、
  necoder で同じことをすると idle メモリ予算（§8）が壊れる。Fleet の Task セル
  （`AgentPanel::new_task`）は並べただけで N 本立たないよう先張りしない
- **静かに失敗する**: ユーザーは何も頼んでいないので、未導入・起動失敗を transcript のエラーに
  しない（送信すれば同じ経路で必ず出る）。1 スレッド 1 回だけ試し、落ちた後の立て直しは
  composer 直上の「再開」に任せる（自動リトライで無限に立て直さない）
- **広告は agent 固有**（同じ実行ファイルなら誰に聞いても同じ）。受け取った一覧は
  `AgentPanel::catalog` に agent 別で控え、**まだセッションの無いタブのピル**もそこから選択肢と
  表示名を出す。そこで選んだ value_id は sticky に載り、次に開くセッションへ
  `SessionPreferences` で渡る（＝先に選んでから送れる）
- **抱える数に上限**（`MAX_IDLE_PREWARMED_SESSIONS` = 2）: 先張りしたまま 1 度も送っていない
  セッションは 2 本までで、超えたら古い順に畳む（送信路を捨てるだけ＝`HostProcess` の drop で
  プロセスも落ちる。`session_serial = 0` に戻して「意図して畳んだ」と印を付け、切断バナーを
  出さない）。実測（2026-09-11）: 未送信のセッションは adapter + 子で **~16MB**、使うと ~46MB、
  会話が育つと ~244MB。だから「見ただけのタブ」の分だけを刈る。畳んでもピルは在庫から出続ける
- **定期ポーリングはしない**。一覧の変化はセッション中に `ConfigOptionUpdate` で push される
- `settings.agent_prewarm`（既定 on）で off にできる＝ idle メモリを優先する選択肢を残す

**起動してすぐ落ちたエージェントの理由（2026-09-27）**: `host::HostProcess` は長寿命の子（ACP エージェント・
言語サーバ）の stderr を**末尾だけ**メモリに持つ（`host::StderrTail`・最後の 40 行・1 行 1000 バイト・読む側は
最後まで読み続けて古い行から捨てる＝子は詰まらない）。以前は `Stdio::null()` で捨てていたので、すぐ落ちた
エージェントは「ACP セッションが異常終了 … Broken pipe」としか分からなかった。`run_session_on` はセッションを
開く前に失敗し、子が終わっていたら（最長 1.5 秒待つ）`acp_client::AgentExited`（終了コード・stderr の末尾）で
戻り、同じ物を `AgentEvent::ExitedAtStartup` で流す。

- **stderr は画面だけ**: stderr には秘密が混ざり得る。`AgentExited` の `Display`（ログ・保存される transcript に
  載る側）には stderr を入れず、UI は composer の上のカードにだけ出す（`Thread.startup_exit`・永続化しない）。
  カードに出す前に、エージェントへ渡した env のうち秘密らしい名前の値と、よく知られた鍵の形を `••••` に伏せる
  （`acp_client::mask_secrets`。取りこぼし得るので「画面だけ」と組にしている）
- 手元のエージェントには「SSH 切断など」と言わない（セッション断のバナーと transcript の 1 行を host で出し分ける）

### 7.4 Fleet の描画の持ち場（作業面は 2026-09-20 に削除）

Fleet 中央の**作業**タブ（`FleetCenterView::Work` / `workbench.rs` / `work_layout.rs`）は 2026-09-11 に既定から降格、
2026-09-16（FLEET-V2 F3）で描画経路から外れ、**2026-09-20（F7）でコードごと削除した**。残したのは 2 点だけ:

- **端末の名札**: Task 内ターミナル（`FleetPane::Shell { id }`）の `id` は necoder が採番する名札で、PTY そのものではない。
  `TerminalDock` が `detached: BTreeMap<u64, Entity<TerminalView>>` で名札 → 実体を持つ。採番は
  `chrome.next_terminal_id`（プロセス内で単調増加・閉じた番号を使い回さない）。PTY は再起動を越えないので保存しない。
- **窓の状態の復元**: `Workspace::restore_window_state`（`persisted_state` の対）。Fleet / Chat のどちらで閉じたか・
  左ドック幅だけを戻す。保存形式から `work_layout` / `fleet_view` は外した（旧 payload の余分なキーは serde が無視する）。

Fleet の描画は `fleet_view.rs`（枠・系譜ヘッダ・編隊図の 4 表示・下段）+ `fleet_stage.rs`（舞台 / Task カード /
スレッドタブ行 / 見出しのトグル / サイドペイン / ブリッジ）+ `fleet_sidebar.rs`（Task 一覧・要対応・Captain バー）+ `new_task_dialog.rs`
（＋Task）+ `captain.rs`（wake・任命の面・采配ログ）の 5 枚。**同じ Entity を 1 フレームに 2 回描かない**のは舞台側の責任
（1 Task = 1 カード・会話ペインは 1 枚・`AgentPanel` の自前タブ行は `sync_embedded_panels` が Fleet 中だけ畳む）。
周りに parity の部品（2026-09-26〜27）: `task_creation.rs`（作成中の行・取り消し・やり直し・O20。＋Task・fan-out・
Captain の分解案の承認が同じ流れを通る）・`captain_proposals.rs`（分解案のカードと承認・FLEET-V2 §5.5）・
`task_details.rs`（詳細…・O21）・`cleanup.rs`（片付けの画面・O22）・`review_controller.rs`（変更レビュー・O6）。
「変更」のサイドペインは session に 1 枚の `review_view::ReviewView` をそのまま描く（Editor のタブにも同じ Entity を
出すが、Fleet と Editor は同じフレームに描かない）。

### 7.5 1 worktree に ACP は何本でも（Fleet グリッドの既定・2026-09-11）

Fleet 中央の既定（系譜グラフ＋セルのグリッド・UI-SPEC §6.1）では、同じ作業ディレクトリに
**独立した ACP と端末をいくつでも並べられる**。ここで守る契約は 2 つ:

**① 実体は `ProjectSession` が所有し、セルは参照しか持たない。**
`fleet_agents: Vec<Entity<AgentPanel>>` が全パネル（先頭 = その worktree を開いたときの初期パネル）を
持ち、`agent_panel` は**いま操作している 1 枚**を指すだけの別名。`FleetPane::Agent { space, panel }` /
`Shell { space, id }` はセル側の見え方で、**セルを閉じても実体は消えない**（会話も PTY も走り続け、
herd から同じ実体へ戻せる）。配置替え・拡大でも作り直さない。端末は §7.4 の名札方式
（`TerminalDock` が `id` → Entity を持つ）。

**② 横断で読むときは `ProjectSession::agent_statuses` を通す。**
`(panel, thread_index, status)` の三つ組を返し、herd / 管制キュー / ニュース / 統計 / IPC digest /
`remote_snapshot` は全部ここから読む。理由は 2 つ:

- **thread 添字はパネル内でのみ一意**。外へ渡すときは `(panel, thread)` の対で運ぶか、thread ID で
  引き直す（`remote_thread` は `AgentPanel::contains_thread` で持ち主を引く＝スマホから選択外の
  ペインも指せる）。添字だけを渡す API は「たまたまいま選ばれているパネル」を見る壊れ方をする
- `session.agent_panel` を直接読む箇所は 1 対多にした瞬間に全部バグる。集約関数へ寄せるまでは
  「直したつもりで直っていない」状態が続く（9 ファイルに散っていた・JOURNAL 2026-09-11）

**`RunningRegistry`（全窓横断のスレッド台帳）は panel ごとの行を保持して root で集約する。**
root キーに直接 upsert すると、同じ root の 2 枚目の ACP が 1 枚目の行を消す。パネルの解放は
`cx.on_release` で自分の行だけ落とす。

### 7.6 実行中のターンへの差し込み（`_session/steering`・2026-10-02）

送信待ちの「今すぐ」（UI-SPEC §6 の送信待ち）は、エージェントが広告していれば**実行中のターンを止めずに**文を渡す。
以前は `session/cancel` でターンを畳んでから送り直しており、途中のツール作業を捨てていた。広告の無い
エージェントは今までどおり「中断 → 閉じたら先頭として送る」。

- **判定は initialize の実物から**: 応答の**最上位**の `_meta.steering.supported == true`（`agentCapabilities` の
  中ではない）。claude-agent-acp 0.81.2（`dist/acp-agent.js` の `initialize`・`STEER_METHOD`）と codex-acp 1.13.1
  （`dist/index.js` の `initialize`・`SESSION_STEERING_METHOD`）が同じ形で広告する（レジストリの今の版
  claude-agent-acp 0.84.0・codex-acp 2.1.0 もソース上は同じ契約）。`acp_client::steering_supported`
  が読み、`AgentEvent::SessionStarted.steerable` で UI へ渡す
- **線の形**: request `_session/steering` `{sessionId, prompt: ContentBlock[], _meta: {steering: {idleBehavior:
  "promptRequired"}}}` → `{outcome}`。`injected` = 走っているターンに入った / `promptRequired` = もうターンが
  無かった（文は**届いていない**）/ `startedNewTurn` = エージェントが自分で新しいターンを始めた。
  `SteerOutcome` は `Injected` / `TurnOver` / `StartedTurn` / `Refused(理由)`（エラー応答・知らない outcome）
- **`SessionCommand::Steer { id, text, images }`**: `run_session_on` は応答を**ターンの select の 1 本**として待つ
  （その場で await しない＝待つ間も更新は流れる）。待機中も同じ future を見張る（ターンの終わり際に送った分は
  応答がターンの後に来る）。1 セッション 1 本ずつで、後から来た分は前の応答を待つ。ターンが閉じた時に
  待っていた分は送らずに `TurnOver`。待機中に届いた `Steer` も送らずに `TurnOver`（＝普通の送信に回す）
- **select の順は 更新 → 差し込みの応答 → prompt 応答 → コマンド**。差し込みの応答を prompt 応答より先に
  読むのは、両方が同時に届いた時に「差し込んだ文」をターンの終わりより前の位置に置くため。prompt 応答を
  コマンドより先に読むのは、終端が手元に届いているのに `Steer` を終わったターンへ送らないため
- **UI の後始末は 1 か所（キュー）**: `TurnOver` と `Refused` は文を送信待ちの先頭へ戻す。`TurnOver` は
  ターンの終わりのフラッシュがそのまま次のターンとして送り、`Refused` は理由を 1 行出して「中断 → 送り直し」
  に落ちる。差し込みの応答を待つ間は、ターンが閉じても送信待ちを流さない（順序を守る）
- **ターンの終わりの判定は変えない**: ターンは `session/prompt` の応答（`stopReason`）でだけ閉じる。差し込みは
  新しいターンを作らない — claude-agent-acp は差し込みの後、ターンの決着を SDK の idle まで遅らせ
  （`Turn.steeredEchoes`）、codex-acp は Codex の `turn/steer` で同じターンに足す。どちらも差し込んだ文に
  答え終えてから prompt 応答を返す。zeron が Claude / Codex のアダプタを捨てた理由（背景の作業のために
  ターンを開いたままにして完了判定が狂う・`zeron/crates/harness/src/lib.rs` 冒頭）に当たらないことは、
  実アダプタで確かめた（`cargo run -p acp_client --example probe_steering`・2026-10-02）: `sleep 6` を
  走らせている最中に差し込むと、claude-agent-acp 0.81.2（haiku）も codex-acp 1.13.1（gpt-5.6-luna）も
  すぐ `injected` を返し、**走っているシェルは止めずに最後まで走らせ**、終わってから差し込んだ文に答えて
  ターン 1 本のまま `Completed` で閉じた（ツールの終わりから Claude 1.6 秒・Codex 2.4 秒）。Claude は呼び出しを
  **書いている途中**に差し込むと、その生成を止めて（優先度 `now`）呼び出しを出し直す（書きかけの呼び出しは
  走らず、結果の更新も来ない）
- **既知の端: codex-acp 1.13.1 は `idleBehavior` を読まない**（`parseSessionSteerParams` は `sessionId` と
  `prompt` だけ）。Codex のターンが閉じた（`turn/completed`）直後〜prompt 応答がこちらに届くまでの数 ms に
  差し込みが着くと、Codex は自分で新しいターンを始める（`startedNewTurn`）。necoder は文を transcript に出し、
  返事は待機中の更新の道（目標の自走ターンと同じ）で届くが、そのターンの実行中・完了は示せない。
  残りの送信待ちは自動では流さない（codex-acp は自分で始めたターンの最中に `session/prompt` が来ると
  ターンの控えを上書きする）。次のターンの終わりか、人の送信で流れる。Claude は `promptRequired` を返すので
  この端は無い

### 7.7 設定の層と接続（issue #38 H3・2026-10-02）

**project 層はリポジトリが持つファイル**。設定は既定 → user → リポジトリの `.necoder/settings.json` を深く重ねるが、
clone しただけのリポジトリが「どこへ何を送るか・何を起こすか・何を聞かずに通すか」を決められると、キーチェーンの
キーを別の宛先へ送らせたり、任意のコマンドを起こさせたりできる。だから `settings_core::USER_ONLY_KEYS` を
project 層から**外してから**重ねる（`SettingsStore::load`。外したキーは `ignored_project_keys` に残し標準エラーに 1 行）:

| キー | リポジトリに決めさせると |
|---|---|
| `connections` / `agent_connections` | 接続の `base_url` を差し替えて、キーを別の宛先へ送らせる |
| `agent_servers` | 任意の起動コマンド・`ANTHROPIC_BASE_URL` などの env |
| `mcp_servers` | 任意の MCP のコマンド・URL（ヘッダの `${VAR}` は手元の env を展開する＝秘密を外へ送らせる） |
| `agent_permission_default` / `agent_config_defaults` | 「聞かずに進める」権限モードを既定にする |
| `allow_terminal_send` | CLI から端末へキーを打たせる |
| `terminal_shell` / `terminal_shell_args` | 端末を開くたびに任意のコマンド |
| `captain_agent` | 自分から起きて 1 ターン走る Captain の任命（人の明示操作に限る） |
| `claude_ai_connectors` | claude.ai のコネクタ（メール・カレンダー等）をセッションへ持ち込む |
| `chat` | 全チャットのシステムプロンプトへの追記と置き場（プロジェクトに紐づかない） |

読むのは見た目と、人が押すまで何も起きない物（`quick_commands` は押すと端末で走るが、押すまで何も起きず中身は
ツールチップで見える）。書き手（`persist_*`）は元から user のファイルだけを書く。

**接続 = エージェントがモデルの API を呼ぶ口と、その契約**（GLM Coding Plan・Kimi Code・DeepSeek API …）:

- **置き場**: 宛先（ひな形・名前・形式・ベース URL）は `connections.<id>`、エージェントごとの選択は
  `agent_connections.<agent id>`（どちらも user の層だけ）。**API キーは OS のキーチェーン**
  （`acp_client::connections::secrets`・項目名 `necoder.connection.<id>`・macOS = ログインのキーチェーン /
  Windows = 資格情報マネージャ / Linux は未対応で保存できない旨を出す）。settings.json にキーの欄は無い。
  **necoder 自身はこのキーで API を呼ばない** — エージェントを起こす時に渡すだけ（issue #38 §5-1）
- **キーの読み書きは背景のスレッド**（macOS は許可のダイアログで止まりうる）。設定の面のキーの有無は中身を読まずに
  項目の属性だけで引く（`SecretStore::contains`・ダイアログを出さない）。中身を読むのはセッションを起こす時だけ。
  キーの置き場はアプリの起動（`settings::install_os_keychain`）だけが OS のキーチェーンにする — 置かれていなければ
  メモリだけの置き場（テストと隔離した offscreen は本物のキーチェーンに触れない。debug ビルドの
  `NECODER_SECRET_STORE=memory[:id,…]` も同じ）
- **渡し方**（`connections::Harness`・渡し方の分かるエージェントだけ）:
  - Claude Code（claude-agent-acp）: Anthropic 互換だけ。`initialize` の `agentCapabilities.providers` を見て、
    広告していれば `session/new` / `session/load` の**前に** `providers/set`（`providerId: "main"`・
    `apiType: "anthropic"`・キーは `Authorization: Bearer`、`x-api-key` の口は両方に載せる）。Rust の ACP crate 1.3 は
    この要求を持たないので、型はスキーマ crate（feature `unstable_llm_providers`）から引いて `UntypedMessage` で送る。
    アダプタはこの値を Claude Code の env と**設定**の両方へ入れる（リポジトリの `.claude/settings.json` の env でも
    上書きできない）。プロセス単位の設定で、necoder はスレッド 1 本 = プロセス 1 本なので混ざらない。
    **広告しない古いアダプタ**は、セッションを開かずにプロセスを畳み、`ANTHROPIC_BASE_URL` /
    `ANTHROPIC_AUTH_TOKEN`（`x-api-key` の口は `ANTHROPIC_API_KEY`）を足して 1 回だけ起こし直す。この時は他の
    宛先と資格情報の変数（`ANTHROPIC_API_KEY`・`CLAUDE_CODE_OAUTH_TOKEN` …）を空にしてから入れる（手元のキーを
    別の会社へ送らない・アダプタの `providers/set` と同じ顔ぶれ）
  - DeepSeek Harness（足したエージェントのコマンドが `dsh-acp`）: dsh の DeepSeek の経路は Anthropic Messages の口で、
    `OPENAI_BASE_URL` / `ANTHROPIC_BASE_URL` を読まず `providers/set` も持たない。DeepSeek API の接続だけを
    `DSH_PROVIDER=deepseek` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_API_KEY` で渡す
  - OpenCode: ひな形が OpenCode の組み込みのプロバイダ（models.dev の id）に当たる時だけ、そのプロバイダの環境変数で
    キーを渡し、`OPENCODE_CONFIG_CONTENT`（リポジトリの `opencode.json` より強い）の `enabled_providers` でその
    プロバイダだけにする。宛先は OpenCode の既定の口なので、ベース URL を書き換えた接続は渡さない
  - それ以外（Codex・Copilot・Qwen Code・Kimi CLI・Grok Build・他の足したエージェント）は渡さない。Pi は別 Task
- **env の積み順**: task.env → `agent_servers` の env → プリセット（Chat・席）→ **接続**（最後＝どれにも上書きさせない）。
  接続を渡すセッションでは、リポジトリの `.necoder/task.env` から宛先・プロキシ・証明書・差し込むコードを変える変数
  （`ANTHROPIC_*` / `OPENAI_*` / `OPENCODE_*` / `HTTPS_PROXY` / `NODE_OPTIONS` / `NODE_EXTRA_CA_CERTS` / `DYLD_*` …・
  `connections::guarded_from_task_env`）を読まず、読まなかった名前を transcript に 1 行出す
- **渡せない時は起こさない**: 選んだ接続をそのエージェントが受け付けない・キーがキーチェーンに無い・キーチェーンを読めない
  時は、黙って自分のログインで走らせず（別の契約に請求させない）、理由を transcript に出す
- **リモート（SSH 先で起こすエージェント）には渡さない**（キーを手元の外へ出さない・H3 の範囲外）。そのスレッドは
  SSH 先のエージェント自身のログインで動き、エージェントごとに一度だけ transcript で知らせる
- **効くのは次に起動するセッションから**（アカウントの切り替えと同じ）。ただし先張りしたまま 1 度も使っていない
  セッションは、設定が変わって鍵が変わったら畳んで張り直す（前の接続のまま最初の送信が走らない）
- **見せ方**: composer の設定のチップとカードのエージェントの行に `· 接続の名前` を添える（Claude Code に他社の接続を挿しても中身を隠さない・§5-2。カードの在庫も [`StockKey`] で引く）。
  使用量の鍵（R08）にも接続を入れる（別の契約の値を混ぜない）

**モデル一覧の在庫の鍵**（`agent_panel::StockKey`）: 広告の在庫（モデル・思考量・権限モード）は今まで表示名だけで
引いていたが、接続やログインを替えると一覧が変わる。鍵 = 使用量の鍵（エージェント・動かしている場所・認証の置き場・
認証に関わる env の指紋・接続）+ ログインの指紋（`connections::login_fingerprint`: ログインと設定のファイルの更新時刻と
大きさ・Claude Code は `.claude.json` のアカウントの id。資格情報の中身は読まない。zeron の `model_context` と同じ
考え方を独立に実装）。セッションを立てた時に決めてスレッドに持たせ、まだセッションの無いタブは今の設定で立てた時の
鍵で引く（ログインの指紋は最後に確かめた値＝描画でファイルを見ない。ピルのメニューを開いた時に確かめ直す）。
スレッド自身の広告も、前のセッションの鍵が今の鍵と違えば出さない。実行ファイルの版は鍵に入れない（起こす前に
解決しないと分からない・版が変われば次のセッションの広告で入れ替わる）。slash コマンドと会話名の在庫は接続で
変わらないので表示名のまま（`HarnessStock`）。

## 8. 性能予算の測り方（目標: Zed 比 ~80%）

- `cargo bench` + 起動時間計測を `scripts/` に置き、一般的な「キー入力→フレーム提示」の
  ヒストグラムを necoder のイベント境界から計測する
- Zed の benchmark コードは取り込まず、比較する場合は同一端末・同一fixture・同一操作で外部から測る。
  予算超過は CI 的に検知する（しきい値をスクリプトに埋める）
- UX 優先の明示判断（DECISIONS §8）: 予算内なら速度チューニングより UI-SPEC の完成度を優先する

## 9. Remote Host 境界（2026-07-13 確定）

Remote SSH を UI の条件分岐として足さない。local/SSH 共通の `Host` を foundation 層へ置き、
`project` / `search` / `lang` / `terminal` / `acp_client` は host の capability を使う。

```
workspace/view -> project model -> Host trait <- LocalHost / SshHost
                                      |
                         versioned RPC over system OpenSSH
                                      |
                           necoder-remote-server
```

- path identity は `(HostId, RemotePath)`。remote path を local `PathBuf` として OS API に渡さない。
- UI/tree-sitter/dirty backup/credential は local。FS/watcher/search/Git/LSP/PTY/task/ACP は remote。
- **Host は同期 trait ＝ UI スレッド（render・アクションハンドラ）から直接呼ばない**（2026-07-16 監査で規律化）。
  remote は 1 呼び出しが最大 `REQUEST_TIMEOUT`（30s）ブロックしうる。確立パターン
  （push/pull・gutter diff・横断検索と同じ `host.clone()` → `background_executor().spawn` → 前景で反映）に寄せる。
  render 内での FS/RPC 列挙は禁止 — ツリーが `slot.rows` にキャッシュするのと同型で、表示時に読んでキャッシュする。
- 再接続（`host::ReconnectingClient`・2026-09-07 現在）: heartbeat は 5s ごとに Ping（timeout 5s）。
  時間切れでも受信バイトが進んでいれば「混んでいるだけ」と見て切らない（低速回線の大きな転送を巻き添えにしない）。
  張り直しが決まったら古い接続に乗っていた request を即失敗させ session プロセスを落とす（待たせると各自の timeout まで帰らない）。
  ControlMaster の作り直しは Hello が**時間切れ**のときだけ（即 EOF は remote-server 側の問題）。
  端末/LSP/ACP の session は `ControlMaster=auto` で起こし master 不在なら自分が master になる＝ launch spec を組む関数はネットワークに触らない。
  到達不能なホストでは 1 回の試行に十数秒かかるので、**待っている間に起きた失敗は待ち人が試し直さない**
  （到着より前の失敗＝ユーザーの新しい操作だけが繋ぎに行く）。切断中に操作するほど遅くなるのを防ぐ。
  実 SSH の回帰は `crates/host/tests/remote_ssh_live.rs`（`scripts/test-remote-ssh-docker.sh` が
  Linux artifact の用意から container の凍結注入まで面倒を見る）。
- 接続状態の公開（2026-09-08）: `Host::connection_state()`（atomic 読み・I/O 無し＝ UI スレッド可）/
  `Host::watch_connection()`（変化の購読・std mpsc）/ `Host::reconnect()`（背景で heartbeat を 1 回前倒し。
  生きていれば何もしない）。遷移は `ReconnectingClient` が一手に握る（Unconnected → Connecting → Connected / Disconnected）。
  workspace は host ごとに 1 本の pump（`remote_connection.rs`）で notify を受け、statusbar の SSH チップが色と
  「再接続」チップを出す。**SSH セッションに乗るプロセス（ACP/LSP/PTY）は自動再接続の外**: ssh の子が
  落ちると stdout が EOF になり、そのプロセスは消える。ACP は `acp_client` が EOF（`is_incoming_transport_closed` /
  待機中は `incoming_closed`）を見てセッションを畳み（`AgentEvent::SessionLost`）、次の送信で立ち上げ直す。
  待機中（ターンとターンの間）も `session/update` を読み、ターン中と同じ `handle_session_message` で捌く
  （コマンド一覧は `session/new` 直後、会話名はターン終了の数秒後に届く。読まずにいると次の prompt まで
  UI に出ない・O2）。`session/load` の再生は本文を捨て、状態（コマンド一覧・会話名・目標）だけ流す。
  会話は `session/load`（エージェントが `loadSession` を広告するとき・id は `storage.thread_sessions`）で引き継ぐ。
  例外はエージェント側の過去の会話を開いた時（O15・`SessionPreferences::replay_history`）で、再生を
  `acp_client::history::ReplayLog` が発話・本文・ツールへ畳み、load 成功後に `AgentEvent::HistoryReplayed` で 1 回だけ
  渡す（live のイベントとしては流さない＝二重に載らない）。一覧（`session/list`）は `history::list_sessions_on` が
  **一覧のためだけにエージェントを 1 本**起こして読む（`session/new` も prompt もしない）。どちらも `Host` 越しなので、
  SSH のプロジェクトではリモートのエージェント（＝リモートのアカウントの会話）に訊く。「新しいセッションで続ける」で
  忘れた会話 id・引き継げずに替わった id は `storage.thread_past_sessions` に残し、履歴の重複除けに使う。
  LSP/PTY の同種の再 spawn は未着手（ROADMAP M9 残件）。
- SSH は system binary + ControlMaster。認証・known_hosts・ProxyJump を再実装しない。
  GUI 起動の ssh には TTY が無いので、パスワード / passphrase / host key 確認だけは
  `SSH_ASKPASS` に necoder 自身を指して入力欄へ中継する（`host::install_askpass` →
  `control_ipc` の `askpass` → `remote_ssh::AskpassPrompt`）。秘密を要求できるのは
  その ssh のために発行した token を持つ子プロセスだけで、秘密は保存しない。
- server は単一 static binary、client と protocol/version を handshake、daemon + proxy で再接続可能にする。
- wire は length-prefixed typed header + raw body。初版は request id/capability/frame limit を持ち、
  stream/event/cancel は watch・PTY の protocol 化と同時に追加する。
- 途中で用が無くなる読み取り専用の command（変更レビューの `git diff` 等・`--no-optional-locks`）は
  `Host::run_command_cancellable(spec, cancel)`（2026-09-27・R02）: local は子の出力を待つ間に印を見て、
  立ったら子を止める。remote は 1 往復の途中で止める口が無いので、既定の実装が最後まで走らせてから結果を
  捨てる（呼ぶ側は次の区切りで止まる。stream/cancel の protocol 化と一緒に直す）。
- 人が打ったシェルコマンド（エージェントパネルの `!`・#37）は `Host::run_user_command(spec, cancel, output_limit)`
  （2026-09-27）: `run_command_cancellable` と違い**副作用のある command 用**で、止めるのは人の操作だけ。出力は
  stream ごとに頭と尻を上限まで持ち、間は読み捨てる（子は止めない）。local は子を自分のプロセスグループで起こし、
  止める時はグループごと SIGTERM → 2 秒で SIGKILL。`pre_exec` で**シグナルのマスクと無視を既定へ戻す** —
  背景の executor のスレッド（macOS は GCD）は非同期のシグナルをブロックしていて、std の `Command` はマスクを
  子へ継ぐ（戻さないと子に SIGTERM が届かず、止めるたびに SIGKILL の猶予を待つ）。remote は既定の実装
  （1 往復で最後まで走らせて丸める・`can_stop_user_command` = false＝呼ぶ側は待たずに「中断」として畳む）。
- local implementation を先に `Host` へ移し、既存機能の回帰 test 後に SSH implementation を挿す。
- security/performance/reliability の受入条件は
  [`research/remote-ssh-2026.md`](./research/remote-ssh-2026.md) を正とする。
