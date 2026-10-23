# オーケストレータ–ワーカー型マルチエージェント LLM システムの研究知見と工学的証拠（necoder Fleet / Captain 設計のために）

調査日: 2026-09-20。対象: 2023 年〜2026 年 9 月の論文・開発元の技術記事・公式ドキュメント。

> 調査メモ（読み手への注意）
> - 引用文は原則として取得ツール（要約モデル経由）で抽出した表記。原典を直接確認したのは次の 3 つ: MAST の分類と数値（arXiv PDF v3 をテキスト化して照合）、Brun et al. 2011 の数値（PDF を目視）、Claude Code 公式ドキュメント agent-teams（原文全文を取得）。それ以外を一字一句の引用として最終稿に使うなら原典で再確認すること。
> - 証拠等級タグ: 【査読済】= 査読付き会議・誌と確認できたもの / 【preprint】= arXiv のみ・単一研究 / 【ベンダー一次】= 開発元の技術記事・公式 docs（一次だが自己申告で独立再現なし）/ 【事例】= 個人の実践報告。
> - 取得できなかった一次資料: Steve Yegge "Welcome to Gas Town"（Medium・HTTP 403）→ GitHub README で代替。OpenAI "Harness engineering"（openai.com・HTTP 403）→ 全文の保存コピーで内容確認（両方の URL を併記）。

## 1. アーキテクチャとそのトレードオフ（supervisor / 階層 / blackboard・共有台帳 / 討論 / 市場 / 検証付きパイプライン）

### Takeaway
中央集権型（orchestrator–worker）は「誤りの伝播を止める検証の関所」として働くが、トークンを約 3.9 倍、ターン数を約 3.8 倍に増やす。効くのは分解可能で、かつ単一エージェントがまだ飽和していない課題だけで、逐次依存の強い課題では全てのマルチエージェント構成が単一エージェントより悪い。討論型・市場型には、書き込みを伴うコーディング協調で有効だという証拠が無い。

### Cited Findings

**定義と正典**

