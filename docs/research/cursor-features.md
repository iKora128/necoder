# Cursor 機能全列挙

## 概要

- **素性**: Cursor は Anysphere 社が開発するプロプライエタリな AI コードエディタ。VSCode の非公開 fork であり、ソースは公開されていない。よって本調査はコードではなく Web 一次情報（公式 docs / changelog / blog / security ページ）に基づく。
- **調査時点**: 2026-07-11。最新版は **Cursor 3.11**（2026-07-10 リリース）。docs.cursor.com は cursor.com/docs へ恒久リダイレクト済み。
- **バージョンの流れ（2025〜2026）**: 1.x（2025年前半〜秋: Bugbot、Background Agent、Memories 等）→ **2.0**（2025-10-29: 自社モデル Composer と並列マルチエージェント UI）→ 2.1〜2.6（Plan/Debug モード、Plugins、Marketplace）→ **3.0**（2026-04-02: Agents Window という エージェント第一の新 UI）→ 3.1〜3.11（マルチタスク、Canvases、Automations、iOS アプリ等）。
- **注意**: ページ内容は自動取得の要約に基づくため、URL は将来のドキュメント再編で変わる可能性がある。個別モデル名や価格は 2026-07 時点の記載。

主要出典（ハブ）:
- 公式 docs: https://cursor.com/docs
- Changelog: https://cursor.com/changelog
- 機能ページ: https://cursor.com/features
- 料金: https://cursor.com/pricing
- セキュリティ: https://cursor.com/security

---

## VSCode から継承しているもの（基礎エディタ機能）

テキスト編集、シンタックスハイライト、LSP ベースの言語機能、デバッガ、統合ターミナル、Git 統合、拡張機能システム、settings.json / keybindings.json、コマンドパレット、マルチルートワークスペースといった一般エディタレイヤは VSCode 相当をそのまま継承している。自作エディタの観点では「ここは VSCode が無料で提供してくれている土台」であり、Cursor はこの層でほぼ発明をしていない。fork 由来の差分だけ列挙する。

| 項目 | 内容 | 出典 |
|---|---|---|
| 拡張マーケットプレイス | MS Marketplace は規約上使えず、**Open VSX** を in-app の拡張ライブラリとして利用。主要拡張は Anysphere 名義で自前ビルドを配布（ほぼドロップイン互換）。 | https://forum.cursor.com/t/extension-marketplace-changes-transition-to-openvsx/109138 / https://cursor.com/docs/configuration/extensions |
| MS 専有拡張の欠落 | Pylance、C# Dev Kit、C/C++、Live Share などは Microsoft 専有のため利用不可。VSIX の手動インストール（ドラッグ&ドロップ / Install from VSIX）は可能。 | https://www.datacamp.com/blog/cursor-vs-vs-code |
| VSCode からのインポート | ワンクリックで Extensions / Themes / Settings / Keybindings を移行（Settings > General > Account > VS Code Import）。VSCode プロファイル（Gist / ローカルファイル）の手動インポートも可。 | https://cursor.com/docs/configuration/migrations/vscode |
| バージョン追従 | 常に最新 VSCode ではなく「やや古い安定版」をベースに、定期的に最新へ rebase する方針。fork 故に upstream 追従は遅延する。 | https://cursor.com/docs/configuration/migrations/vscode |
| UI デフォルト変更 | アクティビティバーがデフォルトで水平（チャットパネルの横幅確保のため）。`workbench.activityBar.orientation` で従来の垂直に戻せる。 | https://cursor.com/docs/configuration/migrations/vscode |
| fork 外への展開 | JetBrains IDEs 対応（Agent Client Protocol 経由、2026-03）、CLI、Web（cursor.com/agents）、iOS アプリ / Android PWA。エディタ本体に閉じない方向へ拡大中。 | https://cursor.com/changelog |

---

## AI 機能全列挙（本命）

区分の意味: **中核** = AI ネイティブエディタとして欠くと成立しない。**準中核** = 完成度と差別化を決める。**オプション** = プラットフォーム拡張であり後回しにできる。

### Tab（補完）

