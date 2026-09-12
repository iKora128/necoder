# Fleet v2 — Captain と Task の 1 画面（設計と実装計画の正）

更新: 2026-09-12（本人承認）。**Fleet の UI / 概念 / Captain の正はこの文書**。
UI-SPEC §11・FLEET-CONTROL-PLAN の UI 部分（P3 管制タブ）・GLOSSARY の Fleet 系用語は本文書に従って書き換える
（乖離したら文書を先に直す）。ドメインの不変条件（1 Task = 1 branch = 1 worktree・台帳・Git gate）は
`FLEET-ARCHITECTURE.md` のまま有効で、本文書はその上に画面と Captain を載せ直す。
ビジュアルの正は `mock/fleet-v2.html`（`#l1` 1 枚 / `#l2` 2 列 / `#l3` 3 列 / `#captain` Captain / `#new` ＋Task）。
比較調査と経緯は `docs/research/fleet-ux-2026-09.md`。

## 0. 一言で

**Fleet は「Task（= worktree）を作る → 見張る → 裁く → 統合する」の 1 本の流れを、
左 = Task 一覧 / 中央 = 選んだ Task 1 枚 / 采配 = Captain、の 1 画面で回す面。**
概念はユーザーに **Task** と **その中のタブ** と **Captain** の 3 つしか見せない。

## 1. ねらいと原則

1. **待ち時間を埋める**: 1 体が走っている間に別の作業を別 worktree で走らせる。
2. **どれが止まっていて・どれが終わったかを一目で**: 状態は形と動き、色は識別（UI-SPEC §1.3 の掟は不変）。
3. **どこに書いているかを取り違えない**: 宛先チップとトークン常時表示は Fleet でも不変。
4. **戻すのは安全に**: Conflict Radar（read-only）→ 人間の Integrate。Captain も integrate はできない。
5. **Captain が中核**: 人間は目標を Captain に渡し、Captain が Task を切って見張って報告する。
   Task に直接話すのは「介入」で、常にできるが台帳に残る。

反対に**やらないこと**: 全 Task をグリッドに敷き詰める（読めない）・中央を 3 面のタブで切り替える・
＋ボタンを 3 種類出す・他プロジェクトの Task をサイドバーに混ぜる・状態に色相を使う。

## 2. 用語（GLOSSARY へ転記済み。ここが出所）

| 概念 | code | 日本語 UI | 英語 UI | 備考 |
|---|---|---|---|---|
| **Captain**（采配だけをする 1 本のスレッド） | `captain`（← `coordinator`） | Captain | Captain | 旧「監督」。UI 文字列・NewsKind・設定キー `captain_agent` に改名 |
| ↳ **Captain バー**（サイドバー最上段・最後の采配 1 行・押すとカードへ） | `captain_bar` | — | — | Task 行には混ぜない |
| **Task**（= branch = worktree = ProjectSession） | `TaskSpace` / `SpaceId` | Task | Task | 変更なし |
| 統合先（main の worktree・保護） | `SpaceKind::Integration` | 統合先 | Integration | Task 一覧の中に `⌂` で出す |
| **Fleet サイドバー**（要対応 + Captain + Task 一覧 + ＋Task） | `fleet_sidebar` | Fleet サイドバー | Fleet sidebar | 変更なし（中身が変わる） |
| **要対応**（サイドバー最上段の裁く列） | `attention_queue` | 要対応 | Attention | 管制タブから移設 |
| **舞台**（中央。Task カードを 1〜3 枚） | `stage` / `StageLayout::{One,Two,Three}` | 舞台 | Stage | 旧グリッド。上限 3 |
| **Task カード**（舞台の 1 枚） | `TaskCard` | Task カード | Task card | 旧セル |
| ↳ **Task タブ**（カードの中の面） | `TaskTab::{Thread(id),Diff,Terminal(id),Files}` | スレッド / 変更 / ターミナル / ファイル | Thread / Diff / Terminal / Files | 旧 FleetPane 5 種・作業タブの面 |
| ↳ **ピン**（並べる Task を選ぶ） | `pinned` | 並べる | Pin | サイドバー行の `◫` |
| **系譜の帯**（中央上の薄い系譜） | `lineage_strip` | 系譜 | Lineage | ⌄ で従来の 4 表示に展開 |
| **次へ**（phase に応じた唯一の主操作ボタン） | `next_action` | （phase 別の語） | （phase 別） | §4.3 |
| **＋ Task**（1 プロンプト = 1 worktree） | `new_task_dialog` | ＋ Task | + Task | 旧 ＋ACP / ＋Terminal / ＋Worktree を統合 |
| **準備スクリプト**（worktree 作成直後に 1 回） | `worktree_setup` | 準備 | Setup | `.necoder/worktree-setup.sh` |
| **介入**（Captain を通さず Task に直接書く） | `human_send`（`NewsKind::HumanSend`） | （宛先チップで示す） | | 台帳に積む |
| **采配ログ**（Captain の判断と実行の履歴） | `NewsKind::Captain` | 采配ログ | Captain log | ニュースの Captain 行 + Captain カードのタブ |