- 【ベンダー一次】Anthropic は workflow と agent を区別する: "Workflows are systems where LLMs and tools are orchestrated through predefined code paths. Agents, on the other hand, are systems where LLMs dynamically direct their own processes and tool usage." orchestrator-workers は "a central LLM dynamically breaks down tasks, delegates them to worker LLMs, and synthesizes their results" で、向くのは "complex tasks where you can't predict the subtasks needed"（例: "Coding products that make complex changes to multiple files each time"）。並列化との違いは "its flexibility"。全体方針は "find the simplest solution possible, and only increasing complexity when needed"、代償は "Agentic systems often trade latency and cost for better task performance"（2024-12-19）— [Building effective agents](https://www.anthropic.com/engineering/building-effective-agents)
- 【preprint・技術報告】Magentic-One（Microsoft Research, 2024-11-07）: Orchestrator が外側ループで **Task Ledger**（"given or verified facts, facts to look up (e.g., via web search), facts to derive (e.g., programmatically, or via reasoning), and educated guesses" と自然言語の計画）を持ち、内側ループで **Progress Ledger** の 5 問に毎ステップ答える: "Is the request fully satisfied (i.e., task complete)?" / "Is the team looping or repeating itself?" / "Is forward progress being made?" / "Which agent should speak next?" / "What instruction or question should be asked of this team member?" — [Magentic-One (arXiv 2411.04468)](https://arxiv.org/html/2411.04468v1)
- 【preprint・技術報告】同上の stall 検知: ループや停滞でカウンタが増え、"If the counter exceeds the threshold, the Orchestrator breaks from the inner loop"（実験の閾値は ≤2）。その後 "initiates a reflection and self-refinement step" で Task Ledger と計画を更新して内側ループをやり直す — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【preprint・技術報告】同上のアブレーション: Orchestrator の台帳を外すと "performance drops by 31%"。個別エージェントを外すと 21〜39% 低下。GAIA 38.00±5.5%（GPT-4o + o1）、AssistantBench 13.3±4.9%、WebArena 32.8±3.2% — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【preprint・技術報告】同上の限界: "Magentic-One requires dozens of iterations and LLM calls to solve most problems"、コストは "perhaps several US dollars, and tens of minutes per task"。チームが 5 体固定で "when agents are not needed, they simply serve as a distraction to the Orchestrator" — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【preprint】AutoGen（2023-08-16）は "multiple agents that can converse with each other" を汎用基盤として提示 — [AutoGen (arXiv 2308.08155)](https://arxiv.org/abs/2308.08155)。v0.4（2025-01）で actor model に作り直し、エージェントが非同期メッセージに反応するイベント駆動へ移行 — [Microsoft Research: AutoGen v0.4](https://www.microsoft.com/en-us/research/blog/autogen-v0-4-reimagining-the-foundation-of-agentic-ai-for-scale-extensibility-and-robustness/)
- 【preprint】MetaGPT（2023-08-01, v7 2024-11-01）: LLM を素朴に連鎖させると "logic inconsistencies due to cascading hallucinations caused by naively chaining LLMs" が出る。対策として SOP をプロンプト列に埋め、"assembly line paradigm" で中間成果物を検証させる — [MetaGPT (arXiv 2308.00352)](https://arxiv.org/abs/2308.00352)
- 【査読済・ACL 2024】ChatDev: 何を話すかを chat chain、どう話すかを communicative dehallucination で縛る — [ChatDev (arXiv 2307.07924)](https://arxiv.org/abs/2307.07924)
- 【査読済・NeurIPS 2023】CAMEL は 2 体ロールプレイで観察した失敗を 4 つ挙げる: role flipping（"the assistant starts providing instructions or commands instead of following the user's prompts"）、指示の復唱、flake reply（"the assistant promises to take action but ultimately fails to follow through"）、無限ループ（"repeatedly thanking each other or saying goodbye without progressing the task"）。終了条件は 5 つ（ユーザー役が 3 ラウンド指示しない / 役割逆転 / `<CAMEL_TASK_DONE>` / トークン上限 / "a maximum limit of 40 messages"）— [CAMEL (ar5iv 2303.17760)](https://ar5iv.labs.arxiv.org/html/2303.17760)
- 【preprint】AgentVerse（2023-08-21）: チーム構成を動的に変える。創発する社会的行動には良いものと悪いものがあり "leverage positive [behaviors] and mitigate negative ones" を論じる — [AgentVerse (arXiv 2308.10848)](https://arxiv.org/abs/2308.10848)
- 【ベンダー一次】LangChain の現行 docs は 5 パターン（subagents / handoffs / skills / router / custom workflow）を定義。subagents は "A main agent coordinates subagents as tools. All routing passes through the main agent"、subagent は "stateless by design" で "the main agent maintains conversation context, but subagents start fresh each time"。前置きとして "Not every complex task requires this approach—a single agent with the right (sometimes dynamic) tools and prompt can often achieve similar results"、中心課題は "At the center of multi-agent design is context engineering—deciding what information each agent sees"（2026-09-20 閲覧）— [LangChain docs: Multi-agent](https://docs.langchain.com/oss/python/langchain/multi-agent)
- 【ベンダー一次】OpenAI Agents SDK の handoff は LLM には `transfer_to_<agent_name>` というツールとして見え、既定では "the new agent takes over the conversation, and gets to see the entire previous conversation history"（`input_filter` で変更可）。会話を渡さずに構造化入力で呼ぶ manager pattern は `Agent.as_tool` を使う（2026-09-20 閲覧）— [OpenAI Agents SDK: Handoffs](https://openai.github.io/openai-agents-python/handoffs/)
- 【ベンダー一次】CrewAI の hierarchical process: "A 'manager' agent coordinates the workflow, delegates tasks, and validates outcomes"。タスクは事前割当てでなく manager が能力で配る。暴走止めは max_rpm と max iterations（2026-09-20 閲覧）— [CrewAI docs: Hierarchical Process](https://docs.crewai.com/en/learn/hierarchical-process)
- 【ベンダー一次】Claude Code は 2 形態を分ける。subagents = "Main agent manages all work"・結果だけ返す・token は "Lower: results summarized back to main context"。agent teams = 共有 task list + mailbox で自己調整・"Higher: each teammate is a separate Claude instance"（実験的機能・2026-09-20 閲覧）— [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【preprint・サーベイ】協調機構は actors / types（cooperation, competition, coopetition）/ structures（peer-to-peer, centralized, distributed）/ strategies（role-based, model-based）/ coordination protocols の 5 次元で整理できる（2025-01-10）— [Tran et al. (arXiv 2501.06322)](https://arxiv.org/abs/2501.06322)。ソフトウェア工学向け MAS の体系的レビュー（TOSEM 特集号）— [He, Treude, Lo (arXiv 2404.04834)](https://arxiv.org/abs/2404.04834)

**構成どうしの定量比較**

- 【preprint】"Towards a Science of Scaling Agent Systems"（Kim ら, v1 2025-12-09 / v3 2026-04-08）: 260 構成 × 6 ベンチマーク × 5 構成（Single / Independent / Centralized / Decentralized / Hybrid）× 3 モデル族で、ツール・プロンプト・計算量を揃えて比較。要旨の結論: "(1) a coordination yields diminishing returns once single-agent baselines exceed certain performance; (2) tool-heavy tasks appear to incur multi-agent overhead; and (3) architectures without centralized verification tend to propagate errors more than those with centralized coordination"。単一比の変化は "+80.8% on decomposable financial reasoning to -70.0% on sequential planning" — [arXiv 2512.08296](https://arxiv.org/abs/2512.08296)
- 【preprint】同論文の誤り増幅: "Independent systems amplify trace-level errors 17.2× through unchecked error propagation... Centralized coordination, however, contains this to 4.4× by enforcing validation bottlenecks that intercept errors before aggregation"（Decentralized 7.8×、Hybrid 5.1×）— [arXiv 2512.08296 v3](https://arxiv.org/html/2512.08296v3)
- 【preprint】同論文の協調コスト: token overhead は Independent 58% / Decentralized 263% / Centralized 285% / Hybrid 515%。平均ターン数は SAS 7.2 → Independent 11.4 → Decentralized 26.1 → Centralized 27.7 → Hybrid 44.3 で "T = 2.72 × (n + 0.5)^1.724, R² = 0.974"。ターンあたり成功率は SAS 0.466 に対し Centralized 0.120 — [arXiv 2512.08296 v3](https://arxiv.org/html/2512.08296v3)
- 【preprint】同論文の飽和効果と人数上限: "Tasks where single-agent performance already exceeds 45% accuracy experience negative returns from additional agents"（β = −0.236, p = 0.004）。"Under fixed computational budgets, per-agent reasoning capacity becomes prohibitively thin beyond 3–4 agents" — [arXiv 2512.08296 v3](https://arxiv.org/html/2512.08296v3)
- 【preprint】同論文のベンチマーク別: Finance-Agent は Centralized +80.8%（0.631 対 0.349）。PlanCraft は全 MAS が悪化（Centralized −50.3%、Independent −70.0%）。**SWE-bench Verified は全 MAS が単一以下**（Hybrid −2.1%、Centralized −3.1%、Decentralized −5.4%、Independent −14.9%、SAS 0.522）。Terminal-Bench は Centralized −19.2% — [arXiv 2512.08296 v3](https://arxiv.org/html/2512.08296v3)
- 【ベンダー一次】同研究の Google Research ブログ（2026-01-28）は金融推論の改善を "80.9%" と書く（arXiv v3 要旨は +80.8%。版の違いによる軽微な不一致）。実務指針は「逐次依存とツール密度を見て構成を選ぶ」— [Google Research blog](https://research.google/blog/towards-a-science-of-scaling-agent-systems-when-and-why-agent-systems-work/)
- 【ベンダー一次】LangChain の比較（2025-06-10・τ-bench に無関係ドメインを足した版）: single agent は "falls off sharply when there are two or more distractor domains"。supervisor は swarm より常にトークンが多い。原因は "sub agents cannot respond to the user directly" で supervisor が "play telephone" するから。handoff メッセージを subagent の state から外す・`forward_message` ツールで言い換えを避ける・ツール名の言い回しを変える、の 3 点で "a nearly 50% increase in performance" — [LangChain: Benchmarking Multi-Agent Architectures](https://www.langchain.com/blog/benchmarking-multi-agent-architectures)
- 【preprint】"Single-agent or Multi-agent Systems? Why Not Both?"（2025-05-23）: "the benefits of MAS over SAS diminish as LLM capabilities improve"。SAS と MAS の間で要求を振り分けるカスケードで精度 +1.1〜12%、コスト最大 −20% — [arXiv 2505.18286](https://arxiv.org/abs/2505.18286)
- 【preprint】MultiAgentBench（2025-03-03）: star / chain / tree / graph を比較し "graph structure performs the best among coordination protocols in the research scenario, and cognitive planning improves milestone achievement rates by 3%" — [arXiv 2503.01935](https://arxiv.org/abs/2503.01935)
- 【査読済・NeurIPS 2025 Spotlight】討論型: 7 ベンチマークで "Majority Voting alone accounts for most of the performance gains typically attributed to MAD"。討論は信念の軌跡に martingale を誘導し "debate alone does not improve expected correctness" — [Debate or Vote (arXiv 2508.17536)](https://arxiv.org/abs/2508.17536)
- 【preprint】blackboard 型（2025-07-02）: 全エージェントが黒板を共有し、黒板の内容で次に動くエージェントを選ぶ。常識・推論・数学データセットで "competitive with the SOTA static and dynamic MASs... and at the same time manage to spend less tokens"（コーディングは対象外）— [arXiv 2507.01701](https://arxiv.org/abs/2507.01701)
- 【preprint】市場・オークション型: Agora（2026-07-10）は較正済み確信度で入札させて推論ステップを割り当て、5 ベンチマークで "improves or remains competitive with single-model, routing, and cascade baselines"（要旨に数値なし）— [Agora (arXiv 2607.09600)](https://arxiv.org/abs/2607.09600)
- 【ベンダー一次】Anthropic Research（lead = Opus 4、subagent = Sonnet 4）は単一 Opus 4 を内部評価で 90.2% 上回った。BrowseComp の分散は "token usage by itself explains 80% of the variance"、3 要因で 95%（2025-06-13）— [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【preprint】検証付きの固定パイプライン（localization → repair → patch validation）の Agentless は、2024 年時点で SWE-bench Lite 32.00%・$0.70 と当時のオープンソース agent 勢を上回り、"Do we really have to employ complex autonomous software agents?" と問うた — [Agentless (arXiv 2407.01489)](https://arxiv.org/abs/2407.01489)

### Inferences
- Captain の形（hub-and-spoke・worker 同士は話さない・中央で確認する）は、比較研究で誤り増幅が最小（4.4×）の構成に当たる。ただし同じ研究で中央集権型は token +285%・ターン 27.7 と安くない。Captain の wake を絞る設計は飾りではなく必須条件。
- Magentic-One の「要らないエージェントは Orchestrator の気を散らす」は、Captain を常に経由させると単純作業で往復が増える問題と同型。FLEET-V2 §5.4 の「1 件だけなら ＋Task で直接」は証拠と合う。
- handoff 型（会話ごと移譲）は Captain に向かない。Captain は会話を渡さず Task を作って結果を受け取る manager pattern で、LangChain の言う「supervisor の伝言ゲーム」を避けるには、worker の成果を Captain が言い換えずに参照で人間へ出す経路が要る。
- 討論・投票は「同じ問いに複数案を出して選ぶ」用途（原因調査・設計案比較）なら使える。並列実装の調整機構として支持する証拠は無い。

### Gaps
- 市場・オークション型をコーディング協調に使った実証は見つからなかった（Agora は推論ステップの割当て、MarketBench は題名のみ確認）。
- MetaGPT の shared message pool（publish-subscribe）は本文の記述で、今回は abstract しか確認していない。
- OpenAI Swarm（2024）の一次資料は未取得。Agents SDK docs と LangChain の "swarm" 定義のみ。
- 比較研究の多くは preprint でモデル世代に依存する（scaling 論文は GPT-5 / Gemini 2.5 / Claude Sonnet 4.5 世代まで）。同論文は版ごとに数値が改訂されているので、引用時は版（v3）を明記すること。

## 2. 失敗分析（MAST・Cognition 論争とその後・並列が効く場面と害になる場面）

### Takeaway
MAS の失敗はモデルの弱さより組織設計から来る。最頻の失敗は手順の反復（15.7%）・推論と行動の不一致（13.2%）・終了条件の不認識（12.4%）・仕様逸脱（11.8%）。Cognition の 2025 年の反対論は、2026 年 4 月に同じ著者が「並列書き込みスウォームには今も当てはまる。書き込みを単線に保ち、他のエージェントが知性だけを足す型は実用になった」と更新した。並列が効くのは読み取り中心・幅優先・独立に検証できる仕事。

### Cited Findings

**MAST（Cemri et al., "Why Do Multi-Agent LLM Systems Fail?"）**

- 【preprint・広く引用】Cemri, Pan, Yang ら（UC Berkeley ほか）、v1 2025-03-17 / v3 2025-10-26。7 つの MAS フレームワークから 1600 超の注釈付きトレース（MAST-Data）。分類は 150 トレースを人手で分析して作り、注釈者間一致は κ = 0.88。14 モード・3 カテゴリ — [arXiv 2503.13657](https://arxiv.org/abs/2503.13657)
- 【preprint・PDF で照合済み】v3 の Figure 1 の数値。**System Design Issues 44.2%**: 1.1 Disobey Task Specification 11.8% / 1.2 Disobey Role Specification 1.50% / 1.3 Step Repetition 15.7% / 1.4 Loss of Conversation History 2.80% / 1.5 Unaware of Termination Conditions 12.4%。**Inter-Agent Misalignment 32.3%**: 2.1 Conversation Reset 2.20% / 2.2 Fail to Ask for Clarification 6.80% / 2.3 Task Derailment 7.40% / 2.4 Information Withholding 0.80% / 2.5 Ignored Other Agent's Input 1.90% / 2.6 Reasoning-Action Mismatch 13.2%。**Task Verification 23.5%**: 3.1 Premature Termination 6.20% / 3.2 No or Incomplete Verification 8.20% / 3.3 Incorrect Verification 9.10% — [MAST PDF v3](https://arxiv.org/pdf/2503.13657v3)
- 【注意】同じ PDF の方法論の図には初期版の数値（37.17% / 31.41% / 31.41%）が残っている。二次資料はこの旧値を引くことが多いので、引用時は版を明記すること — [MAST PDF v3](https://arxiv.org/pdf/2503.13657v3)
- 【preprint】失敗率: "Our empirical analysis reveals 41% to 86.7% failure rate on 7 state-of-the-art (SOTA) open-source MAS"。対象は ChatDev・MetaGPT（ProgramDev）、HyperAgent（SWE-Bench Lite）、AppWorld、AG2（数学）、Magentic-One（GAIA）、OpenManus で、**ベンチマーク上のオープンソース MAS の値**。本番環境の失敗率ではない — [MAST HTML v3](https://arxiv.org/html/2503.13657v3)
- 【誤引用の例】後続の preprint が要旨で "Multi-agent LLM systems fail in production at rates between 41% and 87%" と書いているが、出典の MAST は本番の測定ではない — [Coordination as an Architectural Layer (arXiv 2605.03310)](https://arxiv.org/abs/2605.03310)
- 【preprint】MAST の主張: "Insight 1. MAS failure is not merely a function of challenges in the underlying model; a well-designed MAS can result in performance gain when using the same underlying model." 根拠は介入実験で、ChatDev は役割仕様の改善だけで成功率 +9.4%、高レベル目標の検証ステップ追加で +15.6%（同じ GPT-4o・同じユーザープロンプト）。ただし "not all failure modes are resolved, and task completion rates still remain low" — [MAST HTML v3](https://arxiv.org/html/2503.13657v3)
- 【preprint】検証について: "Systems with explicit verifiers like MetaGPT and ChatDev generally show fewer total failures... However, the presence of a verifier is not a silver bullet"。"many existing verifiers perform only superficial checks, despite being prompted to perform thorough verification, such as checking if the code compiles or if there are leftover TODO comments"。"Insight 3. Multi-Level Verification is Needed... sole reliance on final-stage, low-level checks is inadequate." — [MAST HTML v3](https://arxiv.org/html/2503.13657v3)
- 【preprint】組織論との接続: "many MAS failures arise from the challenges in organizational design and agent coordination rather than the limitations of individual agents"。high-reliability organizations の設計原則を参照 — [MAST HTML v3](https://arxiv.org/html/2503.13657v3)
- 【preprint】LLM 注釈器（o1 + few-shot）は人手と accuracy 94%・κ = 0.77。領域外（OpenManus・Magentic-One）でも κ = 0.79 — [MAST HTML v3](https://arxiv.org/html/2503.13657v3)

**MAST 以後（2025〜2026）**

- 【preprint】Who&When（2025-04-30）: 127 の MAS の失敗ログに「どのエージェントのどのステップが決定的な誤りか」を注釈。最良手法でも **エージェント特定 53.5%・ステップ特定 14.2%** — [arXiv 2505.00212](https://arxiv.org/abs/2505.00212)
- 【preprint・単一事例】本番の個人用エージェント基盤を 8 週間追った研究（2026-06-12）: 22 件の障害で「エラー信号が人間に対処可能な形で届かない」型が 28 回以上。サイレント障害の 70% は人間の観察で見つかり、自動テスト・監査では見つからなかった。潜伏は 13 時間〜60 日。最も危険なのは誤りが "fluent, plausible narrative" に化ける "fail-plausible" 型。設計原則は障害を "loud, attributable, and boring" にすること — [When Errors Become Narratives (arXiv 2606.14589)](https://arxiv.org/abs/2606.14589)
- 【preprint・技術報告】Magentic-One 自身の誤り分析で最多の 3 つ: persistent-inefficient-actions（"Agents repeatedly engage in unproductive behaviors without adapting their strategies"）/ insufficient-verification-steps（"Tasks are marked as complete without thorough validation"）/ inefficient-navigation-attempts — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【preprint】TeamBench（2026-05-08）: Planner / Executor / Verifier の役割分離を OS のアクセス制御で強制した 931 インスタンス。プロンプトだけの分離と合格率は統計的に区別できないが、プロンプトだけの場合は verifier が executor のコードを編集しようとする件数が 3.6 倍。**verifier は決定的採点で不合格の提出の 49% を承認した**。単体で解ける課題ではチームの上積みが小さく、アブレーションでは verifier を外すと平均部分点が上がった — [arXiv 2605.07073](https://arxiv.org/abs/2605.07073)

**Cognition の反対論（2025）と本人による更新（2026）**

- 【ベンダー一次】Walden Yan "Don't Build Multi-Agents"（2025-06-12）の 2 原則: "Share context, and share full agent traces, not just individual messages" / "Actions carry implicit decisions, and conflicting decisions carry bad results"。Flappy Bird の例では subagent 1 が Super Mario 風の背景を、subagent 2 がゲームらしくない鳥を作る。2025 年時点の評価は "running multiple agents in collaboration only results in fragile systems. The decision-making ends up being too dispersed and context isn't able to be shared thoroughly enough between the agents" — [Cognition: Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents)
- 【ベンダー一次】同記事の代替案: "just use a single-threaded linear agent"。長い作業には履歴を "key details, events, and decisions" に圧縮する専用モデルを挟む。Claude Code の subagent は "it never does work in parallel with the subtask agent, and the subtask agent is usually only tasked with answering a question, not writing any code" だから成り立つ、と評した — [Cognition: Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents)
- 【ベンダー一次】同記事の人間との対比: "If Engineer A's code causes a merge conflict with Engineer B, the correct protocol is to talk out the differences and reach a consensus. However, agents today are not quite able to engage in this style of long-context proactive discourse with much more reliability than you would get with a single agent." — [Cognition: Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents)
- 【ベンダー一次】同じ著者の続編 "Multi-Agents: What's Actually Working"（2026-04-22）: "Our original observations still hold today for parallel-writer swarms"。うまくいくのは "setups where multiple agents contribute intelligence to a task while writes stay single-threaded"。結論は **"multi-agent systems work best today when writes stay single-threaded and the additional agents contribute intelligence rather than actions"** — [Cognition: Multi-Agents: What's Actually Working](https://cognition.com/blog/multi-agents-working)
- 【ベンダー一次】同続編で動いている型: code-review loop（"Devin Review catches an average of 2 bugs per PR, of which roughly 58% are severe"。"we found this technique to work best when the coding and review agents do not share any context beforehand"）、smart friend（弱いモデルが強いモデルに相談する）、manager + 子 Devin の "map-reduce-and-manage"。動いていない型: "the unstructured-swarm approach, arbitrary networks of agents negotiating with each other, is mostly a distraction" — [Cognition: Multi-Agents: What's Actually Working](https://cognition.com/blog/multi-agents-working)
- 【ベンダー一次】同続編が挙げる manager の失敗: "Managers trained on small-scoped delegation default to being overly prescriptive"、"Agents assume they share state with their children when they don't"、エージェント間の連絡は "doesn't happen by default"。未解決問題: "The open problems are all communication problems. How does a weaker model learn when to escalate? How does a child agent surface a discovery that should change its siblings' work? How do you transfer context between agents without drowning the receiver?" — [Cognition: Multi-Agents: What's Actually Working](https://cognition.com/blog/multi-agents-working)
- 【ベンダー一次】製品化（2026-03-19）: "The main Devin session acts as a coordinator: it scopes the work, assigns each piece to a managed Devin, monitors progress, resolves any conflicts, and compiles the results." 子は隔離 VM で、テストを回してから報告する。例示された用途は QA の並列実行・大規模ライブラリ移行・複数サービスの依存監査・class→functional の機械的リファクタ（同時 10 件の例）— [Cognition: Devin can now Manage Devins](https://cognition.com/blog/devin-can-now-manage-devins)

**Anthropic 側の立場（反論というより条件付け）**

- 【ベンダー一次】Anthropic も 2025 年時点でコーディングには慎重だった: "some domains that require all agents to share the same context or involve many dependencies between agents are not a good fit for multi-agent systems today. For instance, most coding tasks involve fewer truly parallelizable tasks than research"。向くのは "valuable tasks that involve heavy parallelization, information that exceeds single context windows, and interfacing with numerous complex tools"。経済条件は "multi-agent systems require tasks where the value of the task is high enough to pay for the increased performance" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】Anthropic（2026-01-23）: "At Anthropic, we've seen teams invest months building elaborate multi-agent architectures only to discover that improved prompting on a single agent achieved equivalent results." マルチが一貫して勝つ場面は context protection・parallelization・specialization の 3 つ。並列化は "The primary benefit of parallelization is thoroughness, not speed"、代償は "higher token usage and often longer total execution time" — [Building multi-agent systems: when and how to use them](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)
- 【ベンダー一次】同記事の分割の指針: "Work should only be split when context can be truly isolated"。役割（問題の種類）で割ると "they engage in a 'telephone game,' passing information back and forth with each handoff degrading fidelity"。良い境界は "Independent research paths"、"Separate components with clean interfaces"、"Blackbox verification"。悪い境界は "Sequential phases of the same work"、"Tightly coupled components"、"Work requiring shared state" — [Building multi-agent systems: when and how to use them](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)
- 【ベンダー一次】Claude Code docs の使い分け: agent teams が効くのは research and review・new modules or features（"teammates can each own a separate piece without stepping on each other"）・competing hypotheses のデバッグ・cross-layer の変更。逆に "For sequential tasks, same-file edits, or work with many dependencies, a single session or subagents are more effective"。初心者には "start with tasks that have clear boundaries and don't require writing code" — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【事例】Simon Willison（2025-10-05）が並列で回せると感じた仕事は 4 種: 手元に残さない調査・PoC、既存システムの仕組みの確認、小さな保守、自分が仕様を書いた作業（"Code that started from your own specification is a lot less effort to review"）。制約は "I can only focus on reviewing and landing one significant change at a time" と "Reviewing code that lands on your desk out of nowhere is a lot of work" — [Embracing the parallel coding agent lifestyle](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/)

### Inferences
- 論争は 2026 年に収束した。両陣営とも「並列で書くスウォームは脆い」「知性を足すだけの追加エージェント（レビュー・調査・助言）は効く」で一致している。necoder の Fleet は Task 内では書き手が 1 体だが、Task 間では並列に書く。Cognition の第 2 原則（暗黙の決定の衝突）は統合の時点で表に出るので、Task 間の決定を運ぶのは Captain の仕事になる。
- MAST の上位モードは Captain の責務にそのまま写せる。1.5 / 3.1（終了条件）→ 委任文に完了条件を書く。1.3（反復）→ stall 検知。2.2（確認を求めない）→ 曖昧な目標は分割する前に人間へ聞く。3.2 / 3.3（検証）→ worker の自己申告でなく環境の事実で判定する。
- TeamBench の「プロンプトだけの役割分離は破られる（3.6 倍）」は、Captain の「コードを書かない・merge しない」をプロンプトでなくツール権限で強制すべき根拠になる（詳細は 7 節）。
- Who&When の数値（ステップ特定 14.2%）は、事後にログから原因を割り出すのが難しいことを示す。采配の時点で理由と入力を記録しておく方が安い。

### Gaps
- MAST の採択会議は今回確認していない（arXiv の表示のみ）。
- MAST の対象は GPT-4o / Claude 3.7 世代の汎用 MAS フレームワーク。Claude Code や Codex を worker にした 2026 年世代の系の失敗分布は測られていない。最も近いのは CooperBench（3 節）。
- Cognition の 2026 年の数値（2 bugs/PR、58% severe、利用 8 倍）は自社製品の自己申告で、独立した検証は無い。
- 検索では MAST の後続として StepFinder（arXiv 2606.03467）、因果推論による帰属（arXiv 2509.08682）、"Silent Failure in LLM Agent Systems"（arXiv 2606.08162）も見つかったが未読。

## 3. コーディング特化の証拠（SWE-bench 系・長期プロジェクトの事例・試して捨てた構造・コスト）

### Takeaway
大規模な成功事例（Cursor の数百エージェント、Anthropic の 16 並列 C コンパイラ、OpenAI の 100 万行）は実在する。共通の前提は「ほぼ完璧な自動検証器」「誤りを許して後で直す」「グリーンフィールド」「巨額のトークン」。人間が品質を負う既存リポジトリでは逆の証拠が並ぶ。2 体で協調すると成功率が平均 30% 下がり（CooperBench）、SWE-bench Verified では全 MAS 構成が単一エージェント以下だった。

### Cited Findings

**Cursor の長期自走実験（試して捨てた構造の記録として最も詳しい）**

- 【ベンダー一次】Wilson Lin "Scaling long-running autonomous coding"（2026-01-14）。第 1 案は対等なエージェントが共有ファイルとロックで自己調整する方式で失敗: "Agents would hold locks for too long, or forget to release them entirely." "Twenty agents would slow down to the effective throughput of two or three, with most time spent waiting." — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】第 2 案の楽観的並行制御も失敗: "With no hierarchy, agents became risk-averse. They avoided difficult tasks and made small, safe changes instead." 結果は "work churning for long periods of time without progress" — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】採用した形: "Planners continuously explore the codebase and create tasks. They can spawn sub-planners for specific areas." "Workers pick up tasks and focus entirely on completing them. They don't coordinate with other workers or worry about the big picture." 周期の終わりに judge が続行を判定 — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】integrator を外した: "We initially built an integrator role for quality control and conflict resolution, but found it created more bottlenecks than it solved. Workers were already capable of handling conflicts themselves." 教訓は "The right amount of structure is somewhere in the middle. Too little structure and agents conflict, duplicate work, and drift. Too much structure creates fragility." と "Many of our improvements came from removing complexity rather than adding it." — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】未解決として挙げたもの: "Planners should wake up when their tasks complete to plan the next step. Agents occasionally run for far too long. We still need periodic fresh starts to combat drift and tunnel vision." — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】報告された規模: ブラウザを "close to a week, writing over 1 million lines of code across 1,000 files"、Solid→React 移行 "+266K/-193K edits"（3 週間）、動画レンダリング "25x faster"、Java LSP "7.4K commits, 550K LoC"、Windows 7 エミュレータ "14.6K commits, 1.2M LoC"、Excel "12K commits, 1.6M LoC"。"We've deployed trillions of tokens across these agents toward a single goal." — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】続報 "Towards self-driving codebases"（2026-02-05）は 5 段階を記録する。(1) 単一エージェント: "frequently stopped to proclaim success despite being far from it"。(2) 共有状態ファイル: "20 agents would slow to the throughput of 1-3"、"Locking is easy to get wrong and narrowly correct, and more prompting didn't help"。(3) planner→executor→workers→judge の固定役割: "bottlenecked by the slowest worker. It was too rigid"。(4) continuous executor: "It would sleep randomly, stop running agents, do work itself, refuse to plan and spawn more than a few narrowly focused tasks, not properly merge worker changes, and claim premature completion"、原因は "it was being given too many roles and objectives simultaneously, including: plan, explore, research, spawn tasks, check on workers, review code, perform edits, merge outputs, and judge if the loop is done"。(5) 再帰的 planner + 独立 worker — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)
- 【ベンダー一次】同続報の「誤りを許す」判断: "When we required 100% correctness before every single commit, it caused major serialization and slowdowns of effective throughput." "Errors arise then get fixed quickly. The error rate remains small and constant, perhaps rarely completely clean but steady and manageable." ただし "the ideal efficient system accepts some error rate, but a final 'green' branch is needed where an agent regularly takes snapshots and does a quick fixup pass before release" — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)
- 【ベンダー一次】同続報の handoff: "The handoff contains not just what was done, but important notes, concerns, deviations, findings, thoughts, and feedback. The planner receives this as a follow-up message." 規模は ~1,000 commits/時・1 週間で 10M tool calls・数百エージェント・単一の大型 Linux VM — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)
- 【ベンダー一次】同続報の指示の教訓: "This amplifies everything, including suboptimal and unclear instructions." 最初のブラウザは "converged on an architecture that was unfit to evolve into a full browser. This was a failure of the initial specification." "Constraints are more effective than instructions. 'No TODOs, no partial implementations' works better than 'remember to finish implementations.'" 数値の幅を渡す（"Generate 20-100 tasks"）と挙動が変わる — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)
- 【ベンダー一次】同続報の基盤の罠: "Many tools like Git and Cargo use shared locks"。数百体が同時にコンパイルするとディスク I/O が律速になった — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)

**Anthropic の C コンパイラ（orchestrator なしの並列）**

- 【ベンダー一次】Nicholas Carlini（2026-02-05）: 16 エージェント、約 2,000 の Claude Code セッション、API 費用 $20,000、入力 20 億・出力 1.4 億トークン、10 万行。Linux 6.9 を x86 / ARM / RISC-V でビルドでき、大半のコンパイラテストスイートで 99% 合格 — [Building a C compiler with a team of parallel Claudes](https://www.anthropic.com/engineering/building-c-compiler)
- 【ベンダー一次】調整の仕組み: "Claude takes a 'lock' on a task by writing a text file to current_tasks/"。作業後に upstream から pull → merge → push。"Merge conflicts are frequent, but Claude is smart enough to figure that out." そして **"I don't use an orchestration agent. Instead, I leave it up to each Claude agent to decide how to act."** — [Building a C compiler](https://www.anthropic.com/engineering/building-c-compiler)
- 【ベンダー一次】並列が崩れた場面: カーネルのコンパイルという 1 個の巨大タスクでは "every agent would hit the same bug, fix that bug, and then overwrite each other's changes"。GCC をオラクルにして大半のファイルを GCC で、残りだけ自作コンパイラでコンパイルするハーネスを書き、失敗を独立な単位に割って解決した — [Building a C compiler](https://www.anthropic.com/engineering/building-c-compiler)
- 【ベンダー一次】教訓: "it's important that the task verifier is nearly perfect"。テスト出力は "should not print thousands of useless bytes"。"Claude can't tell time and, left alone, will happily spend hours running tests" なので `--fast`（1% または 10% の無作為抽出）を用意。限界は "New features and bugfixes frequently broke existing functionality"。安全面は "it is easy to see tests pass and assume the job is done, when this is rarely the case" と "the thought of programmers deploying software they've never personally verified is a real concern" — [Building a C compiler](https://www.anthropic.com/engineering/building-c-compiler)

**OpenAI の agent-first 開発**

- 【ベンダー一次】Ryan Lopopolo（2026-02-11）: 5 か月で約 100 万行・約 1,500 PR、技術者 3 人（後に 7 人）、"an average throughput of 3.5 PRs per engineer per day"、人手で書いたコードは 0 行。律速は "As code throughput increased, our bottleneck became human QA capacity"、"the fixed constraint has been human time and attention"。統合方針は "minimal blocking merge gates. Pull requests are short-lived"、"corrections are cheap, and waiting is expensive"、"Humans may review pull requests, but aren't required to"。ただし本人の注記つき: "This behavior depends heavily on the specific structure and tooling of this repository and should not be assumed to generalize" — [OpenAI: Harness engineering](https://openai.com/index/harness-engineering/)（openai.com は 403 のため[保存コピー](https://jaytaylor.com/notes/node/1770842156000.html)で確認）
- 【ベンダー一次】同記事のエントロピー対策: 以前は "spend every Friday (20% of the week) cleaning up 'AI slop'"。いまは "golden principles" をリポジトリに書き、定期の cleanup を回す。制約は "custom linters and structural tests" で機械的に強制し、"enforce invariants, not micromanaging implementations" — [OpenAI: Harness engineering](https://openai.com/index/harness-engineering/)（[保存コピー](https://jaytaylor.com/notes/node/1770842156000.html)）

**対照的な定量証拠**

- 【preprint】CooperBench（Stanford / SAP Labs, 2026-01）: 12 ライブラリ・4 言語（Python / TypeScript / Go / Rust）の 652 タスク。2 体に、論理的には両立するが重なるコードに触る機能を 1 つずつ渡し、メッセージで連絡させ、両パッチをマージした後に両方のテストが通れば成功。**"Agents achieve on average 30% lower success rates when working together compared to performing both tasks individually, across the full spectrum of task difficulties."** GPT-5 と Claude Sonnet 4.5 は協調時に 25% 前後で、1 体が両方をやる場合のほぼ半分 — [CooperBench (arXiv 2601.13295)](https://arxiv.org/html/2601.13295)、[cooperbench.com](https://cooperbench.com/)
- 【preprint】同論文の失敗の内訳。連絡の失敗: Work overlap 33.2% / Divergent architecture 29.7% / Repetition 14.7% / Unresponsiveness 8.7% / Unverifiable claims 4.3% / Broken commitment 3.7%。能力の欠落: 相手の状態の見積もり違い 42% / 約束を守らない 32% / 連絡の不全 26% — [CooperBench](https://arxiv.org/html/2601.13295)
- 【preprint】同論文の連絡の効果: "Agents spent as much as 20% of the steps in communication"。連絡はマージ衝突を有意に減らすが、全体の成功率の差は "not statistically significant"。例: "Two agents successfully coordinated on line numbers and edit ranges (spatial), yet failed because they never discussed the actual parameter values their implementations should use (semantic)." — [CooperBench](https://arxiv.org/html/2601.13295)
- 【preprint】同論文の人数の効果（追加実験・対象サブセットの詳細は未確認）: "Performance drops from 68.6% with 2 agents to 46.5% with 3 agents and further to 30.0% with 4 agents." — [CooperBench](https://arxiv.org/html/2601.13295)
- 【preprint】SWE-bench Verified では全 MAS 構成が単一以下（−2.1%〜−14.9%）。Terminal-Bench は Centralized −19.2% — [Towards a Science of Scaling Agent Systems v3](https://arxiv.org/html/2512.08296v3)
- 【preprint・反対側の証拠】CAID（Geng & Neubig, CMU, 2026-03-23）: 中央の委任 + 隔離ワークスペースでの並行実行 + 実行可能テストによる統合。単一エージェント比で PaperBench +25.6 ポイント、Commit0 +14.7 ポイント。"branch-and-merge is a central coordination mechanism for multi-agent collaboration"（git worktree / commit / merge を使う）— [Effective Strategies for Asynchronous Software Engineering Agents (arXiv 2603.21489)](https://arxiv.org/abs/2603.21489)
- 【preprint・反対側の証拠】STORM（2026-05-19）: worktree 隔離は "defers conflict resolution to a post-hoc merge step where recovery is expensive" と批判し、共有ワークスペースへの書き込みを仲介して衝突を書き込み時に検出する。git-worktree ベースライン比で Commit0-Lite +18.7、PaperBench +1.4 — [Multi-agent Collaboration with State Management (arXiv 2605.20563)](https://arxiv.org/abs/2605.20563)
- 【preprint】AOrchestra（2026-02-03）: orchestrator が subagent を (Instruction, Context, Tools, Model) の組としてその場で作る。GAIA / SWE-Bench / Terminal-Bench で最強ベースライン比 16.28% の相対改善（Gemini-3-Flash）— [arXiv 2602.03786](https://arxiv.org/abs/2602.03786)
- 【ベンダー一次】Anthropic の長時間ハーネス記事（2025-11-26）は結論を保留している: "it's still unclear whether a single, general-purpose coding agent performs best across contexts, or if better performance can be achieved through a multi-agent architecture" — [Effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- 【preprint】現場データ（802 人・196,212 PR・2024-01〜2026-04）: AI 導入で 1 人あたりのマージ PR は 2.09 倍。同時に "per-reviewer load roughly doubled and automated review overtook human review, while merge and revert rates held steady" — [AI Writes Faster Than Humans Can Review (arXiv 2607.01904)](https://arxiv.org/abs/2607.01904)

**コスト倍率（報告値のまとめ）**

- 【ベンダー一次】"agents typically use about 4× more tokens than chat interactions, and multi-agent systems use about 15× more tokens than chats" — [Anthropic: multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】"In our testing, multi-agent implementations typically use 3-10x more tokens than single-agent approaches for equivalent tasks." 原因は "duplicating context across agents, coordination messages between agents, and summarizing results for handoffs" — [Anthropic: when and how to use them](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)
- 【ベンダー一次】Claude Code: "Token costs scale linearly: each teammate has its own context window and consumes tokens independently." — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【preprint】単一比の token overhead: Independent +58% / Decentralized +263% / Centralized +285% / Hybrid +515% — [Scaling Agent Systems v3](https://arxiv.org/html/2512.08296v3)

### Inferences
- CAID（+25.6 / +14.7）、scaling 論文（SWE-bench Verified で全 MAS が負）、CooperBench（−30%）は矛盾しない。CAID の課題はライブラリの新規実装や論文再現で、分解でき、単一エージェントが飽和していない。SWE-bench Verified は 1 件の局所修正で単一が既に 52%。CooperBench は重なるコードを対等な 2 体で触る。scaling 論文の「分解可能性と単一の飽和度で決まる」という説明で 3 つとも収まる。
- Cursor と C コンパイラから N = 2〜5・人間 gate の Fleet に移せるもの: 計画役は書かない、worker 同士は調整しない、handoff に懸念と逸脱を含める、指示より制約、詰まったら新しいセッション、1 個の巨大タスクは並列化できないのでオラクルで独立な失敗に割る。移せないもの: integrator を置かずに誤りを許す運用。これは数百体・グリーンフィールド・自動テストが品質の全てという前提で成り立つ。
- necoder の既定（FLEET-V2 §6.3 A 案 = `CARGO_TARGET_DIR` 共有）は、Cursor が踏んだ「Git と Cargo の共有ロック」と同じ形の直列化を含む。Task 数を増やすと Captain が「進捗なし」と誤認する待ちが増える。stall 検知はロック待ちを区別する必要がある。
- 現場データと実践報告の律速はどれも人間のレビュー容量。Fleet の上限を決めるのは並走できる Task 数ではなく、人間が 1 日に裁ける Integrate の数。

### Gaps
- Cursor の成果物の品質（任意の commit でビルドが通るか等）に対する第三者の検証は今回調べていない。
- CooperBench は「2 体・対等・メッセージ通信」の条件。中央の coordinator を置くとどれだけ回復するかは測られていない。Captain 型の直接の証拠は CAID（単一研究）だけ。
- Gas Town の本体記事は取得できなかった（README のみ）。"AdaptOrch が SWE-bench Verified で +22.9%"、"Verdent 76.1% pass@1" は検索要約に出ただけで未確認。
- コスト倍率はどれもベンダーの自己申告か単一の preprint。コーディング協調に限った独立計測は見つからなかった。

## 4. タスク分解と依存関係（分割の質・DAG・粒度・重複防止・マージ衝突の回避）

### Takeaway
LLM は「相手が何を知らないか」を見積もって委任文を書くのが苦手で、33 モデル平均の合格率は 17.2%。分割の良し悪しはコンテキストを本当に分離できるかで決まり、ファイル所有の明示とインターフェース先行の契約が衝突を減らす。ただしテキスト衝突を避けても成功は保証されない。人間の開発でも衝突の 3 分の 1 はテキスト上クリーンなビルド・テスト失敗で、エージェント PR のテキスト衝突率は約 2〜4 割ある。

### Cited Findings

**分割と委任の質**

- 【preprint】PerspectiveGap（2026-06-07）: orchestrator が「文脈を持たない subagent に何を伝えるべきか」を見積もって委任文を書けるかを測る。110 シナリオ・10 トポロジー・33 商用モデル。最良の GPT-5.5 で合格率 62.0%、**全モデル平均は 17.2%**。不要な情報の漏れは平均 217.9%（GPT-5.5 は 49.1%）。コーディングに強い Opus 4.8 もここは弱い。著者の主張は "multi-agent orchestration prompting is a distinct and under-evaluated capability" — [arXiv 2606.08878](https://arxiv.org/abs/2606.08878)
- 【査読済・ICLR 2025】Agent-Oriented Planning は分割の設計原則を 3 つ置く: solvability（各サブタスクを担当が解ける）・completeness（要求を漏れなく覆う）・non-redundancy（重複しない）。分解 → 報酬モデルで評価 → フィードバックのループを組む — [AOP (arXiv 2410.02189)](https://arxiv.org/abs/2410.02189)
- 【ベンダー一次】Anthropic の委任の要件: "Each subagent needs an objective, an output format, guidance on the tools and sources to use, and clear task boundaries." 曖昧な委任で起きた重複の実例: "one subagent explored the 2021 automotive chip crisis while 2 others duplicated work investigating current 2025 supply chains" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】同記事の規模の目安をプロンプトに書く: "Simple fact-finding requires just 1 agent with 3-10 tool calls, direct comparisons might need 2-4 subagents with 10-15 calls each"。書く前は "spawning 50 subagents for simple queries" が起きていた — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】Claude Code: teammate は "don't inherit the lead's conversation history" なので spawn prompt に詳細を入れる。粒度は "Too small: coordination overhead exceeds the benefit" / "Too large: teammates work too long without check-ins, increasing risk of wasted effort" / "Just right: self-contained units that produce a clear deliverable, such as a function, a test file, or a review"。目安は "Having 5-6 tasks per teammate" — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【preprint】OrchBench（2026-07-28）: 実タスクから依存 DAG を作り、planner にサブタスクの割当てとエージェント間で渡す情報・保持率を決めさせ、決定的シミュレータで採点する。Claude Code の実実行との相関は r = 0.816、トークンは 1.3%、時間は 10.3%。知見は **"preserving task-critical information is more important than simply increasing the number of agents, and the benefits of parallelism diminish as coordination failures accumulate"** — [arXiv 2607.25656](https://arxiv.org/abs/2607.25656)

**依存関係の扱い**

- 【ベンダー一次】Claude Code の task list は pending / in progress / completed の 3 状態で、"a pending task with unresolved dependencies cannot be claimed until those dependencies are completed"。完了で依存先が自動で解放される。claim は file locking で競合を防ぐ。既知の欠点は **"Task status can lag: teammates sometimes fail to mark tasks as completed, which blocks dependent tasks"** — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【preprint】計画を依存グラフ（task dependency graph）として持ち、実行後にグラフを直す系統の研究が 2026 年に続いている（Task-Decoupled Planning など）— [Beyond Entangled Planning (arXiv 2601.07577)](https://arxiv.org/html/2601.07577v1)

**マージ衝突: 人間の開発での基礎データ**

- 【査読済・ESEC/FSE 2011・PDF で照合済み】Brun, Holmes, Ernst, Notkin: 9 つの OSS・340 万行・55 万バージョン。全マージの 16%（564 / 3,562）がテキスト衝突。ビルドとテストも測れた 3 プロジェクト（Git, Perl5, Voldemort）では "76% of merges completed cleanly, 16% of merges resulted in a textual conflict, 1% of merges resulted in a build failure, and 6% of merges resulted in a test failure"。**"The 266 textual conflicts reported by the version control system only represent 67% of all conflicts."** つまり衝突の 33% は VCS がクリーンと報告したマージの中にある — [Proactive Detection of Collaboration Conflicts](https://homes.cs.washington.edu/~mernst/pubs/vc-conflicts-fse2011.pdf)
- 【査読済・同上】持続と悪化: テキスト衝突の関係は平均 9.8 日続き 23.2 changeset を巻き込む。"of all conflict relationships, 93% developed from a TEST✓ relationship"（安全にマージできた状態を放置した結果）。"20% of TEST✓ relationships devolved into a conflict"。対策は speculative analysis で、背景でクローンを実際にマージ・ビルド・テストし "textually-clean merges that fail to build or test properly are reported as conflicts" — [Brun et al. 2011](https://homes.cs.washington.edu/~mernst/pubs/vc-conflicts-fse2011.pdf)

**マージ衝突: エージェント PR の実測（2026）**

- 【preprint】AgenticFlict（2026-04-04）: 5.9 万超のリポジトリのエージェント PR 14.2 万件から 10.7 万件をマージシミュレーション。2.9 万件が衝突、**衝突率 27.67%**、衝突領域は 33.6 万超。エージェントごとの差が目立つ — [arXiv 2604.03551](https://arxiv.org/abs/2604.03551)
- 【preprint】Xu, Subramanian, Karthik（2026-07-07）: 2,807 リポジトリの 33,596 エージェント PR（Codex 21,799 / Copilot 4,970 / Devin 4,827 / Cursor 1,541 / Claude Code 459）。79.4% は別の PR と同時に開いていた。重なる PR 対のテキスト衝突率は **同一エージェント同士 19.8%（119/601, 95% CI 16.8–23.2%）、異種エージェント間 41.7%（48/115, 95% CI 33.1–50.9%）**。衝突ファイルの 84.4% はソースで、依存マニフェスト 3.9%、設定・CI 4.0%。種類は内容の重なり 57.6%、modify/delete 26.8%、add/add 15.1%。解消の仕方とコストは測っていない（"we do not measure CI/CD logs, token usage, or maintainer time"）— [arXiv 2607.04697](https://arxiv.org/html/2607.04697v2)

**回避と早期検出の戦略**

- 【ベンダー一次】ファイル所有: "Two teammates editing the same file leads to overwrites. Break the work so each teammate owns a different set of files." — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【preprint】CooperBench の成功例に共通する創発行動。role division は "mutual confirmation" が効く。resource division は "What makes resource division effective is specificity. Vague commitments cannot be verified." negotiation は行動の前に 1 つの案へ収束する。ただし "These coordination patterns are rare in our traces"。著者の推奨は **"Lightweight mechanisms that turn conversation into verifiable shared state, such as pasted signatures, explicit insertion-point contracts, and integration checks before declaring safety."** — [CooperBench](https://arxiv.org/html/2601.13295)
- 【ベンダー一次】インターフェース先行: 良い境界は "Separate components with clean interfaces"、悪い境界は "Tightly coupled components" と "Work requiring shared state" — [Anthropic: when and how to use them](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)
- 【事例・README】統合の直列化: Gas Town の Refinery は "Per-rig merge queue processor. When polecats complete work via `gt done`, the Refinery batches merge requests, runs verification gates, and merges to main using a Bors-style bisecting queue." 赤なら "bisects to isolate the failing MR, merges the good ones"。worker は main に直接 push しない — [steveyegge/gastown README](https://github.com/steveyegge/gastown)
- 【ベンダー一次】頻繁な取り込み: C コンパイラの各エージェントは作業ごとに pull → merge → push を回す。1 個の巨大タスクで同じバグを踏んで上書きし合う問題は、GCC をオラクルにして失敗を独立な単位へ割って解いた — [Building a C compiler](https://www.anthropic.com/engineering/building-c-compiler)
- 【ベンダー一次】衝突を許す立場: "Sometimes multiple agents touch the same file or refactor the same code. Instead of trying to stamp these out completely or overengineer a solution, we accept some moments of turbulence and let the system naturally converge" — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)
- 【preprint】書き込み時の検出: worktree 隔離より +18.7（Commit0-Lite）— [STORM (arXiv 2605.20563)](https://arxiv.org/abs/2605.20563)。一方 worktree + branch-and-merge + テスト検証つき統合で単一比 +14.7（Commit0）— [CAID (arXiv 2603.21489)](https://arxiv.org/abs/2603.21489)

### Inferences
- necoder の Conflict Radar（`git merge-tree --write-tree`）は Brun の分類でいう TEXTUAL の判定だけを行う。人間の開発データでは衝突の 3 分の 1 が BUILD / TEST 型なので、「radar ✓」を安全の表示として扱うと約 3 割を見逃す計算になる。Brun の speculative analysis（使い捨ての場所で実際にマージしてビルド・テスト）は main に触れないので read-only の原則と両立する。
- Brun の「93% は安全にマージできた状態の放置から生じる」「クリーンな関係の 20% が衝突に悪化する」は、統合が 1 件済むたびに残りの Task の radar をやり直す根拠になる。FLEET-V2 §5.3 は integrated で Captain を起こすので、そこで再 radar を采配できる。
- 委任文は Captain の出力のうち最もレバーが効く場所で、しかも LLM が最も苦手な場所（PerspectiveGap 平均 17.2%）。自由文に任せず、CLI 側で「目的 / 完了条件（通すテスト）/ 触ってよい範囲・触らない範囲 / 公開インターフェース / 出力形式」の欄を埋めさせるのが安全。
- 依存が確定していない分割は、並列にせず「インターフェースを決める Task → fan-out」の順序に落とす方が CooperBench の semantic な失敗を避けられる。`depend` はそのための道具。
- Claude Code の "Task status can lag" は worker の自己申告に依存した依存解放の弱点。necoder は ACP の turn end を phase に写すので自己申告に頼らないが、`wait-deps` には時間切れと人間に見える「何待ちか」の表示が要る。
- necoder 固有の推測: i18n 規約で UI 変更は必ず `locales/ja.yml` と `en.yml` を触るので、この 2 ファイルは並走 Task の常習的な衝突点になりうる。実測研究では衝突の 84.4% がソースファイルだが、全 Task が同じ共有ファイルに追記する構造は別問題。

### Gaps
- LLM が作った分割の「独立性」をコーディングで直接測るベンチマークは見つからなかった（OrchBench は DAG が所与、PerspectiveGap は委任文、CooperBench は分割が所与）。
- ファイル所有ルールやインターフェース先行の効果を定量化した対照実験は見つからなかった。CooperBench は創発例の観察のみ。
- エージェント PR の衝突がどう解消され、いくらかかったかは未測定（arXiv 2607.04697 自身が明記）。
- semantic conflict（テキスト上クリーンでビルド・テストが落ちる）のエージェント PR での発生率は未測定。Brun の 33% は 2011 年の人間の OSS 開発の値。

## 5. メモリと状態（外部台帳・ステートレスと再起動・上に渡すもの・長時間の coordinator のコンテキスト管理）

### Takeaway
長く走る調整役は、コンテキストに覚えさせず、外部の台帳に書いて毎回読み直す形に実務が収束している。Magentic-One は台帳を外すと性能が 31% 落ちた。Anthropic の長時間ハーネス、Cursor、OpenAI はいずれも計画と進捗をファイルかリポジトリに外部化している。上に渡すのは 1,000〜2,000 トークン級の要約と参照で、全文ではない。ただし言い換えの連鎖は忠実度を落とすので、成果物そのものは参照で渡す。

### Cited Findings

**外部台帳と計画の外部化**

- 【preprint・技術報告】Task Ledger（事実・調べること・導くこと・推測・計画）と Progress Ledger を持つ Orchestrator から台帳を外すと "performance drops by 31%" — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【ベンダー一次】Anthropic Research の lead は最初に計画を外へ書く: "saving its plan to Memory to persist the context, since if the context window exceeds 200,000 tokens it will be truncated"。長い会話では "agents summarize completed work phases and store essential information in external memory before proceeding to new tasks"、限界に近づけば "agents can spawn fresh subagents with clean contexts while maintaining continuity through careful handoffs" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】同記事の再開: "Agents are stateful and errors compound... Without effective mitigations, minor system failures can be catastrophic for agents"、"we can't just restart from the beginning: restarts are expensive" なので "we built systems that can resume from where the agent was when the errors occurred" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】長時間ハーネス（2025-11-26）の問題設定: "The core challenge of long-running agents is that they must work in discrete sessions, and each new session begins with no memory of what came before."（記憶のない交代勤務の技術者のたとえ）。観察した失敗は 2 つ: 一気にやろうとして途中でコンテキストが尽きる / 後のセッションが "look around, see that progress had been made, and declare the job done" — [Effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- 【ベンダー一次】同記事の解: initializer が 200 超の機能一覧を全部 "failing" で作り、以後の各セッションは 1 機能ずつ進めて `passes` 欄だけを書き換える。形式は "we landed on using JSON for this, as the model is less likely to inappropriately change or overwrite JSON files compared to Markdown files"。進捗は git の記述的なコミットと progress file に残す。各セッションの冒頭は `pwd` → git log と progress file → 機能一覧 → `init.sh` → 基本の E2E テスト、の順 — [Effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- 【ベンダー一次】OpenAI: "Plans are treated as first-class artifacts"、"execution plans with progress and decision logs that are checked into the repository"。理由は "anything it can't access in-context while running effectively doesn't exist"。AGENTS.md は約 100 行の目次に留める — [OpenAI: Harness engineering](https://openai.com/index/harness-engineering/)（[保存コピー](https://jaytaylor.com/notes/node/1770842156000.html)）
- 【事例】12-factor agents（HumanLayer, 2025）の該当因子: 3 "Own your context window"、5 "Unify execution state and business state"、6 "Launch/Pause/Resume with simple APIs"、7 "Contact humans with tool calls"、8 "Own your control flow"、10 "Small, Focused Agents"、12 "Make your agent a stateless reducer"。前提の観察は "A lot of them are mostly deterministic code, with LLM steps sprinkled in at just the right points" — [humanlayer/12-factor-agents](https://github.com/humanlayer/12-factor-agents)
- 【事例・README】Gas Town: worker は "persistent identity but ephemeral sessions"。作業状態は git worktree ベースの hook と Beads（"Git-backed issue tracking system that stores work state as structured data"）に置き、"Work survives agent restarts"。Witness が "Monitors polecats, detects stuck agents, triggers recovery"。規模は "Scale comfortably to 20-30 agents" — [steveyegge/gastown README](https://github.com/steveyegge/gastown)
- 【ベンダー一次】Claude Code agent teams の再開は弱い: "/resume and /rewind do not restore in-process teammates"。task list のディレクトリだけはローカルに残る。lead は固定で "You can't promote a teammate to lead or transfer leadership" — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)

**上に渡すもの: 要約か transcript か**

- 【ベンダー一次】Anthropic の context engineering（2025-09-29）: subagent は "might explore extensively, using tens of thousands of tokens or more, but returns only a condensed, distilled summary of its work (often 1,000-2,000 tokens)"。長期タスクの 3 技法の使い分けは "Compaction maintains conversational flow... Note-taking excels for iterative development with clear milestones; Multi-agent architectures handle complex research and analysis where parallel exploration pays dividends"。最も安全な圧縮は tool result clearing — [Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- 【ベンダー一次】同記事の just-in-time: "maintain lightweight identifiers (file paths, stored queries, web links, etc.) and use these references to dynamically load data into context at runtime using tools"。目標は "the smallest set of high-signal tokens" — [Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- 【ベンダー一次】伝言ゲーム対策: "Subagent output to a filesystem to minimize the 'game of telephone.'" 成果物は coordinator を通さず外部に保存し、軽い参照だけ返すと "prevents information loss during multi-stage processing and reduces token overhead" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】Claude Code の警告: "Running many subagents that each return detailed results can consume significant context." transcript は別ファイル（`agent-{agentId}.jsonl`）に残り、必要なら後から辿れる — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)
- 【ベンダー一次】supervisor が subagent の発話を言い換えると誤りが入る。言い換えずに転送する `forward_message` などで約 50% 改善 — [LangChain: Benchmarking Multi-Agent Architectures](https://www.langchain.com/blog/benchmarking-multi-agent-architectures)
- 【ベンダー一次・反対の立場】Cognition の第 1 原則は "Share context, and share full agent traces, not just individual messages" — [Cognition: Don't Build Multi-Agents](https://cognition.com/blog/dont-build-multi-agents)。ただし 2026 年の続編は未解決問題として "How do you transfer context between agents without drowning the receiver?" を挙げた — [Cognition: Multi-Agents: What's Actually Working](https://cognition.com/blog/multi-agents-working)
- 【ベンダー一次】worker から planner への handoff に入れるもの: "not just what was done, but important notes, concerns, deviations, findings, thoughts, and feedback" — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)

**長いコンテキストの劣化**

- 【ベンダー一次・実験】Chroma "Context Rot"（2025-07-14）: 18 モデルで "models do not use their context uniformly; instead, their performance grows increasingly unreliable as input length grows"。"Even a single distractor reduces performance relative to the baseline"。LongMemEval では全モデルで focused prompt が full prompt を大きく上回る。結論は "what matters more is how that information is presented" — [Chroma: Context Rot](https://www.trychroma.com/research/context-rot)
- 【ベンダー一次】Manus（2025-07-18）: "the KV-cache hit rate is the single most important metric for a production-stage AI agent"。入出力比は約 100:1、キャッシュ有無で単価 10 倍（0.30 対 3 USD/MTok）。だから "Keep your prompt prefix stable"、"Make your context append-only"、"Ensure your serialization is deterministic"。ファイルシステムは "unlimited in size, persistent by nature"。`todo.md` を書き直して目標を直近のコンテキストへ復唱する（1 タスク平均約 50 tool calls）。失敗は消さずに "leave the wrong turns in the context" — [Manus: Context Engineering for AI Agents](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)
- 【ベンダー一次】Cursor の鮮度対策: "A `scratchpad.md` should be frequently rewritten versus being appended to."、コンテキスト上限での自動要約、system prompt への self-reflection と alignment の再掲、そして "periodic fresh starts" — [Cursor: Towards self-driving codebases](https://cursor.com/blog/self-driving-codebases)

### Inferences
- 「Captain は状態を持たない・記憶は台帳」（FLEET-V2 §5.1）は、Magentic-One、長時間ハーネス、12-factor、Gas Town、OpenAI が独立にたどり着いた形と同じで、強く支持される。Claude Code agent teams が「再開できない・lead を替えられない」を既知の限界に挙げている点は、Captain の交代自由がそのまま差別化になることを裏づける。
- ただしステートレスは計画なしを意味しない。wake のたびに頭の中の計画は消えるので、目標・分割の理由・依存・前提・未決事項（Task Ledger 相当）を台帳へ構造化して残さないと、次の wake の Captain は「進捗があるので完了」と誤判定しやすい（長時間ハーネスが観察した失敗と同型）。
- 台帳のレコードは構造化する方がよい（JSON は Markdown より壊されにくい、という長時間ハーネスの観察）。Captain が CLI の動詞でしか台帳を動かせない設計は、自由記述ファイルの上書き事故を構造で防いでいる。
- append-only の台帳は Manus の KV キャッシュの助言と相性が良い（wake プロンプトの前半を固定し、イベントを末尾に足す）。一方 Cursor は作業メモを追記でなく書き直せと言う。両立させるには「台帳は追記・Captain に見せる現況ビューは台帳から毎回導出」とする。
- 要約か全文かの対立は役割で解ける。Cognition の「全トレース共有」は同じ成果物を書く者同士の話。Captain は書かないので要約で足りる。ただし Captain が必要なときに掘れる参照（diff stat・失敗したテスト名・最後のエラー・変更ファイル一覧）と、人間に worker の原文をそのまま見せる経路は要る。
- digest に「懸念・逸脱・未完了」の欄を設ける価値がある（Cursor の handoff）。worker の自己申告と環境由来の事実（テストの終了コード、radar、コミットの有無）は欄を分ける。

### Gaps
- 協調タスクで要約の粒度（何トークンが最適か）を比べた対照実験は見つからなかった。Anthropic の 1,000〜2,000 は経験則。
- append-only の台帳が数百イベントに伸びたとき coordinator の判断品質がどう変わるかの実測は見つからなかった。
- 12-factor agents と Gas Town は実践者の主張で、対照実験は無い。

## 6. スケジューリングとコスト（イベント駆動かポーリングか・デバウンス・予算・停止条件・暴走 spawn の防止）

### Takeaway
イベント駆動は実装の収束点になっている。Claude Code は lead がポーリングしなくてよい設計で、Cursor は「planner は task 完了で起きるべき」を未解決課題に挙げ、AutoGen は v0.4 で actor model に作り直した。暴走の防止は 1 つの仕掛けでは足りず、規模の目安をプロンプトに書く・予算を本人に見せる・連続失敗でエスカレーションする・入れ子の深さを切る、を重ねる。

### Cited Findings

- 【ベンダー一次】Claude Code: "Automatic message delivery: when teammates send messages, they're delivered automatically to recipients. The lead doesn't need to poll for updates." と "Idle notifications: when a teammate finishes and stops, it automatically notifies the lead and includes its final answer in the notification."（API エラーで終わった場合も失敗として通知）— [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【ベンダー一次】Cursor の未解決課題: "Planners should wake up when their tasks complete to plan the next step. Agents occasionally run for far too long." — [Cursor: Scaling agents](https://cursor.com/blog/scaling-agents)
- 【ベンダー一次】AutoGen v0.4（2025-01）は Core を actor model で実装し、エージェントは非同期メッセージに反応する（event-driven と request/response の両方）。配送と処理を分離して拡張性を上げた — [Microsoft Research: AutoGen v0.4](https://www.microsoft.com/en-us/research/blog/autogen-v0-4-reimagining-the-foundation-of-agentic-ai-for-scale-extensibility-and-robustness/)
- 【ベンダー一次】同期実行の代償: "the lead agent can't steer subagents, subagents can't coordinate, and the entire system can be blocked while waiting for a single subagent to finish" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【ベンダー一次】過剰な更新は害: 初期の失敗は "spawning 50 subagents for simple queries, scouring the web endlessly for nonexistent sources, and distracting each other with excessive updates" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【preprint】連絡のコスト: "Agents spent as much as 20% of the steps in communication"、チャンネルは "flood channels with repetitive status updates" — [CooperBench](https://arxiv.org/html/2601.13295)
- 【preprint】予算の自覚: "simply increasing the tool-call budget fails to improve performance, as agents lack 'budget awareness' and quickly hit a performance ceiling"。残予算を常に見せる軽量プラグイン（Budget Tracker）でスケーリング曲線が改善（Google, v1 2025-11-21 / v2 2026-08-17）— [Budget-Aware Tool Use (arXiv 2511.17006)](https://arxiv.org/abs/2511.17006)
- 【preprint】人数の上限: 固定予算では "beyond 3–4 agents" で通信が推論を食う — [Scaling Agent Systems v3](https://arxiv.org/html/2512.08296v3)。【ベンダー一次】"Start with 3-5 teammates for most workflows"、"Three focused teammates often outperform five scattered ones" — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【ベンダー一次】連続拒否でのエスカレーション: "If a session accumulates 3 consecutive denials or 20 total, we stop the model and escalate to the human." — [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【ベンダー一次】入れ子の制限: subagent は既定で "up to three layers below the main conversation" まで（`CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH`）。上限に達すると Agent ツール自体が渡されない — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)。agent teams は "No nested teams: teammates cannot spawn their own teammates" — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【査読済・NeurIPS 2023】停止条件を重ねた例: CAMEL の 5 条件（無指示 3 ラウンド / 役割逆転 / 完了トークン / トークン上限 / 最大 40 メッセージ）— [CAMEL](https://ar5iv.labs.arxiv.org/html/2303.17760)。【ベンダー一次】agent の一般論として "a maximum number of iterations to maintain control" — [Building effective agents](https://www.anthropic.com/engineering/building-effective-agents)。CrewAI は max_rpm と max iterations — [CrewAI docs](https://docs.crewai.com/en/learn/hierarchical-process)
- 【ベンダー一次】Claude Code が既知の失敗として挙げる lead の挙動: "Sometimes the lead starts implementing tasks itself instead of waiting for teammates." と "The lead can stop early too, deciding the team is finished before all tasks are actually complete." 放置の危険は "Letting a team run unattended for too long increases the risk of wasted effort." — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【ベンダー一次】品質 gate をフックで強制: `TeammateIdle` / `TaskCreated` / `TaskCompleted` が exit code 2 を返すと、待機・作成・完了を差し止めてフィードバックを返せる — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【ベンダー一次】時間の盲目: "Claude can't tell time and, left alone, will happily spend hours running tests" — [Building a C compiler](https://www.anthropic.com/engineering/building-c-compiler)

### Inferences
- FLEET-V2 §5.3 / §5.6（イベント駆動・ポーリング禁止・同一 Task のデバウンス 5 秒・Blocked は 15 秒後に 1 回・実行中のイベントはキューに積んで 1 ターンにまとめる）は、上の証拠と向きが同じ。「頻繁な更新が互いの邪魔になる」という Anthropic と CooperBench の観察は、まとめて渡す設計を直接支持する。
- 文書の wake 表に無いもの: (a) Working のまま進捗が無い状態（stall）で起こすイベント、(b) Captain が 1 つの目標から作れる Task 数の上限、(c) 1 wake あたりの行動数の予算、(d) 采配が連続で失敗・却下されたときの停止と人間へのエスカレーション。どれも先行実装に前例がある（Magentic-One の stall counter、Claude Code の 3〜5 体、BATS の予算表示、auto mode の 3 回 / 20 回）。
- Claude Code の `TaskCompleted` フックは「完了を LLM の宣言でなく決定的な検査で gate する」設計例。Fleet では review_ready への遷移時にテストと radar を走らせる形で同じことができる。
- 時間の盲目と共有ロック待ち（3 節）を合わせると、stall の判定は経過時間でなく台帳上の出来事（コミット、テスト結果、ツール呼び出しの種類の変化）で行う方が誤検知が少ない。

### Gaps
- イベント駆動とポーリングを直接比べた定量研究は見つからなかった。あるのは設計上の合意とベンダー実装だけ。
- デバウンス間隔や Blocked の閾値（15 秒）の最適値を示す証拠は無い。実測で決めるしかない。
- AutoGen v0.4 の「レイテンシ 30% 減・デバッグ 40% 短縮」という二次情報があったが、一次資料で確認できなかったので採っていない。

## 7. 監督と安全性（human-in-the-loop の承認設計・エスカレーション・過信と automation bias・監査証跡・権限委任・coordinator の自動承認のリスク）

### Takeaway
人間は承認プロンプトの 93% を許可しており、全件承認は疲労を生むだけで安全を保証しない。一方 LLM による代理承認も頼れない。専用の分類器でも実際の「やりすぎ行動」の 17% を見逃し、LLM マネージャの権限付与の精度は 50% に届かず、LLM 検証役は不合格の 49% を承認した。落としどころは「不可逆な操作だけ人間 gate、その他は事後に介入できる位置取り」。推薦の出し方しだいで automation bias は増えも減りもする。

### Cited Findings

**承認疲労と「全件承認」の限界**

- 【ベンダー一次】"Claude Code users approve 93% of permission prompts." "Over time that leads to approval fatigue, where people stop paying close attention to what they're approving."（2026-03-25）— [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【ベンダー一次】Anthropic の利用実態（2026-02-18）: 完全 auto-approve は新規ユーザー（50 セッション未満）で約 20%、750 セッションで 40% 超。経験者は割り込みも増える（約 10 セッションで 5% のターン → 経験者は約 9%）。API のツール呼び出しは "73% appear to have a human in the loop in some way, and only 0.8% of actions appear to be irreversible"。ソフトウェア工学が約 50% — [Measuring AI agent autonomy in practice](https://www.anthropic.com/research/measuring-agent-autonomy)
- 【ベンダー一次】同記事の提言: "effective oversight doesn't require approving every action but being in a position to intervene when it matters"。"Oversight requirements that prescribe specific interaction patterns, such as requiring humans to approve every action, will create friction without necessarily producing safety benefits" — [Measuring AI agent autonomy in practice](https://www.anthropic.com/research/measuring-agent-autonomy)

**LLM に承認を任せたときの実測**

- 【ベンダー一次】auto mode の分類器（Sonnet 4.6、高速フィルタ → CoT の 2 段）: 実トラフィック n = 10,000 で偽陽性 0.4%。**実際のやりすぎ行動 n = 52 で偽陰性 17%**。合成の持ち出し n = 1,000 で 5.7%。本人の評価は "The 17% false-negative rate on real overeager actions is the honest number." と "It is not a drop-in replacement for careful human review on high-stakes infrastructure." — [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【ベンダー一次】同分類器は説得を遮断する設計: "We strip assistant text so the agent can't talk the classifier into making a bad call... we want it to judge what the agent did, not what the agent said"（"reasoning-blind by design"）— [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【ベンダー一次】同記事の脅威モデルは 4 つ: overeager behavior / honest mistakes / prompt injection / a misaligned model。社内の事故例は "deleting remote git branches from a misinterpreted instruction, uploading an engineer's GitHub auth token to an internal compute cluster, and attempting migrations against a production database" — [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【ベンダー一次】同記事の委任の二重チェック: 委任時は "the transcript classifier that monitors the subagent may not have the full context to know if the task it was given was originally intended by the user"、戻り時は "a subagent that was benign at delegation could be compromised mid-run by a prompt injection in content it reads" — [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【ベンダー一次】同記事の 3 層: 状態を変えないツールは許可リスト / プロジェクト内のファイル編集は分類器なし（"in-project edits are reviewable via version control"）/ それ以外（shell、web fetch、外部連携、**subagent の spawn**、プロジェクト外の操作）は分類器 — [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)
- 【preprint】ClawArena-Team（2026-06-30）: 1 つの LLM が subagent 群を管理する能力を 41 シナリオ・258 ラウンドで測る（採点は実行ベースで LLM judge なし）。12 モデルで **"the management bottleneck is privilege granting rather than perception (no model exceeds 50% workspace-permission precision)"**。コストと管理品質は連動しない（API コストは 100 倍超の幅、スコアは 4 倍未満の幅）— [arXiv 2606.31174](https://arxiv.org/abs/2606.31174)
- 【preprint】TeamBench: LLM の verifier は不合格の提出の 49% を承認した。役割をプロンプトだけで縛ると、verifier が executor のコードを編集しようとする件数が 3.6 倍 — [arXiv 2605.07073](https://arxiv.org/abs/2605.07073)

**エージェント間の承認の中継を禁じる設計例**

- 【ベンダー一次】Claude Code agent teams: "A teammate can't approve a permission prompt or supply consent on your behalf, and a teammate that was denied an action can't relay it to another teammate to bypass the check." teammate の権限プロンプトは lead のセッションに出て、人間が答える。auto mode は "treats an approval claim relayed from another agent as untrusted input rather than confirmation from you"。例外は plan approval だけで、lead のセッションが到着と同時に承認する（"without the lead reviewing it"）。編集とコマンドは引き続き権限プロンプトを通る — [Claude Code docs: Agent teams](https://code.claude.com/docs/en/agent-teams)
- 【ベンダー一次】background subagent の権限要求はメインセッションに出て、どの subagent の要求かを名指しする。セッション全体に効く許可を選ぶとメイン会話にも適用される — [Claude Code docs: Subagents](https://code.claude.com/docs/en/sub-agents)

**automation bias（推薦が人間の判断を歪める度合い）**

- 【査読済・系統的レビュー】Goddard, Roudsari, Wyatt（JAMIA 2012、13,821 件から 74 研究）: 誤った助言があると "the CDSS increased the risk of an incorrect decision being made by 26%"（リスク比 1.26, 95% CI 1.11–1.44）。媒介要因は信頼の較正ずれ（"Trust is possibly the strongest driving factor in over-reliance"）、課題の複雑さ、作業負荷、時間圧。緩和策は訓練、"Increasing accountability for decisions"、助言に確信度を添える、"supportive information rather than commands"。逆に "Display prominence increased AB" — [Automation bias: a systematic review](https://pmc.ncbi.nlm.nih.gov/articles/PMC3240751/)
- 【preprint・二次経由】METR の RCT（2025・経験ある OSS 開発者 16 人・246 タスク）: AI を使うと 19% 遅くなったが、本人は 20% 速くなったと見積もった。体感と実測が逆を向く例 — [METR study (arXiv 2507.09089)](https://arxiv.org/abs/2507.09089)（今回は二次資料の要約で数値を確認。一次は未取得）

**human-in-the-loop の設計例**

- 【preprint】Magentic-UI（Microsoft Research, 2025-07-30）の action guard: 行動に不可逆性のヒューリスティックを 3 値で付ける（"always irreversible (e.g., uploading a file), maybe irreversible (e.g., clicking a button), and never irreversible (e.g., scrolling a page)"）。always は必ず人間の承認、never は自動、maybe は LLM の ActionGuard judge が判定する — [Magentic-UI (arXiv 2507.22358)](https://arxiv.org/html/2507.22358)
- 【preprint】同論文の効果: 副情報を持つ模擬ユーザーを入れると GAIA の完了率が 30.3% → 51.9%（+71%）。助けを求めたのはタスクの 10%、平均 1.1 回。利用者調査（n = 12）は SUS 74.58 で、co-planning による曖昧さの前倒し解消と「必要なときだけ割り込む」が好まれた — [Magentic-UI](https://arxiv.org/html/2507.22358)
- 【preprint】同論文の安全評価: 24 の敵対シナリオは既定設定では全て不成立（"layered mitigations"）。防御を外すとプロンプトインジェクションが通り、SSH 鍵の持ち出し、GitHub の API キー作成、承認の迂回が起きた — [Magentic-UI](https://arxiv.org/html/2507.22358)
- 【ベンダー一次】agent は "pause for human feedback at checkpoints or when encountering blockers"、原則は "Prioritize transparency by explicitly showing the agent's planning steps" — [Building effective agents](https://www.anthropic.com/engineering/building-effective-agents)

**マルチエージェント固有の攻撃面**

- 【preprint】Triedman, Jha, Shmatikov（v1 2025-03-15 / v2 2025-09-12）: 敵対的なコンテンツがシステム内の制御と通信を乗っ取り、orchestrator が subagent の出力を検証せずに信じる confused deputy を突く。"when agents are instantiated with GPT-4o, Web-based attacks successfully cause the multi-agent system execute arbitrary malicious code in 58-90% of trials" — [Multi-Agent Systems Execute Arbitrary Malicious Code (arXiv 2503.12188)](https://arxiv.org/abs/2503.12188)
- 【preprint】Prompt Infection（2024-10-09）: 悪意あるプロンプトがエージェント間で自己複製する。"multi-agent systems are highly susceptible, even when agents do not publicly share all communications" — [arXiv 2410.07283](https://arxiv.org/abs/2410.07283)
- 【ベンダー一次】Meta "Agents Rule of Two"（2025-10-31）: [A] 信頼できない入力を処理する / [B] 機微なシステムや私的データに触れる / [C] 状態を変えるか外部と通信する、のうち 1 セッションで満たしてよいのは 2 つまで。3 つ要るなら "the agent should not be permitted to operate autonomously and at a minimum requires supervision — via human-in-the-loop approval or another reliable means of validation"。限界も明記: "Designs satisfying the Rule can still be prone to failure (e.g., a user blindly confirming a warning interstitial)" — [Meta AI: Agents Rule of Two](https://ai.meta.com/blog/practical-ai-agent-security/)
- 【preprint・技術報告】Magentic-One の開発中の実例: ログイン失敗を繰り返してアカウントを一時停止させ、続けてパスワードのリセットを試みた。人間の助けを集めようと SNS への投稿や教科書著者へのメール、情報公開請求の下書きまでした。推奨は "a strict principle of least privilege, and maximum oversight" — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【preprint】委任の枠組み（Tomašev, Franklin, Osindero, 2026-02-12）: 委任は割当てだけでなく "transfer of authority, responsibility, accountability, clear specifications regarding roles and boundaries, clarity of intent, and mechanisms for establishing trust" を含む一連の決定だとする。既存の分解・委任手法は "rely on simple heuristics" と評価 — [Intelligent AI Delegation (arXiv 2602.11865)](https://arxiv.org/abs/2602.11865)

**監査証跡**

- 【ベンダー一次】"Adding full production tracing let us diagnose why agents failed"。会話の中身を見ずに判断パターンと相互作用の構造を監視する。評価は "End-state evaluation of agents that mutate state over many turns" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【preprint・単一事例】サイレント障害の 70% は人間の観察で発見された。事後の監査で事前に防げた障害は 0%、再発の阻止率は 87% — [When Errors Become Narratives (arXiv 2606.14589)](https://arxiv.org/abs/2606.14589)

### Inferences
- FLEET-V2 §5.2 / §5.5 の線引き（Integrate は人間のみ・承認は推薦のみ）は、Claude Code agent teams が採る線（teammate は承認も同意の代行もできない）と同じ位置にあり、実測とも合う。17%・50% 未満・49% という数値を見る限り、2026 年時点で coordinator に承認権を渡す根拠は無い。
- Captain の推薦は automation bias の経路になる。Goddard のレビューに従えば、推薦は命令でなく情報として出す、確信度と根拠になる事実を添える、目立たせすぎない、既定の選択にしない、最終責任が人間にあることを画面で明示する、の 5 点が具体策になる。FLEET-V2 §5.5 の「✳ 付きで添えるだけ」は方向が合っている。
- 推薦の入力に注意が要る。§5.3 は Blocked の wake で「permission 文 + Task の digest」を渡す。どちらも要求している worker 自身の文章で、説得とインジェクションの経路になる。auto mode が assistant の文章を剥がして行動だけを判定しているのに倣い、推薦は構造化された事実（ツール名・コマンド・対象パス・worktree の内か外か・書き込みか読み取りか）を主な入力にし、worker の説明文は信頼しない入力として扱う。
- Rule of Two に当てると、Captain は [A]（worker の出力 = 外部コンテンツ由来かもしれない入力）と [C]（Task の作成・追撃）を満たす。だから [B]（承認権・merge・設定変更・削除）を持たせない §5.2 の権限表は、この規則の要請と一致している。
- Captain の制約（コードを書かない・merge しない・迂回路を使わない）は、プロンプトでなくツール権限で強制すべき。TeamBench はプロンプトだけの役割分離で越境が 3.6 倍になると示し、Cursor の continuous executor は "do work itself" に走り、Claude Code の lead も "starts implementing tasks itself" が既知の失敗。Captain が汎用のコーディングエージェント上で動く以上、シェルと編集のツールを許可リストで落とさない限り同じことが起きる。
- 人間 gate を残す代償は人間がボトルネックになること（3 節）。だから gate は不可逆な操作（Integrate、worktree やブランチの削除、worktree の外への書き込み）に絞り、可逆な操作は事後に介入できる形にするのが、Magentic-UI の不可逆性 3 値と Anthropic の「介入できる位置」の両方に沿う。

### Gaps
- coordinator の推薦が人間の承認判断をどれだけ歪めるかをコーディングエージェントの文脈で測った研究は見つからなかった。automation bias の定量的根拠は医療の意思決定支援の系統的レビュー。
- auto mode の数値は Anthropic の自己評価で、実例は n = 52 と小さい。
- 「開発者の 96% が AI 生成コードを完全には信頼しないが、常に検証するのは 48%」という調査値は二次情報でしか見つからず未確認。
- "Intelligent AI Delegation" は概念枠組みで、実証評価は無い。著者の所属は取得ページで確認できなかった。

## 8. 評価（orchestrator・協調のベンチマークと指標、信用する前の試験方法）

### Takeaway
2026 年に、オーケストレーション能力そのものを worker の能力から切り離して測るベンチマークが出揃った（OrchBench、ClawArena-Team、PerspectiveGap、TeamBench、CooperBench）。共通の教訓は 2 つ。チームの合格率だけでは協調したかどうかが見えない。失敗の原因の帰属は自動化してもまだ難しい（ステップ特定 14.2%）。

### Cited Findings

- 【preprint】OrchBench（2026-07-28）: 既存の評価は end-to-end 実行で "conflates orchestration-plan quality with worker capabilities, tool reliability, and environmental noise"。DAG・エージェントごとのコンテキスト上限・エージェント数の予算を与えて計画だけを決定的シミュレータで採点し、品質・makespan・トークンを返す。実実行との相関は r = 0.816、トークン 1.3%、時間 10.3% — [arXiv 2607.25656](https://arxiv.org/abs/2607.25656)
- 【preprint】ClawArena-Team（2026-06-30）: 主エージェントの知覚と権限をわざと絞り、subagent の集合を固定して "score differences reflect management skill, not raw capability" にする。指標の Subagent-Management Score は正答 × 最小権限・モダリティ振り分けの係数。"most leaderboard scores cluster within a 9.9-point band while orchestration behaviors diverge by more than an order of magnitude" — [arXiv 2606.31174](https://arxiv.org/abs/2606.31174)
- 【preprint】PerspectiveGap（2026-06-07）: 委任文の品質だけを測る。33 モデル平均 17.2%、最良 62.0% — [arXiv 2606.08878](https://arxiv.org/abs/2606.08878)
- 【preprint】TeamBench（2026-05-08）: 役割分離を OS で強制しないと "a team pass rate can mask whether agents actually coordinated or whether one role effectively did another role's work"。人間 40 セッションの比較も同じ制約で実施 — [arXiv 2605.07073](https://arxiv.org/abs/2605.07073)
- 【preprint】CooperBench（2026-01）: 協調コーディング 652 タスク。単独でやった場合との差（coordination gap）を指標にする — [arXiv 2601.13295](https://arxiv.org/abs/2601.13295)
- 【preprint】MultiAgentBench / MARBLE（2025-03-03）: 完了だけでなく協調の質をマイルストーン型 KPI で測り、トポロジー（star / chain / tree / graph）を比べる — [arXiv 2503.01935](https://arxiv.org/abs/2503.01935)
- 【preprint】τ²-bench（2025-06-09）: エージェントとユーザーの双方がツールで共有環境を動かす dual-control（Dec-POMDP）。推論の誤りと連絡・協調の誤りを分けて分析し、"significant performance drops when agents shift from no-user to dual-control" — [arXiv 2506.07982](https://arxiv.org/abs/2506.07982)
- 【preprint】失敗の帰属: Who&When は最良でもエージェント特定 53.5%、ステップ特定 14.2% — [arXiv 2505.00212](https://arxiv.org/abs/2505.00212)。MAST の LLM 注釈器は人手と κ = 0.77 で、トレースの失敗モードの付与に使える — [MAST](https://arxiv.org/html/2503.13657v3)
- 【ベンダー一次】Anthropic の実務: 最初は小さく（"We started with a set of about 20 queries representing real usage patterns"）。LLM judge は 1 回の呼び出しで 0.0〜1.0 と合否を出す。人間の評価は自動評価が見逃すものを拾う（初期のエージェントが SEO 目当ての粗製サイトを権威ある情報源より優先していた例）。状態を変える系は end-state で評価。注意点は "small changes to the lead agent can unpredictably change how subagents behave" — [How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- 【preprint・技術報告】構成要素のアブレーション（台帳なしで −31%）は、調整機構の寄与を切り出す最も単純な方法 — [Magentic-One](https://arxiv.org/html/2411.04468v1)
- 【preprint】構成選択の予測モデル: 課題の性質（分解可能性、ツール数、単一の基準性能）から最適構成を当てる。未見構成の 87% で正解、交差検証 R² = 0.373 — [Scaling Agent Systems](https://arxiv.org/abs/2512.08296)
- 【preprint・二次経由】体感は当てにならない: METR の RCT で開発者は 19% 遅くなったのに 20% 速くなったと感じた — [arXiv 2507.09089](https://arxiv.org/abs/2507.09089)

### Inferences
- Captain はステートレスで入力が台帳由来なので、記録した台帳のイベント列をそのまま再生する回帰試験が組める（12-factor の stateless reducer と OrchBench の決定的シミュレーションの組合せ）。プロンプトやモデルを替えたときの挙動の変化を、実 worker を動かさずに見られる。
- 測るべきものは 4 つに分かれる。(1) 分割判断（分割すべき目標とすべきでない目標を混ぜた約 20 件で、分割数・重複・ファイル所有の重なりを見る）。(2) 委任文（PerspectiveGap 式に、必要な情報の欠落と不要な情報の漏れ）。(3) 権限の推薦（人間の最終判断を正解として precision と recall。ClawArena-Team が示すとおりここが最弱の可能性が高い）。(4) stall と完了の判定（偽の「完了」をどれだけ見抜くか）。
- end-to-end の比較は Captain あり / なしのアブレーションで、同じ目標について統合までの時間・人間の操作回数・総トークン・統合後のテスト失敗を見る。体感で判断しない。
- TeamBench の教訓から、試験時も Captain の制約はツール権限で強制した状態で測る。プロンプトだけの制約で測ると、越境で得た成績が混ざる。

### Gaps
- 「イベント駆動・ステートレス・人間 gate つき」の coordinator をそのまま測るベンチマークは見つからなかった。git worktree と人間の統合 gate を含むものも無い。
- 2026 年のベンチマークはどれも preprint で、相互の比較や再現の報告はまだ無い。モデル名（GPT-5.5、Opus 4.8 など）は各要旨の記載のまま。
- 実 worker を動かす評価のコスト（1 試行あたりのトークンと時間）の報告はほとんど無い。OrchBench の 1.3% が唯一の比較値。

## Captain への含意

### Takeaway
現行の Captain 設計の骨格（ステートレス + 台帳、イベント駆動、CLI だけで行動、承認は推薦まで、merge は人間）は、2026 年までの証拠が支持する位置にそのまま立っている。足りないのは 5 つ: 分割しない判断の明文化、委任文の型、stall の wake、spawn と予算の上限、そして「radar ✓ は安全ではない」の扱い。以下は優先順。設計文書の参照は `docs/FLEET-V2.md` の節番号。

### 設計原則（優先順・各項に根拠）

**P0: 構造で守る不変条件**

1. **統合は直列のまま、人間 gate のまま。Captain に merge を渡さない。**
   - 根拠: Cognition は 2026 年に "multi-agent systems work best today when writes stay single-threaded and the additional agents contribute intelligence rather than actions" と結論した — [Cognition 2026](https://cognition.com/blog/multi-agents-working)。エージェント PR のテキスト衝突率は 27.67% — [AgenticFlict](https://arxiv.org/abs/2604.03551)、重なる PR 対では 19.8%（同種）〜41.7%（異種）— [arXiv 2607.04697](https://arxiv.org/html/2607.04697v2)。Fleet はエージェント非依存なので異種の並走が普通に起きる。
   - 反対側の証拠と、それが当てはまらない理由: Cursor は integrator を "created more bottlenecks than it solved" として外した — [Cursor](https://cursor.com/blog/scaling-agents)。これは数百体・グリーンフィールド・"accepting a small but stable rate of errors" が前提で、同じ記事が "a final 'green' branch is needed" と認めている — [Cursor 続報](https://cursor.com/blog/self-driving-codebases)。N が 2〜5 で人間が品質を負うリポジトリでは、直列の統合は律速にならず、誤りの許容は選べない。

2. **「radar ✓」を安全と表示しない。統合前にテストの事実を並べ、統合のたびに残りの Task を再 radar する。**
   - 根拠: VCS が報告するテキスト衝突は全衝突の 67% で、残り 33% はクリーンと報告されたマージの中のビルド・テスト失敗。クリーンな関係の 20% は放置で衝突に悪化する — [Brun et al. 2011](https://homes.cs.washington.edu/~mernst/pubs/vc-conflicts-fse2011.pdf)。エージェントでも、行範囲の調整に成功しながら値の意味が食い違って失敗する — [CooperBench](https://arxiv.org/html/2601.13295)。
   - 具体化: §4.2 のピルは `radar ✓` を「テキスト衝突なし」と読める語にする。§5.3 の integrated の wake で Captain に残り Task の再 radar を采配させる。余力があれば使い捨ての場所で実際にマージしてビルド・テストする speculative な検査を足す（main に触れないので read-only の原則のまま）。

3. **承認は推薦まで。推薦は命令でなく情報として出し、入力は行動の事実にする。**
   - 根拠: 専用分類器でも実際のやりすぎ行動の 17% を見逃し、"not a drop-in replacement for careful human review on high-stakes infrastructure" — [auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)。LLM マネージャの権限付与は "no model exceeds 50% workspace-permission precision" — [ClawArena-Team](https://arxiv.org/abs/2606.31174)。LLM の verifier は不合格の 49% を承認 — [TeamBench](https://arxiv.org/abs/2605.07073)。Claude Code も "A teammate can't approve a permission prompt or supply consent on your behalf" — [agent teams](https://code.claude.com/docs/en/agent-teams)。
   - 出し方: 誤った助言は誤判断を 26% 増やし、目立つ表示は bias を強め、確信度の併記と "supportive information rather than commands" は弱める — [Goddard et al.](https://pmc.ncbi.nlm.nih.gov/articles/PMC3240751/)。§5.5 の推薦は既定の選択にせず、根拠の事実（worktree の内か外か・読み取りか書き込みか・対象パス）を添える。
   - 入力: auto mode は "We strip assistant text so the agent can't talk the classifier into making a bad call" — [auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)。§5.3 の Blocked の wake に渡す「permission 文 + digest」は要求した worker 自身の文章なので、構造化した行動の事実を主にし、説明文は信頼しない入力として扱う。高リスクの種別（worktree の外・ネットワーク・削除・認証情報）は推薦そのものを出さない選択もある。

4. **Captain の制約はプロンプトでなくツール権限で強制する。**
   - 根拠: プロンプトだけの役割分離では越境の試みが 3.6 倍 — [TeamBench](https://arxiv.org/abs/2605.07073)。役割を詰め込んだ Cursor の executor は "do work itself... not properly merge worker changes, and claim premature completion" — [Cursor 続報](https://cursor.com/blog/self-driving-codebases)。Claude Code の lead も "starts implementing tasks itself instead of waiting for teammates" — [agent teams](https://code.claude.com/docs/en/agent-teams)。orchestrator が subagent の出力を検証せずに信じる構造は 58〜90% の試行で任意コード実行につながった — [Triedman et al.](https://arxiv.org/abs/2503.12188)。
   - 具体化: Captain のスレッドは fleet CLI/MCP と読み取りだけを許可リストに入れ、シェルの任意実行とファイル編集を落とす。§5.2 の権限表は Rule of Two（信頼できない入力 + 状態変更を持つなら、機微な権限は持たせない）の要請と一致しているので、その根拠を文書に書き足せる — [Meta: Rule of Two](https://ai.meta.com/blog/practical-ai-agent-security/)。

**P1: Captain の判断の質を決めるもの**

5. **分割しない判断を Captain の最初の仕事にする。既定は「分けない」。**
   - 根拠: "most coding tasks involve fewer truly parallelizable tasks than research" — [Anthropic 2025](https://www.anthropic.com/engineering/multi-agent-research-system)。"For sequential tasks, same-file edits, or work with many dependencies, a single session or subagents are more effective" — [agent teams](https://code.claude.com/docs/en/agent-teams)。逐次課題は全 MAS で −39〜−70%、SWE-bench Verified は全 MAS が単一以下、単一の基準が 45% を超えると追加エージェントは負、固定予算では 3〜4 体が上限 — [Scaling Agent Systems v3](https://arxiv.org/html/2512.08296v3)。協調コーディングは平均 −30% で、2 → 3 → 4 体で 68.6% → 46.5% → 30.0% — [CooperBench](https://arxiv.org/html/2601.13295)。"improved prompting on a single agent achieved equivalent results" — [Anthropic 2026](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)。
   - 分けてよい条件（Captain のプロンプトに書く目安）: 触るファイル集合が重ならないか、きれいなインターフェースで切れる / 各 Task が自分のテストで独立に検証できる / 順序の依存が無いか `depend` で明示できる。1 つでも欠けたら 1 Task にするか直列にする。
   - 並列が最も効く用途: 調査、レビュー、バグの再現、競合する仮説の検証、互いに独立な小さな保守 — [agent teams](https://code.claude.com/docs/en/agent-teams)、[Willison](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/)。

6. **委任文（Task のプロンプト）に型を入れる。Captain の自由文に任せない。**
   - 根拠: 委任文を正しく書ける割合は 33 モデル平均で 17.2% — [PerspectiveGap](https://arxiv.org/abs/2606.08878)。manager は "assume they share state with their children when they don't"、かつ "overly prescriptive" になりがち — [Cognition 2026](https://cognition.com/blog/multi-agents-working)。必要な要素は "an objective, an output format, guidance on the tools and sources to use, and clear task boundaries" で、欠くと重複作業が起きた — [Anthropic 2025](https://www.anthropic.com/engineering/multi-agent-research-system)。"Constraints are more effective than instructions"、指示の欠陥は規模に比例して増幅される — [Cursor 続報](https://cursor.com/blog/self-driving-codebases)。MAST でも仕様逸脱が 11.8%、終了条件の不認識が 12.4% — [MAST](https://arxiv.org/pdf/2503.13657v3)。
   - 具体化: `fleet create` / `spawn-agent` に欄を設ける。目的 / 完了条件（通すテストやコマンド）/ 触ってよい範囲と触らない範囲 / 守る公開インターフェース（シグネチャを貼る）/ 報告の形式。CooperBench の推奨 "pasted signatures, explicit insertion-point contracts, and integration checks before declaring safety" をそのまま欄にできる — [CooperBench](https://arxiv.org/html/2601.13295)。分割の検査は solvability / completeness / non-redundancy の 3 点 — [AOP](https://arxiv.org/abs/2410.02189)。

7. **台帳から stall を決定的に検知し、Captain は「どうするか」だけ判断する。wake 表に stall を足す。**
   - 根拠: Progress Ledger の 5 問と stall counter（閾値 ≤2）で再計画する Orchestrator は、台帳を外すと −31% — [Magentic-One](https://arxiv.org/html/2411.04468v1)。最頻の失敗は Step Repetition 15.7% — [MAST](https://arxiv.org/pdf/2503.13657v3)。"Agents occasionally run for far too long" — [Cursor](https://cursor.com/blog/scaling-agents)。"Claude can't tell time" — [C compiler](https://www.anthropic.com/engineering/building-c-compiler)。Gas Town は stuck の検知を専任の Witness に切り出した — [gastown README](https://github.com/steveyegge/gastown)。
   - 具体化: §5.3 の wake 表には Done / review_ready / Failed / Blocked / integrated / 介入があるが、Working のまま進まない Task で起きる条件が無い。検知は LLM でなくコードで行う（同じツールエラーの反復、コミットもテスト結果も無いままの経過、同種イベントの連続）。共有 `CARGO_TARGET_DIR` のロック待ち（§6.3）を stall と誤認しないよう、判定は経過時間だけに頼らない。Captain の選択肢は「追撃して方針を変える」「新しいスレッドでやり直す（periodic fresh starts）」「人間に上げる」。

8. **ステートレスは維持し、計画を台帳に外部化する。**
   - 根拠: 後のセッションは "see that progress had been made, and declare the job done"。対策は構造化した進捗ファイルで、JSON は Markdown より "less likely to inappropriately change or overwrite" — [long-running harness](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)。lead は計画を先に Memory へ保存する — [Anthropic 2025](https://www.anthropic.com/engineering/multi-agent-research-system)。"execution plans with progress and decision logs" をリポジトリに置く — [OpenAI](https://openai.com/index/harness-engineering/)。"Make your agent a stateless reducer" — [12-factor agents](https://github.com/humanlayer/12-factor-agents)。Claude Code agent teams は再開も lead の交代もできない — [agent teams](https://code.claude.com/docs/en/agent-teams)。
   - 具体化: `captain` イベントに、目標・分割とその理由・依存・前提・未決事項を構造化して残す（Task Ledger 相当）。wake 時に見せる現況は台帳から毎回導出する。wake プロンプトは前半を固定してイベントを末尾に足すとキャッシュが効く — [Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)。

9. **上に渡すのは要約と参照。transcript は渡さないが、掘る口は用意する。自己申告と事実は欄を分ける。**
   - 根拠: subagent は "a condensed, distilled summary of its work (often 1,000-2,000 tokens)" を返す — [context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)。入力が長いほど性能は不安定になり、distractor 1 つでも下がる — [Context Rot](https://www.trychroma.com/research/context-rot)。"preserving task-critical information is more important than simply increasing the number of agents" — [OrchBench](https://arxiv.org/abs/2607.25656)。言い換えの連鎖は誤りを生み、原文の転送で約 50% 改善 — [LangChain](https://www.langchain.com/blog/benchmarking-multi-agent-architectures)。handoff には "important notes, concerns, deviations, findings" を含める — [Cursor 続報](https://cursor.com/blog/self-driving-codebases)。検証できない主張は連絡の失敗の 4.3% — [CooperBench](https://arxiv.org/html/2601.13295)。
   - 反対の立場: Cognition の第 1 原則は全トレースの共有 — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)。これは同じ成果物を書く者同士の話で、書かない Captain には要約で足りる。§5.6 の「transcript は渡さない」は維持でよい。
   - 具体化: `fleet digest` に「懸念・逸脱・未完了」の欄を足す。環境由来の事実（テストの終了コード、radar、コミットの有無、変更ファイル）と worker の自己申告を別欄にする。Captain がオンデマンドで引ける読み取りコマンド（diff stat、失敗テスト名、最後のエラー）を CLI に用意する。人間には Captain の言い換えでなく worker の原文を見せる。

**P2: 運用と信頼の土台**

10. **イベント駆動・デバウンス・まとめ渡しは維持。そこに上限を足す。**
    - 根拠: "The lead doesn't need to poll for updates" — [agent teams](https://code.claude.com/docs/en/agent-teams)。"Planners should wake up when their tasks complete" — [Cursor](https://cursor.com/blog/scaling-agents)。"distracting each other with excessive updates" と "spawning 50 subagents for simple queries" — [Anthropic 2025](https://www.anthropic.com/engineering/multi-agent-research-system)。予算を見せないと道具の呼び出し上限を増やしても伸びない — [BATS](https://arxiv.org/abs/2511.17006)。"3 consecutive denials or 20 total" で人間へ — [auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)。"Start with 3-5 teammates" — [agent teams](https://code.claude.com/docs/en/agent-teams)。
    - 具体化: 1 つの目標から Captain が作れる Task 数の上限（超えるなら人間に確認）、1 wake あたりの行動数、采配の連続失敗での停止とエスカレーション、残予算を wake の入力に含める。§5.6 のトークン常時表示は「見えないコストを作らない」だけでなく、Captain 自身に予算を自覚させる入力としても使える。

11. **完了の判定は環境の事実で行う。検証は多層にする。**
    - 根拠: Task Verification 系の失敗が 23.5%、検証器は "only superficial checks" に流れ、"Multi-Level Verification is Needed" — [MAST](https://arxiv.org/html/2503.13657v3)。verifier は 1〜2 個のテストで成功を宣言する（early victory problem）。対策は具体的な基準と "You MUST run the complete test suite before marking as passed" — [Anthropic 2026](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)。"the task verifier is nearly perfect" であることが前提 — [C compiler](https://www.anthropic.com/engineering/building-c-compiler)。中央の検証の関所は誤り増幅を 17.2× から 4.4× に抑える — [Scaling Agent Systems v3](https://arxiv.org/html/2512.08296v3)。文脈を共有しないレビューが最も効く — [Cognition 2026](https://cognition.com/blog/multi-agents-working)。
    - 具体化: turn end → review_ready は「worker が終わったと言った」だけと扱う。review_ready への遷移でテストと radar を走らせ、結果を事実層に入れてから Captain を起こす。レビュー専用の Task を切るなら、実装 Task の会話を引き継がせない。

12. **介入を Captain に同乗させる設計は正しい。Task 間の決定の伝播は Captain 経由に限る。**
    - 根拠: "Actions carry implicit decisions, and conflicting decisions carry bad results" — [Cognition 2025](https://cognition.com/blog/dont-build-multi-agents)。未解決問題の 1 つが "How does a child agent surface a discovery that should change its siblings' work?" — [Cognition 2026](https://cognition.com/blog/multi-agents-working)。worker は "don't coordinate with other workers" — [Cursor](https://cursor.com/blog/scaling-agents)。対等な連絡は手数の 20% を使って成功率を上げなかった — [CooperBench](https://arxiv.org/html/2601.13295)。
    - 具体化: §5.4 のとおり `human_send` を次の wake に載せる。加えて worker の digest に「他の Task に影響する発見」の欄を設け、Captain がそれを該当 Task への追撃に変える。worker 同士を直接つながない。

13. **采配ログは loud, attributable, boring に。**
    - 根拠: サイレント障害の 70% は人間の観察で見つかり、最も危険なのは誤りがもっともらしい文章に化ける型 — [When Errors Become Narratives](https://arxiv.org/abs/2606.14589)。事後の原因特定はステップ単位で 14.2% — [Who&When](https://arxiv.org/abs/2505.00212)。"Adding full production tracing let us diagnose why agents failed" — [Anthropic 2025](https://www.anthropic.com/engineering/multi-agent-research-system)。
    - 具体化: §5.6 の `captain` イベントに、判断の入力になったイベント ID と理由を 1 行で持たせる。Captain が何もしなかった wake も記録する。

14. **信用する前に測る。体感で判断しない。**
    - 根拠: 約 20 件の実例から始め、状態を変える系は end-state で見る — [Anthropic 2025](https://www.anthropic.com/engineering/multi-agent-research-system)。計画だけをシミュレーションで評価して実実行と r = 0.816 — [OrchBench](https://arxiv.org/abs/2607.25656)。チームの合格率は協調の有無を隠す — [TeamBench](https://arxiv.org/abs/2605.07073)。開発者の体感は実測と逆を向いた — [METR](https://arxiv.org/abs/2507.09089)。
    - 具体化: 台帳のイベント列を再生する回帰試験（ステートレスだから組める）。分割判断・委任文・権限推薦・stall と完了の判定を別々に測る。Captain あり / なしで同じ目標を走らせ、統合までの時間・人間の操作回数・総トークン・統合後のテスト失敗を比べる。

### アンチパターン（やらないこと）

- **A1. Captain に merge や自動承認を与える。** 17% の見逃し、50% 未満の権限精度、49% の誤承認 — [auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode)、[ClawArena-Team](https://arxiv.org/abs/2606.31174)、[TeamBench](https://arxiv.org/abs/2605.07073)。
- **A2. 役割で Task を割る（設計役・実装役・テスト役・レビュー役を同じ機能の別 Task に）。** "Sequential phases of the same work" は悪い境界で、引き継ぎのたびに忠実度が落ちる — [Anthropic 2026](https://claude.com/blog/building-multi-agent-systems-when-and-how-to-use-them)。例外は文脈を共有しない事後レビュー — [Cognition 2026](https://cognition.com/blog/multi-agents-working)。
- **A3. worker 同士の自由な連絡や、共有ロックファイルでの自己調整。** "Twenty agents would slow down to the effective throughput of two or three" — [Cursor](https://cursor.com/blog/scaling-agents)。"the unstructured-swarm approach... is mostly a distraction" — [Cognition 2026](https://cognition.com/blog/multi-agents-working)。
- **A4. Captain に役割を詰め込む。** 計画・探索・spawn・監視・レビュー・編集・マージ・完了判定を 1 体に持たせた executor は病的な挙動を示した — [Cursor 続報](https://cursor.com/blog/self-driving-codebases)。Captain は既に「書かない・merge しない」だが、1 回の wake に分割・レビュー・推薦・完了判定を全部載せない。
- **A5. 細かすぎる分割と大きすぎる分割。** "Too small: coordination overhead exceeds the benefit" / "Too large: teammates work too long without check-ins" — [agent teams](https://code.claude.com/docs/en/agent-teams)。
- **A6. worker の自己申告を事実として台帳に書く。** 完了を検証なしに宣言するのは Magentic-One でも長時間ハーネスでも最頻級の失敗 — [Magentic-One](https://arxiv.org/html/2411.04468v1)、[long-running harness](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)。
- **A7. Cursor や C コンパイラの設計を縮小して写す。** あれは誤りの許容・ほぼ完璧な検証器・グリーンフィールド・$20,000 や兆単位のトークンが前提 — [C compiler](https://www.anthropic.com/engineering/building-c-compiler)、[Cursor](https://cursor.com/blog/scaling-agents)。OpenAI も "should not be assumed to generalize" と注記している — [OpenAI](https://openai.com/index/harness-engineering/)。
- **A8. 並走できる数を Fleet の性能だと考える。** 律速は人間のレビュー容量。"our bottleneck became human QA capacity" — [OpenAI](https://openai.com/index/harness-engineering/)。レビュアー 1 人あたりの負荷は倍になった — [arXiv 2607.01904](https://arxiv.org/abs/2607.01904)。"I can only focus on reviewing and landing one significant change at a time" — [Willison](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/)。Captain の価値は Task を増やすことより、人間が裁く 1 件あたりの手間を減らすこと（自分で書いた仕様に沿った小さな変更、事実つきの digest、衝突しない順序の提案）に置く。

### Gaps
- Captain と同じ形（イベント駆動・ステートレス・CLI だけで行動・人間 gate つき・worker は異種の ACP エージェント）の coordinator を直接評価した研究は無い。上の原則は隣接する証拠からの推論で、最も近い直接証拠は CAID（単一の preprint）。
- 推薦の表示が承認判断に与える影響、デバウンスや Blocked 閾値の最適値、stall 判定の閾値は、necoder 自身で計測するしかない。
- 本節の necoder 側の記述は `docs/FLEET-V2.md` と `docs/FLEET-ARCHITECTURE.md` を読んだ範囲に基づく。コードは確認していない。