Cursor の看板機能。汎用 LLM ではなく、自社訓練の低遅延専用モデルで動く。1日 4 億リクエスト超を処理し、オンライン強化学習（実ユーザーの受諾/拒否を報酬に、表示するか否かの判断までポリシーに組み込む。受諾 +0.75 / 拒否 -0.25 / 非表示 0）で更新される。2025-09 の新モデルでは提案数を 21% 減らしつつ受諾率を 28% 上げたとのこと。

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| インライン補完（ghost text） | カーソル位置にグレー文字で提案表示。直近の編集、周辺コード、linter エラーを入力に使う | 中核 | https://cursor.com/docs/tab/overview |
| マルチライン編集 | 複数行の同時修正、import 文の自動追加、関連コードの協調的な書き換え | 中核 | https://cursor.com/docs/tab/overview |
| ジャンプ予測（jump-in-file） | 提案受諾後にもう一度 Tab で「次の編集位置」を予測してジャンプ | 中核 | https://cursor.com/docs/tab/overview |
| クロスファイル編集 | あるファイルの変更に伴う別ファイルの修正を予測し、エディタ下部のポータルウィンドウに表示 | 準中核 | https://cursor.com/docs/tab/overview |
| 部分受け入れ | Cmd+→ で単語単位に受諾 | 中核 | https://cursor.com/docs/tab/overview |
| 制御系 | ステータスインジケータから一時スヌーズ、全体無効化、ファイルタイプ別無効化。ショートカット再割当可 | 準中核 | https://cursor.com/docs/tab/overview |
| 自社モデル + オンライン RL | 静的データセットではなく、頻繁なチェックポイント更新と実ユーザーフィードバックで on-policy 訓練 | 中核（思想として） | https://cursor.com/blog/tab-rl / https://cursor.com/blog/tab-update |

### Agent / Chat

Cmd+I で開くエージェントが現在の主役。旧 Composer はチャット製品名としては消え、「Composer」は自社モデル名として残った。2025年前半の Agent / Ask / Manual + カスタムモード体制から、現在は **Agent / Plan / Ask / Debug** に再編されている（CLI も Agent / Plan / Ask を踏襲）。

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| Agent モード | ファイル編集と自動適用、ターミナル実行と出力監視、コード検索（semantic + grep + ファイル読み）、Web 検索、ブラウザ操作、画像生成（assets/ 保存）と画像解析。ツール呼び出し回数は無制限 | 中核 | https://cursor.com/docs/agent/overview |
| Ask モード | 読み取り専用の質問モード（コード変更なし） | 中核 | https://cursor.com/docs/cli/overview |
| Plan モード | Shift+Tab で切替。明確化質問 → コードベース調査 → Markdown のプラン生成 → レビュー/編集 → 実装、の流れ。プランはホームディレクトリに保存され workspace へ移せる。プラン作成と実装で別モデルを使える。Mermaid 図の埋め込み対応（2.2） | 準中核 | https://cursor.com/docs/agent/planning / https://cursor.com/docs/agent/modes |
| Debug モード | ランタイムログをアプリに計装して根本原因を探す（2.2、2026年に複数並列セッション対応） | オプション | https://cursor.com/changelog |
| チェックポイント / 巻き戻し | 大きな変更前に自動スナップショット。Git と無関係に以前の状態へ復元できる | 中核 | https://cursor.com/docs/agent/overview |
| キュー投入メッセージ | エージェント作業中に次の指示をキュー（Enter）、Cmd+Enter で割り込み即送信 | 準中核 | https://cursor.com/docs/agent/overview |
| To-do / タスクリスト | エージェントが長いタスクを To-do に分解し進捗表示（1.2 で導入） | 準中核 | https://cursor.com/changelog/1-2 |
| 並列マルチエージェント | 1 プロンプトで最大 8 エージェントを並列実行。各自が git worktree またはリモートマシンの分離コピーで作業し衝突を防ぐ（2.0）。3.0 で local / worktree / cloud / リモート SSH を横断 | 準中核 | https://cursor.com/changelog/2-0 / https://cursor.com/changelog/3-0 |
| best-of-n と自動判定 | `/best-of-n` で同一タスクを複数エージェントに競わせ、multi-agent judging が並列実行結果を自動評価（2.2） | オプション | https://cursor.com/changelog/3-0 / https://cursor.com/changelog |
| /multitask | タスクを分割して非同期サブエージェント群に同時割り当て（3.2） | オプション | https://cursor.com/changelog |
| サブエージェント | 組み込みは Explore（高速モデルで検索）、Bash（冗長出力の隔離）、Browser（DOM ノイズのフィルタ）。カスタムは `.cursor/agents/`（または `~/.cursor/agents/`）に YAML frontmatter 付き Markdown（name, description, model: inherit/指定, readonly, is_background）。独立コンテキストウィンドウで並列実行 | 準中核 | https://cursor.com/docs/agent/subagents |
| Hooks | `hooks.json`（Enterprise MDM / Team / プロジェクト `.cursor/hooks.json` / ユーザー `~/.cursor/hooks.json` の優先順）。beforeSubmitPrompt, preToolUse/postToolUse, beforeShellExecution, beforeMCPExecution, beforeReadFile, afterFileEdit, subagentStart/Stop, sessionStart/End, stop 等のライフサイクルに介入。stdin/stdout の JSON でブロック（deny）、入力書き換え、監査、コンテキスト注入ができる。シェル実行型と LLM 評価型の 2 種 | 準中核 | https://cursor.com/docs/agent/hooks |
| Skills | SKILL.md（YAML frontmatter、name はフォルダ名と一致）を `.cursor/skills/`（リポジトリ内のどこでも、`.agents/skills/` も可）に置く。scripts や assets を同梱でき、エージェントが自動適用するか `/skill-name` で明示呼び出し（disable-model-invocation で slash 専用化）。3.3 でよく使うスキルのピン留め | 準中核 | https://cursor.com/help/customization/skills / https://cursor.com/changelog/2-4 |
| Plugins / Marketplace | skills + subagents + MCP + hooks + rules を 1 インストールにパッケージ（2.5）。Cursor Marketplace に Amplitude, AWS, Figma, Linear, Stripe など 30+ パートナー。チーム専用マーケットプレイス（Default Off/On/Required の配布制御） | オプション | https://cursor.com/changelog |
| 長時間実行エージェント | 計画優先で長期タスクを自律実行する research preview（Ultra/Teams/Enterprise、cursor.com/agents） | オプション | https://cursor.com/changelog |
| 音声入力 / Voice モード | マイクから自然言語で指示、キーワードで実行トリガー（2.0、3.1 で STT 改善）。Design Mode 中のナレーション操作も可 | オプション | https://cursor.com/changelog/2-0 |
| 会話まわりの UX | 明確化 Q&A（2.4）、過去会話の検索（2.5/3.11）、`/side` `/btw` のサイドチャット（3.11）、ピン留めチャット（2.2） | オプション | https://cursor.com/changelog |
| Canvases | エージェントの応答としてダッシュボードや図表などのインタラクティブな成果物を生成。共有スナップショット化（3.5）、コンテキスト使用量の可視化キャンバス | オプション | https://cursor.com/changelog |
| ブラウザ制御 | 後述（Background/Cloud の節ではなくエージェント能力）。ナビゲート、クリック、入力、スクロール、スクリーンショット、console/network 監視。エディタ内埋め込みブラウザで実行過程を表示。要素選択と DOM 転送。Cookie 等はワークスペース単位で永続。実行は手動承認がデフォルト、Enterprise はオリジン許可リスト | 準中核 | https://cursor.com/docs/agent/browser |
| Design Mode | 埋め込みブラウザ上で UI を視覚的に選択・注釈（⌘⇧D、Shift+drag、⌥+click）、位置や色やシャドウをデザインサイドバーで直接調整し、apply でコード化（3.0、3.7 で複数要素選択） | オプション | https://cursor.com/changelog/3-0 |
| 画像生成 / PDF | エディタ内画像生成（Google Nano Banana Pro、2.4）、PDF 添付対応 | オプション | https://cursor.com/changelog/2-4 |

