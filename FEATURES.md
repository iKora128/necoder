# FEATURES — 生きた機能バックログ

タグ: **MVP**（日常使用が始められる最小線）/ **v1**（現代エディタの parity）/ **later** / **never**（当面やらない）。
根拠と各機能の詳細は [`docs/research/feature-matrix.md`](docs/research/feature-matrix.md) と research 3本を参照。
このファイルは仕様書ではなくバックログ — 着手時に上から書き換えていく。

進め方は [`docs/MVP-PLAN.md`](docs/MVP-PLAN.md)（週末テスト → Step 1〜8）。

---

## 1. コア編集

- [ ] MVP: rope バッファ（`ropey`）。CRDT は焼き込まない（コラボ never とセットの判断）
- [ ] MVP: 位置/範囲/選択モデル（全 API の土台。最初に型を固める）
- [ ] MVP: カーソル編集（挿入/削除/改行）、行番号表示
- [ ] MVP: undo/redo（トランザクション単位のグルーピング）
- [ ] MVP: ファイル開く/保存、外部変更の検知
- [ ] MVP: IME（日本語入力。GPUI の input handler。**初日から検証する** — 後付けが一番危険）
- [ ] MVP: クリップボード
- [ ] v1: multi-cursor（上下追加・全一致選択）、矩形選択
- [ ] v1: 自動インデント、括弧自動クローズ、コメントトグル
- [ ] v1: soft wrap、保存時処理（末尾空白除去・最終改行）、自動保存
- [ ] later: スニペット、折りたたみ、multibuffer（Zed の中心発明 — pane 抽象だけ multibuffer 前提で設計しておく）
- [ ] later: vim モード
- [ ] never: Helix モード、modeline、ファイル横断 undo

## 2. 描画・UI

- [x] 原則（2026-07-11 決定・mock v0.2 反映）: **色は識別に集約**（レール/タブ下線/キャレット/スレッド色）。グラデ額縁・選択面などの装飾には使わない
- [x] 方針（2026-07-11 決定）: **性能予算 = Zed 比 ~80% を下限目標**（入力レイテンシ・起動）。UX 優先。necoder 固有の計測境界で自動ベンチを導入し、Zed は同条件で外部計測する
- [x] 決定（2026-07-11）: **ウィンドウモデル = レールで窓内切替、⌘⏎／右クリックで新窓**（1窓 = アクティブな project×branch。詳細 docs/ARCHITECTURE.md §5）
- [ ] MVP: GPUI ウィンドウ + 行の**仮想化描画**（可視行のみ。3エディタ共通の中核）
- [ ] MVP: スクロール、単一ペイン、タブバー
- [ ] MVP: **プロジェクト色（titlebar 額縁 + アクティブタブ + statusbar）** ← 差別化の種。mock 反映済み
- [ ] MVP: テーマ（dark/light 各1。トークンは mock の CSS 変数 `--bg0..3/--fg0..2/--syn-*` をそのまま構造体に）
- [ ] v1: **テーマのきせかえ**（2026-07-11 決定）: テーマセレクタ（ライブプレビュー付き・Zed方式）+ **ユーザー定義テーマ = トークン上書きの JSON 1枚**（`themes/*.json`）。プロジェクト色（Peacock相当）とは独立に共存
- [ ] later: 公開テーマ形式を解析する VSCode / Zed テーマのインポート（独立実装）、テーマの拡張配布
- [ ] MVP: ステータスバー（各機能がアイテム登録する方式 — 最初から登録式に）
- [ ] v1: ペイン分割、3方向ドック、デコレーション基盤（検索/diff/診断ハイライトの共通土台）
- [ ] v1: 通知/トースト、コンテキストメニュー、ツールバー+パンくず
- [ ] later: ミニマップ、sticky scroll、インデントガイド、Markdown/画像プレビュー、Zen モード
- [ ] never: 印刷、スプラッシュ演出

## 3. ナビゲーション