**廃止語**: 監督 / Coordinator（→ Captain）・セル / `FleetPane`（→ Task カード / Task タブ）・
作業 / 列 / ペイン / 面 / `WorkColumn` / `WorkPane` / `WorkSurface`（→ 廃止）・
管制（中央タブとしては廃止。**リモート管制**の名前だけは P9 の完了まで据え置き）・herd（従来どおり code 専用）。

## 3. 画面構成

```text
レール | Fleet サイドバー（256px）      | 中央
      |  ⚑ Captain · ✳ 最後の采配   ⌘0 |
      |  要対応 N        ⏎ で先頭へ    |  系譜の帯（64px・⌄ で展開・⌘⇧G で畳む）
      |   [◐ tab色分け 45s 許可/常に/拒否] |  舞台: Task カード × 1（既定）/ 2 / 3
      |   [✓ gpui起動  変更を見る/Integrate]|   ┌ ● 名前 ⎇ branch [phase] …… [次へ] ⤢ ⋯
      |  necoder ⎇ main · 3 Tasks       |   │ ●スレッド | ●レビュー | 変更 +N −M | ターミナル | ファイル | ＋▾
      |   ⌂ main     統合先 · 保護        |   │ transcript / diff / PTY / ツリー
      |   ⠿ rope設計 task/rope ⚑ +214 −87 |   │ 宛先チップ ／ トークン
      |     › 頼んだこと                  |   └ composer
      |     いま何を / どう終わったか       |  下段: ニュース / ターミナル（既存）
      |  ＋ Task（⌘N）／ 凡例             |
```

### 3.0 入口（Fleet をどこから開くか）

現状の入口は titlebar 右上の小さなトグル（fg2・11px）・メニュー・コマンドパレットだけで、既定キーも無く、
レールの ⚡ は herd サイドバーを開くだけで Fleet には入らない。「モードの切替」が「設定っぽいトグル」に見える位置にある。
直すのは 4 点:

1. **titlebar 左のモード切替**: プロジェクトピルの右隣に `Editor | Fleet` のセグメント（`Workspace::mode`）。
   「今どちらの面にいるか」は「どのプロジェクトか」の隣にあるべき情報。右上のトグルは廃止。
   Fleet 側のセグメントに **要対応の件数バッジ**（`◐ 2`・err 色ボーダー・0 なら出さない）を載せ、Editor で作業中でも
   裁くべきものがあれば目に入る。
2. **レールに Fleet の入口を置く**（herd サイドバーは Fleet の一部になるので単独表示を廃止・`ToggleHerdSidebar` 削除）。
   *実装時の訂正（2026-09-12・F0.5）*: 「レールの ⚡」は**そもそも存在しなかった**（`rail.herd` 設定だけが孤児で、
   どのアイコンにも結ばれていなかった）。よってアイコンを新設し、設定キーは `rail.fleet` に改名。
   絵は同梱済みの `layout-grid.svg`（旧 Fleet トグルと同じ＝入口が 2 つに見えない。`zap.svg` は同梱していない）。
   要対応があれば err 色・Fleet 表示中は accent。
3. **既定キー**: `⌘⇧M`（Mode）で Editor ⇄ Fleet。母語 Zed 互換の確定待ちの枠内で、Zed 未使用のキーを選ぶ。
   Fleet 内の ⌘0..9 / ⌘N（§7）は Fleet 文脈のみ。
4. **初回の導線**: 2 本目の Task を切った瞬間（または Captain を任命した瞬間）に一度だけトースト
   「並走を Fleet で見る（⌘⇧M）」。それ以外の案内は出さない。

### 3.1 レール

変わらない。レールで選んだ **1 プロジェクト（= 1 リポジトリ）の編隊だけ**を右に出す。他プロジェクトの稼働は
レールのアイコン下ドットと statusbar ロールアップが担う（二重に出さない）。レールを切り替えると編隊ごと切り替わる。

### 3.2 Fleet サイドバー

上から 5 段。左カラムの排他規則（Todo / git / エクスプローラを開いていればそれ）は solo と同じで変えない。

