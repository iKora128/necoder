# 管制（Control）実装計画 — 編隊統括ダッシュボード・遷移スナップショット・監督・Herdr sidecar

更新: 2026-07-23。**この文書は実装エージェント向けの自己完結プラン**（設計セッションの決定を全て織り込み済み）。
設計の正: `FLEET-ARCHITECTURE.md` / ビジュアルの正: `mock/fleet-dashboard.html`（部品カタログ: `mock/fleet-digest-patterns.html`）/ 進捗の正: `ROADMAP.md` M14。
フェーズは P0 から順に。**各フェーズが単独で価値を持ち、単独でマージ可能**であること。

## 0. 確定済みの設計判断（この計画の前提・変更しない）

1. **監視役は 3 分解**: 決定論の事実層（プロトコル/Git 由来・LLM なし）/ 任命制の監督（ただの ACP スレッド）/ 人間への注意ルーティング（UI）。**台帳が記憶・監督は状態を持たない**（監督の交代・再起動が自由になる）。
2. **名称「管制」確定**（ユーザー承認 2026-07-23）。編隊モード中央面の新タブ。英語 UI は `Control`。
3. **規律**: 色=スレッド識別のみ / 状態=形と動き（`activity_dot` 系）/ ✳ テラコッタ=LLM 生成テキストの印 / 破線=画面推定（Herdr 由来）。`TaskPhase`・`ThreadActivity`・Git health を 1 つの enum に潰さない（FLEET-ARCHITECTURE 不変条件）。
4. **遷移スナップショット（digest）**: 状態遷移時のみ生成・ポーリングなし。**Tier 1** = 決定論テンプレート合成（LLM なし・即時・常に正しい）、**Tier 2** = `oneshot()` による 1 行要約（遷移時のみ・Done/Failed のみ・ledger にキャッシュ）。**要約は状態を上書きしない**（状態は状態機械が決め、文は添えるだけ）。
5. **Herdr は sidecar**（別プロセス・socket API 経由）。ソース移植はしない（単一バイナリ構成でライブラリ crate が無いため）。**頭脳（Fleet/TaskSpace/監督/Plan/Permission/Review/ledger）は Shirushi、PTY とプロセス生存だけ Herdr**。worktree 操作は必ず Shirushi が所有（Herdr の `worktree.*` は使わない）。
6. **ACP 優先の起動形態選択**: ACP を話せるエージェントの既定は ACP（正確な状態・permission UI）。**明示選択したときだけ常駐モード（Herdr・CLI 形態）**で、推定監視に落ちることを UI が明示する（破線）。
7. **Herdr の観測値は業務上の正にしない**: Shirushi 側で `TurnId`/`generation` を持ち、Herdr の pane 状態遷移と照合する（working 中に prompt した場合に前ターン完了を誤認する競合の防止）。
8. **守るべき操作（spawn / phase 遷移 / integrate / send）は Shirushi の MCP/CLI にだけ置く**。Herdr socket を直接叩く経路（Herdr が pane に注入する `HERDR_SOCKET_PATH`）は台帳と permission の迂回路になるため、監督のツールセットには含めない。
9. リモート管制（QR・スマホ閲覧+許可判断）は**最後**（P9・構想枠）。

## 1. 現状マップ（着手前に読む場所。行番号は 2026-07-23 時点＝ドリフトしたら grep）

