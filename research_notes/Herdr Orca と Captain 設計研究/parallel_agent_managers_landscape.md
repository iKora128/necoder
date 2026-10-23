# 並列コーディングエージェント管理ツールの全景 2025–2026（herdr / Orca を除く）

調査日: 2026-09-20。対象は「1 人の人間が N 体の AI コーディングエージェントを並走させて監督する」ことを主目的にしたツールと、IDE ベンダーが出した agent manager / mission control 面。herdr と Orca は別担当のため扱わない。
読み手は necoder の Fleet（Task = worktree・左に状態つき Task 一覧・中央に Task カード 1 枚・任意の調整役 Captain）の作者。

信頼度の印:
- 【一次】公式 docs / changelog / blog / README を取得して本文を確認した。
- 【二次】レビュー記事・比較記事・第三者の要約・検索結果の要約しか取れていない。
- 【矛盾】出典どうしが食い違う。
- 引用・製品名・コマンド・URL は原語のまま。日付は出典に書かれていたもの。2026 年のこの分野は月単位で変わるので、日付の無い主張は「2026-09 時点でそう書いてあった」と読むこと。

## 1. 画面の型はどこに収束したか（一覧+1 枚 / カンバン / インボックス / 端末グリッド）とカンバン・インボックスの評判

### Takeaway
2026-09 時点の収束形は「左 = セッション（Task）の一覧を状態つきで、プロジェクトや状態でグルーピング ／ 中央 = 選んだ 1 本を chat・diff・terminal・preview のタブかペインで」。カンバンは Vibe Kanban → Nimbalyst → Windsurf 2.0 / Devin Desktop と採用が続いたが個人用途では評判が割れ、Conductor は「状態でグループ化したサイドバー」に畳んだ。インボックスは Antigravity が消して反発を受け、Codex（Triage）と Warp（通知メールボックス）に残る。タイル表示は既定ではなく明示操作として VS Code・Cursor 3・Claude desktop に戻ってきている。

### Cited Findings

#### 比較表（コンパクト。詳細と追加の出典は下の「ツール別メモ」）