0. **Captain バー**（`captain_bar`・最上段・高さ 46px）: `⚑` + `Captain` + agent 名 + `⎇ main` + `⌘0`、2 行目に最後の采配 1 行（✳）。クリックで Captain カードへ。未任命なら「Captain を任命する」（設定の Captain 節へ）。Task 行の並びに混ぜない（Task ではないので）。
1. **要対応**（`attention_queue`）。並び: Blocked 経過時間順 → Failed → レビュー待ち（radar 済み）→ 完了・未確認。
   カード = 状態グリフ + 名前 + 経過 / 1 行の内容（permission 文・エラー・digest）/ **Captain の推薦（あれば・✳ 付き）**
   / インライン操作。操作は phase ごとに固定:

   | 種別 | 操作（左が主） |
   |---|---|
   | 承認待ち | 許可 / 常に許可 / 拒否（ACP 広告ラベルそのまま） |
   | 失敗 | 修正を指示 / 開く / 破棄 |
   | レビュー待ち（radar ✓） | 変更を見る / Integrate / 確認 |
   | レビュー待ち（radar ✗） | 変更を見る / 修正を指示 |
   | 完了・未確認 | 確認（Done → Idle）/ 開く |

   先頭カードに「次 ⏎」。0 件なら見出しだけ（面積を取らない）。既存の `control_view.rs` のキュー描画と配線を移設する。
2. **リポジトリ見出し** = `● 名前 ⎇ 統合先ブランチ · N Tasks · Captain 任命済み/未任命`。
3. **Task 行**（`fleet_sidebar.rs`）。3 段固定・高さ 54px:
   - 1 段目: 状態グリフ（`activity_dot`・Task 色）+ **名前** + `⎇ branch`（mono・省略）+ `⚑`（Captain が起動した Task）
     + 右にトークン + `+N −M`（`git diff --shortstat` を Task 単位でキャッシュ・render 中に取らない）
   - 2 段目: `› 頼んだこと`（`Thread.last_prompt`。人間の発話なので ✳ を付けない。先頭から詰める）
   - 3 段目: digest（Blocked = 許可待ちの内容 / Done = 末尾 1〜2 文 / Failed = エラー / Working = 実行中ツール + plan）
   - 右端 2 段目に `⌘1..9`。*実装時の訂正（F1）*: これは**レールの並び**（`ActivateProjectN`）で、
     サイドバーの行順ではない。行順に振ると押した先が表示と食い違うため、**表示を実キーに合わせた**。
     ホバーで `◫`（ピン = 舞台に並べる・F2）と `🗑`。ダブルクリックで改名（既存）。
   - **統合先行**（`⌂ main · 統合先 · 保護`）は Captain の次。2 段目に「N 本が分岐中 · 今日 M 件統合」。クリックで
     舞台に main の Task カード（タブ = ターミナル / ファイル / 変更（= 今日の統合）。スレッドは Captain のみ）。
4. **＋ Task** ボタン（⌘N）と凡例（形の説明・中立色）。

### 3.3 系譜の帯

高さ 64px。main の線 + 分岐 N 本（Task 色）+ 統合済みは淡色で main に戻る + 先端に状態グリフ + 右に名前チップ。
チップ / 先端クリックで舞台へ。右端 ⌄ で従来の 4 表示（扇形 / ツリー / カード / ハブ）に展開（高さは今の系譜グラフと同じ）。
⌘⇧G で畳む（畳むと舞台に面積が戻る）。データは既存 `fleet_lanes`。

### 3.4 舞台

`StageLayout::{One,Two,Three}`（既定 One・⌘⇧1/2/3・titlebar 右のトグル）。**4 枚以上は並べない**（herdr の 3 本上限）。
One = サイドバーで選んだ Task を差し替え表示。Two / Three = ピンした Task を左から（不足分は選択中で埋める）。
カードの幅は等分・最小 420px（下回るなら列数を落とす）。「拡大 ⤢」は One に切り替えるだけ（旧サムネイル列は廃止）。
舞台から外れた Task はサイドバーから戻る。閉じても実体（ProjectSession / AgentPanel / PTY）は残る（既存不変条件）。

### 3.5 Task カード

- **上線 2px** = Task 色（識別）。枠はフォーカス時のみ Task 色 55% mix。
- **ヘッダ**: `●` + 名前（ダブルクリック改名）+ `⎇ branch` + **phase ピル**（§4.2 の 4 段 + 補足）+ 右端に
  **次へ**（§4.3・フォーカス中のカードだけ）+ `⤢` + `⋯`（片付けメニュー・既存 5 段のまま）。
  旧ヘッダの `· ACP` 等の surface 名・phase の生文字列・Review / Integrate の 2 ボタン・`🗑` は廃止（`🗑` は ⋯ とサイドバーのホバーに残す）。
