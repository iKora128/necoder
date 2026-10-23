# Orca（stablyai/orca・並列コーディングエージェントの ADE）調査ノート

調査日: 2026-09-20。数値・バージョンはすべてこの日の取得値。
一次資料は GitHub API / リポジトリ本体（README・`docs/site/content/docs/*.mdx`・`skill-guides/`・`src/`・issue・discussion）を直接読んだ。
公式ドキュメント（https://www.onorca.dev/docs/…）は同じ mdx からビルドされるので、引用元はリポジトリ側のファイル URL にしてある。
HN のコメントは Algolia API の生テキスト（逐語）。一方 agentconn / Nimbalyst / Superset / note / Qiita / 個人ブログは WebFetch（要約モデル経由）で読んだため、「」内が逐語でない可能性がある。
「検証済み」= 一次資料で確認、「第三者」= 二次資料の主張、「推論」= 本ノートの解釈、と書き分ける。

## 0. どの「Orca / ORCA」か（同名プロダクトの整理）

### Takeaway
ユーザーの言う「ORCA」は **stablyai/orca（Stably AI の MIT 製 ADE）で確定**。リポジトリ URL・会社・ライセンスは GitHub API と YC のページで裏が取れた。
AI エージェント界隈には同名が少なくとも 8 つあり、そのうち「複数エージェントの采配」を主題にするのは fmfsaisai/orca（tmux の lead/worker）・Danau5tin の Orca-Agent（コードに触らないオーケストレータ）・論文 ORCA（ORchestrating Causal Agent）の 3 つ。

