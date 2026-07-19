# Workspace architecture refactor

作成 2026-07-20。対象は `crates/workspace` と、そこから本来の所有者へ移す周辺機能。

ステータス: **実行中**。開始 checkpoint は `ca46f40`。

## 0. 目的

`crates/workspace/src/workspace.rs` は約 13,000 行、`Workspace` は約 95 フィールド、単一
`impl Workspace` は 300 メソッド超を持つ。長さそのものより、Workspace が次の機能の状態・非同期処理・
描画・永続化を直接所有していることが問題である。

- project / tab / pane
- explorer とファイル操作
- git status / branch / panel / diff / blame
- LSP / diagnostics / completion / hover / rename / code actions
- project search / buffer search
- Agent / terminal / Todo / settings
- hot exit / watcher / picker / notification / updater

この refactor の完了条件は「ファイルを分けた」ではない。Workspace をウィンドウシェルとイベントルーターへ
戻し、各状態をライフサイクル上の正しい所有者へ移すことである。

## 1. 不変条件

1. 既存機能と公開 API を維持する。公開型の移動中は旧パスから re-export する。
2. view crate は `workspace` を依存先にしない。child → shell は typed event、shell → child は公開メソッドで通信する。
3. `Host` / FS / Git / DB 呼び出しを `Render` から行わない。非同期処理と世代管理は各 controller が所有する。
4. `Workspace` の全フィールドを一括 `pub(crate)` 化しない。移動に必要な最小 visibility だけを付与し、最終的に
   feature 固有フィールドを Workspace からなくす。
5. project 切替で dirty buffer、Editor Entity、Agent session、Terminal を破棄しない。
6. `actions!` の action id、i18n key、state.json の後方互換を維持する。
7. 各段階で `cargo check --workspace` と `cargo test --workspace` を green にする。
8. 機械移動と所有権変更を別コミットにし、失敗時に切り分けられる履歴を保つ。

## 2. 最終アーキテクチャ

```text
foundation/model
  host / storage / theme_core / settings_core / editor_core / project / search / lang
                                  ↑
feature views
  editor_view / explorer / git_ui / search_ui / agent_panel / terminal_view / settings
                                  ↑
shell
  workspace
                                  ↑
composition
  shirushi
```

### 2.1 Workspace と ProjectSession

```rust
pub struct Workspace {
    sessions: Vec<ProjectSession>,
    active_session: usize,
    rail: Entity<ProjectRail>,
    overlays: Entity<WorkspaceOverlays>,
    notifications: Entity<NotificationCenter>,
    chrome: ChromeState,
    persistence: WorkspacePersistence,
    updater: UpdateController,
    focus_handle: FocusHandle,
}

pub struct ProjectSession {
    context: ProjectContext,
    editor: Entity<EditorArea>,
    explorer: Entity<Explorer>,
    git_panel: Entity<GitPanel>,
    agent_panel: Entity<AgentPanel>,
    terminal_dock: Entity<TerminalDock>,
    repository: RepositoryController,
    watcher: ProjectWatcher,
}
```

レール切替は `active_session` を変えるだけとし、非アクティブ session の Entity を保持する。

### 2.2 EditorArea

EditorArea は project session 内の編集面を所有する。

- tabs / active tab / split / recently closed
- file open / close / save / navigation
- buffer search / transient diff tab
- LSP lifecycle / diagnostics / completion / hover
- format / rename / code actions / references / symbols / workspace edit
- diff / hunk / blame / inline edit
- dirty buffer の hot exit

LSP の JSON 応答型と server registry は `lang` に置き、EditorArea は typed value を扱う。

### 2.3 独立 feature view

| 所有者 | 移すもの | shell への主イベント |
|---|---|---|
| `explorer` crate | tree/columns/icons、選択、展開、命名、ファイル操作、context menu | `OpenPath`, `FilesChanged`, `Focus` |
| `git_ui` crate | Git panel、branch menu、commit/stage/push/pull、history | `RepositoryChanged`, `OpenDiff`, `Toast` |
| `search_ui` crate | project search state/render/keyboard handling | `OpenMatch`, `Dismissed` |
| `terminal_view::TerminalDock` | terminal tabs、header、active terminal | `OpenPath`, `Focus` |
| `settings::SettingsView` | settings home、agent setup、onboarding | `RunCommand`, `OnboardingCompleted` |
| `workspace::TodoPanel` | Todo board UI | `SendToAgent`, `FilesChanged` |

Git snapshot と watcher は panel の開閉に依存してはならない。ProjectSession の controller が一度読み、Explorer・
EditorArea・GitPanelへ fan-out する。

### 2.4 typed event

