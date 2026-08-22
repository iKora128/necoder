# Workspace architecture refactor

作成 2026-07-20、完了 2026-07-20。対象は `crates/workspace` と、そこから本来の
所有者へ移した周辺機能。開始 checkpoint は `ca46f40`、最終監査は `d83578a` で固定した。

ステータス: **完了**。

## 0. 結果

開始時の `crates/workspace/src/workspace.rs` は約 13,000 行、`Workspace` は約 95 個の
直接フィールド、単一 `impl Workspace` は 300 メソッド超だった。完了時点では次の形になった。

- `workspace.rs`: 1,399 行（production body は 975 行、残りは完了条件を固定する test）
- `Workspace`: 8 直接フィールド
- `ProjectSessions`: project metadata、active index、長寿命 session の単一 owner
- `ProjectSession`: EditorArea と project 単位の child Entity / process / controller の owner
- `EditorArea`: tabs、pane、LSP、diagnostics、diff、navigation、hot exit の owner
- `explorer` / `git_ui` / `search_ui`: `workspace` 非依存 crate
- root `Render`: shell effect を effect-cycle 末尾へ送り、UI 合成中に Host / FS / Git / DB を呼ばない

ファイルを物理分割しただけではなく、project 切替時にも dirty buffer、Editor Entity、Agent
session、Terminal process を保持するライフサイクルへ変更した。

## 1. 不変条件

1. 既存機能と公開 API を維持する。公開型の移動中は旧パスから re-export する。
2. view crate は `workspace` を依存先にしない。child → shell は typed event、shell → child は公開メソッドで通信する。
3. `Host` / FS / Git / DB 呼び出しを `Render` から行わない。非同期処理と世代管理は controller が所有する。
4. feature 固有 state を `Workspace` の直接フィールドに置かない。
5. project 切替で dirty buffer、Editor Entity、Agent session、Terminal を破棄しない。
6. `actions!` の action id、i18n key、`state.json` の後方互換を維持する。
7. 各 checkpoint で対象 crate を検証し、最終的に workspace 全体を検証する。
8. 機械移動と所有権変更を小さい checkpoint に分ける。

## 2. 確定アーキテクチャ

```text
foundation/model
  host / storage / theme_core / settings_core / editor_core / project / search / lang
                                  ↑
feature entities
  editor_view / explorer / git_ui / search_ui / agent_panel / terminal_view / settings
                                  ↑
shell
  workspace
                                  ↑
composition
  necoder
```

### 2.1 Workspace / ProjectSessions / ProjectSession

```rust
pub struct Workspace {
    project_sessions: ProjectSessions,
    theme: Theme,
    focus_handle: FocusHandle,
    chrome: ChromeState,
    overlays: WorkspaceOverlays,
    notifications: NotificationCenter,
    persistence: WorkspacePersistence,
    updater: UpdateController,
}

struct ProjectSessions {
    projects: Vec<ProjectSlot>,
    active: usize,
    sessions: Vec<ProjectSession>,
}

pub struct ProjectSession {
    editor_area: EditorArea,
    explorer: Entity<Explorer>,
    git_panel: Entity<GitPanel>,
    search_panel: Option<Entity<SearchPanel>>,
    agent_panel: Entity<AgentPanel>,
    terminal_dock: Entity<TerminalDock>,
    todo_panel: Entity<TodoPanel>,
    repository: RepositoryController,
    // project 単位の watcher / agent routing / picker cache
}
```

通常の rail 切替は `ProjectSessions.active` だけを変更する。初回表示時だけ記録されたタブを遅延
復元し、既に loaded な session は `tabs.clear()` も process 再生成も行わない。ブランチ切替は同じ
project 内容を明示的に reload する別操作なので、通常切替とは分離している。

### 2.2 EditorArea

`EditorArea` は 1 project session の編集面を一括所有する。

- tabs / active tab / split / recently closed
- file open / close / save / navigation
- buffer search / transient diff tab
- LSP lifecycle / diagnostics / completion / hover
- format / rename / code actions / references / symbols / workspace edit
- diff / hunk / blame / inline edit
- dirty buffer の hot exit