### Inline Edit（Cmd+K）

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| エディタ内 Cmd+K | コード選択 → Cmd+K → 自然言語指示 → その場に diff 適用。追加指示で反復修正できる | 中核 | https://cursor.com/docs/inline-edit |
| クイック質問モード | Opt+Return で編集せずに質問。回答に対し「do it」で実装へ移行 | 準中核 | https://cursor.com/docs/inline-edit |
| Agent への昇格 | 複数ファイルに波及する場合、選択状態で Cmd+L すると選択コードをコンテキストにして Agent に引き継ぐ | 中核 | https://cursor.com/docs/inline-edit |
| ターミナル Cmd+K | 統合ターミナル下部にプロンプトバーを開き、自然言語からシェルコマンドを生成。Esc で挿入のみ、Cmd+Enter で即実行 | 準中核 | https://cursor.com/docs/agent/tools/terminal |

### コンテキストシステム

@-mention（現行ドキュメントに載る一次セット）:

| @記法 | 内容 | 区分 | 出典 |
|---|---|---|---|
| @Files / @Folders | ファイル・フォルダをコンテキストに添付（`@auth.ts`、`@src/components/`） | 中核 | https://cursor.com/docs/context/mentions |
| @Docs | インデックス済みドキュメント（自分で URL を追加したカスタムドキュメント含む）を検索して参照 | 準中核 | https://cursor.com/docs/context/mentions |
| @Terminals | ターミナル出力をコンテキスト化 | 準中核 | https://cursor.com/docs/context/mentions |
| @Past Chats | 過去会話の要約を参照 | 準中核 | https://cursor.com/docs/context/mentions |
| @Commit | 未コミットの作業状態 diff | 準中核 | https://cursor.com/docs/context/mentions |
| @Branch | main との branch 全体 diff | 準中核 | https://cursor.com/docs/context/mentions |
| @Browser | 内蔵ブラウザのコンテキストを添付 | オプション | https://cursor.com/docs/context/mentions |
| 画像 / 音声 | 画像はドラッグ&ペースト、音声はマイクアイコンで口述 | 準中核 | https://cursor.com/docs/context/mentions |