child Entity は sibling を直接呼ばない。

```text
ProjectWatcher -> ProjectSession -> Explorer / RepositoryController / EditorArea
ExplorerEvent  -> Workspace      -> EditorArea
GitPanelEvent  -> Workspace      -> EditorArea / notifications
EditorAreaEvent-> Workspace      -> Explorer / chrome / persistence
AgentPanelEvent-> Workspace      -> EditorArea / notifications
TerminalEvent  -> Workspace      -> EditorArea
SettingsEvent  -> Workspace      -> TerminalDock / chrome
```

## 3. 最終ソース構成

```text
crates/workspace/src/
  lib.rs
  workspace.rs
  project_context.rs
  project_session.rs
  rail.rs
  chrome.rs
  commands.rs
  overlays.rs
  notifications.rs
  persistence.rs
  todo_panel.rs
  updater.rs
  dev_probes.rs
  editor_area/
    mod.rs
    tabs.rs
    panes.rs
    navigation.rs
    buffer_search.rs
    language.rs
    diagnostics.rs
    completion.rs
    workspace_edit.rs
    diff.rs
    inline_edit.rs
    hot_exit.rs

crates/explorer/src/lib.rs
crates/git_ui/src/lib.rs
crates/search_ui/src/lib.rs
```

既存の `[lib] path = "src/workspace.rs"` は廃止し、`src/lib.rs` を facade にする。

## 4. 実行順

### Phase A — 安全網と facade

1. 仕掛かりを checkpoint 化し clean tree にする。
2. public API、state.json round trip、project removal index、project switch の characterization test を追加する。
3. `src/lib.rs` を導入し、公開 API を re-export する。
4. persistence、純粋 helper、dev probe を feature module へ移す。

### Phase B — model と typed LSP

1. `ProjectSource` / `ProjectContext` を分離する。
2. LSP response parser と `LanguageServerSpec` を `lang` へ移す。
3. workspace shell から `serde_json::Value` と executable 探索を除去する。

### Phase C — feature Entity / crate

1. ExplorerをEntity化して `explorer` crateへ移す。
2. `TerminalDock` を `terminal_view` に追加する。
3. Git panelをEntity化して `git_ui` crateへ移す。
4. Project searchをEntity化して `search_ui` crateへ移す。
5. SettingsViewを `settings` crateへ移す。
6. TodoPanelを workspace 内の独立Entityへ移す。

### Phase D — EditorArea

1. tab/pane/file lifecycleをEditorAreaへ移す。
2. navigation/buffer search/hot exitを移す。
3. language機能とeditor overlayを移す。
4. diff/blame/inline editを移す。

### Phase E — ProjectSession

1. ProjectSessionが各child Entityとcontrollerを所有するよう変更する。
2. project切替時の `tabs.clear` / LSP破棄 / terminal破棄 / pathからの再openを撤去する。
3. Agent threadをproject/branchで分離し、非アクティブprojectの実行を保持する。
4. state.json保存元を各ProjectSessionのEditorAreaへ一本化する。

### Phase F — shell縮退と登録境界

1. Workspace root Renderをchrome、rail、active session、overlayの合成だけにする。
2. hard-coded `command_entries` をCommandRegistryへ移す。
3. stableなPanel event境界をPanelRegistryへ登録できる形にする。
4. `main.rs` の `SHIRUSHI_*` probeをdev supportへ移し、production APIの `debug_*` 群を除去する。
5. ARCHITECTURE / ROADMAP / JOURNALを実装に同期する。

## 5. 検証

各コミット:

```bash
cargo check --workspace
cargo test --workspace
git diff --check
```

最終検証:

- default / Explorer各view / Git / project search / settings / Todo / picker のoffscreen render
- project Aでdirty bufferを作り、A→B→Aで内容とundo履歴が残る
- project A/BのAgent・Terminalが混線せず切替後も生存する
- local/remote両方でrender中にHostを呼ばない
- state.json旧形式と新形式のround trip
- i18n ja/en parity

## 6. 完了条件

- `crates/workspace/src/workspace.rs` は 1,500 行以下
- `Workspace` の直接フィールドは 20 以下
- WorkspaceにGit/LSP/Search/Terminal/Explorer固有stateがない
- project切替でdirty bufferまたはchild Entityを破棄しない
- child → shell通信はtyped event
- `explorer` / `git_ui` / `search_ui` がworkspace非依存crateとして存在する
- root RenderからHost/FS/Git/DB呼び出しがない
- `debug_*` production APIがない
- 公開APIとstate.json後方互換を維持する
- 全workspace test green、構造条件を検索と行数で確認済み
- ARCHITECTURE / ROADMAP / JOURNALが更新済み

