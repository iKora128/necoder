# 管制（Control）実装計画 — 編隊統括ダッシュボード・遷移スナップショット・監督・Herdr sidecar

更新: 2026-07-23。**この文書は実装エージェント向けの自己完結プラン**（設計セッションの決定を全て織り込み済み）。
設計の正: `FLEET-ARCHITECTURE.md` / ビジュアルの正: `mock/fleet-dashboard.html`（部品カタログ: `mock/fleet-digest-patterns.html`）/ 進捗の正: `ROADMAP.md` M14。
フェーズは P0 から順に。**各フェーズが単独で価値を持ち、単独でマージ可能**であること。

## 0. 確定済みの設計判断（この計画の前提・変更しない）

1. **監視役は 3 分解**: 決定論の事実層（プロトコル/Git 由来・LLM なし）/ 任命制の監督（ただの ACP スレッド）/ 人間への注意ルーティング（UI）。**台帳が記憶・監督は状態を持たない**（監督の交代・再起動が自由になる）。
2. **名称「管制」確定**（ユーザー承認 2026-07-23）。編隊モード中央面の新タブ。英語 UI は `Control`。
3. **規律**: 色=スレッド識別のみ / 状態=形と動き（`activity_dot` 系）/ ✳ テラコッタ=LLM 生成テキストの印 / 破線=画面推定（Herdr 由来）。`TaskPhase`・`ThreadActivity`・Git health を 1 つの enum に潰さない（FLEET-ARCHITECTURE 不変条件）。
4. **遷移スナップショット（digest）**: 状態遷移時のみ生成・ポーリングなし。**Tier 1** = 決定論テンプレート合成（LLM なし・即時・常に正しい）、**Tier 2** = `oneshot()` による 1 行要約（遷移時のみ・Done/Failed のみ・ledger にキャッシュ）。**要約は状態を上書きしない**（状態は状態機械が決め、文は添えるだけ）。
5. **Herdr は sidecar**（別プロセス・socket API 経由）。ソース移植はしない（単一バイナリ構成でライブラリ crate が無いため）。**頭脳（Fleet/TaskSpace/監督/Plan/Permission/Review/ledger）は necoder、PTY とプロセス生存だけ Herdr**。worktree 操作は必ず necoder が所有（Herdr の `worktree.*` は使わない）。
6. **ACP 優先の起動形態選択**: ACP を話せるエージェントの既定は ACP（正確な状態・permission UI）。**明示選択したときだけ常駐モード（Herdr・CLI 形態）**で、推定監視に落ちることを UI が明示する（破線）。
7. **Herdr の観測値は業務上の正にしない**: necoder 側で `TurnId`/`generation` を持ち、Herdr の pane 状態遷移と照合する（working 中に prompt した場合に前ターン完了を誤認する競合の防止）。
8. **守るべき操作（spawn / phase 遷移 / integrate / send）は necoder の MCP/CLI にだけ置く**。Herdr socket を直接叩く経路（Herdr が pane に注入する `HERDR_SOCKET_PATH`）は台帳と permission の迂回路になるため、監督のツールセットには含めない。
9. **リモート管制（QR・スマホから見る/裁く）は P9**（設計確定 2026-07-25・詳細は §2 P9）。**Tailscale/VPN は使わない**。経路を 3 つに割り（PWA 配信=静的 / 通知=Mac から Apple へ直 / データ経路=リレー）、運用が要るのは**リレー 1 つだけ**。リレーは Cloudflare Workers + Durable Objects の**交換機**（判断も記憶もしない）で、**QR がそのまま資格情報＝アカウント不要**、封は transport 非依存、**認証の権威は Mac 側**。P7/P8 より先に着手する（2026-07-25・ユーザー判断）。

## 1. 現状マップ（着手前に読む場所。行番号は 2026-07-23 時点＝ドリフトしたら grep）