`EditorArea` 自身は追加の `Entity` ではなく長寿命 aggregate とした。実際に描画・入力を持つ
`EditorView` は既に個別の `Entity` であり、aggregate まで二重に包むと session 内の同期 command が
すべて cross-entity update になるためである。ライフサイクル分離は `ProjectSession` の常駐で満たす。

LSP の JSON 応答型と server registry は `lang` にあり、EditorArea は typed value を扱う。

### 2.3 Feature Entity と shell adapter

| Entity / owner | 所有するもの | shell への主イベント |
|---|---|---|
| `explorer::ExplorerProject` / `Explorer` | tree cache、view mode、命名、context menu、選択 interaction | `OpenPath`, `FilesChanged`, `Focus` 契約 |
| `git_ui::GitPanel` | panel input、branch menu、repository snapshot、busy state | `RepositoryChanged`, `OpenDiff`, `OpenWorktree`, `Toast`, `StageHunk` 契約 |
| `search_ui::SearchPanel` | project search state / render / keyboard | `OpenMatch`, `Dismissed` |
| `terminal_view::TerminalDock` | terminal tabs、header、active process | `OpenPath`, `Dismissed` |
| `settings::SettingsView` | settings / agent setup / onboarding | `RunCommand`, `OnboardingCompleted` |
| `workspace::TodoPanel` | Todo board state / render | `SendToAgent`, `FilesChanged`, `Toast` |

当初案の「Explorer / Git の Render と操作をすべて feature crate へ移す」は監査時に改めた。
これらの callback は active `ProjectSlot`、rail、picker、通知、window を同時に操作するため、子 Render
へ押し込むと project model の複製か巨大な往復 event protocol が必要になる。そこで project 固有の
model / interaction state / typed event 契約は `explorer` / `git_ui`、ウィンドウと chrome を伴う実描画
callback は `explorer_view.rs` / `git_view.rs` の shell adapter、とするのを最終境界にした。

実際に child Entity から shell へ上がる Search / Agent / Terminal / Settings / Todo の通信はすべて
typed event である。Explorer / Git の現在の操作は shell adapter 内で完結するため child → shell 通信
ではなく、両 crate の event enum は将来 adapter を Dock API へ移す際の契約として定義・購読している。
Git snapshot と watcher は panel の開閉に依存せず、ProjectSession controller が一度読み各 consumer
へ配る。

### 2.4 Typed event と登録境界

```text
ProjectWatcher -> ProjectSession -> Explorer / RepositoryController / EditorArea
ExplorerEvent  -> PanelRegistry  -> Workspace -> EditorArea（adapter 移行用契約）
GitPanelEvent  -> PanelRegistry  -> Workspace -> EditorArea / notifications（同上）
AgentPanelEvent-> PanelRegistry  -> Workspace -> EditorArea / notifications
TerminalEvent  -> PanelRegistry  -> Workspace -> EditorArea
SettingsEvent  -> PanelRegistry  -> Workspace -> TerminalDock / chrome
```

`CommandRegistry` が palette action の一意な一覧を、`PanelRegistry` が child subscription を管理する。
Window を必要とする typed event の後処理は root Render 内で実行せず、effect-cycle 末尾に遅延する。

## 3. ソース構成

```text
crates/workspace/src/
  lib.rs                 public facade / re-export
  workspace.rs           shell type、action wiring、root composition、pure helpers
  project_session.rs     ProjectSlot / ProjectSessions / ProjectSession と生成・復元
  project_switch.rs      session 切替、Agent destination
  project_watcher.rs     project 単位の watch routing
  commands.rs            CommandRegistry
  panels.rs              PanelRegistry
  persistence.rs         state.json schema / compatibility
  rail.rs / rail_view.rs
  chrome.rs
  overlays.rs
  notifications.rs
  explorer_controller.rs / explorer_view.rs
  git_controller.rs / git_view.rs
  todo_panel.rs
  dev_probes.rs          debug build only API
  updater.rs
  editor_area/
    mod.rs
    tabs.rs
    language.rs
    diagnostics.rs
    overlays.rs
    diff.rs
    inline_edit.rs
    hot_exit.rs

crates/explorer/src/lib.rs
crates/git_ui/src/lib.rs
crates/search_ui/src/lib.rs
```

