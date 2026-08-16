# ARCHITECTURE — 実装設計図

目的: この文書 + [`UI-SPEC.md`](./UI-SPEC.md) + [`ROADMAP.md`](./ROADMAP.md) + [`../FEATURES.md`](../FEATURES.md) + `mock/index.html` だけで、
新しいエージェントセッションが**質問なしで**実装を進められる状態を保つ。乖離が出たらコードではなくまずこの文書群を直す。

## 1. 層構造と依存方向（鉄則）

```
[shell]      shirushi(bin) ─ 結線・起動・メニュー
[shell]      workspace ─ レール / chrome / active ProjectSession の合成・event routing
[view]       editor_view / explorer / git_ui / search_ui / agent_panel / terminal_view / settings / graph_view(M14)
[model]      editor_core / project / acp_client / search / lang / storage
[foundation] ui(部品+Registry) / theme_core / settings_core / keymap_core / i18n
[外部]       gpui(path=zed/) / agent-client-protocol(crates.io) / ropey / alacritty_terminal
```

依存の向き（Zed と同じ）。違反 import を見つけたら実装でなく設計を疑う:
- `editor_core` は **GPUI を知らない**（純データ + ロジック。テストが最速で回る層）
- view 層は `workspace` を知らない（workspace が view を「載せる」。逆はない）
- `ui` は `theme_core` / `i18n` のみに依存
- AI・Git・ターミナルを editor_core が import したら誤り（Zed の editor が vim/collab/agent を知らないのと同型）

`workspace` の具体的な ownership と source 境界は
[`REFACTOR-WORKSPACE.md`](./REFACTOR-WORKSPACE.md) を正とする。

## 2. crate 対応表（作る順・移植元・ライセンス備考）

| crate | 中身 | 出自 | 時期 |
|---|---|---|---|
| `shirushi` (bin) | 結線・起動 | 済（骨組み） | M1 ✓ |
| `theme_core` | トークン構造体・dark/light・ProjectIdentity/ThreadColor | UI-SPEC §1 を型に写す | M2 |
| `i18n` | `t!` マクロ・ja/en YAML 同梱 | 自作（薄い。§6。rust-i18n は crate 局所で不適） | M2 ✓ |
| `editor_core` | Buffer(ropey)・Selection・Transaction/undo | ropey。zed `text` は**参考のみ**（CRDT 不採用の決定済み） | M2 |
| `editor_view` | 行仮想化描画・gutter・キャレット・IME | zed `editor` の element 実装を参考（GPL 移植可） | M2 |
| `settings_core` | default→user→project 3層マージ・`.shirushi/`・監視・スキーマ | zed `settings` を**削って移植** | M3 |
| `keymap_core` | JSON keymap・コンテキスト述語 | gpui の keymap 機構 + zed 参考 | M3 |
| `ui` | Button/List/Picker/Modal + **Registry 群**（§4） | zed `ui`/`picker` 参考に新規 | M3 |
| `workspace` | レール・ドック・ペイン・タブ・statusbar・状態永続化 | zed `workspace` を**大幅に削って移植** | M3 |
| `project` | fs 抽象・worktree（走査/監視/gitignore/git status） | **zed `fs`+`worktree` をほぼそのまま移植** | M3 |
| `acp_client` | ACP セッション・アダプタ導入/起動 | **crates.io `agent-client-protocol`** + zed `agent_servers`(6k行) 移植 | M4 |
| `agent_panel` | スレッド色タブ・宛先チップ・transcript・composer | zed `acp_thread`(14k行) 移植 + UI-SPEC §6 | M4 |
| `explorer` | ツリー/カラム/アイコン 3ビュー・ファイル操作・右クリック | ビューは新規、モデルは `project` | M5 |
| `search` | バッファ内 / ripgrep 横断 | zed `search` 参考 | M6 |
| `lang` | tree-sitter ハイライト・LSP クライアント | zed `language`/`lsp` を削って移植 | M7 |
| `git_ui` / `terminal_view` | gutter diff / 統合ターミナル | zed `buffer_diff` / `terminal` 移植 | M8 |
| `graph_view` | 系譜グラフ（worktree×commit の DAG・custom Element・4表示） | 新規（editor_view と同型・データは `project` の git log・webview 不使用） | M14 |

移植の作法: ファイル冒頭に `// Ported from zed crates/<name> (GPL-3.0-or-later, 2026-07 時点のソース)`。
collab / CRDT / テレメトリの経路は移植時に**落とす**。Remote SSH は 2026-07-13 に方針変更し、
Zed の GPL コードを直接移植せず [`research/remote-ssh-2026.md`](./research/remote-ssh-2026.md) の
`Host` 境界として独立実装する（将来の Apache-2.0 化の道を閉じない）。設定キーは Shirushi の体系に改名。

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
  永続化は `ProjectSlot.open_files: Vec<PathBuf>` + `active_file`（プロジェクト単位でタブ列を復元）。
  非アクティブプロジェクトのタブは**遅延復元**（レール切替時に開く）。LSP は didOpen/didClose をタブ開閉に追従、
  didChange は編集タブごと（`lsp_sent_versions: HashMap<Path, u64>` で誤スキップを防ぐ）。