| 場所 | 何があるか |
|---|---|
| `crates/necoder/src/fleet.rs` | TaskSpace の CRUD + Git gate（`create_task`/`list_tasks`/`update_task`/`wait_task`(250ms poll・再起動を跨ぐ)/`review_task`/`integrate_task`/`run_cli`）。`PHASES` 文字列定数（146 付近） |
| `crates/necoder/src/mcp.rs` | MCP サーバ 11 tools（ファイル/Git 5 + fleet 6）。冒頭コメントに「GUI ライブ制御 IPC は後続」 |
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

検証ループは全フェーズ共通: `cargo check -p necoder` → `cargo test -p necoder` → UI 変更時 `./scripts/screenshot-app.sh`（または `NECODER_SCREENSHOT` + `NECODER_FLEET=1` 等の状態フック）→ PNG を Read して目視。i18n は `t!()` で ja/en 両方（parity テストが落ちる）。

### P0 — 縫い目の一本化（前提工事・UI 変更なし）

- `TaskPhase` の単一定義化: 共有の場所（`storage` crate 推奨・両者が既に依存）へ移し、`fleet.rs::PHASES` を廃止して enum の `as_str/from_str` に寄せる。
- `Integration` は task phase から外し、`SpaceKind { Integration, Task }` として分離（「IntegrationSpace は phase ではなく space の種別」）。UI の写像はそのまま。
- 真実源を storage に固定: GUI の `transition_task_space` と CLI/MCP の `update_task` が**同じ関数**（storage 書き込み + `task_events` 追記）を通るように。GUI メモリ側は読み出しキャッシュと明示。
- 受入: `necoder fleet status` と GUI の phase 遷移が同一コードパスを通る。全 test green。

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
  1. **ヘッダ**: 「管制 necoder ⎇ main + N worktrees」+ 目標文（編隊 goal・ledger に持つ）+ グリフ付き stat チップ（◐要対応/✓完了未確認/●稼働/⇡統合今日・要対応はエラー色ボーダー+速い脈動）+ トークン合計/予算メーター。
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

- GUI ライブ制御 IPC（`mcp.rs` 冒頭で予告済み）: Unix socket（`~/.necoder/` 配下・权限 0600）で起動中 GUI と headless CLI/MCP を接続。
- 新 MCP tools + CLI サブコマンド: `fleet_spawn_agent(task_id, agent, prompt)`（worktree に AgentPanel/thread を起こす＝`add_worktree_agent` の分解再利用）/ `fleet_send(task_id, message)` / `fleet_digest(task_id)`（Tier1 digest + plan + diff + phase を返す）/ `fleet_events(since)`（task_events の差分）。
- **digest の 3 段圧縮を守る**: 監督向け既定は事実層+Tier1 のみ。フル transcript は返さない（コンテキスト経済・Cognition の教訓）。
- 受入: GUI を開いた状態で `necoder fleet spawn-agent … && necoder fleet wait … review_ready` がヘッドレスで通り、GUI に thread が現れる。

### P6 — 監督席（Coordinator seat）+ 依存待ち

- `TaskSpaceRecord` に `depends_on: Vec<SpaceId>` を追加。`fleet_wait` を activity/phase 両対応に拡張。
- 監督 = **任命制のただのスレッド**: IntegrationSpace に住む pinned thread + fleet ツールセット + システムプロンプトテンプレート（役割・規律・「手を動かさない」）。任命 UI は Settings（既定ドリフト禁止の原則どおり）。7 種カタログのどれでも可。
- **wake はイベント駆動**: 常駐ポーリング禁止。Blocked（15s 閾値・worry と同じ）/ Done / Failed 遷移で、変化分の digest を添えて監督に 1 ターン渡す。監督の発言/采配は `coordinator` イベントとして task_events へ（ニュースに載る＝監査可能）。
- integrate は radar clean + 人間 gate 既定（監督は提案まで）。
- 受入: **ROADMAP M14 総合受入の自走部分** — 「B の完了を待って merge」を監督が fleet ツールだけで実行（integrate の最終承認は人間）。

### P7 — Herdr sidecar（常駐ランタイム・最優先の"常駐"価値）

