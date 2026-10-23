# 製品・OSS における「他のコーディングエージェントを采配するエージェント」の実装（2026-09-20 時点）

調査日 2026-09-20。対象は「目標を分解し、ワーカーを起動し、見張り、完了/失敗/停止に反応し、人間に報告する」リード型エージェント。
表記: **[一次]** = 公式 docs / 公式ブログ / リポジトリを取得して確認、**[二次]** = 第三者記事・検索スニペット経由（未検証を含む）、**[ソース]** = OSS のソースコードを直接確認。
推論は各節の Inferences にだけ書く。比較対象の necoder Captain の仕様は `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md` §5 と `/Users/daichi/Work/experience/necoder/crates/workspace/src/workspace/captain.rs` を読んだ内容に基づく。
Codex のソース確認は参照クローン `/Users/daichi/Work/experience/necoder/codex`（openai/codex rev `46aa019`・2026-08-25）で行った。

## 1. トポロジー — 単一リード+フラット / 階層 / メッシュ / 判定者つきパイプライン、誰が誰を spawn できるか

### Takeaway
出荷済みの主流は「単一リード + フラットなワーカー（ネスト深さ 1）」で、Captain と同じ形。ピア同士の自己調整（共有ファイル + ロック）は Cursor が失敗と報告し、Cognition も非構造スウォームを "mostly a distraction" と評価した。品質面の共通解は「実装者と別コンテキストの判定者/検証者」をパイプラインに足すことで、Captain にはまだこの役が無い。

### Cited Findings

#### 早見表

