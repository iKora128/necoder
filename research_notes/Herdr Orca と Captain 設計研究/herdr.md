# herdr — 複数 AI コーディングエージェントを並走・監視する Rust 製 TUI（2026-09-20 時点の深掘り）

調査日: 2026-09-20。一次資料は `gh api` で **生のまま**読んだ GitHub リポジトリ（README / docs 0.9.1 版 / CHANGELOG / ソース / issue / discussion / API schema）で、ここからの引用は原文どおり。
ブログ・HN・note・Zenn は WebFetch（小型モデルの要約）経由なので、そこからの「引用」は**要約器が抜き出した文言**であり一字一句は未照合（該当箇所に「※要約経由」と付けた）。
数値（star・DL・版）はすべて取得日つき。necoder 側の前提（`docs/research/fleet-ux-2026-09.md` / `docs/FLEET-V2.md`）と食い違う点は **【necoder 文書との相違】** と明記した。

## 1. herdr とは何か（リポジトリ・作者・ライセンス・言語・初版・リリース頻度・採用シグナル・現行版・開発は続いているか）

### Takeaway
herdr は Can Celik（GitHub: ogulcancelik）がほぼ 1 人で書いた Rust 製の「エージェント対応ターミナルマルチプレクサ」で、2026-03-27 の v0.1.0 から半年で v0.9.1（2026-09-16）・star 39.7k に達し、YC F26 入りと $6M seed を経て **Herdr, Inc.** の製品になった。ライセンスは **2026-07-22 に AGPL-3.0-or-later（+商用デュアル）から Apache-2.0 へ変更**されており、「Apache-2.0」という理解は現時点で正しいが 7 月以前の記事は AGPL と書いている。