- `AgentRuntime` trait を切る（spawn / prompt / wait / read / events / digest ソース）。実装: `AcpRuntime`（既存を包む・authority=Acp）/ `HerdrRuntime`（authority=HerdrIntegration|HerdrHeuristic）。
- 状態モデル拡張: `RuntimeActivity { Starting, Idle, Working, Blocked, DoneUnread, Unknown, Exited, Failed }` + `RuntimeAuthority { Acp, HerdrIntegration, HerdrHeuristic }`。表示は既存 `ThreadActivity` へ写像（Unknown は既存グリフ `·`・**成功扱いしない**・Herdr の Done を `ReviewReady` に直結させず「遷移の提案」まで）。
- Herdr 接続: 専用 named session（`--session necoder`）を necoder が起動/再利用。`ping` → schema 取得（`herdr api schema --json` からクライアント型を生成・内部型のコピー禁止）→ snapshot 相当で全景取得 → `events.subscribe`。切断時は snapshot からやり直し。
- 対応表 `HerdrRunBinding { agent_run_id(正), session_name, workspace_id, pane_id, terminal_id, agent_name, native_session_ref, generation }` を **necoder の ledger に永続**（Herdr ID を主キーにしない・Herdr metadata の永続性に依存しない）。`TurnId`/`generation`/`prompt_sent_at` で wait の誤認を防ぐ。
- 起動形態選択 UI: スレッド起動時に「ACP（既定・深い統合）/ 常駐（Herdr・再起動を跨ぐ・推定監視）」。常駐は破線グリフ + `Herdr·推定` バッジ。**「接続復帰」と「Agent 再開」を UI で区別**（Herdr サーバごと死んだ場合に戻るのはレイアウト+native session が中心のため）。
- Herdr 不在時は機能が静かに消えるだけ（任意依存）。ライセンス: **Apache 2.0 の正式リリースを version 固定**（master 直依存にしない）・LICENSE/NOTICE 同梱・checksum 固定。
- 受入（ユーザー承認済みの文言）: **「necoder から Claude Code（CLI 形態）を Herdr Runtime で起動し、necoder を終了しても処理が継続し、再起動後に同一 AgentRun として状態・出力・native session を再取得できる」**。ドッグフーディング効果（`cargo run` 再ビルドでエージェントが死なない）を JOURNAL に実測記録。

### P8 — 読み取り専用 observer + 常駐 shell/テスト

- `TerminalBackend` trait（`subscribe_frames`/`send_input`/`resize`）を `TerminalView` から切り出し、`LocalPtyBackend`（現行）+ `HerdrObserverBackend`（読み取り専用・direct terminal attach の ANSI ストリーム表示）。書き込み制御（takeover/フォーカス所有権）は後段。
- `FleetPane::Tests` を Herdr pane 常駐に。**画面に文字が出たことをテスト成功の正にしない**: コマンド末尾に構造化 sentinel（exit code + marker）を出すラッパーで、結果は necoder が台帳化。
- 受入: dev server / テストが necoder 再起動を跨いで生き、結果が台帳とニュースに載る。

### P9 — リモート管制（QR・スマホから見る/裁く）

**2026-07-25 設計確定**（旧版の「到達性は Tailscale/LAN を明示・構想枠」は破棄）。外出先のスマホのブラウザから、Fleet の状況を見て、許可/拒否と短い指示を返す。**Tailscale/VPN は使わない**（ユーザーに設定させない = この機能の出発点）。

#### 前提（3 つ。ここから構成が決まる）

1. **Mac は NAT の内側**にいて外から直接叩けない。→ 公開された**待ち合わせ場所（リレー）が 1 つ要る**。VPN を使わないと決めた以上、回避不能。
2. **iOS の Web Push はホーム画面に追加した PWA だけが受け取れ、それには固定の HTTPS ドメインが要る**（Service Worker は安全なオリジンを要求し、`http://192.168.x.x` では動かない。起動ごとに URL が変わる quick tunnel 系も購読が壊れる）。→ **ドメインを 1 つ買う**。回避不能。
3. **他人に配る**。→「各自ドメインを買って設定する」は成立しない。**既定のリレーは本人が用意する**。

#### 経路を 3 つに割る（運用が要るのは 1 つだけ）