| 場所 | 何があるか |
|---|---|
| `crates/shirushi/src/fleet.rs` | TaskSpace の CRUD + Git gate（`create_task`/`list_tasks`/`update_task`/`wait_task`(250ms poll・再起動を跨ぐ)/`review_task`/`integrate_task`/`run_cli`）。`PHASES` 文字列定数（146 付近） |
| `crates/shirushi/src/mcp.rs` | MCP サーバ 11 tools（ファイル/Git 5 + fleet 6）。冒頭コメントに「GUI ライブ制御 IPC は後続」 |
| `crates/workspace/src/workspace.rs` | `TaskPhase` enum（668 付近・11 値）/ `TaskSpace`（719）/ `FleetPane`（793）/ `ChromeState.fleet_*`（819） |
| `crates/workspace/src/workspace/fleet_view.rs` | 編隊 UI 全部（`toggle_fleet_mode`/`seed_fleet_cells`/`add_worktree_agent`(389)/`add_fleet_agent`/`review_task_for_merge`/`integrate_task`/`fleet_lanes`(572)/`render_fleet`(634)/系譜グラフ 4 表示） |
| `crates/agent_panel/src/agent_panel.rs` | `Thread`（259）/ `ThreadActivity`（204・導出は `activity()` 310）/ `AgentStatus`・`statuses()`（982）/ `RunningRegistry`（236）/ `MascotMotion`（4057・plead/worry は 15s 閾値 2921 付近）/ トークン表示 `render_meta`（2881） |
| `crates/acp_client/src/acp_client.rs` | `AgentEvent`（97: Chunk/ToolStarted/Usage/PermissionRequest/Plan/TurnEnded/Failed）/ `TurnEnd`（135）/ `PlanItem`・`PlanStatus`（77-89・全量置換）/ `oneshot()`（321・claude/codex のみ）/ `AGENTS` カタログ 7 種（205-297） |
| `crates/storage/` | `TaskSpaceRecord`（`result_summary` フィールドあり）/ `task_events` 追記ログ |
| `crates/terminal_view/src/terminal_view.rs` | `TerminalView`（173）が PTY（`notifier`）と alacritty `Term` を直接所有（P8 の trait 切り出し対象） |

**既知の縫い目（P0 で直す・この上に積まない）**:
- phase 定義が 2 箇所: `fleet.rs::PHASES`（文字列・`integration` 無し）と `workspace::TaskPhase`（enum・`Integration` あり）。
- TaskSpace の真実源が 2 箇所: `storage::TaskSpaceRecord`（永続）と `workspace::TaskSpace`（メモリ）。同期は `persist_task_space`/`to_record` の手動。
- **spawn の断絶**: `fleet_create_task`（CLI/MCP）は worktree を作るがエージェントを起動しない。起動できるのは GUI の `add_worktree_agent` だけ。

## 2. フェーズ計画

検証ループは全フェーズ共通: `cargo check -p shirushi` → `cargo test -p shirushi` → UI 変更時 `./scripts/screenshot-app.sh`（または `SHIRUSHI_SCREENSHOT` + `SHIRUSHI_FLEET=1` 等の状態フック）→ PNG を Read して目視。i18n は `t!()` で ja/en 両方（parity テストが落ちる）。

### P0 — 縫い目の一本化（前提工事・UI 変更なし）

- `TaskPhase` の単一定義化: 共有の場所（`storage` crate 推奨・両者が既に依存）へ移し、`fleet.rs::PHASES` を廃止して enum の `as_str/from_str` に寄せる。
- `Integration` は task phase から外し、`SpaceKind { Integration, Task }` として分離（「IntegrationSpace は phase ではなく space の種別」）。UI の写像はそのまま。
- 真実源を storage に固定: GUI の `transition_task_space` と CLI/MCP の `update_task` が**同じ関数**（storage 書き込み + `task_events` 追記）を通るように。GUI メモリ側は読み出しキャッシュと明示。
- 受入: `shirushi fleet status` と GUI の phase 遷移が同一コードパスを通る。全 test green。

### P1 — 遷移スナップショット Tier 1（digest・LLM なし）