旧 `[lib] path = "src/workspace.rs"` は廃止し、`src/lib.rs` を facade にした。

## 4. Checkpoint

主要 checkpoint は次の順で作成した。

- `ca46f40`: refactor 前の機能変更を保存
- `8e17b66`〜`29d9e8d`: facade と persistence
- `7484098`〜`5aed851`: project / lang model 境界
- `ac09fa8`〜`793789e`: Terminal / Search / Explorer / Git / Todo / Settings Entity
- `e6a13ae`〜`4f01830`: 長寿命 ProjectSession と EditorArea ownership
- `ef76f25`〜`f486fe9`: Repository / CommandRegistry / PanelRegistry
- `d56718f`: dev probe を debug build へ隔離
- `c1aef93`: ProjectSessions owner を統合
- `32249c5`: shell effect を Render 外へ遅延
- `f47d63f`〜`7fe7f64`: session 型配置と切替不変条件を確定
- `3676ce3`: Render 用の remote host 表示情報を session metadata へ cache
- `d83578a`: 公開互換、session 往復、Render I/O、release、画面 QA の完了条件を test / probe 化

## 5. 検証

最終確認コマンド:

```bash
cargo check --workspace
cargo test --workspace
cargo check -p necoder --release
git diff --check
```

完了条件の実証:

- 開始 checkpoint `ca46f40` と rustdoc の公開項目一覧を比較し、公開 top-level item と `Workspace` method の
  削除が 0 件であることを確認した。`actions!` 定義も byte-for-byte 同一。
- `locales/ja.yml` / `locales/en.yml` は開始時から不変で、i18n unit / doc / parity test が通る。
- 旧 schema と現 schema の `state.json` を一時ファイルへ書き、公開 `load_state` /
  `load_saved_state` から復元する test が通る。
- 2 project を A → B → A と切り替え、A の dirty text と undo 履歴、および Agent / Explorer /
  Git / Todo / TerminalDock / TerminalView の Entity ID が保持され、B では別 ID になる GPUI test が通る。
  Terminal は deterministic test executor の制約から PTY-free fixture を使うが、実際の
  `TerminalDock` tab と `TerminalView` Entity の所有経路を検証する。
- local / remote の両 source で root を実際に `window.draw` し、全 `Host` method を数える audit host が
  Render 中 0 call である GPUI test が通る。
- `cargo tree -i workspace --edges normal` で `workspace` の consumer は composition crate `necoder`
  だけ。feature crate からの逆依存はない。
- `tabs.clear()` は初回復元 / 明示 reload のみ。通常 switch は loaded session を再生成しない。
- release の実 cfg に `debug_assertions` がなく、生成 `.rlib` に debug probe symbol がないことを確認した。
- screenshot build を隔離 DB / state で起動し、default、Explorer 3 view、Git、Search、Settings、Todo、
  Picker の 9 状態を 2560×1600 offscreen image で目視した。各起動ログに error はない。
- `workspace.rs` は 1,399 行、`Workspace` は 8 直接フィールド。production body は 975 行で、
  976 行目以降は上記の完了条件を固定する test module。

## 6. 完了条件

- [x] `crates/workspace/src/workspace.rs` は 1,500 行以下（1,399 行、production body 975 行）
- [x] `Workspace` の直接フィールドは 20 以下（8 個）
- [x] Workspace の直接フィールドに Git / LSP / Search / Terminal / Explorer 固有 state がない
- [x] project 切替で dirty buffer または child Entity を破棄しない
- [x] child → shell 通信は typed event
- [x] `explorer` / `git_ui` / `search_ui` が workspace 非依存 crate として存在する
- [x] root Render から Host / FS / Git / DB 呼び出しがない
- [x] `debug_*` production API がない
- [x] 公開 API と `state.json` 後方互換を維持する
- [x] workspace test と構造条件を確認済み
- [x] ARCHITECTURE / ROADMAP / JOURNAL を更新済み