| ツール（確認時点） | 単位 | 画面の骨格 | 状態の見せ方 / 注意の引き方 | 隔離 | レビュー / 統合 | 内蔵の調整役 | 主な出典 |
|---|---|---|---|---|---|---|---|
| **Conductor**（0.87.0・2026-09-18） | Workspace = 1 branch = 1 worktree（中に複数 chat） | 左 repo→workspace 一覧（backlog / in progress / in review / done でグループ + セクション）・中央 chat・右 diff / terminal / checks | working アニメ・needs permission・unread dot・GitHub 状態（merged / failing CI / conflicts）・完了音・⌘⌥L で次の要対応【二次】 | git worktree + Conductor Cloud（sandbox・2026-07-30） | diff のインラインコメント→composer 添付・diff 内で直接編集・Checks タブ・⌘⇧P で PR・GitHub Stacks | なし。ただし Conductor CLI / MCP でエージェント自身が workspace を作り・送り・監視できる（2026-08） | [changelog](https://www.conductor.build/changelog) / [workflow](https://www.conductor.build/docs/concepts/workflow) / [releasebot](https://releasebot.io/updates/conductor) |
| **Nimbalyst**（旧 Crystal） | Session（worktree は任意） | セッションのカンバン（Backlog / In Progress / Waiting / Review / Done・自動移動）+ ビジュアルエディタ群 | running / completed / failed / paused・モバイル push | git worktree | diff レビュー・iOS から承認/却下 | なし | [session kanban](https://nimbalyst.com/blog/claude-code-session-kanban-organize-ai-agents/) / [repo](https://github.com/nimbalyst/nimbalyst) |
| **Sculptor**（Imbue） | Workspace = worktree + branch | 稼働中の全エージェントを 1 面に・各 workspace に terminal と diff | 詳細不明 | 現在は worktree。旧来は Docker コンテナ + Pairing Mode【矛盾】 | 同じ面で review / merge・衝突を検知してエージェントに戻す【二次】 | なし | [product](https://imbue.com/product/sculptor) / [repo](https://github.com/imbue-ai/sculptor) / [Ry Walker](https://rywalker.com/research/sculptor) |
| **Vibe Kanban**（コミュニティ保守） | Issue → workspace（attempt ごとに worktree） | ブラウザのカンバン + task 詳細（chat / diff / preview） | 列 = 状態 | git worktree | インラインコメントをそのままエージェントへ・merge / PR / rebase | なし（MCP server 経由で外部エージェントがカードを作れる） | [review](https://vibecoding.app/blog/vibe-kanban-review) / [Show HN](https://news.ycombinator.com/item?id=44533004) |
| **Superset** | Workspace = worktree | 左 workspace サイドバー・端末タブ（無限 split）・diff ビューア・内蔵ブラウザ | working インジケータ・完了チャイム・Dock バッジ | git worktree・`.superset/config.json`（setup / teardown / run）・ポート検出 | diff を見て編集・PR | なし（CLI / MCP server / SDK で外から操縦） | [repo](https://github.com/superset-sh/superset) |
| **Emdash**（YC W26） | Task = worktree | project→task の一覧 + agent 端末 + diff / checks（カンバン表示ありとの記述は【二次】） | 詳細不明 | 事前に温めた worktree（起動 0.5–1 s）・`$EMDASH_PORT`・setup / run / teardown・SSH リモート | エージェント間の diff 横並び・PR・CI checks・issue 連携 | なし（意図的に持たない） | [Show HN](https://news.ycombinator.com/item?id=47140322) / [docs](https://emdash.com/docs/) |
| **Codex app**（OpenAI・2026-02-02〜） | Thread（Project 配下・Local / Worktree / Cloud） | 左 Project→Thread + **Triage** 節・中央 thread・右 review pane | Triage = 既読/未読つきインボックス・所見なしの run は自動アーカイブ【二次】 | Codex 管理 worktree（直近 15 本保持）/ permanent worktree / cloud コンテナ | review pane のインラインコメント・commit / push / PR・Hand off（Local⇄Worktree） | subagents が GA（メインスレッドが spawn / route / wait / close）。アプリ横断の調整役は無い | [unofficial KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/) / [subagents docs](https://learn.chatgpt.com/docs/agent-configuration/subagents) |
| **Claude desktop「Code」タブ / Claude Code on the web**（再設計 2026-04-14） | Session（Local / Cloud / SSH・worktree は任意） | 左 セッション一覧（status / project / environment でフィルタ・project でグループ）・中央は chat / diff / browser / terminal / file / plan / tasks / subagent のペインを自由配置 | 見ていないセッションが終わると OS 通知・CI 完了通知・Dispatch は携帯へ push | `<project-root>/.claude/worktrees/`・`.worktreeinclude`・PR の merge/close で自動アーカイブ | diff のインラインコメントを ⌘⏎ で一括送信・Review code・CI の Auto-fix / Auto-merge | agent teams は CLI のみ（experimental）。Desktop には「他セッションを一覧・閲覧・送信」する cross-session messaging | [blog](https://claude.com/blog/claude-code-desktop-redesign) / [desktop docs](https://code.claude.com/docs/en/desktop) |
| **Devin**（web） | Session（VM） | 左 セッション一覧（nav タブ・グルーピング・フィルタ）・中央 session | 永続ステータスラベル（"PR created" / "Awaiting instructions" / "Approve session"）+ 注意ドット・Slack | クラウド VM | PR・Devin Review | **あり**: Managed Devins。子は親の下にツリー表示 | [blog](https://cognition.com/blog/devin-can-now-manage-devins) / [release notes 2026](https://docs.devin.ai/release-notes/2026) |
| **Devin Desktop**（旧 Windsurf・改名 2026-06-02【二次】） | Agent session（Space に束ねる） | **Agent Command Center** = 状態別カンバン（起動時の既定面）+ Spaces | 実行中のカードはグレーアウトで読み取り専用・完了か入力待ちで OS 通知 1 回 | local（worktree）+ Devin の VM | Devin の PR を IDE 内でレビュー・local エージェントへ手戻し | 調整役は Devin 側（Managed Devins）。ACC は人間が調整 | [Windsurf 2.0](https://devin.ai/blog/windsurf-2-0) / [ACC docs](https://docs.devin.ai/desktop/agent-command-center) |
| **Augment Intent**（public beta 2026-02） | Workspace = worktree + living spec | spec + coordinator chat + agents / terminal / diff / browser | 詳細不明 | specialist ごとに worktree | verifier 通過後に統合・commit→PR→merge をアプリ内で | **あり**: Coordinator → Implementors（waves）→ Verifier。spec を人間が承認してから fan-out | [blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration) |
| **JetBrains Air**（public preview 2026-03） | Task = 1 agent session | task 一覧 + 1 task を大きく（chat / diff / terminal / preview） | 別 task が要対応になると通知・task state switcher ⌘1 | Local / git worktree / Docker / Cloud（tech preview） | コードベース文脈の diff・worktree やコンテナから本体へ持ち帰る | なし | [JetBrains blog](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/) |
| **Cursor 3 Agents Window**（2026-04-02） | Agent（local / worktree / cloud / SSH） | 左に全 agent（全 repo 横断・mobile / Slack / GitHub / Linear 起動分も）・中央 chat と diff・Agent Tabs は横並び or グリッド | cloud agent は demo と screenshot を検証用に出す | `/worktree`・cloud VM・local⇄cloud handoff | 新 diffs view で stage / commit / PR・`/best-of-n` | `/multitask`（async subagents・3.2）。planner / worker / judge は研究段階 | [Cursor 3](https://cursor.com/blog/cursor-3) / [3.2](https://cursor.com/changelog/04-24-26) |
| **VS Code Agents window**（Preview）+ **GitHub Agent HQ / mission control** | Session（local / background / cloud・Copilot / Claude / Codex） | 別ウィンドウ: sessions list（workspace か時間でグループ・pin・カスタムグループ）/ chat / Changes / Files。⌘1–9 でグリッド位置のセッションへ | 行に status と変更統計。GitHub 側は session log をリアルタイム表示・途中で steer | background は git worktree・cloud は GitHub 側の環境 | Changes で範囲コメント→agent・Commit / Sync / Mark Ready・one-click merge conflict resolution | セッション内の parallel subagents のみ | [agents window](https://code.visualstudio.com/docs/agents/run/agents-window) / [Agent HQ](https://github.blog/news-insights/company-news/welcome-home-agents/) |
| **Google Antigravity 2.0**（2026-05-19） | Conversation（Project 配下） | Editor view と Agent Manager の 2 面。Manager 左 = project / status / recency でグループ化した会話一覧 | Inbox（全 project の承認待ち）が 2026-02 頃に消えて反発。通知トーストは右下スタック | 会話ごとに worktree を自動作成・自動片付け | Artifacts（plan / task list / screenshot / browser 録画）をレビュー | **あり**: main agent が dynamic subagents を定義・spawn・poll | [I/O deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive) / [forum](https://discuss.ai.google.dev/t/did-they-remove-the-inbox-in-agent-manager/126943) |
| **Zed Parallel Agents**（2026-04-22） | Thread（worktree は thread ごとに任意） | 左 Threads Sidebar（project でグループ）+ Agent Panel・右に Project / Git パネル | 行 = title・status indicator・agent。Terminal Threads も同じ列に | title bar の worktree picker。アーカイブで worktree の git 状態を保存して disk から削除、復元で戻る | 既存の agent diff | なし | [blog](https://zed.dev/blog/parallel-agents) / [docs](https://zed.dev/docs/ai/parallel-agents) |
| **Kiro autonomous agent**（preview 2025-12-02） | Task（sandbox） | web app + GitHub issue ラベル | 不明点は質問してくる | 隔離 sandbox・同時 10 task | 説明つき PR・レビュー指摘を学習 | **あり（内部）**: research/planning・code・verification の 3 sub-agent | [Kiro blog](https://kiro.dev/blog/introducing-kiro-autonomous-agent/) |
| **Amp** | Thread | VS Code 拡張の Agents Panel（thread 一覧・j/k・Space で最後のメッセージを展開） | 詳細不明 | remote の Orbs【二次】 | thread を URL で共有 | main agent が subagents / Oracle を呼ぶ（セッション内） | [Agents Panel](https://ampcode.com/news/agents-panel) |
| **Warp + Oz** | Agent conversation / run | 端末タブ + Agent Management Panel。orchestration 中は pill bar（親 + 子ごとに pill） | タブの状態アイコン（working / blocked / completed / errored）・トースト最大 2 枚・通知メールボックス（All tabs / Unread / Errors）・OS 通知 | local 会話 / cloud environment | 親が（任意で）結果を merge | **あり**: `/orchestrate`（分解案を人間が承認してから spawn）・親子 1 段・子は別 harness 可・message bus | [orchestration](https://docs.warp.dev/platform/orchestration/) / [notifications](https://docs.warp.dev/agents/capabilities/agent-notifications/) |
| **cmux** | Workspace = 縦タブ（中に split pane） | 左 縦タブ（branch・PR・cwd・ports・最新通知の本文）・右 端末 split + ブラウザ | 待ちの pane に青リング・タブ点灯・通知パネル・⌘⇧U で最新未読 | なし（worktree は利用者側） | なし | なし。subagent / teammate を native pane に変換。コミュニティの cmux-team は 4 層階層 | [repo](https://github.com/manaflow-ai/cmux) |
| **Claude Squad** | Instance = tmux session + worktree | 左 instance 一覧・右 preview / diff タブ | 詳細不明 | tmux + git worktree | `s` commit/push・`c` checkout | なし（`-y` autoyes） | [repo](https://github.com/smtg-ai/claude-squad) |
| **ccmanager** | Session（worktree × project） | TUI メニュー | Idle / Busy / Waiting・状態変化 hook | git worktree・devcontainer | worktree の作成 / merge / 削除 | なし（AI 判定の auto-approval あり） | [repo](https://github.com/kbwo/ccmanager) |
| **Agent of Empires** | Session（tmux + worktree） | TUI + web dashboard（モバイル可） | running / waiting / idle / errored | worktree + Docker / Podman / Apple Containers | 詳細不明 | なし | [review](https://vibecodinghub.org/blog/agent-of-empires-review) |
| **Gas Town**（Steve Yegge・2026-01） | Rig / convoy | tmux ベース。人間は Mayor と話す【二次】 | 詳細不明 | 詳細不明 | Refinery（merge queue 担当のエージェント） | **あり**: Mayor + Polecats / Refinery / Witness / Deacon / Dogs / Crew の 7 役 | [Codex KB](https://codex.danielvaughan.com/2026/04/08/gas-town-multi-agent-factory/) |
| **Omnara / Happy** | ローカルの Claude Code / Codex セッション | モバイル / web のセッション一覧 + chat | push 通知・ワンタップ承認 | なし | スマホで diff 確認 | なし | [Omnara](https://www.omnara.com/) / [Happy](https://happy.engineering/) |
| **Terragon** | cloud task | 2026-02-09 に終了 | | | | | [shutdown](https://www.terragonlabs.com/) |
| **Chorus** | 対象外 | 複数 LLM に同時に聞く Mac の chat アプリで、エージェント管理ではない | | | | | [repo](https://github.com/meltylabs/chorus) |

#### ツール別メモ: デスクトップ / GUI

**Conductor（Melty Labs・macOS）**
- 単位は workspace。"1 workspace maps to 1 branch"、"A branch can only be checked out in one workspace at a time"。新規 workspace は fetch してから既定の base（例 `origin/main`）に新しい branch を切る【一次】 — [Workspaces and branches](https://www.conductor.build/docs/concepts/workspaces-and-branches)
- 1 workspace に複数エージェントを入れる使い分けを公式が書いている: "Use multiple workspaces when tasks should land independently. Use multiple agents in one workspace when the work shares the same branch, code state, and context."【一次】 — [Parallel agents](https://www.conductor.build/docs/core/parallel-agents)
- 公式の流れは 6 段: create workspace → develop → review（⌘⇧D の diff viewer）→ "Create a pull request with Command + Shift + P" → merge → "Archive finished workspaces so your sidebar stays focused"。Checks タブで "git status, CI, deployments, comments, and todos before you merge" を見る。"Inline comments become composer attachments"【一次】 — [Workflow](https://www.conductor.build/docs/concepts/workflow)
- 0.35.0（2026-02-11）で workspace を "backlog, in progress, in review, and done" の 4 状態で整理し、PR タイトルがあればそれで名前を出すようにした。状態遷移が自動か手動かは release note に書かれていない【一次】 — [0.35.0](https://www.conductor.build/changelog/0.35.0-workspace-status)
- 2026 年後半の主な出荷: 0.77.0（07-23）early access multiplayer・API・background tasks / 0.78.0（07-30）Conductor Cloud / 0.80.0（08-10）Stacks / 0.82.0（08-20）Conductor MCP / 0.84.0（09-02）diff 内で直接編集 / 0.85.0（09-09）sidebar sections・routines / 0.86.0（09-16）loadouts の共有 / 0.87.0（09-18）ファイル横断検索・OpenCode 2.0【一次】 — [Changelog](https://www.conductor.build/changelog)
- Conductor Cloud は "in isolated sandboxes, so agents keep working when you close the app or step away from your computer"。0.80.0 で cloud workspace に Conductor CLI を同梱し、エージェントが "orchestrate work across Conductor, including creating workspaces, starting chats, sending follow-up messages, and monitoring their progress" できる。0.82.0 の Conductor MCP では "Review workspace diffs and leave inline comments"、"Create another workspace and ask an agent to investigate X" が可能。0.85.0 で routines（"run on a schedule or via GitHub action"）・private workspaces・matrix-style の状態インジケータ【二次: リリースノートの転載】 — [Releasebot](https://releasebot.io/updates/conductor)
- 状態インジケータは Settings → Experimental の "Workspace status matrix" で、routine runs / attention / errors / running pull request checks / merged pull requests を区別する【二次: 検索結果の要約】 — [Changelog](https://www.conductor.build/changelog)
- 第三者が Conductor の docs と changelog を読み込んでまとめた調査（GitHub issue）によれば: サイドバーは PR ライフサイクルのグルーピング + git 状態の色ストライプ + GitHub 状態（merged / failing CI / conflicts）+ エージェントの注意状態（"working animation"・"needs permission indicator"・"unread dot"）。⌘⌥L で "Next chat needing attention"、選択中の workspace をクリックすると "jumps to its oldest unread chat"。一覧は活動で並び替わり、そのせいで「サイドバーが勝手に跳ぶ」バグ修正が changelog に複数ある。行コメントは 0.10.0（2025-09-08）から。逆向きに Claude が人間の diff にコメントすることもできる。承認は既定でオフ（approvals-off）。worktree は `~/conductor/workspaces/<repo>/<workspace>`、名前は都市名。⌘N か "Create from…" で branch / PR / GitHub issue / Linear issue から作る。archive は未コミットを commit として保存してからディレクトリを消す。設定は `.conductor/settings.toml` の `scripts.setup` / `scripts.run.<id>` / `scripts.archive`、gitignore 済みファイルは `.worktreeinclude` で持ち込む。ターンごとに private な git ref で checkpoint を取る。ポーリングが繰り返し CPU 問題になった形跡が changelog にある（0.7.0 / 0.37.0 / 0.61.0 / 0.72.0）【二次】 — [infinitus issue #270](https://github.com/deathemperor/infinitus/issues/270)
- 日本語の使用記（2025-12-17）: 画面は「左にスレッド、真ん中にチャット、右上にDiff、右下にターミナル」。setup の例は `"setup": "npm install; cp $CONDUCTOR_ROOT_PATH/.env .env"`。ブラウザ UI の Vibe Kanban と比べ、アプリなので「途中でどっかいくこともあまりない」 — [Zenn explaza](https://zenn.dev/explaza/articles/cacf0fd6ad83f6)
- 公式ガイドは Claude Code 純正の `claude --worktree` との差を「workspace sidebar, setup scripts, run scripts, diff review, checks, and pull request flow を足すもの」と位置づけている【二次: 検索結果の要約】 — [Run multiple Claude Code sessions](https://www.conductor.build/docs/guides/parallel-agents/run-multiple-claude-code-sessions)

**Crystal → Nimbalyst（Stravu）**
- Crystal のリポジトリ名は "(Crystal is now Nimbalyst)"【一次】 — [stravu/crystal](https://github.com/stravu/crystal)
- 2026-02 に Crystal を deprecate、2026-03 に Nimbalyst の free beta、2026-04-29 に desktop と iOS を MIT で公開【二次: wiki】 — [Nimbalyst wiki](https://ai.miraheze.org/wiki/Nimbalyst)
- セッションカンバンの列は "Backlog … In Progress: Sessions actively running, Waiting: Sessions that need your input, Review: Completed sessions you need to review, Done: Reviewed and merged"。"automatic status tracking" でカードは手で動かさない。カードには "session name, project, agent type, last activity"。自社の弱点として "The opinionated kanban workflow may not suit developers who prefer freeform, list-based session management." と書いている【一次: ベンダーのブログ】 — [Session kanban](https://nimbalyst.com/blog/claude-code-session-kanban-organize-ai-agents/)
- macOS / Windows / Linux + iOS / Android の companion。モバイルは完了と承認待ちで push 通知を送る【一次】 — [repo](https://github.com/nimbalyst/nimbalyst) / [Mobile app docs](https://docs.nimbalyst.com/mobile/mobile-app)

**Sculptor（Imbue）**
- 2026-09 時点の製品ページ: "An extensible, open source app for running coding agents in parallel."、"Each workspace is an isolated worktree with its own branch, terminal, and diff view."、"Free, open source, and MIT licensed."、Mac（Apple Silicon）と Linux【一次】 — [Sculptor product page](https://imbue.com/product/sculptor)
- README は MIT、Claude Code と Pi を統合、"Container Backend" は experimental 扱い、全体が "experimental research preview"、233 stars【一次】 — [imbue-ai/sculptor](https://github.com/imbue-ai/sculptor)
- 【矛盾】2026-06 の第三者調査は「常に Docker コンテナで隔離」「Pairing Mode は Mutagen による双方向同期」「まだ OSS ではない（2026-05 の commit に "we aren't quite open source yet"）」「衝突を検知してエージェントに戻せる」「Suggestions（beta）」と記す。旧 LP も "parallel agents without the hassle of git worktrees" と訴求していた。2026 年 6〜9 月の間に worktree 既定 + MIT 公開へ移ったと読むのが自然だが、移行の告知記事は見つけていない — [Ry Walker Research](https://rywalker.com/research/sculptor) / [旧 LP](https://imbue.com/sculptor/)

**Vibe Kanban（BloopAI → コミュニティ）**
- 2026-04-10 に Bloop が終了を発表。約 30 日後にクラウド（共有カンバン・コメント・組織）を停止。ローカル版は Apache-2.0 のコミュニティ保守として存続、27.6k stars（2026-08 時点）。レビューは "leave inline comments that go straight back to the agent, and open a pull request with an AI-generated description"【二次】 — [Vibe Kanban Review 2026](https://vibecoding.app/blog/vibe-kanban-review)
- Show HN（2025-07-11）で作者 louiskw 自身が "Things move across the board so quickly when AI is doing the work that ~50% of the columns seem pretty redundant" と認めた。lharries は "they often need to be dependent on each other — and then I want to make a change to the first one"、作者は "we defo need better ways to stack and review work"。作者の MacBook は 4 並列で重くなる（"mainly rustc"）。既定でオンだった telemetry は批判を受けて数時間で opt-in に修正 — [Show HN: Vibe Kanban](https://news.ycombinator.com/item?id=44533004)
- 日本語の使用記（2025-11-04）: 「タスクを並行して実行できる分、マージの際に競合が発生することが多くなります」「1 発で良いコードができあがってくることはあまりありません」 — [Zenn torakm](https://zenn.dev/torakm/articles/510848199fc216)
- 弱点として "worktree bloat on large repos" が挙がっている【二次】 — [Augment: 9 orchestrators](https://www.augmentcode.com/tools/open-source-agent-orchestrators)

**Superset**
- 自称は "an agentic IDE to orchestrate 100+ coding agents in parallel"。"Track every agent from the sidebar, with working indicators, completion chimes, and dock badges when one needs your attention"。端末は "infinite splits, presets, and persistent sessions that survive restarts"。`.superset/config.json` に setup / teardown / run。単一バイナリの CLI・MCP server（他のエージェントが workspace を作って管理できる）・TypeScript SDK。macOS と Linux（experimental）、14.4k stars、Elastic License 2.0【一次】 — [superset-sh/superset](https://github.com/superset-sh/superset)

**Emdash（General Action・YC W26）**
- Show HN で創業者は worktree を事前に温めて task 起動を 500–1000 ms にしたこと、`$EMDASH_PORT`、project ごとの setup / run / teardown script を挙げた。調整役を作らない理由は "CLIs themselves are getting good at [agent coordination] natively"。当時 21 CLI 対応（後の記事では 34）【一次: 創業者の投稿】 — [Show HN: Emdash](https://news.ycombinator.com/item?id=47140322)
- docs の機能: エージェント間の diff 横並び（"Review changes across agents side-by-side"）・in-app browser・CI/CD checks・Linear / Jira / GitHub / Notion / Asana の issue 連携・SSH リモート・tmux セッション・Automations（定期実行を "reviewable tasks" として残す）【一次】 — [Emdash docs](https://emdash.com/docs/)
- カンバン形式の task 管理があるという記述は検索結果の要約でしか確認できていない【二次】 — [teqvolt review](https://teqvolt.com/open-source/emdash-open-source-parallel-coding-agents)

**Codex app / Codex cloud（OpenAI）**
- Codex app は 2026-02-02 に macOS で登場、Windows は 2026-03-04。"command center for agents" を名乗り、project ごとに thread、worktree 内蔵。公式の発表ページは 403 で本文を取れなかった【二次】 — [IntuitionLabs](https://intuitionlabs.ai/articles/openai-codex-app-ai-coding-agents) / [公式発表（403）](https://openai.com/index/introducing-the-codex-app/)
- 非公式ナレッジベースによる構造: Project → Thread（Local / Worktree / Cloud）。automation の結果は sidebar の **Triage** に入り、既読/未読で絞れ、所見なしの run は自動アーカイブ。レビューは diff + インラインコメント → Approve（commit / push / PR）/ Revise / Reject。Codex 管理 worktree は直近 15 本を保持、permanent worktree は手動で長寿命。worktree 作成は約 0.8 秒・約 120 MB。Hand off で Local⇄Worktree を移す【二次: 非公式】 — [Codex KB: automations](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/)
- PR ブランチ上では sidebar に PR 文脈とレビュアーのフィードバックが出て、review pane は diff の横にコメントを並べる【二次: 公式 docs の検索要約】 — [Code review docs](https://developers.openai.com/codex/app/review/)
- Subagents は GA で既定オン。基本は明示依頼（"spawn two agents"）。組み込みは `default` / `worker` / `explorer`、自作は `~/.codex/agents/` か `.codex/agents/`。上限は `agents.max_concurrent_threads_per_session`。CLI は `/agent` でスレッド切替、非アクティブなスレッドの承認要求も表に出て `o` で開ける。web の sidebar は "reports subagent activity; it doesn't provide controls to stop or steer an individual subagent"。指針は "Be more careful with parallel write-heavy workflows, because agents editing code at once can create conflicts."【一次】 — [Subagents docs](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- Codex cloud は task ごとに別コンテナで並列実行し、`codex cloud` の `--attempts` で best-of-N（最大 4）【二次: 検索結果の要約】 — [agent37: Codex Cloud](https://www.agent37.com/blog/codex-cloud)

**Claude desktop「Code」タブ / Claude Code on the web（Anthropic）**
- 2026-04-14 の再設計: "The new sidebar puts every active and recent session in one place"、"filter by status, project, or environment, or group the sidebar by project"、PR が merge / close されるとセッションが自動アーカイブ、"Every pane is drag-and-drop"、side chat（⌘;）は "pull context from the main thread, but don't add anything back"、表示は Verbose / Normal / Summary の 3 段【一次】 — [Redesigning Claude Code on desktop](https://claude.com/blog/claude-code-desktop-redesign)
- docs: worktree は既定で `<project-root>/.claude/worktrees/`、gitignore 済みファイルは `.worktreeinclude`、branch prefix を設定可能。"The desktop app sends an OS notification when a Code session finishes a task and you aren't currently viewing that session"。diff は行クリックでコメント、⌘⏎ で一括送信、"Review code" で Claude が先に diff へコメント。PR 後は CI status bar に **Auto-fix** と **Auto-merge**（squash）のトグル、CI 完了でも通知。⌘+クリックで 2 セッションを並べて見られる。tasks ペインと subagent ペインがある【一次】 — [Desktop docs](https://code.claude.com/docs/en/desktop)
- Desktop では "Agent teams … are available in the CLI, not in Desktop"。代わりに Claude が "list your other Code tab sessions, read what each has been doing, and send messages between them"。安全規則は 4 つ（アーカイブ前に確認・誰も見ていないセッションからは送れない・受信側の inbound controls・受信メッセージは引用して送信元を明示）【一次】 — [Desktop docs](https://code.claude.com/docs/en/desktop)
- web 版は claude.ai/code で Anthropic のクラウド上で走り、`--remote` で送り `--teleport` で手元に戻す。2026-04 から routines【二次: 検索結果の要約】 — [Web quickstart](https://code.claude.com/docs/en/web-quickstart)

**Devin（Cognition）**
- 2026-03-19 "Devin can now Manage Devins": "The main Devin session acts as a coordinator: it scopes the work, assigns each piece to a managed Devin, monitors progress, resolves any conflicts, and compiles the results."。各子は "a full Devin, running in its own isolated virtual machine"。親は子へのメッセージ・ACU 消費の監視・sleep / terminate・自分宛ての予約メッセージができる。人間も "Each has its own session link, so you can inspect its work or message it directly"【一次】 — [Cognition blog](https://cognition.com/blog/devin-can-now-manage-devins)
- 2026 release notes: 05-08 sidebar に永続ステータスラベル（"PR created" / "Awaiting instructions" / "Approve session"）とタイムスタンプ。05-17 sub-Devin を入れ子ツリーで表示。08-21 child session に tree connector、sidebar を再設計（customizable nav tabs・grouping・filtering、PR 状態別 / repo 別のグルーピング、Slack 連携の操作）【一次】 — [Devin release notes 2026](https://docs.devin.ai/release-notes/2026)
- 前身の MultiDevin（2024-09）は manager 1 + worker 最大 10 で、成功した worker の変更を 1 つの branch / PR にまとめた【二次: 検索結果の要約】 — [Cognition on X](https://x.com/i/status/1836866702182863286)
- nader dabit の紹介: "not subagents sharing your session's context, but full Devins in isolated VMs"【二次: 検索結果の要約】 — [X post](https://x.com/dabit3/status/2092665092097839216)

**Devin Desktop（旧 Windsurf）**
- Windsurf 2.0（2026-04-15）: Agent Command Center は "every agent you're running, local and cloud, in a single Kanban view, organized by status"。動機は "The more agents you manage, the faster you hit the limits of your own attention and working memory."。Spaces は "agent sessions, PRs, files, and context" を束ねる。local の作業を 1 クリックで Devin（クラウド）に渡す【一次】 — [Windsurf 2.0](https://devin.ai/blog/windsurf-2-0)
- ACC の docs（docs.windsurf.com は docs.devin.ai/desktop へリダイレクトする）: 列は "what is in flight, what needs your attention, and what is finished"（正確な列名は未記載）。"Sessions are locked while their agent is running: they appear greyed out and are read-only until the agent finishes."。"Devin Desktop sends a single native OS notification when an agent session finishes or needs your input."【一次】 — [ACC docs](https://docs.devin.ai/desktop/agent-command-center)
- 2026-06-02 に Devin Desktop へ改名、ACC が起動時の既定面に。Cascade は 2026-07-01 に deprecate され Devin Local（Rust 書き直し・subagent 対応）へ。ACP 対応で Codex / Claude Agent / OpenCode を同じ面に載せる。評は「チームには正しいが、1 人には "unnecessary complexity"」「コードが "one click deeper"」【二次】 — [aicoderscope](https://aicoderscope.com/blog/windsurf-devin-desktop-rebrand-acp-2026/)
- Wave 13（2026-03 中旬）は worktree で最大 5 並列 + Arena Mode【二次】 — [aiautomationglobal](https://aiautomationglobal.com/blog/windsurf-wave-13-parallel-agents-arena-mode-ai-ide-2026)

**Augment Intent**
- 2026-02-10 公開（06-18 更新）。coordinator が Context Engine で "understand your task and propose a plan as a spec"、人間が承認、"fans work out to implementor agents that can run in waves"、最後に "a verifier agent checks the results against the spec … before handing the work back to you for review"。BYOA（Claude Code / Codex / OpenCode）。macOS の public beta【一次】 — [Intent blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration)
- specialist の persona は Investigate / Implement / Verify / Critique / Debug / Code Review【二次】 — [Ry Walker: Augment Intent](https://rywalker.com/research/augment-intent)

**JetBrains Air**
- 2026-03 の public preview。"see one task (meaning one agent session) at a time"、"You'll get a notification when another task needs your attention, so you can quickly switch to it while the agent keeps working."。実行環境は Local / git worktree / Docker / cloud sandbox（tech preview）。Codex・Claude Agent・Gemini CLI・Junie、ACP と ACP Agent Registry、BYOK。macOS 先行【一次】 — [JetBrains blog](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/)
- `.air/worktree.json` と `.air/docker.json` で環境を定義。権限は Ask / Auto-Edit / Plan / Full Access。"Once a task starts, you can switch models but not providers."【二次】 — [recca0120](https://recca0120.github.io/en/2026/05/06/jetbrains-air-agentic-ide/)
- 廃止した Fleet IDE の上に作られたと報じられている（necoder の「Fleet」と語が被る点に注意）【二次: 記事タイトルのみ確認】 — [Machine Herald](https://machineherald.io/article/2026-03/16-jetbrains-launches-air-an-agentic-development-environment-built-on-abandoned-fleet-ide/)

#### ツール別メモ: ターミナル / TUI

**cmux（manaflow）**
- "Open source Ghostty-based macOS terminal with vertical tabs and notifications for AI coding agents."。縦タブに git branch・紐づく PR の状態と番号・cwd・listening ports・最新通知の本文。通知は OSC 9/99/777 か `cmux notify`。"Cmd+Shift+U jumps to the most recent unread"。全操作が CLI と Unix socket で可能。"When an agent spawns subagents or teammates, cmux turns them into native panes and splits."。GPL-3.0-or-later（商用条件は別途）、macOS のみ、27.3k stars【一次】 — [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux)
- 日本語記事（2026-03-22・07-25 更新）: 周辺に using-cmux / cmux-team / cmux-remote / cfork。cmux-team は `Master（ユーザーと対話、task を作成）→ Manager（アイドル待機、task 検出 → Conductor 起動）→ Conductor（git worktree でタスクを自律実行）→ Agent（実作業）` の 4 層 — [Zenn hummer](https://zenn.dev/hummer/articles/cmux-ecosystem-claude-code)

**Claude Squad / ccmanager / Agent of Empires / tmux 系**
- Claude Squad: tmux セッション + git worktree、左に instance 一覧・右に preview / diff タブ、`n` `N` `D` `s` `c` `r`、`-y/--autoyes` は experimental。AGPL-3.0、8.5k stars【一次】 — [smtg-ai/claude-squad](https://github.com/smtg-ai/claude-squad)
- ccmanager: Idle / Busy / Waiting をエージェント別の検出戦略で判定、状態変化で任意コマンドを叩く hook、worktree の作成 / merge / 削除、multi-project、worktree 間の会話コピー、AI 判定による auto-approval、devcontainer、tmux 不要。MIT、1.2k stars【一次】 — [kbwo/ccmanager](https://github.com/kbwo/ccmanager)
- Agent of Empires: Rust。tmux + worktree + Docker / Podman / Apple Containers + TUI / web dashboard（モバイル向けビューあり）。状態は running / waiting / idle / errored。MIT、2.7k stars、v1.12.0（2026-07-01）【二次】 — [vibecodinghub](https://vibecodinghub.org/blog/agent-of-empires-review)
- tmux 系オーケストレーター: NTM（named sessions・broadcast prompts・conflict detection・TUI dashboard）、workmux（tmux + worktree のシェル）、tmux-orchestrator（各 agent に tmux window と worktree と双方向メッセージ）、claude-tmux-orchestration（heartbeat 監視 + ファイルベースの協調）、wmux（再起動を越えるセッション）【二次】 — [NTM review](https://vibecoding.app/blog/ntm-review) / [tmux-orchestrator](https://github.com/hesreallyhim/awesome-claude-code/issues/1279) / [claude-tmux-orchestration](https://github.com/primeline-ai/claude-tmux-orchestration) / [wmux](https://github.com/openwong2kim/wmux)

**Warp（Agent Management Panel + Oz）**
- 通知は 3 種: "Complete"・"Request - the agent is blocked and needs your input"・"Error"。トーストは "Up to two toasts are visible at a time"。ベルの通知メールボックスは "All tabs" / "Unread" / "Errors" で絞れ、"Mark all as read"、矢印と Enter で操作。各タブに状態アイコン（working / blocked / completed / errored）と未読バッジ。背面時は OS 通知。Claude Code・Codex・OpenCode は plugin 設定で同じ通知に乗る【一次】 — [Agent notifications](https://docs.warp.dev/agents/capabilities/agent-notifications/)
- Orchestration: 親 1 + 子 N の 1 段のみ（"exactly one level deep"）。local→local / local→cloud / cloud→cloud / cloud→cloud-local。子は別 harness（Claude Code・Codex）でよい。状態は INPROGRESS / SUCCEEDED / FAILED / BLOCKED / ERROR / CANCELLED。UI は "a horizontal pill bar above the agent view header shows the parent on the left and one pill per child"、Oz web は親の行の下に子をまとめ "Sub-agents" タブを足す。通知は "on the parent agent's conversation only"。エージェント間は "a durable, server-backed message bus" で、終了した子も agent ID 宛てのメッセージで起きる。`/orchestrate` は分解案を出して "Approval is required before the parent launches children"、`/plan` は model / harness / environment / host / parallelism を選べる inline の orchestration config を出す。パターン集は Supervisor/worker・Fan-out/fan-in・Critic/verifier・Review swarm・DAG・Swarm【一次】 — [Multi-agent orchestration](https://docs.warp.dev/platform/orchestration/)
- 2026-05-19 に Oz を multi-harness の control plane（Claude Code・Codex・Warp Agent）として発表【一次】 — [Warp blog](https://www.warp.dev/blog/multi-harness-cloud-agent-orchestration)

**Gas Town（Steve Yegge）**
- 2026-01 に OSS 化。Go 製で 20–30 本の Claude Code を役割階層で動かす。Mayor（主調整役）・Polecats（使い捨て worker）・Refinery（merge queue）ほか計 7 役。利用者の感想として「Mayor と話すのが好評」【二次: 検索結果の要約】 — [Codex KB: Gas Town](https://codex.danielvaughan.com/2026/04/08/gas-town-multi-agent-factory/) / [Welcome to Gas Town](https://steve-yegge.medium.com/welcome-to-gas-town-4f25ee16dd04)

#### ツール別メモ: IDE 統合

**Cursor**
- 2.0（2025-10-29）: "a sidebar for your agents and plans"、"run up to eight agents in parallel on a single prompt"、"uses git worktrees or remote machines to prevent file conflicts"【一次】 — [Cursor 2.0](https://cursor.com/changelog/2-0)
- 3.0（2026-04-02）: Agents Window。sidebar に "all local and cloud agents"（mobile・web・desktop・Slack・GitHub・Linear から起動した分も）、"inherently multi-workspace"、local⇄cloud の handoff は双方向、"Cloud agents produce demos and screenshots of their work for you to verify"、新しい diffs view で "stage, commit, and manage PRs"、"switch back to the Cursor IDE at any time"。時代認識は "third era of software development, where fleets of agents work autonomously to ship improvements"【一次】 — [Meet the new Cursor](https://cursor.com/blog/cursor-3)
- 3.2（2026-04-24）: `/multitask` は "run async subagents to parallelize your requests instead of adding them to the queue"。Agents Window の worktree は背景で走らせ、1 クリックで local の前面へ。multi-root workspace【一次】 — [Cursor 3.2](https://cursor.com/changelog/04-24-26)
- "Agent Tabs inside the window let you view multiple chats side-by-side or in a grid."、`/best-of-n` は同じ task を複数モデルで別 worktree に走らせて並べて選ぶ【二次】 — [AgentPatterns](https://www.agentpatterns.ai/tools/cursor/agents-window/)
- 研究（2026-01-14）"Scaling long-running autonomous coding": ロックつきのフラットな協調は失敗（"Twenty agents would slow down to the effective throughput of two or three"）。楽観的並行制御に変えると "agents became risk-averse"。planners（sub-planner を生む）/ workers（互いに調整しない）/ judge の階層で、約 1 週間・1,000 ファイル・100 万行超のブラウザを書いた。"Multi-agent coordination remains a hard problem"、"periodic fresh starts to combat drift and tunnel vision" がまだ要る。製品 UI には入っていない【一次】 — [Cursor blog](https://cursor.com/blog/scaling-agents)

**VS Code / GitHub**
- Agents window（Preview）: "a dedicated, agent-first Visual Studio Code window for assigning high-level tasks and tracking agent sessions across workspaces."。5 領域（Sessions list / Customizations / Chat / Changes / Files）。セッションは workspace か時間でグループ、カスタムグループ・pin・ドラッグ並べ替え。一覧は "the session's status and change statistics while it works" を出す。⌘1–9 は "Focus session by grid position"。Changes は branch / uncommitted / all / 最後の agent ターンを切り替え、"leave range-based feedback for the agent"、Commit / Sync / Mark Ready。subagent chat を横や縦に並べられる【一次】 — [Agents window docs](https://code.visualstudio.com/docs/agents/run/agents-window)
- 1.109（2026-01 リリース・ブログ 02-05）: "The Agent Sessions view gives you one place to see all your agent sessions—local, background, cloud"。Claude と Codex を Copilot 契約の下で local / cloud 実行。parallel subagents は "You can see which tasks are running, which agent is being used, and expand any subagent to see the full prompt and result."。姿勢は "Fire-and-forget when the task is clear, hands-on when you want control, or somewhere in between"【一次】 — [VS Code blog](https://code.visualstudio.com/blogs/2026/02/05/multi-agent-development)
- Agent HQ（2025-10-28）: mission control は GitHub・VS Code・mobile・CLI にまたがる一貫した面。branch controls・agent identity・"One-click merge conflict resolution"。Anthropic / OpenAI / Google / Cognition / xAI のエージェントを Copilot 契約で【一次】 — [GitHub blog](https://github.blog/news-insights/company-news/welcome-home-agents/)
- 2026-01-26 にリポジトリへ Agents タブ。セッションをクリックすると session log がリアルタイムで見え、途中で steering できる（"Steering consumes AI credits per message"）。"Open in VS Code" / "Continue in GitHub Copilot CLI" で手元へ【一次】 — [Agents tab changelog](https://github.blog/changelog/2026-01-26-introducing-the-agents-tab-in-your-repository/) / [Agent management docs](https://docs.github.com/en/copilot/concepts/agents/cloud-agent/agent-management)
- GitHub の運用ガイド（2025-12-01）: 並列向きは research / analysis / docs / security review / 別モジュールの作業、直列向きは依存あり・未知領域・仮説検証が要る問題。"Agents working in parallel can create merge conflicts if they touch the same files. Be thoughtful about partitioning work."。介入の合図は failing tests・範囲外ファイルの変更・scope creep・意図の誤解・同じ失敗の繰り返し。"Catch a problem five minutes in, and you might save an hour of ineffective work."。レビュー順は session logs → files changed → checks【一次】 — [How to orchestrate agents using mission control](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/)

**Google Antigravity**
- 2 面構成: Editor View（同期作業）と Agent Manager（複数エージェントを非同期に spawn して監督する mission control）。Artifacts は plan / task list / screenshot / browser recording などのレビュー用成果物【二次】 — [helply guide](https://helply.com/blog/google-antigravity-explained) / [Artifacts docs](https://antigravity.google/docs/artifacts/)
- Inbox（全 project の承認待ちを 1 か所に）は 2026-02-26 頃から見えなくなった。changelog に記載なし。"I loved the inbox feature…"、"managing parallel task execution" に重要だった、"please bring back the inbox" と反発が並び、Google の公式回答は無い。1 人が「サポートは chat history が大きすぎたためと説明した」と報告【一次: 公式フォーラム上の利用者投稿】 — [Forum thread](https://discuss.ai.google.dev/t/did-they-remove-the-inbox-in-agent-manager/126943)
- 【矛盾】2026 年の第三者ガイドには Inbox を現役機能として説明するものが残っている — [helply guide](https://helply.com/blog/google-antigravity-explained)
- 2.0（2026-05-19）: subagents は "built-in roles, generic clones … or dynamically registered on the fly"。orchestrator は "loops and polls the subagent states periodically"、応答は main agent の progress log に流れ、結果の会話は sidebar に出る。worktree は "autonomously creates the worktrees for those conversations and handles cleanup automatically"。sidebar は "group conversations by project, status, or recency, change the display name of projects, and toggle on worktree names"。ほかに scheduled tasks（cron）・hooks・voice【一次】 — [I/O 2026 deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive)
- CLI の `/agents` は Agent Manager Panel を開き、custom agent の選択と、背景 subagent の追跡・閲覧・停止を兼ねる【二次: 公式ページの検索要約のみ】 — [/agents docs](https://antigravity.google/docs/cli/commands/agents/)
- 2026-09 の更新: 通知トーストは右下に出て hover で開くコンパクトなスタックに。承認した権限は会話の残りで記憶【二次】 — [gradually.ai changelog](https://www.gradually.ai/en/changelogs/antigravity/)

**Zed**
- 2026-04-22 公開。Threads Sidebar は project でグループ化し、既定で左（Agent Panel の隣）、Project Panel と Git Panel は右へ。"Isolate worktrees, when you want to, and decide per thread."。哲学は "agentic engineering" = "combining human craftsmanship with AI tools to build better software"【一次】 — [Zed blog](https://zed.dev/blog/parallel-agents)
- docs: 行は title・status indicator・agent。Terminal Threads も terminal アイコンで同じ一覧に並ぶ。worktree は title bar の picker で管理。アーカイブは "saves the worktree's Git state and removes it from disk"、履歴から戻すと worktree も復元。ctrl-tab で最近の thread を巡回、shift-backspace でアーカイブ。sidebar は複数 project を同時に持てる【一次】 — [Parallel agents docs](https://zed.dev/docs/ai/parallel-agents)
- Terminal Threads は Zed 1.3.5（2026-05-20）で出荷【二次】 — [ChatForest review](https://chatforest.com/reviews/zed-1-0-ai-code-editor-parallel-agents-rust-review/)

**Kiro / Amp**
- Kiro autonomous agent（2025-12-02）: "up to 10 tasks concurrently"、内部は "one handles research and planning, another writes code, and a verification agent checks output before moving forward"、不確かなら質問、PR を開く、レビュー指摘を以後の作業に反映。Pro / Pro+ / Power 向けプレビュー【一次】 — [Kiro blog](https://kiro.dev/blog/introducing-kiro-autonomous-agent/)
- Kiro IDE の custom subagents は 2026-02-05 実装、並列実行可【二次】 — [dev.to aws-builders](https://dev.to/aws-builders/aws-parallel-execution-of-tasks-using-kiros-custom-subagents-kiro-n77)
- Amp Agents Panel（2026-01-09）: VS Code 拡張の左 navbar か Cmd-Opt-I。"j/k or arrow keys to move between threads"、"Space to expand a thread panel to show the last message or tool result"、Enter で開く、`e` でアーカイブ。見立ては "more time will be spent by humans in managing and orchestrating work across multiple agent threads"【一次】 — [Amp news](https://ampcode.com/news/agents-panel)
- Amp の Oracle（second-opinion ツール）と subagents（Search・Librarian・task）はセッション内の委任。CLI の `threads: map` で mention / handoff / fork で繋がった thread の俯瞰が出る。再構築版 Neo で handoff は廃止【二次: 検索結果の要約】 — [Amp guide](https://sidbharath.com/blog/amp-code-guide/) / [Amp, Rebuilt](https://ampcode.com/news/neo)

#### ツール別メモ: リモート操作と終了したもの

- Omnara: "The open-source alternative to Claude Managed Agents"。エージェントは手元で走り、phone / web / Apple Watch から監視・承認・音声入力。ノート PC がオフラインになるとセッションをクラウドへ移す。無料は月 10 セッション、$9/月で無制限、Apache 2.0【二次: 検索結果の要約】 — [omnara.com](https://www.omnara.com/) / [repo](https://github.com/omnara-ai/omnara)
- Happy: OSS の remote control（iOS / Android / web）、E2E 暗号化、無料。Omnara は self-host しない限り会話を平文でサーバーに置く、と Happy 側は比較している【一次: ベンダーの比較ページ】 — [Happy](https://happy.engineering/) / [alternatives](https://happy.engineering/docs/comparisons/alternatives/)
- Terragon: クラウドで Claude Code / Codex を走らせる background agent orchestrator。2026-02-09 に終了（十分な traction に届かず）。2026-01-16 時点の OSS スナップショットを公開、代替として Claude Code Web と Codex Web を推奨【一次】 — [Terragon shutdown](https://www.terragonlabs.com/) / [terragon-oss](https://github.com/terragon-labs/terragon-oss)
- Chorus は Melty Labs（Conductor の開発元）の Mac 用マルチ LLM chat アプリ。会社が Conductor に集中するため OSS 化された。並列エージェント管理ツールではない【二次】 — [meltylabs/chorus](https://github.com/meltylabs/chorus) / [Ben Congdon](https://benjamincongdon.me/blog/2025/12/13/Chorus-is-Good-Software/)

#### カンバンとインボックスの受け止められ方

- 否定側: Vibe Kanban 作者自身の "~50% of the columns seem pretty redundant" — [Show HN](https://news.ycombinator.com/item?id=44533004)
- 否定側: "kanban doesn't make sense for coding agents we tried it 6 months ago. every task just ended up in the "needs review" column"（投稿者の所属は未確認）【二次: 検索結果の要約】 — [X post](https://x.com/justinsunyt/status/2050051184766148992)
- 否定側: ACC を既定面にした Devin Desktop は個人には "unnecessary complexity"【二次】 — [aicoderscope](https://aicoderscope.com/blog/windsurf-devin-desktop-rebrand-acp-2026/)
- 肯定側: Nimbalyst は "the underlying problem of tracking multiple work items through stages is exactly the same" と主張し、Waiting と Review を独立した列にして自動で動かす — [Nimbalyst](https://nimbalyst.com/blog/claude-code-session-kanban-organize-ai-agents/)
- 中間: Conductor は板にせず、サイドバーを backlog / in progress / in review / done でグループ化した — [0.35.0](https://www.conductor.build/changelog/0.35.0-workspace-status)
- 事業面: Bloop は「毎日数千人が使うがほとんど無料ユーザー」でビジネスモデルを見つけられず終了した【二次: 検索結果の要約】 — [vibecoding.app](https://vibecoding.app/blog/vibe-kanban-review)
- インボックス: Antigravity の Inbox 消失への反発は上記の通り。Codex の Triage と Warp の通知メールボックスは現役 — [Forum](https://discuss.ai.google.dev/t/did-they-remove-the-inbox-in-agent-manager/126943) / [Codex KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/) / [Warp](https://docs.warp.dev/agents/capabilities/agent-notifications/)

### Inferences
- necoder の Fleet v2（左 = Task 一覧 / 中央 = 1 枚 / 並べるのは明示で 3 まで）は 2026 年の多数派と同じ型。Conductor・Codex app・Claude desktop・Zed・Air・Superset・Emdash・Devin がこの型で、外れているのはカンバン既定の Devin Desktop と Nimbalyst、ブラウザ板の Vibe Kanban だけ。
- 既存メモ `docs/research/fleet-ux-2026-09.md` の「グリッドに敷き詰めるのは GUI 勢は誰もやっていない」は 2026-09 時点では言い過ぎ。VS Code Agents window（grid position のショートカット）・Cursor 3 の Agent Tabs（side-by-side or grid）・Claude desktop（2 セッション横並び + ペイン自由配置）が「明示操作でのタイル」を持つ。既定が 1 枚である点は変わらないので、Fleet の「既定 1 枚・ピンで 2〜3 列」はこの流れと整合する。
- カンバンへの不満は「列の半分が空」と「全部が needs review に溜まる」の 2 点に集約される。板が悪いのではなく、エージェントの作業は in-progress が短く review が長いので、列の面積配分が実態に合わない。Conductor の「状態でグループ化した一覧」と necoder の「要対応を上段に常設 + Task 行」は同じ解に着いている。
- インボックスが惜しまれた理由は「プロジェクト横断で承認待ちだけを 1 か所に」だった点。necoder の要対応は選択中リポジトリ限定で、横断は titlebar のバッジとレールのドットに任せている。Antigravity の反発を見る限り、横断の要対応一覧（件数だけでなく中身）には需要がある。
- Sculptor が「worktree は面倒だからコンテナ」から worktree 既定へ移った（と読める）のは、ローカル並走の隔離は worktree で十分、コンテナは起動コストに見合わないという市場の判定を示す。

### Gaps
- OpenAI の Codex app 発表ページは 403 で、Codex app の UI は非公式ナレッジベースと検索要約に依存している。
- Antigravity 公式の Agent Manager docs は URL を特定できず（404）、2.0 の Manager のレイアウトは I/O ブログと二次情報からの再構成。Inbox が 2.0 で復活したかは確認できなかった。
- Emdash のカンバン表示、Devin Desktop の列名、Zed / Cursor 3 の行内グリフの種類は一次資料で確認できていない。
- Sculptor の worktree 移行と OSS 化の正確な日付は不明。
- カンバン UI の好悪を測った定量調査は見つからなかった（逸話のみ）。

## 2. エージェント状態をどう表し、どうやって人間の注意を引くか

### Takeaway
状態の語彙はどこも 4〜6 値（working / needs input / done-unread / failed + idle）に落ち着き、差が出るのは「注意の引き方」。効いている工夫は (a) 次の要対応へ 1 キーで跳ぶ、(b) 行に最新の一言やテキストのラベルを載せる、(c) 完了音と Dock バッジ、(d) 見ていないセッションだけ OS 通知、(e) orchestration 中は通知を親に集約、の 5 つ。

### Cited Findings
- Conductor: working アニメ / needs permission / unread dot。⌘⌥L で "Next chat needing attention"、選択中 workspace のクリックで最古の未読 chat へ。完了音は選択式（"Quack" を含む Novelty 群）【二次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270) / [Changelog](https://www.conductor.build/changelog)
- Conductor の experimental "Workspace status matrix" は routine runs / attention / errors / running PR checks / merged PR を別状態として描く（エージェント状態と PR 状態を 1 つのグリフに潰さない）【二次: 検索結果の要約】 — [Changelog](https://www.conductor.build/changelog)
- Conductor 0.77.0 は background task 待ちを transcript 内に "a dedicated timer and task details" で見せる【二次】 — [Releasebot](https://releasebot.io/updates/conductor)
- cmux: 待ちの pane に青リング、タブ点灯、行に最新通知の本文、⌘⇧U で最新の未読へ。通知は端末エスケープ（OSC 9/99/777）か `cmux notify` を agent hook に繋ぐ【一次】 — [cmux](https://github.com/manaflow-ai/cmux)
- Warp: Complete / Request / Error の 3 分類、トーストは同時 2 枚まで、通知メールボックスに All tabs / Unread / Errors のフィルタと "Mark all as read"、タブに状態アイコンと未読バッジ、背面時は OS 通知。orchestration 中は通知が親の会話にだけ出て、子の状態は pill bar で見る【一次】 — [Agent notifications](https://docs.warp.dev/agents/capabilities/agent-notifications/) / [Orchestration](https://docs.warp.dev/platform/orchestration/)
- Superset: "working indicators, completion chimes, and dock badges when one needs your attention"【一次】 — [Superset](https://github.com/superset-sh/superset)
- Devin: グリフではなくテキストの永続ラベル（"PR created" / "Awaiting instructions" / "Approve session"）+ タイムスタンプ。"Approve session" の注意ドットは読むと消える【一次 / 二次】 — [Release notes 2026](https://docs.devin.ai/release-notes/2026)
- Devin Desktop: 実行中のセッションは "greyed out and are read-only"、通知は完了か入力待ちで "a single native OS notification"【一次】 — [ACC docs](https://docs.devin.ai/desktop/agent-command-center)
- Claude desktop: OS 通知は「終わった かつ そのセッションを見ていない」時だけ。CI 完了でも通知。Dispatch 起動分は携帯へ push。sidebar は status / project / environment でフィルタ【一次】 — [Desktop docs](https://code.claude.com/docs/en/desktop)
- Claude Code agent teams（端末）: 入力欄の下の agent panel に teammate の行。idle が 3 行を超えると "2 idle agents" のように畳む。全員 idle になって 30 秒で idle 行を隠す。working / failed / 閲覧中の行は常に残す【一次】 — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Codex: automation の結果は Triage に既読/未読つきで溜まり、所見なしは自動アーカイブ。CLI は非アクティブなスレッドの承認要求も表に出し `o` で開く【二次 / 一次】 — [Codex KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/) / [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- ccmanager は Idle / Busy / Waiting + 状態変化 hook、Agent of Empires は running / waiting / idle / errored、Nimbalyst は running / completed / failed / paused + カンバンの Waiting 列【一次 / 二次】 — [ccmanager](https://github.com/kbwo/ccmanager) / [AoE](https://vibecodinghub.org/blog/agent-of-empires-review) / [Nimbalyst](https://nimbalyst.com/blog/claude-code-session-kanban-organize-ai-agents/)
- VS Code Agents window は行に status と変更統計、Zed は行に status indicator。どちらも docs に通知の仕組みの記述は無い【一次】 — [VS Code](https://code.visualstudio.com/docs/agents/run/agents-window) / [Zed](https://zed.dev/docs/ai/parallel-agents)
- JetBrains Air は別 task が要対応になると通知し、その間もエージェントは走り続ける【一次】 — [JetBrains](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/)
- モバイル: Nimbalyst・Omnara・Happy・Claude（Dispatch）が「完了 / 承認待ち」を push し、その場で承認させる【一次 / 二次】 — [Nimbalyst mobile](https://docs.nimbalyst.com/mobile/mobile-app) / [Omnara](https://www.omnara.com/) / [Desktop docs](https://code.claude.com/docs/en/desktop)
- 10 体超を回した開発者の記録（2026-07-22）: 最初に困ったのは "What is every agent doing right now?"。提案は Running / Confirm / Input の 3 値に絞り、判断が要るセッションだけに目を向けさせること。エージェントに定型の報告（完了事項・検証結果・未決事項）を出させる案も — [dev.to kikakkz](https://dev.to/kikakkz/i-ran-10-ai-coding-agents-in-parallel-the-bottleneck-wasnt-the-ai-12e3)

### Inferences
- 「次の要対応へ跳ぶ」の行き先は 2 流派ある。Conductor は最古の未読、cmux は最新の未読。承認待ちは待たせた時間がそのままエージェントの遊休になるので、Blocked を経過時間順に並べる Fleet の要対応は Conductor 側（最古優先）で、⌘⇧U の行き先も「最新」ではなく「要対応の先頭」に揃えるのが筋。
- Devin のテキストラベルと Conductor の status matrix は、どちらも「エージェントの活動状態」と「Git / PR の状態」を別チャンネルで見せている。Fleet v2 の「ThreadActivity は状態グリフ、Git health は radar 補足」と同じ規律で、業界でも潰さない方向。
- 「見ていないセッションだけ通知」（Claude desktop）と「トースト同時 2 枚まで」（Warp）は、N 体並走で通知が洪水になる問題への最小限の答え。Fleet は舞台に出ている Task の完了通知を抑え、OS 通知の同時数に上限を置くとよい。
- Warp の「orchestration 中は通知を親だけに」は Captain にも使える。Captain が起こした Task の Done / Failed は Captain の報告 1 通にまとめ、人間に直接届けるのは人間が gate の Blocked（承認待ち）だけにすると、通知は Captain の采配 1 本 + 承認 N 本に減る。
- 音と Dock バッジを持つのは Conductor と Superset。necoder の設計文書に音は無い。目を離している時間を埋めるのが Fleet の目的なら、完了音（選べる・消せる）は安い追加。

### Gaps
- Cursor 3・Zed・VS Code の通知挙動（OS 通知 / 音 / バッジ）の一次記述を取れていない。
- 通知の種類ごとの効果（どれが反応時間を縮めるか）を測った公開データは無い。

## 3. 隔離の仕組み（worktree / コンテナ / クラウド VM・準備スクリプト・ポート・片付け）

### Takeaway
ローカル並走の隔離は git worktree が事実上の標準になり、コンテナは opt-in、クラウド VM は「PC を閉じても続く非同期作業」用に住み分けた。worktree の 3 大問題（未追跡ファイル・ポート衝突・溜まる worktree）への答えも揃ってきていて、`.worktreeinclude`、setup / run / archive の 3 script、`$XXX_PORT` の注入、アーカイブ時の状態保存 + ディレクトリ削除が共通解。

### Cited Findings
- worktree 採用: Conductor / Codex app / Claude desktop / Cursor / Zed / VS Code（background agents）/ JetBrains Air / Superset / Emdash / Nimbalyst / Vibe Kanban / Claude Squad / ccmanager / Agent of Empires / Antigravity 2.0 / Augment Intent / 現行 Sculptor — [Conductor](https://www.conductor.build/docs/concepts/workspaces-and-branches) / [Claude desktop](https://code.claude.com/docs/en/desktop) / [Cursor 2.0](https://cursor.com/changelog/2-0) / [Zed](https://zed.dev/docs/ai/parallel-agents) / [VS Code](https://code.visualstudio.com/docs/agents/run/agents-window) / [Air](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/) / [Antigravity](https://antigravity.google/blog/google-io-2026-feature-deep-dive) / [Sculptor](https://imbue.com/product/sculptor)
- コンテナ: Air の Docker（`.air/docker.json`）、Agent of Empires の Docker / Podman / Apple Containers、ccmanager の devcontainer、Sculptor の Container Backend（experimental）。Sculptor の第三者評では最初のエージェント起動に約 1 分【二次】 — [recca0120](https://recca0120.github.io/en/2026/05/06/jetbrains-air-agentic-ide/) / [AoE](https://vibecodinghub.org/blog/agent-of-empires-review) / [ccmanager](https://github.com/kbwo/ccmanager) / [Ry Walker](https://rywalker.com/research/sculptor)
- クラウド VM / sandbox: Devin（子ごとに VM）、Cursor cloud agents、Codex cloud、Claude Code on the web、Conductor Cloud（2026-07-30）、Warp Oz、Kiro（sandbox・同時 10 task）、Air の cloud（tech preview） — [Devin](https://cognition.com/blog/devin-can-now-manage-devins) / [Cursor 3](https://cursor.com/blog/cursor-3) / [Conductor](https://www.conductor.build/changelog) / [Warp](https://docs.warp.dev/platform/orchestration/) / [Kiro](https://kiro.dev/blog/introducing-kiro-autonomous-agent/)
- local⇄cloud の handoff: Cursor 3（双方向）、Codex（Hand off で Local⇄Worktree）、Claude（`--remote` / `--teleport`）、Devin Desktop（1 クリックで Devin へ）、Omnara（PC がオフラインになるとクラウドへ移す）【一次 / 二次】 — [Cursor 3](https://cursor.com/blog/cursor-3) / [Codex KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/) / [Windsurf 2.0](https://devin.ai/blog/windsurf-2-0)
- 未追跡ファイル: Claude Code は `.worktreeinclude` で gitignore 済みファイルを新 worktree に持ち込む。Conductor も `.worktreeinclude` か `file_include_globs` を読む【一次 / 二次】 — [Desktop docs](https://code.claude.com/docs/en/desktop) / [infinitus #270](https://github.com/deathemperor/infinitus/issues/270)
- script: Conductor は `scripts.setup` / `scripts.run.<id>` / `scripts.archive`、Superset は setup / teardown / run、Emdash は setup / run / teardown、Air は `.air/worktree.json`【二次 / 一次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270) / [Superset](https://github.com/superset-sh/superset) / [Show HN: Emdash](https://news.ycombinator.com/item?id=47140322)
- ポート: Emdash は `$EMDASH_PORT` を task ごとに注入、Superset は workspace ごとのポート検出つき内蔵ブラウザ、cmux は行に listening ports を出す — [Show HN: Emdash](https://news.ycombinator.com/item?id=47140322) / [Superset](https://github.com/superset-sh/superset) / [cmux](https://github.com/manaflow-ai/cmux)
- 起動速度: Emdash は worktree を事前に温めて 500–1000 ms、Codex は約 0.8 秒・約 120 MB / worktree【一次 / 二次】 — [Show HN: Emdash](https://news.ycombinator.com/item?id=47140322) / [Codex KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/)
- 片付け: Conductor は archive で未コミット分を commit として保存してからディレクトリ削除。Zed は worktree の git 状態を保存して disk から消し、復元で戻す。Codex は管理 worktree を直近 15 本だけ保持。Claude desktop は PR の merge / close で自動アーカイブ。Antigravity は worktree の cleanup も自動【二次 / 一次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270) / [Zed docs](https://zed.dev/docs/ai/parallel-agents) / [Codex KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/) / [Claude blog](https://claude.com/blog/claude-code-desktop-redesign) / [Antigravity](https://antigravity.google/blog/google-io-2026-feature-deep-dive)
- worktree の置き場: Conductor は `~/conductor/workspaces/<repo>/<workspace>`（repo の外）、Claude は `<project-root>/.claude/worktrees/`（repo の中・変更可）【二次 / 一次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270) / [Desktop docs](https://code.claude.com/docs/en/desktop)
- monorepo 対策として Conductor は `git sparse-checkout` を使う【二次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270)
- 資源: Vibe Kanban 作者の MacBook は 4 並列で重くなる（主因は rustc）。大きい repo では worktree の肥大が弱点 — [Show HN](https://news.ycombinator.com/item?id=44533004) / [Augment](https://www.augmentcode.com/tools/open-source-agent-orchestrators)
- 初期 Conductor は既存 checkout を使わず clone する方式で、依存の再インストールや外部 API 設定が重い project では不評だった（HN・2025-07） — [HN: Conductor](https://news.ycombinator.com/item?id=44594584)

### Inferences
- `.worktreeinclude` は Claude Code と Conductor の 2 社が読む、事実上のクロスツール規約になりつつある。necoder の `.necoder/worktree-setup.sh` は自由度で勝るが、`.worktreeinclude` があればそれも読む（setup script の前にコピーする）と、他ツールから来た repo がそのまま動く。
- 「archive = 状態を保存してディレクトリを消す、復元で戻す」（Zed / Conductor）は、Fleet の片付けメニュー 5 段より一段単純な心的モデル。`target/` を worktree ごとに持つ Rust ではディスク回収の効果が大きい。
- ビルド資源の衝突（rustc で 4 並列が限界）は Rust の repo に固有の上限で、necoder 自身がこれに当たる。`CARGO_TARGET_DIR` 共有で `cargo test` が直列化するという既定の副作用は、並列数の実効上限を UI より先に決める可能性がある。
- ポート注入は 3 ツールが別名で実装している。`.necoder/task.env` に `NECODER_PORT`（Task ごとに一意）を自動で書く一行を足せば同じ問題を解ける。

### Gaps
- `CONDUCTOR_PORT` 相当の変数名は一次資料で確認できていない（Zenn 記事で `$CONDUCTOR_ROOT_PATH` のみ確認）。
- worktree と共有ビルドキャッシュ（Rust の target・node_modules）を製品として解いているツールは見つからなかった。各社とも setup script 任せ。
- Windows の長いパス問題に触れた一次資料は無い。

## 4. レビューと統合（diff・インラインコメント・PR・マージキュー・並列 Task 間の衝突検知）

### Takeaway
「diff にインラインコメントを付けて、まとめてエージェントに返す」はほぼ全 GUI が持つ標準装備になった。2026 年の差分は (a) 人間の前にエージェントが 1 回レビューする、(b) PR 後の CI を auto-fix / auto-merge する、(c) 依存する Task を stack する、の 3 つ。並列 Task 同士の衝突を統合前に予測する UI を第一級で持つツールは見つからず、衝突は「起きてからエージェントに直させる」のが主流。

### Cited Findings
- インラインコメント → エージェント: Conductor（"Inline comments become composer attachments"）、Claude desktop（行クリック → ⌘⏎ で一括送信）、Vibe Kanban、VS Code（"range-based feedback"）、Codex（review pane）、Antigravity（Artifacts へのコメント）、Conductor MCP 経由ではエージェントが diff にコメントを残せる — [Conductor workflow](https://www.conductor.build/docs/concepts/workflow) / [Claude desktop](https://code.claude.com/docs/en/desktop) / [Vibe Kanban review](https://vibecoding.app/blog/vibe-kanban-review) / [VS Code](https://code.visualstudio.com/docs/agents/run/agents-window) / [Releasebot](https://releasebot.io/updates/conductor)
- diff 内で直接編集: Conductor 0.84.0（2026-09-02）、Superset の diff ビューア。Conductor は one-click revert を Claude に通知して "you don't have to explain yourself"【一次 / 二次】 — [Changelog](https://www.conductor.build/changelog) / [Superset](https://github.com/superset-sh/superset) / [infinitus #270](https://github.com/deathemperor/infinitus/issues/270)
- エージェントによる事前レビュー: Claude desktop の "Review code"（compile error・明確なロジックエラー・脆弱性・明白なバグに絞り、style や既存の問題は指摘しない）、Conductor の `Review` アクション、Sculptor の Suggestions（beta）、Verdent の Review Subagent（3 モデルで相互検証）、Intent の verifier、GitHub の agent workflow 内 code review step【一次 / 二次】 — [Claude desktop](https://code.claude.com/docs/en/desktop) / [Conductor workflow](https://www.conductor.build/docs/concepts/workflow) / [Ry Walker](https://rywalker.com/research/sculptor) / [Verdent](https://www.verdent.ai/guides/multi-agent-coding-tools) / [Intent](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration) / [Agent HQ](https://github.blog/news-insights/company-news/welcome-home-agents/)
- PR と CI: Conductor は ⌘⇧P で PR、Checks タブ、sidebar に merged / failing CI / conflicts。Claude desktop は CI status bar に Auto-fix と Auto-merge（squash・GitHub 側の auto-merge 設定が前提）。Emdash は GitHub Actions の check を app 内に表示。VS Code は Commit / Sync / Mark Ready【一次】 — [Conductor workflow](https://www.conductor.build/docs/concepts/workflow) / [Claude desktop](https://code.claude.com/docs/en/desktop) / [Emdash](https://emdash.com/docs/) / [VS Code](https://code.visualstudio.com/docs/agents/run/agents-window)
- 依存する Task: Conductor 0.80.0（2026-08-10）が GitHub Stacks を内蔵。Vibe Kanban では「task が互いに依存する」「stack して review する方法が要る」が未解決の要望として残った — [Changelog](https://www.conductor.build/changelog) / [Show HN](https://news.ycombinator.com/item?id=44533004)
- 衝突: GitHub は "One-click merge conflict resolution"。Conductor は「エージェントに解かせてからテストを回し直し diff を見直す」と書く。Sculptor は衝突を検知してエージェントに戻す。NTM は "conflict detection" を掲げる。Gas Town は merge queue 専任の Refinery を置く。MultiDevin / Managed Devins は manager が成功した worker の変更を 1 つの branch / PR にまとめ、衝突も manager が解く【一次 / 二次】 — [Agent HQ](https://github.blog/news-insights/company-news/welcome-home-agents/) / [Conductor workflow](https://www.conductor.build/docs/concepts/workflow) / [Ry Walker](https://rywalker.com/research/sculptor) / [NTM](https://vibecoding.app/blog/ntm-review) / [Gas Town](https://codex.danielvaughan.com/2026/04/08/gas-town-multi-agent-factory/) / [Devin](https://cognition.com/blog/devin-can-now-manage-devins)
- 予防の指針は「ファイル所有を分ける」: GitHub は "Be thoughtful about partitioning work"、Claude agent teams は "Two teammates editing the same file leads to overwrites. Break the work so each teammate owns a different set of files."、Codex は write-heavy な並列に注意を促す — [GitHub](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/) / [Agent teams](https://code.claude.com/docs/en/agent-teams) / [Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- best-of-N（同じ task を複数モデルで走らせて選ぶ）: Cursor の `/best-of-n`、Codex cloud の `--attempts`（最大 4）、Emdash の diff 横並び、Windsurf の Arena Mode【二次】 — [AgentPatterns](https://www.agentpatterns.ai/tools/cursor/agents-window/) / [agent37](https://www.agent37.com/blog/codex-cloud) / [Emdash](https://emdash.com/docs/)
- 検証の証拠を出させる: Cursor の cloud agent は demo と screenshot、Antigravity は browser 録画と screenshot の Artifacts、Claude desktop は Browser ペインで自動検証（screenshot・DOM・クリック）【一次 / 二次】 — [Cursor 3](https://cursor.com/blog/cursor-3) / [Artifacts](https://antigravity.google/docs/artifacts/) / [Claude desktop](https://code.claude.com/docs/en/desktop)
- checkpoint / 巻き戻し: Conductor はターンごとに private git ref、0.83.0 で "Reset chat now rewinds only chat by default, with 'Also reset files' checkbox"【二次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270)
- Superset の計画: 人間の前に品質ゲートを重ねる（adversarial agents・積み重ねた review agents・ブラウザでの自動テスト）、エージェントが要約 / preview / テスト結果を付けて差し出す UI — [Superset blog](https://superset.sh/blog/roadmap-to-100-agents)

### Inferences
- Fleet の Conflict Radar（統合前に read-only で衝突を予測する）に当たるものを第一級の UI として持つツールは、今回の調査範囲では見つからなかった。各社は「起きた衝突をエージェントに直させる」か「PR 上の conflicts 表示」止まり。ここは necoder の差別化点として残っている（見つからなかっただけで、存在しない証明ではない）。
- Integrate を人間だけに残す necoder の線引きは業界でも保守側。Devin と Warp は調整役に merge させ、人間の gate を最後の PR に置く。中間案は「Captain が統合ブランチ / 統合 PR の下書きまで作り、main への Integrate は人間」。
- CI の Auto-fix / Auto-merge（Claude desktop）と routines（Conductor・Codex automations・Antigravity scheduled tasks）は、レビュー帯域の外で進められる仕事を増やす方向。Fleet の phase に当てはめると「レビュー待ち → 統合済み」の間に「checks 待ち」が入る余地がある。
- 依存 Task の stack は Conductor が GitHub Stacks に寄せて解いた。Fleet の `depend` / `wait-deps` は順序だけを扱うので、「Task B を Task A の branch から切る」を ＋Task の詳細に足すと同じ穴が埋まる。
- 「Review code」が指摘範囲を明示的に絞っている点は設計の参考になる。Captain の承認推薦や radar の出力も「何を言わないか」を決めると読む量が減る。

### Gaps
- 並列 Task 間の衝突を統合前に可視化する UI の例（ファイル重複ヒートマップ等）は見つからなかった。
- merge queue を GUI に持つ個人向けツールは確認できなかった（Gas Town の Refinery はエージェント役であり UI ではない）。
- agent による事前レビューが人間のレビュー時間をどれだけ減らすかの公開データは無い。

## 5. 内蔵の調整役（coordinator / manager / planner）を持つのは誰か。UI でどう見せているか

### Takeaway
2026-09 時点で、目標を分解して他のエージェントを spawn・監視する調整役を製品として持つのは Devin（Managed Devins）・Warp（`/orchestrate` + Oz）・Augment Intent（Coordinator）・Antigravity 2.0（dynamic subagents）・Kiro（内部 3 役）・Gas Town（Mayor）。Claude Code の agent teams は experimental で CLI 限定、Cursor の planner / worker は研究段階。ワークツリー管理系（Conductor・Superset・Emdash・Vibe Kanban・cmux・Zed・Air・Claude Squad 等）は人間が唯一の調整役で、代わりに CLI / MCP を開けて「どのエージェントでも調整役になれる」形にしている。見せ方は例外なく「特別な chat」で、足される UI は pill bar・入れ子ツリー・共有 spec の 3 種。

### Cited Findings
- **Devin**: 調整役は通常の Devin セッション。"scopes the work, assigns each piece to a managed Devin, monitors progress, resolves any conflicts, and compiles the results"。子へのメッセージ・ACU 監視・sleep / terminate・自分宛ての予約メッセージ。子は sidebar で親の下に tree connector つきで入れ子。人間は子の session link から直接覗いてメッセージできる。用途は並列 QA・大規模 migration・security audit・refactor【一次】 — [Cognition](https://cognition.com/blog/devin-can-now-manage-devins) / [Release notes](https://docs.devin.ai/release-notes/2026)
- **Warp / Oz**: 親 1 段。`/orchestrate` は分解案を提示し "Approval is required before the parent launches children"。`/plan` は inline の orchestration config（model / harness / environment / host / parallelism）を出す。UI は pill bar（親が左・子ごとに pill・クリックで子の会話へ）と Oz web の Sub-agents タブ。通知は親だけ。message bus は harness 非依存・場所非依存で、終了した子も ID 宛てで起こせる【一次】 — [Warp orchestration](https://docs.warp.dev/platform/orchestration/)
- **Augment Intent**: Coordinator が spec を起こし、人間が承認、implementor が waves で並走、verifier が spec と照合。人間は spec を編集でき、どのエージェントにも直接話せる。"living spec" は作業の進行で更新され、全エージェントが同じ状態を共有する【一次 / 二次】 — [Intent blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration) / [Ry Walker](https://rywalker.com/research/augment-intent)
- **Antigravity 2.0**: main agent が subagent を "dynamically registered on the fly" で定義して spawn、"loops and polls the subagent states periodically"、応答は main の progress log に流れ、結果の会話は sidebar に出る。subagent は main の tool 設定と権限を継承【一次】 — [I/O deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive)
- **Kiro autonomous agent**: research/planning・code・verification の 3 sub-agent を内部で協調させる。利用者には 1 つの task として見える【一次】 — [Kiro](https://kiro.dev/blog/introducing-kiro-autonomous-agent/)
- **Claude Code agent teams**（experimental・`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`）: team lead + teammates + shared task list（pending / in progress / completed・依存つき・file lock で claim）+ mailbox（`~/.claude/teams/{team-name}/inboxes/{agent-name}.json`）。表示は in-process（入力欄の下の agent panel で ↑↓ と Enter）か split panes（tmux / iTerm2）。人間はどの teammate にも直接話せる。teammate の権限要求は lead のセッションに上がり人間が答える。plan 承認だけは lead が自動で通す。"A teammate can't approve a permission prompt or supply consent on your behalf"。制約: 1 セッション 1 team・入れ子不可・lead 固定・resume で teammate は戻らない・"Task status can lag"。推奨は "Start with 3-5 teammates"、1 人あたり 5–6 task。既知の失敗は「lead が待たずに自分で実装し始める」「lead が早く終わったと判断する」。quality gate は `TeammateIdle` / `TaskCreated` / `TaskCompleted` hook【一次】 — [Agent teams docs](https://code.claude.com/docs/en/agent-teams)
- **Claude desktop**: agent teams は使えない。代わりに Claude が他セッションを一覧し、読み、メッセージを送れる。受信メッセージは引用され送信元が明示される。アーカイブ前は人間に確認【一次】 — [Desktop docs](https://code.claude.com/docs/en/desktop)
- **Codex**: subagents は GA。"Codex handles orchestration across agents, including spawning new subagents, routing follow-up instructions, waiting for results, and closing agent threads"。web の sidebar は活動を報告するだけで個別の停止や steer はできない。IDE 拡張は background-agent panel から状態確認・停止・個別 thread を開ける【一次】 — [Subagents docs](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- **Cursor**: 製品は `/multitask`（async subagents が要求を並列化し、大きい task を分割）。研究では planners / sub-planners / workers / judge。フラットな自己協調は失敗し階層が効いた。"Multi-agent coordination remains a hard problem"【一次】 — [Cursor 3.2](https://cursor.com/changelog/04-24-26) / [Scaling agents](https://cursor.com/blog/scaling-agents)
- **VS Code**: セッション内の parallel subagents を展開して prompt と結果を見られる。セッション横断の調整役は無い【一次】 — [VS Code blog](https://code.visualstudio.com/blogs/2026/02/05/multi-agent-development)
- **Gas Town**: 人間は Mayor と話し、Polecats が使い捨てで働き、Refinery が merge queue を回す。20–30 並列【二次】 — [Codex KB](https://codex.danielvaughan.com/2026/04/08/gas-town-multi-agent-factory/)
- **cmux-team**（コミュニティ）: Master（人間と対話して task を作る）→ Manager（idle で待ち、task を見つけて Conductor を起こす）→ Conductor（worktree で自律実行）→ Agent — [Zenn hummer](https://zenn.dev/hummer/articles/cmux-ecosystem-claude-code)
- **調整役を持たず、API を開ける派**: Conductor（cloud workspace に CLI 同梱・0.80.0、Conductor MCP・0.82.0: workspace の作成 / 送信 / 監視、diff へのインラインコメント、「別の workspace を作って X を調べさせる」）、Superset（CLI / MCP / SDK）、Vibe Kanban（MCP server でエージェントが板にチケットを作れる。カード移動と板の状態取得までできるという記述は検索要約のみで出所未特定）、cmux（CLI + socket）【二次 / 一次】 — [Releasebot](https://releasebot.io/updates/conductor) / [Superset](https://github.com/superset-sh/superset) / [Show HN: Vibe Kanban](https://news.ycombinator.com/item?id=44533004) / [cmux](https://github.com/manaflow-ai/cmux)
- **意図的に持たない派**: Emdash の創業者は "CLIs themselves are getting good at [agent coordination] natively" として task レベルに留まる。比較記事の評は "No agent-to-agent coordination; teams must layer additional tooling" — [Show HN: Emdash](https://news.ycombinator.com/item?id=47140322) / [Augment](https://www.augmentcode.com/tools/open-source-agent-orchestrators)
- **LLM を使わない調整**: Bernstein は "deterministic scheduling" で調整に LLM を呼ばず、merge 前に Janitor が検証。Agent Orchestrator（旧 Composio）は CI 失敗とレビューコメントに自動で反応する human-on-the-loop。Microsoft Conductor は YAML の workflow【二次】 — [Augment](https://www.augmentcode.com/tools/open-source-agent-orchestrators)
- Anthropic の 2026 Agentic Coding Trends Report は、orchestrator が専門 sub-agent を並列に束ねる構成への移行を 2026 年の主題に据える【二次: 検索結果の要約】 — [Anthropic report](https://resources.anthropic.com/2026-agentic-coding-trends-report)

### Inferences
- 調整役の見せ方は全社「特別な chat」。ダッシュボードやタイムラインを主にした例は無い。足される UI は Warp の pill bar（親子の横断）・Devin の入れ子ツリー（帰属）・Intent の living spec（共有の成果物）の 3 つで、Fleet v2 の「ブリッジ = main の Captain スレッド + 編隊図 + 采配ログ」は同じ系統にいて、采配ログ（監査できる履歴）を持つ分だけ先にある。
- Captain の設計に最も近い競合は Conductor MCP / CLI（2026-08）。「どのエージェントでも調整役になれる」という Captain の差別化は、Conductor が API を開けたことで薄まった。残る差は (1) Captain という役割・権限表・wake 条件を製品が定義している、(2) 台帳に状態を置くので交代が自由、(3) Integrate と承認は人間だけ、の 3 点。
- 「fan-out の前に人間が分解案を承認する」は Warp と Intent が揃って採用し、Claude agent teams は採らず（lead が自動承認）、Devin も自動。Fleet v2 の Captain は `fleet create` を無確認で叩ける設計なので、分解案を 1 枚のカードにして人間が承認してから spawn する段（Warp の orchestration config ブロック相当）を足す価値がある。Task 数・worktree 数・トークンが一気に増える操作だから。
- Claude agent teams の既知の失敗（lead が自分で実装し始める・早く終わったと判断する・task 状態が遅れる）は Captain でも起きうる。「Captain はコードを書かない」は 1 つ目への構造的な答えになっている。3 つ目は、台帳を Task の自己申告ではなく necoder 側の観測（ThreadActivity・git）で更新する設計が効く。
- Cursor の研究結論（フラットな自己協調は失敗・階層が要る・定期的な fresh start が要る）は、状態を持たない Captain + 台帳という設計を支持する。Captain を再起動しても続きが読めることが fresh start の前提。
- 権限の線引きは Claude Code の「他エージェントからのメッセージは人間の同意ではない・teammate は代理承認できない」と Fleet v2 の「Captain は推薦のみ」が一致。Warp も承認は人間。調整役に承認を代行させる製品は今回見つからなかった（ccmanager の auto-approval は別種で、調整役ではなく判定器）。
- 調整役の深さは 1 段が主流（Warp "exactly one level deep"・Claude "No nested teams"）。Cursor の研究だけが sub-planner を許す。Captain → Task の 1 段に留めるのは妥当。

### Gaps
- 内蔵オーケストレーターの実利用率（何割の利用者が使うか）や成功率の公開データは無い。
- Devin の子セッション数の上限、Warp の子の上限、Warp orchestration の出荷日は一次資料で確認できなかった（MultiDevin の「最大 10」は 2024 年の情報）。
- Conductor MCP を使ってエージェントに調整させた実運用の報告は見つけていない。
- Antigravity の Agent Manager 上で subagent 群がどう描かれるか（ツリーか平置きか）は未確認。

## 6. 1 人が現実に監督できる並列数と、最初に壊れるもの

### Takeaway
公開されている数字はほぼ逸話と製品上限だが、驚くほど揃っている。能動的に対話できるのは 2〜3 本、監督できるのは 3〜5 本、熟練チームで 5〜7 本、製品の上限は 5〜10。最初に壊れるのはレビュー帯域、次が「どれが自分を待っているか」の把握（コンテキストスイッチ）、その次がマージ衝突、最後がマシン資源。

### Cited Findings
- Simon Willison（2025-10-05）: "the natural bottleneck on all of this is how fast I can review the results"、"I can only focus on reviewing and landing one significant change at a time"【二次: 検索結果の要約】 — [simonwillison.net](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/)
- Armin Ronacher: "I sometimes kick off parallel agents, but not as much as I used to do. The thing is: it's only so much my mind can review!"。Pragmatic Engineer は、並列運用がうまくいくと語るのは今のところ senior 以上だけだと観察 — [Pragmatic Engineer](https://newsletter.pragmaticengineer.com/p/new-trend-programming-by-kicking)
- Addy Osmani（2026-01-08）: 自身は background 4–5 本 + human-in-the-loop 3–5 本。"closer to a handful of background agents" を推奨。壊れる順は review → merge conflicts → context switching。対策は WIP 制限・詳細な brief・kill criteria・約 15 分の check-in。Boris Cherny は local 5 本 + web / mobile でさらに多数（Osmani 記事の要約では 10–15。Cherny の原典は未確認）— [Addy Osmani](https://addyosmani.com/blog/coding-agents-manager/)
- Superset チーム（2026-02-02）: "we're able to reliably manage 5-7 coding agents in parallel"、"most of our agents spend more time waiting for us to review their work than they spend doing it"、"You can't review 100 diffs a day" — [Superset blog](https://superset.sh/blog/roadmap-to-100-agents)
- 10 体超を複数マシンで回した開発者（2026-07-22）: "The bottleneck became me"。2–3 体を超えると状況が掴めなくなり、監視に時間を取られる — [dev.to kikakkz](https://dev.to/kikakkz/i-ran-10-ai-coding-agents-in-parallel-the-bottleneck-wasnt-the-ai-12e3)
- 日本語記事（2026-02-23）の目安: 並列 2 = 推奨 / 3 = 良好 / 4–5 = 上級者向け / 6+ = 非推奨。「個人開発では2-3が現実的」「アクティブに対話するのは2つまで」、worktree は「3-4個をアクティブ上限」 — [Zenn amu_lab](https://zenn.dev/amu_lab/articles/cmux-claude-code-parallel-guide)
- Anthropic の docs: agent teams は "Start with 3-5 teammates"、"Three focused teammates often outperform five scattered ones" — [Agent teams](https://code.claude.com/docs/en/agent-teams)
- GitHub mission control は task switcher で 3–5 の同時セッションを捌く想定【二次】 — [AgentPatterns](https://agentpatterns.ai/tools/copilot/agent-mission-control/)
- 製品の上限: Cursor 2.0 は 1 プロンプトで最大 8、Windsurf Wave 13 は最大 5、Kiro は同時 10 task、MultiDevin は worker 最大 10【一次 / 二次】 — [Cursor 2.0](https://cursor.com/changelog/2-0) / [Wave 13](https://aiautomationglobal.com/blog/windsurf-wave-13-parallel-agents-arena-mode-ai-ide-2026) / [Kiro](https://kiro.dev/blog/introducing-kiro-autonomous-agent/) / [Cognition on X](https://x.com/i/status/1836866702182863286)
- HN（Conductor・2025-07）: petesergeant "I can just about manage to stay focused enough to properly review...a single Claude agent."、oc1 "the bottleneck was...never the ai not being able to write 10 different poc simultaneously—but the human factor—needing to carefully review what ai produced" — [HN](https://news.ycombinator.com/item?id=44594584)
- HN（Kanbots・2026）: cold_harbor "the bottleneck moves from generation to review...8 parallel cards means 8x the diffs to read, none of the timelines overlap."、d4rkp4ttern "the fraction of human reviewed code is approaching zero far faster than anyone is realizing" — [HN](https://news.ycombinator.com/item?id=48239413)
- HN（Emdash）: nerder92 はエージェント主導に向くのは task の "20 to 30%" だけと報告 — [HN](https://news.ycombinator.com/item?id=47140322)
- 組織データ（Faros AI・2025-06・開発者 1 万人超 / 1,255 チーム）: AI 高活用チームは task 完了 +21%、merge した PR +98%、PR レビュー時間 +91%、PR サイズ +154%、バグ +9%【二次: 検索結果の要約】 — [Faros AI](https://www.faros.ai/blog/ai-software-engineering)
- Anthropic の 2026 レポート: 開発者は仕事の約 60% で AI を使うが、"fully delegate" できるのは 0–20%【二次: 検索結果の要約】 — [Anthropic report](https://resources.anthropic.com/2026-agentic-coding-trends-report)
- 「実効上限は 3–5。それを超えると codebase 自体（merge 工程）がボトルネック」とする記述が検索要約にあったが、出所ページを特定できていない【二次・出所未特定】 — [Codex KB: human review bottleneck](https://codex.danielvaughan.com/2026/05/24/human-review-bottleneck-code-review-strategies-agent-output/)
- 資源面: Vibe Kanban 作者の MacBook は 4 並列で重くなる（主に rustc） — [Show HN](https://news.ycombinator.com/item?id=44533004)
- Cognition 自身の言葉: "The more agents you manage, the faster you hit the limits of your own attention and working memory." — [Windsurf 2.0](https://devin.ai/blog/windsurf-2-0)
- 効率の一例: 並列化でトークン消費は約 1.5 倍、完了 task は約 1.6 倍という日本語記事の報告【二次: 検索結果の要約・出所記事未特定】 — [Zenn amu_lab](https://zenn.dev/amu_lab/articles/git-worktree-claude-code-2026)

### Inferences
- 舞台の上限を 3 枚にした Fleet v2 の判断は、「能動的に対話できるのは 2〜3 本」という複数の独立した証言と一致する。サイドバーに並ぶ Task が 5〜7 を超えたあたりが 1 人の上限で、それ以上は調整役（Captain）か自動の品質ゲート無しでは回らない。
- 壊れる順が「レビュー → 把握 → 衝突 → 資源」なら、UI 投資の優先もこの順。Fleet は把握（要対応・digest・頼んだこと）と衝突（radar）に厚く、レビュー帯域そのものを広げる仕掛け（エージェントの事前レビュー・検証の証拠・変更の要約）が相対的に薄い。
- Superset の「エージェントは働く時間よりレビュー待ちの時間が長い」は、Task の phase 別の滞留時間（レビュー待ちで何分寝ているか）を見せる価値を示す。どこが詰まっているかを本人に見せる指標になる。
- ベンダーが出す並列数の telemetry は 1 つも見つからなかった。各社が「100 体」「fleets」を掲げつつ中央値を出さないこと自体が、実利用の並列数が小さいことを示唆する。

### Gaps
- 並列数と成果（マージ率・手戻り率）の関係を測った対照研究は見つからなかった。
- Boris Cherny の原典、Simon Willison の記事本文は直接取得していない（検索要約と二次引用）。
- 調整役エージェントを入れると監督できる数が何倍になるかのデータは無い。

## 7. 価格・ライセンス・OSS 状況（1 行ずつ）

### Takeaway
ワークツリー管理系は「アプリ無料 + BYO サブスク」が基本で、課金は協業機能とクラウド実行に寄っている。OSS で単体事業を成立させるのは難しく、Bloop（Vibe Kanban）と Terragon は 2026 年前半に終了した。ライセンスは MIT / Apache-2.0 が多数派で、copyleft は Claude Squad（AGPL-3.0）と cmux（GPL-3.0-or-later）、source-available は Superset（ELv2）。

### Cited Findings
- Conductor: クローズドソース・macOS。2026-06 時点の競合記事は "Free currently; paid collaboration features planned"。2026-07 の 0.77.0 で multiplayer が "Pro users" 向け early access、07-30 に Conductor Cloud【二次】 — [Nimbalyst blog](https://nimbalyst.com/blog/best-agent-management-tools-2026/) / [Releasebot](https://releasebot.io/updates/conductor)
- Nimbalyst: MIT。個人無料、Teams $20/user/月（free beta）。macOS / Windows / Linux + iOS / Android【一次: ベンダー】 — [Nimbalyst blog](https://nimbalyst.com/blog/best-agent-management-tools-2026/) / [repo](https://github.com/nimbalyst/nimbalyst)
- Sculptor: 現行ページは MIT・無料（Claude のプランが必要）。2026-06 の第三者調査は「未 OSS・beta 中は無料」【矛盾】 — [product](https://imbue.com/product/sculptor) / [Ry Walker](https://rywalker.com/research/sculptor)
- Vibe Kanban: Apache-2.0・コミュニティ保守・クラウドは 2026-05 頃に停止【二次】 — [review](https://vibecoding.app/blog/vibe-kanban-review)
- Superset: Elastic License 2.0（source-available）。無料 1 user。Pro は $15/user/月（年払い）とする記事と $20/seat/月とする記事がある【矛盾・年払いと月払いの差の可能性】 — [repo](https://github.com/superset-sh/superset) / [Nimbalyst blog](https://nimbalyst.com/blog/best-agent-management-tools-2026/) / [vibecodinghub](https://vibecodinghub.org/tools/superset)
- Emdash: OSS・local-first（SQLite）。ライセンスは Show HN の要約で MIT、比較記事で Apache-2.0【矛盾】 — [Show HN](https://news.ycombinator.com/item?id=47140322) / [Augment](https://www.augmentcode.com/tools/open-source-agent-orchestrators)
- Codex app: ChatGPT のプランに含まれる。macOS 2026-02-02・Windows 2026-03-04【二次】 — [IntuitionLabs](https://intuitionlabs.ai/articles/openai-codex-app-ai-coding-agents)
- Claude desktop / web: Claude のプランに含まれる。agent teams はトークンを teammate 数に比例して消費【一次】 — [Desktop docs](https://code.claude.com/docs/en/desktop) / [Agent teams](https://code.claude.com/docs/en/agent-teams)
- Devin Desktop: Free / Pro $20 / Max $200 / Teams $40 per user。Devin は Pro 以上に同梱で共有 quota を消費【二次】 — [aicoderscope](https://aicoderscope.com/blog/windsurf-devin-desktop-rebrand-acp-2026/) / [testingcatalog](https://www.testingcatalog.com/windsurf-2-0-adds-devin-and-agent-command-center/)
- Augment Intent: Augment の credits を Auggie CLI と同レートで消費。macOS の public beta、Windows は waitlist【一次】 — [Intent blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration)
- JetBrains Air: 本体は無料 DL。JetBrains AI Pro / Ultimate に全エージェント込み、BYOK 可【一次】 — [JetBrains](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/)
- VS Code + GitHub: Claude / Codex エージェントは Copilot 契約の下で動く。steering は 1 メッセージごとに AI credits を消費【一次】 — [VS Code blog](https://code.visualstudio.com/blogs/2026/02/05/multi-agent-development) / [GitHub docs](https://docs.github.com/en/copilot/concepts/agents/cloud-agent/agent-management)
- Kiro autonomous agent: Pro / Pro+ / Power 向けプレビュー、プレビュー中は追加費用なし【一次】 — [Kiro](https://kiro.dev/blog/introducing-kiro-autonomous-agent/)
- cmux: GPL-3.0-or-later（準拠できない組織向けに商用条件あり）・macOS のみ・27.3k stars【一次】 — [cmux](https://github.com/manaflow-ai/cmux)
- Claude Squad: AGPL-3.0・8.5k stars ／ ccmanager: MIT・1.2k stars ／ Agent of Empires: MIT・2.7k stars【一次 / 二次】 — [Claude Squad](https://github.com/smtg-ai/claude-squad) / [ccmanager](https://github.com/kbwo/ccmanager) / [AoE](https://vibecodinghub.org/blog/agent-of-empires-review)
- Omnara: Apache 2.0、無料は月 10 セッション、$9/月で無制限 ／ Happy: OSS・無料・E2E 暗号化【二次 / 一次】 — [Omnara](https://www.omnara.com/) / [Happy](https://happy.engineering/docs/comparisons/alternatives/)
- Terragon: 2026-02-09 終了、OSS スナップショットを公開【一次】 — [terragon-oss](https://github.com/terragon-labs/terragon-oss)
- その他: Mux（Coder）は AGPL-3.0 の desktop + browser アプリ、Paneflow は MIT、Opcode は AGPL-3.0 で開発停滞、T3 Code は MIT で `npx t3@latest`【二次】 — [Nimbalyst blog](https://nimbalyst.com/blog/best-agent-management-tools-2026/) / [codeagentswarm](https://www.codeagentswarm.com/en/guides/best-tools-to-run-multiple-ai-coding-agents)

### Inferences
- necoder と同じ AGPL 系で並列管理をやっているのは Claude Squad と Mux。AGPL は比較記事で「制約」として書かれがちだが、個人利用のデスクトップアプリでは採用の障害になっていない（Claude Squad 8.5k stars）。
- 単体の並列管理ツールで課金が成立しているのは、クラウド実行（Conductor Cloud・Devin・Oz）か協業機能を持つところ。ローカル専業の OSS（Vibe Kanban・Terragon）は無料利用者を収益化できずに終わった。necoder-router（推論の定額）で収益を取る構想は、この市場で観測された失敗パターン（管理 UI そのものに課金）を避けている。
- 公式ベンダー（Anthropic・OpenAI・Cursor・Google・Microsoft・JetBrains・Zed）が 2026 年前半に一斉に並列セッション管理を純正で出したため、サードパーティの「Claude Code を並べるだけ」の価値は薄れた。残る差別化は複数ベンダーのエージェントを同じ面で扱うこと（ACP）・調整役・統合の安全性。

### Gaps
- Conductor Pro / Cloud の価格、Warp Oz の価格は取得できなかった。
- Emdash と Superset のライセンス / 価格の食い違いは未解消。

## 8. 統合: 収束した型 / 分かれた点 / 未解決の問題 / 盗む価値のある案（converged patterns / open problems）

### Takeaway
画面の骨格・隔離・レビューの往復は 2026 年に収束した。分かれているのは「調整役を製品が持つか API を開けるだけか」「板か一覧か」「統合の gate をどこに置くか」。未解決なのはレビュー帯域・並列 Task 間の衝突の事前検知・依存 Task・調整役の監査と費用。Fleet v2 は収束点の上に立っており、足すなら (1) fan-out 前の分解案の承認、(2) `.worktreeinclude` 互換、(3) 通知の親集約と完了音、(4) アーカイブ = 状態保存 + 削除、(5) エージェントの事前レビューと検証の証拠、(6) 依存 Task の stack。

### Cited Findings

#### 収束した型
- 左 = 状態つき一覧・中央 = 1 本: Conductor / Codex app / Claude desktop / Zed / Air / Superset / Devin / VS Code Agents window / Cursor 3 — [Conductor](https://www.conductor.build/docs/concepts/workflow) / [Claude](https://claude.com/blog/claude-code-desktop-redesign) / [Zed](https://zed.dev/blog/parallel-agents) / [Air](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/) / [VS Code](https://code.visualstudio.com/docs/agents/run/agents-window) / [Cursor 3](https://cursor.com/blog/cursor-3)
- 一覧のグルーピングは project が既定で、status / recency / PR 状態に切り替えられる: Claude desktop・Antigravity 2.0・Devin・VS Code・Conductor — [Claude](https://claude.com/blog/claude-code-desktop-redesign) / [Antigravity](https://antigravity.google/blog/google-io-2026-feature-deep-dive) / [Devin](https://docs.devin.ai/release-notes/2026) / [Conductor 0.35.0](https://www.conductor.build/changelog/0.35.0-workspace-status)
- 1 単位 = 1 branch = 1 worktree、同じ branch を共有したい時だけ 1 単位に複数エージェント: Conductor が明文化 — [Parallel agents](https://www.conductor.build/docs/core/parallel-agents)
- ライフサイクルは作る → 走らせる → レビュー → PR → merge → archive で、archive は一覧を掃除する行為として扱う: Conductor・Claude desktop（PR で自動アーカイブ）・Zed — [Conductor workflow](https://www.conductor.build/docs/concepts/workflow) / [Claude](https://claude.com/blog/claude-code-desktop-redesign) / [Zed docs](https://zed.dev/docs/ai/parallel-agents)
- diff のインラインコメントをまとめてエージェントに返す — [Claude desktop](https://code.claude.com/docs/en/desktop) / [Conductor workflow](https://www.conductor.build/docs/concepts/workflow)
- エディタ中心から agent 中心の別ウィンドウ / 別面へ: Cursor 3（IDE へ "switch back" できる）・VS Code（別ウィンドウ）・Antigravity（2 面）・Devin Desktop（ACC が既定）・Zed（Threads を左、Project を右へ） — [Cursor 3](https://cursor.com/blog/cursor-3) / [VS Code](https://code.visualstudio.com/docs/agents/run/agents-window) / [Zed](https://zed.dev/blog/parallel-agents) / [aicoderscope](https://aicoderscope.com/blog/windsurf-devin-desktop-rebrand-acp-2026/)
- マルチベンダー化: ACP を Air・Devin Desktop が採用、VS Code は Claude / Codex を同居、Warp Oz は multi-harness、Conductor は Claude Code / Codex / OpenCode / Cursor を同梱【一次 / 二次】 — [Air](https://blog.jetbrains.com/air/2026/03/air-launches-as-public-preview-a-new-wave-of-dev-tooling-built-on-26-years-of-experience/) / [VS Code blog](https://code.visualstudio.com/blogs/2026/02/05/multi-agent-development) / [Warp](https://www.warp.dev/blog/multi-harness-cloud-agent-orchestration) / [infinitus #270](https://github.com/deathemperor/infinitus/issues/270)
- 定期実行（routines / automations / scheduled tasks）が並列管理の面に入ってきた: Conductor 0.85.0・Codex automations・Claude routines・Antigravity scheduled tasks・Emdash Automations — [Releasebot](https://releasebot.io/updates/conductor) / [Codex KB](https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/) / [Antigravity](https://antigravity.google/blog/google-io-2026-feature-deep-dive) / [Emdash](https://emdash.com/docs/)

#### 分かれた点
- 板か一覧か: Devin Desktop と Nimbalyst は板、Conductor は状態グループつき一覧、Antigravity はインボックスを消して status グルーピングへ — [ACC docs](https://docs.devin.ai/desktop/agent-command-center) / [Nimbalyst](https://nimbalyst.com/blog/claude-code-session-kanban-organize-ai-agents/) / [Conductor 0.35.0](https://www.conductor.build/changelog/0.35.0-workspace-status) / [Forum](https://discuss.ai.google.dev/t/did-they-remove-the-inbox-in-agent-manager/126943)
- 調整役: 製品が持つ（Devin・Warp・Intent・Antigravity・Kiro）か、API を開けて任せる（Conductor・Superset・Vibe Kanban・cmux）か、持たないと決める（Emdash）か — [Devin](https://cognition.com/blog/devin-can-now-manage-devins) / [Warp](https://docs.warp.dev/platform/orchestration/) / [Releasebot](https://releasebot.io/updates/conductor) / [Show HN: Emdash](https://news.ycombinator.com/item?id=47140322)
- 統合の gate: 調整役が merge して人間は PR で見る（Devin・Warp）か、人間が task ごとに見る（Conductor・Claude desktop）か、CI が通れば自動（Claude の Auto-merge）か — [Devin](https://cognition.com/blog/devin-can-now-manage-devins) / [Warp](https://docs.warp.dev/platform/orchestration/) / [Claude desktop](https://code.claude.com/docs/en/desktop)
- 承認の既定: Conductor は approvals-off が既定、Claude agent teams は teammate の権限要求を人間に上げる、ccmanager は AI 判定の auto-approval【二次 / 一次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270) / [Agent teams](https://code.claude.com/docs/en/agent-teams) / [ccmanager](https://github.com/kbwo/ccmanager)
- 走らせる場所: local 専業（Superset・Emdash・cmux）、cloud 専業（Devin・Kiro）、両方 + handoff（Cursor・Codex・Claude・Conductor・Warp） — [Cursor 3](https://cursor.com/blog/cursor-3) / [Changelog](https://www.conductor.build/changelog)

#### 未解決の問題
- レビュー帯域: "You can't review 100 diffs a day"、"8 parallel cards means 8x the diffs to read" — [Superset blog](https://superset.sh/blog/roadmap-to-100-agents) / [HN](https://news.ycombinator.com/item?id=48239413)
- レビューの空洞化: "the fraction of human reviewed code is approaching zero far faster than anyone is realizing" — [HN](https://news.ycombinator.com/item?id=48239413)
- 並列 Task 間の衝突: 指針は「ファイル所有を分けろ」で、事前検知の UI は未発見 — [GitHub](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/) / [Agent teams](https://code.claude.com/docs/en/agent-teams)
- 依存する Task: "we defo need better ways to stack and review work"。Conductor の Stacks が最初の製品的な答え — [Show HN](https://news.ycombinator.com/item?id=44533004) / [Changelog](https://www.conductor.build/changelog)
- 調整役の信頼性: "Multi-agent coordination remains a hard problem"、lead が早く終わる / 自分で実装する / task 状態が遅れる — [Cursor](https://cursor.com/blog/scaling-agents) / [Agent teams](https://code.claude.com/docs/en/agent-teams)
- 調整のコスト: agent teams は "use significantly more tokens than a single session"、Devin は親が子の ACU を監視する機能を足した — [Agent teams](https://code.claude.com/docs/en/agent-teams) / [Devin](https://cognition.com/blog/devin-can-now-manage-devins)
- アイドル時の負荷: Conductor の changelog にはポーリング起因の CPU 問題が 4 回出てくる【二次】 — [infinitus #270](https://github.com/deathemperor/infinitus/issues/270)
- 単体事業の持続性: Bloop と Terragon の終了 — [vibecoding.app](https://vibecoding.app/blog/vibe-kanban-review) / [Terragon](https://www.terragonlabs.com/)

### Inferences

盗む価値のある案（Fleet v2 への当てはめ・優先度順）:
1. **fan-out 前の分解案の承認カード**（Warp `/orchestrate`・Intent の spec 承認）。Captain が `fleet create` を叩く前に「Task N 本・各 branch 名・エージェント / model・依存」を 1 枚に出し、人間が直して承認する。Fleet v2 §5.2 の権限表では Captain は無確認で Task を作れるので、費用と worktree 数が一気に増える唯一の操作に gate が無い。
2. **`.worktreeinclude` 互換**（Claude Code・Conductor）。`.necoder/worktree-setup.sh` の前に読むだけで、他ツールで整備済みの repo がそのまま動く。
3. **通知の親集約 + 同時数の上限 + 完了音**（Warp・Conductor・Superset）。Captain が起こした Task の Done / Failed は Captain の報告に畳み、人間に直接届くのは承認待ちだけにする。見ているセッションは通知しない（Claude desktop）。
4. **archive = 状態保存 + ディレクトリ削除、復元で戻す**（Zed・Conductor）と **PR の merge で自動アーカイブ**（Claude desktop）。Rust の `target/` を抱える necoder ではディスク回収の効果が大きい。
5. **エージェントの事前レビューと検証の証拠**（Claude "Review code"・Cursor の demo / screenshot・Antigravity Artifacts）。レビュー待ちに入る時に radar と並べて「要約・テスト結果・指摘」を Task カードに添える。指摘範囲を絞る（style は言わない）のが要点。
6. **依存 Task の stack**（Conductor Stacks）。＋Task の詳細に「この Task の branch から切る」を足し、統合順を帯に出す。
7. **Devin 型の帰属表示**: Captain が起こした Task を `⚑` の印だけでなく、サイドバーで Captain の下に tree connector で束ねる表示を選べるようにする。人間が直接切った Task と Captain の Task の区別が一覧の構造で読める。
8. **テキストのステータスラベル + 経過時間**（Devin の "Awaiting instructions"）。状態に色相を使わない Fleet の掟と相性が良く、グリフの意味を覚えていない時の保険になる。
9. **プロジェクト横断の要対応**（Antigravity の Inbox への需要）。サイドバーは 1 プロジェクトのままで、titlebar のバッジを押すと全プロジェクトの要対応が中身つきで出る popover を足す。
10. **phase 別の滞留時間**（Superset の観察）。「レビュー待ちで何分寝ているか」を Task 行か帯に出す。

Fleet v2 が既に業界より先にいる点（推定）:
- 統合前の Conflict Radar を第一級の UI にしている。
- 調整役の権限表・wake 条件・采配ログ（監査）を文書で定義している。調整役を持つ製品でも、ここまで権限を明文化した公開資料は見つからなかった。
- 状態を持たない Captain + 台帳。Cursor の研究が必要だと言う fresh start を構造で許している。
- 「介入」を台帳に積んで Captain に伝える仕組み。Claude Code の cross-session messaging が送信元の明示と引用までは持つが、人間の介入を調整役に同乗させる仕組みは他に見ていない。

注意点:
- Captain の差別化「エージェント非依存」は Conductor MCP / CLI（2026-08）と Warp（子は別 harness 可）で薄まった。差は役割と権限を製品が決めていることに移った。
- JetBrains Air が Fleet IDE の後継として報じられているため、「Fleet」という語は JetBrains 文脈の連想を持つ。英語圏に出す時は用語の衝突に留意。

### Gaps
- herdr と Orca はこのノートに含めていない（別担当）。表に並べて比べる時は別ノートと突き合わせること。
- Factory（Droid）・Verdent・Tonkotsu・Zencoder・Paseo・CodeAgentSwarm・Kanbots・Multica・Symphony・Cline のカンバンなど、名前だけ確認して中身を調べていないツールがある。特に Verdent（並列エージェント + Review Subagent）と Cline のカンバンは追加調査の候補。
- Windsurf / Devin Desktop の ACC 上で Managed Devins の親子がどう描かれるかは未確認。
- 2026-09 時点の各ツールの実画面（スクリーンショット）は確認しておらず、レイアウトの記述は docs と記事の文章に基づく。