- **Task タブ行**（高さ 30px）: `● スレッド名`（複数可・色はスレッド色・上線 2px）| 区切り | `変更 +N −M` | `ターミナル N` | `ファイル` | 右端 `＋▾`。
  `＋▾` = スレッドを足す / ターミナルを足す / ファイルを開く。**同じ worktree に何本足しても Task は 1 枚のまま**。
  スレッドタブの × は既存 `remove_thread`（archive まで 1 本）。
- **本体**: スレッド = 既存 `AgentPanel` を **chrome を畳んで**埋め込む（自前のスレッドタブ行は描かない・メタ行 1 行・
  トークンメーターは畳まない）。変更 = 既存 Diff surface。ターミナル = `TerminalDock.detached` の名札で PTY を持ち回る
  （既存）。ファイル = worktree のツリー（クリックで solo のエディタに開く。Fleet 内にエディタは持たない）。
- **composer**: 宛先チップ `● スレッド名 ／ プロジェクト ⎇ branch` + トークン `used / limit` + 入力枠（Task 色の枠）
  + ピル（Agent / model · effort / 承認モード）。既存そのまま。

### 3.6 Captain カード

Task カードと同じ器で、ヘッダは `⚑ Captain ⎇ main · <agent>`、phase ピルは「次のイベントで起きる · 直近 HH:MM」。
タブ = `Captain`（スレッド）| `采配ログ N` | `Task N`（自分が起動した Task の一覧・クリックで舞台へ）。
transcript には人間の発話・Captain の思考（✳）・fleet コマンドの実行（⏺ / ⎿）・**台帳イベントの受信**
（「イベント · 14:05（台帳）」の灰色カード。人間の発話と区別する）・Captain の報告が時系列で出る。
composer の宛先は `⚑ Captain ／ プロジェクト ⎇ main`。ピルに「采配のみ · integrate は人間」を常時表示。

### 3.7 下段

変更なし（ニュース / ターミナル・高さ共有・上縁ドラッグ）。ニュースの Captain 行は丸チップ（既存の coordinator 表示）。
ターミナルタブの見出しに `· ⎇ branch`（どの worktree の PTY かを常時）。

## 4. Task の一生

### 4.1 ＋ Task ダイアログ（1 プロンプト = 1 worktree）

- 入力は **1 つ**（複数行）。1 行目から `task/<slug>` を自動生成（ASCII 化・40 字・衝突時は `-2`）。クリックで編集可。
- 表示行: ブランチ / worktree パス（`<repo の親>/<repo 名>-worktrees/<slug>`・設定 `worktree_dir` で変更可）/
  エージェント・model · effort・承認モードのピル（sticky 規則は 2026-07-27 のまま = `default_*`）/
  準備スクリプトの有無（✓ パス表示 / 無ければ「作る」→ テンプレを `.necoder/worktree-setup.sh` に書いて開く）。
- 「詳細 ▾」: 既存ブランチから（worktree だけ作る）/ 同じ worktree にスレッドを足す（隔離しない・読むだけの用途向けと明記）。
- ⌘⏎ = `git worktree add -b` → 準備スクリプト → ProjectSession → 台帳 `planned` → スレッド起動 → プロンプト送信 →
  `working` → 舞台に出す。GUI も CLI/MCP の `fleet create` + `spawn-agent` も同じ関数を通す。
- 「複数に分けたいなら Captain に目標を渡す（⌘0）」の案内をヘッダに常時。

### 4.2 phase の表示写像（内部の `TaskPhase` 11 値は変えない）

| 表示（4 段 + 補足） | 内部 phase | ピルの補足 |
|---|---|---|
| **稼働中** | planned / working | `plan k/n` |
| **承認待ち** | blocked | 経過秒（15s 超で脈動を速く） |
| **レビュー待ち** | review_ready / changes_requested / merge_ready | `radar ✓` / `radar ✗ 衝突 N` / 未実行 |
| **統合済み** | integrating / integrated | `HH:MM` |
| 失敗 | failed | エラー先頭 |
| （非表示） | archived | サイドバーから消える（既存） |

ThreadActivity（Working / Blocked / Done / Idle）は状態グリフ、Git health は `radar` 補足、と**別の場所に出す**（1 enum に潰さない）。

### 4.3 次へ（唯一の主操作ボタン）

| 表示 phase | ボタン | 実装 |
|---|---|---|
| 稼働中 | レビューへ（quiet） | `transition_task_space(review_ready)` + radar 実行 |
| 承認待ち | 許可 | `respond_permission`（他の選択肢は要対応カード / transcript 内） |
| レビュー待ち radar ✓ | Integrate | `integrate_task`（dirty main / 衝突は拒否・既存 gate） |
| レビュー待ち radar ✗ | 修正を指示 | composer にフォーカス + 衝突ファイル名を挿入 |
| 統合済み | 片付け | ⋯ メニューを開く（既存 5 段） |
| 失敗 | 修正を指示 | composer にフォーカス |