## 7. 独立検証（2026-07-20・別セッション）

完了報告を受けて第三者視点で再検証した結果と、その場で直した問題の記録。

### 7.1 追認できた主張

- `cargo check --workspace` green。`cargo tree -i workspace` の consumer は `necoder` のみ（feature crate の逆依存なし）
- `Workspace` は 8 直接フィールド、`workspace.rs` は 1,399 行、§3 のファイル構成は実在
- 監査 test は実体がある（A→B→A の dirty/undo/child Entity 保持、RenderAuditHost による draw 中 Host call 0 など）
- ROADMAP / JOURNAL / ARCHITECTURE の更新も確認

### 7.2 発見して修正した問題: テストの flaky SIGABRT

検証初回の `cargo test --workspace` で workspace lib の test process が **SIGABRT**
（`notify-rs fsevents loop` スレッドの「panic in a destructor during cleanup」）。
両 gpui 監査 test が**監視中の temp root を `remove_dir_all` してから** process teardown に入る構造で、
per-session に生き残る notify watcher（A→B→A 後は 2 本）の fsevents stream 破棄と race する。
隔離実行 32 回では再現せず、フルスイート並列時のみ顕在化する低頻度 race。

**修正**: 両 test の teardown で全 session の `_watch` / `_watch_pump` を root 削除より先に落とす。
修正後 `cargo test --workspace` 連続 2 回 green（exit code はパイプを介さず直接捕捉）。
notify は 6.1.1 のまま。上流の teardown 修正の取り込み（バージョン更新）は将来候補として記録する。

### 7.3 実態の註記（文書の表現と実装の距離）

- **「モジュールへ分割」の実装は `include!`**（テキスト結合）。真のモジュールは `persistence` / `updater` のみで、
  残り 24 ファイルは単一モジュールへ合成される（JOURNAL の学び欄にも記載あり）。名前空間・可視性の境界は無く、
  `cargo fmt` は include 対象ファイルを整形せず、rust-analyzer の include! 対応も限定的。
  commit `884bfdc` の「責務別モジュールへ分割」という表現は実装と一致しない
- **「8 直接フィールド」は Deref 連鎖と併読が必要**: `Workspace` →（`ProjectSessions` 経由）→ `sessions[active]` の
  `Deref/DerefMut` があり、既存メソッドは `self.tabs` 等で**アクティブ session の状態へ暗黙アクセス**する。
  所有権の移動は本物だが、アクセス境界は Deref で迂回されている（smart pointer 以外の Deref は
  Rust API guidelines が避けるパターン。アクセス時点の active に依存する点も含め、明示化はフォローアップ）
- **`dev_probes.rs` は看板と中身が不一致**: `debug_*` は item 単位で `#[cfg(debug_assertions)]` gate 済みだが、
  同居する SSH 入力・TerminalDock イベント・updater（~350 行）は常駐の本番コード。
  「debug build only API」はファイル単位では成立していない
- **`editor_area/mod.rs`**: リポ規約（mod.rs 禁止）に反する命名。include! なので実際にはモジュールですらない
- **crate 化の実態**: `explorer` 209 行 / `git_ui` 104 行は typed event 契約 + project state の薄い crate で、
  実装の大半（shell adapter の controller/view 計 ~2,950 行）は workspace 側に残る（§2.3 の判断どおり。規模感をここに固定）

### 7.4 UI 目視

オフスクリーン（`NECODER_SCREENSHOT` + `--features screenshot`）で default 状態を 2560×1600 撮影。
titlebar ピル / レール / explorer / タブ下線 / エディタ / Agent ドック（スレッド色・トークン・マスコット・
composer 宛先チップ）/ statusbar とも崩れなし。