### Cited Findings
- リポジトリは `herdrdev/herdr`、説明文 "the runtime your coding agents live on"、主言語 Rust、ライセンス Apache-2.0、作成 2026-03-27T17:54:33Z、star **39,703** / fork 2,993 / watcher 105 / open issue+PR 350、最終 push 2026-09-20、archived=false（いずれも 2026-09-20 取得） — [GitHub API: repos/herdrdev/herdr](https://api.github.com/repos/herdrdev/herdr) / [repo](https://github.com/herdrdev/herdr)
- 所有者は Organization `herdrdev`（作成 2026-07-25・説明 "open agent runtime"・連絡先 hey@herdr.dev・公開 repo は `herdr` と `herdr-nix` の 2 つ） — [GitHub API: orgs/herdrdev](https://api.github.com/orgs/herdrdev)
- 旧 URL は `github.com/ogulcancelik/herdr`（Coles 記事・crates.io の repository 欄・リリース bot のリンクがこの URL のまま） — [coles.codes](https://coles.codes/posts/herding-agents-with-herdr/) / [crates.io](https://crates.io/crates/herdr)
- **出典の日付の不整合（要注意）**: Coles 記事はページ上の日付が "10 June 2026" だが、本文は "on the 0.7.x line (I'm running 0.7.3)" と書き、`herdr plugin install` も使う。GitHub 上の v0.7.3 公開は 2026-07-07、plugin install の追加は 0.7.0（2026-06-15）なので、**日付が誤りか、無記名で後日更新された**かのどちらか。本ノートでは同記事を「2026-06〜07 の運用記」として扱う — [coles.codes](https://coles.codes/posts/herding-agents-with-herdr/) / [releases](https://github.com/herdrdev/herdr/releases)
- 作者は GitHub `ogulcancelik`（表示名 "Can Celik"・commit 名義 "Ogulcan Celik"・follower 851）。コミット数は ogulcancelik 1,251 / akbash-bot 116 / JJLiebig 89 / kangal-bot 76 / github-actions 48 / 以下 1 桁（dhh が 5 commits で名を連ねる）（2026-09-20 取得） — [contributors](https://github.com/herdrdev/herdr/graphs/contributors) / [ogulcancelik](https://github.com/ogulcancelik)
- LICENSE の履歴: 2026-03-22 "initial release" → 2026-05-26 "docs: clarify dual licensing" → **2026-07-22 "chore: relicense herdr under apache-2.0"** — [LICENSE の commit 履歴](https://github.com/herdrdev/herdr/commits/master/LICENSE)
- CHANGELOG 0.8.0（2026-08-03）: "Relicensed Herdr from AGPL-3.0-or-later to Apache-2.0." 事前に issue #1340「Relicensing herdr from AGPL-3.0 to Apache-2.0 - I need your permission」（2026-07-11・コメント 39）で貢献者の同意を集めた — [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md) / [#1340](https://github.com/herdrdev/herdr/issues/1340)
- 2026-07-10 公開（09-02 更新）のレビューは今も "AGPL-3.0-or-later"・"Commercial license available"・"Bus factor is 1" と記載（※要約経由。ライセンス記述は古い） — [bitdoze](https://www.bitdoze.com/herdr-agent-multiplexer/)
- リリース: 初版 v0.1.0 = 2026-03-27、最新 v0.9.1 = 2026-09-16。GitHub Releases は **stable 57 本 + preview 31 本**（2026-09-20 取得）。3〜5 月はほぼ毎日〜数日おき（例: 0.4.0〜0.4.11 が 04-05〜04-16）、7 月以降の stable は 1〜3 週おき（0.7.4 07-15 / 0.7.5 07-21 / 0.8.0 08-03 / 0.8.2 08-19 / 0.9.0 09-07 / 0.9.1 09-16） — [releases](https://github.com/herdrdev/herdr/releases)
- preview チャンネルは 0.6.7（2026-06-03）で追加。理由は "as Herdr transitions toward less frequent, more stable releases" — [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- 技術スタック（Cargo.toml で確認）: `version = "0.9.1"`・`license = "Apache-2.0"`・ratatui 0.30 / crossterm 0.29 / portable-pty =0.9.0（`vendor/portable-pty` にパッチ）/ tokio / interprocess / regex / toml。言語比は Rust 約 10.4MB が大半 — [Cargo.toml](https://github.com/herdrdev/herdr/blob/master/Cargo.toml) / [languages API](https://api.github.com/repos/herdrdev/herdr/languages)
- README の自己定義: "herdr keeps terminals running in a background server when you close the client or lose your SSH connection"・"every pane is marked working, blocked, or idle"・"agents drive herdr through the cli and socket api: they can spawn panes, prompt each other, and wait until another agent is genuinely blocked"・"herdr doesn't wrap or replace them"・"one rust binary, no electron"（※要約経由） — [README](https://github.com/herdrdev/herdr)
- インストール: `curl -fsSL https://herdr.dev/install.sh | sh` / `brew install herdr` / `mise use -g herdr` / Windows は PowerShell の `install.ps1`（※要約経由） — [README](https://github.com/herdrdev/herdr)
- Homebrew formula `herdr` 0.9.1・install 数 **30 日 10,206 / 90 日 48,476 / 365 日 52,280**（2026-09-20 取得） — [formulae.brew.sh API](https://formulae.brew.sh/api/formula/herdr.json)
- crates.io には **0.1.0（2026-03-27）しか無く** DL 1,189。配布経路ではない（2026-09-20 取得） — [crates.io/crates/herdr](https://crates.io/crates/herdr)
- star の推移（出典ごとに日付が違う）: "nearly 15,000"（2026-07-10 記事）→ 25,000 + 340,000 downloads + plugin 500+（2026-08-06 作者ブログ）→ 31,766（2026-08-23 記事）→ 39,347（2026-09-08 作者ブログ）→ 39,703（2026-09-20 API） — [bitdoze](https://www.bitdoze.com/herdr-agent-multiplexer/) / [YC 参加の告知](https://herdr.dev/blog/herdr-is-joining-y-combinator/) / [developersdigest](https://www.developersdigest.tech/blog/herdr-deep-dive-agent-terminal-multiplexer) / [seed の告知](https://herdr.dev/blog/herdr-raised-a-seed/)
- 2026-08-06: Y Combinator（F26）参加を告知。"The runtime, what you use right now, stays free." / Apache-2.0 維持 / 商用は「laptop・VPS・sandbox に散らばった機械をつなぐ」方向（"those machines are disconnected. They should be connected."）（※要約経由） — [herdr.dev/blog](https://herdr.dev/blog/herdr-is-joining-y-combinator/)
- 2026-09-08: **$6M seed**（lead: Bessemer Venture Partners。Y Combinator / e2vc / Tobi Lütke / Dane Knecht / Görkem Yurtseven）。採用中（応募は SSH 経由）（※要約経由） — [herdr.dev/blog](https://herdr.dev/blog/herdr-raised-a-seed/)
- 公式サイトのフッタ: "Apache 2.0 · Herdr, Inc. · v0.9.1"、"Herdr Cloud coming soon"（2026-09-20 取得・※要約経由） — [herdr.dev](https://herdr.dev)
- 量の指標: issue 累計 2,111 / PR 累計 926 / discussion 累計 1,357（2026-09-20 取得）。トリアージは `akbash-bot`、リリース告知は `kangal-bot` が自動で行う — [issues](https://github.com/herdrdev/herdr/issues) / [discussions](https://github.com/herdrdev/herdr/discussions)
- docs は英語に加えて `ja/` と `zh-cn/` の全ページ翻訳を同梱 — [docs ツリー](https://github.com/herdrdev/herdr/tree/master/docs/next/website/src/content/docs/ja)

### Inferences
- 開発は極めて活発（最終 push が調査当日・直近 2 週間で stable 2 本）。ただしコミットの約 8 割が作者 1 人 + bot で、seed 後の採用が進むまで実質 bus factor 1。
- org への移管は org 作成日（2026-07-25）以降＝ Apache 化（07-22）→ org 化（07-25）→ YC 告知（08-06）→ seed（09-08）が一続きの「会社化」の動き。
- 商用の軸は「複数マシンの接続 / Herdr Cloud」で、**オーケストレーション機能を売る気配は無い**（§5 と整合）。
- pre-1.0 で破壊的変更が多い（0.7.5 で `wait` → `agent wait`、`agent send` → `agent send-keys` に置換など）。herdr の CLI に依存する設計は版固定が要る。

### Gaps
- YC の会社ページ（ycombinator.com/companies/herdr）と HN の YC 告知スレ（item?id=49201003）は検索結果に出ただけで本文は未読。チーム人数は不明。
- 「libghostty-vt を vendoring している」は二次情報（検索要約）でしか見ておらず、リポジトリでは `src/ghostty/` と `vendor/` の存在を確認したのみ。
- "340,000 downloads" の算出方法（インストーラ DL か brew 込みか）は不明。

## 2. 中核概念と語彙 — session / workspace（= Space）/ tab / pane / agent / worktree の対応と、「3 列・各 ≥50 桁」ルールの出所

### Takeaway
階層は **session（サーバの名前空間）> workspace（UI 上の呼び名は Space）> tab > pane（本物の端末）> agent（pane 内で認識されたプロセス）**。worktree は「Git checkout の来歴を持つ普通の workspace」で、親 workspace の下に字下げでグループ表示される。**「横 3 本まで・各 50 桁以上」は herdr の仕様ではなく、Zenn 記事の著者が自作 hook スクリプトに入れた閾値**である。

### Cited Findings
- Workspace: "A workspace is the top-level project container. Use one workspace per repo, task, or investigation." / "Its sidebar state rolls up from the agents inside it" — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- Tab: "A tab is a layout inside a workspace. Use tabs to separate views like `agents`, `logs`, `server`, or `review`." — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- Pane: "A pane is a real terminal." 右 / 下に split・改名・CLI から read / 入力 / close 可能 — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- Agent: "An agent is a process Herdr recognizes inside a pane. Herdr detects agents from foreground processes, screen manifests, and optional integrations." — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- Session: "A session is a persistent Herdr server namespace." / "Use workspaces first. Use named sessions when you need completely separate panes, sockets, and persisted runtime state." — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- Client / server: "The server owns panes and process state. The client is the terminal UI attached to that server." `ctrl+b q` で detach、`herdr server stop` で終了。複数 client は各自の workspace / tab を見られる — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- UI 上の語は **Space**: 設定キーが `[ui.sidebar.spaces]`・`ui.agent_panel_sort = "spaces"`（"workspaces" は別名として受理）、文中も "the active Space and focused Agent row" — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx) / [config-reference.json](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/data/config-reference.json)
- 作者の一文での定義: "A terminal pane belongs to an agent. A pane belongs to a tab. Tabs belong to a project."（※要約経由） — [YC 参加の告知](https://herdr.dev/blog/herdr-is-joining-y-combinator/)
- Worktree: "Worktrees are normal Herdr workspaces with Git checkout provenance." `worktree create` は checkout を作り、workspace として開き、親 repo の workspace とグループ化する — [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx)
- グループ化された worktree は普通の workspace として focus / 改名 / close でき、親行を閉じるとグループ全体が閉じるが checkout フォルダもブランチも消えない — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)
- ID 体系: workspace `w1` / tab `w1:t1` / pane `w1:p1`。閉じた ID は再利用しない。pane を別 workspace へ move すると ID が変わる。ID と agent 名は **1 サーバ内でだけ一意** — [SKILL.md](https://github.com/herdrdev/herdr/blob/master/skills/herdr/SKILL.md)
- 0.9.0（2026-09-07）から machine が最上位に加わった: "Manage Local and saved SSH machines from one Herdr window, with a combined agent list, machine-scoped navigation, notifications, and automatic reconnects." — [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- 永続化の 4 経路: detach/reattach（プロセス生存）/ server 再起動（layout のみ復元・プロセスは死ぬ）/ pane screen history（既定 off・秘密が残るため）/ **native agent session restore**（`claude --resume <id>`・`codex resume <id>` 等。既定 on）/ live handoff（実験的） — [session-state.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/session-state.mdx)
- **「3 列・≥50 桁」の出所**: Zenn 記事の hook スクリプト内のコメント「横に並べる列の上限と、1列あたりの下限幅。どちらかを割る分は寄せずに元の space へ残す。60 だとサイドバーを出したまま（ペイン領域152桁）では2列で止まるので、3列が入る 50 にしてある」。超過時は `herdr notification show "$repo はこれ以上並べられません"` を出す（※要約経由の抜粋） — [Zenn: gemcook](https://zenn.dev/gemcook/articles/herdr-worktree-parallel)
- herdr 本体側で見つかるのは同梱 skill の努力目標だけ: "Split a wide pane to the right and a narrow or tall pane down. Avoid repeated same-direction splits that create unusably narrow columns or short rows." — [SKILL.md](https://github.com/herdrdev/herdr/blob/master/skills/herdr/SKILL.md)
- Coles は workspace = worktree 1 つ、その中を 3 pane（agent / Neovim / Yazi）に割る運用（※要約経由） — [coles.codes](https://coles.codes/posts/herding-agents-with-herdr/)

### Inferences
- **【necoder 文書との相違】** `fleet-ux-2026-09.md` §2 の表「1 repo = 1 画面 = worktree を横に最大 3 本（各 ≥50 桁）」「SessionStart hook で pane 自動作成」と、`FLEET-V2.md` §3.4「4 枚以上は並べない（herdr の 3 本上限）」は、herdr の仕様ではなく **Zenn 記事著者（muranakaaa 氏）の個人カスタマイズ**を herdr の規則として読んだもの。「上限 3」の根拠は「152 桁の端末で 50 桁 × 3 が読める下限」という 1 ユーザーの経験則であり、herdr 自体は pane 数を制限しない。necoder の判断（最小 420px・上限 3）は独立に妥当だが、出典の書き方は直した方がよい。
- necoder の語彙との対応: herdr の workspace（Space）≒ necoder の Task（= worktree = ProjectSession）、herdr の tab ≒ Task 内のレイアウト切替（necoder は廃止済み）、pane ≒ スレッドタブ / サイドペイン、session ≒ necoder には無い（プロセス = 1 つ）。herdr は「workspace = repo でも task でも investigation でもよい」と緩く、**worktree と Task を 1:1 に縛る不変条件は持たない**。
- herdr の `done` が「client ごとの既読」に依存する（§3）のは、複数 client（手元 + スマホ SSH）を前提にした設計の帰結。

### Gaps
- herdr 本体に pane 数・列数の上限があるかは、docs 0.9.1 と config reference を読んだ範囲では記述なし（「無い」と断定できるのは文書上のみ。ソースの全量は未確認）。
- Zenn 記事の hook スクリプト全文は要約経由でしか見ておらず、幅計算の細部は未照合。

## 3. エージェント状態の検出 — どんな状態があり、どう検出し、どのエージェントに対応し、どこで壊れるか

### Takeaway
状態は `blocked / working / done / idle / unknown` の 5 値だが、内部の実体は **Blocked / Working / Idle / Unknown の 4 値 + pane ごとの `seen` フラグ**で、`done` は「idle かつ未読」という**注意の状態**であってエージェントが申告する状態ではない。検出は「前景プロセスの特定 → pane ごとに権威を 1 つ選ぶ」の 2 段で、**Claude Code と Codex の状態は hook ではなく画面下部のスクレイピング（TOML の screen manifest）**が権威である（0.6.7・2026-06-03 に hook 方式から撤退）。このため精度は「エージェントの TUI 変更との追いかけっこ」に依存し、誤検出 issue が途切れない。

### Cited Findings
- 状態の定義表: `blocked` = "The agent needs input, approval, or a decision." / `working` = "actively running" / `done` = "The agent finished and you have not looked at it yet." / `idle` = "finished or waiting and has been seen" / `unknown` = "Herdr cannot confidently classify the state." — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- 既読は client ごと: "Each client tracks which completions it has displayed. Viewing a completion in one client does not clear another client's Done badge. CLI/API statuses use the server's seen state ... both `idle` and `done` mean ready for input." — [concepts.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/concepts.mdx)
- ソース上の優先度（rollup と priority ソートの共通関数）: `(Blocked, _) => 4`, `(Idle, false /*未読 = done*/) => 3`, `(Working, _) => 2`, `(Idle, true) => 1`, `(Unknown, _) => 0` — [src/workspace/aggregate.rs](https://github.com/herdrdev/herdr/blob/master/src/workspace/aggregate.rs)
- 権威は pane ごとに 1 つ: "Herdr first detects the foreground process in each pane. After that, each pane has one status authority." 完全な lifecycle hook を持つエージェントは統合が権威になり "It does not also run screen manifest fallback for that same lifecycle authority. This avoids two competing sources of truth." — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- hook を持たない / 不完全なエージェントは "reads the live bottom-buffer screen snapshot. It evaluates TOML manifests against that snapshot to classify `idle`, `working`, and `blocked`."（端末タイトルや進捗の OSC シーケンスも証拠に使える）。スナップショットはスクロール位置ではなくバッファ最下部から取る — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- 権威の対応表（0.9.1）: **lifecycle hook / plugin が権威** = Pi, OMP, Kimi Code CLI, OpenCode, Kilo Code CLI, MastraCode。**screen manifest が権威（統合は session 識別のみ）** = Claude Code, Codex, GitHub Copilot CLI, Devin CLI, Droid, Qoder CLI, Qwen Code, Letta Code, Cursor Agent CLI, Hermes Agent, Antigravity CLI, Grok CLI。**manifest のみ（統合なし）** = Amp, Kiro CLI, Maki, Muse。"Detected but less thoroughly tested: Gemini CLI and Cline." — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx) / [integrations.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/integrations.mdx)
- hook を権威にしない理由: "Integrations marked `session` ... are intentionally not lifecycle authorities. ... their hooks do not cover the whole lifecycle. They can miss permission approval results, escape interrupts, or other transitions." — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- Claude Code 統合の現状: "The hook reports Claude Code session identity to the local Herdr socket on session start. Claude Code state comes from Herdr's screen manifest detection." v10 は `SessionStart` の source（`startup` / `resume` / `clear` / `compact` / `fork`）だけに反応。`~/.claude/hooks/herdr-agent-state.sh` を書き `settings.json` に登録する — [integrations.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/integrations.mdx)
- **hook 方式だった頃の対応表**（issue #198 が引用する生成済み settings.json・2026-05）: `PreToolUse, PostToolUse, PostToolUseFailure, SubagentStop, UserPromptSubmit → working` / `PermissionRequest → blocked` / `SessionEnd → release` / `Stop → idle` — [#198](https://github.com/herdrdev/herdr/issues/198)
- その頃の故障: 完了したターンの後も `working` に貼り付く。作者の診断は "i'm suspecting claude code hooks having a race condition where `PostToolUse` arriving later than `end`"、加えて subagent / cron の子 Claude が working を報告する問題。統合は版を持ち、herdr 更新後に `herdr integration install claude` の再実行が必要（"do we have to do it again each time we update herdr?"） — [#198](https://github.com/herdrdev/herdr/issues/198)
- 撤退の経緯（CHANGELOG）: 0.3.0（04-03）"authoritative hook-driven state reporting" 導入 → 0.5.2 subagent hook が親を idle に見せる不具合 → 0.5.12 post-tool hook を working 扱いしない → 0.6.0 subagent 完了 hook が idle の pane を蘇らせない → 0.6.1 画面が「待ち」を示すなら古い hook の working を上書き → 0.6.6 古い permission 報告で blocked に貼り付く不具合 → **0.6.7（2026-06-03）"Claude Code, Codex, GitHub Copilot CLI, Droid, Kimi Code CLI, and Qoder CLI integrations now report session identity only. Native state for those agents comes from Herdr's screen detection"** → 0.6.9（06-10）manifest のリモート自動更新と `herdr agent explain` を追加 — [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- manifest の中身（`claude.toml`・version `2026.09.11.1`）: ルール = `id` / `state` / `priority` / `region`（`osc_title`・`bottom_non_empty_lines(12)`・`last_non_empty_above_prompt_box`・`after_last_horizontal_rule`・`prompt_box_body`・`whole_recent`）/ 照合子（`contains`・`regex`・`line_regex`・`any`・`all`・`not`）。例: `esc to interrupt` を含む行 → working、`do you want to proceed?` + 選択肢行 → blocked、`❯` の入力欄 → idle。コメントに "Braille covers <= 2.1.227; half-circles are the 2.1.228 busy spinner" とあり、**Claude Code の版ごとにスピナー字形を追っている** — [claude.toml](https://github.com/herdrdev/herdr/blob/master/distribution/agent-detection/claude.toml)
- 配信用ディレクトリ `distribution/agent-detection/` に 22 エージェント分の manifest（+ `index.toml`）があり、バイナリ同梱分に加えて herdr.dev から自動更新される（再起動不要・`[update] manifest_check = false` で無効化）。`~/.config/herdr/agent-detection/<agent>.toml` のローカル上書きが最優先。新エージェントの追加はバイナリ更新が必要 — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx) / [agent-detection/](https://github.com/herdrdev/herdr/tree/master/distribution/agent-detection)
- blocked は意図的に厳しめ: "Herdr only marks `blocked` when the live bottom-buffer snapshot matches known visible approval, question, or permission UI. If no manifest rule matches for a known agent, Herdr falls back to `idle`"（`default_known_agent_idle_fallback`）。"unusual new agent prompts may initially show as `idle` instead of `blocked`"。誤分類は表示と wait にしか影響しない、と明記 — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- デバッグ手段: `herdr agent explain <target>` が manifest の出所 / 版 / 一致ルール / 領域ごとの証拠 / fallback 理由を出す — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- プロセス検出の穴: wrapper / VM / sandbox が実プロセスを隠すと検出できず `HERDR_AGENT=claude fence -- claude` のようなヒントが要る。pane 内で tmux を起動すると herdr には `tmux` しか見えない — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- 未解決・最近の誤検出（2026-08〜09）: 入力欄のゴースト提案文で idle ルールが外れ fallback idle になる（#2959・open）/ background shell が残ると prompt 待ちでも working（#3414・作者 "our original PR was wrong IMO"）/ Codex 0.154.0 で稼働中が idle（#4092）/ Antigravity の permission が idle 扱い（#3871・open）/ nix で包んだ Claude Code が agent 一覧に出ない（#803・open・👍7）/ OpenCode が blocked に貼り付く（#4214・open）/ Kimi の quota エラーが working のまま（hook 権威だと画面検出を飛ばすため・#3997・open） — [#2959](https://github.com/herdrdev/herdr/issues/2959) / [#3414](https://github.com/herdrdev/herdr/issues/3414) / [#4092](https://github.com/herdrdev/herdr/issues/4092) / [#3871](https://github.com/herdrdev/herdr/issues/3871) / [#803](https://github.com/herdrdev/herdr/issues/803) / [#4214](https://github.com/herdrdev/herdr/issues/4214) / [#3997](https://github.com/herdrdev/herdr/issues/3997)
- 第三者の評価: Claude Code では "can take a few seconds to recognise a blocked prompt"（2026-08-02・※要約経由）/ "Detection is probabilistic. Screen manifests break when agent interfaces change"（2026-08-23・※要約経由） — [coles.codes: herdr vs cmux](https://coles.codes/posts/herdr-vs-cmux/) / [developersdigest](https://www.developersdigest.tech/blog/herdr-deep-dive-agent-terminal-multiplexer)
- 自作エージェントは CLI で状態を申告できる: `"$HERDR_BIN_PATH" pane report-agent "$HERDR_PANE_ID" --source custom:my-agent --agent my-agent --state working`（`idle` / `blocked`・`--message`・順序保証の `--seq`・終了時は `pane release-agent`）。`HERDR_ENV=1` の時だけ動かす規約 — [integrations.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/integrations.mdx)

### Inferences
- 質問の「SessionStart / Notification / Stop hook で検出しているのか」への答え: **2026-09 時点では No**。Claude Code の hook は `SessionStart` だけを使い、用途は「再起動後に `claude --resume <id>` するための session ID 取得」。`Stop → idle` 等の写像は 2026-04〜05 に試して捨てた方式。`Notification` hook を使った形跡は読んだ範囲に無い。
- herdr が hook を捨てた理由（順序が保証されない・subagent が親の状態を汚す・承認結果や Esc 中断のイベントが無い・統合の版ずれ）は、**hook ベースで状態を組もうとする全ツールに当てはまる教訓**。necoder は ACP の構造化イベント（permission request / tool call / stop reason）を権威にできるので、この軍拡競争に参加しなくて済む。これは herdr に対する構造的な優位で、`blocked` を「数秒遅れ・厳しめ・外れたら idle」にせざるを得ない herdr と違い、承認待ちを即時かつ確実に出せる。
- 逆に herdr の「権威は 1 つ」「分からなければ `unknown` / fallback 理由を explain で見せる」は、necoder が CLI 形態のエージェント（ACP 非対応）を Fleet に混ぜる時にそのまま使える規律。
- `done` を状態ではなく「idle × 未読」の導出にしている設計は、necoder の `ThreadActivity::Done`（確認で Idle へ）と同型。違いは herdr が **見ただけで自動既読**にする点（→ §4・§7 の不満の源）。

### Gaps
- 検出の遅延・誤検出率の定量データは見つからなかった（作者・第三者とも数値を出していない）。
- manifest のリモート更新頻度の実績（何日おきか）は未集計。`claude.toml` の版番号が日付形式（2026.09.11.1）であることからの推測に留まる。
- Gemini CLI / Cline の検出品質は "less thoroughly tested" 以上の情報なし。

## 4. 注意の管理 — 人間に「このエージェントが呼んでいる」をどう伝えるか

### Takeaway
中心はサイドバーの **rollup**（agent → pane → tab → workspace に blocked を伝播）で、補助が toast / OS 通知 / done 用と request 用の 2 種の音。**「次の blocked へ跳ぶ」専用キーは既定では存在しない**: あるのは `prefix+o`（表示中の通知の対象へ）だけで、`next_agent` / `previous_agent` / `focus_agent 1..9` は既定で未割当、並びも既定は workspace 順（`priority` は opt-in）。通知 popup も既定 off。

### Cited Findings
- Rollup: "A blocked agent makes its pane, tab, and workspace look blocked. A working agent makes the workspace look active. A done agent stays visible until you view it." / "This is the main Herdr workflow: start several agents, let them work in parallel, and use the sidebar to see which project needs a decision, which one is still running, and which one is ready to review." — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- サイドバーは 2 パネル。既定の行レイアウトは Agents = `["state_icon","machine","workspace","tab"]` + `["agent"]`、Spaces = `["state_icon","workspace"]` + `["branch","git_status"]`。使える組込みトークンは `state_icon` / `state_text` / `machine` / `workspace` / `tab` / `pane` / `agent` / `terminal_title` / `terminal_title_stripped` / `$name`（カスタム）。**トークン使用量・コスト・「頼んだこと」に当たる組込みトークンは無い** — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)
- 状態の見た目: 既定は "compact colored dots"。形でも区別したい時は `[ui] status_indicators = "symbols"`。"The symbols are static, so this option does not enable spinner animation." — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)
- ユーザーの記述では未読 done は青・既読は緑: "blue when unseen, green once I've looked at it" — [discussion #864](https://github.com/herdrdev/herdr/discussions/864)
- 並び順: `ui.agent_panel_sort` = `"spaces"`（既定）| `"priority"`。0.7.1（2026-06-24）で追加。同時に Agents パネルの workspace / all フィルタを廃止し常に全 agent を表示 — [config-reference.json](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/data/config-reference.json) / [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- priority ソートの動機（issue #318・👍9）: "A blocked agent at the bottom of the list stays at the bottom until scanned for." / "Sorting by attention priority makes the panel function as a queue of work instead of a static workspace map." 追随コメント: "with 10+ agents across workspaces, the blocked ones are the highest-signal rows and they're buried in workspace order." バケット内を「最後に状態が変わった時刻の新しい順」で並べる案も出ている — [#318](https://github.com/herdrdev/herdr/issues/318)
- キー（既定）: `open_notification_target = "prefix+o"`（"Focus the currently visible notification target."）/ `goto = "prefix+g"`（session navigator）/ `prefix+w`（workspace navigation）/ `toggle_sidebar = "prefix+b"` / `new_worktree = "prefix+shift+g"`。**`next_agent`・`previous_agent`・`focus_agent`（1-9）・`next_workspace`・`previous_workspace` は "Unset by default."** docs の記述例は `focus_agent = "prefix+alt+1..9"` — [config-reference.json](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/data/config-reference.json) / [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)
- 基本 5 キー: `prefix+c` 新 tab / `prefix+v`・`prefix+minus` split / `prefix+h/j/k/l` pane 移動 / `prefix+w` workspace / `prefix+q` detach。prefix 既定 `ctrl+b`。`prefix+?` で全割当。マウスは一級市民（"Keyboard control is optional."） — [keyboard.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/keyboard.mdx)
- 日本語ユーザーの実例: `next_agent = "prefix+a"` / `previous_agent = "prefix+shift+a"` を自分で割り当てて「返事待ちで止まっているペイン」へ跳ぶ（2026-08-04・herdr 0.7.5・※要約経由） — [てちこまログ](https://techikoma.com/blog/herdr-terminal-workspace-claude-code/)
- 通知 popup: `ui.toast.delivery` の既定は **`"off"`**。`herdr`（アプリ内 toast）/ `terminal`（外側の端末に依頼・SSH 越しでも届く）/ `system`（OS 通知）。`delay_seconds` 既定 1 で "Herdr notifies only if the pane is still in the same state when the delay expires."。**active tab の分は抑止** — [config-reference.json](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/data/config-reference.json) / [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)
- 音: `ui.sound.enabled` 既定 true（"Play sounds when agents change state in background workspaces."）。`done_path`（終了）と `request_path`（入力要求）を別の mp3 にできる。agent 別に `default|on|off`（Droid は既定 off）。`HERDR_DISABLE_SOUND` で無効化 — [config-reference.json](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/data/config-reference.json) / [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx)
- 任意の通知を出す CLI: `herdr notification show <title> [--body TEXT] [--position ...] [--sound none|done|request]`（title 80 字・body 240 字に切る） — [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx) / [socket-api.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/socket-api.mdx)
- 行に「いま何をしているか」を載せるのは自前: `herdr pane report-metadata <pane> --source ... --token summary="reviewing authentication"` で `$summary` を出す / Claude Code だけ `terminal_title_stripped`（端末タイトルからスピナー字形を除いたもの）を行に足す `rows_by_agent` 例が docs にある。"Tokens and state labels are visual-only. Waits, notifications, and workspace rollups still use the semantic state." — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx) / [integrations.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/integrations.mdx)
- スマホ: 専用アプリ無しで SSH クライアントから同じ session を開く。"The TUI adapts to narrow screens"。0.7.2 で "The mobile switcher now starts from an agents-first summary and renders worktrees as a tree" — [how-to-work.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/how-to-work.mdx) / [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- 最大の不満の 1 つ = **既読が軽すぎる**（discussion #864・▲26・2026-06-29〜09-18 まで +1 が続く）: "The pane finished while I was away, herdr had focus, so the completion was consumed and it is green by the time I look at it. I never read a word of it. ... the finished work quietly falls off my queue" / "I open an agent's tab by accident while scanning the list, it flips to seen immediately, and there's no way back"。提案は「Mark unread（自動既読に負けない sticky）」「手動専用の `followup` 状態」「**focus して約 1 秒留まった時だけ既読にする dwell**」。2026-09-20 時点で未実装 — [discussion #864](https://github.com/herdrdev/herdr/discussions/864)
- 通知まわりの不具合例: 消えない通知（#1827・👍5）/ OS 通知クリックで該当 pane へ跳びたい（discussion #625・▲18）/ toast・音・枠ラベルを切ったら CPU 負荷が収まったという報告（#3217） — [#1827](https://github.com/herdrdev/herdr/issues/1827) / [#3217](https://github.com/herdrdev/herdr/issues/3217) / [discussion #625](https://github.com/herdrdev/herdr/discussions/625)

### Inferences
- herdr の注意設計は「一覧を見れば分かる」止まりで、**能動的な呼び出し（popup）も最短の跳び先キーも既定では off / 未割当**。tmux 互換の「プログラムのキーを奪わない」方針の副作用。GUI の necoder は ⌘⇧U（次の要対応へ）・titlebar のバッジ・要対応キューを既定で持てるので、ここは herdr より踏み込んでよい領域。
- 「1 秒遅延 + その時点でも同じ状態なら通知」「見ている tab の分は鳴らさない」「done と request で音を分ける」「うるさいエージェントは個別に mute」は、誤検出の多い環境で磨かれた実戦的な細部で、そのまま借りる価値がある（necoder は ACP で誤検出が少ない分、遅延は短くてよい）。
- #864 は necoder への直接の教訓: **Done は「舞台に出した」だけでは消さず、明示の「確認」か dwell で消す**。FLEET-V2 §3.2 の「完了・未確認 → 確認（Done → Idle）」は herdr ユーザーが欲しがっているものそのもの。加えて「後で見る（再フラグ）」が欲しいという声が複数あるので、確認の逆操作を用意する余地がある。
- herdr の既定の行には「頼んだこと / いま何を」が無く、ユーザーが hook + metadata で後付けしている。necoder の Task 行 3 段（`› 頼んだこと` + digest）は herdr の既定を明確に超えている。

### Gaps
- 状態ごとの既定の色・記号の正確な対応（blocked = 何色 / symbols モードの字形）は docs に表が無く、ソースの該当箇所も特定できなかった。上の「青 / 緑」は 1 ユーザーの記述。
- 「次の blocked へ」相当を priority ソート + `focus_agent 1` で代用する運用は discussion #864 の 1 コメント（"a priority-sorted panel and a direct shortcut to its first row"）から読み取ったもので、公式の推奨としては書かれていない。

## 5. 自動化の表面 — CLI / socket API / hook / 設定。エージェントは herdr を操作できるか。coordinator / task queue / 依存は在るか

### Takeaway
CLI と socket API は同じ表面で、**エージェント自身が pane を割り・別エージェントを起動し・prompt を送り・状態を wait できる**（`agent start / prompt / wait / read / send-keys`）。しかし herdr が提供するのは**プリミティブだけ**で、orchestrator・supervisor・task・queue・依存・承認ポリシー・送信者の帰属は**存在しない**（API schema 277KB に `task` / `queue` / `depend` / `permission` / `origin` / `audit` が 0 回）。それらを求める issue は 2026-09 だけでも複数あり、すべて "not planned → Ideas へ" で閉じられている。**調整役は人間だけ**、が現状の設計。

### Cited Findings
- 設計の 3 プリミティブ: Layout（workspace / tab / pane の配置）/ Pane（生の端末: run・send・read・wait-output）/ Agent（認識済みエージェントを名前か pane で操作）。"A script can control them, or one agent can create work for other agents, inspect their state, and collect their results." — [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- 典型レシピ: `herdr pane split --current --direction right --no-focus` → `herdr agent start reviewer --kind codex --pane "$review_pane" -- -m gpt-5.4` → `herdr agent prompt reviewer "Review the current diff" --wait --timeout 120000` → `herdr agent read reviewer --source recent-unwrapped --lines 120` — [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- `agent start` は 24 種の `--kind`（pi, claude, codex, gemini, cursor, devin, agy, cline, omp, mastracode, opencode, copilot, kimi, kiro, droid, amp, grok, hermes, kilo, qodercli, qwen, letta, maki, muse）。既存の shell pane が必要で "never creates, splits, or moves layout"。検出されて入力可能になるまで戻らない（既定 30 秒）。起動中に blocked なら即 `agent_not_ready` — [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- `agent prompt` の意味論: 稼働中の相手にも送れる。**相手が `blocked` なら入力を送らず `agent_blocked` を返す**。`--wait` は送信後 5 秒以内に `working` / `blocked` を観測できなければ `agent_prompt_stalled`。"It does not track individual turns."。"A timeout or `agent_prompt_stalled` does not prove that no input was sent. Read the agent before retrying to avoid submitting the same prompt twice." — [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- `agent wait` は "server-owned and event-driven" で pane の占有者を固定する。対象は 1 体だけ。wait に既定タイムアウトは無い — [socket-api.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/socket-api.mdx) / [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- 長い応答の回収は苦しい: Claude Code / OpenCode は alternate screen に描くので、idle の時だけマウススクロールを模して履歴を集める。取れなければ "ask the agent to write it as Markdown in a temporary directory and reply only with the file path" — [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- 輸送: "newline-delimited JSON over a local socket"（Unix socket / Windows は named pipe）。`events.subscribe` で長寿命ストリーム、`session.snapshot` で初期状態（購読 → snapshot → バッファ適用の順を指定）。イベントは workspace.* / tab.* / pane.*（`pane.agent_detected`・`pane.agent_status_changed` を含む）/ layout.updated / worktree.created・opened・removed — [socket-api.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/socket-api.mdx) / [herdr-api.schema.json](https://github.com/herdrdev/herdr/blob/master/docs/next/api/herdr-api.schema.json)
- **API schema（277,222 bytes・2026-09-20 取得）を grep した結果**: `audit` 0 / `acl` 0 / `permission` 0 / `authorize` 0 / `origin` 0 / `sender` 0 / `queue` 0 / `depend` 0 / `task` 0 / `orchestr` 0。agent 系メソッドは `agent.explain / focus / get / list / prompt / read / rename / send_keys / start / view.set / view.clear / wait` の 12 個 — [herdr-api.schema.json](https://github.com/herdrdev/herdr/blob/master/docs/next/api/herdr-api.schema.json)
- pane 内プロセスに注入される環境変数: `HERDR_ENV=1` / `HERDR_PANE_ID` / `HERDR_TAB_ID` / `HERDR_WORKSPACE_ID` / `HERDR_BIN_PATH` / `HERDR_SOCKET_PATH` — [integrations.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/integrations.mdx) / [SKILL.md](https://github.com/herdrdev/herdr/blob/master/skills/herdr/SKILL.md)
- 同梱 skill（`npx skills add herdrdev/herdr --skill herdr -g`）は**発動条件を絞っている**: "Use only when the user explicitly mentions Herdr ... Do not use merely because a task could benefit from a background terminal, delegation, or parallel work. Requires HERDR_ENV=1." / "Do not create a workspace, tab, worktree, or different cwd unless the user explicitly requests that topology or location." — [SKILL.md](https://github.com/herdrdev/herdr/blob/master/skills/herdr/SKILL.md) / [Zenn](https://zenn.dev/gemcook/articles/herdr-worktree-parallel)
- skill の安全規則: `--no-focus`（人間の focus を奪わない）/ ID は JSON 応答から読む（サイドバーの順から推測しない）/ 自分が作っていない workspace・tab・pane・session を閉じない / `--trust-repository` を再試行の手段にしない / `herdr server stop` を勝手に打たない / **blocked の相手には "Inspect the blocked UI and ask the user before answering it."** — [SKILL.md](https://github.com/herdrdev/herdr/blob/master/skills/herdr/SKILL.md)
- 設定: `~/.config/herdr/config.toml`（Windows は `%APPDATA%\herdr\config.toml`）・TOML・無くても動く・`herdr --default-config` で全量・`herdr server reload-config` で再読込。見た目は client 側、worktree / 統合 / カスタムコマンドは server 側の設定 — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)
- 無人運用: client が居ない時は 120×40 の仮想端末（`[server] headless_cols / headless_rows` で変更）。0.9.1 で `herdr --machine <label>` により保存済み SSH マシンへ CLI を転送（"failed remote commands never fall back to Local"） — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx) / [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- plugin v1（0.7.0・2026-06-15）: manifest（`herdr-plugin.toml`）に actions / `[[events]]`（例 `on = "worktree.created"`）/ panes / link handlers / startup hook。`herdr plugin install <owner>/<repo>[/subdir]`。**"it does not review or sandbox plugin code"**。GitHub の `herdr` topic が marketplace（審査なし）。作者によれば公開 1 か月で plugin 500+（2026-08-06） — [plugins.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/plugins.mdx) / [YC 参加の告知](https://herdr.dev/blog/herdr-is-joining-y-combinator/)
- **スコープ外の明言（作者本人）**: Kanban / task 管理の要望（#301・2026-05-26）に "thanks, it looks great but i dont want to bring big specialized features like this to main herdr, but i'm working on a plugin system that enable you too hookup something like this!" → closed as not planned — [#301](https://github.com/herdrdev/herdr/issues/301)
- 作者の方針: "choosing what goes in the core is the most important decision" / "Everything else stays possible through extensions: your style, your flows, your company's setup"（※要約経由） — [YC 参加の告知](https://herdr.dev/blog/herdr-is-joining-y-combinator/)
- **orchestrator を作る側の要望は、issue として出された 4 件（下記）がすべて `not_planned` で閉じられている**（トリアージ bot `akbash-bot` が「新しいインターフェース = 機能要望 → Ideas discussions へ」と判定。作者本人の却下ではない点に注意） — [#4027](https://github.com/herdrdev/herdr/issues/4027) ほか下記:
  - 編隊レベルの状態遷移イベント（#4027・2026-09-13）: "up to 7 concurrent agents ... three separate incidents where a `blocked` agent (permission prompt, model dialog) sat undiscovered for long stretches because the polling loop had ended, its interval was too coarse, or the poller itself was lost with its shell." 回答は「`events.subscribe` に pane_id を列挙すればよい」 — [#4027](https://github.com/herdrdev/herdr/issues/4027)
  - 複数対象の `agent wait --any`（#4216・2026-09-16）: "Worst case with N agents: (N−1) × timeout added latency" — [#4216](https://github.com/herdrdev/herdr/issues/4216)
  - idle の時だけ送る / busy なら拒否（#4052・2026-09-13）: 回答 "`--wait` waits after submission; it does not reserve the conversation or isolate tasks." — [#4052](https://github.com/herdrdev/herdr/issues/4052)
  - herdr の外に居る coordinator からの操作を skill で許可（#3568・2026-09-03）: "The current skill intentionally authorizes control only from inside a Herdr-managed pane." — [#3568](https://github.com/herdrdev/herdr/issues/3568)
  - orchestrator の prompt で done バッジや音を出さない `agent prompt --quiet`（discussion #4336・2026-09-18・コメント 0 = 未対応） — [discussion #4336](https://github.com/herdrdev/herdr/discussions/4336)
- **送信者の帰属が無い**（discussion #3584・2026-09-03）: 別エージェントが `herdr agent prompt <driver> <text>` で送った文を、受け手の Claude Code は transcript に `"origin": {"kind": "human"}` と記録する。"the receiver asserts a human origin that is false"。`AgentPromptParams` は `{target, text, wait}` だけで、`agent.prompt*` の購読イベントも無い — [discussion #3584](https://github.com/herdrdev/herdr/discussions/3584)
- イベント取りこぼし: `events.subscribe` は約 500 件を超える滞留で黙って落とし、欠落の通知も無い（#4178・open・2026-09-15） — [#4178](https://github.com/herdrdev/herdr/issues/4178)
- コミュニティ製の上物: **Herdr Supervisor**（hao1939・Pi 拡張 + コンテナ）。動機は "terminal state tells me whether a worker is busy or idle, but it does not tell me whether the outcome I wanted is actually complete."。設計は「1 worker = 1 つの durable outcome」「意味のあるイベントで起こし、保険として bounded review」「Herdr の状態は複製しない」。"It is not a native Herdr plugin, a Kanban board, a scheduler, or a replacement runtime." — [discussion #3607](https://github.com/herdrdev/herdr/discussions/3607) / [#301 のコメント](https://github.com/herdrdev/herdr/issues/301)
- 第三者の総括: "It is not an agent platform. No task graph, no approval policy engine"（※要約経由） — [developersdigest](https://www.developersdigest.tech/blog/herdr-deep-dive-agent-terminal-multiplexer)

### Inferences
- 質問への直答: **orchestrator / supervisor agent は herdr に無い。人間が唯一の coordinator**。ただし「エージェントが coordinator を演じるための手足（start / prompt / wait / read）」は公式に提供され、README も "prompt each other" を売りにしている。つまり herdr は「Captain を作れる土台」であって「Captain」ではない。
- not planned の判定は bot によるもので「永久にやらない」とは限らないが、作者の「core を痩せたまま保つ」発言・Kanban 却下・商用軸（マシン接続）の 3 点から、**task / 依存 / 承認 gate が core に入る見込みは低い**と読むのが妥当。
- herdr 上で orchestration を組んだ人が踏んでいる穴は、necoder の Captain 設計が先回りで塞いでいる所と一致する: ①ポーリングが切れて blocked を見逃す（→ FLEET-V2 §5.3 イベント駆動・ポーリング禁止）②対象 1 体ずつしか待てない（→ 台帳イベントを 1 ターンにまとめて渡す）③誰が送ったか分からない（→ `human_send` / `captain` を台帳に残す）④busy な会話に無関係な依頼が混ざる（→ 1 Task = 1 worktree = 専用スレッド）⑤「状態」と「目的の達成」は別（→ TaskPhase と ThreadActivity を別 enum に保つ）。
- herdr には**技術的な承認 gate が無い**: CLI を持つエージェントは `agent send-keys` で他エージェントの許可ダイアログに答えられ、止めているのは skill の文章（"ask the user"）だけ。necoder の権限表（Captain は承認応答 ×・Integrate ×）は herdr に無い強い保証。
- 逆に herdr から借りるべき細部: `agent_blocked` での送信拒否、`agent_prompt_stalled`（5 秒で活動が見えなければ失敗扱い）、「timeout は未送信の証明ではない → 再送前に読め」、`--no-focus`、ID は応答から読む。`fleet send` の意味論にそのまま使える。

### Gaps
- herdr-supervisor のリポジトリ本体（design.md / research.md）は未読。利用者数も不明。
- discussion #3584 への作者の返答は確認できていない（▲1・コメント未取得）。
- 「Herdr Cloud」に orchestration が含まれるかは公開情報なし。

## 6. worktree ワークフロー — Task ごとの作成、未追跡ファイル（.env）の罠、準備スクリプト、片付け、main へ戻す

### Takeaway
worktree は 0.6.1〜0.6.2（2026-05-22/23）から**本体機能**で、サイドバーの `New worktree`（`prefix+shift+g`）か `herdr worktree create` で checkout を作り、親 workspace の下にグループ化して開く。片付けは安全寄り（まず通常の `git worktree remove`、拒否されたら確認して強制・**ブランチは消さない**）。一方で **`.env` 等の未追跡ファイルの持ち込み・依存の準備・main への統合・diff レビュー・衝突検知は core に無く**、plugin の `worktree.created` イベントかユーザーの自作に任されている。

### Cited Findings
- 導入: 0.6.1 "Added explicit sidebar Git worktree groups plus native worktree creation, existing checkout open, and safe checkout cleanup flows"（要望 #137・2026-05-17）→ 0.6.2 "Added CLI and socket API support for Git worktrees with `herdr worktree list/create/open/remove`" — [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md) / [#137](https://github.com/herdrdev/herdr/issues/137)
- CLI: `herdr worktree create [--workspace ID | --cwd PATH] [--branch NAME] [--base REF] [--path PATH] [--label TEXT] [--focus|--no-focus] [--trust-repository]`。既存ローカルブランチなら checkout、無ければ `--base` か `HEAD` から作成 — [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx)
- 置き場: `[worktrees] directory = "~/.herdr/worktrees"`（既定）→ `<directory>/<repo>/<branch-slug>`。兄弟フォルダ配置のためのプレースホルダ要望が issue 中 2 番目に 👍 が多い（#261・👍24） — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx) / [#261](https://github.com/herdrdev/herdr/issues/261)
- 削除: "Herdr runs `git worktree remove`, first asking Git to remove safely. If Git refuses because the checkout has modified or untracked files, Herdr asks again before running the forced remove. Branches are not deleted." `workspace close` は herdr の状態だけを閉じ、親を閉じるには `--group` が要る（0.9.0 から） — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx) / [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx)
- 準備の差し込み口は plugin のイベント: manifest 例に `[[events]] on = "worktree.created"`。CLI reference のインストール例は `ogulcancelik/herdr-plugin-examples/worktree-bootstrap`。`worktree create/open` に `--env` を足す要望（#1518）は閉じられている — [plugins.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/plugins.mdx) / [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx) / [#1518](https://github.com/herdrdev/herdr/issues/1518)
- Zenn 記事の運用（2026-09-08・herdr 0.8.0・※要約経由）: Claude Code に「herdrで feature/user-auth のworktree作ってClaude Code立ち上げて」と頼む → `herdr worktree create` → Claude Code の `SessionStart` hook（`~/.claude/hooks/herdr-repo-workspace.sh`）が新 pane を**同じ repo の space へ `herdr pane move ... --split right` で寄せ**、幅を `herdr pane resize` で均等化し、ブランチ名で pane と agent を改名する。普段は「2つか3つのペインを開いて、1つのペインの差分をレビューしている間に、別のペインのClaude Codeが実装を進めている」 — [Zenn: gemcook](https://zenn.dev/gemcook/articles/herdr-worktree-parallel)
- 同記事のコメント（まめ研究員）: 「worktree には commit 済みの内容しか入らないこと。…未追跡ファイル（git status で ?? のもの）が worktree 側に存在せず」/「Windows 固有ですが、パスの長いファイルを含む履歴だと git worktree add が Filename too long で止まります。core.longpaths=true で解消しました」（※要約経由） — [Zenn: gemcook](https://zenn.dev/gemcook/articles/herdr-worktree-parallel)
- Coles の運用（ページ日付 2026-06-10・本文は 0.7.3 = §1 の不整合の注を参照）: "worktree-per-task flow"。Claude Code 側の `claude -w feat/billing` で worktree を切り、workspace = worktree、レビューは plugin `persiyanov/herdr-reviewr`（`cmd+r`）（※要約経由） — [coles.codes](https://coles.codes/posts/herding-agents-with-herdr/)
- 既知の穴: submodule を含む worktree が消せなかった（#1797・👍8 → 0.9.1 で明示の強制削除）/ bare repo 対応は 0.6.9 / 途中で失敗した削除の再試行は 0.9.1 / jj（Jujutsu）対応の要望が多い（discussion #480 ▲29・#2209 ▲22） — [#1797](https://github.com/herdrdev/herdr/issues/1797) / [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md) / [discussion #480](https://github.com/herdrdev/herdr/discussions/480) / [discussion #2209](https://github.com/herdrdev/herdr/discussions/2209)
- 隔離は無い: "No isolation. Two panes pointed at the same checkout can edit the same files."（※要約経由）/ Coles "Does not sandbox agents; isolation is a separate job"（※要約経由） — [developersdigest](https://www.developersdigest.tech/blog/herdr-deep-dive-agent-terminal-multiplexer) / [coles.codes](https://coles.codes/posts/herding-agents-with-herdr/)
- Spaces 行の既定 2 段目は `branch` + `git_status`（ahead / behind が非 0 の時だけ） — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx)

### Inferences
- herdr の worktree 機能は「checkout を作って workspace として開く / 安全に消す」まで。**作成直後の準備（.env・依存・ビルドキャッシュ）と、終わった作業を戻す工程（レビュー → 統合 → 片付け）は製品の外**。necoder の `.necoder/worktree-setup.sh` + `task.env`（FLEET-V2 §6.2）と Conflict Radar → Integrate は、herdr が空けている場所をそのまま埋めている。
- 「ブランチは決して消さない」「Git に安全な削除を先に試させ、拒否されたら理由を見せて再確認」「閉じる ≠ 消す」は necoder の片付けメニュー（失うものを数える確認）と同じ思想で、維持すべき。
- worktree の置き場は herdr 既定が `~/.herdr/worktrees/<repo>/<slug>`（ホーム配下に集約）、necoder は `<repo の親>/<repo 名>-worktrees/<slug>`（兄弟配置）。herdr で兄弟配置の要望が 👍24 を集めていることは、necoder の既定が需要に沿っている傍証。
- Zenn 記事のように「Claude Code の hook から herdr CLI を叩いてレイアウトを自動整形する」層をユーザーが自作している = herdr は Task とレイアウトの対応を持たない。necoder は Task = 舞台のカードなので、この糊が要らない。

### Gaps
- `herdr-plugin-examples` は `ogulcancelik/` でも `herdrdev/` でも API が 404 を返し（2026-09-20）、`worktree-bootstrap` の中身（.env をコピーするのか等）は確認できなかった。docs の例示リストは `agent-telegram-notify` / `github-link-preview` / `dev-layout-bootstrap`。
- herdr に merge / rebase / PR 作成 / diff 表示の組込み機能があるかは、docs 0.9.1 を読んだ範囲では**記述なし**（reviewr は第三者 plugin）。
- 未追跡ファイル問題に対する herdr 公式の見解・FAQ は見つからなかった。

## 7. 実ユーザーの声 — 称賛・不満・現実的な同時並走数・認知負荷

### Takeaway
称賛は「どれが自分を待っているかが一目で分かる」「SSH / スマホから同じ session に戻れる」「Electron でない 1 バイナリ」「マウスとスクロールが tmux より素直」「API で道具を作れる」に集中。不満は「状態の誤検出」「性能（多数同時稼働時の CPU）」「Windows の入力まわり」「pre-1.0 の破壊的変更」「既読が軽すぎて完了を見失う」。**日常的な同時並走は 2〜4 体が中心**で、5〜8 体・10 体超は「一覧がキューとして機能しないと破綻する」という文脈で語られる。

### Cited Findings
- Coles（ページ日付 2026-06-10・本文は 0.7.3）: 問題の定義 "When you run three or four agents in parallel on a remote box, the work that slows you down is figuring out which one needs you right now." / 価値 "knowing natively that pane 2 is a blocked agent session waiting on me" / 3〜4 体を並走 / リモート Ubuntu に SSH / tmux と同じ prefix で筋肉記憶が使える / No Electron・ホスト型の control plane 無し（※要約経由） — [coles.codes](https://coles.codes/posts/herding-agents-with-herdr/)
- Zenn（2026-09-08）: 2〜3 pane。1 つの差分をレビューしている間に他が実装を進める、という使い方（※要約経由） — [Zenn: gemcook](https://zenn.dev/gemcook/articles/herdr-worktree-parallel)
- HN スレ（166 points・110 comments・2026-06 末。以下すべて※要約経由）: 称賛 = "Their API is awesome too — you can build tools around it."（nullbio）/ "native-feeling scroll ... a killer feature over tmux"（cush）/ SSH + Tailscale でスマホから（xyzsparetimexyz）。並走数 = "5-8 at a time"（shepherdjerred）/ 4 セッション（zihotki）。懐疑 = "do ppl really run that many agents in parallel that they cannot comfortably multiplex them using just a terminal emulator?"（rw_panic0_0）/ "Dang, that's not too much context switching for your brain?"（butlike）/ "Will people really need a bunch of different agents?"（mintflow）。可視性の不足 = "difficulty to see what each of them is doing, what program did they call, and where they are getting stuck"（kerlenton）。並走の正当化 = "wait for 5 minutes and more for an answer, I can switch to another project"（WinstonSmith84）。"Isn't there race conditions?" → "Git worktrees."。Android の Termius でタップが効かない（bangau1） — [Hacker News](https://news.ycombinator.com/item?id=48714802)
- 多数並走の実例: "with 10+ agents across workspaces, the blocked ones ... buried in workspace order"（ShaneMain）/ "~5–15 panes per session" で priority ソートを 2.5 週間自前運用（akopanev） — [#318](https://github.com/herdrdev/herdr/issues/318)
- 認知負荷（task 管理の要望）: "Keeping track of the details of each agent's action and what remains to be done is difficult. Limiting how many agents one can concurrently manage because one can quickly lose context." — [#301](https://github.com/herdrdev/herdr/issues/301)
- 認知負荷（既読問題）: "Often, I like to peek at the results of an ended turn to see if it's a quick reply or something I need to think about. If I'm already in the middle of another cognitive task, I'd prefer to come back to it later."（2026-09-16） — [discussion #864](https://github.com/herdrdev/herdr/discussions/864)
- 性能: 0.8.2 で複数エージェント同時稼働時に macOS のファンが回る（#3217・open）。4 体の pi で `herdr server` が **275.4% CPU** という報告、toast・音・枠ラベルを切って収まったという報告、Linux 192 コア機で **30 pane（うち 26 が coding agent）** を動かす利用者の計測。ほかに 60Hz 上限（#1134・👍32 = issue 中最多）/ pane を出すたびに 256 色を再問い合わせして数秒固まる（#3266・open）/ 0.9.0 で remote attach が重い（#3769） — [#3217](https://github.com/herdrdev/herdr/issues/3217) / [#1134](https://github.com/herdrdev/herdr/issues/1134) / [#3266](https://github.com/herdrdev/herdr/issues/3266) / [#3769](https://github.com/herdrdev/herdr/issues/3769)
- コメント数の多い issue 上位 30 件のうち Windows 起因が約 10 件（貼り付けが改行で送信される・Esc が飲まれる・mouse report が残る・curl の窓が点滅 等・2026-09-20 取得）。Windows は 0.8.2（2026-08-19）で GA — [issues（コメント順）](https://github.com/herdrdev/herdr/issues?q=is%3Aissue+sort%3Acomments-desc) / [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)
- 最も支持された要望は「複数のリモート herdr サーバ」（discussion #515・▲104）で、0.9.0 で実装。作者は外部 PoC を断り "this is a big change i want to tackle myself for now" — [discussion #515](https://github.com/herdrdev/herdr/discussions/515)
- 制限の整理（bitdoze・※要約経由）: pre-1.0 で破壊的変更あり / solo developer / sandbox なし / Windows は beta（執筆時）。plugin は 83+ repo、herdr-file-viewer 64★・herdr-reviewr 44★ — [bitdoze](https://www.bitdoze.com/herdr-agent-multiplexer/)
- 日本語圏: note / 企業 tech blog に 2026-07〜09 の導入記が複数。VISASQ（2026-07-30・0.7.5）は「エージェント同士が会話できる」を最大の利点に挙げ、`done` を「終わったのに未読」と説明。note の入門記（2026-07-09・0.7.3）は「閉じ方（デタッチ：Control+B →q）に最初つまずいた」「複数エージェント同時運用での実用性はまだ未検証」（※要約経由） — [VISASQ Dev Blog](https://tech.visasq.com/herdr-multi-agent) / [note: kazu_t](https://note.com/kazu_t/n/nb4958baf3290) / [てちこまログ](https://techikoma.com/blog/herdr-terminal-workspace-claude-code/)
- IME: 0.6.1 で CJK IME 向けに `experimental.reveal_hidden_cursor_for_cjk_ime` と `cjk_ime_agents` を追加（Claude Code・pi・codex は自前でカーソルを描くため macOS の変換候補窓が追従しない問題への対処） — [CHANGELOG](https://github.com/herdrdev/herdr/blob/master/CHANGELOG.md)

### Inferences
- 並走数の分布は「書き手 = 2〜4、ヘビーユーザー = 5〜8、orchestration 実験者 = 7〜15、性能検証 = 26」。**人間がレビューまでやる前提では 2〜4 が実用域**で、それを超える人は例外なく「一覧を優先度キューにする」「状態変化を push で受ける」「未読を保つ」を要求している。necoder の「要対応キュー常設 + 舞台は既定 1 枚・上限 3」は、この分布と整合する。
- 「何をしているか分からない」（HN kerlenton）と「状態は分かるが目的が達成されたかは分からない」（Supervisor）は同じ不足の 2 面で、herdr の状態語彙（terminal lifecycle）の限界。necoder の digest（頼んだこと → いま何を / どう終わったか）と phase（レビュー待ち / 統合済み）がここに効く。
- 性能の不満が「通知・音・枠ラベルを切ると収まる」形で出ているのは、多数の pane を毎フレーム走査する設計の税。necoder も Task 数に比例する render 中の処理（`git diff --shortstat` を render で取らない、等）を避ける現行方針を保つべき。
- Windows の不具合の山は、necoder の W フェーズに対する警告でもある（ConPTY・貼り付け・修飾キーは herdr でも 3 か月かかっている）。

### Gaps
- Reddit / X のスレッドは今回拾えていない（検索結果に一次投稿が出なかった）。HN は 1 スレのみ精読、YC 告知スレは未読。
- 「何体で生産性が頭打ちになるか」の計測・調査は見つからなかった。すべて自己申告。
- HN コメントの引用は要約器経由で、発言者名と文言の厳密な対応は未照合。

## 8. 並べて語られる競合・類似ツールと herdr の位置づけ

### Takeaway
herdr は「**tmux の後継としての agent-aware multiplexer**」で、位置は tmux / Zellij（エージェントを知らない）と Conductor / cmux / Orca（GUI・mac 寄り）の間。差別化は「エージェントが走るマシンの上で動く（SSH・スマホ・複数マシン）」「エージェントを包まず検出する」「1 バイナリ」。task / worktree のライフサイクルを主語にする Claude Squad・dmux・Conductor 型とは設計の主語が違う（herdr の主語は端末）。

### Cited Findings
- 一覧での分類: "Parallel Coding Agents — Terminal (TUI/CLI)"。説明 "Background runtime that owns your agents' terminals: sessions survive reboot and reattach from any terminal or SSH, panes are marked working/blocked/idle, and agents themselves spawn panes and prompt each other over a CLI and socket API. One Rust binary." 同じ節に agent-deck / agent-of-empires（"TUI paired with web view accessible from phones"）/ cmux（"Ghostty-based macOS terminal with vertical tabs and per-agent notifications"）/ dmux（"pairing agents with git worktrees over tmux"）/ claude-squad（"Detached background sessions with worktrees per agent"）ほか。別の節として "Multi-Agent Swarms"・"Autonomous Loop Runners"・"Autonomous Task Runners" がある（※要約経由） — [awesome-agent-orchestrators](https://github.com/andyrewlee/awesome-agent-orchestrators)
- herdr vs cmux（Coles・2026-08-02・※要約経由）: cmux = macOS ネイティブで agent は Mac 上、herdr = agent が走るマシン上で動き client が attach。結論 "If you work locally on a Mac, pick cmux. If your agents run on boxes you attach to, pick herdr."。cmux の利点は WebKit のブラウザ、herdr の利点は plugin（reviewr）。cmux 側の不満は "4.4 GB over about 23 hours" のメモリリーク — [coles.codes: herdr vs cmux](https://coles.codes/posts/herdr-vs-cmux/)
- HN での比較（※要約経由）: Conductor は "way better IME. Running _in_ the terminal is a flex, but doesn't actually bring any advantage."（scubbo）⇔ "mac only. closed source."（mellosouls）/ "Unlike Cmux, Herder will not call out to multiple web services upon start-up."（evanjrowley）/ tmux で足りるのでは（giwook）⇔ "more mouse / touch friendly...worked out of the box"（lucasban）/ Zellij + stop hook で tab に ping（joch）/ Emacs の agent-shell（isityettime）/ Zed + Terminal Thread + tmux（whinvik） — [Hacker News](https://news.ycombinator.com/item?id=48714802)
- tmux / Zellij との対比: "tmux has 18 years of battle-tested stability"、Zellij は "built-in sandboxing" を持つ、herdr は agent awareness と orchestration 用 API を足す（※要約経由） — [bitdoze](https://www.bitdoze.com/herdr-agent-multiplexer/)
- herdr は tmux の中でも動くが、herdr の pane 内で tmux を起動するとエージェント検出は効かない — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- multi-remote を待てずに "ended up with named workspaces in cmux" に流れた利用者もいた（2026-05-30） — [discussion #515](https://github.com/herdrdev/herdr/discussions/515)

### Inferences
- 一覧の分類学が示すとおり、herdr は「並走させて見張る道具」であり「swarm / autonomous runner」ではない。necoder の Fleet + Captain は「Desktop の並走ツール」と「（人間 gate つきの）coordinator」を 1 製品に持つので、herdr と直接は競合せず、**herdr が意図的に空けている上の層**に居る。
- herdr の強み（SSH・スマホ・複数マシン・どの端末でも）は GUI エディタの necoder には構造的に真似しにくい。necoder 側の対応物はリモート管制（P9）と Remote SSH（M9）で、「席を離れている間の blocked をどう捌くか」は herdr が最も磨いている領域として参照する価値がある。

### Gaps
- Claude Squad / ccmanager / agent-of-empires と herdr を**直接**比較した一次記事は見つからなかった（一覧での同居のみ）。
- Orca と herdr の比較記事は今回の範囲では未発見（担当外の可能性が高いので深追いせず）。

## 9. necoder への含意 — herdr が正しくやっていて残すべきもの／herdr が解いていないもの（目的文からの問い）

### Takeaway
残すべきは「状態語彙と rollup の優先度」「done = idle × 未読という導出」「権威は 1 つ・分からなければ unknown」「通知の節度（1 秒遅延・表示中は鳴らさない・done / request で音を分ける）」「エージェント向け CLI の安全な意味論（blocked には送らない・stalled 検出・focus を奪わない）」「worktree は消しても branch は消さない」。herdr が解いていないのは「**目的の達成の追跡・task と依存・coordinator・承認 gate・送信者の帰属・worktree の準備と統合・未読の保持・行の中身（頼んだこと / いま何を）**」で、Captain と Fleet v2 が狙っている場所とほぼ一致する。

### Cited Findings
- 優先度 `Blocked(4) > Idle×未読 = done(3) > Working(2) > Idle×既読(1) > Unknown(0)` を rollup とソートで共有 — [src/workspace/aggregate.rs](https://github.com/herdrdev/herdr/blob/master/src/workspace/aggregate.rs)
- "each pane has one status authority ... This avoids two competing sources of truth." — [agents.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agents.mdx)
- 通知は 1 秒後も同じ状態の時だけ・active tab は抑止・done と request の音を分離・agent 別 mute — [configuration.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/configuration.mdx) / [config-reference.json](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/data/config-reference.json)
- `agent prompt` は blocked の相手に送らず `agent_blocked`、5 秒で活動が見えなければ `agent_prompt_stalled`、timeout は未送信の証明ではない — [agent-automation.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/agent-automation.mdx)
- 見ただけで既読になり完了を見失う、という不満が 3 か月続いている — [discussion #864](https://github.com/herdrdev/herdr/discussions/864)
- orchestration を組む利用者が「polling が切れて blocked を長時間見逃した」と報告 — [#4027](https://github.com/herdrdev/herdr/issues/4027)
- エージェント発の入力が受け手に human として記録され、API に帰属・監査・権限の語が無い — [discussion #3584](https://github.com/herdrdev/herdr/discussions/3584) / [herdr-api.schema.json](https://github.com/herdrdev/herdr/blob/master/docs/next/api/herdr-api.schema.json)
- "terminal state tells me whether a worker is busy or idle, but it does not tell me whether the outcome I wanted is actually complete." — [discussion #3607](https://github.com/herdrdev/herdr/discussions/3607)
- task / Kanban は core に入れない、と作者が明言 — [#301](https://github.com/herdrdev/herdr/issues/301)
- worktree の準備は plugin イベント任せ、統合の機能は docs に無い — [plugins.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/plugins.mdx) / [cli-reference.mdx](https://github.com/herdrdev/herdr/blob/master/docs/versions/0.9.1/website/src/content/docs/cli-reference.mdx)

### Inferences
**herdr が正しくやっていること（necoder が維持 / 借用すべき）**
1. **状態は 4 値 + 既読 1 bit**。`done` を独立状態にせず「idle × 未読」で導出するので、エージェント側の申告に「完了」が要らない。necoder の ThreadActivity（Working / Blocked / Done / Idle）と同型で、この形は正しい。
2. **rollup の優先度を 1 つの関数に閉じ、サイドバーの集約・並び・キー巡回で共有**。necoder の要対応キュー（Blocked 経過順 → Failed → レビュー → 完了未確認）は herdr の順に Failed / レビューを足した上位互換。バケット内は「最後に状態が変わった時刻」で並べる、という #318 の追加案は経過順と同じ発想。
3. **権威は 1 つ**。ACP のイベントと端末の見た目を混ぜない。CLI 形態のエージェントを混ぜる時だけ herdr 式の「unknown を正直に出す + explain」を使う。
4. **通知の節度**（短い遅延で揺れを吸収・見ている所は鳴らさない・「終わった」と「呼んでいる」を別の音に）。
5. **エージェント向け CLI の意味論**（blocked には送らない / stalled を失敗として返す / 再送前に読め / `--no-focus` / ID は応答から）。`fleet send`・`spawn-agent` の契約にそのまま写せる。
6. **片付けの安全側**（branch は消さない・Git に先に拒否させる・閉じる ≠ 消す）。
7. **行はトークンの組合せ + 外部から申告できる表示専用メタデータ**（`$summary`）で、意味論（state）と表示（label）を分ける。necoder の digest は「表示専用」であることを保ち、状態の判定に使わない。

**herdr が解いていないこと（Fleet / Captain の存在理由）**
1. **目的の達成**: 端末の lifecycle しか見ない。review_ready / merge_ready / integrated のような phase は無い。
2. **coordinator**: 無い。プリミティブだけ渡し、task・依存・queue は core に入れない方針。上物を作る利用者は polling・単一 wait・取りこぼし（#4178）で苦労している → Captain のイベント駆動 wake・まとめ渡し・台帳がそのまま答えになる。
3. **承認 gate**: 技術的には無い（skill の文章だけ）。necoder の「Captain は推薦のみ・応答と Integrate は人間」は herdr に対する明確な差。
4. **帰属と監査**: エージェント発の入力が human と記録される。necoder は `human_send` / `captain` を台帳とニュースに残す設計を保つこと（Captain を通さない介入を Captain に知らせる §5.4 も herdr には無い概念）。
5. **worktree の前後**: 準備（.env・依存・target 共有）と統合（radar・Integrate）は無い。
6. **未読の保持**: 見ただけで消える。necoder は明示の「確認」を既定にし、dwell と「後で見る」を検討する価値がある。
7. **行の中身**: 既定は workspace・tab・agent 名だけ。「頼んだこと → いま何を」は利用者の自作。
8. **コスト**: トークン・費用の組込み表示は無い。necoder の「トークン常時表示」「Captain のコストも Σ に含める」は独自の価値。
9. **検出の確実性**: 画面スクレイピングの宿命で、agent の版が上がるたびに壊れる。ACP を権威にできる necoder はここで構造的に有利。

**necoder 文書の直しどころ**
- `fleet-ux-2026-09.md` §2 の herdr 行と `FLEET-V2.md` §3.4 の「herdr の 3 本上限」は、herdr の仕様ではなく Zenn 記事著者の hook スクリプトの閾値（152 桁の端末で 50 桁 × 3）。「上限 3」の判断自体は変える必要は無いが、根拠は「herdr 準拠」ではなく「1 利用者の経験則 + necoder のカード最小幅 420px」と書くのが正確。
- 同表の「状態は working / blocked / idle / done の 4 値」は、正確には 5 値（+ `unknown`）で、`done` は導出。「SessionStart hook で pane 自動作成」も herdr の機能ではなく同記事のカスタマイズ（しかも「作成」ではなく既存 pane の「移動」）。

### Gaps
- herdr の今後（Herdr Cloud・チーム拡大後）に orchestration 層が入るかは不明。入れば上の「解いていないこと」2〜4 の前提が変わるので、四半期ごとに CHANGELOG と discussions の Ideas を見直すのがよい。
- necoder 側の実装状況（F6 の残: 台帳イベントの灰色カード・承認の推薦・トークン表示）との突き合わせは本調査の範囲外。