### 4.4 片付け・削除

既存のまま（⋯ 5 段・「失うものを数える」確認・`confirm_worktree_delete`）。worktree を削除すると
`worktree_dir` 配下のフォルダごと消えるので、その中の `target/` 等も同時に消える（§6 と整合）。

## 5. Captain

### 5.1 位置づけ

**任命制のただの ACP スレッド**（P6 の監督をそのまま昇格）。IntegrationSpace（main の worktree）に住み、
コードを書かず、fleet CLI/MCP だけで采配する。**状態を持たない**（記憶は台帳）。だから交代・再起動・
エージェント変更が自由で、Claude Code / Codex / Gemini / OpenCode どれでも Captain になれる。
これが Claude Code の Agent Teams や Codex 内蔵のマルチエージェントとの差別化（Captain はエージェント非依存）。

### 5.2 権限表

| 操作 | Captain | 人間 | 備考 |
|---|---|---|---|
| Task を作る / 起動する（`fleet create` + `spawn-agent`） | ○ | ○ | Captain が作った Task は `⚑` 帰属 |
| Task に追撃する（`fleet send`） | ○ | ○（介入・§5.4） | |
| 待つ / 依存を宣言（`wait` / `depend` / `wait-deps`） | ○ | | |
| レビュー実行（`fleet review` = radar） | ○ | ○ | read-only |
| phase 遷移の報告（`fleet status`） | ○ | ○ | |
| Task を終了（archived） | ○ | ○ | worktree は消さない |
| **Integrate** | **×** | ○ | 人間 gate（不変） |
| **承認待ちへの応答** | **× （推薦のみ）** | ○ | §5.5 |
| worktree / ブランチの削除 | × | ○ | |
| 設定の変更・Captain 自身の交代 | × | ○ | |

Herdr socket 直叩き等の迂回路は Captain のツールセットに含めない（FLEET-CONTROL-PLAN §0-8 のまま）。

### 5.3 起きる条件（イベント駆動・ポーリング禁止）

| イベント | 起きる | 渡すもの |
|---|---|---|
| 人間が Captain に書いた | 即時 | 発話 + 現況（`fleet list` 相当の事実層） |
| Task が Done / review_ready / Failed | 即時 | 遷移 + digest（3 段圧縮・transcript は渡さない） |
| Task が Blocked | 15 秒経過後 1 回 | permission 文 + Task の digest |
| 人間が Task に介入した（§5.4） | 次の wake に同乗（単独では起こさない） | 介入の原文 |
| Task が integrated | 即時 | 遷移（残 Task の采配のため） |

実行中は重ねない（次のイベントはキューに積み、1 ターンにまとめて渡す）。

### 5.4 人間の 2 つの玄関

- **既定 = Captain に話す**（⌘0）。目標・優先順位・やめる指示。Captain が Task に分解して起動する。
- **介入 = Task に直接話す**（サイドバー行 / 舞台のカード）。composer はそのまま。送信時に台帳へ
  `human_send`（原文）を積み、ニュースに載せ、次の Captain wake に同乗させる。Captain は介入を前提に采配を続ける
  （Captain が知らないまま進む状態を作らない）。
- 1 件だけの単純な作業は ＋ Task で直接切ってよい（毎回 Captain を通すと往復が 1 段増える）。

### 5.5 承認の推薦（gate は人間のまま）

Blocked で起きた Captain は「許可してよい: cargo test の実行（worktree 内・読み取りのみ）」の形で 1 行の推薦を返せる。
UI は要対応カードに ✳ 付きで添えるだけで、**応答はしない**。ポリシーによる自動承認は本文書の範囲外（後段の判断）。

### 5.6 コストの規律

- 渡すのは事実層 + Tier1 digest + キャッシュ済み Tier2 だけ（既存 `fleet digest` の 3 段圧縮）。transcript は渡さない。
- Blocked は 15 秒閾値、同一 Task の連続イベントはデバウンス（5 秒）。
- Captain のトークンは Task 行の右端と statusbar Σ に含めて常時見せる（見えないコストを作らない）。
- Captain の采配は毎回 `task_events` に `captain` として残す（監査可能・ニュースの丸チップ）。

### 5.7 任命

設定 `captain_agent`（旧 `coordinator_agent`・プロジェクト設定 `.necoder/settings.json` でも可・既定ドリフト禁止）。
未任命の間はサイドバーの Captain 行が「任命する」になり、要対応・Task 行・＋Task は全部そのまま使える
（Captain 無しでも Fleet は成立する）。

