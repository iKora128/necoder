# 設計判断の詳細（根拠つき）

README のPart 2を、判断の理由まで含めて展開したもの。後で「なんでこう決めたっけ」を
思い出すための記録。

---

## 1. なぜ「fork」でも「クリーンルーム全部」でもなく「permissiveな土台に乗る」か

3択ではなく、実質こう:

| ルート | 中身 | 問題 |
|---|---|---|
| Zedアプリ(GPL)をfork | 全部もらえる | GPL縛り＋**永遠のリベース地獄**（Zedは高速で進む） |
| 全部クリーンルーム | ライセンス自由 | **person-decades**。最難ルート |
| **permissiveな部品に乗って新規** | GPUI(Apache)/Tree-sitter(MIT)/LSP/rope/terminal を再利用し、自分のUXを上に載せる | **これが現実解**。人年が現実的レンジに落ちる |

- **クリーンルーム = GPLコードを一切見ずに再実装**すればGPL非適用（ライセンス自由）。
  だが「見ない」縛りは効率が悪い。
- **GPUIはApache-2.0**なので、"forkせずGPUIだけ借りて新規アプリ"が可能で、しかもGPL非強制。
  これが「自分のZed的エディタ」の最短。

## 2. GPUI vs Tauri —— これは"思想の分岐"

| | GPUI | Tauri |
|---|---|---|
| UI | Rustネイティブ・GPU描画 | **Webview**（HTML/CSS/JS）＋Rust裏 |
| 速さ | Zed級 | 軽Electron。編集コアはwebview性能に縛られる |
| UI拡張性 | **難しい**（Zedと同じ悩みを再発明） | **簡単**（Web技術だから） |
| 実態 | "Zed側"（速さ） | "軽いVSCode側"（拡張の楽さ） |

- **Tauri = webview = Zedが速さのために捨てたモデル**。UI拡張は楽だが"Zedの速さ"は諦める側。
- **GPUIは速いがUI拡張は自分で #53403 問題を再発明**する羽目に。
- → **「速さが魂」ならGPUI**。今回はこちらに傾倒。ただし下記の代償を受け入れる前提。

## 3. GPUIの「Zedのために作られてる」とは具体的に何か

技術的には汎用GUIフレームワーク。だが:

1. **in-tree開発**（`crates/gpui`）。機能もAPI変更も**Zedの都合・タイミング**で入る。
2. **安定版リリースが無い**。crates.ioでsemver保証、ではなく**gitから直接**。
   → **APIが予告なく変わる**（＝"API churn"）。互換維持の約束は外部利用者に無い。あなたのコードが壊れうる。
3. **docsが薄い**。一番の利用者がZed自身のコード → "自分でアプリ作る人向け"の入門が貧弱。
   **Zedソースを読んで学ぶ**のが実態。
4. **非Zedの事例・コミュニティが少ない**。詰まっても情報が乏しい。自力＋ソース頼み。

**たとえ**: 会社が看板製品用の内製ツールをOSS公開した状態。モノは本物に良いが
"あなたのため"には整備されていない。

**対比**: egui / iced / Tauri / Dioxus は**アプリ開発者のための製品**（docs・安定版・チュートリアル・
コミュニティ完備）。GPUIは**Zedのエンジンを共有してもらってる**位置づけ。

**代替**: **Floem**（Lapceが使用）は"エディタから生まれたが、より汎用・standalone向けに整備"。
Lapceが速いエディタをFloemで作れている＝**GPUI以外でも同じ土俵に立てる**証拠。
→ GPUIの早期採用コストが重すぎると感じたら Floem/iced に逃げる道がある。

## 4. マルチプラットフォーム（GPUI）

- 対応: **macOS / Linux / Windows**（デスクトップ3種）。Zed自身が3つで配布＝実証済み。
  クロスプラットフォームは **Blade**（Vulkan/Metal抽象化）が担う。
- **非対応: Web(WASM) / モバイル(iOS/Android)**。デスクトップ専用。
- 成熟度: **mac（最成熟, Metal）> linux（安定, Wayland/X11）> windows（最後発・一番荒い）**。
- 配布は結局OSごとの儀式（mac署名/notarize等）。フレームワークが変わっても消えない
  （※Tauri desktopアプリで体験済みのはず）。

## 5. ライセンス（GPL？）