歴史的・補助的な @記法（旧 docs とコミュニティ資料に見えるもの。@Web や @Codebase は「エージェントが自律的に web 検索 / semantic search ツールを呼ぶ」方式へ吸収されつつある）: @Code、@Web、@Git、@Codebase、@Cursor Rules、@Definitions（Inline Edit 用）、@Lint Errors、@Recent Changes、@Link、`#` でのファイル添付、`/` コマンド。出典: https://docs.cursor.com/en/context/@-symbols/overview / https://toolsbase.dev/en/reference/cursor-commands

インデックスと除外:

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| Codebase indexing | ワークスペースを開くと自動開始。関数やクラス単位にチャンク → 自社 embedding モデルでベクトル化 → サーバ側ベクトル DB に保存。80% 完了で semantic search が使用可能。5 分毎に差分同期 | 中核 | https://cursor.com/docs/context/codebase-indexing |
| インデックスのプライバシー設計 | コード平文はサーバに保存しない（インデックス中メモリ内のみ）。ファイルパスは暗号化して送信、保存されるのは embeddings のみ。検索時はクライアント側で復号。最終アクセスから 6 週間で破棄 | 中核 | https://cursor.com/docs/context/codebase-indexing |
| semantic search + grep | grep 単独に比べコードベース質問の精度 +12.5%（1,000 ファイル超で効果大）。エージェントが記号検索は grep、概念検索は semantic と使い分ける | 中核 | https://cursor.com/docs/context/codebase-indexing |
| マルチルート / チーム共有 | multi-root workspace の全コードベースを自動インデックス。チームでのインデックス共有（個人のファイルアクセス権は尊重） | 準中核 | https://cursor.com/docs/context/codebase-indexing |
| .cursorignore | AI アクセス自体を遮断（Agent / Tab / Inline Edit / @ 参照）。ただしターミナルと MCP ツールは貫通し、LLM の性質上完全な保護は保証されない旨明記。gitignore 構文、`!` による復活、デフォルト除外（lock ファイル、.env*、node_modules、バイナリ等）、グローバル除外リスト、親ディレクトリ探索（hierarchical ignore） | 中核 | https://cursor.com/docs/context/ignore-files |
| .cursorindexingignore | インデックスからのみ除外し、AI アクセスは許す（巨大な生成物や vendor 向け） | 準中核 | https://cursor.com/docs/context/ignore-files |
| コンテキスト可視化 | プロンプト入力欄でコンテキストをインラインの pill として表示、自動収集（2.0）。rules / skills / MCP / subagents 別のトークン内訳表示（3.3） | 準中核 | https://cursor.com/changelog/2-0 / https://cursor.com/changelog |

### Rules / Memories

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| Project Rules | `.cursor/rules/*.mdc`（バージョン管理対象）。frontmatter で 4 タイプ: Always Apply / 説明文に基づきエージェントが判断 / glob 一致で自動添付 / @ 指名時のみ。ネスト配置可。500 行以下推奨。ルール内容はモデルコンテキストの先頭に注入される | 中核 | https://cursor.com/docs/context/rules |
| AGENTS.md | frontmatter 不要の簡易形式。プロジェクトルートとサブディレクトリに置け、親子で結合、より具体的な方が優先。他ツールとの相互運用の要 | 中核 | https://cursor.com/docs/context/rules |
| User Rules | 全プロジェクト共通の個人設定（Customize → Rules）。Agent（Chat）にのみ効き、Inline Edit には効かない | 準中核 | https://cursor.com/docs/context/rules |
| Team Rules | Team/Enterprise プランでダッシュボード管理。即時有効化と強制（メンバーが無効化できない）を選べる。優先順は Team → Project → User | オプション | https://cursor.com/docs/context/rules |
| 旧 .cursorrules | レガシー形式。現行 docs からは事実上退場（`.cursor/rules` と AGENTS.md へ移行） | 記録のみ | https://cursor.com/docs/rules |
| Memories | 会話をバックグラウンドのモデルが観察して覚えるべき事実を提案し、ユーザー承認で保存（1.0 で導入）。プロジェクトスコープ。Settings → Rules の Generate Memories でオンオフ、一覧から編集や削除 | 準中核 | https://forum.cursor.com/t/rules-vs-memories-and-global-vs-project/137149 / https://cursor.com/changelog |
| BUGBOT.md | Bugbot 専用のレビュー規約ファイル（`.cursor/BUGBOT.md`、ディレクトリツリーを遡って合成） | オプション | https://cursor.com/docs/bugbot |