## 6. worktree の運用（1 branch = 1 worktree・準備スクリプト・ビルドコスト）

### 6.1 原則

- **1 Task = 1 branch = 1 worktree**。`git worktree add -b` で同時に作る（既存）。1 つの worktree に別ブランチを 2 体は
  git の仕様上不可能。同じブランチに 2 体は「詳細」からのみ（互いのファイルを踏むと明記）。
- worktree の置き場は `<repo の親>/<repo 名>-worktrees/<slug>`（設定 `worktree_dir`）。リポジトリの外に置くので
  探索・grep・LSP が別 worktree を舐めない。

### 6.2 準備スクリプト `.necoder/worktree-setup.sh`

worktree 作成直後に 1 回、Task の worktree を cwd にして実行する。環境変数 `NECODER_MAIN_ROOT`（統合先の絶対パス）
`NECODER_TASK_ROOT`（新 worktree）`NECODER_TASK_BRANCH`。失敗（非 0）は Task を `failed` にしてログを要対応に出す。
用途は 3 つで、テンプレ（「作る」ボタンが書く内容）もこの 3 つ:

```sh
#!/bin/sh
# 1) 追跡外ファイルを持ち込む（worktree には commit 済みしか入らない）
for f in .env .env.local .necoder/settings.local.json; do
  [ -f "$NECODER_MAIN_ROOT/$f" ] && cp "$NECODER_MAIN_ROOT/$f" "$NECODER_TASK_ROOT/$f"
done
# 2) Task のプロセス（ACP・ターミナル）に渡す環境変数（KEY=VALUE を 1 行ずつ）
cat > "$NECODER_TASK_ROOT/.necoder/task.env" <<EOF
CARGO_TARGET_DIR=$NECODER_MAIN_ROOT/target
EOF
# 3) 依存の準備（言語ごと・任意）
# pnpm install --prefer-offline
```

necoder は `.necoder/task.env` を読み、その Task で起動する ACP プロセスとターミナルの環境に注入する
（necoder に言語の知識を持たせない。Rust 固有の判断はスクリプト側）。`.necoder/task.env` は `.gitignore` 推奨。

### 6.3 ビルドコストの答え（necoder 自身 = Rust・`target/` 12GB）

worktree を切ると `target/` が worktree ごとに別になり、GPUI の依存を全部作り直す（初回 10 分超・12GB × Task 数）。
選択肢は 3 つで、**既定は A**:

| 案 | やること | 得るもの | 失うもの |
|---|---|---|---|
| **A. target を共有**（既定） | `CARGO_TARGET_DIR=<main>/target` を task.env で注入 | 依存は 1 回だけコンパイル・ディスクは 12GB のまま・設定 1 行・追加ツール無し | cargo は target ディレクトリを排他ロックするので、**同時に走る `cargo test` は直列化**される（待ちは秒〜数分）。`target/debug/necoder` は最後にビルドした worktree の物になる（ドッグフーディングは .app から起動するので影響なし） |
| B. worktree ごとの target + sccache | `brew install sccache` + `RUSTC_WRAPPER=sccache` | ビルドが完全に並列・依存はキャッシュから即時 | ディスクが worktree ごとに 12GB 級・sccache は incremental（ワークスペース crate）を対象外なので自前 crate は worktree ごとに 1 回フルビルド |
| C. 何もしない | | | 初回 10 分超 × Task 数・12GB × Task 数。不採用 |

理由: 並走するエージェントの cargo 呼び出しは `cargo check` が大半で、A のロック待ちは実用上短い。
ワークスペース crate の成果物は fingerprint にパスが入るので worktree 間で汚し合わない（依存だけが共有される）。
`cargo test` の直列化が体感で痛くなったら B に切り替える（task.env の 1 行と sccache 導入だけ・両立も可）。
Node 系は pnpm のストア共有で同型（`pnpm install --prefer-offline`）。Windows の `Filename too long` は W フェーズで扱う。

## 7. キー（Fleet 文脈・母語 Zed 互換の枠内）

| キー | 動作 |
|---|---|
| ⌘⇧M | Editor ⇄ Fleet（全文脈） |
| ⌘N | ＋ Task |
| ⌘0 | Captain へ（舞台 One に差し替え） |
| ⌘1..9 | Task 行 N へ |
| ⏎（サイドバー） | 要対応の先頭へ没入 |
| ⌘⇧U | 次の要対応へ |
| ⌘⇧1 / 2 / 3 | 舞台 1 枚 / 2 列 / 3 列 |
| ⌘⇧G | 系譜の帯を畳む / 展開 |
| esc（composer） | 実行中ターンの中断（既存） |

