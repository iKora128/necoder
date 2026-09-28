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
| ↳ **引用**（エディタで選んだ行を `path:10-14` と抜粋のコードブロックにして composer の末尾へ足す。送信はしない・その下に注記を書く。⌥⌘K・O29 / D04。変更レビューの**注記**とは別＝保存しない） | `QuoteSelectionInThread` / `quote_lines` / `AgentPanel::append_quote_to_composer` | 引用 | Quote |
| ↳ **プレビューチップ**（ツールカードからプレビュー表示で開く） | `preview_chip` | プレビュー | Preview |
| **Fleet サイドバー**（状態一覧） | `fleet_sidebar`（← `herd`） | Fleet サイドバー | Fleet sidebar |
| **AI スレッド一覧**（Editor の左カラム・全プロジェクトのスレッド状態。レールの activity アイコン・2026-09-24） | `render_herd_sidebar` / `show_herd`（`herd` は code 専用）・設定 `rail.threads` | AI スレッド一覧 | AI threads |
| **系譜グラフ** | `lineage` / `graph` | 系譜 | Lineage |
| ↳ 表示（4 種） | `GraphView::{Fan,Tree,Card,Hub}` | 扇形 / ツリー / カード / ハブ | Fan / Tree / Card / Hub |
| **舞台**（Fleet 中央。Task カードを 1〜3 枚） | `stage` / `StageLayout::{One,Two,Three}` | 舞台 | Stage |
| **Task カード**（舞台の 1 枚 = 1 Task。← セル） | `TaskCard` | Task カード | Task card |
| ↳ **スレッドタブ**（カードの中の会話。ペインバーの左） | `FleetPane::Agent` + `AgentPanel` の thread | スレッド名 | Thread name |
| ↳ **サイドペイン**（会話の横に開く面。ペインバーの右のトグル。← Task タブ / FleetPane / 面） | `stage_side()`（`FleetPane::{Diff,Shell,Editor}`） | 変更 / ターミナル / ファイル | Changes / Terminal / Files |
| **ブリッジ**（統合先 main のカード = Fleet の家。← Captain カード。2026-09-20: Captain は main に住むスレッドなので別カードにしない） | 統合先の slot + `FleetPane::{Formation,CaptainLog}` | ブリッジ（UI には出さない語。見えるのは `⚑ Captain` タブと `編隊図`） | Bridge |
| ↳ **編隊図**（ブリッジのサイドペイン。ハブ / 扇形 / ツリー / カード。← 系譜グラフの展開表示。舞台の上に縦に展開する旧表示は廃止・系譜の帯は 1 行のまま残る） | `FleetPane::Formation` / `render_formation` | 編隊図 | Formation |
| **変更レビュー**（worktree の全変更を 1 画面で読む面。Fleet の「変更」サイドペインの中身もこれ・session に 1 枚） | `review_view::ReviewView` / `OpenReview` | 変更レビュー | Review |
| ↳ **比較の基準**（Task の base / HEAD / ブランチの分岐点 / コミット） | `project::review::ReviewBase` | 比較 | Compare |
| ↳ **見た**（ファイルに付ける既読の印。付けると畳む。見た時の基準と差分の指紋に結び付き、変わったら外れる） | `reviewed` / `ReviewedMark` | 見た | Viewed |
| ↳ ↳ **見た後に変更あり**（見た印を付けた後でファイルの差分が変わった。印は外れて畳みも開く） | `reviewed_stale` | 見た後に変更あり | Changed since viewed |
| ↳ **注記**（diff の行に付けるコメント。未送信 / 送信済み / 解決。対象は拡張できる = 将来 Design Mode のページ要素も） | `ReviewNote` / `NoteTarget` / storage `review_notes` | 注記 | Note |
| ↳ ↳ **位置が変わった注記**（付けた行の中身が変わり、同じ中身の場所も 1 か所に決まらない注記。行に付けず見出しの直後に元の抜粋つきで出す） | `NotePosition::Lost` / `lost_notes` | 位置が変わった注記 | Moved note |
| ↳ **注記トレイ**（変更レビューの下端。件数・一覧・送る） | `render_tray` | 注記 | Notes |
| ↳ **送る**（注記を 1 通のプロンプトにまとめて宛先のスレッドへ） | `ReviewEvent::SendNotes` / `AgentPanel::send_user_prompt_to` | 送る / 未解決だけ再送 | Send / Resend unresolved |
| ↳ **ピン**（舞台に並べる Task を選ぶ） | `pinned` | 並べる | Pin |
| **系譜の帯**（舞台の上の 1 行の系譜・押すとその Task へ。4 表示の編隊図はブリッジのサイドペイン） | `lineage_strip` | 系譜 | Lineage |
| **次へ**（phase に応じた唯一の主操作ボタン） | `next_action` | （phase 別の語） | （phase 別） |
| **＋ Task**（1 プロンプト = 1 worktree のダイアログ） | `new_task_dialog` | ＋ Task | + Task |
| ↳ **並べて比べる**（fan-out。1 つの依頼を選んだエージェントごとの Task へ・舞台に並べる・O23） | `plan_fanout` / `FanoutTask` / `create_prompted_tasks` | 並べて比べる | Compare side by side |
| **準備スクリプト**（worktree 作成直後に 1 回） | `worktree_setup`（`.necoder/worktree-setup.sh` / `task.env`） | 準備 | Setup |
| **`.worktreeinclude`**（無視しているファイルのうち、新しい Task へ写す物の一覧・`.gitignore` と同じ書き方・準備の前） | `copy_worktree_includes_on` / `prepare_task_worktree_on` | （ファイル名のまま） | (file name) |
| **レール**（左の色バー） | `rail` | レール | Rail |
| **project**（レールの 1 枠） | `ProjectSlot` / `slot` | プロジェクト | Project |
| 長寿命 UI 束（1 project 分） | `ProjectSession` | — | — |
| **衝突の帯**（git の衝突の印が残ったファイルの上に出る帯。今の側 / 入ってくる側 / 両方・次へ） | `conflicts` / `render_conflict_bar` / `ResolveConflictOurs` 等 / `AbortMergeOrRebase` | 衝突 / 今の側 / 入ってくる側 / 両方 | Conflict / Current / Incoming / Both |
| **TaskSpace**（Fleet の隔離作業単位） | `TaskSpace` / `SpaceId` | Task | Task |
| ↳ **親の Task / 子の Task**（起点に別の Task のブランチを選んで作った Task は、その Task の子。依存待ち `depends_on` とは別） | `TaskSpace::parent` / `task_parents` / `nest_task_rows` / `task_descendants` | 親の Task / 子の Task | Parent Task / Child Tasks |
| **IntegrationSpace**（保護された統合先） | `SpaceKind::Integration`（P0 で phase から分離） | Integration | Integration |
| ↳ 統合先の選び方（同じリポジトリに統合先扱いが複数ある時はメインの作業ツリー） | `integration_slot_for` / `TaskSpace::linked` | — | — |
| **リソース**（necoder 本体と子プロセスのメモリをプロジェクト / Task ごとに見る画面・使っていないエージェントを止める・O22） | `resources` / `ShowResources` / `AgentPanel::stop_quiet_agents` | リソース / 使っていないエージェントを止める | Resources / Stop idle agents |
| **Ports**（necoder の中で待ち受けている開発サーバのポート。開く / 止める） | `ports` / `ShowPorts` | 開いているポート | Open ports |
| **外部の worktree**（リポジトリの worktree のうち Task でないもの。necoder の外で作ったものも含む） | `external_worktrees` / `fleet_worktrees` | 外部の worktree | Other worktrees |
| ↳ **取り込む**（外部の worktree をレールに開いて Task にする） | `adopt_worktree` / `make_task_space` | 取り込む | Adopt |
| ↳ **消えています**（Task の worktree が necoder の外で消された。片付け = レールから外す） | `vanished_worktree` / `forget_vanished_task` | 消えています / 片付け | Gone / Clean up |
| **thread**（Task 内の会話 / AgentRun 1 本） | `Thread` | スレッド | Thread |
| **agent**（話す相手の AI） | `AgentKind` / `agent` | エージェント | Agent |
| ↳ **足したエージェント**（組み込みの 7 件の外から一覧に足したエージェント。settings.json の `agent_servers.<新しい id>` に自分のコマンドを書いた物・issue #38 H1。表示名は `name`・無ければ id） | `acp_client::CustomAgent` / `acp_client::AgentCatalog` / `acp_client::Agent`（組み込みと共通の入口）/ `settings::agent_catalog` | 足したエージェント / 外す | Added agent / Remove |
| ↳ **エージェントを追加**（ACP の公開レジストリの全件から選んで足す画面。足すと `agent_servers.<レジストリの id>` に `type: registry` と `name` を書く・issue #38 H2） | `settings::add_agent`（`AddAgentDialog`）/ `acp_client::registry::Registry::search` / `acp_client::CustomLaunch::Registry` | エージェントを追加 / 追加 / 追加済み / 組み込み / 取り直す | Add agent / Add / Added / Built in / Refresh |
| ↳ **配布の形**（レジストリの項目の起動の仕方。このマシン向けの binary → npx → uvx の順で選ぶ） | `acp_client::registry::DistributionKind` / `RegistryAgent::launch_for` | binary / npx / uvx | binary / npx / uvx |
| ↳ ↳ **binary の配備**（レジストリの URL から落とし、sha256 で照合して、版ごとの置き場に展開する。最初の起動の時に背景で） | `acp_client::deploy`（`BinaryTarget::deploy` / `binary_root`）/ `Agent::deploy_command` | 落としています / 検証の値がありません | Downloading / No checksum |
| ↳ **アカウント**（エージェントの設定の置き場のフォルダ。`CLAUDE_CONFIG_DIR` / `CODEX_HOME` で指す・資格情報は読まない・O14） | `account_env_var` / `accounts_root` / `agent_servers.<id>.env` | アカウント / 既定 / 新しいアカウント | Account / Default / New account |
| ↳ **slash コマンド**（エージェントが広告する `/name` の命令。composer の行頭 `/` で補完・O2） | `acp_client::SlashCommand` / `AgentEvent::Commands` / `Thread.commands` | コマンド | Command |
| ↳ **レシピ**（repo ごとの定型プロンプト。`.necoder/recipes/*.md`・`/` 補完に `/necoder:<名前>`・選ぶと本文が入る・O16） | `recipes` / `Recipe` / `RECIPE_PREFIX` | レシピ | Recipe |
| ↳ **シェルモード**（composer の 1 文字目の `!`。人が打ったコマンドをスレッドの作業ディレクトリで走らせ、エージェントには送らない。結果は次の送信に添える。**composer からの人の送信だけ**が解釈する・#37） | `agent_panel::shell`（`shell_command_input` / `ShellRun`）/ `Entry::Shell`（DB の role `shell`）/ `Host::run_user_command` | シェルで実行 | Run in the shell |
| ↳ ↳ **シェルの結果**（`!` の行の出力・終了コード。次の通常の送信に `<bash-input>` 等で添える予定の物は composer のチップ。添えたら渡し済み） | `ShellRun::pending` / `pending_shell_attachment` | シェルの結果 | shell output |
| ↳ **会話名**（エージェントが付けたスレッドの題名。手動改名が優先） | `AgentEvent::TitleChanged`（ACP `session_info_update.title`）/ 手動の印 `thread_custom_names` | （スレッド名） | (thread name) |
| ↳ **目標**（`/goal` でエージェントが追う目的。composer の上に 1 行） | `acp_client::AgentGoal` / `AgentEvent::GoalChanged` / `Thread.goal` | 目標 | Goal |
| ↳ **スレッド履歴**（⌘⇧H / 🕘 の画面。3 区分: このプロジェクトのスレッド・エージェントの過去の会話・本文の一致。← スレッド履歴 Picker） | `history_view::ThreadHistoryState`（action `ThreadHistory`） | スレッド履歴 | Thread history |
| ↳ **エージェントの過去の会話**（エージェントが ACP `session/list` で返す会話。CLI で作った物を含む。necoder が持つ会話・necoder の用事の会話は除く。開くと `session/load` で再開） | `acp_client::history::AgentSessionSummary` / `AgentPanel::open_agent_session` | エージェントの過去の会話 | Agent's past conversations |
| ↳ **本文の一致**（全スレッドの発言の全文検索の結果。1 スレッド 1 件） | `Storage::search_thread_turns` / `storage::TurnSearchHit` | 本文の一致 | Message matches |
| ↳ **再生**（`session/load` でエージェントが送り直す過去の会話。開いた時だけ transcript に積む） | `AgentEvent::HistoryReplayed` / `acp_client::history::ReplayLog` | —（「以前の N 件は省略しました」の区切りだけ出る） | — |
| ↳ **新しいセッションで続ける**（handoff。今の会話の抜粋を前置きに、同じタブで新しいセッションへ。要約はしない） | `AgentPanel::continue_in_new_session` / `Thread.handoff_preamble`（action `ContinueInNewSession`） | 新しいセッションで続ける | Continue in a new session |
| ↳ **区切り**（transcript の会話ではない一行: 省略・新しいセッション・Captain の会話の交代。エージェントには送らない。検索・前置きに入れない） | `Entry::Notice`（DB の role `notice`） | （文そのもの） | （the text itself） |
| ↳ **necoder の知らせ**（人ではなく necoder が書いてスレッドへ送った発話。Captain を起こす台帳の未読など。エージェントには user ターンとして届くが、transcript では人の発話と分けた灰色のカード。出所の名前は呼び手が渡す。「頼んだこと」「最終入力」・本文の一致には数えない。送る本文は `<necoder-event>` の印つき・DB は role `auto_prompt`＝再起動と再生を越えて残る・FLEET-V2 §3.6） | `Entry::AutoPrompt` / `AgentPanel::send_auto_prompt_to` | イベント · 〈時刻〉（〈出所〉） | Event · 〈time〉 (〈source〉) |
| ↳ **前置き**（スレッドの送信に毎回付ける necoder の文脈。Captain の役割・現況・直近の采配。transcript には出さない・送る本文では `<necoder-context>` で囲み、再生では外す） | `AgentPanel::set_prompt_context` | （UI には出さない語） | — |
| ↳ **前の会話**（新しいセッションで続けた・引き継げずに替わった会話 id。履歴で重ねて出さない鍵） | `thread_past_sessions`（storage） | — | — |
| ↳ **使用量**（エージェントがターンごとに報告したトークンと推定コスト。**推定コストは実際の請求額ではない**。報告の無い値は NULL＝`—`（0 と書かない）・一部のターンにしか報告が無い合計は `≥`（下限）。台帳に 1 ターン 1 行・O11・R08） | `turn_usage`（storage）/ `storage::TurnTokenCounts` / `AgentEvent::TurnUsage` / `AgentEvent::SessionCost` / `agent_panel::usage::CostMeter` / `usage::reported_total_label` | 使用量 / 推定コスト | Usage / Est. cost |
| ↳ **レート制限**（プランの利用上限の窓と使用率。エージェントが知らせてきた**最後の値**・O11） | `acp_client::usage::RateLimits` / `AgentEvent::RateLimits` / `agent_panel::usage::UsageLimits` | レート制限 | Rate limits |
| ↳ **使用量の鍵**（レート制限の値を分ける単位 = エージェント + 動かしている場所 + 認証の置き場 + 認証に関わる env の指紋。鍵が違えば値を混ぜない・R08。指紋はハッシュだけで秘密の値は持たない。セッションを立てた時に決めてスレッドに持たせ、そのセッションの知らせはその鍵へ） | `agent_panel::usage::UsageKey`（`profile` / `credential_fingerprint`・範囲は `CREDENTIAL_WORDS`）/ `Thread.usage_key` / `AgentPanel::active_usage_key` | —（見出しに `dev-box` / 置き場のパス / `設定の env #1a2b3c` を添える） | —（`settings env #1a2b3c`） |
| ↳ **窓**（レート制限の期間） | `LimitWindow::{FiveHour, Weekly, Named, Minutes}` | 5 時間枠 / 週枠 / 〈名前〉の週枠 | 5-hour window / Weekly window / Weekly (〈name〉) |
| ↳ **使用量の統計**（日付 × エージェントの集計画面） | `usage_view::render_usage_stats` / `Storage::daily_usage` | 使用量の統計 | Usage statistics |
| **遷移スナップショット**（状態遷移時の 1 行） | `digest` / `digest_tail` / `Thread.digest` | （文そのもの・ラベル無し） | （no label） |
| **要対応**（Fleet サイドバー最上段・裁く列。← 管制の要対応キュー） | `AttentionItem` / `attention_queue` | 要対応 | Attention |
| **統合パイプライン**（TaskPhase 列の帯） | `render_pipeline` | 統合パイプライン | Integration pipeline |
| **ニュース**（task_events の鏡・時系列） | `NewsItem` / `NewsKind` | ニュース | News |
| **Captain**（任命制の采配スレッド。コードを書かず Captain 用の MCP で采配・Task を切るのは承認後・integrate は人間。← 監督） | `captain`（`NewsKind::Captain`・設定 `captain_agent`） | Captain | Captain |
| ↳ **Captain の席**（Captain スレッドに付く決まり: 道具は Captain 用の MCP だけ・権限の問いには necoder が答える・編集と shell は断る） | `agent_panel::SeatPolicy`（id `captain`）・`necoder mcp --captain` | （UI には出さない語） | — |
| ↳ **分解案**（Captain が出す「この N 本に分けたい」。承認した行だけ worktree とブランチになる） | `CaptainProposal`・`fleet_propose_tasks`・DB `captain_proposals` | 分解案 | Split / proposal |
| ↳ **分解案カード**（要対応に出る承認の面。行ごとの印・承認 / 却下） | `AttentionKind::Proposal` | ⚑ Captain の分解案 · N 本 | ⚑ Captain's split · N Tasks |
| ↳ **読んだ位置**（Captain が台帳をどこまで読んだか。再起動を跨いで未配達の知らせを残す） | DB `captain_cursors` | （UI には出さない語） | — |
| ↳ **会話の交代**（膨らんだ Captain の会話を捨て、同じタブで新しい会話を始める。前置きが続きを渡す） | `AgentPanel::rotate_thread_session` | （transcript の区切り 1 行） | — |
| ↳ **介入**（Captain を通さず Task に直接書く。台帳に残る） | `human_send`（`NewsKind::HumanSend`） | （宛先チップで示す） | — |
| ↳ **采配ログ**（Captain の判断と実行の履歴） | `NewsKind::Captain` | 采配ログ | Captain log |
| ↳ **台帳の知らせ**（Captain を起こす 1 通 = 台帳の未読。necoder の知らせの 1 つで、出所は「台帳」） | `send_captain_wake` / `captain.ledger_source` | イベント · 14:05（台帳） | Event · 14:05 (ledger) |
| **集約気分**（編隊の最悪状態に追従する 1 匹） | `fleet_mood_mascot` | — | — |
| **常駐**（Herdr sidecar 実行形態・P7） | `HerdrRuntime`（予定） | 常駐 | Resident (Herdr) |
| **リモート管制**（スマホから見る/裁く・P9） | `serve --control` / `remote_control` | リモート管制 | Remote control |
| ↳ **ペアリング**（QR で端末を繋ぐ・1 回きり） | `pairing` / `room_id` | ペアリング | Pairing |
| ↳ **デバイス**（ペア済みの端末・失効の単位） | `PairedDevice` | デバイス | Device |
| ↳ **リレー**（room id が一致する 2 本を繋ぐ交換機） | `relay`（`relay/`・DO） | リレー | Relay |
| ↳ **封**（transport 非依存の暗号化フレーム） | `seal` / `open` / `SealedFrame` | — | — |
| **片付けメニュー**（Task カードの ⋯・残るものが減る順の段） | `FleetCellMenuState` / `FleetCellAction` | 片付け | Clean up |
| **片付けの画面**（Task をまとめて終了 / 失うものが無い worktree をまとめて削除・O22） | `cleanup` / `ShowCleanup` / `CleanupState` | Task の片付け | Clean up Tasks |
| ↳ カードを閉じる（舞台から外すだけ） | `close_fleet_cell` | カードを閉じる | Close card |
| ↳ Task を終了（台帳を archived に） | `archive_fleet_cell_task` | Task を終了 | Finish Task |
| ↳ worktree を削除（ディスクから消す） | `delete_fleet_cell_worktree` | worktree を削除 | Delete worktree |
| ↳ 削除の確認（失うものを数えて見せる） | `WorktreeDeleteConfirm` / `WorktreeStakes` | — | — |
| **下段ドック**（Fleet 下の可変高タブ面） | `FleetBottomView` / `bottom_height` | 下段 | Bottom pane |
| **OS の通知**（通知センター。その窓を見ていない時だけ・スレッドごとに 1 件） | `system_notifications` / `post_agent_notification` / 設定 `system_notifications` | OS の通知 | System notifications |
| **Dock バッジ**（要対応の件数・全窓の合計・macOS） | `dock_badge` | — | — |
| **通知の履歴**（titlebar のベル。エージェントの出来事を窓ごとに最新 100 件・未読 / 既読・O13） | `inbox` / `InboxItem` / `ShowInbox` | 通知の履歴 / すべて既読 / 未読に戻す | Notification history / Mark all read / Mark as unread |
| **プレビュータブ**（エクスプローラの 1 回クリックで開く・次の 1 回クリックで置き換わる・名前が斜体・O26） | `EditorTab::preview` / `open_file_preview`（設定 `preview_tabs`） | プレビュータブ | Preview tab |
| **ピン留め**（タブを左端に留め、まとめて閉じる操作と ⌘W で閉じない・窓セッションに残る・O26） | `EditorTab::pinned` / `TogglePinTab` / 窓セッション `pinned_files` | ピン留め / ピン留めを外す | Pin tab / Unpin tab |
| **自動保存**（他へ移った時 / 手を止めた時に未保存のタブを書く・外で変わったタブは書かない・O26） | `auto_save`（設定 `auto_save`・`AutoSave`） | 自動保存 / 他へ移った時 / 手を止めた時 | Auto save / On focus change / After a pause |
| **作業中はスリープさせない**（作業中のスレッドが全窓で 1 本でもある間だけ、放っておいた時のスリープを止める・O13） | `keep_awake`（設定 `keep_awake`・`KeepAwake`） | 作業中はスリープさせない | Keep awake while agents work |
| **質問待ち**（エージェントが選択肢付きで聞いてきて止まっている。状態は承認待ちと同じ Blocked） | `PanelEvent::QuestionWaiting` / `AttentionKind::Question` | 質問待ち | Waiting for an answer |
| **終了の確認**（⌘Q・窓を閉じる時。動いているエージェント・ターミナルがある時だけ。窓を閉じる時はその窓の分だけ数える） | `quit_guard` / `QuitConfirmState`（設定 `confirm_quit`） | 終了時の確認 | Confirm before quitting |
| ↳ **隠して動かし続ける**（既定。mac はアプリを隠す・他 OS は最小化。ほかの窓が残る時はその窓だけを最小化。プロセスは止めないが、終了・クラッシュを越えては続かない） | `QuitChoice::KeepRunning` | 隠して動かし続ける / 最小化して動かし続ける | Hide and keep running / Minimize and keep running |
| **AI 全画面**（solo で中央エディタを Agent に差し替える。左/下ドックは各自の ON/OFF） | `agent_full_screen` / `ToggleAgentFullScreen` | AI を全画面 | AI full screen |
| **「最新へ」ボタン**（transcript を遡り中だけ右下に出る・最下部へ戻す） | `render_jump_to_latest` | 最新へ | Jump to latest |
| **プレビュー**（`.md` のネイティブ整形表示 / `.html` のOS標準WebView表示。source ⇄ preview・⌘⇧V。開発サーバを見るのは別のタブ＝下の Web タブ） | `rendered_markdown` / `rendered_html` / `ToggleRenderedMarkdown` / `markdown_preview` / `webview_view` | プレビュー | Preview |
| ↳ **Web タブ**（localhost の開発サーバを見るタブ。読み込めるのはループバックの http(s) だけ・汎用ブラウザではない。鍵は URL。入口はパレット「プレビュー: localhost を開く…」と transcript / Markdown のリンク） | `TabContent::Web` / `WebPreviewView` / `webview_view::localhost` / `Workspace::open_url` | Web タブ | Web tab |
| ↳ **Design モード**（Web タブのページの要素を選び、切り抜きと説明を composer へ「要素のチップ」として添える。送信はしない。⌘⇧D） | `ToggleDesignMode` / `webview_view::design` / `ElementCapture` / `WebPreviewEvent::ElementPicked` | Design モード / 要素のチップ | Design Mode / element chip |
| ↳ **Web Inspector**（Web タブのツールバーの `</>`・自分の localhost のアプリを調べる開発者向けの道具。Design モードは使わない。macOS は WebKit の物（窓の題名は OS が付ける「Webインスペクタ」）を necoder の窓の中に付けず**別の窓**で開く・Windows は WebView2 の DevTools。「DevTools」とは書かない） | `ToggleWebInspector` / `WebViewView::toggle_devtools` / `webview_view::inspector` | Web Inspector | Web Inspector |
| ↳ **内蔵の配信**（HTML ファイルを Web タブで開くための、necoder の中の小さな静的配信。`127.0.0.1`・token 付きの URL・そのファイルのフォルダだけ・使う Web タブがある間だけ動く） | `webview_view::static_server` / `StaticHosting` / `StaticMount` / `Workspace::open_static_web_tab` / `OpenHtmlInWebTab` | 内蔵の配信 / Web タブで開く | Built-in server / Open in Web tab |
| **ne コマンド**（ターミナルから開く CLI・`code`/`cursor` 相当） | `cli_shim`（シム生成）/ `necoder cli`（実体） | ne コマンド | ne command |
| **ターミナル**（下ドックの統合ターミナル。Task カードの「ターミナル」サイドペインと同じ実体） | `terminal_view::TerminalView` / `TerminalDock` | ターミナル | Terminal |
| ↳ **ターミナル内検索**（⌘F・scrollback を含む。エディタの「検索」= バッファ内検索とは別） | `terminal::Find` / `terminal_view::search` | ターミナル内を検索 | Find in terminal |
| ↳ **ターミナルのリンク**（`path:line` と URL・OSC 8。URL を開く受け口は 1 か所） | `TerminalLink` / `TerminalEvent::OpenUrl` / `open_terminal_url` | （下線のみ） | (underline only) |
| ↳ **どこからでも呼べるターミナル**（⌃`・どの画面の上にも浮かべる・ホームで開く・プロジェクトに紐付かない・隠しても止めない） | `terminal_float` / `FloatingTerminal` / `ToggleFloatingTerminal` | ターミナル: どこからでも（浮かべる） | Floating terminal |
| ↳ **エディタ領域のターミナル**（ファイルと同じタブ列に開くターミナル。PTY は下ドックと同じ session の `TerminalDock` が id で持つ・一時タブ・閉じると止まる。下ドックと行き来できる） | `TabContent::Terminal` / `terminal_tabs` / `NewTerminalTab` / `MoveTerminalToEditor` | エディタ領域のターミナル | Terminal in the editor area |
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
| `herd` / herd サイドバー | `fleet_sidebar` / Fleet サイドバー（Editor の左カラムでは AI スレッド一覧） | Fleet に統一（`herd` は UI に一度も出ない code 専用語） |
| 監督 / `coordinator` / Coordinator | Captain / `captain` | Fleet の比喩に合わせ中核機能として改名（2026-09-12）。旧設定キー `coordinator_agent` の読み替えは作らない |
| セル / `FleetPane` / surface | Task カード（スレッドタブ + サイドペイン） | 同じ worktree のターミナルが別セルになって増殖していた。全部 Task の中へ（FLEET-V2 §3.5） |
| 作業 / 作業ツリー / 列 / ペイン / 面 / 配置（`workbench` / `WorkColumn` / `WorkPane` / `WorkSurface` / `RepositoryLayout`） | 廃止 | 0.1.15 の作業タブは実機で取り回しが悪く降格 → FLEET-V2 で削除 |
| 管制（中央タブ）/ `FleetCenterView` | 要対応（サイドバー常設）+ ブリッジ（統合先のカード） | 中央タブ 3 面は「今どこを見ているか」を失わせる。**リモート管制**の名前だけ P9 完了まで据え置き |
| ＋ACP / ＋Terminal / ＋Worktree | ＋ Task（会話はスレッドタブ隣の ＋・端末はターミナルペイン見出しの ＋） | 押す前に「どれか」を考えさせない。1 プロンプト = 1 worktree |
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