### MCP

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| 設定ファイル | プロジェクト `.cursor/mcp.json` とグローバル `~/.cursor/mcp.json`。`${env:NAME}` `${workspaceFolder}` 等の変数展開 | 準中核 | https://cursor.com/docs/context/mcp |
| トランスポート | stdio（ローカル）、SSE、Streamable HTTP（リモート）。リモートは OAuth 対応（デスクトップ `cursor://anysphere.cursor-mcp/oauth/callback`、Web/Cloud 用固定 callback） | 準中核 | https://cursor.com/docs/context/mcp |
| プロトコル対応範囲 | Tools、Prompts、Resources、Roots、Elicitation、Apps（MCP Apps: チャートやホワイトボード等のインタラクティブ UI をチャット内表示、2.6）。画像レスポンス（base64）も解析対象にできる | 準中核 | https://cursor.com/docs/context/mcp |
| 導入 UX | Marketplace / cursor.directory からのワンクリックインストール（deeplink）。`/mcp list` 等の CLI 管理 | 準中核 | https://cursor.com/docs/context/mcp |
| 実行制御 | デフォルトはツール実行前に承認要求。Auto-review モードでは allowlist は即実行、それ以外は分類器サブエージェントが判断 | 準中核 | https://cursor.com/docs/context/mcp |
| チーム / Enterprise 統制 | ダッシュボードからチーム共有 MCP を配布（Cloud Agents でも利用）。Enterprise はコマンド/URL パターンの allowlist、サーバ単位のネットワークポリシー（Allow all / Allowlist / Deny all / No sandbox） | オプション | https://cursor.com/docs/context/mcp |

### Background / Cloud Agent, Bugbot

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| Cloud Agents（旧 Background Agents） | クラウドの隔離 VM 上でフル開発環境を持って動くエージェント。GitHub / GitLab / Azure DevOps / Bitbucket からクローンし、別ブランチで作業して merge-ready な PR を作成。スクリーンショットや動画やログを成果物として添付 | オプション | https://cursor.com/docs/cloud-agent |
| 環境定義 | エージェント主導の対話セットアップ、スナップショット再利用、`.cursor/environment.json` と Dockerfile（ビルドシークレット、レイヤキャッシュで 70% 高速化、環境のバージョン履歴とロールバック） | オプション | https://cursor.com/docs/cloud-agent |
| 起動サーフェス | デスクトップ（Cloud 選択）、Web（cursor.com/agents）、iOS アプリ（public beta）と Android PWA、Slack の @cursor、Microsoft Teams、Jira、Linear、GitHub/Bitbucket コメント、API、CLI の `&` プレフィックス | オプション | https://cursor.com/docs/cloud-agent / https://cursor.com/changelog |
| モバイル遠隔操作 | Remote Control でデスクトップのセッションをスマホから指示。Live Activities とプッシュ通知で進捗追跡（3.9） | オプション | https://cursor.com/changelog |
| 実行形態 | 常に Max Mode + 選択モデルの API 課金。支出上限の設定が必須。self-hosted（オンプレ VM）オプションあり（2026-03） | オプション | https://cursor.com/docs/cloud-agent / https://cursor.com/changelog |
| Computer use | VM 内でブラウザ/デスクトップを操作して変更のテストやデモ動画生成まで行う（2026-02、3.8 で /automate デモ生成） | オプション | https://cursor.com/changelog |
| クラウドサブエージェント | `/in-cloud` でローカル会話からクラウド VM にサブタスクを分離、`/babysit` で PR 準備を裏で継続 | オプション | https://cursor.com/changelog / https://cursor.com/docs/agent/subagents |
| Automations | スケジュールやトリガー（Slack、Linear、GitHub、PagerDuty、webhook）で走る常駐エージェント。メモリツールで改善、マルチリポ対応、リポジトリ不要のテンプレート（Slack ダイジェスト等） | オプション | https://cursor.com/changelog |
| Bugbot | PR の diff を解析してバグ、脆弱性、品質問題を検出。PR 全体コメント + インラインコメント + Fix in Cursor / Fix in Web ボタン。既存コメントを読んで重複回避。GitHub（GHES 含む）、GitLab（self-hosted 含む）、Bitbucket（Data Center 含む）対応、CI チェック連携 | オプション | https://cursor.com/docs/bugbot |
| Bugbot の学習と設定 | `.cursor/BUGBOT.md`、PR フィードバックからの learned rules（解決率 78%）、ダッシュボードの glob ルール、`@cursor remember` によるその場教示。effort レベル（Default 0.7 バグ/回、High 0.95、Custom）。Autofix は Cloud Agent が修正を作成し 35% 超が本体 PR にマージされたとのこと。2026-06 時点で平均レビュー 90 秒 | オプション | https://cursor.com/docs/bugbot / https://cursor.com/changelog |
| Security Review（beta） | PR の脆弱性やインジェクションを見る Security Reviewer と、定期スキャンの Vulnerability Scanner（Slack 通知） | オプション | https://cursor.com/changelog |
| CLI | ターミナルの対話 TUI と非対話 print モード（`-p`、CI 向け構造化出力）。Agent / Plan / Ask モード、モデル切替、セッション再開、sandbox 制御、`&` でクラウドへ引き継ぎ、`/debug` `/btw` `/statusline` 等 | 準中核 | https://cursor.com/docs/cli/overview |
| SDK | TypeScript / Python でプログラマブルにエージェントを構築（`@cursor/sdk`、custom tools、permissions.json、ネスト可能なサブエージェント、SSE ストリーミングの Cloud Agents API） | オプション | https://cursor.com/changelog |

