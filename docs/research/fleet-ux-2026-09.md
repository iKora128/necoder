# Fleet の UI/UX 再設計メモ（2026-09-12・提案。決定ではない）

「Fleet がゴチャついて、どうすればいいのか混乱する」への回答。herdr / Orca / Conductor / cmux の
設計を比較し、**Task（= worktree）を主語にした 1 画面**へ畳む案。ビジュアルは `mock/fleet-v2.html`
（`#l1` 1 枚 / `#l2` 2 列 / `#l3` 3 列 / `#new` ＋Task ダイアログ）。採用したら UI-SPEC §11 と
GLOSSARY を書き換えてから実装に入る（文書が正・CLAUDE.md の約束）。

## 1. Fleet が本来やりたかったこと（BACKGROUND / DECISIONS から）

1. **待ち時間を埋める**: 1 体が走っている間に別の作業を別ブランチで走らせる。
2. **どれが止まっていて・どれが終わったかを一目で**（herdr の状態一覧が原点）。
3. **どのブランチ / worktree に居るかを取り違えない**（宛先チップ・色）。
4. 終わった作業を main へ安全に戻す（Conflict Radar・explicit Integrate）。
5. エージェント自身が編隊を動かせる（fleet CLI/MCP・監督）。

5 つとも「Task を作る → 見張る → 裁く → 統合する」の 1 本の流れに乗っている。
今の画面はこの流れを **8 つの概念**（TaskSpace / セル / ペイン / 面 / 列 / 管制 / 系譜 / 作業）と
**3 つの中央タブ** と **3 つの＋ボタン** に分散させていて、流れが見えない。それが「ゴチャつき」の正体。

## 2. 比較調査（2026-09 時点）

| ツール | 単位 | 画面の骨格 | 状態の見せ方 | 取り入れる点 |
|---|---|---|---|---|
| **herdr**（Rust TUI・Apache-2.0） | Space = worktree、Pane = その中 | 1 repo = 1 画面 = worktree を**横に最大 3 本**（各 ≥50 桁）。左サイドバーに全 Space の状態 | working / blocked / idle / done の 4 値を一列で | 「横並びは 3 本まで」「サイドバー = 状態一覧」「SessionStart hook で pane 自動作成」 |
| **Orca**（ADE・MIT） | Task = worktree | 左に Task 一覧、中央に**選んだ Task 1 つ**（agent + diff + terminal + preview） | Task 行に状態、通知 | 1 プロンプトから Task を作る。中央は 1 つに集中、比較したい時だけ並べる |
| **Conductor**（Mac） | Workspace = worktree | 左に repo → workspace 一覧、中央に選んだ workspace（chat / diff / terminal タブ） | 行の点滅・完了通知 | ライフサイクル「作る → 作業 → レビュー → PR → アーカイブ」を **1 本のボタンで次へ**。**setup script**（`.env` コピー・依存導入）で worktree の未追跡ファイル問題を解く |
| **cmux**（Ghostty 系） | Workspace = tab | 縦タブのサイドバー。行に branch / PR / cwd / ports / **最新通知の本文** | 待ちのペインに青リング・タブ点灯・⌘⇧U で最新未読へ | 行に「最新の一言」を載せる（= digest）。未読へ 1 キーで跳ぶ |

**収束している型**: 「左 = Task 一覧（状態つき）／ 中央 = 選んだ Task を大きく 1 つ／ 並べるのは明示操作で 2〜3 本まで」。
グリッドに全部を敷き詰めるのは tmux 文化の名残で、GUI 勢は誰もやっていない（読めない）。

Zenn 記事（gemcook・herdr × worktree）のコメント欄の罠も拾う: worktree には **commit 済みしか入らない**
（`.env` 等の未追跡ファイルが消える）／ Windows は `Filename too long`。前者は Conductor 同様
**worktree 作成直後の準備スクリプト**で解く（`.necoder/worktree-setup.sh`・W フェーズで後者）。

## 3. 現状の診断（何が混乱を生むか）

- **概念が多い**: Task / セル / ペイン / 面 / 列 / 系譜 / 管制 / 作業。ユーザーが覚える必要があるのは
  本来「Task（worktree）」と「その中に何を開くか」の 2 つだけ。
- **＋が 3 つ**（＋ACP / ＋Terminal / ＋Worktree）: 「どれを押せば並走が始まるのか」を毎回考える。
  Orca / Conductor は **＋ 1 つ = プロンプトを書く = worktree が切れてエージェントが走る**。