- **GPUI(Apache)はGPLを強制しない** → ライセンスは**あなたの選択**。
  クリーンルーム＋GPUIなら好きに選べる。GPLv3はApache-2.0を取り込める（Zedがまさにこれ）。
- トレードオフ:
  - **GPL** → copyleft。**他者（大企業含む）が成果をクローズド化して出し抜くのを防ぐ**。Zedの選択理由。
  - **Apache/MIT** → 採用・貢献が増えやすい＋将来の商用/デュアルライセンスの自由。
- 実例の割れ: **Zed=GPL / Lapce=Apache / Helix=MPL / VSCode(Code-OSS)=MIT / Neovim=Apache**。
  → 正解は一つではない。
- **推し**: 「クローズドforkから守りたい」ならGPL（Zedと同型で自然）。
  「最大限広めたい/商用余地残す」なら土台に合わせてApache-2.0。
- **重要**: ライセンスは"作れた後"に効く話。**今の最重要事項ではない**。ソロ初動はどちらでも実害ゼロ。
  ただし外部貢献を受け始めると再ライセンスが困難 → **最初に決めて、あとは忘れる**。
- **→ 決定（2026-07-11）: GPL-3.0**。Zed の GPL crate（`acp_thread` / `agent_servers` / `worktree` / `fs` 等）を
  移植・改変して土台にする戦略とセットの判断（Apache の GPUI / `agent-client-protocol` とは共存可）。
- **→ 改定（2026-07-12）: AGPL-3.0-or-later で "park"**。**AGPL-3.0 は GPL-3.0 と相互互換**（両 §13）。
- **→ 戦略確定（2026-07-12・本人）: 最終的に Apache-2.0（普及第一）**。堀は**ブランド/実行力/デザインの趣味/`shirushi.ai` の運営**でありライセンス保護ではない（色×横断×AI-native はライセンスでは守れない）。**重要**: 現コードは**100% GPL フリー**（gpui/gpui_platform=Apache・acp は crates.io の agent-client-protocol=permissive・worktree/editor 全自作。Zed の GPL crate は"移植"していない）。→ **著作権者=本人 + GPL コード無し + CLA を保つ限り、いつでも Apache へ緩められる**。**今 AGPL で park＝保護しつつ全選択肢を残す最善形**。**Apache の道を閉じない鉄則: Zed の GPL crate を移植せず自作/permissive で代替・貢献は CLA**（当初の「GPL 決定=Zed GPL crate 移植前提」は移植せず自作で組んだため消滅）。「採用を最大化するモード」に入る時に Apache へ緩める。
- **→ 確定（2026-07-15・本人）: AGPL-3.0 を正式ライセンスとして確定**（park → 確定。「最終的に Apache へ」の前提は撤回）。理由: ①コミッタが本人のみ＝Apache の主利得（企業採用・貢献の集めやすさ）が現時点で効かない ②Apache でクローズドにコピーされることへの心理的抵抗 — ソロ多年プロジェクトは「パクられても平気か」という持続性が実利に優先する（Zed が GPL を選んだ理由と同型）③Remote SSH まで実装済み＝将来のクラウド/リモート展開にネットワーク条項（AGPL §13）が効くため GPL より AGPL が適合。07-12 の「堀はブランド/実行力」論は真のまま両立 — ライセンスは堀ではなく**続けるための保険**。**維持する鉄則（目的を「Apache の道」から「再ライセンスの自由」に読み替えて存続）**: Zed の GPL crate はコード移植せず自作/permissive で代替・外部貢献を受け始める最初の 1 件から CLA 必須。この 2 つを守る限り、デュアルライセンス/商用版/将来の緩和は本人の一存で可能なまま（AGPL→緩める方向は一方通行でいつでも撃てる。逆に一度 Apache で出た snapshot は永久に回収不能）。

## 6. 動機の切り分け（判断の本質）

- **不満駆動**（スレッド色が欲しい）→ 自作は桁違いに不釣り合い。VSCodeに戻る or Zedに要望が正解。
- **ビジョン駆動**（理想のエディタを建てたい）→ 正当な多年プロジェクト。Zed自身がそれで始まった。
- **見極め方**: [`MVP-PLAN.md`](./MVP-PLAN.md) の"週末テスト"。GPUIでMVPを書いて
  **楽しくて止まらない→続行 / 苦行→答え**。