### モデル

| 項目 | 説明 | 区分 | 出典 |
|---|---|---|---|
| Composer（自社モデル） | 2.0 で初代を投入した自社のエージェント特化モデル。同等知能のモデル比 4 倍高速、ほとんどのターンを 30 秒以内で完了。semantic search や編集やテスト実行のツールを持たせた環境で強化学習し、リポジトリ内の多段問題解決を学習。Composer 2（2026-03、frontier 級を主張）、Composer 2.5（2026-05、長時間タスクの持続性向上。Fast がデフォルトで $3/M 入力、$15/M 出力） | 中核（自社モデル戦略として） | https://cursor.com/blog/2-0 / https://cursor.com/changelog |
| Auto | タスクに応じて知能とコストと信頼性のバランスでモデルを自動選択。個人プランでは実質無制限に近い扱い | 中核 | https://cursor.com/docs/models |
| Max Mode | コンテキストウィンドウをモデル上限まで拡張するモード。API 実費課金（Teams はサードパーティモデルに +$0.25/M の Cursor Token Rate） | 準中核 | https://cursor.com/docs/models |
| 対応プロバイダ | Anthropic Claude（4.x 系、Fable 5、Opus 4.8 等）、OpenAI GPT-5〜5.6 系（Codex、mini/nano 含む）、Google Gemini（Flash/Pro、3.x）、xAI Grok 4.5（models ページには Cursor と共同訓練との記載）、GLM、Kimi など。モデル名は 2026-07 時点 | 中核 | https://cursor.com/docs/models |
| 利用枠の二層構造 | first-party プール（Auto、Composer、Grok。含み利用が潤沢）と API プール（モデル別レートで消費、個人プランは月 $20〜$400 相当の含み枠） | 準中核 | https://cursor.com/docs/models |
| BYOK | Enterprise は自前 API キーを持ち込める（Cursor Token Rate は残る） | オプション | https://cursor.com/docs/models |

### Privacy / Enterprise

| 機能 | 説明 | 区分 | 出典 |
|---|---|---|---|
| Privacy Mode | 有効時はコードがモデル訓練に使われない。技術的統制とモデルプロバイダとの契約で担保。無料ユーザーでも利用可、チーム管理者が全員に強制でき、新規メンバーは設定を継承 | 中核 | https://cursor.com/security |
| 認証と監査 | SOC 2 Type II、年 1 回以上の第三者ペネトレーションテスト、trust.cursor.com でサブプロセッサ一覧と証明書を公開。中国国内のインフラは不使用と明記 | 準中核 | https://cursor.com/security |
| インデックスの扱い | 前述の通り平文コード非保存、パス暗号化、embeddings のみサーバ保持 | 中核 | https://cursor.com/docs/context/codebase-indexing |
| ID とシート管理 | SAML/OIDC SSO（Teams〜）、SCIM（Enterprise）、MDM 配布、サービスアカウント | オプション | https://cursor.com/pricing / https://cursor.com/security |
| 管理と分析 | 監査ログ、ユーザー/サーフェス別（クライアント、Cloud Agents、Automations、Bugbot、Security Review）の利用分析、会話の分類インサイト、AI code tracking API と Cursor Blame（AI 由来コードの帰属追跡、Enterprise） | オプション | https://cursor.com/changelog / https://cursor.com/pricing |
| 統制 | モデルアクセス制御（プロバイダ/構成単位のブロックリスト）、支出ソフトリミット（50/80/100% アラート）、リポジトリ/MCP アクセス制御、auto-run とブラウザとネットワークの制御、sandboxed terminals（macOS はデフォルト有効、Linux 対応済み。ワークスペース読み書き可 + ネット遮断）、ネットワーク egress ポリシーの強制 | オプション | https://cursor.com/changelog/2-0 / https://cursor.com/changelog |
| 組織構造 | Organizations（複数チームの統括）> Teams > Groups の 3 層管理（2026-06） | オプション | https://cursor.com/changelog |