- **中央タブが 3 面**（管制 / グラフ / 作業）: 同じ Task 群を 3 通りに見せている。切り替えるたびに
  「今どこを見ているか」を失う。
- **系譜グラフが縦の 1/3 を取る**: 美しいが「次に何をすべきか」を決める情報ではない。セルが潰れる。
- **セルが「面」単位**（Task / Terminal / Editor / Diff / Tests）: 同じ worktree のターミナルが別セルに
  なり、グリッドが増殖する。他ツールは全部 **Task の中のタブ**。
- **裁く場所が管制タブに隔離**: 承認待ちを捌くために画面を切り替える。herdr / cmux は
  サイドバーの列を見るだけで済む。

## 4. 提案: 「Task が主語」の 1 画面

```text
レール | Fleet サイドバー                | 中央                                  
        |  要対応（裁く・常設・上）        |  系譜の帯（64px・薄い・畳める）        
        |  リポジトリ ▸ Task 行 …          |  舞台: 選んだ Task 1 枚（既定）         
        |    ⌂ main（統合先・保護）        |    ヘッダ: ● 名前 ⎇ branch [phase] [次へ] ⤢ ⋯
        |    ⠿ rope設計 task/rope +214 −87 |    タブ: ●スレッド… | 変更 | ターミナル | ファイル | ＋
        |      › 頼んだこと / いま何を      |    本体: transcript / diff / PTY / ツリー
        |  ＋ Task（⌘N）                  |    composer（宛先チップ・トークン常時）   
        |  凡例                           |  下段: ニュース / ターミナル（既存のまま）
```

### 4.1 概念を 2 つに畳む

| 今 | 提案 | 備考 |
|---|---|---|
| TaskSpace / セル / 列 | **Task**（= worktree = ProjectSession） | main も Task 行に出す（`⌂ 統合先・保護`） |
| FleetPane::{Task,Terminal,Editor,Diff,Tests} / ペイン / 面 | **Task の中のタブ**: スレッド（N 本）/ 変更 / ターミナル（N 本）/ ファイル | Tests は「ターミナル」の 1 本。Editor は「ファイル」から開く |
| ＋ACP / ＋Terminal / ＋Worktree | **＋ Task**（サイドバー下・⌘N）と、Task 内タブ行の **＋▾**（スレッド / ターミナル / ファイル） | 「同じ worktree で隔離せず走らせる」は ＋Task ダイアログの「詳細」に降格 |
| 中央タブ 管制 / グラフ / 作業 | **タブ帯を廃止**。管制 = サイドバー上段「要対応」+ 系譜の帯（統合の流れ）。グラフ = 帯の ⌄ で展開。作業 = 削除（本人判断で降格済み） | 管制の全画面版はリモート管制（スマホ）の正として残す |
| セルの 8 種グリッド | **舞台**: 1 枚（既定）/ 2 列 / 3 列（上限・herdr 準拠）。並べる Task はサイドバー行の `◫` でピン | 4 枚以上は並べない。「見えないセル」はサイドバーから戻る |

### 4.2 流れを 1 本にする（Conductor 型ライフサイクル）

`＋ Task` → **稼働中** → （承認待ち / 完了・未確認）→ **レビュー待ち**（radar ✓/✗）→ **統合済み** → 片付け。
Task ヘッダの **phase ピル**と、その右の **「次へ」ボタン 1 つ**（稼働中: レビューへ / 承認待ち: 許可 /
レビュー待ち: Integrate / 統合済み: 片付け）で、状態と次の操作を同じ場所に置く。既存の `TaskPhase`
11 値は内部に残し、表示は 4 段に写像する（Git health と ThreadActivity は enum を混ぜない規律のまま）。

### 4.3 ＋ Task ダイアログ（1 プロンプト = 1 worktree）

- 入力は **「何をする？」1 つ**。ブランチ名は 1 行目から `task/<slug>` を自動（クリックで編集）。
- worktree 先・エージェント / モデル / 承認モードのピル・**準備スクリプト**（`.necoder/worktree-setup.sh`・
  あれば ✓ 表示・無ければ「作る」導線）。
- 「詳細 ▾」に: 既存ブランチから / 同じ worktree にスレッドを足す（隔離しない）。
- ⌘⏎ で worktree 作成 → ProjectSession → スレッド起動 → プロンプト送信 → 舞台に出す（現 `add_worktree_agent`
  と `fleet spawn-agent` の合成。MCP/CLI と同じ入口）。