- [ ] MVP: ファイルツリー（開く/作成/リネーム/削除）
- [ ] v1: **エクスプローラ3表示（ツリー / カラム / アイコン、左ドック下部で切替）** — ビュー層は自作、モデル層（走査/監視/gitignore/git状態）は Zed の `worktree`/`fs` を再利用、仮想化は GPUI `uniform_list`（2026-07-11 決定・mock 検証済み）
- [ ] v1: **ファイルブラウザ操作**（戻る/進む/上の階層ナビ、右クリックメニュー: ファイル=開く・分割して開く / フォルダ=**新しいウィンドウでプロジェクトとして開く** / パスコピー / リネーム / ゴミ箱）— mock v0.2 で挙動確認済み
- [ ] later: ディレクトリを一級のビュー（タブ）として編集領域に開く（Finder / dired 方向）
- [ ] MVP: 行ジャンプ（`:42`）
- [ ] v1: **picker 共通基盤**（Zed 方式: 1つの fuzzy picker を file finder / コマンドパレット / テーマ選択で使い回す）
- [ ] v1: fuzzy ファイル検索（⌘P）、コマンドパレット（⌘⇧P）
- [ ] v1: ナビゲーション履歴（戻る/進む）
- [ ] later: アウトライン（tree-sitter クエリ駆動 — LSP 不要で動く）、タブスイッチャー、workspace symbols
- [ ] never: ブックマーク（Zed も持っていない）

## 4. 言語知能

- [ ] MVP: tree-sitter ハイライト（**Rust 1言語**から。増分パース）
- [ ] MVP: 言語機能を最初から**プロバイダ型**に（editor が言語実装を持たない — VSCode/Zed 共通の設計）
- [ ] v1: LSP クライアント（rust-analyzer: 補完/診断/ホバー/定義ジャンプ/フォーマット)
- [ ] v1: tree-sitter インデント/括弧クエリ
- [ ] later: リネーム、code actions、inlay hints、参照検索、多言語（言語追加は拡張の主用途にする）
- [ ] later: セマンティックトークン、シグネチャヘルプ
- [ ] never: 言語の ML 自動判定

## 5. 検索・置換

- [ ] MVP: バッファ内検索（インクリメンタル、大小文字、regex）
- [ ] v1: バッファ内置換
- [ ] v1: プロジェクト横断検索（**ripgrep 子プロセス** — VSCode 方式が最安で最強）
- [ ] later: 横断置換（プレビュー付き）、検索結果の multibuffer 化
- [ ] never: 検索エディタ、構造検索

## 6. VCS / Git

- [ ] v1: git status（ツリー/タブの状態色 — プロジェクト色と干渉しない設計に注意）
- [ ] v1: ガター diff（hunk マーク）
- [ ] later: diff ビュー、ブランチ切替、コミット UI、blame、hunk 単位 stage
- [ ] never: graph/stash UI、ホスティング連携、Git 以外の SCM

## 7. ターミナル

- [ ] v1: 統合ターミナル（`alacritty_terminal` — Zed と同じ crate が単体で使える）
- [ ] v1: パスのリンク化（クリックで file:line へ — problem matcher の代替。Zed 方式）
- [ ] later: 分割/複数タブ、タスク実行（tasks.json）、シェル統合（コマンド境界検出）
- [ ] never: type ahead、自動応答

## 8. デバッグ

- [ ] later: DAP 一式（3エディタとも「無くても成立」扱いが明確 — 丸ごと後回しで正しい）

## 9. 拡張モデル ← 差別化の本丸

- [x] ローカル HTML プレビュー（ソース⇄表示・⌘⇧V）。アプリ UI には使わず、macOS=WKWebView / Windows=WebView2 を遅延生成。Chromium/ブラウザエンジンは同梱しない
- [ ] v1: **登録境界を最初に切る**（コアは機能を知らない。コマンド/キーマップ/テーマ/ステータスバー項目/パネルを登録式に — 本体機能自身が最初の「拡張」として実装される状態を作る）
- [ ] v1: 設計 ADR: 宣言的 UI 拡張の形式（webview なし・プロセス分離なしで、ネイティブ部品の宣言的合成をどこまで許すか。VSCode の contribution points 53種のうち views/statusBarItems/menus/commands/themes が宣言型で実証済み — このリストが実装カタログ）
- [ ] later: WASM ホスト（Zed 方式: wasmtime + capability 宣言制）で資産拡張（言語/テーマ）から開始
- [ ] later: 宣言的 UI 拡張の実装、拡張の配布方法
- [ ] never: 拡張/API向けの汎用 webview（HTMLプレビューの隔離用途は除く）、Marketplace 事業

## 10. 設定・キーマップ