- `Thread` に `digest: Option<String>` を追加。**生成タイミングは `ThreadActivity` の遷移時のみ**（`AgentEvent::TurnEnded`/`PermissionRequest`/`Failed` の受信箇所）。Working 中はライブ素材（最新 `ToolStarted` 名 + `PlanItem` の in_progress）をそのまま流す（生成不要・無料）。
- 素材の優先順位: ① Blocked → `PermissionRequest.title` ② Done → 最終ターン末尾の文（末尾 chunk から 1〜2 文抽出）+ `TurnEnd` ③ Failed → エラー文字列 ④ 共通で plan `completed/total`・diff stat・トークン。
- `AgentStatus` に digest / plan 進捗 / diff stat / 経過時刻を追加 → herd 行の `sub` を digest に差し替え（`render_herd_sidebar`）+ 編隊セルヘッダにも表示。
- 遷移時に `task_events` へ digest を添えて追記（再起動後も残る）。`review_ready` 遷移時は `TaskSpaceRecord.result_summary` に確定値を書く。
- 受入: **herd の各行に、Blocked は許可待ちの内容・Done は最後の発言の末尾が 1 行で出て、遷移のたびに更新される**。offscreen で 3 状態を目視。

### P2 — ニュース常設（ユーザー高評価・早く出す）

- ROADMAP M14「ニュースフィード + 通知の細部」の実装。ソースは `task_events`（P1 で digest が載っている）。下ドックに時系列フィード（mock の F 案 → `fleet-dashboard.html` 下段の書式）: 時刻 + スレッド色チップ + 太字タスク名 + イベント文。
- 行頭チップ=スレッド色（帰属）。**将来の監督の采配も同じログに載る**前提でイベント種別を設計（`phase_change / permission / digest / integration / coordinator`）。
- エージェント別ミュート + window 非フォーカス時のみ通知音（M13 完了音と合流）。
- 受入: 承認待ち/完了/失敗が下フィードに時系列で流れ、個別ミュートできる。

### P3 — 管制タブ（ダッシュボード本体）

- `chrome.fleet_center_view: FleetCenterView { Control, Graph }` を追加し、編隊中央をタブ切替に（既定は当面 Graph のまま・ドッグフーディング後に再判断）。**ビジュアルの正は `mock/fleet-dashboard.html`**（採用確定・上部は洗練済み版）。構成は上から:
  1. **ヘッダ**: 「管制 shirushi ⎇ main + N worktrees」+ 目標文（編隊 goal・ledger に持つ）+ グリフ付き stat チップ（◐要対応/✓完了未確認/●稼働/⇡統合今日・要対応はエラー色ボーダー+速い脈動）+ トークン合計/予算メーター。
  2. **監督バー**: 左に顔（**集約気分マスコット** = 編隊の最悪状態に追従・`MascotMotion` 再利用。JOURNAL 2026-07-23 の「集約気分 1 匹」の回収）+「監督 · <agent>」+ 宛先 + ✳総括文（P4 まではプレースホルダ「監督未任命」表示）+ 更新時刻 + 「⏎ 次」ボタン。
  3. **要対応キュー**（左 336px）: Blocked（経過時間順）→ Failed → Done 未確認の順。カード内に許可/常に許可/拒否・修正を指示・diff を確認/Integrate 等の**インライン操作**（既存の permission respond / `review_task_for_merge` / `integrate_task` に配線）。先頭カードに「次 ⏎」バッジ。
  4. **稼働カード**（右グリッド）: Working 中の TaskSpace。digest + plan メーター（▰▱）+ diff + tok + 経過。+ Task ゴーストセル。クリックで没入（`switch_project` + `focus_thread`＝既存）。
  5. **統合パイプライン**: TaskPhase 列（planned→working→blocked→review_ready→merge_ready→integrated + failed 別枠）にタスクチップ。`fleet_lanes` のデータで足りる。
  6. **ニュース**（P2 を埋込）。
- Done→Idle の**確認済み遷移**を追加（クリック/確認操作で Done ラッチを落とす。herdr の done/idle 区別の採用）。
- キー: ⏎（キュー先頭へ）/ 既存 `ToggleFleet` 系に合わせてパレット登録。全 UI 文字列 `t!("control.*")` ja/en。
- 受入: 5 TaskSpace（Working/Blocked/Done/Failed/推定）で mock 同等の 1 画面が出て、**許可判断がダッシュボードから出ずに完了**する。offscreen 目視。

### P4 — Tier 2（✳ 要約）+ 監督バーの総括