| システム | 状態（日付） | 形 | spawn できるのは | 主出典 |
|---|---|---|---|---|
| Claude Code subagents | GA | 親 + 子（既定で 3 層までネスト・同時 20） | 親セッション / サブエージェント | [sub-agents](https://code.claude.com/docs/en/sub-agents) |
| Claude Code Agent Teams | experimental・既定 off（2026-02-05 発表 [二次]） | lead 固定 + teammates（フラット・メンバー間メッセージ可） | lead のみ（"No nested teams"） | [agent-teams](https://code.claude.com/docs/en/agent-teams) |
| Claude Code Projects | public beta（2026-09-17・Pro/Max） | coordinator の会話 + cloud thread（フラット） | coordinator（Claude） | [claude-projects](https://code.claude.com/docs/en/claude-projects) |
| Codex subagents | 既定で有効 | root thread + 子（v1 は深さ 1・6 本） | root（明示依頼時のみ） | [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents) |
| Cursor 研究ハーネス | 研究（2026-01/02） | 再帰 planner → worker + judge | planner / sub-planner | [scaling-agents](https://cursor.com/blog/scaling-agents) |
| Cursor Projects | beta（2026-09-10） | coordinator + subagents（cloud VM） | coordinator | [projects](https://cursor.com/blog/projects) |
| Devin Managed Devins | 出荷（2026-03-19） | manager Devin + child Devin（各自 VM） | manager | [blog](https://cognition.com/blog/devin-can-now-manage-devins) |
| Gas Town | OSS v1.2.1（2026-06-06） | Mayor + Deacon/Witness/Refinery の監督階層 + polecats | Mayor / 人間（`gt sling`）/ scheduler daemon | [README](https://github.com/steveyegge/gastown) |
| Roo Code Orchestrator | 製品終了（2026-05-15 [二次]） | 親（停止）→ 子を 1 本ずつ直列 | orchestrator モード | [boomerang-tasks](https://roocodeinc.github.io/Roo-Code/features/boomerang-tasks) |
| Kilo Code | orchestrator モードは deprecated | `task` サブエージェント + Agent Manager（人間） | full-tool の任意エージェント / 人間 | [agent-manager](https://kilo.ai/docs/automate/agent-manager) |
| Amp（Puck） | "an experiment"（2026-07-20） | meta-agent + 任意スレッド | 任意のエージェント | [meet-puck](https://ampcode.com/news/meet-puck) |
| Factory Missions | Enterprise/Max（2026-02-26 と判断・下記） | orchestrator → worker（直列）→ validator | programmatic runner | [missions-architecture](https://factory.com/news/missions-architecture) |
| Warp Oz | 出荷（Oz は 2026-02-10） | parent + children（1 段のみ・ハーネス混在可） | parent（人間の承認後） | [orchestration](https://docs.warp.dev/platform/orchestration/) |
| GitHub mission control | public preview | 人間が指揮者 | 人間 | [github.blog](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/) |
| Copilot CLI `/fleet` | 出荷（2026-04 [二次]） | main agent + 並列 subagents | main agent | [docs](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/fleet) |
| Antigravity 2.0 | 出荷（2026-05-19 [二次]） | main agent + dynamic subagents / Manager view は人間 | main agent | [deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive) |
| Augment Intent | 出荷（2026-02-10） | coordinator → implementors（波状）→ verifier | coordinator（plan 承認後） | [blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration) |

#### Claude Code（subagents / Agent Teams / Projects / Agent SDK）
- [一次] subagents は Agent tool（旧 Task）で起動し、既定で "up to three layers below the main conversation" までネストできる。深さ上限では fork 以外から Agent tool を取り上げる。同時実行の既定上限は 20（`CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS`）— [sub-agents](https://code.claude.com/docs/en/sub-agents)
- [一次] Agent Teams の構成要素は Team lead / Teammates / Task list / Mailbox の 4 つ。"Agent teams are experimental and disabled by default"（`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`）。制約として "One team per session" "No nested teams: teammates cannot spawn their own teammates. Only the lead can manage the team." "Lead is fixed ... You can't promote a teammate to lead or transfer leadership." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] teammates 同士は直接メッセージできるが、spawn と管理は lead だけ（星形の spawn + メッシュ状の通信）。teammates は worktree で隔離されない: "Agent teams don't isolate teammates in worktrees, so partition the work" — [agents](https://code.claude.com/docs/en/agents)
- [一次] 非対話（`-p`・Agent SDK）では teammates を spawn しない: "In non-interactive mode with the `-p` flag, including Agent SDK sessions, Claude doesn't spawn teammates" — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] Agent SDK の編成プリミティブは `agents` オプション（`AgentDefinition`: description / prompt / tools / disallowedTools / model / maxTurns / background / permissionMode 等）と、深さ・同時数・支出の 3 つの上限（`CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH` 既定 3 / `CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS` 既定 20 / `maxBudgetUsd`）。数十〜数百体は `Workflow` tool（スクリプトが計画を持つ）へ誘導 — [agent-sdk/subagents](https://code.claude.com/docs/en/agent-sdk/subagents)
- [一次] Projects: "The project conversation: one long-running session where Claude acts as coordinator. It takes what you send, decides what becomes a thread, and keeps track of every thread it started. It sees what threads report back, not every step they take." threads は個別の cloud session で自分の branch と PR を持つ — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [二次] Projects の再設計版は 2026-09-17 に beta 公開 — [MarkTechPost](https://www.marktechpost.com/2026/09/17/anthropic-launches-claude-code-projects-in-beta-parallel-cloud-sessions-that-keep-running-after-you-close-your-laptop/)
- [一次] agent view（research preview）は人間だけが監督する画面で、Projects の docs は "it has no coordinator" と明記 — [claude-projects](https://code.claude.com/docs/en/claude-projects) / [agent-view](https://code.claude.com/docs/en/agent-view)
- [二次] Agent Teams は 2026-02-05 に Opus 4.6 と同時に research preview として発表 — [NxCode](https://www.nxcode.io/resources/news/claude-agent-teams-parallel-ai-development-guide-2026)

#### OpenAI Codex
- [一次] "Current Codex releases enable subagent workflows by default." 組み込みは `default` / `worker` / `explorer`。カスタムは `~/.codex/agents/` か `.codex/agents/` の TOML。ローカル Codex は "spawns agents only after explicit requests or applicable AGENTS.md/skill instructions" — [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- [ソース] v1 の既定は `DEFAULT_AGENT_MAX_THREADS = Some(6)` / `DEFAULT_AGENT_MAX_DEPTH = 1`、v2 は `DEFAULT_MULTI_AGENT_V2_MAX_CONCURRENT_THREADS_PER_SESSION = 4` — [config/mod.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/config/mod.rs)
- [ソース] v2 の道具は `spawn_agent` / `send_message`（ターンを起こさない）/ `followup_task`（idle なら起こす）/ `wait_agent` / `list_agents` / `interrupt_agent`。v2 の子は "will have the same tools as you and the ability to spawn its own subagents" で、`/root/task1/task_3` 形式の正規タスク名で木構造を指す — [multi_agents_spec.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/tools/handlers/multi_agents_spec.rs)
- [二次] 以前は experimental（`/experimental` で有効化・`agents.max_threads` 既定 6 / `agents.max_depth` 既定 1）だった — [Morph](https://www.morphllm.com/codex-multi-agent)
- [二次] Codex app は人間が複数スレッドを worktree で並走させる "command center"。cloud task は `--attempts` で best-of-N — [Verdent](https://www.verdent.ai/guides/what-is-codex-app) / [Developer Toolkit](https://developertoolkit.ai/en/codex/productivity-patterns/multi-agent-workflows/)

#### Cursor
- [一次] 失敗した形: フラットなピアが共有ファイルで調整（"Each agent would check what others were doing, claim a task, and update its status"）。ロックは "Agents would hold locks for too long, or forget to release them entirely"、"Twenty agents would slow down to the effective throughput of two or three"。楽観的並行制御に変えても "With no hierarchy, agents became risk-averse. They avoided difficult tasks and made small, safe changes instead." — [scaling-agents（2026-01-14）](https://cursor.com/blog/scaling-agents)
- [一次] 動いた形: Planners（sub-planner を再帰的に spawn）/ Workers / サイクル末の Judge。品質管理と衝突解決の integrator 役は "it created more bottlenecks than it solved" として削除 — [scaling-agents](https://cursor.com/blog/scaling-agents)
- [一次] 4 段の変遷: 自己調整 → planner/executor/workers/judge（最遅ワーカーに律速）→ continuous executor（1 体に役を盛りすぎて "random sleeping, refusing to plan, claiming premature completion"）→ 再帰 planner。root planner は "owns the entire scope of the user's instructions" だが "does no coding itself" — [self-driving-codebases（2026-02-05）](https://cursor.com/blog/self-driving-codebases)
- [一次] 製品側: subagent の非同期化と "Subagents can also spawn their own subagents, creating a tree of coordinated work" — [changelog 2.5](https://cursor.com/changelog/2-5)。Cursor 3 の Agents Window（2026-04-02 GA）は local / worktree / cloud / SSH の並走を人間が捌く面で、AI の調整役は記述されていない — [agents-window](https://cursor.com/docs/agent/agents-window)
- [一次] Cursor Projects: "The coordinator doesn't write code itself but directs other agents"、"never blocked and is always responsive to direction"、"thousands of subagents" へ委譲 — [projects（2026-09-10）](https://cursor.com/blog/projects)

#### Cognition Devin
- [一次] MultiDevin（2024-09）: "1 'manager' Devin and up to 10 'worker' Devins"。manager は各 worker に task を配り、成功した worker の変更を 1 つの branch / PR にマージする — [Sept '24 update](https://cognition.com/blog/sept-24-product-update)
- [一次] Managed Devins（2026-03-19）: coordinator が scope → 割当 → 監視 → 衝突解決 → 結果の取りまとめを行い、各 child は "its own isolated virtual machine with its own terminal, browser, and development environment" — [blog](https://cognition.com/blog/devin-can-now-manage-devins) / [docs](https://docs.devin.ai/work-with-devin/advanced-capabilities)
- [一次] 2026-04-22 の総括: 効いているのは Code-Review-Loop / Smart Friend / manager-child（map-reduce-and-manage）の 3 型。"Multi-agent systems work best today when writes stay single-threaded and the additional agents contribute intelligence rather than actions"。"The unstructured-swarm approach, arbitrary networks of agents negotiating with each other, is mostly a distraction" — [multi-agents-working](https://cognition.com/blog/multi-agents-working)

#### Gas Town（Steve Yegge）
- [一次] 役割: Mayor（"Global coordinator"・singleton・persistent）/ Deacon（監督 daemon）/ Witness（rig ごとの polecat ライフサイクル管理）/ Refinery（rig ごとの merge queue）/ Polecat（永続 identity・使い捨てセッション）/ Crew（人間が管理する永続ワーカー）/ Dog（Deacon の下働き・Boot は Deacon の健康診断）— [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md)
- [一次] 監視は 3 段: "Daemon (Go process, 3-min heartbeat) → Boot (AI triage) → Deacon (continuous patrol) → Witnesses & Refineries (per-rig)"。"Scale comfortably to 20-30 agents" と主張 — [README](https://github.com/steveyegge/gastown)
- [一次] 起動経路は `gt sling <bead-id> <rig>`（Mayor でも人間でも打てる）。`scheduler.max_polecats` が正なら daemon が容量を見て順次 dispatch する — [scheduler.md](https://github.com/steveyegge/gastown/blob/main/docs/design/scheduler.md)
- [一次] Witness を Claude Code Agent Teams の lead に置き換える案がある（"Status: Future architecture — NOT YET IMPLEMENTED"・2026-02-08）— [witness-at-team-lead.md](https://github.com/steveyegge/gastown/blob/main/docs/design/witness-at-team-lead.md)

#### Roo Code / Kilo Code / Cline
- [一次] Roo の Orchestrator は直列: "The parent task (in Orchestrator mode) pauses, and the new subtask begins in a different, specialized mode." — [boomerang-tasks](https://roocodeinc.github.io/Roo-Code/features/boomerang-tasks)
- [二次] Roo Code は 2026-04-20 に終了を告知し 2026-05-15 に拡張・Cloud・Router を停止（Slack 起点の cloud agent "Roomote" へ転換）— [The New Stack](https://thenewstack.io/roo-code-cloud-ides-ai-coding/) / [Bodega One](https://www.bodegaone.ai/blog/roo-code-shutdown-alternatives)
- [一次] Kilo は専用 orchestrator を廃止（"old `orchestrator` agent is deprecated"）。`task` tool の子（foreground / `background: true`）、人間が捌く Agent Manager（worktree 隔離・最大 4 バージョン並走・`agent_manager` tool は 1 回 20 task まで）、Swarm 掲示板（"Swarm does not start, stop, or assign agents"）の 3 層 — [agent-manager](https://kilo.ai/docs/automate/agent-manager) / [orchestrator-mode](https://kilo.ai/docs/code-with-ai/agents/orchestrator-mode)
- [二次] Cline の subagents は experimental な読み取り専用の調査係（ファイル編集・ブラウザ・MCP・ネストは不可）。並走の面は Cline Kanban（カードごとに worktree・人間が操作）— [Cline docs](https://docs.cline.bot/features/subagents) / [Cline CLI](https://cline.bot/cli)

#### Amp / Factory / OpenHands / Google / Warp / GitHub
- [一次] Amp: "You can now ask your agents in Amp to spawn other agents. In orbs, your local machine, or on any other machine." "They can send messages and files to each other, too." — [from-agent-to-agent（2026-07-17）](https://ampcode.com/news/from-agent-to-agent)。Puck は "a quick assistant and a home base for launching and coordinating other agents"（"It's an experiment, too."）— [meet-puck（2026-07-20）](https://ampcode.com/news/meet-puck)
- [一次] Factory Missions: orchestrator は "avoids accumulating overly granular context, delegating all investigation and implementation to subagents and workers"。"A programmatic runner takes the feature list and spawns a worker for each feature in order."。validator は scrutiny と user-testing の 2 種で、"the system injects validators at milestones" — [missions-architecture](https://factory.com/news/missions-architecture)
- [一次] Factory は並列を捨てた理由を "Agents conflict, duplicate work, and drift without structure" と書き、未解決として再帰的管理の深さ（1 段は動く・それ以上は未検証）を挙げる — [missions](https://factory.com/news/missions)
- [二次] OpenHands SDK は並列の `DelegateTool`（spawn / delegate・`threading.Thread`）と、新しい `TaskToolSet`（1 回の呼び出しで作成と実行・background 対応）の 2 系統 — [DeepWiki](https://deepwiki.com/OpenHands/software-agent-sdk/3.3-sub-agent-delegation-and-task-management) / [Issue #634](https://github.com/OpenHands/OpenHands-CLI/issues/634)
- [一次] Antigravity 2.0: subagent は "modular, specialized, or blank-slate assistant agents spawned programmatically by the main agent"。隔離が要れば "it autonomously creates the worktrees for those conversations and handles cleanup automatically" — [deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive)。Manager view は人間が複数 workspace のエージェントを捌く面 — [Google Developers Blog](https://developers.googleblog.com/build-with-google-antigravity-our-new-agentic-development-platform/)
- [二次] Jules に AI のリードは無い。`--parallel <number>` で同一 task を並走させ、Planning Critic が計画を事前批評する（"9.5% reduction in task failure rates"）— [Jules Tools reference](https://jules.google/docs/cli/reference/) / [changelog](https://jules.google/docs/changelog/)
- [一次] Warp Oz: "An orchestrated workflow always has one parent agent and one or more child agents." "Orchestrations today are exactly one level deep"。local→local / local→cloud / cloud→cloud / cloud→cloud-local の 4 配置 — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [一次] GitHub mission control は人間が指揮者（"You're now a conductor of agents."）— [github.blog（2025-12-01）](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/)。Copilot CLI の `/fleet` は main agent が "orchestrator, managing the workflow and dependencies between the subtasks" になる — [docs](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/fleet)

#### OSS の編成スクリプト群
- [二次] ruflo（旧 claude-flow・2026-01 改名）は queen + hive-mind を掲げるが、独立監査は agent spawn / task 管理 / swarm 調整の大半を "either a stub, a state record with no executor, or cosmetic output that wastes tokens" と評価 — [監査 gist](https://gist.github.com/roman-rr/ed603b676af019b8740423d2bb8e4bf6)
- [一次] ccpm: PRD → epic → task ファイル（`depends_on` / `parallel` / `conflicts_with`）→ GitHub Issues + worktree。1 epic に最大 12 エージェント — [ccpm](https://github.com/automazeio/ccpm)
- [一次] claude-squad は AI の調整役を持たない。tmux セッション + git worktree を人間が TUI で管理し、Claude Code / Codex / Gemini / Aider に対応（AGPL-3.0）— [claude-squad](https://github.com/smtg-ai/claude-squad)
- [一次] Tmux-Orchestrator は Orchestrator → Project Manager → Engineer の 3 層で、tmux の `send-keys` 相当で互いに書き込む — [Tmux-Orchestrator](https://github.com/Jedward23/Tmux-Orchestrator)
- [一次] AgentField は coding harness を REST endpoint として包む control plane（DAG・監査ログ）で、対応先に Claude Code / Codex / Gemini CLI / OpenCode 等を挙げる — [agentfield](https://github.com/Agent-Field/agentfield)
- [一次] リード無しの対照例: Anthropic の C コンパイラ実験は 16 体並列で "I don't use an orchestration agent."。`current_tasks/` のロックファイルを git 同期で取り合う — [building-c-compiler（2026-02-05）](https://www.anthropic.com/engineering/building-c-compiler)

### Inferences
- Captain（任命制の単一リード + フラットな Task・ネスト無し）は Claude Agent Teams / Claude Projects / Cursor Projects / Warp Oz / Devin と同じ形で、2026 年の主流から外れていない。
- 再帰階層が実証されたのは Cursor の研究ハーネス（数百体規模）だけ。Factory も Warp も 1 段に留めている。necoder の舞台上限 3 枚という規模なら階層化の利得は無い。
- 「リードがコードを書かない」は Cursor Projects・Factory・Roo・Cursor 研究の root planner に共通する。Captain の同じ規律はプロンプトでしか守られていないので、構造で守る余地がある（§5・§9）。
- 判定者ロール（Cursor judge / Factory validators / Augment verifier / Devin Review / Jules critic）は Captain に無い。radar は機械的な衝突検査で、内容の検証ではない。

### Gaps
- Puck が自分でコードを書くか、どこに記憶を持つかは公式ページに記述が無い。
- Devin Managed Devins の child 数上限は docs にも blog にも無い（2024 年の MultiDevin の「10」が現行かは不明）。
- Cursor Projects の coordinator が内部で何段の subagent 木を作るかは未公開。
- Yegge 本人の投稿（Medium）は 403 で取得できず、役割の説明はリポジトリの docs と第三者記事で代替した。

## 2. 状態と記憶 — コーディネーターの状態はどこに住み、再起動とコンパクションをどう越えるか

### Takeaway
長時間運用に耐えている系は例外なく、コーディネーターの記憶を会話コンテキストの外（ファイル・issue tracker・git・台帳）へ出し、セッションを使い捨てにしている。Captain の「stateless + 追記専用台帳」はこの系譜の最も純粋な形だが、現行実装には理念とずれる点が 2 つある（長寿命の Captain スレッドが文脈を溜め続ける・wake キューがプロセスメモリにある）。

### Cited Findings
- [一次] Claude Agent Teams: team config は `~/.claude/teams/{team-name}/config.json`、mailbox は `~/.claude/teams/{team-name}/inboxes/{agent-name}.json`、task list は `~/.claude/tasks/{team-name}/`。team config はセッション終了で消え、task list は残る。"`/resume` and `/rewind` do not restore in-process teammates." lead の判断の記憶は lead 自身のコンテキスト — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] Claude Code subagents: transcript は `~/.claude/projects/{project}/{sessionId}/subagents/agent-{agentId}.jsonl` に別ファイルで残り、"when the main conversation compacts, subagent transcripts are unaffected" — [sub-agents](https://code.claude.com/docs/en/sub-agents)
- [一次] Claude Projects: "the conversation works from recent messages, recent threads, and project memory rather than its full history, so it keeps going for as long as the project runs. Put anything that must never be dropped in project memory." project memory は `MEMORY.md` を索引にしたファイル群、project instructions は 16,000 字まで — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [ソース] Codex: 子の完了は `<subagent_notification>` として親の履歴に注入される（`multi_agent.subagent_notification`）。外部台帳は無く、状態はスレッドの履歴に住む — [subagent_notification.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/context/subagent_notification.rs)
- [一次] Cursor 研究: drift 対策として scratchpad.md を "frequently rewritten versus being appended to"、コンテキスト上限での自動要約、自己省察のリマインド — [self-driving-codebases](https://cursor.com/blog/self-driving-codebases)。"Periodic fresh starts" が必要 — [scaling-agents](https://cursor.com/blog/scaling-agents)
- [一次] Cursor Projects: "Each Project maintains a set of files that sync across every cloud and local machine"。エージェントが学んだことを書き足す共有ファイル — [projects](https://cursor.com/blog/projects)
- [一次] Devin: manager は "read the full trajectories of its managed Devins" でき、"Schedule messages to itself" で後の自分へ申し送る。永続化の仕組みは未記載 — [blog](https://cognition.com/blog/devin-can-now-manage-devins)
- [一次] Gas Town: 状態は git-backed hooks（worktree）・Beads 台帳（Dolt）・agent identity に置く。"Every action is attributed." `gt prime` で文脈を回復し、`gt seance` で前任セッションへ問い合わせる — [README](https://github.com/steveyegge/gastown) / [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md)
- [一次] Maggie Appleton の整理: Gas Town は "important information – agent identities and tasks – in Git, then liberally kills off sessions and spins up fresh ones when needed"（Persistent Roles, Ephemeral Sessions）— [maggieappleton.com](https://maggieappleton.com/gastown)
- [一次] Factory Missions: 状態は `validation-contract.md` / `features.json` / `services.yaml` / `AGENTS.md` に分散し、"No single agent needs to hold the complete picture in its context at once." — [missions-architecture](https://factory.com/news/missions-architecture)。"Git is the source of truth" — [missions](https://factory.com/news/missions)
- [一次] Anthropic Research: lead は計画を memory に保存する。"if the context window exceeds 200,000 tokens it will be truncated" — [multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [一次] Warp Oz: "a durable, server-backed message bus"。run ごとに "its own lifecycle, transcript, conversation, and credit usage" — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [一次] Roo: 親は停止したまま自分のコンテキストに状態を持つ。子から戻るのは要約のみ — [boomerang-tasks](https://roocodeinc.github.io/Roo-Code/features/boomerang-tasks)
- [一次] ccpm: "Issues are the source of truth" + `.claude/context/` — [ccpm](https://github.com/automazeio/ccpm)。Tmux-Orchestrator は "Commit after each step completion" と 30 分ごとの commit を記憶にする — [Tmux-Orchestrator](https://github.com/Jedward23/Tmux-Orchestrator)
- [一次] Augment Intent: living spec を "agents read from and update" する — [blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration)
- [ローカル] Captain: `wake_captain` は `ensure_named_thread` で同名の Captain スレッドを再利用し、busy 中のイベントは `self.chrome.captain_pending`（プロセスメモリ）に溜めて次の wake でまとめて送る — `/Users/daichi/Work/experience/necoder/crates/workspace/src/workspace/captain.rs`

### Inferences
- 「stateless」の純度で並べると、Captain（台帳のみ）> Gas Town（beads + git・ただし Mayor のセッションも長寿命）> Claude Projects（memory + 直近メッセージ）> Factory（ファイル群）> Claude Agent Teams / Codex / Roo（リードのコンテキストが正）。Agent Teams の「lead 固定・resume 不可」は、状態をコンテキストに置いた帰結と読める。
- Captain は設計上 stateless だが、実装は 1 本の ACP スレッドへ wake のたびにプロンプト + 現況表を足していく。スレッドの文脈は単調に増え、Cognition の言う "Context Rot" と cache miss のコストを招く。Cursor の "periodic fresh starts"、Gas Town の使い捨てセッション、Claude Projects の「全履歴では動かさない」は同じ問題への答えになっている。台帳が正なら Captain スレッドを N 回の wake ごと、あるいは文脈使用率 X% で新セッションへ切り替えても失うものが無い。ACP の `session/new` を使えばよい。stateless の配当はまだ受け取っていない。
- `captain_pending` がメモリにあるため、necoder を再起動すると未配達の wake が消える。台帳にイベントは残るので、「Captain が最後に読んだ task_event の位置」を台帳側に持てば再起動後に差分を 1 通で渡せる。Warp の global sequence number、Gas Town の mail = bead、Claude の mailbox ファイルは、どれも配達状態を永続化している。
- 共有の学び（プロジェクト memory）を全ワーカーの brief に同乗させる仕組みは Claude Projects / Cursor Projects / Factory / Warp（Agent Memory）が持つが、Captain には無い。

### Gaps
- Devin の manager とClaude Projects の coordinator が、コンパクションをどう越えるかの内部実装は未公開（Projects は上記の 1 文のみ）。
- Cursor Projects の共有ファイルの形式・上限は未公開。

## 3. 起床とスケジューリング — ポーリング / イベント駆動 / 常時ループ、イベントの届け方、デバウンス、コスト制御

### Takeaway
ハーネスが完了・失敗・入力待ちを検知して「リードの次のターンとして注入する」イベント駆動が 2026 年の標準になった（Claude Agent Teams・Codex・Warp・Claude Projects）。LLM 自身に見回りをさせる系（Gas Town の patrol・Tmux-Orchestrator の自己スケジュール・Devin の自分宛メッセージ）はコストとボトルネックの報告が多い。Captain の「即時 + Blocked は 15 秒後に 1 回 + busy 中はまとめる」は最も倹約側にある。

### Cited Findings
- [一次] Claude Agent Teams: "Automatic message delivery ... The lead doesn't need to poll for updates." "Idle notifications: when a teammate finishes and stops, it automatically notifies the lead and includes its final answer in the notification. A teammate whose turn ends on an API error notifies the lead that it failed and includes the error text." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] Claude Code background subagent: "A background subagent's results reach Claude as a completion notification in a later turn." — [sub-agents](https://code.claude.com/docs/en/sub-agents)
- [一次] cross-session messaging: 受信側が idle なら新しいターンを起こし、稼働中なら tool call の合間に読む。`notify_when_idle` は 1 回限りの購読で "neither session polls the other"、12 時間で失効。ループ対策に送信者ごとのレート制限・同一文の重複破棄・受信キュー最大 50 — [cross-session-messaging](https://code.claude.com/docs/en/cross-session-messaging)
- [一次] Claude Code のコスト注意: idle のセッションに届いた cross-session message・scheduled task・goal check-in は毎回フルコンテキストを送る。goal の idle check-in は 1 goal あたり最大 3 回に制限された（v2.1.246）— [costs](https://code.claude.com/docs/en/costs)
- [ソース] Codex: 子が終端状態になると親へ通知メッセージが届く。`wait_agent` v2 は "Wait for a mailbox update from any live agent ... Does not return the content"。タイムアウトは最小 10 秒 / 既定 30 秒 / 最大 1 時間。プロンプトは "Call wait_agent very sparingly." "Do not repeatedly wait by reflex." と指示する — [multi_agents_spec.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/tools/handlers/multi_agents_spec.rs) / [config/mod.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/config/mod.rs)
- [一次] Claude Projects: thread は終わると会話へ報告する。PR を見張る idle thread は "wakes up and uses your plan again when CI fails or a review comment arrives"。idle の project は plan を消費しない — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [一次] Cursor: "Cursor can now monitor your PRs, watch a Slack thread, or run scheduled tasks."（イベント購読）。追撃は "wait for the next tool call instead of cutting the agent off mid-action" — [changelog 2026-08-19](https://cursor.com/changelog/08-19-26)。Projects は Slack / schedule / PR の subscription で自走する — [projects](https://cursor.com/blog/projects)
- [一次] Devin: manager は "Schedule messages to itself"（長い child の様子見のための自己リマインド）。child 完了の通知機構は未記載 — [docs](https://docs.devin.ai/work-with-devin/advanced-capabilities)
- [一次] Gas Town: Go daemon が 3 分ごとに heartbeat を打ち、step 0–13 が健康診断・回復・掃除、step 14 が容量制御つき dispatch — [scheduler.md](https://github.com/steveyegge/gastown/blob/main/docs/design/scheduler.md)。heartbeat の保存先は 3 系統（Deacon ファイル 5 分 stale / 20 分 very-stale、session state、agent-bead label）で、書き込みのずれによる誤検知が起きたため tmux の活動と突き合わせる — [heartbeats.md](https://github.com/steveyegge/gastown/blob/main/docs/concepts/heartbeats.md)
- [一次] Gas Town の律速: Witness は単一スレッドの patrol（1 周 30〜90 秒・1 周で処理できる完了は 1 件）で、"at high throughput it becomes a bottleneck. Zombie polecats accumulate in `done` state" — [polecat-self-managed-completion.md](https://github.com/steveyegge/gastown/blob/main/docs/design/polecat-self-managed-completion.md)
- [一次] Gas Town の原則は "If you find something on your hook, YOU RUN IT."。完了は `gt done` → mail（bead）+ tmux nudge で伝える — [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md) / [mail-protocol.md](https://github.com/steveyegge/gastown/blob/main/docs/design/mail-protocol.md)
- [一次] Warp Oz: 各エージェントに inbox。"Messages and run state transitions share a global sequence number so the parent never observes a child's `SUCCEEDED` state before the message that produced the result."。状態は `INPROGRESS` / `SUCCEEDED` / `FAILED` / `BLOCKED` / `ERROR` / `CANCELLED` — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [二次] Amp: self-scheduling（"Agents set their own schedules, wake themselves up"）と event-driven orbs（GitHub CI 失敗・Linear・監視アラート・任意の HTTP）。handler は 30 秒の処理期限。解説記事は "No documented wake-frequency cap or budget guardrail" と指摘 — [Digital Applied](https://www.digitalapplied.com/blog/amp-event-driven-orbs-self-scheduling-agents-2026)
- [一次] Tmux-Orchestrator: `./schedule_with_note.sh [minutes] "note"` でエージェントが自分の check-in を予約する — [Tmux-Orchestrator](https://github.com/Jedward23/Tmux-Orchestrator)
- [一次] Antigravity: 非同期 subagent は main agent を塞がず、background task が "loops and polls the subagent states periodically"。ポーリングするのはハーネスで LLM ではない — [deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive)
- [一次] Factory: 直列の runner が worker を順に起動し、milestone の終わりに validator を挿す。詰まると "the orchestrator halts the mission and hands control back to the user" — [missions-architecture](https://factory.com/news/missions-architecture)
- [一次] Anthropic Research の律速: lead が subagent の組の完了を同期的に待つ — [multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [ローカル] Captain: Done / review_ready / Failed / integrated は即時、Blocked は 15 秒後にまだ Blocked なら 1 回、人間の介入は次の wake に同乗、busy 中はキューに積んで 1 通にまとめる。同一 Task の連続イベントは 5 秒デバウンス（仕様）— `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md` §5.3 / §5.6

### Inferences
- Captain の wake 設計は Claude Agent Teams の idle notification、Codex の `<subagent_notification>`、Warp の message bus と同型で、Gas Town / Tmux-Orchestrator / Devin の「LLM が時間で見回る」形よりコストと律速の両面で優れる。Gas Town の設計文書が自ら Witness の patrol を「直列ボトルネック」と認めている点が最も強い裏付けになる。
- Blocked を即時に上げず 15 秒待つ工夫に相当するものは、調べた範囲の他システムに無い。Claude Agent Teams は permission を lead セッション上の人間へそのまま上げ、Codex は非アクティブなスレッドの承認を overlay で出す。どれも人間が答える前提なので、人間がすぐ答えた場合に Captain を起こさないのは理にかなっている。
- 足りないのは「長く放置された Blocked / Failed の再エスカレーション」。Gas Town は `stale_threshold: 4h` と `max_reescalations: 2` で重大度を上げて人間へ mail / email / SMS を飛ばす。Captain の 1 回きりの wake だけでは、人間が席を外している間の停止が埋もれる。
- `necoder fleet wait` は台帳をポーリングしながら Captain のターンを塞ぐ（`/Users/daichi/Work/experience/necoder/docs/FLEET-ARCHITECTURE.md`）。wake がイベント駆動である以上、Captain は wait せずターンを終える方が一貫する。Cursor Projects が coordinator の売りを "never blocked and is always responsive to direction" と書き、Codex が "Call wait_agent very sparingly" と指示しているのは、リードを待ちで塞ぐと人間の指示が届かなくなるから。
- 配達順序の保証（Warp の global sequence）は Captain にも要る。「done イベント」より先に「その Task の digest」が古いまま渡ると采配を誤る。台帳の連番をそのまま wake の順序に使えば足りる。
- FLEET-V2 §5.6 が書く「同一 Task の連続イベントは 5 秒デバウンス」は、`captain.rs` にも呼び出し元（`notifications.rs` の done / failed / flush、`project_session.rs` の integrated）にも見当たらず、どれも `wake_captain` を即時に呼んでいる。いま連続イベントをまとめているのは「Captain が busy の間は `captain_pending` に積む」だけで、Captain が idle の時に同じ Task から短時間に 2 件来ると 2 ターン消費する。

### Gaps
- Devin の child 完了通知がイベントかポーリングかは docs に無い。
- Claude Agent Teams の mailbox を内部で何秒ごとに読むかは未記載（v2.1.207 以前の「毎秒エラーが出る」不具合の記述から、毎秒の読み取りが推測されるだけ）。

## 4. 通信 — リードはどうブリーフし、ワーカーはどう報告し、エージェント間の伝言はどう運ぶか

### Takeaway
下りは「spawn 時のプロンプトが唯一の文脈」、上りは「最終メッセージ（要約）だけ」が標準で、Captain の digest 方式と一致する。差が付くのはブリーフの型（目的・出力形式・道具・境界・書き込み範囲の分離）と、ワーカーの引き継ぎノートの型（やったこと + 懸念・逸脱・発見）。Cognition は要約だけの受け渡しの限界を指摘し、Devin は manager に全 trajectory を読ませる逃げ道を残した。

### Cited Findings
- [一次] ブリーフの 4 要素: "Each subagent needs an objective, an output format, guidance on the tools and sources to use, and clear task boundaries" — [Anthropic multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [一次] Claude Agent Teams: teammate は CLAUDE.md / MCP / skills を読み込むが "The lead's conversation history does not carry over."。task 固有の詳細は spawn prompt に入れる。"Having 5-6 tasks per teammate keeps everyone productive" — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] Claude Agent SDK: "The only content you pass from parent to subagent is the Agent tool's prompt string"。戻りは最終メッセージのみ。v2.1.210 以降は最終メッセージ中の指示めいたパターン（制御タグの模倣・`Human:` 行など）を無害化してから親に読ませる — [agent-sdk/subagents](https://code.claude.com/docs/en/agent-sdk/subagents)
- [ソース] Codex の委譲指針: "Subtasks must be concrete, well-defined, and self-contained." "For code-edit subtasks, decompose work so each delegated task has a disjoint write set." "instruct the submodel to edit files directly in its forked workspace and list the file paths it changed in the final answer." "Do not redo delegated subagent tasks yourself"。v2 は `fork_turns`（"none" / "all"）で親文脈の継承量を選ぶ — [multi_agents_spec.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/tools/handlers/multi_agents_spec.rs)
- [一次] Codex 公式: subagent は親へ要約を返し、中間出力は返さない — [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- [一次] Roo: 下りは `new_task` の `message` のみ、上りは `attempt_completion` の `result` のみ（"Be mindful that only this summary returns to the parent."）— [boomerang-tasks](https://roocodeinc.github.io/Roo-Code/features/boomerang-tasks)
- [一次] Cursor の handoff: worker の引き継ぎは "not just what was done, but important notes, concerns, deviations, findings, thoughts, and feedback"。情報が鎖を上へ伝わる（"propagating information up the chain"）— [self-driving-codebases](https://cursor.com/blog/self-driving-codebases)
- [一次] Cursor のプロンプト教訓: "Constraints are more effective than instructions"（例: "No TODOs, no partial implementations"）、"Give concrete numbers and ranges"（"Generate 20-100 tasks"）、"Avoid checkbox mentality" — [self-driving-codebases](https://cursor.com/blog/self-driving-codebases)。"The prompts matter more" — [scaling-agents](https://cursor.com/blog/scaling-agents)
- [一次] Gas Town の mail は型つき: `POLECAT_DONE`（Exit: MERGED|ESCALATED|DEFERRED / Issue / MR / Branch）、`MERGE_READY`、`MERGED`、`MERGE_FAILED`（Failure-Type: tests|build|push|other）、`REWORK_REQUEST`（Conflict-Files + rebase 手順）、`RECOVERED_BEAD` — [mail-protocol.md](https://github.com/steveyegge/gastown/blob/main/docs/design/mail-protocol.md)
- [一次] Devin: child は prompt / playbook / tag / ACU limit つきで起動し、manager は途中で "instructions, context, or corrections" を送れる。必要なら child の全 trajectory を読む — [blog](https://cognition.com/blog/devin-can-now-manage-devins)
- [一次] Cognition の 2 原則: "Share context, and share full agent traces, not just individual messages" / "Actions carry implicit decisions, and conflicting decisions carry bad results" — [dont-build-multi-agents（2025-06-12）](https://cognition.com/blog/dont-build-multi-agents)
- [一次] Cognition が manager-child で踏んだ問題: "Managers trained on small-scoped delegation default to being overly prescriptive, which backfires when the manager lacks deep codebase context" / "Agents assume they share state with their children when they don't" / "Cross-agent communication...doesn't happen by default" — [multi-agents-working](https://cognition.com/blog/multi-agents-working)
- [一次] Claude Projects: 新しい thread は必ず project の repositories / instructions / memory から始まる。"A thread's full results stay in the thread"。会話側は報告だけを見る — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [一次] Warp のメッセージ種別: "handoffs, decisions, blocked-on-input requests, status updates, and final results"。child の prompt は spawn 時に parent が与える — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [一次] Factory: worker は feature spec だけを持った fresh context で始め、"write tests before code"。正しさの最終判断は worker のものではない（"the final judgment on correctness is not their call"）— [missions-architecture](https://factory.com/news/missions-architecture)
- [一次] エージェント間の直接伝言: Claude Agent Teams は任意の teammate へ名前で送る（全員宛は 1 人ずつ）— [agent-teams](https://code.claude.com/docs/en/agent-teams)。Kilo Swarm は `board_post` / `board_read` の掲示板 — [agent-manager](https://kilo.ai/docs/automate/agent-manager)。Amp はメッセージとファイルの受け渡し — [from-agent-to-agent](https://ampcode.com/news/from-agent-to-agent)
- [ローカル] Captain: 渡すのは事実層 + Tier1 digest + キャッシュ済み Tier2 で、transcript は渡さない。Task への指示は `fleet create`（プロンプト）と `fleet send` — `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md` §5.6

### Inferences
- 「上りは要約のみ」は Claude / Codex / Roo / Factory / Claude Projects の共通解で、Captain の digest 3 段圧縮は正しい側にいる。ただし Cognition の第 1 原則と、Devin の「manager は必要なら全 trajectory を読める」を踏まえると、Captain にも read-only の掘り下げ手段（例: `necoder fleet digest --tier 3 <task>` や `fleet log <task> --tail N`）があると、失敗原因の診断で推測に頼らずに済む。常時は渡さず、Captain が要求した時だけ渡す。
- 下りのブリーフは、どのブランドが Captain になるかに依存しない necoder の資産にできる。`fleet create` にテンプレートを持たせ、目的 / 完了条件（検証コマンド）/ 触ってよい範囲（disjoint write set）/ 触らない範囲 / 報告の形式を必須欄にすれば、Anthropic の 4 要素と Codex の "disjoint write set" を構造で強制できる。
- Cognition の「manager が過剰に細かく指示して裏目に出る」は Captain に直撃する。Captain は main に住みコードを書かないので、コードベースの深い文脈を持たない。ブリーフは手順ではなく目的・制約・受入条件に寄せるべきで、Cursor の "Constraints are more effective than instructions" とも一致する。
- ワーカーの終了メッセージの型（やったこと / 変更ファイル / 実行した検証と結果 / 懸念・逸脱）をブリーフで要求すれば、necoder の Tier1 digest（末尾 1〜2 文）の質が上がる。Gas Town の `POLECAT_DONE` と Cursor の handoff が先例。
- ワーカー同士の直接通信は Captain の設計に無いが、足す根拠は弱い。Cognition は "doesn't happen by default" と書き、ruflo の hive-mind は合議で 20 分を溶かした（§8）。必要な横の伝達は「Captain が `fleet send` で中継し、台帳に残す」で足りる。

### Gaps
- Claude Projects の coordinator が thread に渡す brief の実文面は未公開。
- Devin の "internal MCP" の tool 一覧は未公開。

## 5. 権限と安全 — 自律でやってよいこと、人間が要ること、マージキューと衝突処理

### Takeaway
「コーディネーターに承認を代行させない」は Anthropic が仕様として明文化しており、Captain の「推薦のみ」と一致する。自律マージは Gas Town の実地報告（テスト失敗のまま main へ統合 → force push で復旧）が最も強い反証で、Cursor も integrator を撤去した。予算・同時数の上限を「リードへのお願い」ではなくハーネスの強制にしているかどうかが暴走防止の分かれ目になる。

### Cited Findings
- [一次] Claude Code: "A teammate can't approve a permission prompt or supply consent on your behalf, and a teammate that was denied an action can't relay it to another teammate to bypass the check." auto mode の分類器は "treats an approval claim relayed from another agent as untrusted input rather than confirmation from you." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] cross-session でも同じ: "a message from another session never counts as your consent, so it can't answer a pending permission prompt on your behalf."、設定・`CLAUDE.md` も他セッションの依頼では変えない — [cross-session-messaging](https://code.claude.com/docs/en/cross-session-messaging)
- [一次] Claude Projects: "When a thread needs your approval, the prompt is inside that thread and the thread waits until you answer it there. Telling Claude in the project conversation to go ahead doesn't reach it." — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [一次] Claude Agent Teams: "Teammate permission prompts appear in the lead session, so approve them there yourself."。摩擦対策は事前の allowlist（"Pre-approve common operations in your permission settings"）。例外は plan 承認で、"Claude Code approves the plan in the lead's session as soon as the request arrives, without the lead reviewing it." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] Codex: "Subagents inherit your current sandbox policy"。CLI では "approval requests can surface from inactive agent threads"（発生元スレッドのラベルつき・`o` で確認）— [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- [一次] Antigravity: subagent は "inherit tool configurations and security permissions from the main agents ... but no more." — [deep dive](https://antigravity.google/blog/google-io-2026-feature-deep-dive)
- [一次] Warp: 分解と起動に人間の承認が要る。"`/orchestrate`: Agent proposes a breakdown and waits for approval." "Approval is required before the parent launches children." — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [一次] Roo: "By default, you must approve the creation and completion of each subtask."（auto-approve で自動化可）。Orchestrator は "cannot read files, write files, call MCPs, or run commands" — [boomerang-tasks](https://roocodeinc.github.io/Roo-Code/features/boomerang-tasks)
- [一次] Gas Town の計画文書は調整専用モードを "Structural, not behavioral" と評価し、現状は "'Witness doesn't implement' is enforced by CLAUDE.md instructions" と認めている — [witness-at-team-lead.md](https://github.com/steveyegge/gastown/blob/main/docs/design/witness-at-team-lead.md)
- [一次] Claude Agent Teams の既知の癖: "Sometimes the lead starts implementing tasks itself instead of waiting for teammates."（対処は "Wait for your teammates ..." と言うこと）。"The lead can stop early too, deciding the team is finished before all tasks are actually complete." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] 自律マージの事故: "After the first PR got merged autonomously, despite the integration tests failing, I quickly closed Gas Town." "I had to get a colleague to `reset --hard` and force push the Dolt repository."（4 本の PR は "None of the PRs were good, and I ended up closing them all."）— [DoltHub A Day in Gas Town（2026-01-15）](https://www.dolthub.com/blog/2026-01-15-a-day-in-gas-town/)
- [一次] Gas Town の Refinery: `gt done` → branch push → MR bead → "Refinery batches and runs verification gates → bisects on failure"（Bors 型）— [README](https://github.com/steveyegge/gastown)。衝突時は `REWORK_REQUEST` に Conflict-Files と rebase 手順を載せて polecat へ戻す — [mail-protocol.md](https://github.com/steveyegge/gastown/blob/main/docs/design/mail-protocol.md)。Refinery は衝突が酷いと実装を "creatively 're-imagine'" する — [maggieappleton.com](https://maggieappleton.com/gastown)
- [一次] Gas Town の既定は yolo: エージェントは `--dangerously-skip-permissions` つきで起動する — [Tenzin Wangdhen](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)。Tmux-Orchestrator の派生版も同じフラグを前提にする — [appressman/tmux-orchestrator](https://github.com/appressman/tmux-orchestrator)
- [一次] Cursor は integrator を撤去し、重なる編集は "accept some moments of turbulence and let the system naturally converge"。100% の正しさを commit 前に求めず、最後に "final 'green' branch" と fixup pass を置く — [self-driving-codebases](https://cursor.com/blog/self-driving-codebases)
- [二次] Cursor Projects の coordinator は自律マージせず、PR は人間の確認へ戻る（"back to you to check"）— [eesel review](https://www.eesel.ai/blog/cursor-projects-review)
- [一次] Claude Projects: thread のカードの **Merge it** は「人間からのメッセージとして」thread に指示を送るボタン。instructions の例文は "Don't merge, force-push, or change CI configuration without asking me in the thread." — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [一次] Devin: 2024 年の MultiDevin は manager が成功した worker の変更を 1 branch / PR にマージした — [Sept '24 update](https://cognition.com/blog/sept-24-product-update)。2026 年版は coordinator が "resolves any conflicts" するが方法は未記載 — [blog](https://cognition.com/blog/devin-can-now-manage-devins)
- [一次] Kilo は自動マージ無し（Apply to local / PR / 手動）— [agent-manager](https://kilo.ai/docs/automate/agent-manager)。Augment Intent も人間が diff を見てマージする — [Intent blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration)
- [一次] 衝突の事前回避: GitHub "Agents working in parallel can create merge conflicts if they touch the same files. Be thoughtful about partitioning work." — [github.blog](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/)。Claude "Two teammates editing the same file leads to overwrites." — [agent-teams](https://code.claude.com/docs/en/agent-teams)。ccpm は task ごとに `conflicts_with` を宣言 — [ccpm](https://github.com/automazeio/ccpm)
- [一次] 予算と同時数の強制: Claude Agent SDK の `maxBudgetUsd` は到達時に spawn を拒否し、background subagent を止め、query を終了する — [agent-sdk/subagents](https://code.claude.com/docs/en/agent-sdk/subagents)。Devin は child ごとに ACU limit — [docs](https://docs.devin.ai/work-with-devin/advanced-capabilities)。Gas Town は `scheduler.max_polecats` — [scheduler.md](https://github.com/steveyegge/gastown/blob/main/docs/design/scheduler.md)
- [一次] 強制でない上限の例: Claude Projects で会話に頼んだ thread 数の上限は "instructions Claude keeps to, not enforced settings, so a thread limit you give this way isn't a hard cap."。強制されるのは "200 new threads per day" だけ — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [一次] ACP の permission: `session/request_permission` の選択肢は `allow_once` / `allow_always` / `reject_once` / `reject_always`。tool call には `read` / `edit` / `delete` / `move` / `search` / `execute` / `think` / `fetch` / `switch_mode` / `other` の kind が付く — [ACP tool-calls](https://agentclientprotocol.com/protocol/tool-calls)
- [ローカル] Captain の権限表: Task 作成・追撃・wait・depend・radar・status・archive は可、Integrate・承認応答・worktree/ブランチ削除・設定変更は不可 — `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md` §5.2

### Inferences
- 「Captain は承認を推薦まで」は、Anthropic が Agent Teams / cross-session / Projects の 3 か所で書いた規則と同じ結論で、しかも Captain は一歩進んで推薦を UI に添える。承認の摩擦を減らす正攻法は Claude docs が勧める事前 allowlist（ポリシー）で、Captain に答えさせることではない。FLEET-V2 §11 の残る判断「承認のポリシー自動化」はこの方向で決めてよい。
- 「Integrate は人間 gate」を崩す理由は見つからなかった。自律マージで動いているのは (a) 使い捨て前提の研究ハーネス（Cursor）、(b) 検証 gate と bisect を後から足した Gas Town、(c) PR レビューが別に控える cloud 系だけ。ローカルの main を直接触る necoder で (b) の初期と同じ事故を起こす価値は無い。
- Captain が自律でやって良く、かつ効果が大きいのは衝突の差し戻しループ。radar ✗ → 衝突ファイル名つきで `fleet send`（rebase 依頼）→ 再 radar。Gas Town の `REWORK_REQUEST` と同型で、main に触れず Captain の権限内に収まる。
- 「Captain はコードを書かない」はプロンプト上の規律で、Gas Town の言う behavioral な強制に留まる。Claude Agent Teams が「lead が自分で実装を始める」を既知の癖として docs に書いている以上、任意ブランドの Captain でも起きると見ておくべき。ACP クライアントである necoder は構造で縛れる。Captain セッションに限り、`fs/write_text_file` を拒否する、kind が `edit` / `delete` / `move` の tool call を permission で拒否する（または検知して警告と台帳記録）、エージェントが対応していれば `session/set_mode` で読み取り系モードに固定する、の 3 つ。
- 上限（同時 Task 数・Task あたりのトークン・1 日の起動数）は necoder 側で強制する。Claude Projects が「お願いは hard cap ではない」と断っているのが反面教師で、`fleet create` が上限超過で非 0 を返せば、どのブランドの Captain でも暴走しない。
- 分解案の事前承認（Warp `/orchestrate`・Claude Projects の Suggested threads・Augment の plan 承認・Factory の計画対話）は、Captain の最初の 1 手の失敗コスト（worktree と準備スクリプトと初回ビルド）を下げる。`fleet create --propose` のような、台帳に planned だけ積んで人間の確認を待つ経路が考えられる。

### Gaps
- Devin の coordinator が衝突をどう解くか（rebase か、PR の統合か）は未公開。
- Puck・Cursor Projects の coordinator に課された権限上の制約は公式には未記載。

## 6. 人間のインターフェース — リードと個別ワーカーへの話しかけ方、介入の可視化、采配の監査

### Takeaway
「既定はリードに話す・ワーカーにも直接話せる」の 2 つの玄関は全製品に共通する。一方で「人間がワーカーへ直接介入したことをリードに知らせる仕組み」を文書化している製品は見つからず、necoder の `human_send`（台帳 + 次の wake に同乗）は調べた範囲で独自。監査は Gas Town（全行為の帰属）・Warp / Factory（監査ログ・OTel）が先行し、Gas Town の利用者からは可観測性の不満が出ている。

### Cited Findings
- [一次] Claude Agent Teams: "You can also talk to any teammate directly without going through the lead." in-process では agent panel で ↑↓ + Enter（閲覧中の Esc はその teammate のターンを中断、`x` で停止、Ctrl+T で task list）、split-pane では tmux / iTerm2 のペインをクリック。idle の行は 30 秒で隠れ、3 体を超える idle は `N idle agents` に畳まれる — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] split-pane は VS Code の統合ターミナル・Windows Terminal・Ghostty では使えない — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] Claude Projects: thread のメッセージ欄に書くと "goes straight to that thread, while a follow-up in the project conversation reaches it only when Claude matches the follow-up to that thread."。Overview pane が Waiting on you / Ready for review / Landing / Resolved で仕分ける。desktop 通知は会話への投稿・thread のエラー・入力待ちで出る — [claude-projects](https://code.claude.com/docs/en/claude-projects)
- [一次] agent view: 状態は Working / Needs input / Idle / Completed / Failed / Stopped。Needs input の行は "Waiting Xm" を出し、Space の peek パネルから attach せずに返答できる。フッターに `← 2 agents`。行の要約は Haiku 級モデルが書く — [agent-view](https://code.claude.com/docs/en/agent-view)
- [一次] Codex: CLI は `/agent` でスレッドを切り替え、app / IDE は subagent パネルで状態確認・停止・スレッドを開く — [Subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
- [一次] Devin: "Each has its own session link, so you can inspect its work or message it directly." — [blog](https://cognition.com/blog/devin-can-now-manage-devins)
- [一次] Warp: parent を見ている間、上部の pill bar に parent と child 1 体ずつの pill が並ぶ — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [一次] Gas Town: 人間は `gt mayor attach` で Mayor に話す。`gt feed`（リアルタイム）/ `gt dashboard`（Web・既定 8080）/ `gt feed --problems`（詰まり・ゾンビ）/ `gt convoy status`。convoy は `--notify overseer` で完了を通知 — [README](https://github.com/steveyegge/gastown) / [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md)
- [一次] Gas Town の利用報告: DoltHub は 1 週間 "never left [their] Claude Code session with the Mayor"（指示は "maximum six sentences"）— [A Week in Gas Town](https://www.dolthub.com/blog/2026-03-24-a-week-in-gas-town/)。逆に "Sometimes 6 PRs merged and I had no idea when I'd slung them. Other times the system seemed stuck and I couldn't tell why." — [Tenzin Wangdhen](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)
- [一次] Mayor の状況認識の誤り: PR が 2 本しか無いのに Mayor が修正済みと主張し、人間が訂正した — [A Day in Gas Town](https://www.dolthub.com/blog/2026-01-15-a-day-in-gas-town/)
- [一次] GitHub mission control の介入の合図: "Failing tests, integrations, or fetches" / "Unexpected files being created" / "Scope creep beyond what you requested" / "Misunderstanding your intent" / "Circular behavior"。"Catch a problem five minutes in, and you might save an hour of ineffective work." — [github.blog](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/)
- [一次] Factory: mission は "not fire-and-forget"。人間は "pause the orchestrator, describe what you are seeing in plain language, and ask it to recover" — [docs](https://docs.factory.ai/cli/features/missions)
- [二次] Antigravity の Inbox は全 workspace・全エージェントの非同期連絡台帳で、承認が要る決定点で通知が立つ — [Arjan KC](https://arjankc.com.np/blog/google-antigravity-agent-manager-explained/)
- [一次] 監査: Gas Town は commit / bead / event に actor を刻む — [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md)。Factory は "OpenTelemetry logging and audit trails" — [missions](https://factory.com/news/missions)。OpenAI cookbook の PM エージェント例は Traces で "audit every hand-off" — [cookbook](https://developers.openai.com/cookbook/examples/codex/codex_mcp_agents_sdk/building_consistent_workflows_codex_cli_agents_sdk)
- [二次] Warp Oz は "built-in auditability" を掲げる — [Warp newsroom（2026-02-10）](https://www.warp.dev/newsroom/2026/2/10/warp-launches-oz-the-orchestration-platform-for-cloud-coding-agents)
- [ローカル] Captain: ⌘0 で Captain へ、Task カードから直接発話。介入は `human_send` として台帳 + ニュース + 次の wake に同乗。采配は `task_events` の `captain` + 采配ログペイン — `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md` §5.4 / §5.6 / `/Users/daichi/Work/experience/necoder/crates/workspace/src/workspace/captain.rs`

### Inferences
- 介入をリードに知らせる仕組みは、読んだ範囲のどの公式文書にも無かった（無いことの証明ではない）。Cognition が manager-child の失敗要因に "Agents assume they share state with their children when they don't" を挙げている以上、介入を台帳経由でリードに渡す `human_send` は実害のある穴を塞いでいる。
- Claude Projects の「会話側への追記は、Claude が該当 thread に対応づけられた時だけ届く」は曖昧さの源になる。necoder は宛先チップで宛先を常に明示しており、この点は既に優位。
- Overview pane の 4 分類（Waiting on you / Ready for review / Landing / Resolved）と agent view の "Waiting Xm" は、necoder の要対応キュー（Blocked 経過時間順 → Failed → レビュー待ち → 完了未確認）とほぼ同じ結論で、設計の裏付けになる。
- 采配ログはニュース由来で揮発に見える（`render_captain_log` は `notifications.news` を読む）。台帳の `captain` イベントから復元する方が「監査可能」の主張と合う。Gas Town の可観測性の不満は「なぜ止まっているか分からない」に集中しているので、采配ログには「何をしたか」に加えて「何を待っているか」を書かせるとよい。
- Mayor が事実と違う完了を主張した事例は、リードに渡す現況を必ず機械的事実から作る（`captain_facts`）という necoder の方針を支持する。

### Gaps
- Devin / Cursor Projects で、人間の child への直接メッセージが manager に見えるかは不明。
- Puck の Slack / モバイル対応は第三者記事のみで、公式ページは web だけを書いている。

## 7. ベンダー中立性とプロトコル — 別ブランドの混成は可能か、ACP / A2A / MCP の位置づけ

### Takeaway
モデル供給元の製品（Claude Code・Codex・Cursor・Devin・Copilot CLI・Antigravity・Jules）はリードもワーカーも自社ハーネスに閉じている。混成を許すのは中立の立場にいる Warp Oz・Gas Town・Augment Intent・GitHub Agent HQ（人間が指揮）・claude-squad など。ACP は editor↔agent の規格で agent 間の概念を持たないので、「ACP でワーカーを駆動し、CLI/MCP でリードに道具を渡す」Captain の構成は、現行規格の上では素直な組み立てになる。

### Cited Findings
- [一次] Claude Code: "In every approach the workers are Claude sessions. To involve a different tool, expose it to Claude as an MCP server." — [agents](https://code.claude.com/docs/en/agents)
- [一次] Warp: "A parent running with the default Warp Agent can spawn children that run with Claude Code or Codex, and vice versa." — [orchestration](https://docs.warp.dev/platform/orchestration/)
- [一次] Gas Town の runtime プリセット: Claude, Gemini, Codex, Kiro, Cursor, Auggie, Amp, OpenCode, Copilot, Pi, OMP（rig ごとに設定）。帰属データで "Deploy different models on similar tasks and compare outcomes" — [README](https://github.com/steveyegge/gastown) / [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md)
- [一次] Augment Intent: "Intent works with different agent providers (Claude Code, Codex, OpenCode)." — [Intent blog](https://www.augmentcode.com/blog/intent-a-workspace-for-agent-orchestration)
- [二次] GitHub Agent HQ は Anthropic / OpenAI / Google / Cognition / xAI のエージェントを同じ面に載せる。2026-02 に public preview — [GitHub blog](https://github.blog/news-insights/company-news/welcome-home-agents/) / [fundesk](https://www.fundesk.io/github-agent-hq-multi-agent-development-guide)
- [一次] Factory は 1 ハーネス内の役割別マルチモデル: 編成 = Opus 4.6、実装 = Sonnet 4.6 / Opus 4.6、検証 = GPT-5.3-Codex、調査 = Kimi K2.5 — [missions](https://factory.com/news/missions)
- [一次] Cognition: フロンティアモデル同士の組み合わせ（Claude + GPT）は本番で効いた（"some models debug better, some handle visual reasoning better"）。小モデル → 大モデルのエスカレーションは未成熟 — [multi-agents-working](https://cognition.com/blog/multi-agents-working)
- [一次] Copilot CLI `/fleet`: subagent は既定で "a low-cost AI model"。プロンプトや custom agent でモデルを指定できる — [docs](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/fleet)
- [一次] ACP: JSON-RPC 2.0。Agent は "typically run as subprocesses of the Client"。メソッドは `initialize` / `authenticate` / `session/new` / `session/load` / `session/prompt` / `session/set_mode` / `session/cancel`、クライアント側に `session/request_permission` / `fs/*` / `terminal/*` / `elicitation/create`。エージェント間・サブエージェントの概念は仕様に無い — [ACP overview](https://agentclientprotocol.com/protocol/overview)
- [一次] ACP の RFD: `session/list`（2026-03-09）/ `session/resume`（2026-04-22）/ `session/close`（2026-04-23）/ `session/delete`（2026-06-05）/ Session Context Size and Cost（2026-06-05）/ Elicitation（2026-07-24）が Completed。`session/fork`・Proxy Chains・Streamable HTTP & WebSocket transport・v2 Prompting は Draft — [RFD updates](https://agentclientprotocol.com/rfds/updates)
- [一次] A2A v1.0.0: Task の状態は SUBMITTED / WORKING / INPUT_REQUIRED / AUTH_REQUIRED / COMPLETED / FAILED / CANCELED / REJECTED。更新の受け取りは polling / streaming / push notification（webhook）の 3 通り。内部状態・道具を見せ合わない不透明性が原則 — [A2A spec](https://a2a-protocol.org/latest/specification/)
- [二次] A2A は 2026-08-17 に Linux Foundation 傘下の Agentic AI Foundation のホストプロジェクトになった — [Axios](https://www.axios.com/2026/08/17/a2a-agentic-ai-foundation-open-ai-standards) / [A2A blog](https://a2a-protocol.org/latest/blog/2026/08/27/a-new-chapter-for-a2a-joining-the-agentic-ai-foundation/)
- [一次] MCP を制御面に使う例: Devin の manager は child を "through an internal MCP" で調整する — [multi-agents-working](https://cognition.com/blog/multi-agents-working)
- [一次] 逆風: "The `codex mcp-server` command and the standalone `codex-mcp-server` binary have been removed." 代替の app server は独自 JSON-RPC で "isn't an MCP server or a drop-in replacement"、しかも "experimental and isn't supported for production workloads" — [Codex MCP server removal](https://learn.chatgpt.com/docs/mcp-server)。この MCP server を使っていた OpenAI cookbook の Project Manager 例は前提を失った — [cookbook](https://developers.openai.com/cookbook/examples/codex/codex_mcp_agents_sdk/building_consistent_workflows_codex_cli_agents_sdk)
- [一次] CLI と MCP の文脈コスト: "Tools like `gh`, `aws`, `gcloud`, and `sentry-cli` are still more context-efficient than MCP servers because they don't add any per-tool listing." — [costs](https://code.claude.com/docs/en/costs)
- [ローカル] Captain: どのエージェントでも任命でき、道具は `necoder fleet` CLI（同じ操作を MCP `fleet_*` でも公開）— `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md` §5.1 / `/Users/daichi/Work/experience/necoder/docs/FLEET-ARCHITECTURE.md`

### Inferences
- 「リードのブランドを選べる」製品は Warp Oz（ハーネス混在の parent）と Gas Town（runtime プリセット）くらいで、リードを状態ごと差し替えられる（任命・交代・解任が自由）と明言しているものは見つからなかった。Claude Agent Teams は "Lead is fixed"。stateless と任命制の組み合わせは Captain の独自性として成立する。
- Codex が自分を MCP server として出すのをやめたことは、ワーカーの駆動を MCP に賭けない方がよいという信号。ACP はセッションの list / resume / close / delete と usage/cost が 2026 年に安定化しており、ワーカー駆動の共通面として育っている。Captain の道具を CLI に寄せたのも、Anthropic 自身が CLI の文脈効率を認めている点と、どのブランドも shell を持つ点で妥当。
- ACP に agent 間の概念が無いので、Captain ↔ Task の関係は necoder 固有の層（台帳 + fleet CLI）で持ち続けることになる。将来リモートや他社のコントロールプレーンと繋ぐなら、A2A の状態語彙（INPUT_REQUIRED / AUTH_REQUIRED ≒ blocked、push notification ≒ wake）へ `TaskPhase` を写像できるようにしておくと橋渡しが安い。
- ベンダー中立は検証の質にも使える。Factory が検証役に別ベンダーのモデルを当て、Cognition がフロンティア間ペアリングの効果を報告している。Captain が「実装した Task と別ブランドのエージェントに read-only のレビュー Task を切る」のは、necoder でしか手軽にできない組み合わせになる。

### Gaps
- ACP の Proxy Chains RFD の中身（多段エージェントの想定か）は Draft の表題と 1 行説明しか確認していない。
- Warp で parent に Claude Code / Codex を置いた時、spawn の道具がどう渡るか（CLI か MCP か）は docs に無い。

## 8. 証拠 — 成功/失敗の報告、トークン倍率、典型的なワーカー数、既知の病理

### Takeaway
倍率の相場は単体比で 3〜15 倍、時間あたりの支出は Gas Town の実測で約 $100/時。ワーカー数は対話型で 3〜6、研究ハーネスだけが数百。病理は (1) 重複作業と上書き、(2) リードの自己実装・早期終了・事実誤認、(3) drift、(4) 監督役のボトルネック化、(5) 無制限 spawn、(6) 検証不在のまま統合、の 6 つに集約でき、うち (4)(5)(6) は Captain の設計が最初から避けている。

### Cited Findings

#### コスト倍率
- [一次] "agents typically use about 4× more tokens than chat interactions, and multi-agent systems use about 15× more tokens than chats"。BrowseComp では "token usage by itself explains 80% of the variance"。Opus 4 lead + Sonnet 4 subagents は単体 Opus 4 を 90.2% 上回った（研究タスク）— [Anthropic multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [一次] "Agent teams use approximately 7x more tokens than standard sessions when teammates run in plan mode" — [costs](https://code.claude.com/docs/en/costs)
- [二次] 3 teammates のチームは同じ作業の単一セッション比で "roughly three to four times the tokens" — [HackerNoon](https://hackernoon.com/navigating-claude-code-agent-teams-in-practice)
- [一次] Factory Missions: median の mission は通常セッションの 12 倍のトークン。median 約 2 時間、14% が 24 時間超、最長 16 日 — [missions](https://factory.com/news/missions)
- [一次] Gas Town: "This whole sixty minute experience cost me about $100 in Claude tokens. That's about 10X the cost of a normal Claude Code session per unit time." — [A Day in Gas Town](https://www.dolthub.com/blog/2026-01-15-a-day-in-gas-town/)。1 週間（1 日 6〜8 時間）で $3,000 — [A Week in Gas Town](https://www.dolthub.com/blog/2026-03-24-a-week-in-gas-town/)
- [二次] Yegge 本人: "This is my third $200/month Claude Pro Max plan. My first two have maxed out" — [paddo.dev](https://paddo.dev/blog/gastown-two-kinds-of-multi-agent/)
- [二次] Cursor Projects: subagent 5 体で "roughly five times the tokens" — [eesel review](https://www.eesel.ai/blog/cursor-projects-review)
- [一次] Anthropic の C コンパイラ: 16 体・約 2,000 セッション・入力 20 億 / 出力 1.4 億トークン・"$20,000 in API costs"・10 万行 — [building-c-compiler](https://www.anthropic.com/engineering/building-c-compiler)
- [二次] ruflo: `init` だけで約 120k トークン、hive-mind の 5 体が実装前に約 20 分交渉し、queen が仕様（localStorage）を "over-engineering" と判断して勝手に cookie へ変えた — [Gerald Chen](https://chenguangliang.com/en/posts/claude-code-multi-agent-orchestration-plugins/)

#### ワーカー数の相場
- [一次] Claude Agent Teams: "Start with 3-5 teammates for most workflows." "Three focused teammates often outperform five scattered ones." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [ソース] Codex の既定: v1 は 6 スレッド、v2 は 4 — [config/mod.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/config/mod.rs)
- [一次] Anthropic Research の規模則: 単純 = 1 体、比較 = 2〜4 体、複雑 = 10 体超 — [multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [一次] Gas Town は 20〜30 体を掲げる — [README](https://github.com/steveyegge/gastown)。実地報告では 7 本投げて 6 本がマージされ 1 本が依存で停止 — [Tenzin Wangdhen](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)
- [一次] Cursor 研究は "Hundreds of concurrent workers"、ピーク "~1,000 commits per hour across 10M tool calls over a period of one week" — [scaling-agents](https://cursor.com/blog/scaling-agents) / [self-driving-codebases](https://cursor.com/blog/self-driving-codebases)
- [一次] ccpm は 1 epic 最大 12、Kilo は 1 リクエスト 20 task / 4 バージョン、MultiDevin は worker 10 — [ccpm](https://github.com/automazeio/ccpm) / [Kilo](https://kilo.ai/docs/automate/agent-manager) / [Cognition](https://cognition.com/blog/sept-24-product-update)

#### 成功の報告
- [一次] Cursor: ブラウザ（100 万行超・1 週間）、Solid → React 移行（+266K / −193K・3 週間超）、動画レンダリング 25 倍高速化。"GPT-5.2 models are much better at extended autonomous work"、Opus 4.5 は "tends to stop earlier and take shortcuts" — [scaling-agents](https://cursor.com/blog/scaling-agents)
- [一次] Gas Town で DoltLite を 1 週間開発し、初日に "all 87,000 SQLite acceptance tests passing"。効いた条件は深い領域知識・きれいな境界・既存の大規模テスト・小さめのコードベース・大きな予算 — [A Week in Gas Town](https://www.dolthub.com/blog/2026-03-24-a-week-in-gas-town/)
- [一次] Factory の Slack クローン事例: 6 milestone が各 2〜4 回の検証で収束。検証が 81 件を見つけ、修正 feature 21 本（実装作業の 34.4%）を生んだ — [missions-architecture](https://factory.com/news/missions-architecture)
- [一次] "Devin Review catches an average of 2 bugs per PR, of which roughly 58% are severe"。レビュー役は実装役と別コンテキストで始めた方が良く効く — [multi-agents-working](https://cognition.com/blog/multi-agents-working)
- [一次] Cursor Projects のベンダー自己申告: "new users merge 30% more PRs"、主利用者は 6 倍 — [projects](https://cursor.com/blog/projects)。第三者は "treat it the way you'd treat any vendor's own productivity stat" — [eesel review](https://www.eesel.ai/blog/cursor-projects-review)

#### 病理
- [一次] 重複・上書き: Anthropic Research の初期は "spawning 50 subagents for simple queries" と分業不全 — [multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)。C コンパイラでは "every agent would hit the same bug, fix that bug, and then overwrite each other's changes"（GCC を正解器にして分割可能にした）— [building-c-compiler](https://www.anthropic.com/engineering/building-c-compiler)
- [一次] 暗黙の決定の衝突: 並列 subagent が別々の前提で作り、合成できない（Flappy Bird の例）— [dont-build-multi-agents](https://cognition.com/blog/dont-build-multi-agents)
- [一次] リードの癖: 自分で実装を始める、全 task 完了前に終了を宣言する、teammate が完了マークを忘れて依存 task が詰まる（"Task status can lag"）— [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] 役の盛りすぎ: Cursor の continuous executor は "random sleeping, refusing to plan, claiming premature completion" — [self-driving-codebases](https://cursor.com/blog/self-driving-codebases)
- [一次] drift と長期の誤差蓄積: "Long-running plans accumulate errors. Milestone validation catches most, but the orchestrator still scopes too broadly sometimes" — [missions](https://factory.com/news/missions)
- [一次] 自己評価バイアス: "An implementer accumulates assumptions far from the correct evaluation zone. A fresh validator's exploration converges into it." — [missions-architecture](https://factory.com/news/missions-architecture)
- [一次] 監督役のボトルネック: Cursor の integrator（"created more bottlenecks than it solved"）— [scaling-agents](https://cursor.com/blog/scaling-agents)。Gas Town の Witness（patrol 直列）— [polecat-self-managed-completion.md](https://github.com/steveyegge/gastown/blob/main/docs/design/polecat-self-managed-completion.md)
- [一次] 運用の脆さ（Gas Town）: 週末までに "141 orphaned Claude Code processes"、daemon が動いていなかった、設定ファイルの丸ごと上書き、"Most work gets done; some work gets lost." — [Tenzin Wangdhen](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)
- [一次] 設計の過剰さへの批判: "The number of overlapping and ad hoc concepts in this design is overwhelming."、"the mayor is dumb as rocks the witness regularly forgets to look at stuff"。Yegge 自身も "Gas Town is complicated. Not because I wanted it to be, but because I had to keep adding components until it was a self-sustaining machine." — [maggieappleton.com](https://maggieappleton.com/gastown)
- [一次] 人間側のボトルネック: "Gas Town churns through implementation plans so quickly that you have to do a LOT of design and planning to keep the engine fed." — [maggieappleton.com](https://maggieappleton.com/gastown)
- [一次] HN の体験談: Mayor が convoy を作らず素の Claude と変わらなかった（bigwheels）、Beads のマージ衝突で片側を捨てた（laxori666）、"complexity of k8s combined with AI-fueled solipsism"（rsanheim）、2〜3 体を自分で操る方が完成まで行けた（tokioyoyo）— [Hacker News](https://news.ycombinator.com/item?id=46458936)
- [一次] コーディングは研究より並列化しにくい: "fewer truly parallelizable tasks than research" — [multi-agent-research-system](https://www.anthropic.com/engineering/multi-agent-research-system)。Claude docs も "For sequential tasks, same-file edits, or work with many dependencies, a single session or subagents are more effective." — [agent-teams](https://code.claude.com/docs/en/agent-teams)
- [一次] 検証器の質が上限を決める: "it's important that the task verifier is nearly perfect" — [building-c-compiler](https://www.anthropic.com/engineering/building-c-compiler)

### Inferences
- necoder の規模（並べるのは 3 枚まで・個人の手元）では、倍率は 3〜7 倍（Claude Agent Teams の相場）に収まる見込み。Captain 自体のコストは wake 回数 × Captain スレッドの文脈長で決まるので、§2 のスレッド交代が効く。
- 成功例に共通する条件は「強い検証器（大規模な既存テスト・正解器・別コンテキストの validator）」と「書き込み範囲の分離」。失敗例に共通するのは「検証なしの統合」と「監督役の直列化」。Captain の設計は後者 2 つを避けている（人間 gate・イベント駆動で patrol 無し）が、前者の検証器は Task のブリーフと人間任せで、仕組みとしてはまだ無い。
- "Task status can lag"（ワーカーの自己申告頼み）を necoder は構造的に避けている。Task の phase は ACP のターン終了と permission 待ちから necoder が導出しており、エージェントの申告に依存しない。
- Gas Town への批判の核は「概念と役割の多さ」と「可観測性の不足」。Fleet v2 の「ユーザーに見せる概念は Task・タブ・Captain の 3 つ」という絞り込みは、この批判への正しい応答になっている。

### Gaps
- コーディングで「リードあり / なし」を同条件で比べた公開ベンチマークは見つからなかった（Anthropic の 90.2% は研究タスク、Cursor は定性的、ruflo の SWE-bench 84.8% は自己申告で、監査側は実行経路の実在を疑っている）。
- Claude Agent Teams / Codex subagents の実利用での失敗率は数値として公開されていない。
- Devin Managed Devins の第三者による実地レビューは見つけられなかった。

## 9. Captain への含意

### Takeaway
Captain の骨格（コードを書かない単一リード・stateless + 台帳・イベント駆動・承認は推薦まで・統合は人間・ブランド不問・介入の可視化）は、2026 年の各社の到達点か、それより規律のある側にあり、変える理由は見つからなかった。直すべきは理念と実装のずれ（Captain スレッドの長寿命化・wake キューの揮発・「書かない」がプロンプト頼み）で、足すべきは検証役・ブリーフの型・上限の強制・分解案の事前承認。

### Cited Findings

#### システム別の対比（Captain と同じ点 / 違う点）

| システム | 同じ点 | 違う点（Captain から見て） | 出典 |
|---|---|---|---|
| Claude Agent Teams | 単一リード・イベント通知・承認の代行禁止・人間はワーカーに直接話せる | lead は固定で状態はコンテキスト（resume 不可）・worktree 隔離なし・Claude 限定・lead が自分で実装しがち | [agent-teams](https://code.claude.com/docs/en/agent-teams) |
| Claude Projects | リードは報告だけ見る・承認は thread 内で人間・全履歴に頼らない | cloud 限定・Claude 限定・thread 数の上限はお願いベース・マージは thread に指示できる | [claude-projects](https://code.claude.com/docs/en/claude-projects) |
| Codex subagents | 深さ 1・完了通知の注入・要約だけ戻す・"wait sparingly" | root 自身が実装も統合もする（リードが書く）・Codex 限定・外部台帳なし | [multi_agents_spec.rs](https://github.com/openai/codex/blob/46aa019e805352c9d7fd9a740cbf7f8b9aeb162d/codex-rs/core/src/tools/handlers/multi_agents_spec.rs) |
| Cursor Projects | リードは書かない・常に応答可能・共有の記憶ファイル・イベント購読 | cloud VM・Cursor 限定・規模は "thousands" を標榜・承認の扱いは未記載 | [projects](https://cursor.com/blog/projects) |
| Cursor 研究ハーネス | planner は書かない・handoff が上へ流れる | 再帰階層・統合役なしで turbulence を許容・judge あり | [self-driving-codebases](https://cursor.com/blog/self-driving-codebases) |
| Devin Managed Devins | manager + 隔離された child・child に直接話せる・child ごとの予算 | manager が衝突解決と取りまとめまで行う・自己リマインドで見回る・全 trajectory を読める | [blog](https://cognition.com/blog/devin-can-now-manage-devins) |
| Gas Town | 台帳が記憶・セッションは使い捨て・ブランド混在・全行為の帰属 | 役割 7 種 + patrol の常時ループ・Refinery が自律マージ・yolo 既定・高コスト | [overview.md](https://github.com/steveyegge/gastown/blob/main/docs/overview.md) |
| Roo Orchestrator | リードは道具を絞られ調整に専念・要約だけ戻る | 直列のみ・製品は 2026-05 に終了 | [boomerang-tasks](https://roocodeinc.github.io/Roo-Code/features/boomerang-tasks) |
| Factory Missions | リードは実装しない・状態はファイルと git・詰まったら人間へ戻す | 直列実行・validator が仕組みとして入る・1 ハーネス内マルチモデル | [missions-architecture](https://factory.com/news/missions-architecture) |
| Warp Oz | 1 段の parent/child・ブランド混在・BLOCKED を状態として持つ | 永続メッセージバスと連番・spawn 前に人間の承認・parent が結果をマージし得る | [orchestration](https://docs.warp.dev/platform/orchestration/) |
| Amp Puck | 「エージェントを管理するエージェント」・スレッドの整理 | 権限・上限が未記載の実験・自己スケジュール | [meet-puck](https://ampcode.com/news/meet-puck) |
| GitHub mission control / Kilo Agent Manager / claude-squad / agent view | worktree 隔離・人間が裁く列 | AI のリードがいない | [github.blog](https://github.blog/ai-and-ml/github-copilot/how-to-orchestrate-agents-using-mission-control/) |

### Inferences

#### 守るもの（変えない）
1. **Captain はコードを書かず、main に住む調整専任** — Cursor Projects（"doesn't write code itself"）、Cursor 研究の root planner（"does no coding itself"）、Factory、Roo が同じ結論。逆に Claude Agent Teams は「lead が自分で実装を始める」を既知の癖として docs に載せている（§1・§5）。
2. **stateless + 追記専用の台帳** — Gas Town の Persistent Roles / Ephemeral Sessions、Claude Projects の「全履歴では動かさない」、Cursor の "periodic fresh starts"、Anthropic Research の計画の外部保存と同じ方向。"Lead is fixed"・teammate の resume 不可は、状態をコンテキストに置いた代償（§2）。
3. **イベント駆動の wake（ポーリング禁止・busy 中はまとめる・Blocked は 15 秒後に 1 回）** — Claude Agent Teams（"The lead doesn't need to poll"）、Codex（"Call wait_agent very sparingly"）、Warp のメッセージバスと同型。patrol 型の Gas Town は設計文書が自ら Witness を直列ボトルネックと認め、実測 $100/時（§3・§8）。
4. **承認は推薦まで・Integrate は人間 gate** — Anthropic は Agent Teams / cross-session / Projects の 3 か所で「他エージェントの発言は同意にならない」と明文化。自律マージは DoltHub の事故（テスト失敗のまま統合 → force push）が反証で、Cursor も integrator を撤去（§5）。
5. **digest だけ渡す** — Claude / Codex / Roo / Factory / Claude Projects の共通解（§4）。
6. **ブランド不問の任命制** — リードごと差し替えられる製品は見つからなかった。Warp / Gas Town / Augment Intent がワーカーの混成まで（§7）。
7. **介入の可視化（`human_send`）** — 同等の仕組みを文書化した製品は無く、Cognition の「共有状態の誤った仮定」への直接の対策になる（§6）。
8. **1 Task = 1 worktree + read-only の radar、phase は necoder が導出** — 隔離なしの Agent Teams は上書きを注意書きで避けるしかなく、自己申告の task 状態は遅れる（§5・§8）。

#### 足すもの（優先順）
- **P0-1 Captain スレッドの交代**: N 回の wake または文脈使用率の閾値で新しい ACP セッションに切り替え、台帳から現況を渡し直す。現状は同名スレッドを再利用し続けるので、stateless の配当を受け取っていない。根拠は Cursor の freshness 機構、Gas Town の使い捨てセッション、Claude Projects、Cognition の Context Rot（§2）。
- **P0-2 wake の配達状態を台帳へ**: `captain_pending` をメモリではなく「Captain が最後に読んだ `task_events` の位置」で表し、再起動後も差分を 1 通で渡す。順序は台帳の連番で保証する。根拠は Warp の global sequence、Gas Town の mail = bead、Claude の mailbox ファイル（§2・§3）。
- **P0-3 「書かない」を構造で守る**: Captain セッションに限り `fs/write_text_file` を拒否し、kind が `edit` / `delete` / `move` の tool call を拒否（最低でも警告 + 台帳記録）し、対応エージェントは読み取り系モードに固定する。ACP クライアントの立場でできるのでブランドに依存しない。根拠は Roo の道具制限、Gas Town の "Structural, not behavioral"、Agent Teams の癖（§5）。
- **P1-1 ブリーフの型**: `fleet create` に「目的 / 受入条件と検証コマンド / 触ってよい範囲（他 Task と disjoint）/ 触らない範囲 / 終了時の報告形式」を持たせる。手順ではなく制約と受入条件を書かせる。根拠は Anthropic の 4 要素、Codex の "disjoint write set"、Cursor の "Constraints are more effective than instructions"、Cognition の「過剰に細かい manager は裏目」（§4）。
- **P1-2 引き継ぎノートの型**: ワーカーの最終メッセージに「やったこと / 変更ファイル / 実行した検証と結果 / 懸念・逸脱」を要求し、digest の入力にする。根拠は Cursor の handoff、Gas Town の `POLECAT_DONE`、Codex の "list the file paths it changed"（§4）。
- **P1-3 検証役**: review_ready になった Task に、Captain が read-only のレビュー Task を（できれば別ブランドで）切り、結果を要対応カードに添える。radar は衝突しか見ない。根拠は Factory（81 件検出・修正が実装の 34.4%）、Devin Review（2 bugs/PR・58% severe・別コンテキストが効く）、Cursor judge、Jules の Planning Critic、Carlini の "task verifier is nearly perfect"、フロンティア間ペアリング（§7・§8）。
- **P1-4 上限の強制**: 同時 Task 数・Task あたりのトークン・1 日の起動数を necoder 側で強制し、超過時は `fleet create` が非 0 を返す。根拠は Codex（6 / 4 本・深さ 1）、Agent SDK の `maxBudgetUsd`、Devin の ACU limit、Gas Town の `max_polecats`、Anthropic 初期の「50 subagents」、Claude Projects の「お願いは hard cap ではない」（§5）。
- **P1-5 分解案の事前承認（任意設定）**: Captain が planned の Task 群を提案し、人間が承認してから worktree を切る。根拠は Warp `/orchestrate`、Claude Projects の Suggested threads、Augment の plan 承認、Factory の計画対話。worktree + 準備スクリプト + 初回ビルドの空振りを減らす（§5）。
- **P1-6 衝突の差し戻しループ**: radar ✗ → 衝突ファイルつきで `fleet send`（rebase 依頼）→ 再 radar を Captain の標準手順にする。main に触れず権限内で完結する。根拠は Gas Town の `REWORK_REQUEST`（§5）。
- **P2-1 放置の再エスカレーションは人間へ**: Blocked / Failed が閾値を超えたら OS 通知などで人間に上げ直す。Captain を何度も起こさない。根拠は Gas Town の `stale_threshold` / `max_reescalations`、agent view の "Waiting Xm"、Claude Projects の desktop 通知（§3・§6）。
- **P2-2 掘り下げの read-only 手段**: `fleet log <task> --tail N` のように、Captain が要求した時だけ transcript の一部を渡す。根拠は Cognition の "share full agent traces" と Devin の全 trajectory 閲覧（§4）。
- **P2-3 プロジェクトの学びを brief に同乗**: `.necoder/` 配下の短い共有ノートか台帳の lesson イベントを、新しい Task のブリーフに足す。根拠は Claude Projects の `MEMORY.md`、Cursor Projects の共有ファイル、Factory の `AGENTS.md`、Warp の Agent Memory（§2）。
- **P2-4 采配ログを台帳から復元し、「何を待っているか」を書かせる**: Gas Town の利用者の不満は「なぜ止まっているか分からない」に集中している（§6）。
- **P2-5 A2A 語彙への写像を用意**: `TaskPhase` ↔ A2A の INPUT_REQUIRED / AUTH_REQUIRED / COMPLETED / FAILED、wake ↔ push notification。ACP に agent 間の概念は無く、Proxy Chains は Draft（§7）。

#### 避けるもの
- **自律マージ役（Refinery 型）と、人間 gate の段階的な緩和** — DoltHub の事故、Cursor の integrator 撤去（§5）。
- **ピア同士の自己調整（共有ファイル + ロック）と自由メッシュ** — Cursor で 20 体が 2〜3 体分に落ちた。Cognition は "mostly a distraction"。ruflo は合議に 20 分（§1・§8）。横の伝達は Captain の `fleet send` と台帳で足りる。
- **LLM による見回りループと役割の増殖** — Gas Town の patrol・heartbeat 3 系統・役割 7 種は、コストと誤検知と「概念が多すぎる」批判を生んだ（§3・§8）。
- **Captain に複数の役を盛ること**（計画 + 実装 + 判定）— Cursor の continuous executor が壊れた原因。検証は別 Task に切る（§1・§8）。
- **Captain のターンを `fleet wait` で塞ぐこと** — リードは常に応答可能であるべき（Cursor Projects の "never blocked"、Codex の "Do not repeatedly wait by reflex"）。wake がイベント駆動なので、Captain はターンを終えればよい（§3）。
- **ワーカーの yolo 既定** — Gas Town と Tmux-Orchestrator は `--dangerously-skip-permissions` 前提。necoder は permission gate + 推薦 + 事前 allowlist の側に留まる（§5）。
- **上限を「Captain へのお願い」で済ませること** — Claude Projects が hard cap ではないと断っている（§5）。
- **ワーカーの駆動を「エージェントが MCP server を出すこと」に賭けること** — Codex は `codex mcp-server` を削除した。ワーカーは ACP、Captain の道具は CLI（MCP は併設）の現構成を保つ（§7）。

### Gaps
- 上の P0〜P2 は公開資料と `captain.rs` / FLEET-V2 の読解からの推論で、necoder 上での実測（Captain の wake あたりトークン・スレッド交代の効果・構造ガードで壊れるブランドの有無）はしていない。
- ACP エージェントごとに、`session/set_mode` の対応状況と、編集系 tool call が必ず `session/request_permission` を通るかは未調査（構造ガード P0-3 の実現性に直結する）。
- FLEET-V2 §5.6 の 5 秒デバウンスは、`captain.rs` と `wake_captain` の呼び出し元（`notifications.rs` / `project_session.rs`）を読んだ範囲では実装が見当たらない。他の経路（タイマーや別モジュール）にある可能性までは潰していない。
