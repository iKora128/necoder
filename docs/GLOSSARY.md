# necoder 用語集（GLOSSARY）

**機能と場所の「正規名」の唯一の正。** 1 概念 = 1 正規名を **code / 日本語UI / 英語UI** の3列で固定する。
実装・i18n・docs がここと乖離したら、**この表へ寄せる**（新しい UI 文字列や識別子はまずここを見る）。

## 命名の原則

- **1 概念 1 名**。同義語・別名を増やさない。
- **agent / エージェント = 「どの AI か」だけ**（Claude Code 等 = `AgentKind`）。Fleet の作業単位は **TaskSpace**、会話単位は thread。
- **Fleet = 多エージェント・モードのコンセプト名**（新概念・ユーザーにもそのまま "Fleet"）。ユーザーに見せる概念は **Task / Task の中のタブ / Captain** の 3 つだけ（正は `FLEET-V2.md`）。
- レールの 1 枠は **project**。Fleet で lifecycle を持つ worktree だけを **TaskSpace** と呼ぶ。
- `Workspace` は**アプリの窓 / シェル全体**（全 project を所有）＝ 作業フォルダではない。

## 正規用語

| 概念 | code | 日本語UI | 英語UI |
|---|---|---|---|
| アプリの窓 / シェル | `Workspace` | ワークスペース | Workspace |
| **Fleet モード**（多エージェント表示） | `fleet` / `fleet_mode` | Fleet | Fleet |
| 既定モード（単一・エディタ） | `solo`（対語・任意） | （無名） | （unnamed / Editor） |
| **Chat モード**（プロジェクトに紐づかない会話。2026-09-18・ROADMAP M16） | `chat` / `chat_mode` | Chat | Chat |
| ↳ **チャット**（Chat の会話 1 本 = thread。別の型は作らない） | `Thread`（`chat: Some(ChatThreadState)`）/ 一覧の行は `ChatRow` | チャット | Chat |
| ↳ **Chat プリセット**（セッションの作り方。プロンプト・ツール・設定の継承・MCP・フォルダの束。正は `CHAT.md` §5） | `acp_client::preset::SessionPreset`（中身を詰めるのは `chat_core::preset::session_preset`） | — | — |
| ↳ **チャットのフォルダ**（会話ごとの永続の作業場所 = cwd。成果物はその中の `artifacts/`。隔離ではない） | `chat_dir`（`<書類>/necoder/<YYYY-MM-DD 先頭の文>/`・`paths::documents_dir()`） | フォルダ | Folder |
| **artifact**（エージェントがチャットのフォルダの `artifacts/` に書いた、見せるための単体ファイル。表示は既存プレビュー） | `artifact` | artifact | Artifact |
| ↳ **添付**（composer へドロップしたファイル・フォルダ。読み取りは自動許可・書き込みは初回確認） | `Thread.context`（既存の @メンション） | 添付 | Attachment |
| ↳ **プレビューチップ**（ツールカードからプレビュー表示で開く） | `preview_chip` | プレビュー | Preview |
| **Fleet サイドバー**（状態一覧） | `fleet_sidebar`（← `herd`） | Fleet サイドバー | Fleet sidebar |
| **系譜グラフ** | `lineage` / `graph` | 系譜 | Lineage |
| ↳ 表示（4 種） | `GraphView::{Fan,Tree,Card,Hub}` | 扇形 / ツリー / カード / ハブ | Fan / Tree / Card / Hub |
| **舞台**（Fleet 中央。Task カードを 1〜3 枚） | `stage` / `StageLayout::{One,Two,Three}` | 舞台 | Stage |
| **Task カード**（舞台の 1 枚 = 1 Task。← セル） | `TaskCard` | Task カード | Task card |
| ↳ **Task タブ**（カードの中の面。← FleetPane / 面） | `TaskTab::{Thread(id),Diff,Terminal(id),Files}` | スレッド / 変更 / ターミナル / ファイル | Thread / Diff / Terminal / Files |
| **変更レビュー**（worktree の全変更を 1 画面で読む面。Fleet の「変更」タブの中身もこれ・session に 1 枚） | `review_view::ReviewView` / `OpenReview` | 変更レビュー | Review |
| ↳ **比較の基準**（Task の base / HEAD / ブランチの分岐点 / コミット） | `project::review::ReviewBase` | 比較 | Compare |
| ↳ **見た**（ファイルに付ける既読の印。付けると畳む） | `reviewed` | 見た | Viewed |
| ↳ **注記**（diff の行に付けるコメント。未送信 / 送信済み / 解決。対象は拡張できる = 将来 Design Mode のページ要素も） | `ReviewNote` / `NoteTarget` / storage `review_notes` | 注記 | Note |
| ↳ **注記トレイ**（変更レビューの下端。件数・一覧・送る） | `render_tray` | 注記 | Notes |
| ↳ **送る**（注記を 1 通のプロンプトにまとめて宛先のスレッドへ） | `ReviewEvent::SendNotes` / `AgentPanel::send_user_prompt_to` | 送る / 未解決だけ再送 | Send / Resend unresolved |
| ↳ **ピン**（舞台に並べる Task を選ぶ） | `pinned` | 並べる | Pin |
| **系譜の帯**（中央上の薄い系譜・⌄ で 4 表示に展開） | `lineage_strip` | 系譜 | Lineage |
| **次へ**（phase に応じた唯一の主操作ボタン） | `next_action` | （phase 別の語） | （phase 別） |
| **＋ Task**（1 プロンプト = 1 worktree のダイアログ） | `new_task_dialog` | ＋ Task | + Task |
| **準備スクリプト**（worktree 作成直後に 1 回） | `worktree_setup`（`.necoder/worktree-setup.sh` / `task.env`） | 準備 | Setup |
| **レール**（左の色バー） | `rail` | レール | Rail |
| **project**（レールの 1 枠） | `ProjectSlot` / `slot` | プロジェクト | Project |
| 長寿命 UI 束（1 project 分） | `ProjectSession` | — | — |
| **TaskSpace**（Fleet の隔離作業単位） | `TaskSpace` / `SpaceId` | Task | Task |
| **IntegrationSpace**（保護された統合先） | `SpaceKind::Integration`（P0 で phase から分離） | Integration | Integration |
| **thread**（Task 内の会話 / AgentRun 1 本） | `Thread` | スレッド | Thread |
| **agent**（話す相手の AI） | `AgentKind` / `agent` | エージェント | Agent |
| ↳ **slash コマンド**（エージェントが広告する `/name` の命令。composer の行頭 `/` で補完・O2） | `acp_client::SlashCommand` / `AgentEvent::Commands` / `Thread.commands` | コマンド | Command |
| ↳ **会話名**（エージェントが付けたスレッドの題名。手動改名が優先） | `AgentEvent::TitleChanged`（ACP `session_info_update.title`）/ 手動の印 `thread_custom_names` | （スレッド名） | (thread name) |
| ↳ **目標**（`/goal` でエージェントが追う目的。composer の上に 1 行） | `acp_client::AgentGoal` / `AgentEvent::GoalChanged` / `Thread.goal` | 目標 | Goal |
| ↳ **使用量**（エージェントがターンごとに報告したトークンと推定コスト。推定コストは実際の請求額ではない。報告の無い値は NULL＝`—`。台帳に 1 ターン 1 行・O11） | `turn_usage`（storage）/ `AgentEvent::TurnUsage` / `AgentEvent::SessionCost` / `agent_panel::usage::CostMeter` | 使用量 | Usage |
| ↳ **レート制限**（プランの利用上限の窓と使用率。エージェントが知らせてきた**最後の値**・O11） | `acp_client::usage::RateLimits` / `AgentEvent::RateLimits` / `agent_panel::usage::UsageLimits` | レート制限 | Rate limits |
| ↳ **アカウント**（レート制限の持ち主 = エージェント × 実行 host × 認証の環境の指紋。値はこれごとに分ける・R08） | `agent_panel::usage::UsageAccount` | —（名前の隣に `dev@devbox（SSH）` / 置き場のパス / `設定の env #…` を添える） | — |
| ↳ **窓**（レート制限の期間） | `LimitWindow::{FiveHour, Weekly, Named, Minutes}` | 5 時間枠 / 週枠 / 〈名前〉の週枠 | 5-hour window / Weekly window / Weekly (〈name〉) |
| ↳ **使用量の統計**（日付 × エージェントの集計画面） | `usage_view::render_usage_stats` / `Storage::daily_usage` | 使用量の統計 | Usage statistics |
| **遷移スナップショット**（状態遷移時の 1 行） | `digest` / `digest_tail` / `Thread.digest` | （文そのもの・ラベル無し） | （no label） |
| **要対応**（Fleet サイドバー最上段・裁く列。← 管制の要対応キュー） | `AttentionItem` / `attention_queue` | 要対応 | Attention |
| **統合パイプライン**（TaskPhase 列の帯） | `render_pipeline` | 統合パイプライン | Integration pipeline |
| **ニュース**（task_events の鏡・時系列） | `NewsItem` / `NewsKind` | ニュース | News |
| **Captain**（任命制の采配スレッド。コードを書かず fleet CLI/MCP で采配・integrate は人間。← 監督） | `captain`（`NewsKind::Captain`・設定 `captain_agent`） | Captain | Captain |
| ↳ **介入**（Captain を通さず Task に直接書く。台帳に残る） | `human_send`（`NewsKind::HumanSend`） | （宛先チップで示す） | — |
| ↳ **采配ログ**（Captain の判断と実行の履歴） | `NewsKind::Captain` | 采配ログ | Captain log |
| **集約気分**（編隊の最悪状態に追従する 1 匹） | `fleet_mood_mascot` | — | — |
| **常駐**（Herdr sidecar 実行形態・P7） | `HerdrRuntime`（予定） | 常駐 | Resident (Herdr) |
| **リモート管制**（スマホから見る/裁く・P9） | `serve --control` / `remote_control` | リモート管制 | Remote control |
| ↳ **ペアリング**（QR で端末を繋ぐ・1 回きり） | `pairing` / `room_id` | ペアリング | Pairing |
| ↳ **デバイス**（ペア済みの端末・失効の単位） | `PairedDevice` | デバイス | Device |
| ↳ **リレー**（room id が一致する 2 本を繋ぐ交換機） | `relay`（`relay/`・DO） | リレー | Relay |
| ↳ **封**（transport 非依存の暗号化フレーム） | `seal` / `open` / `SealedFrame` | — | — |
| **片付けメニュー**（Task カードの ⋯・残るものが減る順の段） | `FleetCellMenuState` / `FleetCellAction` | 片付け | Clean up |
| ↳ カードを閉じる（舞台から外すだけ） | `close_fleet_cell` | カードを閉じる | Close card |
| ↳ Task を終了（台帳を archived に） | `archive_fleet_cell_task` | Task を終了 | Finish Task |
| ↳ worktree を削除（ディスクから消す） | `delete_fleet_cell_worktree` | worktree を削除 | Delete worktree |
| ↳ 削除の確認（失うものを数えて見せる） | `WorktreeDeleteConfirm` / `WorktreeStakes` | — | — |
| **下段ドック**（Fleet 下の可変高タブ面） | `FleetBottomView` / `bottom_height` | 下段 | Bottom pane |
| **OS の通知**（通知センター。その窓を見ていない時だけ・スレッドごとに 1 件） | `system_notifications` / `post_agent_notification` / 設定 `system_notifications` | OS の通知 | System notifications |
| **Dock バッジ**（要対応の件数・全窓の合計・macOS） | `dock_badge` | — | — |
| **質問待ち**（エージェントが選択肢付きで聞いてきて止まっている。状態は承認待ちと同じ Blocked） | `PanelEvent::QuestionWaiting` / `AttentionKind::Question` | 質問待ち | Waiting for an answer |
| **終了の確認**（⌘Q = 全窓・窓を閉じる = その窓。止まるエージェント・ターミナルが動いている時だけ） | `quit_guard` / `QuitConfirmState` / `QuitReason`（設定 `confirm_quit`） | 終了・窓を閉じる時の確認 | Confirm before quitting or closing a window |
| ↳ **隠して動かし続ける**（既定。mac はアプリを隠す・他 OS は最小化・ほかの窓が残る窓閉じはその窓だけ最小化。プロセスは止めない。**necoder が起動している間だけ**続く＝終了・クラッシュ・更新の再起動では止まる） | `QuitChoice::KeepRunning` / `OutOfSight` | 隠して動かし続ける / 最小化して動かし続ける | Hide and keep running / Minimize and keep running |
| **AI 全画面**（solo で中央エディタを Agent に差し替える。左/下ドックは各自の ON/OFF） | `agent_full_screen` / `ToggleAgentFullScreen` | AI を全画面 | AI full screen |
| **「最新へ」ボタン**（transcript を遡り中だけ右下に出る・最下部へ戻す） | `render_jump_to_latest` | 最新へ | Jump to latest |
| **プレビュー**（`.md` のネイティブ整形表示 / `.html` のOS標準WebView表示。source ⇄ preview・⌘⇧V。開発サーバを見るのは別のタブ＝下の Web タブ） | `rendered_markdown` / `rendered_html` / `ToggleRenderedMarkdown` / `markdown_preview` / `webview_view` | プレビュー | Preview |
| ↳ **Web タブ**（localhost の開発サーバを見るタブ。読み込めるのはループバックの http(s) だけ・汎用ブラウザではない。鍵は URL。入口はパレット「プレビュー: localhost を開く…」と transcript / Markdown のリンク） | `TabContent::Web` / `WebPreviewView` / `webview_view::localhost` / `Workspace::open_url` | Web タブ | Web tab |
| ↳ **Design モード**（Web タブのページの要素を選び、切り抜きと説明を composer へ「要素のチップ」として添える。送信はしない。⌘⇧D） | `ToggleDesignMode` / `webview_view::design` / `ElementCapture` / `WebPreviewEvent::ElementPicked` | Design モード / 要素のチップ | Design Mode / element chip |
| **ne コマンド**（ターミナルから開く CLI・`code`/`cursor` 相当） | `cli_shim`（シム生成）/ `necoder cli`（実体） | ne コマンド | ne command |
| **ターミナル**（下ドックの統合ターミナル。Task タブの Terminal と同じ実体） | `terminal_view::TerminalView` / `TerminalDock` | ターミナル | Terminal |
| ↳ **ターミナル内検索**（⌘F・scrollback を含む。エディタの「検索」= バッファ内検索とは別） | `terminal::Find` / `terminal_view::search` | ターミナル内を検索 | Find in terminal |
| ↳ **ターミナルのリンク**（`path:line` と URL・OSC 8。URL を開く受け口は 1 か所） | `TerminalLink` / `TerminalEvent::OpenUrl` / `open_terminal_url` | （下線のみ） | (underline only) |
| **Skill**（エージェントが読む手順書 `SKILL.md`。置き場は `~/.claude/skills`・`~/.codex/skills`・`~/.agents/skills` とプロジェクトの `.claude/skills`・`.agents/skills`） | `agent_skills`（走査・設置）/ `necoder skills`（CLI） | skill | Skill |
| ↳ **necoder の skill**（入口だけの `SKILL.md`。使い方の本文は `ne skills get` が版に合わせて出す） | `agent_skills::stub_skill_md` / `StubState` | necoder の skill | necoder skill |
| **Task の指し方**（CLI の `<task>`。id / `branch:` / `name:` / `active` = GUI で選択中） | `fleet::TaskSelector` / `select_task` | — | — |
| **端末ハンドル**（`ne terminal list` が返す端末の名前 `t<番号>`。GUI を再起動すると変わる） | `terminal_handle`（control_ipc） | — | — |
| **CLI から端末へ入力を送る**（`ne terminal send` の許可。既定 off） | `allow_terminal_send` | CLI から端末へ入力を送る | Let the CLI type into terminals |
| **MCP サーバ**（エージェントに持たせる道具。`session/new` で渡す） | `acp_client::mcp` / `mcp_servers` | MCP サーバ | MCP server |
| ↳ **発見**（他ツールの設定に居るサーバを一覧に載せる・既定 off） | `mcp::discover` / `McpSource` | 〈出所〉で発見 | found in 〈source〉 |
| **無視されたファイルも探す**（⌘P の 2 回目。1 回目で何も一致しない時だけ出る行） | `ignored_files_local` / `FINDER_ACTION_SEARCH_IGNORED` / `Picker::set_fallback_action` | 無視されたファイルも探す / 無視 | Also search ignored files / ignored |
| **フォルダ内を検索**（⌘⇧F の検索を 1 フォルダに絞る。パネルの頭に範囲のチップ） | `SearchPanel::scope` / `open_folder_search` | フォルダ内を検索 / 範囲 | Find in folder / In |
| **ファイル操作の取り消し**（エクスプローラの ⌘Z。作成・名前の変更・移動・複製・ゴミ箱を 1 手ずつ戻す） | `FileOperation` / `FileOperationHistory` / `UndoFileOperation` | ファイル操作を取り消す | Undo file operation |

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
| 監督 / `coordinator` / Coordinator | Captain / `captain` | Fleet の比喩に合わせ中核機能として改名（2026-09-12）。旧設定キー `coordinator_agent` の読み替えは作らない |
| セル / `FleetPane` / surface | Task カード / `TaskTab` | 同じ worktree のターミナルが別セルになって増殖していた。全部 Task の中のタブ（FLEET-V2 §3.5） |
| 作業 / 作業ツリー / 列 / ペイン / 面 / 配置（`workbench` / `WorkColumn` / `WorkPane` / `WorkSurface` / `RepositoryLayout`） | 廃止 | 0.1.15 の作業タブは実機で取り回しが悪く降格 → FLEET-V2 で削除 |
| 管制（中央タブ）/ `FleetCenterView` | 要対応（サイドバー常設）+ Captain カード | 中央タブ 3 面は「今どこを見ているか」を失わせる。**リモート管制**の名前だけ P9 完了まで据え置き |
| ＋ACP / ＋Terminal / ＋Worktree | ＋ Task（と Task タブ行の ＋▾） | 押す前に「どれか」を考えさせない。1 プロンプト = 1 worktree |
| Multi Agent / 編隊（UI） | Fleet | Fleet を新概念としてユーザーにも前面 |
| river / リバー | hub / Hub | 系譜の表示は Fan/Tree/Card/Hub に確定（River は廃止済み） |
| space（一般的なレール枠の意味） | project / `ProjectSlot` | `TaskSpace` / `IntegrationSpace` という型名に限り使用 |
| "panel"（Fleet タイルの意味） | cell | パネルはドック用語 |
| `"Claude"`（agent ラベル） | `"Claude Code"` | `AgentKind::by_label` は完全一致（フォールバックのバグ源） |
| モデル名・思考量・権限モードを「表示名」で保存/比較 | **value_id** で保存/比較 | 同じ物に `opus[1m]` と `Opus (1M context)` の 2 つの綴りがある。表示名は描画専用（DECISIONS 2026-09-09） |
| necoder 独自のモデル綴り（`claude-opus-5` 等の静的一覧） | ACP の広告 | 第三の語彙を作ると必ず広告と食い違う。接続前は候補を出さない |
| トークン台帳 / `token_ledger` | 使用量（`turn_usage`） | `threads.tokens_used` は文脈窓の使用量で累計ではなかった（O11 で削除） |
| クォータ / quota（UI の語として） | レート制限 / 使用量 | adapter の `_meta.quota` は「そのターンに使ったトークン」で上限ではない。上限は レート制限、使った量は 使用量 |

## 正の所在（どこを直すか）

- **UI 文字列** … `locales/ja.yml` / `en.yml`（キーは英語スネークケースの領域プレフィックス・両方必須／`crates/i18n` の parity テストが差分を検出）。
- **コード識別子** … 上表の code 列。
- **この表の正** … 本ファイル。`CLAUDE.md` の一次資料表から参照する。