### Cited Findings
- **(本命) stablyai/orca** — 説明文 "Orca is the ADE for working with a fleet of parallel agents. Run any coding agent with your own subscription. Available on desktop, mobile and remote runtime."、MIT、2026-03-17 作成、topics に `ade` `orchestration` `parallel-agents` `worktrees` `yc-backed`。— [GitHub API](https://api.github.com/repos/stablyai/orca) / [repo](https://github.com/stablyai/orca)
- **fmfsaisai/orca** — "Multi-agent orchestrator — parallel AI agents with tmux + skills. Model-agnostic: any AI CLI as lead or worker."。MIT・star 8・2026-04-16 作成・最終 push 2026-08-25。流れは `User → Lead dispatches → Worker codes → /review self-check → report Lead → /simplify optimize → report User`。`orca --workers 3 --worktree` で 1 lead + 3 worker（worktree は `.orca/worktree/<slug>`・複数 worker では必須）。特徴として "Push, not poll — Workers notify lead on completion. No token-burning loops"、"Bidirectional — Workers can propose plans to lead"。既知の制限に "Heartbeat is not real-time: idle notifications surface on lead's next tool use, not instantly"。— [fmfsaisai/orca](https://github.com/fmfsaisai/orca)
- **Danau5tin の Orca-Agent** — `multi-agent-coding-system`（Apache-2.0・star 1,450・2025-08-30 作成）は "an orchestration agent that dispatches multiple explorer and coder agents to do all the work"。オーケストレータは "receives the user's task but never touches code directly"、"Restrictions: Cannot read or modify code directly - operates purely at architectural level"、"The orchestrator's lack of direct code access forces proper delegation and verification patterns"。サブエージェントが返すべき knowledge artifact を明示し context store に蓄える。TerminalBench 13 位（Claude Sonnet-4 で成功率 37.0%）。2025-11-04 に RL で訓練した 14B のオーケストレータ `Orca-Agent-v0.1`（Qwen3-14B で TerminalBench 相対 +160.71%・H100×32・Docker 256 並列）を公開。— [multi-agent-coding-system](https://github.com/Danau5tin/multi-agent-coding-system) / [Orca-Agent-RL](https://github.com/Danau5tin/Orca-Agent-RL)
- **論文 ORCA: ORchestrating Causal Agent** — Joanie Hayoun Chung / Sumin Lee / Sungbin Lim。2025-08-29 投稿・2026-03-18 v3・CHI EA 2026。関係 DB 上の因果分析を専門エージェント群に分け、"maintaining shared state and introducing human checkpoints" で段階間の一貫性を保つ。ユーザー実験で GPT-4o-mini ベースラインより完遂率 +42 ポイント・反復的なデータ探索の時間 −76%。— [arXiv 2508.21304](https://arxiv.org/abs/2508.21304) / [ACM DL](https://dl.acm.org/doi/10.1145/3772363.3798444)
- **Microsoft Orca（LLM）** — "Orca: Progressive Learning from Complex Explanation Traces of GPT-4"（2023-06-05）。GPT-4 の説明トレースで学習した 13B モデル。エージェント采配とは無関係。— [arXiv 2306.02707](https://arxiv.org/abs/2306.02707)
- **echoVic/orca-agent** — "Orca is a DeepSeek-native coding agent."（MIT・star 506・2026-06-05 作成）。単体のコーディングエージェント。— [echoVic/orca-agent](https://github.com/echoVic/orca-agent)
- **santiagomed/orca** — "LLM Orchestrator built in Rust"（Apache-2.0・star 286・最終 push 2024-03）。LLM パイプライン用ライブラリで休眠中。— [santiagomed/orca](https://github.com/santiagomed/orca)
- **spinnaker/orca** — Spinnaker の "Orchestration engine"（CD パイプライン。AI ではない）。— [spinnaker/orca](https://github.com/spinnaker/orca)
- **OrcaRouter（Continuum-AI-Corp）** — "Self-hosted LLM router …"（star 1,482）。推論ルーター。— [OrcaRouter-Lite](https://github.com/Continuum-AI-Corp/OrcaRouter-Lite)
- **GNOME Orca（スクリーンリーダー）** — 名前衝突の実害あり。Orca の CLI は Linux では `orca-ide` になる（"GNOME Orca — the screen reader … already owns `/usr/bin/orca`"）。— [docs/cli/reference.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/reference.mdx)

### Inferences
- 采配系の 3 つはどれも「オーケストレータはコードに触らない / 人間チェックポイントを挟む / push 型で poll しない」に収束しており、necoder の Captain（コードを書かない・integrate できない・イベント駆動）と同じ結論に独立に達している。
- fmfsaisai/orca の「heartbeat は lead の次の tool use まで届かない」は、PTY/CLI 越しに采配役を起こす方式の共通の弱点で、本命 Orca も同じ問題を抱える（§6）。

### Gaps
- Orca Security（クラウドセキュリティ企業）など非エージェント系の同名は今回は裏取りしていない（本件と無関係なので省略）。
- X（Twitter）で「ORCA」を名乗る別プロダクトの有無は検索に掛からず未確認。

## 1. Identity（リポジトリ・会社・ライセンス・技術スタック・リリース・採用指標・価格）

### Takeaway
https://github.com/stablyai/orca ・Stably AI（YC W22・SF）・MIT・**Electron + React + xterm.js(WebGL) + node-pty + Monaco** のデスクトップアプリ（mac/Win/Linux）に iOS/Android のコンパニオンとヘッドレス `orca serve` が付く。
初コミット 2026-03-17 から 6 か月で star 72,853、安定版をほぼ毎日出し 2026-09-17 時点で v1.4.205。本体は無料で有料プランは見当たらない。

### Cited Findings
- star 72,853 / fork 4,765 / open issue+PR 6,281 / watcher 129 / 主言語 TypeScript / `created_at` 2026-03-17T03:28:57Z / 最終 push 2026-09-20（取得 2026-09-20）。— [GitHub API](https://api.github.com/repos/stablyai/orca)
- 言語内訳: TypeScript 約 156MB、JavaScript 6MB、Swift 369KB、HCL 227KB、Kotlin 92KB、PowerShell、C++、C#、NSIS ほか。— [languages API](https://api.github.com/repos/stablyai/orca/languages)
- コントリビュータは匿名込みで約 381 人（API のページ数から）。main のコミットは約 11,400。— [contributors API](https://api.github.com/repos/stablyai/orca/contributors?per_page=1&anon=true) / [repo](https://github.com/stablyai/orca)
- 依存（main の package.json・`version` 1.4.197・description "Next-gen IDE for parallel agentic development"）: `electron` 43.7.0、`electron-vite`、`react` ^19.2.8、`@xterm/xterm` 6.1.0-beta + `@xterm/addon-webgl` + `@xterm/headless`、`node-pty` ^1.1.0、`monaco-editor` ^0.55.1、`ssh2`、`ws`、`zustand`、`tailwindcss` ^4、`posthog-node`、`@anthropic-ai/claude-agent-sdk` 0.3.251、`agent-browser`。— [package.json](https://github.com/stablyai/orca/blob/main/package.json)
- リポジトリ構成: `src/{main,renderer,cli,shared}`（Electron）、`mobile/`（Expo 系。`mobile/packages/expo-two-way-audio` など）、`cloud/`（モバイル用 relay と push。APNs/FCM・Postgres・Terraform）、`native/`（computer-use の mac/Win/Linux ネイティブ、Windows CLI launcher）、`skills/` `skill-guides/`、`examples/plugins/`。README も "The relay that pairs the mobile app with a desktop host is also in this repository under `cloud/`" と明記。— [README](https://github.com/stablyai/orca/blob/main/README.md) / [repo tree](https://github.com/stablyai/orca)
- 配布: macOS（Apple Silicon / Intel の DMG）、Windows（.exe・SignPath で署名）、Linux（AppImage）、`brew install --cask stablyai/orca/orca`、AUR `stably-orca-bin`、iOS（App Store "Orca IDE" id6766130217 と TestFlight）、Android（APK 0.0.50）、ヘッドレス Linux は `orca serve`。— [README](https://github.com/stablyai/orca/blob/main/README.md)
- リリース: v1.4.193（2026-08-31）→ v1.4.205（2026-09-17）で安定版 12 本 / 18 日。GitHub Releases の総数は 955 件（rc とモバイルを含む）で、prerelease でない安定版は 282 件 = 約 1.5 本/日（2026-09-20・API を全ページ数えた値）。最古のリリースは v1.0.12（2026-03-21）。Android は `mobile-android-v0.0.50`（2026-09-18）。README は "we ship daily, so this list is perpetually behind. The changelog is the real feature list." と書く。— [releases](https://github.com/stablyai/orca/releases) / [releases API](https://api.github.com/repos/stablyai/orca/releases?per_page=100&page=10)
- v1.4.206 は 2026-09-18 にタグが切られたが未公開、v1.4.202 は公開されず内容は v1.4.203 で出た（コントリビュータの追跡 issue による）。— [issue #15190](https://github.com/stablyai/orca/issues/15190)
- 最初の公開告知は 2026-03-27 の "Show HN: Open Source 'Conductor + Ghostty'"（21 points・10 comments）。投稿者 nwparker: "Our team works with Claude Code, Codex, Gemini all day. We love Ghostty, but wanted something where we could work in multiple worktree at once and have multiple agents run. We decided to open source the internal team we use." — [HN 47549197](https://news.ycombinator.com/item?id=47549197)
- 会社: YC のページは "Stably AI (Orca)"・Winter 2022・創業 2022・San Francisco・team size 25。創業者は Jinjing Liang（CEO・元 Google Chrome）と Neil Parker（元 Uber の Tech Lead）。one-liner は "MIT open source terminal-based agent orchestrator"、対象は "engineers managing 10-100 coding agents simultaneously"。もう 1 つの製品は AI の E2E テスト基盤 Stably。— [YC: stably-ai-orca](https://www.ycombinator.com/companies/stably-ai-orca) / [YC: stably-ai](https://www.ycombinator.com/companies/stably-ai)
- チーム規模は資料で食い違う: YC（Orca）ページ 25 人 / YC（Stably AI）ページの検索スニペット 8 人 / agentconn は "four-person" チーム。— [YC](https://www.ycombinator.com/companies/stably-ai-orca)；食い違い [agentconn](https://agentconn.com/blog/orca-ade-agent-fleet-parallel-coding-agents-2026/)
- 価格: 公式サイトは "Free and open source"・"Backed by Y Combinator" で有料 tier の記載なし。エージェントの契約は持ち込み（BYO subscription）。競合 Superset の比較表でも Orca は "Free"、Conductor は "Free tier; Pro $50/mo; Teams $60/user/mo"。— [onorca.dev](https://www.onorca.dev/) / [Superset の比較（競合作成・2026-08-30 更新）](https://superset.sh/compare/orca-vs-conductor)
- リモート計算資源は売らない: "Orca does **not** sell managed VPS hosting. Remote modes always use machines and cloud accounts you control." — [docs/ways-to-run.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/ways-to-run.mdx)
- テレメトリ: PostHog Cloud（US）・匿名 ID・内容は送らない・`DO_NOT_TRACK=1` / `ORCA_TELEMETRY_DISABLED=1` / 設定で opt-out。— [docs/telemetry.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/telemetry.mdx)
- star の推移（第三者の記述を時系列に並べたもの）: 2026-06-28 時点 "8,000スター超・リリース数660超"（[Qiita tamakiiii](https://qiita.com/tamakiiii/items/9b22ca0b287309ebba32)）→ 2026-07 時点 11.7k（[Qiita kai_kou](https://qiita.com/kai_kou/items/0c0bb9bd38a3bf1c1084)）→ 初コミットから 5 か月で 43,000・8 月下旬 53,000・2026-08-24 は +929/日（[agentconn](https://agentconn.com/blog/orca-ade-agent-fleet-parallel-coding-agents-2026/)）→ 2026-09-20 に 72,853（[GitHub API](https://api.github.com/repos/stablyai/orca)）。
- README の downloads バッジは "69m"。ただし v1.4.205 単体で `latest-mac.yml` が 2,269,013 回、`orca-macos-arm64.dmg` が 163,489 回、`orca-windows-setup.exe` が 121,251 回で、自動更新のメタデータ取得が大半を占める。— [releases API v1.4.205](https://api.github.com/repos/stablyai/orca/releases/tags/v1.4.205) / [README バッジ](https://github.com/stablyai/orca/blob/main/docs/assets/readme-downloads.svg)

### Inferences
- 「69m downloads」は利用者数ではない。安定版 1 本あたり mac の更新 zip が 3 日で十数万回落ちているので、アクティブな導入は 10 万台規模と見るのが妥当（x64 と arm64 がほぼ同数など不自然な点もあり、精度は低い）。
- 収益化は未着手に見える。Orca アカウント（Relay・Artifacts・スキル共有）が既にあるので、課金するならクラウド側（relay / 共有 / チーム機能）から始まる公算が高い。HN でも競合の作者が「この市場では orchestration・cloud・enterprise 機能が後から closed になりがち」と指摘している（[HN 49203107](https://news.ycombinator.com/item?id=49203107)）。
- 6 か月で 955 リリース・約 11,400 コミットは、Orca 自身を Orca で並列開発している（`orca.yaml` の `scripts.setup` が自分用・AGENTS.md に worktree の注意書き）ことの帰結と読める。

### Gaps
- star の日次推移を GitHub の stargazers API で直接検証しようとしたが 404 で取れなかった。上の時系列は第三者記事の寄せ集めで、7 月 11.7k → 8 月中旬 43k の急伸は未検証。
- 資金調達額・収益モデルの公式説明は見つからなかった。
- テレメトリ文書は "Orca has no account system" と書くが、設定文書には "Orca account — sign in (same account family as Orca Relay)" があり文書同士が食い違う（[telemetry.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/telemetry.mdx) と [settings.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/settings.mdx)）。どちらが古いかは未確認。

## 2. Core model（Task = worktree か・作り方・対応エージェント・ACP / PTY / SDK）

### Takeaway
単位は **worktree（UI 上の呼び名は workspace）= 1 branch = 自前のタブ/ペイン配置**で、その中に「エージェントセッション = 1 CLI エージェント × 1 ターミナル」を何本でも置ける。
エージェント統合は **生の PTY（node-pty + xterm.js）** が基本で、状態は OSC タイトルとフックから取る。**ACP は使っていない**。GUI の既定の流れは「名前 → worktree → エージェントを選ぶ → プロンプトを貼る」で、1 プロンプト一発起動は CLI（`orca worktree create --agent --prompt`）側にある。

### Cited Findings
- "Orca is worktree-native. Instead of branching and stashing on one checkout, every task gets its own on-disk copy of the repo via `git worktree`." 各 repo に base ref、各 worktree に start-from ref・専用ブランチ・専用のエージェントターミナル。一生は Create → Work → Review → Ship → Archive or delete。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- 作成ダイアログ: タスク名（空なら海の生き物の名前）・start-from（base ref / ローカルブランチ / SHA / リモートブランチ）・GitHub PR / Linear / Jira / GitLab MR のリンク・Project と Run on（実行ホスト）のコンボ・Agent 選択（Blank Terminal 含む・"Set as default"）・Advanced に Branch name と Parent workspace。ブランチ名は既定で workspace 名やリンクした issue から導出。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx) / [docs/first-session.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/first-session.mdx)
- 作成は非同期: "Submitting the Create Worktree dialog closes it immediately — the `git fetch` and `git worktree add` work continues in the background while you keep using Orca." サイドバーに進捗行、タブに setup 状況、キャンセルと Retry あり。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- GUI の標準手順は「worktree を作る → ターミナルの agent combobox でエージェントを選ぶ → プロンプトを貼る」。README の "Fan one prompt across five agents" も、レシピ上は worktree を 3 つ作って "Paste the same prompt into all three." と手作業。— [docs/first-session.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/first-session.mdx) / [docs/recipes/parallel-agents.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/recipes/parallel-agents.mdx)
- 一発起動は CLI にある: `orca worktree create --name child-task --agent codex --prompt "Investigate the flaky login test" --json`。`--setup run|skip|inherit`。Orca 管理下の worktree の中から作ると子 worktree として記録される（`--parent-worktree active` / `--no-parent`）。— [docs/cli/reference.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/reference.mdx)
- タスクの入口は他に GitHub issue / PR・GitHub Projects（サイドバーの Tasks）・Linear・Jira・GitLab。issue から作るとタスク名と issue リンクが前もって入った composer が開く。— [docs/review/github.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/github.mdx)
- エージェント: "Orca works with **any CLI agent** — the agent combobox just launches a process in a terminal." 組み込みは 35 種前後。Deep integration（usage・hot-swap・hooks）は Claude Code / Codex / Cursor CLI。hooks + status は Pi / OMP / Prime Agent / Antigravity / Droid、status のみは OpenCode / Ante / Command Code。Gemini・Amp・Goose・Aider・Cline・Kiro・Kimi・Qwen Code・Devin などは "Auto-setup"。Claude Agent Teams は既定オフで `orca claude-teams` 経由（teammate ごとにネイティブのペイン）。— [docs/agents/supported.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/supported.mdx)
- セッションの定義: "An **agent session** is one CLI agent running in one terminal in one worktree." — [docs/model/agents-sessions.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/agents-sessions.mdx)
- ACP 不使用: リポジトリのコード検索で "agent-client-protocol" / "agentclientprotocol" / "Agent Client Protocol" は 0 件（2026-09-20）。— [GitHub code search](https://github.com/search?q=repo%3Astablyai%2Forca+%22Agent+Client+Protocol%22&type=code)
- チャット風 UI は PTY の上の「見え方」: "The terminal remains the source of truth; Chat UI is a structured transcript + composer for the same PTY."（Experimental・Claude / Codex / Grok / OMP の transcript を解読）。Claude の AskUserQuestion はカードとして描く。— [docs/agents/native-chat.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/native-chat.mdx)
- SDK 経路も育っている: `src/main/claude/claude-agent-sdk-process-spawn.ts`・`claude-stream-json-connection.ts`・`claude-structured-permission-mode.ts`・`claude-permission-presentation.ts` など "claude-agent-sdk" を含むファイルが 32 件。— [GitHub code search](https://github.com/search?q=repo%3Astablyai%2Forca+%22claude-agent-sdk%22+path%3Asrc&type=code)
- 権限の既定は全バイパス: "Orca launches every supported agent with its full-autonomy permission flag pre-applied — Claude with `--dangerously-skip-permissions`, Codex with `--dangerously-bypass-approvals-and-sandbox`, Gemini with `--yolo` … The intent is that the worktree itself is the sandbox"。一方、対応表のページは "A worktree is an isolated checkout, not a security sandbox" と注意し、Settings → Agents → Agent Permissions で Yolo / Manual を切り替えられる。— [docs/model/agents-sessions.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/agents-sessions.mdx) / [docs/agents/supported.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/supported.mdx)
- PTY の持ち主は常駐デーモン: "A background daemon owns the PTYs, so closing the app window doesn't kill Claude Code, Codex, or any other agent CLI mid-task." Cmd-Q・自動更新・アプリのクラッシュを越えて走り続け、次回起動で warm reattach。再起動や電源断では消える。— [docs/model/session-restore.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/session-restore.mdx)
- Hibernation（Experimental・既定オフ）: done のまま 30 分触られない背景エージェントの端末を止め、開き直した時に `claude --resume <id>` / `codex resume <id>` で戻す。orchestration の Dispatch が未決の間や subagent が残っている間は眠らせない。手動の Sleep / "Sleep with Descendants" もある。— [docs/agents/hibernation.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/hibernation.mdx)
- 派生概念: 親子 worktree（サイドバーでの入れ子だけで Git 履歴は変えない）、folder workspace（Git 無しでも可）、複数 repo の project group、Orca 外で作った worktree（Claude Code worktrees / GSD worktrees）は "hidden worktrees" カードから表示に出す。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)

### Inferences
- Orca の「Task」は台帳上の実体ではなく worktree の表示名 + 任意の workspace status（todo / in-progress / in-review / completed）にすぎない。necoder の `TaskPhase` 11 値のような phase 機械は無く、状態は「エージェントの活動」と「PR の状態」の 2 系統から読む。台帳としての Task は orchestration 層（§6）にだけ存在する。
- 35 種に広げられたのは PTY 方式だから。代償として状態検出と「采配役を起こす」部分が脆い（§4・§6）。ACP を軸にする necoder と正反対の賭けで、Orca 側も Claude については SDK の構造化セッションへ寄せ始めている（用途は Chat UI と構造化セッションと推定）。

### Gaps
- `@anthropic-ai/claude-agent-sdk` がどの機能（Native Chat か orchestration の構造化セッションか）で有効なのか、ファイル名以上は読んでいない。
- 1 worktree に複数エージェントを置いた時の衝突対策（同じファイルを踏む問題）についての記述は見つからなかった。

## 3. Screen structure（サイドバー・メイン・同時に見える数・分割・キー）

### Takeaway
3 カラム: **左 = ナビ + worktree カード一覧（状態・repo/branch・エージェントごとの最新発話と経過）／中央 = その worktree 専用のタブ・ペイン木（terminal / editor / browser / diff / PR を自由分割）／右 = ファイル・検索・Source Control・Checks**。下端の status bar にプロバイダの使用率。
中央は「選んだ worktree 1 つ」が基本で、worktree を切り替えるとペイン木ごと入れ替わる。他 worktree を並べるのは明示操作（タブを端へドラッグ / Cmd-J で Shift-Enter）で、枚数の上限やグリッドは無い。全体俯瞰は別面（Agent Dashboard のカンバン・Agents フィード）で持つ。

### Cited Findings
- README のヒーロー画像（端末に `orca@1.4.64-rc.0` が写る）で確認できる構成。左サイドバー: 上に `Tasks` / `Automations` / `Orca Mobile` / `Search`、次に `Workspaces` 見出し（フィルタ・フォルダ追加・`+`）、`Pinned 3`（`.claude` Folder / `main` primary / `master` primary）、`In progress 17` のグループ。カードは 1 行目 = 状態ドット + タイトル、2 行目 = repo チップ `orca` + ブランチ `Jinwoo-H/improve-readme` + issue/PR アイコン、3 行目 = エージェント行（状態グリフ + エージェントのアイコン + 最新メッセージの抜粋 + 経過。例 "continue - Done. PR #5225 is green and me… 4h"）。選択中のカードには複数エージェントのアイコン列。中央: 左ペインに Claude Code の TUI（右上に `▶ dev` の実行ボタン）、右ペインはタブ `Analyze an…` / `improve-re…`（Codex）/ `Terminal 3` / `grok` で上下に分割（上 Codex・下シェル）。右サイドバー: Explorer / Source Control / Checks 系のアイコンとファイルツリー。status bar: Claude "78% 5h · 94% wk"、Codex "100% 5h · 80% wk"。— [README hero image](https://github.com/stablyai/orca/blob/main/docs/assets/readme-hero.jpg)
- サイドバーの既定は project ごとのグループ。専用のフィルタ入力、Hosts / projects の Show、隠すトグル（Sleeping / Default branch / Automation-created / CLI-created / Other-client / Detached HEAD）。worktree は project の先頭にピンできる。右クリックで archive / sleep / delete、Cmd/Shift クリックで複数選択、ダブルクリックで改名。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- 未読の見せ方: "The status bar shows agent activity inline; unread worktrees are bolded rather than badged." — [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- タブとペイン: "Each tab holds one thing: a terminal, an editor buffer, a browser, a diff, a PR." タブを右端へドラッグで左右分割、下端で上下分割、入れ子可。"You can have an agent terminal on the left, a diff view on the top-right, and a browser tab on the bottom-right — all at once." 境界位置は worktree ごとに保存。"Each worktree owns its own tab layout. Switching worktrees swaps the entire pane tree"。— [docs/model/tabs-panes-splits.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/tabs-panes-splits.mdx)
- 複数 worktree を同時に見る方法: "Drag a worktree's tab to the right or bottom edge of a pane to split. You can watch all three agents at once."、および Cmd-J で "Shift-Enter on a worktree opens it in a new split instead of swapping the current pane."。README は端末を "infinite splits" と表現。— [docs/first-session.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/first-session.mdx) / [docs/model/quick-open.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/quick-open.mdx)
- Agent Dashboard（Settings → Experimental）: worktree 横断のカンバン。列は Needs You / Working / Done / Idle（約 30 分無報告・既定は非表示）。カード = ヘッダ（エージェントのアイコン・会話名・状態グリフ）/ プレビュー（最後の発話か task summary）/ フッタ（project・worktree 名・経過・レビュー状態）。"**Needs You** cards tint amber …; **Done** cards tint green; other states stay neutral so tint means “look here”"。ウィンドウ内表示と pop-out。検索と Project / Workspace status / PR 状態のフィルタ。Codex/Claude の subagent は親の下に展開。— [docs/model/agents-sessions.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/agents-sessions.mdx)
- Agents フィード（既定オン）: 全 worktree のイベント（ターン完了・ブロック中の質問・worktree 作成・応答の抜粋）をスレッド化。状態でグループ化し稼働中を先頭に固定、未読バッジ付き、クリックで該当の worktree とペインへ。changelog では 2026-09-04（1.4.197）に "Graduated Agents activity view"、2026-08-07（1.4.176）に Dashboard の map view（experimental）。— [docs/activity.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/activity.mdx) / [changelog](https://www.onorca.dev/changelog)
- キー: `Cmd-J` = Worktree Jump Palette（空クエリで直近 6 セッションを needs-you → done → idle の順に並べ `Cmd-1`〜`Cmd-6` で飛ぶ。一致が無ければ入力文字列で "Create worktree"）、`Cmd-P` = worktree 内ファイル検索、`Cmd+Shift+]` / `[` = タブ移動、`Ctrl+Tab` = 直前のタブ、`Cmd-\` / `Cmd-Shift-\` = 端末を右 / 下に分割、`Cmd-Shift-Backspace` = worktree 削除（確認あり）、`Cmd-Enter` = Commit、diff 上の `c` = 行コメント、`F7` = 次の変更。Toggle Agent Dashboard / Send Review Notes to Agent / Toggle Sleeping Workspaces / Toggle Workspace Board は既定で未割当。上書きは `~/.orca/keybindings.json`。— [docs/model/quick-open.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/quick-open.mdx) / [docs/model/tabs-panes-splits.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/tabs-panes-splits.mdx) / [docs/terminal.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/terminal.mdx) / [docs/settings.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/settings.mdx)
- ブラウザ: worktree ごとの実 Chromium（Electron の webview）。Design Mode は要素クリックで HTML・CSS・切り抜きスクショをプロンプトへ送る。プロファイル分離・端末プリセット（`orca set device --name "iPhone 12"`）。iOS Simulator / Android エミュレータのブリッジも CLI から操作。— [README](https://github.com/stablyai/orca/blob/main/README.md) / [docs/cli/overview.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/overview.mdx)
- 作者自身の差別化説明（対 cmux）: "I like my tabs top-level and not inside panes"、"We also do AI status indicator even for non-focused terminal panes"、"You can also manually mark a worktree as 'unread' (kind of like a star in Gmail)"。— [HN 47549379](https://news.ycombinator.com/item?id=47549379)

### Inferences
- necoder の Fleet v2（左 = Task 一覧 / 中央 = 1 枚 / 並べるのは明示で 3 まで）は Orca と同じ骨格。違いは、Orca の中央が自由なペイン木（IDE 的）で、necoder は「会話 | サイドペイン 1 つ」に固定している点。Orca は自由度の代償として worktree ごとにレイアウトを覚えさせている。
- Orca は俯瞰を「別の面」（Dashboard・Agents フィード・Cmd-J の直近 6 件）へ逃がしている。necoder は要対応をサイドバー常設にしたので、面の切り替え無しで裁ける分だけ短い。
- ヒーロー画像のサイドバーは `In progress 17` という workspace status のグループになっており、文書の「既定は project ごと」と合わない。グルーピングの切り替えがあると推定（文書では未確認）。

### Gaps
- サイドバーのグルーピング切り替え（project / status）の仕様は文書から特定できなかった。
- Workspace Board（`workspace.openBoard`）の画面仕様は設定文書に名前が出るだけで、詳細ページが無い。
- YouTube のデモ動画は視聴していない（daily.dev に紹介記事があることだけ確認）。

## 4. State and attention（状態・検出方法・通知・未読・入力待ち・権限プロンプト）

### Takeaway
状態は 5 種のグリフ（spinner = working / 琥珀の ? = waiting on you / 緑 = done / 赤 = blocked・interrupted・failed / 灰 = idle）で、タブ・サイドバー・Dashboard・Cmd-J に同じ絵を使う。
検出は **OSC タイトル + Orca が各エージェントに仕込むフック**。通知は「working → idle の遷移」で OS 通知 + 音 + worktree のチップ、ヘッダのベルと Dock バッジで未読を数え、右クリックで「未読に戻す」。
権限プロンプトは既定でバイパスするので「承認待ちの列」という発想は薄く、"Needs You" はほぼ「質問された / 入力待ち」を指す。

### Cited Findings
- グリフ: "**Spinner** — working / **Amber question mark** — waiting on you (permission / needs input); sidebar “Needs You” counts use the same glyph / **Emerald check** (dashboard) or **emerald dot** (sidebar) — done / quiet active / **Red dot** — blocked, interrupted, or failed / **Gray dot** — idle / **No indicator** — plain shell"。— [docs/model/agents-sessions.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/agents-sessions.mdx)
- 検出: "State is detected from the terminal's OSC title sequence and agent hooks, which Claude Code, Codex, and several other agents emit." Claude Code は "a status-line hook that emits OSC title events Orca uses for state dots" 付きで起動される。— [docs/model/agents-sessions.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/agents-sessions.mdx) / [docs/agents/claude-code.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/claude-code.mdx)
- Orca 管理のフック: Settings → Agents → Agent status hooks（`orca agent hooks status|on|off`）が working / waiting / done を UI に報告する。エンドポイントは `{userData}/agent-hooks/endpoint.env` に書き、フック実行のたびに読み直すので、アプリ再起動を跨いだ長寿命セッションも報告を続けられる。— [docs/agents/hooks-memory.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/hooks-memory.mdx)
- 利用者の実測（v1.4.200・macOS）: Orca は Claude Code の `PermissionRequest` / `PreToolUse` / `Stop` / `UserPromptSubmit` にフックを入れ、`~/.orca/agent-hooks/claude-hook.sh` が payload を `http://127.0.0.1:${ORCA_AGENT_HOOK_PORT}/hook/claude` に POST する。— [issue #15190 の 2026-09-11 コメント](https://github.com/stablyai/orca/issues/15190)
- idle 判定は誤検出と戦っている: Codex の起動バナーが `tui-idle` を通過する（#18019）、Claude の起動が遅いと tui-idle が誤って立ち dispatch を失う（#9976・v1.4.149）。対策として "tui-idle evidence ranked: a name-only title needs quiescence, the agent's own idle report vetoes"（#20155・v1.4.203）、"a navigation Escape never retires a turn; only the provider hook does"（#20149・v1.4.203）。— [issue #15190](https://github.com/stablyai/orca/issues/15190) / [issue #9976](https://github.com/stablyai/orca/issues/9976)
- 通知: "When an agent transitions from working to idle, Orca fires a notification — system notification, sound, and a chip on the worktree." ヘッダのベルは全 worktree の未読を集約し、クリックで該当の worktree とペインへ。macOS は Dock アイコンに同じ件数。右クリックで mark unread。カテゴリ別の on/off と独自の通知音（MP3 / WAV など）と音量。— [docs/notifications.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/notifications.mdx)
- 入力待ちの昇格: Agents フィードは "An agent that's been waiting on input long enough to surface as blocking." を載せる。— [docs/activity.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/activity.mdx)
- 質問への応答: Chat UI は Claude の AskUserQuestion を transcript 内のカード（Submit 付き）で描く。モバイルからは `continue` / `yes` / 自由文・写真・音声で返せる。— [docs/agents/native-chat.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/native-chat.mdx) / [docs/mobile.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/mobile.mdx)
- エージェントの自己申告: 各 worktree に自由文の comment 欄があり、`orca worktree set --worktree active --comment "…" --workspace-status in-progress` でエージェント自身が更新する。"it's the pattern we recommend for keeping human collaborators in the loop without forcing chat"。上書き前に `orca worktree current --json` で読むよう指示している。— [docs/cli/worktree-checkpoints.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/worktree-checkpoints.mdx)
- 外部シグナル: GitHub Actions の失敗は worktree に赤いチップ、PR の状態（open / draft / merged / closed）はサイドバーと Dashboard のフィルタに出る。— [docs/review/github.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/github.mdx)
- 通知の騒音: orchestration が起動した worker が通知を上げ続けるので止める設定が欲しい、という要望（#15148）。Codex の自動レビュー済み承認が `Needs You` に見える不具合は #21389 で修正（未リリース）。— [issue #15148](https://github.com/stablyai/orca/issues/15148) / [issue #15190](https://github.com/stablyai/orca/issues/15190)

### Inferences
- Orca は色相で状態を語る（琥珀 / 緑 / 赤 / 灰）。necoder は「状態は形と動き・色は識別」で逆の流儀だが、Dashboard の "tint means look here"（注意を要する 2 状態だけに色面を使う）は、色を使わない necoder でも「要対応だけ目立たせ、他は中立」という規律としてそのまま使える。
- 「未読 = 太字」と「手動で未読に戻す」は安い割に効く。necoder の「完了・未確認 → 確認」に、逆向き（確認済み → 未確認に戻す）を足す価値がある。
- 既定 YOLO は承認キューの UI コストを丸ごと消す代わりに安全性を worktree 隔離へ丸投げしている（公式文書自身が "not a security sandbox" と認める）。necoder の要対応 + Captain の推薦はこの穴を埋める差別化点になるが、摩擦として嫌われる危険もある。

### Gaps
- Manual 権限時の承認プロンプトを Orca の UI からワンクリックで許可/拒否できるか（TUI へのキー入力以外の経路）は、Claude の構造化 permission 関連ファイルの存在以外に確認できなかった。
- フックを持たないエージェント（Cursor など）の状態精度についての定量的な記述は無い（issue 上の不具合報告のみ）。

## 5. Review and integration（diff・エージェントへのコメント・PR・merge・衝突・片付け・準備スクリプト）

### Takeaway
レビューの核は **Annotate AI Diff（diff の行にコメントを貯め、1 通にまとめてエージェントへ送り返す）**。統合は **ローカルの merge ではなく PR 経由**（commit → push → Create PR → checks → merge / auto-merge / merge queue / stacked PR まで全部アプリ内）。
並走 Task 間の衝突予測は無く、あるのは PR の conflict 表示・3-way の解消 UI・"Resolve with AI" と、orchestration の「base から 20 コミット超遅れた worktree には dispatch しない」ガードだけ。未追跡ファイル問題には宣言的な仕組みが 3 つとセットアップフックがある。

### Cited Findings
- Diff viewer: start-from ref との差分（staged + unstaged + untracked の combined diff）。比較先は任意の commit / branch / base ref に切替可。画像 diff（side-by-side / swipe / onion-skin）、HTML プレビュー、3-way の merge-conflict UI、ファイルツリー。— [docs/review/diff-viewer.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/diff-viewer.mdx)
- Annotate AI Diff: 行ホバーの `+`（または `c`）→ markdown で記入 → `Cmd-Enter`。"Comments pin to the exact line; Orca tracks them across edits"。**Send to agent** で全コメントを行アンカー付きの 1 プロンプトに合成し、"Send notes to" メニューでその worktree のエージェントを選ぶ（新規起動も可）。Resolve で畳み、未解決は次のバッチに再同梱。— [docs/review/annotate-ai-diff.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/annotate-ai-diff.mdx)
- バッチにする理由: "Sending comments one at a time causes the agent to swing back and forth. Batching keeps the feedback coherent: one round of thinking, one revision pass, and a much higher hit rate." — [docs/review/annotate-ai-diff.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/annotate-ai-diff.mdx)
- Attribution: エージェントが書いた行範囲を記録し gutter に印を出す。人が上書きすると human に戻る。ローカル限定で git には入らない。— [docs/review/attribution.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/attribution.mdx)
- Commit / Push / PR: Source Control パネルで stage → commit（"Generate with AI" で文面を生成）→ push（黙って force しない。履歴を書き換えた時だけ **Force push with lease** を別ボタンで出す）→ hosted review 作成（GitHub / GitLab / Bitbucket Cloud。Azure DevOps と Gitea の PR も表示）。pre-commit フックの失敗は **Fix with AI**（"The agent gets a repair prompt only — it is not asked to bypass hooks, commit, push, or open a review."）。— [docs/review/commit-push.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/commit-push.mdx)
- 主操作ボタンが状態で変わる: "The panel's primary button at the bottom shifts with state so the next useful action is always one click away: **Stage Files** … then **Commit**, then **Push** / **Pull** / **Sync**"。— [docs/review/commit-push.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/commit-push.mdx)
- ブランチ文脈行: `branch → base` を常時表示し、merge base からの `+N −M` チップ（ホバーで Source / Tests / Generated の内訳）。— [docs/review/commit-push.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/commit-push.mdx)
- PR ビュー: checks・reviews・comments をアプリ内で開き、スレッドへの返信とリアクション可。失敗した checks は **Fix broken checks** でエージェントへ渡す。merge ボタンに **Enable auto-merge**、merge queue なら "Merge when ready"。GitHub の stacked PR は "Stack this PR above #N" で作成、スタック一括の "Merge through #N · M PRs" は "Atomic stack merge fails closed"。— [docs/review/github.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/github.mdx)
- 衝突: merge / rebase / cherry-pick / revert が conflict を残すと **Resolve with AI** と **Review conflicts** が並び、Abort merge / Abort rebase もある。— [docs/review/commit-push.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/commit-push.mdx)
- orchestration の stale-base ガード: `DISPATCH_STALE_THRESHOLD = 20`。worktree が base から 20 コミットを超えて遅れていると dispatch を `stale-base-refused` で見送り、spec に `allow-stale-base: true` と書けば通す。コメントに "20 lets normal monorepo day-velocity pass but trips the 168-commit harm from ORCHESTRATOR_FEEDBACK.md"。— [coordinator-stale-base-flag.ts](https://github.com/stablyai/orca/blob/main/src/main/runtime/orchestration/coordinator-stale-base-flag.ts) / [coordinator-task-dispatch.ts](https://github.com/stablyai/orca/blob/main/src/main/runtime/orchestration/coordinator-task-dispatch.ts)
- 「勝者を選ぶ」レシピ: 同じプロンプトを 3 worktree × 3 エージェントに投げ、diff を見比べ、勝者から commit → push → PR、"Delete the two losers — one click removes the worktree and branch."。"Where three agents agree, the answer is probably right. Where they split, you've found the hard part." — [docs/recipes/parallel-agents.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/recipes/parallel-agents.mdx)
- 片付け: 削除はディレクトリとブランチを確認付きで消す。未マージの疑いで git がブランチを残した場合は **Review N Branches** のトーストから選んで強制削除。右クリックに archive / sleep / delete、子を持つ worktree には "Delete with Descendants…"。**Resource Manager → Clean up workspaces** で状態・最終活動・サイズ・Git 状態・リンク済みレビューを見てから一括削除。CLI で `git worktree remove` しても次の更新で追従。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- 運用の勧め: "Delete merged worktrees aggressively. … Leaving dozens of merged worktrees around just slows down the palette." — [docs/recipes/jump-worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/recipes/jump-worktrees.mdx)
- 未追跡ファイルは 3 つの宣言的な仕組み: (1) Settings の **Worktree Shared Paths**（macOS は可能なら APFS の clone-copy、だめなら symlink）、(2) `orca.yaml` の `worktree.sharedDirectories`（`node_modules` や `.cache` を symlink 共有。gitignore 済みのディレクトリのみ）、(3) repo 直下の **`.worktreeinclude`**（`.env` などを各 worktree へコピー。リテラルパスのみで glob と否定は警告付きで無視）。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- セットアップフック: Settings → Repository → Hooks に worktree 作成後のコマンド（"`pnpm install`, `direnv allow`, or a script that restores `.env` files"）。repo に置く形は `orca.yaml` の `scripts.setup`（Orca 自身は `node config/scripts/run-internal-dev-setup.mjs` と `pnpm install`）。CLI は `--setup run|skip|inherit`。— [docs/agents/hooks-memory.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/hooks-memory.mdx) / [orca.yaml](https://github.com/stablyai/orca/blob/main/orca.yaml) / [docs/cli/reference.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/reference.mdx)
- Windows の罠: setup の実行体はスクリプトが `#!` で始まらない限り `.cmd`。Git Bash のペインから `cmd.exe /c` で起動してはいけない（MSYS が `/c` を書き換える）。— [AGENTS.md](https://github.com/stablyai/orca/blob/main/AGENTS.md)
- 利用者の声: "What do you when done with a work tree? Just delete? I wish there was a middle ground of hiding it from the main panel, but keeping it for a week or so" → 別の利用者の答えは sleep + "hide sleep" フィルタ。— [HN 49234738](https://news.ycombinator.com/item?id=49234738) / [HN 49237309](https://news.ycombinator.com/item?id=49237309)

### Inferences
- Orca には necoder の「Conflict Radar → 人間の Integrate」に当たるローカル統合 gate が無い。統合の正は hosted PR で、衝突は PR の状態か merge 後に判明する。並走 Task 同士が同じファイルを踏むことの事前警告は、necoder 側の明確な優位点として残る。
- README の "merge the winner" は機能名ではなく手順の表現（実体は PR の merge ボタン）。
- 既定 YOLO なので、エージェントは worktree の中で `gh pr merge` も実行できる。Orca の「merge は人間」は UI の導線でしかなく、強制された gate ではない。necoder の「Captain は integrate できない」は権限として強制している点が違う。
- `.worktreeinclude` + `sharedDirectories` は「スクリプトを書かなくても 8 割が済む」宣言的な層。necoder の `.necoder/worktree-setup.sh` 1 本構えより入口が低い。

### Gaps
- 複数 worktree の変更を横断して衝突を予測する機能は、docs と issue 検索の範囲では見つからなかった（無いことの証明ではない）。
- "archive" が delete とどう違うか（ディレクトリを残すか）は文書から特定できなかった。"Archive or delete — one click removes the worktree and branch." と 1 行で括られている。

## 6. Orchestration（采配役・分解・依存・キュー・エージェント間メッセージ・Orca 自身を操作する面）

### Takeaway
**ある。** 2026-05 に Experimental として入った "Orchestration" 層で、**Run（名前空間 + coordinator の受信箱）/ Task（spec・依存・状態）/ Dispatch（1 回の試行）/ Message / Decision gate** を Orca ランタイムが永続化する（第三者の検証記事によれば SQLite。[note Komugi](https://note.com/quick_pipit7468/n/n78bdc07a9605)）。
采配役は組み込みの特別なエージェントではなく、**`orchestration` スキルを読んだ任意の CLI エージェントが `orca orchestration …` を叩く**形。初期の組み込みスケジューラ（`orchestration run` / `coordinator-start`）は廃止されて no-op になっている。
最大の未解決問題は **idle な coordinator の起こし方**。現状は coordinator の PTY に「メールボックスへのポインタ」をタイプして Enter を押す方式で、人間の下書きに混ざる・Claude の AskUserQuestion に誤って答える・定期 heartbeat がターンを浪費する、が open のまま。

### Cited Findings
- 定義: "Orchestration is Orca's structured multi-agent layer: a **Run** (namespace + coordinator inbox), **Tasks**, **Dispatches**, supervised **workers**, messages, and decision gates." Settings → Experimental で有効化。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- モデル: "**Run** — durable namespace and home inbox. Never schedules or places workers." / Task の状態は `pending` `ready` `dispatched` `completed` `failed` `blocked` / "**Dispatch** — one attempt of a task on a terminal; lifecycle authority for `worker_done` / heartbeat." / Message の型は `status` `dispatch` `worker_done` `escalation` `question` `heartbeat` / "**Decision gate** — a coordinator-owned question that blocks a task until it is resolved."。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- 導入時期: コントリビュータ Jinwoo-H が 2026-05-05 に "We've added experimental orchestration capabilities to our Orca CLI, feel free to install the skill and try it out! (in Settings -> Experimental -> Agent Orchestration)"。元は 2026-04-15 の利用者提案（Augment Intent 型の coordinator → specialist）。— [Discussion #681](https://github.com/stablyai/orca/discussions/681)
- 旧スケジューラは廃止: "`orca orchestration run` and `run-stop` (and `coordinator-start` / `coordinator-stop`) perform **no effects**. They return recovery text pointing at `orca skills get orchestration --full`."。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- 役割はスキルが文脈から決める: ユーザーが監督・監視・DAG の采配を明示した時だけ Coordinator、Task/Dispatch ID 入りの preamble が注入されていれば Dispatched worker、単なる引き渡しなら Handoff owner（"create no Run, Task, or Dispatch and do not monitor completion"）。"Never substitute a non-Orca subagent tool when Orca orchestration provenance was requested."。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md)
- 標準ループ: `run-create --objective` → `worker-start --spec "<task>" --worktree current|new-child|new-top-level --agent codex [--model --effort] [--setup run]`（独立な波を先に全部起動）→ `check --wait --types "worker_done,escalation,question" --timeout-ms 900000` → 各メッセージを処理（`reply --id`）→ `worker-release` / `worker-retain` / 同じ端末で次の Dispatch → `check --ack <deliveryId> --wait …`。依存付きは `task-create --spec --deps <json>` と `task-list --ready --brief`。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md) / [references/coordinator-loop.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration/references/coordinator-loop.md)
- 受信箱は FIFO の at-least-once: `check` は束縛中の Run の最古の Delivery（最大 50 通）を返し、ack するまで同じ束を再送する。`--peek` / `--all` は読み取り専用。"Type filters decide when a waiter wakes; they do not authorize skipping older actionable mail."。— [references/messaging-and-gates.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration/references/messaging-and-gates.md)
- worker の契約（preamble で注入）: `worker_done` は失敗時も含め必ず 1 回だけ・`--outcome succeeded|failed` 必須（"Never encode failure only in prose"）・body は 3 文（何を変えた / 何が分かった / 何が残った）・`--task-id` と `--dispatch-id` の両方（"so stale retries cannot complete the wrong dispatch"）・`--files-modified` は実値のみ。長時間作業は heartbeat。質問はローカルの TUI ではなく `orca orchestration ask --question … --options "a,b" --timeout-ms 600000`（タイムアウトしても質問は残り `--resume <message_id>` で再開）。節目ごとに `check --terminal <handle>` で coordinator の追伸を読む。`consumer_fenced` が返ったら自分は交代済み。— [references/worker-contract.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration/references/worker-contract.md)
- Task spec の契約: Target / Change / Constraints / Ownership / Observable acceptance の 5 項目を自己完結で書く。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md)
- 安全側の原則: "Only positive proof of exit authorizes stop, abandon, or retry, and only an accepted settlement authorizes release. Every other observation, absence included, is a checkpoint."、liveness は `live` / `unverifiable` / `exited` の 3 値で "contact loss is not process death"、"A heartbeat proves liveness, not completion."、"A successful `orchestration send` proves durable enqueue; its wake or nudge is best-effort attention only"。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md)
- DAG の助言: "Use dependencies only for real ordering and prefer parallel waves over chains deeper than three or four steps"。worker の入れ子には深さ制限（`nested_worker_depth_exceeded`。Run を作り直しても深さは戻らない）。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md)
- レビューの所有権: "A review-only `worker_done` authorizes synthesis of findings, not coordinator file edits. Dispatch or hand off fixes unless the user explicitly assigned them to the coordinator."。— [references/coordinator-loop.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration/references/coordinator-loop.md)
- 完了の会計: 受理した報告ごとに「同じ端末を次の Dispatch で再利用 / `worker-retain` / `worker-release`」のどれか 1 つを必ず行う。`worker-list --terminal-state reclaimable` が空になるまで coordinator のターンを終えない。`worker-release` は出力をアーカイブしてから端末を閉じ、後から `worker-read` で読める。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md)
- メッセージ宛先: `dispatch:<id>`・`run:<id>`・グループ `@all` `@idle` `@claude` `@codex` `@opencode` `@gemini` `@droid` `@grok` `@cursor` `@worktree:<id>`。グループは送り主の Run 内の生きた Dispatch に限る。`worker_done` と heartbeat はグループに送れない。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- Decision gate: `gate-create --task <id> --question "Merge the shared button change into the task branch?" --options '["yes","no"]'` → `gate-resolve --id <gateId> --resolution "yes"`。worker の `ask` への回答に gate を使ってはいけない。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- 遠隔 worker（federation）: `worker-start --task <id> --on windows --worktree new-top-level --repo … --agent codex`。以後は Dispatch ID で宛先を決め `--on` は繰り返さない。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- UI 側: 端末に出る `task_…` はクリック可能で担当端末へフォーカス（SSH / リモートでも）。子 worktree はサイドバーで入れ子になり、Claude/Codex の subagent は親の下に展開される。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx) / [docs/agents/claude-code.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/agents/claude-code.mdx)
- 起こし方の実装と未解決点: 実装は `mailbox-pointer-delivery.ts` ほか（コメントに "commits to typing the pointer into the pane"、Enter を遅延して送る）。追跡 issue の open 項目は "Routine heartbeats wake the coordinator (#14910)"、"nothing wakes an idle coordinator with ready work (#15185)"、"pointer can land in a human draft (#14832, #11242, #18478)"、"pointer plus Enter answers a Claude `AskUserQuestion` because delivery reads only the title-derived idle, never the hook's waiting state (#21745)"。提案中の決定 D3 は "A recorded heartbeat is state, not wake mail"。2026-09-11 には Claude の `Stop` フックを起こす経路に使う案が出ている（PTY に打たない）。— [mailbox-pointer-delivery.ts](https://github.com/stablyai/orca/blob/main/src/main/runtime/orchestration/mailbox-pointer-delivery.ts) / [issue #15190](https://github.com/stablyai/orca/issues/15190)
- 同 issue の目標文: "An idle turn, a process exit and a successful Task are three different facts. Loss of contact is `unverifiable`, never a death, and never permission to start a second editor."。— [issue #15190](https://github.com/stablyai/orca/issues/15190)
- 利用者の不満（2026-06-01）: "the orchestrator can initialize and fire off prompts, but they dont continuously monitor workers nor get notified on their status. As a result, I'm having to prompt the orch to poll worker status every x epoch"。2026-09-02 の追記（v1.4.194・Kimi Code を coordinator、VPS 上の Codex を worker）は、ポーリング 25 分間隔がそのまま死に時間になること、CLI からの federation 操作が "is not available to mobile clients" で拒否されることを報告。— [issue #4376](https://github.com/stablyai/orca/issues/4376)
- UI の要望: サイドバーで orchestrator の下に worker を入れ子にする（部分的に入ったが不整合）、"should have a preference to hide worker agents from the main render terminal tabs … with orchestrations, I mostly care about interacting with the orchestrator."（#4374）。他に worker を coordinator の分割ペインで開く（#17570）、release 後も子 worktree が残る（#18411）、決着済み worker の自動回収が無い（#19388）、Claude Code が明示の `/orchestration` を自前の subagent で置き換える（#16075）。— [issue #4374](https://github.com/stablyai/orca/issues/4374) / [issue #17570](https://github.com/stablyai/orca/issues/17570) / [issue #18411](https://github.com/stablyai/orca/issues/18411) / [issue #19388](https://github.com/stablyai/orca/issues/19388) / [issue #16075](https://github.com/stablyai/orca/issues/16075)
- Orca を操作する面は MCP ではなく CLI + スキル: `npx skills add https://github.com/stablyai/orca --skill orca-cli|orchestration|computer-use|orca-linear|orca-emulator|…`。配るのは薄い stub で、本文は `orca skills get <topic> [--reference <name>|--full]` がその時のバイナリに合った版を返す（"Command flags live in the binary so they cannot drift from the app version."）。MCP は外部サーバを Settings → Integrations → MCP に登録してエージェントへ渡す方向だけ。— [docs/cli/skills.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/skills.mdx)
- 軽い連携は素の端末コマンド: `orca terminal list|read|send --text "continue" --enter|wait --for tui-idle`。"Use `orca terminal send` for a lightweight prompt to an agent you are watching."。— [docs/cli/overview.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/overview.mdx) / [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- 定期実行: `orca automations create --trigger weekdays --time 09:00 --prompt … --provider codex --repo … [--precheck "<shell>"] [--reuse-session]`（cron / RRULE・ホスト横断・missed-run grace）。— [docs/cli/automations.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/automations.mdx)
- 現場の運用例: (a) "I launch Orca and literally just type "checking in" and the CONSTITUTION.md file governs the scanning of github issues and prioritization … the Orca worktree handles the launching of sub-agents that fix things. Occasionally I weigh in with an opinion or pick a recommendation."（OpenCode の subagent + repo の規約ファイル）。(b) "an orca skill where an agent can spin up different sessions(different from subtasks because they share the context and you can chose the harness/model unlike sub agents)"。(c) Linear のラベルで repo とモデルを振り分ける Cyrus と併用し、Orca は手を動かす時だけ使う。— [HN 49742023](https://news.ycombinator.com/item?id=49742023) / [HN 48402198](https://news.ycombinator.com/item?id=48402198) / [HN 49615716](https://news.ycombinator.com/item?id=49615716)
- 第三者の体験記（2026-08-23）: coordinator に仕事を渡して席を離れ、`ask`（worker 発の保留）や gate（coordinator 発の保留）が来たらスマホの push で答える流れを紹介。"No sleep loops. No polling. The wait returns the moment something needs a human." 注意点は「コマンド名がリリースごとに変わるので `orca skills get orchestration --full` が正」「15〜60 分かかるのは普通でタイムアウトは失敗ではない」。— [Aridane Martín](https://aridanemartin.dev/blog/orca-orchestrate-agents/)
- 競合の評: Orca の協調モデルは "closer to a fleet of independent agents you supervise"。対して Conductor は名前付きの役割と 4 つの協調パターン（Supervisor / Pipeline / Consensus / Swarm）を持つ、という整理。— [Superset の比較（競合作成）](https://superset.sh/compare/orca-vs-conductor)
- Discussion に寄せられた実運用の知見（第三者・Orca 外の経験）: "The coordinator should NEVER execute tools"、依存は DAG で明示しないと merge conflict と無駄が出る、5 体で「投票」させたら全員が別解釈で実行した（"Your coordinator needs to be a dictator, not a democrat."）、95%/step なら 10 step で約 60%、「浅い並列 + 次の段の前に人間の承認 gate」に移した、coordinator は 10〜15 体で文脈の限界に当たる。2026-09-10 の投稿は worker の完了判定の失敗 4 種を挙げ、状態は finished / needs input / unknown の 3 つ、さらに FAILED（未実行・再投入可）と UNVERIFIED（実行されたかもしれない）を分けよ、と述べる。— [Discussion #681](https://github.com/stablyai/orca/discussions/681)

### Inferences
- Orca と necoder は同じ結論に達している: **采配役 = ただのエージェント + ホストが持つ永続台帳 + CLI**。Orca が一度作った組み込みスケジューラ（`coordinator-start`）を捨てて「エージェントが CLI を叩く」に寄せた事実は、Captain を「状態を持たない任命制の ACP スレッド」にした判断の追い風になる。
- 違いは起こし方。Orca は PTY に文字を打つしかないので、(1) 人間の入力と競合し、(2) idle 判定の誤りがそのまま誤配送になり、(3) 質問ダイアログに誤って答える。necoder は ACP の prompt ターンとして台帳イベントを渡せる（実行中はキューに積み 1 ターンにまとめる）ので、この種の不具合を構造的に持たない。ここが最大の技術的優位。
- Orca の采配には強制された人間 gate が無い。gate は coordinator が自分で作り自分で解決できる CLI オブジェクトで、worker は YOLO で走る。necoder の「Captain は integrate・承認応答・削除ができない」という権限表は Orca に無い設計。
- Orca の orchestration は「CLI の契約の正しさ」（capability・fence・receipt・at-least-once）に膨大な労力を割く一方、GUI の面（誰が誰の worker か・DAG・采配ログ）はほぼ手付かず（#4374 が 3 か月 open）。necoder のブリッジ（Captain の会話 | 編隊図・采配ログ）は Orca に欠けている部分そのもの。

### Gaps
- Decision gate を解決できる主体（人間だけか、coordinator 自身も可か）と、gate が GUI / モバイルのどこに出るかは一次資料で確認できなかった（Aridane の記事は「スマホに届く」と書くが、gate 専用の UI があるかは不明）。
- orchestration が SQLite ベースという点は note の検証記事の記述と `src/main/runtime/orchestration/db.ts` の存在からの推定で、スキーマは読んでいない。
- orchestration の利用率（どれだけのユーザーが有効化しているか）のデータは無い。issue の件数からは一部のヘビーユーザーが深く使っている様子が見える程度。

## 7. Remote / mobile（リモート実行・スマホからの監視）

### Takeaway
実行場所は 4 通り（ローカル / SSH target / Remote Orca Server / worktree ごとの使い捨て Cloud VM）で、どれも自前の計算資源を持ち込む。
スマホ（iOS / Android・beta）は「デスクトップが正・スマホはリモコン」という割り切りで、状態の一覧・チャット風 transcript・返信（音声 / 写真も）・Source Control の commit・workspace 作成・アカウント切替・push 通知まで備える。relay と push のサーバも同じリポジトリで公開されている。

### Cited Findings
- 4 モードの表: Local / SSH target（エージェントと `git worktree` はリモート、エディタ・diff・UI は手元）/ Remote Orca Server（デスクトップ版か `orca serve` が projects・worktrees・terminals・agent processes を持ち、laptop・web・mobile・automation が同じランタイムを共有）/ Cloud VM（Experimental。`orca.yaml` の recipe で Vercel Sandbox・Fly・Modal・SSH・Docker を起動）。— [docs/ways-to-run.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/ways-to-run.mdx)
- Remote Orca Server は beta で、経路は Tailscale か LAN を推奨。ヘッドレスは `orca-ide serve --pairing-address <tailscale-ip>`（既定ポート 6768）。エージェント CLI の認証はサーバ側で行う（`orca account add --agent claude`）。— [docs/remote-servers.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/remote-servers.mdx) / [docs/cli/reference.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/reference.mdx)
- モバイルでできること: 全ホストの worktree と状態（working / done / waiting on input）の一覧、ファイルツリー、Chat UI か生の端末、scrollback の取得、返信（`continue` / `yes` / 自由文・写真添付・音声入力）、Quick Commands、ブラウザの Web/Mobile 表示、Source Control（stage / unstage / commit・既存 PR のリンク）、アカウント切替と使用率、workspace 作成（Smart / GitHub / Linear / GitLab / Branch / Name）、push 通知。— [docs/mobile.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/mobile.mdx)
- 位置づけ: "The mobile app is intentionally not a full editor — it's a remote control for the desktop you already have running."、"**Desktop still owns the agent** — Chat UI is a view over the paired desktop session, not a separate cloud agent."。— [docs/mobile.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/mobile.mdx)
- ペアリング: デスクトップが一度きりのコードを出す → スマホで貼る（deep link も可）。Orca Relay 経由が推奨で、サインインが要るのは Relay だけ。LAN / Tailscale 直結も可。デスクトップを閉じると直結は切れる。プロトコルは版付きで、古い側はブロックされ更新を促される。— [docs/mobile.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/mobile.mdx)
- relay / push の実装は OSS: `cloud/apps/relay`、`cloud/apps/push`（APNs・FCM・Postgres の durable push store・レート制限）、`cloud/infra/terraform`。— [cloud/README.md](https://github.com/stablyai/orca/blob/main/cloud/README.md)
- changelog 2026-09-16（1.4.204）に "Restored mobile push notifications"（一時的に壊れていた）。— [changelog](https://www.onorca.dev/changelog)
- 利用者: "Exciting to get the notification on my phone."（公式サイト掲載の推薦文）、"I've been using it [Conductor] since February and only recently swapped to using Orca for Remote SSH and perf improvements."、"Orca lets me run agents either on linux box or locally on the macbook … Everything runs in a tailscale network"。— [onorca.dev](https://www.onorca.dev/) / [HN 48418811](https://news.ycombinator.com/item?id=48418811) / [HN 49742023](https://news.ycombinator.com/item?id=49742023)

### Inferences
- 「サーバがセッションの正・クライアントは UI」という Remote Orca Server の切り方は、necoder の `control_ipc` の remote_snapshot（リモート管制 P9）と同じ向き。Orca は PTY をデーモンが持つ構造のおかげで、クライアント差し替えに素直に拡張できている。
- モバイルで commit までさせるのは「外出中に 1 行直して PR」という現実の需要に応えたもの。necoder のリモート管制を作るなら「要対応を裁く + Captain に話す」に絞る方が、Captain という差別化点を活かせる。

### Gaps
- Relay の通信が E2E 暗号化かどうか、relay の利用規約と料金は確認していない。
- web クライアント（ブラウザから Remote Orca Server へ）の提供形態は文書で言及があるだけで詳細は未確認。

## 8. Reception（評価・批判・他ツールとの比較）

### Takeaway
褒められるのは **機能の密度**（Design Mode で要素を選んでエージェントへ返す・diff へのコメント返し・SSH・モバイル通知・sleep）、**エージェント非依存**、**Linux/Windows でも動く**こと。
叩かれるのは **編集体験の遅さと磨き不足**、**Electron の重さ・フリーズ・古いデーモンの滞留**、**毎日リリースで足元が動く**、**「ADE はどれも同じ」**という冷めた見方。比較相手は cmux・Superset・Conductor・herdr が中心で、Claude Squad / Vibe Kanban / Sculptor との直接比較は見つからなかった。

### Cited Findings
- 称賛（HN）: "I'm liking Orca largely because of the built in browser with ability to select an object(s), write a comment and send it back to the agent."（[49235682](https://news.ycombinator.com/item?id=49235682)）／"First tool that actually works well on Linux."（[Show HN](https://news.ycombinator.com/item?id=47549197)）／"I decided to test herdr and orca today and I'm really impressed with both, especially orca."（[49111877](https://news.ycombinator.com/item?id=49111877)）／"Another great option (not mine) is Orca and it's a lot more mature. Really nice to use."（[49174195](https://news.ycombinator.com/item?id=49174195)）／"You can "sleep" the worktree so that it shuts down the terminals / agents to not consume RAM."（[49237309](https://news.ycombinator.com/item?id=49237309)）。
- エージェント非依存の価値: "Orca lets me easily switch from Pi to Claude Code to Kilo to Codex or Hermes or whatever. … All of it lives in a single open source orchestrator to avoid any platform lock in to any AI company going forward"。— [HN 49628681](https://news.ycombinator.com/item?id=49628681)
- herdr との比較: "Onorca.dev is very similar but has a desktop UI that I prefer to herdr. I find it easier to work with. Being UI, it has markdown reader, a native browser integration, linear or github built in integration." ただし "The limitation of Orca or Herdr is that they don't have integration for multi project work."。— [HN 49615716](https://news.ycombinator.com/item?id=49615716)
- 批判（編集体験）: "I tried Orca for a few days, though I liked being able to manage git worktrees from the UI and being able to leave comments for the agent while I'm reviewing the diff, I found editing experience too slow for daily use. It's also missing some minor polish issues (vim bindings, being able to change default shell etc.) that were deal breakers for me."（2026-07-29。default shell は #5097 で後に対応）。— [HN 49103047](https://news.ycombinator.com/item?id=49103047) / [issue #5097](https://github.com/stablyai/orca/issues/5097)
- 批判（同質化）: "Even though all these "ADE"s are mostly the exact same (Superset, T3, Conductor, Paseo,random Reddit-dude,etc.)."（[49235682](https://news.ycombinator.com/item?id=49235682)）／"zed , orca , /.+mux.*/ , ... they all look incredibly / increasingly the same?"（[48237238](https://news.ycombinator.com/item?id=48237238)）／"Orca is my daily driver right now. It could be better but it's far from bad. I was using superset before that-they are nearly identical."（[49104330](https://news.ycombinator.com/item?id=49104330)）／"Pure Codex CLI play none of that Cursor Orca rubbish."（[49130724](https://news.ycombinator.com/item?id=49130724)）。
- 市場の見方: YC だけで herdr・Superset・cmux・Emdash・Orca・Bullet・Conductor が同じ領域にいる。"Being open source by itself is not a product differentiator in this market. The features required for string positioning (orchestration, cloud, custom agents, enterprise features, hosted infrastructure, etc.) often end up either closed source or available only through the hosted product."（競合 Agentastic の作者）。agentconn は懐疑論として「ADE は製品ではなく機能で、VS Code / Cursor / Windsurf に吸収され得る」を紹介。— [HN 49203107](https://news.ycombinator.com/item?id=49203107) / [agentconn](https://agentconn.com/blog/orca-ade-agent-fleet-parallel-coding-agents-2026/)
- 性能・安定性の issue: "Orca suddenly VERY slow!"（#6795・コメント 55・closed）、"Laggy clause code terminal scroll"（#6144・36）、"Orca often freezes for short periods"（#16038・24）、"App updates leave previous daemon generations running forever — invisible agent sessions accumulate and exhaust m…"（#9138・open）、"Main process reaches 100% CPU after terminal/worktree churn and retains thousands of removed worktrees"（#15412・closed）、"Orca remote server memory usage and CPU usage are too high"（#6288）、"What if we rewrite orca by Rust"（#5920・13）。— [#6795](https://github.com/stablyai/orca/issues/6795) / [#6144](https://github.com/stablyai/orca/issues/6144) / [#16038](https://github.com/stablyai/orca/issues/16038) / [#9138](https://github.com/stablyai/orca/issues/9138) / [#15412](https://github.com/stablyai/orca/issues/15412) / [#6288](https://github.com/stablyai/orca/issues/6288) / [#5920](https://github.com/stablyai/orca/issues/5920)
- 要望の上位（+1 順・2026-09-20）: Jujutsu workspaces（#1082・+37）、複数 repo の diff を同時に見る workspace（#1099・+31）、LSP（#961・+31 と #3035・+22）、埋め込み VS Code（#7754・+24）、複数ウィンドウ（#6074・+21）、ヘッドレス/サーバモードの一級化（#4280・+20）、git blame（#9948・+18）、チャット型 UI（#3099・+17）、Docker sandbox（#13665・+15）。— [issues（+1 順）](https://github.com/stablyai/orca/issues?q=is%3Aissue+sort%3Areactions-%2B1-desc)
- 競合 Nimbalyst の評（2026-08-13）: Orca は「この比較で最も機能が密」。弱点は「表面積が大きく、毎日出荷するツールは足元が動く。小さく安定したものが欲しいならこれではない」。— [Nimbalyst](https://nimbalyst.com/blog/open-source-agent-workspace-alternatives-2026/)
- 作者による対 cmux の説明（2026-03）: タブをペインの内側ではなく最上位に置く・フォーカス外のペインにも AI 状態を出す・GitHub 連携・ファイルビューア / 検索 / diff、"Also cmux is Mac only"。— [HN 47549379](https://news.ycombinator.com/item?id=47549379)
- 競合 Superset の比較表（2026-08-30 更新）: Orca = MIT・3 OS・25+ エージェント・Free・iOS と Android・SSH。Conductor = プロプライエタリ・macOS のみ・4 エージェント・Pro $50/月・クラウド workspace は有料。Superset = MCP サーバと TypeScript SDK・Slack からの dispatch・SSO/SCIM。— [Superset](https://superset.sh/compare/orca-vs-conductor)
- 日本語圏: note（2026-09-03・v1.4.194 を実機とソースで検証）は「CLI エージェントのセッションを常時 3〜4 本以上走らせる人」向きで「セッションが常に 1〜2 本の人」には学習コストが勝つ、注意点は Electron のメモリ・埋め込みブラウザでの Google ログイン制限・リリースの速さ、3 体並走の実験では「3体全員が仕込みバグを独立に発見」。Qiita（2026-06-28）は「左ペインにワークツリー一覧、右側にターミナルとエディタ」の一覧性で認知コストが下がり cmux の代替になりそう、注意点は coming soon の機能と Git 前提。Qiita（2026-08-24）は主流になった理由を「CLI経由でエージェント自身がOrcaを操作できる設計」に求める。料金解説記事は「本体は完全無料」で既定の権限バイパスを Manual に変えるよう勧める。README は中 / 日 / 韓 / 西 / 仏 / 葡の 6 言語、WeChat グループは 9 つ目。— [note Komugi](https://note.com/quick_pipit7468/n/n78bdc07a9605) / [Qiita tamakiiii](https://qiita.com/tamakiiii/items/9b22ca0b287309ebba32) / [Qiita kai_kou](https://qiita.com/kai_kou/items/0c0bb9bd38a3bf1c1084) / [data-engineering.jp](https://data-engineering.jp/orca-ide/) / [README](https://github.com/stablyai/orca/blob/main/README.md)
- 公式サイトの推薦文: "Of all the 'manage many agents' tools I've tried, Orca is definitely the best."（Erik.eth）、"Instead of jumping between three agent sessions, I have one."（Trevin Chow）ほか。agentconn は「Codex デスクトップを 1 週間で置き換えた」という開発者の声を紹介し 4.5/5 と評価、短所は並列実行時の API 費用。— [onorca.dev](https://www.onorca.dev/) / [agentconn blog](https://agentconn.com/blog/orca-ade-agent-fleet-parallel-coding-agents-2026/) / [agentconn review](https://agentconn.com/agents/orca/)

### Inferences
- 批判の軸は 2 本に集約できる: (1) **エディタとしての弱さ**（LSP 無し・編集が遅い・vim 無し・複数ウィンドウ無し）、(2) **Electron の重さと、半年で約 950 リリースという速さが生む揺れ**。どちらも necoder の土俵（GPUI ネイティブ・idle RSS 122MB・本物のエディタ・LSP）で、「ADE の並走管理を本物のエディタの中でやる」は空いている席に見える。
- 「ADE はどれも同じ」と言われる中で利用者が Orca を選ぶ理由は、骨格ではなく個別の決め手（Design Mode・diff コメント返し・sleep・SSH・通知）。necoder が語るべき決め手は Captain・Conflict Radar・要対応キュー・色による方向感覚で、骨格は既に横並び。
- orchestration は称賛にも批判にもほとんど登場しない。利用者の大半は「独立した N 体を自分で見張る」使い方で、采配役は一部のヘビーユーザーの領域。Captain を Fleet の中核に置く necoder の賭けは、市場がまだ検証していない。

### Gaps
- Reddit・X・Product Hunt の一次スレッドは検索に掛からなかった（件数ゼロではなく未発見）。
- Claude Squad・Vibe Kanban・Sculptor・Codex アプリ・Cursor との詳細な比較記事は見つからなかった（agentconn の一言と HN の列挙のみ）。
- 第三者ブログの多く（rpabotsworld・zentor・pyshine・byteiota 等）は README の焼き直しで独自の検証が無いため採用していない。

## 9. necoder（Fleet / Captain）への設計上の教訓

### Takeaway
骨格（左 = Task 一覧 / 中央 = 1 枚 / 並べるのは明示）と「采配役 = ただのエージェント + 永続台帳 + CLI」は Orca と一致しており、方向は正しい。
取り込む価値が高いのは **(a) 完了報告の契約と 3 値の生死判定、(b) diff コメントのバッチ返し、(c) `.worktreeinclude` 型の宣言的セットアップと非同期作成、(d) 未読の手動操作と sleep**。逆に necoder が勝てるのは **ACP によるイベント配送（PTY に打たない）・強制された人間 gate・Conflict Radar・Captain の GUI・ネイティブ性能**。

### Cited Findings
- Orca は組み込みスケジューラ（`orchestration run` / `coordinator-start`）を捨て、エージェントが CLI を叩く方式へ移った。— [docs/cli/orchestration.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/orchestration.mdx)
- PTY にポインタを打つ起こし方は「人間の下書きに混ざる」「AskUserQuestion に誤答する」「heartbeat がターンを浪費する」「idle な coordinator を起こせない」を open のまま抱える。— [issue #15190](https://github.com/stablyai/orca/issues/15190)
- worker の契約は「`worker_done` を 1 回・`--outcome` 明示・3 文の要約・Task と Dispatch の両 ID・質問は `ask`」。— [references/worker-contract.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration/references/worker-contract.md)
- 生死は `live` / `unverifiable` / `exited` の 3 値で、不在は何の許可にもならない。Discussion でも FAILED と UNVERIFIED を分けよという実務知見。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md) / [Discussion #681](https://github.com/stablyai/orca/discussions/681)
- Task spec は Target / Change / Constraints / Ownership / Observable acceptance。— [skill-guides/orchestration.md](https://github.com/stablyai/orca/blob/main/skill-guides/orchestration.md)
- base から 20 コミット超遅れた worktree への dispatch を拒否する。— [coordinator-stale-base-flag.ts](https://github.com/stablyai/orca/blob/main/src/main/runtime/orchestration/coordinator-stale-base-flag.ts)
- 利用者は「orchestrator とだけ話したい・worker のタブは隠したい」、worker の通知は騒音、release 後の子 worktree が残る。— [issue #4374](https://github.com/stablyai/orca/issues/4374) / [issue #15148](https://github.com/stablyai/orca/issues/15148) / [issue #18411](https://github.com/stablyai/orca/issues/18411)
- diff コメントは 1 通にまとめて返す方が当たる、という公式の主張。— [docs/review/annotate-ai-diff.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/review/annotate-ai-diff.mdx)
- 未追跡ファイルは `.worktreeinclude`（コピー）と `sharedDirectories`（symlink）と APFS clone-copy で宣言的に解く。作成はダイアログを即閉じて背景で進める。— [docs/model/worktrees.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/model/worktrees.mdx)
- スキルは薄い stub を配り、本文はバイナリが返す（版ずれ防止）。— [docs/cli/skills.mdx](https://github.com/stablyai/orca/blob/main/docs/site/content/docs/cli/skills.mdx)
- 編集体験の遅さと Electron の重さが離脱理由になっている。— [HN 49103047](https://news.ycombinator.com/item?id=49103047) / [issue #6795](https://github.com/stablyai/orca/issues/6795)
- Windows 対応の罠は AGENTS.md に網羅（setup は `.cmd`、MSYS の job breakaway、自動更新を跨ぐデーモンの `%LOCALAPPDATA%` 退避、EDR に嫌われる起動形）。— [AGENTS.md](https://github.com/stablyai/orca/blob/main/AGENTS.md)

### Inferences
- **Captain の起こし方は現行設計（FLEET-V2 §5.3）のままでよく、むしろ売りにする。** Orca が 4 か月かけて未解決の問題は「ターンの外から PTY に打つ」ことに由来する。ACP の prompt ターンとして台帳イベントを渡し、実行中はキューに積んで 1 通にまとめる necoder の方式は、誤配送・下書き汚染・誤答を構造的に起こさない。heartbeat 相当（Working の継続）は「状態であって起床理由ではない」を明文化しておく（Orca の D3 と同じ結論）。
- **`fleet` CLI に完了報告の契約を足す。** 現状の wake は「Done / review_ready / Failed の遷移 + digest」だが、Captain が切った Task の worker に `fleet report --outcome succeeded|failed --summary "<3 文>" --files …` を 1 回だけ呼ばせ、台帳に積む。digest（推定）と自己申告（宣言）の食い違いは Captain と要対応カードの判断材料になる。"Never encode failure only in prose" はそのまま採用できる。
- **状態に `unknown`（UNVERIFIED）を足す。** ACP 接続の切断・プロセスの消失・応答なしを `failed` に潰すと Captain が再投入して副作用が二重に走る。「不在は何の許可にもならない」を Captain のプロンプト（`captain.prompt`）と `fleet list` の事実層に書く。
- **Task の「試行」を識別する。** Orca の Dispatch（試行 ID + capability）は、やり直し後に古い試行の完了報告が新しい Task を完了させる事故を防ぐ。necoder は 1 Task = 1 worktree だが、Captain が同じ Task に別エージェントを当て直す時のために、台帳イベントに thread / attempt の識別子を持たせる。
- **Captain の Task 作成プロンプトに 5 項目テンプレ**（対象 / 変更 / 制約 / 所有範囲 / 観測可能な受入条件）を入れる。所有範囲（触ってよいファイル）は Conflict Radar の事前版として効く。
- **spawn 前の stale-base チェック。** `fleet create` 時に統合先から N コミット以上遅れた base を警告または拒否する。Radar は事後、これは事前。
- **Captain 起動の Task はサイドバーで畳める・OS 通知を抑える。** Orca の利用者要望（orchestrator とだけ話したい・worker の通知は騒音）を先取りする。`⚑` 帰属の Task は個別の完了通知を出さず、Captain の報告と要対応だけを鳴らす設定を既定にする案。
- **片付けの会計を Captain の wake に同乗させる。** 「統合済みだが未片付けの Task N 件」を事実層に入れ、Captain に報告させる（削除は人間のまま）。Orca の孤児 worktree・未回収 worker 問題の回避。
- **変更ペインに行コメント → 一括送信を足す。** 今はファイル一覧 → Editor で diff を開くだけ。「コメントを貯める → 宛先スレッドを選ぶ → 1 通で送る → 未解決は次回に再同梱」は Orca で最も褒められている機能の 1 つで、necoder の「レビュー待ち radar ✗ → 修正を指示」の中身になる。
- **準備スクリプトの前に宣言的な層を置く。** `.worktreeinclude` 互換のコピー一覧と、共有ディレクトリ（symlink / APFS clonefile）を設定で持てば、`.env` のコピーだけのためにシェルスクリプトを書かせずに済む。`CARGO_TARGET_DIR` 共有のような言語固有の判断は引き続き `task.env` とスクリプト側に残す。Windows では Orca の「`#!` が無ければ `.cmd`」の判定が参考になる。
- **＋Task は即閉じて背景で作る。** `git fetch`・`worktree add`・準備スクリプトの間ダイアログを開いたままにせず、サイドバーに進捗行（キャンセル / Retry）を出す。準備の失敗を `failed` + 要対応に出す現設計と素直に繋がる。
- **未読の手動操作と sleep。** 「確認済み → 未確認に戻す」（Gmail のスター相当）は実装が軽く、N 体並走の「後で見る」を支える。sleep（エージェントのプロセスを止め、開いた時に ACP の session load / resume で戻す）は idle メモリ予算を守る武器になり、「削除はしたくないが邪魔」という需要にも応える。
- **fleet CLI のスキルは stub + バイナリ配信にする。** `necoder fleet help --for-agent` のように版に合った手引きを本体が返せば、Captain 用プロンプトと CLI の版ずれを防げる。
- **やらないことの確認。** Orca の面の多さ（Tasks / Automations / Dashboard / Agents フィード / Workspace Board / Cmd-J）は「毎日足される機能の置き場」として増えた結果で、「表面積が大きく足元が動く」という批判を招いている。necoder の「概念は Task・タブ・Captain の 3 つ」「要対応はサイドバー常設」という畳み方は維持する。
- **権限の既定は別の賭け。** Orca は全バイパスで承認キューを消した。necoder は要対応 + Captain の推薦（§5.5）で gate を保つ。摩擦が離脱理由にならないよう、「worktree 内・読み取り系は自動許可」程度のポリシーを早めに検討する（FLEET-V2 の「残る判断」と同じ論点）。

### Gaps
- Orca の orchestration を実際に動かしての検証はしていない（文書・ソース・issue の読解のみ）。Captain の設計比較を固めるなら、Orca 実機で「目標 1 つ → coordinator が 2 worker → ask と gate → release」の一周を試す価値がある。
- Conductor の「4 つの協調パターン（Supervisor / Pipeline / Consensus / Swarm）」は Superset の比較記事（競合作成）の記述のみで、一次資料は未確認。Captain の比較対象としては別途調査が要る。