## 7. 「外圧としてのfork」戦略について（結論：やり方次第）

- 良い圧力＝**動くPoC＋実需要(👍)＋マージできる綺麗なPR**を**協調的に**出す。forkは"証拠品"。
- 悪い圧力＝rival product的な「これ入れろ」。**意見の強いZedチームには逆効果**
  （PR #58755 でメンテナがLLM生成コミュニケーションに不快感→空気悪化、を実際に目撃）。
- 特に**UI拡張API(#53403)はメンテナが「チーム主導でやる」と明言** →
  そこはforkしてPR圧をかけても "ありがとう、でも自分たちで" になる領域。**需要を積んで待つ**が正着。
- コメント/要望は**自分の言葉で短く**（AI生成の長文べた貼りは同じ地雷）。

## 8. 決定ログ（2026-07-11 追記）

- **名前: Shirushi（しるし・印）** — 暫定確定。ドメイン `shirushi.ai` 取得済み。
  crate/バイナリ名 `shirushi`、プロジェクトローカル設定は `.shirushi/`
- **ライセンス: AGPL-3.0（2026-07-15 確定）**（§5 参照。鉄則: Zed GPL crate のコード移植禁止・外部貢献は CLA — 再ライセンスの自由を保つため）
- **性能予算: Zed 比 ~80% を下限目標**（入力レイテンシ・起動）。UX 向上を性能より優先する
  — GPUI 採用自体が「UX の一環」という位置づけ。ベンチは zed の editor_benchmarks / input_latency_ui を移植
- **重い Phase 管理はしない**: M0〜M4 の軽量マイルストーン + FEATURES.md のタグで運用（CLAUDE.md 参照）
- **ビルド前提の罠**: macOS 26 / Xcode 26 は Metal Toolchain が別コンポーネント
  → `xcodebuild -downloadComponent MetalToolchain` が必要（GPUI ビルド全般の前提）
- **ウィンドウモデル（改訂 2026-07-19・M10-2）: 1窓 = 複数 project×branch を持つレール。既定はレール内で開く**
  — レール＝窓内の切替器（複数スロット）。**ブランチ/worktree を開くと既定で新スロットとしてレールに載る**
  （旧: 1窓=1worktree で新窓に開く。新窓が散らばると色による方向感覚が窓境界で途切れるため転換）。
  **新窓は「レール項目を右クリック→新しいウィンドウで開く」の明示操作**に格下げ（⌘⇧N も新窓のまま）。
  レール右クリック＝コンテキストメニュー（色スウォッチ＋新窓／レールから外す／worktree・ブランチ削除。
  破壊的操作は二段確認）。旧「右クリック＝色ピッカー」はメニュー最上段のスウォッチ＋「その他の色…」へ吸収。
  「削除」の3階層を分離: **レールから外す=表示のみ（安全）／worktree を削除=`git worktree remove`／
  ブランチを削除=worktree ごと `git branch -D`**。worktree タブ（`worktree_branch=Some`）だけ後2者を出す。
  同一リポジトリの別ブランチは identity 色が親と衝突しがち → 未使用パレット色に倒して同色2枚を避ける。
  状態は (project, branch) 単位で保存復元。ARCHITECTURE §5）
- **i18n: 初日から内蔵**（rust-i18n・`t!` 規律・ja/en 同梱・追加言語は YAML 1枚＝言語パック。
  根拠: 後付けは地獄 — Zed は i18n 無しで後発が入れられない実例、VSCode は language pack 方式。ARCHITECTURE §6）
- **ドキュメント駆動開発へ移行**: ROADMAP（受入条件）/ ARCHITECTURE（設計図）/ UI-SPEC（見た目仕様）/
  JOURNAL（日誌）の4枚 + mock を正とし、`/goal` コマンドが未チェックの受入条件を上から自走で消化する
- **ローカル永続化 DB: Turso 採用（2026-07-16・本人指定）** — SQLite の pure-Rust 再実装（MIT・async ネイティブ）。
  用途は hot exit / スレッド永続化 / トークン台帳 / checkpoint メタに**限定**（設定・todos.md・keymap は
  「ファイルが真実」のまま。検索索引は持たない＝ regex 走査が正）。薄い `storage` crate に隔離し、
  成熟度の問題が出たら rusqlite へ 1 crate 差し替えで退避できる面を保つ（ARCHITECTURE §7）
- **自動アップデート: 自前で確定（2026-07-16・本人）** — GitHub Releases + 署名検証。velq / karui と同型の
  確立パターンがあるため Sparkle は不採用（M13 で実装）
- **checkpoint の方式: content-addressed ブロブストア自作で確定（2026-07-17・比較表提示済み）** —
  (a) content-addressed（採用）: Turso に checkpoint→file→hash のメタ・blob は
  `~/Library/Application Support/Shirushi/blobs/<hh>/<hash>` のファイル・重複排除が自然・GC は SQL で列挙。
  (b) ターン毎全コピー: 実装最小だが turn×ファイルで線形膨張（エージェントは同じファイルを何度も触る = 最悪ケース）。
  (c) shadow git: delta 圧縮は魅力だが git 依存・ユーザー .git との干渉・turn 毎 commit コスト —
  research/cursor-features.md の「Git 非依存の信頼担保」と不整合。
- **「UI スレッドで Host を呼ばない」を規律化（2026-07-16 監査）** — Host は同期 trait で remote は
  1 呼び出し最大 30s ブロックしうる。詳細と確立パターンは ARCHITECTURE §9、監査結果は JOURNAL 2026-07-16
- **「自分で決めた既定はドリフトしない」を UX 原則として確定（2026-07-19・本人）** — ユーザーが明示的に
  設定した既定（既定エージェント・既定モデル・effort 等）は、**意図的に変更しない限り変わらない**。
  last-used 記憶で新規スレッドが「前回選んだもの」に引きずられる方式（Claude Code 拡張・Zed もこれ）は**不採用**。
  根拠: 1 回の気まぐれ選択が恒久既定を汚す不快さ（例: 一度 Fable／高コストモデルを選ぶと以後の新スレが
  それに固定され、無意識にトークンを溶かす）を UX の芯として避ける＝「色による方向感覚＝迷わない」と同系の判断。
  **実装規律: per-thread のピル操作はそのスレッドだけに効き、グローバル既定 (`default_agent` 等) を書き換えない。
  既定の変更は Settings 画面（明示操作）でのみ**。監査（2026-07-19）で唯一の違反＝Agent ピルの `default_agent`
  巻き添え保存を除去済み（`agent_panel::select_option`）。モデル既定も「Settings で各エージェントに ★ で決める」
  方式に寄せる（当面は agent 自身の既定＝安定なのでドリフトは無い）。設定画面は M12・`Workspace.show_settings`。
- **M14 マルチエージェント編隊 = herdr から取るのは #3-4 のみ（2026-07-20・本人）** — herdr（AGPL・クリーンルーム参照）分析の結論: 状態検知・状態一覧・会話復元は ACP ネイティブで既にカバー/上回る（`ThreadActivity`/`activity_dot`/状態つき `RunningRegistry`・スレッド永続化。基盤は 2026-07-20 別セッションで実装済み）。**取り込むのは「エージェント自身が編隊を操作する API」（herdr の #3-4）のみ** — `shirushi` CLI ＋ ACP ツール/MCP で worktree 追加・ペイン追加・`wait agent-status`（他エージェントの完了待ち）を**クリーンルーム自作**（ソースを読まず挙動から・§5 の鉄則どおり）。**プラグイン市場（#5）は不採用**（登録式境界は既存・エコシステムは later）。トースト個別ミュート（#7）は既存通知の小改善に合流。**編隊ビュー = 専用だが軽量**（既存 `ProjectSession` を再利用し、左ファイルブラウザは残して **focus-follows-agent**＝フォーカス中エージェントの worktree を映す＝「ファイルブラウザの真」を一意化）。**系譜グラフ = 扇形（Fan）既定**（＋ツリー/カード/ハブ切替・ネイティブ描画・webview 不使用。River は廃止・Hub 追加＝2026-07-22）。**状態は色相を持たない**（既決・§1.3/JOURNAL 2026-07-20）を編隊でも貫く。M13 末尾「エージェントタブ Grouped ビュー」は**②スレッドが worktree dest を持つモデル拡張**で決着。正は UI-SPEC §11・ROADMAP M14・mock 編隊モード。
- **M14 Fleet を TaskSpace-first へ確定（2026-07-22・前項を更新）** — 既定は `+ Task` = branch + linked worktree + ProjectSession + AgentPanel。Fleet cell は Agent thread でなく TaskSpace を表し、既存 AgentPanel を完全再利用する。同じ Task への Agent 追加だけを明示操作にする。main は protected IntegrationSpace、永続 lifecycle/event ledger と Conflict Radar / explicit Integrate を Orchestration Engine が所有する。CLI/MCP の create/list/status/wait/review/integrate も同じ ledger を使う。正は `FLEET-ARCHITECTURE.md`。
- **管制（Control）の設計判断 9 点を確定（2026-07-23・本人承認。実装計画=`FLEET-CONTROL-PLAN.md`・その §0 の転記）** —
  ① 監視役は 3 分解: 決定論の事実層（プロトコル/Git 由来・LLM なし）/ 任命制の監督（ただの ACP スレッド）/ 人間への注意ルーティング（UI）。**台帳が記憶・監督は状態を持たない**（交代・再起動が自由）。
  ② 名称「管制」（英 `Control`）。編隊モード中央面の新タブ。
  ③ 規律: 色=スレッド識別のみ / 状態=形と動き / ✳ テラコッタ=LLM 生成テキストの印 / 破線=画面推定（Herdr 由来）。`TaskPhase`・`ThreadActivity`・Git health を 1 enum に潰さない。
  ④ 遷移スナップショット（digest）は状態遷移時のみ生成・ポーリング無し。Tier1=決定論テンプレ合成 / Tier2=`oneshot()` 1 行要約（Done/Failed のみ・ledger キャッシュ）。**要約は状態を上書きしない**。
  ⑤ Herdr は sidecar（別プロセス・socket API）。頭脳は Shirushi・PTY とプロセス生存だけ Herdr。worktree 操作は Shirushi 所有。
  ⑥ ACP 優先の起動形態選択: ACP 対応エージェントの既定は ACP。常駐（Herdr・CLI 形態）は明示選択時のみ・推定監視であることを破線で明示。
  ⑦ Herdr の観測値は業務上の正にしない: `TurnId`/`generation` で照合し前ターン完了の誤認を防ぐ。
  ⑧ 守るべき操作（spawn/遷移/integrate/send）は Shirushi の MCP/CLI にだけ置く（Herdr socket 直叩きは台帳と permission の迂回路＝監督のツールセットに含めない）。
  ⑨ リモート管制（QR・スマホ）は最後（P9・構想枠）。
  **P0 実装補足（2026-07-23）**: phase 遷移の唯一の入口は `Storage::commit_task_transition`（snapshot+event 同一 transaction）。`TaskPhase`/`SpaceKind` の単一定義は storage crate（GUI/CLI/MCP が共有・文字列は DB とプロセス境界のみ）。IntegrationSpace は phase でなく `SpaceKind`（DB は既存互換の sentinel 表現・storage 内に封じる）。
- **Task ledger は「GUI 稼働中 = 単一 writer」で確定（2026-07-24・P5 実装中の発見）** — Turso は**プロセス排他ロック**で、GUI と headless CLI/MCP が同じ DB を同時に開けない（`Locking error` を e2e で確認）。対応: GUI が生きている間の台帳読み書きは**すべて GUI の storage ハンドル（単一ワーカースレッド）へ IPC（`~/.shirushi/gui.sock`・0600・1 行 JSON）で集約**し、headless 直開きはロック検出（`is_lock_error`）で自動フォールバック。GUI 不在時は従来どおり直接 DB。副次効果: 遷移が常に GUI の入口（`transition_task_space`）を通るためニュース/総括/管制が headless 操作でも即時に追従する。守るべき操作（spawn/send/遷移）を Shirushi の CLI/MCP にだけ置く原則（FLEET-CONTROL-PLAN §0-8）とも整合。rusqlite へ退避する場合もこの単一 writer 構造は維持する（多 writer に戻さない）。
- **herdr は Apache 2.0 化 → クリーンルーム制約を解除（2026-07-24・本人確認）** — 従来の「AGPL・クリーンルーム参照（挙動のみ・ソース不可視）」は失効。`herdr/`（.gitignore 済み・zed/ と同じ参照クローン方式）でソース参照可・必要なら手法の移植も可（Apache 2.0 の帰属表記を守る）。M14 の「herdr から取るのは #3-4 のみ」の採否判断自体は変えない（取り込み済み概念: 状態一覧 / done・idle ラッチ / 編隊操作 API）。