---

## UI 面での VSCode との差分

VSCode の UI を土台にしつつ、「エージェントを主役にする」方向へ段階的に改装してきた。2.0 と 3.0 が大きい節目。

| UI 要素 | 内容 | 出典 |
|---|---|---|
| サイドバーチャット | Cmd+I（Agent）と Cmd+L（選択コードを添えて送る）で右パネルにチャット。アクティビティバーを水平化して幅を確保 | https://cursor.com/docs/agent/overview / https://cursor.com/docs/configuration/migrations/vscode |
| Tab の ghost text | インライン提案 + 次編集位置へのジャンプ + クロスファイル用ポータルウィンドウという、補完を「編集の連続予測」に拡張した表示系 | https://cursor.com/docs/tab/overview |
| エディタ内 diff レビュー | エージェントの複数ファイル変更をファイル間を行き来せず一括レビュー（2.0 で刷新）、word-level のインライン diff、Reviews / Commits / Changes タブ、PR 分割アクション（3.3） | https://cursor.com/changelog/2-0 / https://cursor.com/changelog |
| プロンプトバー | コンテキストをインライン pill 表示、自動コンテキスト収集で手動添付を減らす（2.0） | https://cursor.com/changelog/2-0 |
| Agents Window（3.0） | エディタから独立したエージェント第一のウィンドウ。ローカル、worktree、クラウド、SSH の全エージェントを 1 箇所に表示。Slack や GitHub や モバイル発のエージェントも同じサイドバーに載る | https://cursor.com/changelog/3-0 / https://cursor.com/blog/cursor-3 |
| Agent Tabs とタイル | チャットを横並びやグリッドで表示（3.0）、ペイン分割のタイルレイアウト（3.1）、Mission Control 的な「開いているウィンドウのライブプレビュー付きグリッドビュー」 | https://cursor.com/changelog / https://cursor.com/features |
| レイアウトプリセット | agent / editor / zen / browser の 4 レイアウトを ⌘⌥⇥ で切替（2.3）。フルスクリーンタブ ⌘⇧M と 3 段階密度のコンパクトチャット（3.4） | https://cursor.com/changelog |
| 内蔵ブラウザ + Design Mode | ブラウザをエディタ内ペインとして埋め込み、要素選択と DOM 転送、スタイルの視覚編集からコード反映まで | https://cursor.com/docs/agent/browser / https://cursor.com/changelog/3-0 |
| その他 | 音声入力 UI、Canvases（チャット内のインタラクティブ成果物）、会話検索、`/side` サイドチャット、エディタ内 AI コードレビュー（2.1） | https://cursor.com/changelog |

---

## 料金と機能の線引き

2026-07 時点。個人は含み利用枠（クレジット）の大きさが実質の差で、機能自体はほぼ全プランに開放されている点が特徴。

| プラン | 価格 | 解放されるもの | 出典 |
|---|---|---|---|
| Hobby（無料） | $0 | 限定回数の Agent リクエストと Tab 補完。Privacy Mode は無料でも使える | https://cursor.com/pricing / https://cursor.com/security |
| Pro | $20/月 | Agent 上限の拡大、frontier モデル群、Grok と Composer の潤沢な含み枠、MCP / skills / hooks、Cloud Agents、Bugbot（usage-based）。含み枠は $20 相当 | https://cursor.com/pricing |
| Pro+ / Ultra | $60 / $200/月 | 中身は Pro と同じで含み枠が 3 倍 / 20 倍（$400 相当）。Ultra は新機能の優先アクセス | https://cursor.com/pricing / https://aiproductivity.ai/blog/cursor-pricing/ |
| Teams | $40/ユーザー/月 | 集中課金と管理、チームマーケットプレイス（社内 rules / skills / plugins 配布）、Bugbot の組織運用、チーム共有コンテキストの Cloud Agents と Automations、利用分析、チーム全体の Privacy Mode 強制、SAML/OIDC SSO。サードパーティモデルに Cursor Token Rate（$0.25/M） | https://cursor.com/pricing / https://cursor.com/docs/models |
| Enterprise | 個別見積 | プール利用枠、請求書払い、SCIM、リポジトリ/モデル/MCP アクセス制御、auto-run とブラウザとネットワークの統制、監査ログ、サービスアカウント、AI code tracking API、self-hosted Cloud Agents、Organizations 階層、BYOK | https://cursor.com/pricing |

別建てで動く課金: Bugbot（含み枠消費後は従量）、Cloud Agents（常に Max Mode + モデル API 実費、支出上限必須）、Max Mode（API レート）。「エディタ機能はほぼ無料〜$20 に開放し、モデル消費量とチーム統制で稼ぐ」構造。