- [ ] MVP: JSON 設定（default → user → project の3段マージ、コメント付き JSON、保存即反映）
- [ ] v1: **プロジェクトローカル設定ディレクトリ**（`.necoder/settings.json` — `.vscode`/`.claude` 相当）: プロジェクト色・**レール用カスタムアイコン（画像/絵文字）**・プロジェクト固有設定（2026-07-11 決定）
- [ ] MVP: JSON キーマップ + コンテキスト述語（`Editor && mode == full` 形式 — VSCode の when 句 / Zed のコンテキスト式と同型）
- [ ] v1: JSON スキーマ自動生成（設定ファイルに補完が効く — Zed 方式）
- [ ] later: GUI 設定エディタ、ベースキーマッププリセット（VSCode/Zed 互換）、設定プロファイル
- [ ] never: Settings Sync、企業ポリシー

## 11. コラボレーション

- [ ] never: リアルタイム共同編集・通話・チャンネル（「本体に無くても一流」を VSCode/Cursor が証明済み。CRDT 非搭載の判断とセット）

## 12. AI / エージェント ← そもそもの出発点

- [ ] v1: **ACP クライアント**（Agent Client Protocol — 自前エージェントを作らず Claude Code / Codex / Gemini CLI を接続。Zed が実証済みの道。**Claude Code はアダプタが CLI を子プロセスで包む形＝既存サブスクのログインがそのまま使え、API キー課金不要**。キーポイントと確認済み 2026-07-11）
  - 土台（ソース確認済み 2026-07-11）: プロトコルは crates.io の **`agent-client-protocol`**（Apache-2.0、v1.2 系、agentclientprotocol/rust-sdk — Zed 自身もこれを外部依存で使用）。Claude 側は npm **`@agentclientprotocol/claude-agent-acp`** を子プロセス起動するだけ。**一から書くのはプロトコルでもアダプタでもなく「UI とプロセス管理」だけ**
  - Zed in-tree の `acp_thread` / `agent_servers` は GPL-3.0-or-later のためコードを取り込まない。crates.io の `agent-client-protocol` と公開 ACP 仕様を使い、プロセス管理と UI は necoder の要件から独立実装する
- [ ] v1: チャット UI の見た目は **VSCode Claude Code 拡張を踏襲**（⏺/⎿ トランスクリプト、✳ Thinking、Todos）。スレッド色は入力枠・送信ボタン・宛先チップまで貫通（mock 検証済み）
- [ ] v1: **スレッド = 色付きタブ**（titlebar ビーコン + statusbar ドット連動。mock 反映済み）
- [ ] v1: **トークン使用量の常時表示**（Zed+ACP で不可視だった痛点。クライアント UI の責任範囲として設計）
- [ ] v1: thinking 常時展開表示、Enter=改行/⌘Enter=送信（IME 対策）
- [ ] v1: エージェント編集の diff レビュー（accept/reject）
- [ ] later: チェックポイント/巻き戻し（Cursor から盗む概念 — Git 非依存の信頼担保）、@-mention（file/folder/diff/terminal）、AGENTS.md 読み込み（事実上の業界標準・対応コスト低）、MCP
- [ ] later: インラインアシスタント（⌘K 相当）
- [ ] never: edit prediction（Cursor は専用モデル+オンライン RL で成立 — 個人では土俵に乗れない）、自前モデル、Cloud Agents / Bugbot / Automations 相当

## 13. その他

- [ ] v1: **i18n を初日から内蔵**（rust-i18n、`t!` 規律、**ja/en 同梱**。追加言語 = locales/xx.yml 1枚 = 言語パック。2026-07-11 決定、docs/ARCHITECTURE.md §6）
- [ ] later: 追加言語パックの配布（拡張機構に載せる）・コミュニティ翻訳の受け口
- [ ] MVP: クラッシュしても作業を失わない（未保存バッファのバックアップ — VSCode の hot exit 相当の最小版）
- [ ] v1: 状態永続化（開いていたファイル/レイアウト復元 — Zed 方式は SQLite）
- [x] v1: CLI（`ne <path>` で開く — 2026-08-30。`cli_shim` + `necoder cli`。実行中 GUI へは IPC 転送・設置は 設定 > コマンドライン / `necoder install-cli`）
- [ ] later: 自動更新、セッション復元の高度化、Workspace Trust 相当
- [ ] v1: **Remote SSH**（2026-07-13 着手。Host/RPC/OpenSSH/daemon/files/Git/LSP/PTY/ACP は実装済み。
  task・watch・dirty buffer backup・配布署名・askpass UI・実 Linux 長時間受入は未完了）
- [ ] never: telemetry、notebooks、Web 版
