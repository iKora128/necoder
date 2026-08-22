# Fleet architecture — worktree-native multi-agent programming

更新: 2026-07-22。Fleet の実装と設計判断の正。

## Product model

通常 View は 1 worktree の `ProjectSession` を表示し、その中で複数の editor tab と Agent thread を使う。
Fleet は repository を次の単位へ分ける。

```text
RepositoryFleet
├─ IntegrationSpace (main worktree, protected)
└─ TaskSpace 1..N (1 task = 1 branch = 1 linked worktree = 1 ProjectSession)
   ├─ AgentPanel (thread / AgentRun 1..N, composer は panel に 1 つ)
   ├─ Terminal
   ├─ Editor
   ├─ Diff / Review
   └─ Tests (TerminalDock を通常 Terminal と分離)
```

Fleet cell の主単位は AgentRun ではなく TaskSpace。既存の完全な `Entity<AgentPanel>` を Task cell に
そのまま埋め込む。したがって markdown、tool call、permission、composer、thread tab、ACP streaming、
履歴は通常 View と同じ実装であり、同じ worktree に `AgentPanel` を複製して active/composer が競合する
こともない。同じ Task に Agent を追加すると cell ではなく panel 内の thread が増える。

## Default workflow and safety

既定操作は `+ Task` で、設定値には左右されない。

1. IntegrationSpace の HEAD から `task/*` branch を作る。
2. `git worktree add -b` で linked worktree を作る。
3. worktree 固有の ProjectSession / AgentPanel を作る。
4. stable Space ID で Task ledger に永続化し、Agent thread を用意する。

同じ worktree に複数 Agent を入れる操作は `+ Agent to this Task` という明示操作だけ。main worktree は
Agent の通常作業面にせず IntegrationSpace として扱う。

Task lifecycle は Agent runtime と Git health から分離する。

```text
planned → working → blocked ─┐
                    └────────┴→ review_ready → merge_ready → integrating → integrated
                                             ↘ changes_requested → working
```

Agent の permission wait は `blocked`、turn end は `review_ready` へ写像する。Review は worktree を
変更しない `git merge-tree --write-tree`（Conflict Radar）で判定する。Integration は `merge_ready` の
明示操作だけで、dirty main と conflict を拒否する。merge が失敗した場合は自動 `merge --abort` する。

## Persistence and orchestration

`task_spaces` は stable Space ID、repository ID、root、branch、base/head OID、phase、result summary を持つ。
`task_events` は phase change / spawn / result / integration の追記ログ。rail index や cell index は identity
に使わない。Agent thread は同じ Space ID を storage scope として保存し、別 Task の Panel に混入しない。

Coordinator Agent / script の操作面は GUI と同じ ledger と Git safety gate を使う。

```bash
necoder fleet create [integration-root] [title]
necoder fleet list [integration-root]
necoder fleet status <task-id> <phase> [summary]
necoder fleet wait <task-id> <phase> [timeout-seconds]
necoder fleet review <task-id> [integration-root]
necoder fleet integrate <task-id> [integration-root]
```

MCP にも `fleet_create_task`, `fleet_list_tasks`, `fleet_update_task`, `fleet_wait_task`,
`fleet_review_task`, `fleet_integrate_task` を公開する。wait は GUI process の一時 state でなく永続 ledger を
poll するため、Coordinator や UI が再起動しても継続できる。

## Independent implementation and licensing boundary

2026-07-23 の比較調査で herdr の公開 source code を閲覧済みのため、本機能を厳密な
clean-room 実装とは呼ばない。調査では「worktree を隔離単位にする」「複数 Agent の状態を集約する」
「Agent/script が別 Agent を spawn/wait できる」という公開された振る舞いと設計上の概念を比較材料とする。

herdr の source code、内部型、protocol 実装、UI asset を necoder へ複製・翻案・移植しない。
実装は necoder の `ProjectSession`, ACP events, GPUI Entity, Git CLI wrapper, Turso storage と本文書の
TaskSpace-first ドメインモデルから独立に設計する。herdr のコードまたは asset の取り込みが必要になった場合は、
実装に先立って別途ライセンス判断を行う。

## Invariants

- 1 TaskSpace = 1 branch = 1 worktree = 1 ProjectSession = 1 AgentPanel entity。
- cell を閉じても Task/Agent は終了しない。Agent の終了は thread archive、Task の終了は lifecycle 操作。
- IntegrationSpace への write は明示 Integrate のみ。
- Git/DB/Host I/O は render 中に行わない。
- 色は identity、状態は形と動き。Task phase、ThreadActivity、Git health を一つの enum に潰さない。