| | 何を使うか | 運用負荷 |
|---|---|---|
| ① PWA の配信 | ドメイン + **Cloudflare Pages**（静的のみ・**ユーザーデータを 1 バイトも置かない**） | ほぼゼロ |
| ② 通知 | **Web Push** — Mac が Apple/Google の push endpoint へ**直接** POST（**リレーを経由しない**） | ゼロ |
| ③ データ経路 | **Cloudflare Workers + Durable Objects**（リレー） | ここだけ本物 |

①が独立しているのが効く: Service Worker / push 購読 / WebAuthn が要求する「固定 HTTPS オリジン」は**静的サイトで満たせて**、データがどこを通るかとは無関係。

```
      ┌── Cloudflare Pages（無料・静的 PWA・データ無し）
スマホ ┤
      └── wss ──▶ DO（$5/月・交換機・中身は読めない）◀── wss ── Mac
Mac ──直接 POST──▶ Apple Push ──▶ スマホ（リレー不経由・無料）
```

#### 確定した設計判断（変更しない）

1. **リレーは交換機**。room id が一致する接続を 2 本つないでバイト列を流すだけ。判断も記憶もしない。「賢くないこと」が安全性とコストの根拠。
2. **QR がそのまま資格情報**＝**アカウント不要**（サインアップ / OAuth / ユーザーレコードが一切存在しない）。QR は `room_id`（128bit 乱数）+ 鍵（256bit 乱数）を運び、**URL フラグメント（`#`）に載せる**＝サーバにもリレーにも送信されない。QR は光学的な secure channel なので、これで鍵共有は完了（ECDH は不要・将来 re-key が要るときに足す）。
3. **封をするレイヤーは transport 非依存**。LAN / リレーのどれを通っても中身は読めない（AES-GCM か ChaCha20-Poly1305。ブラウザ側は `crypto.subtle` のネイティブ実装）。リレーから見えるのは room id・フレーム長・時刻だけ。
4. **認証の権威は Mac 側**。デバイス表（名前 / 作成 / 最終接続 / スコープ）は `storage` が持ち、失効は Mac から即時。リレーは何も判断しない・できない。
5. **デバイストークンは JWT ではなく不透明ランダム（256bit）+ サーバ側 allowlist**。発行者と検証者が同じ Mac 1 台なので JWT のステートレス検証の利点が効かず、exp / 鍵ローテーション / リプレイの宿題だけが増える。失効が即座なのが決定的。**例外: VAPID は仕様上 ES256 の JWT が必須**（不特定の push サービスに自分を名乗る用途なので、そこはステートレスが正しい）。
6. **3 段圧縮を remote でも守る**（P5 と同じ）。transcript もリポジトリの内容も**構造上この経路を通らない**＝帯域とコストが小さく、リレーの信頼要件も下がる。
7. **スコープ既定**: 読み取り + permission 応答 + prompt 送信まで。`integrate` と削除系はブロック、または WebAuthn 必須。
8. **LAN 高速経路は作らない**。HTTPS ページから `http://192.168.x.x` は mixed content で叩けず、回避には証明書の細工（鍵を配布物に同梱する類）が要る。管制が流すのはテキストで、CF の anycast 経由でも体感しない。**経路は 1 本**にして、遅いと感じてから考える。
9. **リレーは本体と同じ repo の `relay/`・AGPL**（分けない）。`mock/` `lp/` と同じく Rust 以外のディレクトリとして同居。permissive に分ける案は破棄 —— 100 行の交換機は AGPL で弾かれた側が自分で書けるので、copyleft で守るものも permissive で得るものも無く、リポジトリとライセンスと説明が増えるだけ。**§13 の義務は「改変版をサービスとして走らせる人が、その利用者に改変ソースを提供する」だけで、necoder のユーザーにも、necoder で書いたコードにも一切及ばない**。やることは PWA のどこかにソースへのリンクを置く 1 点。
10. **使わないもの**: Tailscale / VPN（出発点）・ngrok / Cloudflare Tunnel（URL 不安定 or 各自ドメインが必要で配布に向かない）・Firebase（Google 依存 + 従量）・Google / OAuth ログイン（QR があれば不要）・WebRTC（TURN が結局リレー）・VPS（DO と基本料が同額で運用だけ増える）。
11. **スマホ面は「管制」だけ。エディタは載せない**。`FEATURES.md` の `never: Web 版` はブラウザ版エディタの不採用を指しており、リモート管制はそれに抵触しない（別物として扱う）。参照元の MulmoTerminal がブラウザにフル端末を載せているのは**元が Web アプリだからそうなっただけ**で、設計判断ではない。**通知が主役で、操作は許可/拒否と短い指示まで**という割り切りを崩さない。