## 8. i18n キー（`fleet.*` / `captain.*`・ja/en 両方必須）

`fleet.attention` `fleet.attention_empty` `fleet.next_badge` `fleet.tasks_count` `fleet.integration_row` `fleet.integration_sub`
`fleet.asked` `fleet.pin` `fleet.stage_one/two/three` `fleet.lineage` `fleet.lineage_expand`
`fleet.tab_diff` `fleet.tab_terminal` `fleet.tab_files` `fleet.tab_add` `fleet.tab_add_thread/terminal/file`
`fleet.phase_working/blocked/review/integrated/failed` `fleet.radar_ok/ng/none`
`fleet.next_review/allow/integrate/fix/cleanup` `fleet.new_task` `fleet.new_task_hint` `fleet.new_task_branch_auto`
`fleet.new_task_setup_found/missing/create` `fleet.new_task_more` `fleet.new_task_existing_branch` `fleet.new_task_same_worktree`
`fleet.new_task_start` `fleet.setup_failed`
`captain.title` `captain.row_sub` `captain.appoint` `captain.phase` `captain.tab_log` `captain.tab_tasks`
`captain.dest` `captain.pill_scope` `captain.prompt` `captain.recommend` `captain.spawned_by` `captain.human_send`
既存の `control.*` は要対応カードで使うものだけ `fleet.*` に移し、残りは削除。`coordinator_*` は `captain_*` に改名。

## 9. 既存コードとの対応

| 今 | どうする |
|---|---|
| `ProjectSession` / `fleet_agents` / `agent_panel`（現在の操作先） | そのまま。Task カードのスレッドタブ = `fleet_agents` の各 panel の thread |
| `agent_statuses` の集約・`RunningRegistry`・digest・ニュース・台帳・fleet CLI/MCP・`coordinator.rs`（wake） | そのまま（`coordinator` → `captain` 改名のみ） |
| `render_herd_sidebar`（herd_view.rs） | Task 行 3 段 + Captain 行 + 統合先行 + 要対応（control_view.rs のキュー描画を移設）に書き換え |
| `render_fleet_grid` / `fleet_cells` / `fleet_maximized` / サムネイル列 | `stage.rs` に置き換え（`StageLayout` + `pinned: Vec<SpaceId>`。cells の index 管理を廃止し SpaceId で持つ） |
| `render_fleet_cell` + `FleetPane` 7 種 | `task_card.rs`（ヘッダ・phase ピル・次へ・Task タブ行）。`FleetPane` は `TaskTab` に |
| `render_fleet_add_buttons` / `render_fleet_add_tile` / `add_fleet_agent` / `add_terminal_to_selected_task` / `open_worktree_picker` | `new_task_dialog.rs`（1 入力）+ Task タブ行の `＋▾`。`add_worktree_agent` は ＋Task の実体として残す |
| `render_center_tabs` / `FleetCenterView` / `render_control`（管制タブ） | 削除。要対応と Captain バーの配線は移設。`control_ipc` の remote_snapshot はそのまま（リモート管制の正） |
| `workbench.rs` / `work_layout.rs` / `render_work_sidebar` / 設定 `work_tabs_position` | 削除（solo のファイルタブ向き設定は残すなら名前を `tabs_position` に） |
| 系譜グラフ 4 表示 + `render_graph_header` | 展開時の描画として残す。既定は `lineage_strip` |
| `TerminalDock.detached`（名札で PTY） | そのまま。Task タブの Terminal(id) が使う |
| 片付けメニュー / 削除確認 / 改名 / `fleet_seeded` | そのまま（seed は「Captain 行 + 統合先行 + 既存 Task」に） |
| プローブ `NECODER_FLEET*` / `NECODER_CONTROL_PROBE` / `SHIRUSHI_FLEET_PROBE` | `NECODER_FLEET=1`（舞台 One）/ `NECODER_FLEET_STAGE=2|3` / `NECODER_FLEET_NEW=1` / `NECODER_FLEET_CAPTAIN=1` に整理 |

## 10. 実装フェーズ（各フェーズが単独で価値を持ち、単独でマージ可能）