### 4.4 サイドバー行の情報（cmux + P1 の回収）

行 = 状態グリフ（形と動き・色は Task 色）+ **名前** + `⎇ branch` + トークン + `+N −M` ／
2 行目 = **`› 頼んだこと`**（last_prompt・P1 の残）／ 3 行目 = **いま何を / どう終わったか**（digest）。
「頼んだこと → やったこと」の読み順で、N 体並走の「ズレているやつ」が一目で分かる。⌘1..9 で行へ。

### 4.5 要対応（裁く場所）をサイドバー上段へ常設

管制タブの要対応キューをそのまま左上に置く（Blocked 経過順 → Failed → レビュー → 完了未確認）。
カード内インライン操作（許可 / 常に許可 / 拒否・変更を見る・Integrate・確認）は既存配線を流用。
⏎ で先頭へ没入（既存 `FleetControl` の keymap を移す）。0 件なら見出しだけ（面積を取らない）。

### 4.6 系譜は「帯」に

高さ 64px の帯: main の線 + 分岐 N 本 + 先端に状態グリフ + 右に名前チップ。クリックで舞台へ。
⌄ で従来の 4 表示（扇形 / ツリー / カード / ハブ）へ展開。「色で方向感覚」は帯で保ち、面積は舞台へ返す。

### 4.7 キー（案・母語 Zed 互換の枠内で）

| キー | 動作 |
|---|---|
| ⌘N（Fleet 中） | ＋ Task |
| ⌘1..9 | Task 行へ（舞台を差し替え） |
| ⏎（サイドバーにフォーカス） | 要対応の先頭へ没入 |
| ⌘⇧U | 次の要対応へ（cmux 準拠） |
| ⌘⇧1 / 2 / 3 | 舞台 1 枚 / 2 列 / 3 列 |
| ⌘⇧G | 系譜の帯を展開 / 畳む |

## 5. 既存実装との対応（捨てるものは少ない）

- **そのまま使う**: `ProjectSession`（Task の実体）・`fleet_agents`（Task 内のスレッド）・`AgentPanel` 埋め込み・
  `agent_statuses` の集約・digest / ニュース / 台帳 / fleet CLI・MCP・監督・片付けメニュー・削除確認・
  `TerminalDock.detached`（名札で PTY を持ち回る）・系譜の 4 表示（展開時）。
- **形を変える**: `render_herd_sidebar` → 要対応 + Task 行の 3 段表示 / `render_fleet_grid` → 舞台（1〜3 枚・ピン）/
  `render_fleet_cell` → Task カード（ヘッダ + タブ行）/ `render_fleet_add_buttons` → ＋Task ダイアログ /
  `render_control` → 要対応部分を左へ移し、全画面版はリモート管制の正に。
- **消す**: `FleetCenterView`（タブ帯）・`FleetPane` の面 5 種（Task 内タブへ）・`workbench.rs` / `work_layout.rs`
  （作業タブ。降格済み・用語も GLOSSARY から外す）・8 枚グリッドの行スクロール。

## 6. 決めてもらいたいこと

1. 舞台の既定を **1 枚**にしてよいか（今は「全部並べる」が既定。herdr 記事は 3 本並列を推すが、ACP の
   transcript は端末より縦に長いので 1 枚 + サイドバーが読みやすいと判断）。
2. 系譜を **帯（64px）に縮めてよいか**（4 表示は展開で残す）。
3. 管制タブを **廃止**し、要対応をサイドバー常設 + 全画面版はスマホ用に限定してよいか。
4. ＋ Task の既定エージェント / モデルは `default_*`（sticky 規則は 2026-07-27 のまま）でよいか。
5. `.necoder/worktree-setup.sh` の採用（Conductor 型・未追跡ファイル問題の解）。

参考: [herdr × worktree 記事](https://zenn.dev/gemcook/articles/herdr-worktree-parallel) /
[herdr の運用記](https://coles.codes/posts/herding-agents-with-herdr/) /
[Orca](https://agentconn.com/blog/orca-ade-agent-fleet-parallel-coding-agents-2026/) /
[Conductor](https://www.conductor.build/docs/)・[HN の反応](https://news.ycombinator.com/item?id=44594584) /
[cmux](https://github.com/manaflow-ai/cmux) /
[orchestrator 一覧](https://github.com/andyrewlee/awesome-agent-orchestrators)