#### P9a — ローカル管制サーバ + QR ペアリング（費用ゼロ・ドメイン不要）

- `necoder serve --control`: **GUI 稼働中でも不在でも動く**（P5 のロック検出 → `gui.sock` IPC fallback をそのまま再利用）。
- 読み取りビュー = **管制タブの写像**（stat チップ / 要対応キュー / 稼働カード / 統合パイプライン / ニュース）。ビジュアルの正は `mock/fleet-dashboard.html` の**レスポンシブ派生**であって、新規デザインではない。
- QR ペアリング: 使い捨てコード（TTL 90s・単回）→ デバイストークン発行 → `storage` のデバイス表に記録。設定画面に**デバイス一覧と取り消し**。
- **`control_ipc` に permission の list / respond を追加**（現状 GUI 内の `control_view.rs` / `agent_panel.rs` にしか無く、**「裁く」半分が IPC に出ていない唯一の穴**）。
- LAN は**平の HTTP**（SW も push も無い普通のタブ）。**P9c のリレーが落ちても家の中では動く保険**として残す。
- 受入: 同じ Wi-Fi のスマホで QR を撮ると管制の読み取りビューが出て、**permission に応答できる**。

#### P9b — 静的 PWA + Web Push（ドメイン代のみ・リレー不要）

- ドメイン 1 つ + Cloudflare Pages。PWA（manifest + Service Worker + ホーム画面追加）。
- Web Push 送信を Rust で: **VAPID（ES256 署名）** + **RFC 8291（aes128gcm）** でペイロードを暗号化し、購読の endpoint へ直接 POST。**Apple にも中身は読めない**＝`never: telemetry` を崩さない。
- **通知ペイロード（約 4KB）に digest を載せる**。「タスク B が完了。全 21 test green」「permission 待ち: workspace.rs への書き込み」が通知として届いた時点で**状況確認が完了する**＝**「知る・読む」がリレー無しで成立**。
- 購読の失効 / 再購読の面倒を見る（iOS は PWA をしばらく開かないと購読を落とす）。
- **WebAuthn（Face ID）を書き込み操作の第 2 要素**に（固定オリジンがあって初めて成立）。
- 検証: **デスクトップ Chrome は本物の Web Push を受け取れる**ので送信経路は実証可能。`crypto.subtle` / SW / WebSocket は Safari と同じ API なので、封の往復・ペアリング・再接続もヘッドレス Chrome で通せる。**iOS 固有（ホーム画面追加が必要・Face ID）だけが実機**。
- 受入: 外出先で完了 / 承認待ちが通知として届き、**通知だけで何が起きたか読める**。

#### P9c — リレー（月 $5・「返す」経路）

- `relay/` に Cloudflare Worker + Durable Object。**Hibernation API 準拠が必須**（下記）。
- **DO は何も永続化しない**（部屋に 2 本ぶら下がっているという事実だけ）。ゼロ知識の要件と hibernation の条件が一致する。
- `idFromName(room_id)` で決定的に同じ部屋へ。**QR はペアリング 1 回きり**で、room id と鍵を両側（Mac は `storage`・スマホは localStorage）に保存し、再接続は同じ room id で繋ぎ直す。撮り直すのはデバイス失効時とスマホ初期化時だけ。
- **再接続処理は必須**: hibernate は切断ではない（接続は CF 網に維持され、メッセージで再初期化される）が、**Worker のデプロイは全 WebSocket を切断する**し、電波 / スリープ / Wi-Fi 切替でも切れる。
- 初日に入れる: room ごと・IP ごとのレート制限 / フレームサイズ上限 / room TTL / **Cloudflare の請求アラート**。払うのは本人。
- `relay_url` の差し替えで self-host（`wrangler deploy` 1 回。アカウントが無いので移行データも無い）。
- 受入: モバイル回線から permission に応答でき、Mac 再起動と Worker デプロイを跨いで自動再接続する。