### 7.5 フォローアップ候補（軽い順）— **全 5 項目実施済み（2026-07-20・§8）**

1. ~~`editor_area/mod.rs` → `editor_area.rs` 改名~~ ✓
2. ~~`dev_probes.rs` の本番コード移動~~ ✓
3. ~~include! → 真の `mod` 化~~ ✓
4. ~~notify 6.1.1 の更新~~ ✓（8.2.0）
5. ~~Deref 連鎖の段階的な明示化~~ ✓（アクセサ導入 + 代表 2 モジュール移行。全面移行は「触った機能から」継続）

## 8. §7.5 の実施記録（2026-07-20・独立検証と同セッション）

§7.3 の乖離を実装側から解消した。挙動不変・公開 API 不変（`lib.rs` facade / `workspace::{Workspace,
ProjectSource, RestoredTabs, state_path, load_state, load_saved_state}` は据え置き）。

### 8.1 include! 全廃 — 真のモジュールツリー

```text
crates/workspace/src/
  lib.rs                 mod persistence / mod workspace / pub mod updater（facade 不変）
  persistence.rs         （従来から真のモジュール）
  updater.rs             （同上）
  workspace.rs           hub: pub(crate) use 束・actions!・共有語彙型・Workspace/ChromeState 等・
                         new/初期化・Render・監査 test（1,416 行）
  workspace/             子モジュール 18 + editor_area/ 孫 7
    chrome.rs / commands.rs / dev_probes.rs / explorer_controller.rs / explorer_view.rs /
    git_controller.rs / git_view.rs / notifications.rs / overlays.rs / panels.rs /
    project_session.rs / project_switch.rs / project_watcher.rs / rail.rs / rail_view.rs /
    remote_ssh.rs / todo_panel.rs
    editor_area.rs + editor_area/{tabs,language,diagnostics,diff,inline_edit,hot_exit,overlays}.rs
```

- 仕組み: 子は `use crate::workspace::*;` 1 行で hub の名前空間を継承（hub の import 束を
  `pub(crate) use` 化 + **Rust の descendant 可視性**＝子は親 hub の private 型に触れる）。
  hub は `pub(crate) use 子::*;` で共有型を再フラット化。cross-module になるメソッドは一括
  `pub(crate)`（従来の単一モジュールと同じ crate 内可視・絞り込みは触った機能から）
- 効果: rustfmt / rust-analyzer が全ファイルを first-class に扱える。`private_interfaces` 警告 0
- `remote_ssh.rs` 新設（SSH 導線一式）。`on_terminal_dock_event` は panels.rs（購読ハンドラ集約先）、
  `open_thread_history` は overlays.rs、updater 2 本は chrome.rs（statusbar 持ち）へ。
  dev_probes.rs は**全 item `#[cfg(debug_assertions)]` の `debug_*` のみ**になり §1 の看板が実態と一致
- `editor_area/mod.rs` は消滅（`editor_area.rs` + 子ディレクトリ・mod.rs 禁止規約に復帰）

### 8.2 notify 8.2.0

`notify = "6.1"` → `"8"`（8.2.0）。使用面（`recommended_watcher` / `watch` / `Event.paths`）は
API 互換でコード変更なし。§7.2 の teardown 防御（test 側で watcher を root 削除より先に落とす）は
上流修正と独立に維持する。

### 8.3 Deref 明示化の入口

`ProjectSessions::active_session(_mut)` と `Workspace::session(_mut)` を追加。
**新規コードの規約: アクティブ session の状態は `self.session()` / `self.session_mut()` 経由**
（Deref の暗黙 `self.<field>` は「アクセス時点の active に依存する」ことが見えないため）。
project_watcher / project_switch は移行済み。残りは触った機能から漸進（big-bang しない）。

### 8.4 検証

`cargo check --workspace` / `cargo test --workspace` ×2 / `cargo check -p necoder --release` /
オフスクリーン UI 目視。checkpoint は「1 バッチ = 1 コミット」（6335f78 〜）。
