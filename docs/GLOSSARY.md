# Shirushi 用語集（GLOSSARY）

**機能と場所の「正規名」の唯一の正。** 1 概念 = 1 正規名を **code / 日本語UI / 英語UI** の3列で固定する。
実装・i18n・docs がここと乖離したら、**この表へ寄せる**（新しい UI 文字列や識別子はまずここを見る）。

## 命名の原則

- **1 概念 1 名**。同義語・別名を増やさない。
- **agent / エージェント = 「どの AI か」だけ**（Claude Code 等 = `AgentKind`）。Fleet の作業単位は **TaskSpace**、会話単位は thread。
- **Fleet = 多エージェント・モードのコンセプト名**（新概念・ユーザーにもそのまま "Fleet"）。
- レールの 1 枠は **project**。Fleet で lifecycle を持つ worktree だけを **TaskSpace** と呼ぶ。
- `Workspace` は**アプリの窓 / シェル全体**（全 project を所有）＝ 作業フォルダではない。

## 正規用語

| 概念 | code | 日本語UI | 英語UI |
|---|---|---|---|
| アプリの窓 / シェル | `Workspace` | ワークスペース | Workspace |
| **Fleet モード**（多エージェント表示） | `fleet` / `fleet_mode` | Fleet | Fleet |
| 既定モード（単一・エディタ） | `solo`（対語・任意） | （無名） | （unnamed / Editor） |
| **Fleet サイドバー**（状態一覧） | `fleet_sidebar`（← `herd`） | Fleet サイドバー | Fleet sidebar |
| **系譜グラフ** | `lineage` / `graph` | 系譜 | Lineage |
| ↳ 表示（4 種） | `GraphView::{Fan,Tree,Card,Hub}` | 扇形 / ツリー / カード / ハブ | Fan / Tree / Card / Hub |
| **セル**（グリッドのタイル） | `FleetCell`（← `FleetPane`・提案） | セル | Cell |
| ↳ セルの中身 | `FleetPane::{Task,Terminal,Editor,Diff,Tests}` | Task / ターミナル / エディタ / Diff / Tests | Task / Terminal / Editor / Diff / Tests |
| **レール**（左の色バー） | `rail` | レール | Rail |
| **project**（レールの 1 枠） | `ProjectSlot` / `slot` | プロジェクト | Project |
| 長寿命 UI 束（1 project 分） | `ProjectSession` | — | — |
| **TaskSpace**（Fleet の隔離作業単位） | `TaskSpace` / `SpaceId` | Task | Task |
| **IntegrationSpace**（保護された統合先） | `SpaceKind::Integration`（P0 で phase から分離） | Integration | Integration |
| **thread**（Task 内の会話 / AgentRun 1 本） | `Thread` | スレッド | Thread |
| **agent**（話す相手の AI） | `AgentKind` / `agent` | エージェント | Agent |
| **管制**（編隊統括ダッシュボード・中央タブ） | `FleetCenterView::Control` / `control_view` | 管制 | Control |
| **遷移スナップショット**（状態遷移時の 1 行） | `digest` / `digest_tail` / `Thread.digest` | （文そのもの・ラベル無し） | （no label） |
| **要対応キュー**（管制左・裁く列） | `AttentionItem` / `attention_queue` | 要対応 | Attention |
| **統合パイプライン**（TaskPhase 列の帯） | `render_pipeline` | 統合パイプライン | Integration pipeline |
| **ニュース**（task_events の鏡・時系列） | `NewsItem` / `NewsKind` | ニュース | News |
| **監督**（任命制の采配スレッド・P6） | `coordinator`（`NewsKind::Coordinator`） | 監督 | Coordinator |
| **集約気分**（編隊の最悪状態に追従する 1 匹） | `fleet_mood_mascot` | — | — |
| **常駐**（Herdr sidecar 実行形態・P7） | `HerdrRuntime`（予定） | 常駐 | Resident (Herdr) |
| **リモート管制**（スマホから見る/裁く・P9） | `serve --control` / `remote_control` | リモート管制 | Remote control |
| ↳ **ペアリング**（QR で端末を繋ぐ・1 回きり） | `pairing` / `room_id` | ペアリング | Pairing |
| ↳ **デバイス**（ペア済みの端末・失効の単位） | `PairedDevice` | デバイス | Device |
| ↳ **リレー**（room id が一致する 2 本を繋ぐ交換機） | `relay`（`relay/`・DO） | リレー | Relay |
| ↳ **封**（transport 非依存の暗号化フレーム） | `seal` / `open` / `SealedFrame` | — | — |
| **片付けメニュー**（セルの ⋯・残るものが減る順の段） | `FleetCellMenuState` / `FleetCellAction` | 片付け | Clean up |
| ↳ セルを閉じる（画面から外すだけ） | `close_fleet_cell` | セルを閉じる | Close cell |
| ↳ Task を終了（台帳を archived に） | `archive_fleet_cell_task` | Task を終了 | Finish Task |
| ↳ worktree を削除（ディスクから消す） | `delete_fleet_cell_worktree` | worktree を削除 | Delete worktree |
| ↳ 削除の確認（失うものを数えて見せる） | `WorktreeDeleteConfirm` / `WorktreeStakes` | — | — |
| **下段ドック**（Fleet 下の可変高タブ面） | `FleetBottomView` / `bottom_height` | 下段 | Bottom pane |
| **AI 全画面**（solo で中央エディタを Agent に差し替える。左/下ドックは各自の ON/OFF） | `agent_full_screen` / `ToggleAgentFullScreen` | AI を全画面 | AI full screen |
| **「最新へ」ボタン**（transcript を遡り中だけ右下に出る・最下部へ戻す） | `render_jump_to_latest` | 最新へ | Jump to latest |

> 日本語で「Fleet」を「編隊」と表記したくなったら、UI 文字列のここだけ差し替える（概念名は Fleet で固定）。

## 二義に注意（避ける衝突）

- **agent** — 「どの AI か」(`AgentKind`) だけに固定。Fleet の隔離単位は TaskSpace、会話は thread。
- **session** — `ProjectSession`（1 project の UI/controller 束）と ACP `session`（LLM 接続）は別物。前者を「session」と略さない。
- **workspace ≠ フォルダ** — 窓/シェル全体。1 フォルダ = project。
- **panel** — ドックの `*_panel`（agent / git / todo …）を指す。Fleet のタイルは **cell**（"panel" と呼ばない）。

## 廃止・禁止語（見つけたら置換）

| 禁止 | → 正 | 理由 |
|---|---|---|
| `herd` / herd サイドバー | `fleet_sidebar` / Fleet サイドバー | Fleet に統一（`herd` は UI に一度も出ない code 専用語） |
| Multi Agent / 編隊（UI） | Fleet | Fleet を新概念としてユーザーにも前面 |
| river / リバー | hub / Hub | 系譜の表示は Fan/Tree/Card/Hub に確定（River は廃止済み） |
| space（一般的なレール枠の意味） | project / `ProjectSlot` | `TaskSpace` / `IntegrationSpace` という型名に限り使用 |
| "panel"（Fleet タイルの意味） | cell | パネルはドック用語 |
| `"Claude"`（agent ラベル） | `"Claude Code"` | `AgentKind::by_label` は完全一致（フォールバックのバグ源） |

## 正の所在（どこを直すか）

- **UI 文字列** … `locales/ja.yml` / `en.yml`（キーは英語スネークケースの領域プレフィックス・両方必須／`crates/i18n` の parity テストが差分を検出）。
- **コード識別子** … 上表の code 列。
- **この表の正** … 本ファイル。`CLAUDE.md` の一次資料表から参照する。