#### 費用（2026-07-25 時点の単価。着手前に現行 pricing を再確認）

Workers Paid = **$5/月**（Requests 100万/月込み・超過 $0.15/100万、Duration 400,000 GB-s/月込み・超過 $12.50/100万 GB-s）。**WebSocket の受信メッセージは 20:1 で計上**される（100 通 = 5 リクエスト）＝この用途を狙った割引。

重めのユーザーを 1 日 2,000 メッセージと置くと 1 人あたり **3,000 requests/月** → 込み枠で**約 330 人**、**1,000 人で約 $5.3 / 10,000 人で約 $9**。固定費は**ドメイン年 2,000 円 + 月 $5** で、ユーザー数でほぼ動かない。

**唯一の落とし穴 = Hibernation を落とすと桁が変わる**。素朴に書くと DO が常駐して接続中ずっと duration が課金され、**100 ユーザー常時接続で月 $5 → 約 $400**。実装規律: `ws.accept()` ではなく **`state.acceptWebSocket()`**、ハンドラはクロージャを持つイベントリスナではなく **`webSocketMessage()` / `webSocketClose()` メソッド**。**クロージャに状態を持たせた瞬間に hibernate できなくなる**＝「リレーが何も覚えない交換機であること」がそのまま課金条件になっている。

#### 実機が要る部分（人の手番・M9 受入と同じ扱い）

iPhone での QR → ホーム画面追加 → **通知の実受信** / **Face ID（WebAuthn）が PWA 内で動くこと** / モバイル回線での実挙動 / ドメイン購入・CF デプロイ・請求アラート設定。**iOS の PWA と Web Push はこの計画で最も読めない領域**（iOS バージョンで挙動が変わる）。**初回の実機テストで修正が 1 周発生する前提**で見積もる。

#### 順序の申し送り

P9 は計画上 **P7（常駐）・P8 を飛び越す**（2026-07-25・ユーザー判断）。P9a/b は P7 に依存しないので技術的な問題は無い。ただし「外から見た先でエージェントが生きている」価値を支えるのは P7 なので、P9c まで済んだら P7 に戻る。

## 3. 実装エージェントへの注意

- `CLAUDE.md` と `zed/CLAUDE.md`（GPUI 節・Rust 規約）が正。GPUI API はネット記事や記憶を疑い `zed/crates/gpui/examples/` を見る。
- mock を先に直してから実装（乖離したら文書/mock を直す方が先）。`mock/fleet-dashboard.html` を index.html へ移植する際は編隊モード節へ統合し、検証はヘッドレス Chrome スクショ + Read 目視。
- 文書更新も作業のうち: GLOSSARY（管制/Control・遷移スナップショット/digest・監督/Coordinator・常駐/Herdr Runtime・集約気分）、UI-SPEC §11（管制タブ・✳ と破線の意味論・stat チップ）、DECISIONS（本計画 §0 の 1〜9 を決定ログとして転記）、ROADMAP M14（各フェーズ完了時にチェック + JOURNAL 記録）。
- 性能予算: 管制タブは 8 TaskSpace で 60fps・digest 生成はレンダ外（Git/DB/Host I/O を render 中に行わない不変条件）。

## 4. 受入の全体像（M14 総合受入との対応）

- P1+P2+P3 で「N 体並走を色相を足さずに 1 画面で捌く」が完成。
- P5+P6 で「"B の完了を待って merge" を自走」が完成（＝M14 総合受入）。
- P7 で常駐（このプロジェクトのドッグフーディング速度に直結する最重要機能）。
- P9 で「外出先から見る・裁く」。**P9a（LAN・費用ゼロ）→ P9b（PWA + 通知・ドメイン代のみ・ここまでで「知る/読む」が完成）→ P9c（リレー・月 $5・「返す」が完成）** の 3 段で、各段が単独で価値を持つ。P7/P8 より先に着手する。