- タスクレベル: Done/Failed 遷移時に `oneshot()`（`acp_client.rs:321`・thread タイトル生成と同じ機構）で 1 行要約 → `task_events` にキャッシュ・キュー카드の ✳ イタリック行に表示。設定でオフ可。対応外エージェント（oneshot テンプレ無し）は Tier 1 のみに自然フォールバック。
- 編隊レベル: キューに影響する遷移でデバウンス（5s 程度）して総括文を oneshot 生成 → 監督バーの ✳ 文。**要約は状態を上書きしない**（数字とキューは常に事実層から）。
- 受入: Done 遷移から数秒で ✳ 行が付く。oneshot 失敗時も UI が欠けない（Tier 1 表示のまま）。

### P5 — fleet API 拡張（spawn 断絶の解消 + 読む道具）

- GUI ライブ制御 IPC（`mcp.rs` 冒頭で予告済み）: Unix socket（`~/.shirushi/` 配下・权限 0600）で起動中 GUI と headless CLI/MCP を接続。
- 新 MCP tools + CLI サブコマンド: `fleet_spawn_agent(task_id, agent, prompt)`（worktree に AgentPanel/thread を起こす＝`add_worktree_agent` の分解再利用）/ `fleet_send(task_id, message)` / `fleet_digest(task_id)`（Tier1 digest + plan + diff + phase を返す）/ `fleet_events(since)`（task_events の差分）。
- **digest の 3 段圧縮を守る**: 監督向け既定は事実層+Tier1 のみ。フル transcript は返さない（コンテキスト経済・Cognition の教訓）。
- 受入: GUI を開いた状態で `shirushi fleet spawn-agent … && shirushi fleet wait … review_ready` がヘッドレスで通り、GUI に thread が現れる。

### P6 — 監督席（Coordinator seat）+ 依存待ち

- `TaskSpaceRecord` に `depends_on: Vec<SpaceId>` を追加。`fleet_wait` を activity/phase 両対応に拡張。
- 監督 = **任命制のただのスレッド**: IntegrationSpace に住む pinned thread + fleet ツールセット + システムプロンプトテンプレート（役割・規律・「手を動かさない」）。任命 UI は Settings（既定ドリフト禁止の原則どおり）。7 種カタログのどれでも可。
- **wake はイベント駆動**: 常駐ポーリング禁止。Blocked（15s 閾値・worry と同じ）/ Done / Failed 遷移で、変化分の digest を添えて監督に 1 ターン渡す。監督の発言/采配は `coordinator` イベントとして task_events へ（ニュースに載る＝監査可能）。
- integrate は radar clean + 人間 gate 既定（監督は提案まで）。
- 受入: **ROADMAP M14 総合受入の自走部分** — 「B の完了を待って merge」を監督が fleet ツールだけで実行（integrate の最終承認は人間）。

### P7 — Herdr sidecar（常駐ランタイム・最優先の"常駐"価値）

- `AgentRuntime` trait を切る（spawn / prompt / wait / read / events / digest ソース）。実装: `AcpRuntime`（既存を包む・authority=Acp）/ `HerdrRuntime`（authority=HerdrIntegration|HerdrHeuristic）。
- 状態モデル拡張: `RuntimeActivity { Starting, Idle, Working, Blocked, DoneUnread, Unknown, Exited, Failed }` + `RuntimeAuthority { Acp, HerdrIntegration, HerdrHeuristic }`。表示は既存 `ThreadActivity` へ写像（Unknown は既存グリフ `·`・**成功扱いしない**・Herdr の Done を `ReviewReady` に直結させず「遷移の提案」まで）。
- Herdr 接続: 専用 named session（`--session shirushi`）を Shirushi が起動/再利用。`ping` → schema 取得（`herdr api schema --json` からクライアント型を生成・内部型のコピー禁止）→ snapshot 相当で全景取得 → `events.subscribe`。切断時は snapshot からやり直し。
- 対応表 `HerdrRunBinding { agent_run_id(正), session_name, workspace_id, pane_id, terminal_id, agent_name, native_session_ref, generation }` を **Shirushi の ledger に永続**（Herdr ID を主キーにしない・Herdr metadata の永続性に依存しない）。`TurnId`/`generation`/`prompt_sent_at` で wait の誤認を防ぐ。
- 起動形態選択 UI: スレッド起動時に「ACP（既定・深い統合）/ 常駐（Herdr・再起動を跨ぐ・推定監視）」。常駐は破線グリフ + `Herdr·推定` バッジ。**「接続復帰」と「Agent 再開」を UI で区別**（Herdr サーバごと死んだ場合に戻るのはレイアウト+native session が中心のため）。
- Herdr 不在時は機能が静かに消えるだけ（任意依存）。ライセンス: **Apache 2.0 の正式リリースを version 固定**（master 直依存にしない）・LICENSE/NOTICE 同梱・checksum 固定。
- 受入（ユーザー承認済みの文言）: **「Shirushi から Claude Code（CLI 形態）を Herdr Runtime で起動し、Shirushi を終了しても処理が継続し、再起動後に同一 AgentRun として状態・出力・native session を再取得できる」**。ドッグフーディング効果（`cargo run` 再ビルドでエージェントが死なない）を JOURNAL に実測記録。