```rust
// theme_core — 「きせかえ」の2軸: テーマ（面の配色）× プロジェクト色（識別）
pub struct Theme { /* UI-SPEC §1 のトークン表と 1:1 のフィールド */ }
pub enum ThemeSource { BuiltIn(&'static str), User(PathBuf) } // themes/*.json = トークン上書き JSON
impl Theme { pub fn load(source) -> Result<Theme>; }          // 欠けたキーは built-in にフォールバック
pub struct ProjectIdentity { pub color: Hsla, pub icon: IconSource } // .shirushi/settings.json > 手動 > 自動巡回
pub enum IconSource { Monogram(char), Emoji(String), Image(PathBuf) }
```
テーマセレクタ（ライブプレビュー付き・Zed 方式）は M3 の Picker 基盤に載せる。VSCode/Zed テーマのインポートは later（zed `theme_importer` 移植）。

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

**Fleet モード（M14・UI-SPEC §11）**: 主単位は thread ではなく `TaskSpace`（1 task = 1 branch = 1 linked worktree = 1 `ProjectSession`）。各 Task cell は通常 View と同じ完全な `Entity<AgentPanel>` を埋め込み、同一 Task の複数 Agent は panel 内 thread として所有する。既定 `+ Task` は常に worktree を隔離し、main は protected `IntegrationSpace`。Task lifecycle / Agent runtime / Git health は別軸で、永続 event ledger、read-only merge preview、明示 integration gate を通す。詳細と clean-room 境界は [FLEET-ARCHITECTURE.md](FLEET-ARCHITECTURE.md)。

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

- **ファイルが真実（DB に入れない）**: settings.json（user/project）・`.shirushi/todos.md`（M12 Todo ボード — ファイルであること自体が要件）・keymap.json・テーマ JSON。git が真実のもの（status/diff/blame）も入れない。検索索引も持たない（regex 走査が正 — DECISIONS §8）
- **ローカル DB（`~/Library/Application Support/Shirushi/shirushi.db`）**: [Turso](https://github.com/tursodatabase/turso)（SQLite の pure-Rust 再実装・MIT・async ネイティブ）を採用。用途は
  ①**hot exit**（dirty バッファ全文 + path/version/カーソル。WAL で kill -9 耐性）
  ②**スレッド永続化**（threads/turns テーブル。turn 毎 INSERT 追記 = JSON 全書き換えを避ける。ブラウズはページング）
  ③**トークン台帳**（turns の集計ビューでほぼ無料）
  ④**checkpoint のメタデータ**（turn→file→blob hash。blob 本体は content-addressed ファイル or DB — M12 着手時に比較）
- **隔離**: DB アクセスは薄い `storage` crate に閉じ込める（SQL を UI 層に漏らさない）。Turso はまだ若いので、問題が出たら rusqlite へ 1 crate の差し替えで退避できる面を保つ。書き込みは全て background executor（async API がそのまま「UI スレッドで塞がない」規律に合う）
- `state.json`（開タブ・レイアウト）は当面 JSON のまま。肥大したら shirushi.db へ統合

## 8. 性能予算の測り方（目標: Zed 比 ~80%）

- M2 で `zed/crates/editor_benchmarks` / `input_latency_ui` を移植し、`cargo bench` + 起動時間計測を `scripts/` に置く
- 計測は「キー入力→フレーム提示」のヒストグラム（input_latency_ui の方式）。予算超過は CI 的に検知（しきい値をスクリプトに埋める）
- UX 優先の明示判断（DECISIONS §8）: 予算内なら速度チューニングより UI-SPEC の完成度を優先する

## 9. Remote Host 境界（2026-07-13 確定）

Remote SSH を UI の条件分岐として足さない。local/SSH 共通の `Host` を foundation 層へ置き、
`project` / `search` / `lang` / `terminal` / `acp_client` は host の capability を使う。

```
workspace/view -> project model -> Host trait <- LocalHost / SshHost
                                      |
                         versioned RPC over system OpenSSH
                                      |
                           shirushi-remote-server
```

- path identity は `(HostId, RemotePath)`。remote path を local `PathBuf` として OS API に渡さない。
- UI/tree-sitter/dirty backup/credential は local。FS/watcher/search/Git/LSP/PTY/task/ACP は remote。
- **Host は同期 trait ＝ UI スレッド（render・アクションハンドラ）から直接呼ばない**（2026-07-16 監査で規律化）。
  remote は 1 呼び出しが最大 `REQUEST_TIMEOUT`（30s）ブロックしうる。確立パターン
  （push/pull・gutter diff・横断検索と同じ `host.clone()` → `background_executor().spawn` → 前景で反映）に寄せる。
  render 内での FS/RPC 列挙は禁止 — ツリーが `slot.rows` にキャッシュするのと同型で、表示時に読んでキャッシュする。
- SSH は system binary + ControlMaster。認証・known_hosts・ProxyJump を再実装しない。
- server は単一 static binary、client と protocol/version を handshake、daemon + proxy で再接続可能にする。
- wire は length-prefixed typed header + raw body。初版は request id/capability/frame limit を持ち、
  stream/event/cancel は watch・PTY の protocol 化と同時に追加する。
- local implementation を先に `Host` へ移し、既存機能の回帰 test 後に SSH implementation を挿す。
- security/performance/reliability の受入条件は
  [`research/remote-ssh-2026.md`](./research/remote-ssh-2026.md) を正とする。