---

## 自作エディタへの示唆

Cursor の構成を逆算すると、AI ネイティブエディタは 4 層に分解できる。GPUI で作る場合、Cursor が fork でタダ取りした第 1 層（エディタ基礎、LSP、ターミナル、Git、拡張機構）を自前で払う必要がある点が最大の違いで、Cursor 自身はこの層に投資せず AI 層に全振りした。

**第 1 層: エディタ基礎（Cursor は VSCode から継承）**
自作では Zed / GPUI 側のエコシステムで賄う層。Cursor から学ぶことは少ないが、「Open VSX + 自前ビルド拡張」という現実解は、拡張エコシステムを自前で持てない場合の参考になる。

**第 2 層: AI 中核（これが無いと AI ネイティブと呼べない）**
- Tab 型補完: ghost text、複数行編集、部分受諾、そして受諾後の「次の編集位置へのジャンプ」。Cursor はここを汎用 LLM でなく専用低遅延モデル + オンライン RL で作っており、補完は「テキスト生成」ではなく「編集行動の予測」問題として扱っている。自作でも最初から「ジャンプとクロスファイル」を UI 契約に含めておくと後で効く。
- エージェントチャット: ツールループ（検索、読み、編集、ターミナル）、diff の一括レビュー UI、Git 非依存のチェックポイント巻き戻し。チェックポイントは信頼の担保としてほぼ必須。
- Inline Edit（Cmd+K）とターミナル Cmd+K: チャットに行かない最短経路。Agent への昇格導線（Cmd+L）とセットで設計する。
- コンテキスト注入: @-mention の型（file / folder / diff / terminal / 過去会話 / docs）と codebase index。インデックスをサーバ側 embeddings にするかローカルにするかは privacy 姿勢と直結するので初期に決める必要がある。Cursor は「平文非保存 + embeddings のみサーバ」という折衷を取った。
- Rules: AGENTS.md 互換は事実上の業界標準になっており、対応コストも低い。`.cursor/rules` 型の glob 自動添付まで作るかは後で良い。
- モデルルーティング: 複数プロバイダ + Auto 選択 + 適用（apply）専用の高速パス。「思考するモデル」と「diff を当てる高速モデル」の分離は Cursor の初期からの設計。

**第 3 層: 準中核（完成度と差別化）**
Plan モード、サブエージェントと並列実行（git worktree 分離）、MCP クライアント、hooks、.cursorignore 体系、Privacy Mode、CLI。MCP と AGENTS.md は他ツールとの相互運用点なので優先度高め。8 並列エージェントのような派手さより、worktree 分離という基盤の方が本質的な投資。

**第 4 層: オプション（プラットフォーム事業の層）**
Cloud Agents、Bugbot、Automations、Marketplace / Plugins、Canvases、Design Mode、モバイル/Web、SDK。2026 年の Cursor はほぼこの層に新機能を積んでおり、エディタというよりエージェント実行基盤の会社になりつつある。個人開発の自作エディタでは丸ごと後回しでよいが、「エディタ外（CI、PR、Slack）でも同じエージェントが動く」という方向性自体は、アーキテクチャを editor-embedded にしすぎない理由になる。エージェントコアをエディタプロセスから分離しておく（Cursor が CLI / SDK / ACP 経由の JetBrains 対応で示した形）のが教訓。

要するに Cursor の独自価値は、(1) 編集行動を予測する専用 Tab モデル、(2) diff 適用とチェックポイントを備えたエージェントループ、(3) サーバ側インデックスによるコードベース理解、の 3 点に集約され、残りはその上の運用装置と言える。

---

### 参照 URL 一覧

公式: cursor.com/docs, cursor.com/changelog（および /2-0, /3-0, /2-4, /1-2, /page/2〜10）, cursor.com/features, cursor.com/pricing, cursor.com/security, cursor.com/blog/2-0, cursor.com/blog/cursor-3, cursor.com/blog/tab-rl, cursor.com/blog/tab-update, cursor.com/docs/{tab/overview, agent/overview, agent/modes, agent/planning, agent/browser, agent/subagents, agent/hooks, agent/tools/terminal, inline-edit, context/mentions, context/codebase-indexing, context/ignore-files, context/rules, context/mcp, cloud-agent, bugbot, models, cli/overview, configuration/migrations/vscode, configuration/extensions, help/customization/skills}, trust.cursor.com

非公式（補助）: forum.cursor.com（OpenVSX 移行、Rules vs Memories、Cursor 3 スレッド）, datacamp.com（VS Code 比較 / Cursor 3）, aiproductivity.ai（料金）, toolsbase.dev（@記法一覧）, infoq.com / devops.com（2.0 報道）