### P8 — 読み取り専用 observer + 常駐 shell/テスト

- `TerminalBackend` trait（`subscribe_frames`/`send_input`/`resize`）を `TerminalView` から切り出し、`LocalPtyBackend`（現行）+ `HerdrObserverBackend`（読み取り専用・direct terminal attach の ANSI ストリーム表示）。書き込み制御（takeover/フォーカス所有権）は後段。
- `FleetPane::Tests` を Herdr pane 常駐に。**画面に文字が出たことをテスト成功の正にしない**: コマンド末尾に構造化 sentinel（exit code + marker）を出すラッパーで、結果は Shirushi が台帳化。
- 受入: dev server / テストが Shirushi 再起動を跨いで生き、結果が台帳とニュースに載る。

### P9 — リモート管制（QR・構想枠・最後）

外出先のスマホから管制を見る・裁く: `shirushi serve --control` がローカル web（管制の読み取りビュー + 許可/確認/Integrate 承認のみの最小操作面）を立て、**QR コード**（URL + ワンタイムトークン）を発行 → スマホで開く。前提: 認証トークン必須・既定は読み取り専用・書き込みは許可判断系のみ・到達性は Tailscale/LAN を明示（公開インターネットに直で晒さない）。P7 の常駐と組み合わせて「家で編隊を走らせて外から様子を見る」が完成形。**着手は M14 完了後・設計から**。

## 3. 実装エージェントへの注意

- `CLAUDE.md` と `zed/CLAUDE.md`（GPUI 節・Rust 規約）が正。GPUI API はネット記事や記憶を疑い `zed/crates/gpui/examples/` を見る。
- mock を先に直してから実装（乖離したら文書/mock を直す方が先）。`mock/fleet-dashboard.html` を index.html へ移植する際は編隊モード節へ統合し、検証はヘッドレス Chrome スクショ + Read 目視。
- 文書更新も作業のうち: GLOSSARY（管制/Control・遷移スナップショット/digest・監督/Coordinator・常駐/Herdr Runtime・集約気分）、UI-SPEC §11（管制タブ・✳ と破線の意味論・stat チップ）、DECISIONS（本計画 §0 の 1〜9 を決定ログとして転記）、ROADMAP M14（各フェーズ完了時にチェック + JOURNAL 記録）。
- 性能予算: 管制タブは 8 TaskSpace で 60fps・digest 生成はレンダ外（Git/DB/Host I/O を render 中に行わない不変条件）。

## 4. 受入の全体像（M14 総合受入との対応）

- P1+P2+P3 で「N 体並走を色相を足さずに 1 画面で捌く」が完成。
- P5+P6 で「"B の完了を待って merge" を自走」が完成（＝M14 総合受入）。
- P7 で常駐（このプロジェクトのドッグフーディング速度に直結する最重要機能）。
- P9 はその先の構想（QR リモート管制）。