| # | 内容 | 受入（offscreen 目視 + test） |
|---|---|---|
| **F0 用語と設定の改名 ✅ 2026-09-12** | `coordinator` → `captain`（`captain.rs` / 設定 `captain_agent` / `NewsKind::Captain` / 台帳 kind `captain` / i18n `captain.*` / GLOSSARY）。旧キーの読み替えは**作らない**（後方互換なし方針） | 全 test green・i18n parity |
| **F0.5 入口 ✅ 2026-09-12** | titlebar 左の `Editor \| Fleet` セグメント + 要対応バッジ・レールに Fleet アイコン・⌘⇧M・初回トースト・右上トグルと `ToggleHerdSidebar` の削除（§3.0） | Editor 中に承認待ちが出るとセグメントにバッジが出て ⌘⇧M で Fleet に入れる |
| **F1 サイドバー ✅ 2026-09-12** | Task 行 3 段（last_prompt を `Thread` に持つ = P1 残の回収）+ Captain 行 + 統合先行 + ⌘0/⌘1..9。他プロジェクトのグループを出さない。**残: `+N −M`（shortstat キャッシュ）と ピン `◫` は F2 / ⚑ 帰属は F6 / 要対応は F3** | 3 Task 並走で「頼んだこと / いま何を」が各行に出る。レール切替で編隊が切り替わる |
| **F2 舞台と Task カード** | `StageLayout` One/Two/Three + ピン。Task カード = ヘッダ（phase ピル・次へ）+ Task タブ行 + 既存 AgentPanel（chrome 畳み）/ Diff / Terminal / Files。`FleetPane` → `TaskTab` | One で 1 枚が読める。Three で 3 枚が 420px 以上。＋▾ でスレッド / ターミナルを足してもカードが増えない。閉じても実体が残る（既存 test を移植） |
| **F3 要対応をサイドバーへ・中央タブ廃止** | control_view のキュー描画と配線を移設。`FleetCenterView` / 管制タブ / 作業タブを削除 | 承認がサイドバーから完了する。⏎ / ⌘⇧U |
| **F4 ＋ Task ダイアログ + 準備スクリプト + task.env** | 1 入力 → slug → worktree → setup → spawn → prompt。`task.env` 注入。「作る」テンプレ | ダイアログから 1 発で Task が走る。`.env` が新 worktree に入る。necoder 自身で `CARGO_TARGET_DIR` 共有が効く（2 本目の Task の `cargo check` が依存を再コンパイルしない） |
| **F5 系譜の帯** | 64px 帯 + ⌄ 展開 + ⌘⇧G | 帯の先端クリックで舞台へ。展開で従来 4 表示 |
| **F6 Captain の玄関** | Captain カード（采配ログ / Task タブ・台帳イベントの灰色カード）・人間→Captain の即時 wake・介入の `human_send` 記録と同乗・承認の推薦（✳）・トークン表示 | 目標を 1 つ書くと Captain が 2 Task を切って起動し、完了イベントで radar を回して要対応に「確認だけで統合できます」を添える（実 e2e・integrate は人間） |
| F7 掃除 | workbench / work_layout / 旧プローブ / 旧 i18n キーの削除。UI-SPEC §11 を本文書の要約に置き換え。FLEET-CONTROL-PLAN の P3 に「F3 で移設」を記す | `cargo check --workspace` 警告 0 |

順序は F0 → F1 → F2 → F3 → F4 → F5 → F6 → F7。F1 と F5 は独立。**各フェーズの終わりに本文書と JOURNAL を更新**。

F0 で追加した i18n キーは `captain.title`（Captain バーの見出し・ニュースの帰属名・**Captain スレッドの表示名**を兼ねる）/
`captain.appoint`（未任命の行）/ `captain.prompt`（wake テンプレート）の 3 つ。§8 の残りは使う面（F1/F6）と同時に足す。
実装は `/goal` の規律（現在地把握 → 1 歩 → 検証 → 文書）で進める。ドッグフーディング中はスクショを回さず本人目視。

## 11. 今回確定した判断（DECISIONS に転記済み）

1. Fleet は Task を主語にした 1 画面（左 = 一覧 / 中央 = 1 枚 / 並べるのは明示で 3 まで）。
2. 「監督」は **Captain** に改名し、Fleet の中核に置く。人間の既定の玄関は Captain、Task への直接発話は介入として台帳に残す。
3. ターミナルは Task の中のタブ。エージェントの起動は ACP が既定で、CLI 形態は明示選択のみ（DECISIONS ⑥ のまま）。
4. 1 Task = 1 branch = 1 worktree。同じ worktree に 2 体は「詳細」からのみ。
5. サイドバーはレールで選んだ 1 プロジェクトの編隊だけ。他プロジェクトはレールのドットで足りる。
6. Rust のビルドコストは `CARGO_TARGET_DIR` 共有を既定（task.env）。並列が痛くなったら sccache。
7. 管制タブ・作業タブは廃止。要対応はサイドバー常設。管制の全画面版はリモート管制（P9）の正として残す。

**残る判断**（実装中に本人に聞く）: 承認のポリシー自動化（§5.5 の先）/ Captain の推薦を要対応カードで
ワンクリック適用にするか / `worktree_dir` の既定パス / solo のタブ向き設定の名前。
