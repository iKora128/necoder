# Fleet v2 — Captain と Task の 1 画面（設計と実装計画の正）

更新: 2026-09-12（本人承認）。**Fleet の UI / 概念 / Captain の正はこの文書**。
UI-SPEC §11・FLEET-CONTROL-PLAN の UI 部分（P3 管制タブ）・GLOSSARY の Fleet 系用語は本文書に従って書き換える
（乖離したら文書を先に直す）。ドメインの不変条件（1 Task = 1 branch = 1 worktree・台帳・Git gate）は
`FLEET-ARCHITECTURE.md` のまま有効で、本文書はその上に画面と Captain を載せ直す。
ビジュアルの正は `mock/fleet-v2.html`（`#l1` 1 枚 / `#l2` 2 列 / `#l3` 3 列 / `#captain` Captain / `#new` ＋Task）。
比較調査と経緯は `docs/research/fleet-ux-2026-09.md`。
Herdr / Orca の深掘り・采配役（Captain 型エージェント）の製品と研究・ゲームの類推の是非は `docs/research/captain-orchestrators-2026-09.md`（2026-09-20 調査・Captain の P0〜P2 の改善表つき）。

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
| ↳ **スレッドタブ**（カードの中の会話。会話の列の頭の行。スレッドだけが並ぶ） | `FleetPane::Agent` + `AgentPanel` の thread | スレッド名 | Thread name | 旧 Task タブのスレッド分 |
| ↳ **サイドペイン**（会話の横に開く面。開け閉めは Task 見出しの右のトグル。← Task タブ / FleetPane / 面） | `stage_side()`（`FleetPane::{Diff,Shell,Editor}`） | 変更 / ターミナル / ファイル | Changes / Terminal / Files | 旧 Task タブの面の分。タブではなく分割 |
| ↳ **ピン**（並べる Task を選ぶ） | `pinned` | 並べる | Pin | サイドバー行の `◫` |
| **系譜の帯**（中央上の薄い系譜） | `lineage_strip` | 系譜 | Lineage | ⌄ で従来の 4 表示に展開 |
| **次へ**（phase に応じた唯一の主操作ボタン） | `next_action` | （phase 別の語） | （phase 別） | §4.3 |
| **＋ Task**（1 プロンプト = 1 worktree） | `new_task_dialog` | ＋ Task | + Task | 旧 ＋ACP / ＋Terminal / ＋Worktree を統合 |
| **準備スクリプト**（worktree 作成直後に 1 回） | `worktree_setup` | 準備 | Setup | `.necoder/worktree-setup.sh` |
| **介入**（Captain を通さず Task に直接書く） | `human_send`（`NewsKind::HumanSend`） | （宛先チップで示す） | | 台帳に積む |
| **采配ログ**（Captain の判断と実行の履歴） | `NewsKind::Captain` | 采配ログ | Captain log | ニュースの Captain 行 + ブリッジの采配ログペイン |

**廃止語**: 監督 / Coordinator（→ Captain）・セル / `FleetPane`（→ Task カード / Task タブ）・
作業 / 列 / ペイン / 面 / `WorkColumn` / `WorkPane` / `WorkSurface`（→ 廃止）・
管制（中央タブとしては廃止。**リモート管制**の名前だけは P9 の完了まで据え置き）・herd（従来どおり code 専用）。

## 3. 画面構成

```text
レール | Fleet サイドバー（256px）      | 中央
      |  ⚑ Captain · ✳ 最後の采配   ⌘0 |
      |  要対応 N        ⏎ で先頭へ    |  系譜の帯（30px・列数 ▯ ▯▯ ▯▯▯・⚑ 編隊図 = ⌘⇧G）
      |   [◐ tab色分け 45s 許可/常に/拒否] |  舞台: Task カード × 1（既定）/ 2 / 3
      |   [✓ gpui起動  変更を見る/Integrate]|   ┌ ● 名前 ⎇ branch [phase] … [次へ] │ 変更 +N −M  ターミナル  ファイル  ⤢ ⋯
      |  necoder ⎇ main · 3 Tasks       |   │ ●スレッド ●レビュー ＋          │ 変更 · N ファイル            ×
      |   ⌂ main     統合先 · 保護        |   │ 会話（transcript）              │ サイドペイン（diff / PTY / ツリー）
      |   ⠿ rope設計 task/rope ⚑ +214 −87 |   │ 宛先チップ ／ トークン
      |     › 頼んだこと                  |   └ composer
      |     いま何を / どう終わったか       |  下段: ニュース / ターミナル（既存）
      |  ＋ Task（⌘N）／ 凡例             |
```

**Fleet の家はブリッジ**（統合先 main のカード・§3.6）: `⚑ Captain` の会話 | 編隊図。レールで main を選んで Fleet に入ると
ここが出る。Task を選ぶと上図の Task カード（会話 | サイドペイン）に替わり、⌘0 / `⚑ 編隊図` / サイドバーの Captain バーで戻る。

### 3.0 入口（Fleet をどこから開くか）

現状の入口は titlebar 右上の小さなトグル（fg2・11px）・メニュー・コマンドパレットだけで、既定キーも無く、
レールの ⚡ は herd サイドバーを開くだけで Fleet には入らない。「モードの切替」が「設定っぽいトグル」に見える位置にある。
直すのは 4 点:

1. **titlebar 左のモード切替**: プロジェクトピルの右隣に `Editor | Fleet` のセグメント（`Workspace::mode`）。
   「今どちらの面にいるか」は「どのプロジェクトか」の隣にあるべき情報。右上のトグルは廃止。
   Fleet 側のセグメントに **要対応の件数バッジ**（`◐ 2`・err 色ボーダー・0 なら出さない）を載せ、Editor で作業中でも
   裁くべきものがあれば目に入る。数えるのは承認待ち・質問待ちのスレッド + Failed な Task + 裁いていない Captain の分解案
   （質問待ちは O12 で追加・分解案は §5.5・macOS では同じ数を全窓の合計で Dock のバッジにも出す）。
2. **レールに Fleet の入口を置く**（herd サイドバーは Fleet の一部になるので単独表示を廃止・`ToggleHerdSidebar` 削除）。
   *実装時の訂正（2026-09-12・F0.5）*: 「レールの ⚡」は**そもそも存在しなかった**（`rail.herd` 設定だけが孤児で、
   どのアイコンにも結ばれていなかった）。よってアイコンを新設し、設定キーは `rail.fleet` に改名。
   絵は同梱済みの `layout-grid.svg`（旧 Fleet トグルと同じ＝入口が 2 つに見えない。`zap.svg` は同梱していない）。
   要対応があれば err 色・Fleet 表示中は accent。
   *訂正（2026-09-24・本人要望）*: 「herd の単独表示を廃止」は撤回。Fleet から戻ると `show_herd` が残って Editor の左カラムに
   全プロジェクトのスレッド一覧が出ており、本人がそれをレールから開きたいと言った。レールに **AI スレッド一覧**
   （`activity.svg`・設定 `rail.threads`・UI-SPEC §2）を置き、Fleet 中は同じアイコンが Fleet サイドバーへの帰り道になる。
   Fleet の入口は引き続き `layout-grid.svg` の 1 つだけ。
3. **既定キー**: `⌘⇧M`（Mode）で Editor ⇄ Fleet。母語 Zed 互換の確定待ちの枠内で、Zed 未使用のキーを選ぶ。
   Fleet 内の ⌘0..9 / ⌘N（§7）は Fleet 文脈のみ。
4. **初回の導線**: 2 本目の Task を切った瞬間（または Captain を任命した瞬間）に一度だけトースト
   「並走を Fleet で見る（⌘⇧M）」。それ以外の案内は出さない。

### 3.1 レール

変わらない。レールで選んだ **1 プロジェクト（= 1 リポジトリ）の編隊だけ**を右に出す。他プロジェクトの稼働は
レールのアイコン下ドットと statusbar ロールアップが担う（二重に出さない）。レールを切り替えると編隊ごと切り替わる。

### 3.2 Fleet サイドバー

上から 5 段。左カラムの排他規則（Todo / git / エクスプローラを開いていればそれ）は solo と同じで変えない。

0. **Captain バー**（`captain_bar`・最上段・高さ 46px）: `⚑` + `Captain` + agent 名 + `⎇ main` + `⌘0`、2 行目に最後の采配 1 行（✳）。クリックでブリッジ（§3.6）の Captain の会話へ。未任命なら「未任命 — 押して任命する」で、押すとブリッジに任命の面が出る（§5.7）。Task 行の並びに混ぜない（Task ではないので）。
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
   *2026-09-28*: Captain の推薦は**承認待ちのカードにだけ**出る（§5.5）。permission 文と待ち時間の下・ボタンの上に
   `✳ Captain: 許可してよい` と、その下に理由を 2 行まで（✳ だけテラコッタ・見立ては太さで立てる・溢れた分は省略して全文はツールチップ。
   *統合時の訂正*: 見立てと同じ行に理由を置くと、サイドバー 256px では理由が 10 字ほどで切れて読めなかった）。
   表示だけで、許可・拒否のボタンの働きは変えない。
2. **リポジトリ見出し** = `● 名前 ⎇ 統合先ブランチ · N Tasks · Captain 任命済み/未任命`。
3. **Task 行**（`fleet_sidebar.rs`）。3 段固定・高さ 54px:
   - 1 段目: 状態グリフ（`activity_dot`・Task 色）+ **名前** + `⎇ branch`（mono・省略）+ `⚑`（Captain が起動した Task）
     + 右にトークン + `+N −M`（`git diff --shortstat` を Task 単位でキャッシュ・render 中に取らない）
   - 2 段目: `› 頼んだこと`（`Thread.last_prompt`。人間の発話なので ✳ を付けない。先頭から詰める）
   - 3 段目: digest（Blocked = 許可待ちの内容 / Done = 末尾 1〜2 文 / Failed = エラー / Working = 実行中ツール + plan）
   - 右端 2 段目に `⌘1..9`。*実装時の訂正（F1）*: これは**レールの並び**（`ActivateProjectN`）で、
     サイドバーの行順ではない。行順に振ると押した先が表示と食い違うため、**表示を実キーに合わせた**。
     ホバーで `◫`（ピン = 舞台に並べる・F2）と `🗑`。ダブルクリックで改名（既存）。
     ピンと舞台の列数は窓セッションに残る（O21・2026-09-26・再起動を越える。消えた Task のピンは読み飛ばし、次にピンする時に枠から外す）。
   - **統合先行**（`⌂ main · 統合先 · 保護`）は Captain の次。2 段目に「N 本が分岐中 · 今日 M 件統合」。クリックで
     舞台に main の Task カード。*訂正（2026-09-19）*: スレッドは隠さない。main の panel は Editor で普段使っている会話そのものなので、
     Fleet に入った途端に見えなくなる方が害が大きい（スレッドタブ行とトグルは他の Task と同じ）。
     統合先は**メインの作業ツリー**（linked worktree でない slot・`TaskSpace::linked`）。同じリポジトリに統合先扱いの
     slot が複数ある時（`task/` でない linked worktree を ⌘O で開いた等）もメインを選び、サイドバー・＋Task・Captain で
     同じ 1 つ（`integration_slot_for`・O21。以前はサイドバーだけ最後の 1 つを選んでいた）。
   - **消えています**（O21）: レールにある Task の worktree が `git worktree list` から消えていたら（necoder の外で
     `git worktree remove` 等）、3 段目の下に `消えています（necoder の外で削除）`（warn）+ `片付け`（レールから外す）。
   - **絞り込み**（O21・2026-09-26）: Task が 6 本以上ある時（か、語が入っている間）だけ、統合先行と Task 行の間に
     `⌕ Task を絞り込む`（高さ 26・bg2・枠 1px・角 6）。語を空白で区切り、全部の語が名前・ブランチ・頼んだこと・digest の
     どれかに含まれる Task だけ残す（大文字小文字は無視）。見出しの `N Tasks` と統合先行の「N 本が分岐中」は絞り込む前の数。
     0 件なら「一致する Task はありません」（fg2）。
     欄の右に並べ方のチップ（10px fg2・押すたびに `並び: レール` → `並び: 最近`（依頼の新しい順・無い物は後ろ）→ `並び: 要対応`（承認・質問待ち → 作業中 → 完了・未確認 → 静か）→ レール）。同じ鍵の間はレールの順。窓を閉じるまで覚える（保存しない）。
   - **複数選択**（O21・2026-09-26）: Task 行の **⌘ クリック**（Windows / Linux は Ctrl）で選択に足す / 外す、**⇧ クリック**で
     起点（最後に ⌘ / ⇧ で押した行）から押した行まで（見えている並び＝絞り込み・並べ替えの後）。選んだ行は地を bg3 に
     上げる（色相は使わない）。選んでいる間だけ Task 行の上に `N 本を選択 · 休ませる · 舞台に並べる · 片付け… · ×`
     （bg2 の帯・チップは枠 1px）。休ませる = 静かなエージェントをまとめて止める（トーストは 1 回）/ 舞台に並べる =
     先に選んだ 3 本まで（多ければ「先に選んだ 3 本を並べました」）/ 片付け… = 片付けの画面（O22）を、選んだ Task に
     印を付けて開く（消すのは画面で失うものを数えてから）。操作の後と普通のクリックで選択は外れる。窓を閉じるまで（保存しない）。
   - **親子**（O21・A07・2026-09-26）: 起点に別の Task のブランチを選んで作った Task は、その Task の**子**（台帳の
     `task_parents`・CLI の JSON の `parent`）。子は親の直後に 1 段（14px）下げて `↳`（fg2）を付けて並ぶ（兄弟の間は
     今の並べ替えのまま・親が一覧に居ない子と輪になった親子は上の段）。⋯ の「休ませる」は子孫の分も止め、子がいれば
     「子の Task ごと片付ける…」で片付けの画面を親子に印を付けて開く。レールの右クリック「この Task から新しい Task…」
     が入口（起点に親のブランチを入れた ＋ Task）。親を消しても子は残る（上の段に戻る）。
   - **作成中の行**（O20・2026-09-26）: ＋ Task の「作る」を押した直後から、worktree と準備スクリプトが終わるまで
     Task 行の上に 1 本ずつ出す（fan-out も順に作るので、後ろは「順番を待っています…」）。左のバーは地の色（Task の色は
     レールに開いた時に決まる）、ドットは作業中の形・fg2。1 段目 = 依頼の 1 行目（fan-out の印つき）、2 段目 = 段
     （統合先を最新にしています / worktree を作っています / 準備スクリプトを流しています・fg2）。右端の `×` = 取り消す:
     まだ何も作っていなければその場で外し、作っている最中なら「取り消しています…」にして、その段が終わった所で
     worktree と今切ったブランチを消す（既にあったブランチは消さない。走っている git や準備スクリプトを途中で止める口は
     Host に無いので、止めずに捨てる）。作れなかった行は `作れませんでした: <理由の 1 行目>`（warn）+ `やり直す`（同じ依頼・
     同じ詳細で、その行のリポジトリの統合先からもう一度）+ `×`（閉じる）で残る。全文はトーストにも出す。窓を閉じるまで。
     Captain の分解案の承認（§5.5）で切る Task も同じ行で出る（行ごとに順に作り、作れなかった行だけやり直せる）。
3.5 **外部の worktree**（O21・Task 行の下・`▾ 外部の worktree（N）` で畳める）: 選んでいるリポジトリの worktree のうち
   Task になっていないもの = レールに無いもの（**Orca・Claude Code・手で `git worktree add` したもの**も git の一覧から拾う）と、
   レールにあるが統合先扱いのもの（メイン以外）。行 = `◌`（中立）+ フォルダ名 + `⎇ branch` + `取り込む`。
   **取り込む** = レールに開いて Task にする（ブランチ名に関係なく・ブランチもファイルもそのまま）。台帳に Task として残し、
   再起動しても Task のまま（linked worktree に限り、台帳の Task を branch 接頭辞より優先する）。
   一覧（`git worktree list`）を読むのは、Fleet を出した時・レールの worktree が増えた / 減った時・窓が前に出た時だけ
   （ポーリングしない・背景で 1 回）。
4. **＋ Task** ボタン（⌘N）と凡例（形の説明・中立色）。

### 3.3 系譜の帯

*2026-09-20 作り直し*: 舞台の上は **1 行（30px）だけ**: `系譜` + 帯（main の `━` / 分岐 `┬━ 名前` / 統合済み `╰━`・Task 色・
クリックで舞台の選択へ）+ 列数トグル（`▯ ▯▯ ▯▯▯`）+ `⚑ 編隊図`。
編隊図そのもの（ハブ / 扇形 / ツリー / カード）は**ブリッジのサイドペイン**に住む（§3.6）。旧仕様の「⌄ で舞台の上に展開」は
廃止した — 縦に積むと Task カードが潰れ（高さ 360px の図 + 下段 290px で会話の面積がほぼ 0）、畳んだままだと Fleet らしい絵が
どこにも出ないので、実機では常に畳まれ「ただのチャット」に見えていた。`⚑ 編隊図` と ⌘⇧G（`ToggleLineage`）は
「ブリッジへ行って編隊図を開く / ブリッジで開いていれば閉じる」。データは既存 `fleet_lanes`。

### 3.4 舞台

`StageLayout::{One,Two,Three}`（既定 One・⌘⇧1/2/3）。*訂正（2026-09-19）*: トグル（`▯ ▯▯ ▯▯▯`）は**系譜ヘッダの右**に置く。
専用の 30px 行は廃止し、舞台の上は「系譜 + 帯 + 列数 + ⚑ 編隊図」の 1 行だけにした（§3.3・縦 64px を舞台へ返す）。**4 枚以上は並べない**。*訂正（2026-09-21）*: 旧記述の「herdr の 3 本上限」は誤りで、herdr 本体に pane 数の上限は無い。出所は Zenn 記事の著者が自作 hook に入れた設定値（`MAX_COLUMNS=3` / `MIN_COLS=50`・152 桁の端末に 50 桁 × 3 列）。上限 3 の判断は変えない — 根拠は 1 利用者の経験則、カードの最小幅、監督制御の研究の相場（直接の制御は約 3 機・`docs/research/captain-orchestrators-2026-09.md`）。
One = サイドバーで選んだ Task を差し替え表示。Two / Three = ピンした Task を左から（不足分は選択中で埋める）。
カードの幅は等分・最小 420px（下回るなら列数を落とす）。「拡大 ⤢」は One に切り替えるだけ（旧サムネイル列は廃止）。
舞台から外れた Task はサイドバーから戻る。閉じても実体（ProjectSession / AgentPanel / PTY）は残る（既存不変条件）。

### 3.5 Task カード

- **上線 2px** = Task 色（識別）。枠はフォーカス時のみ Task 色 55% mix。
- **ヘッダ**: `●` + 名前（ダブルクリック改名）+ `⎇ branch` + **phase ピル**（§4.2 の 4 段 + 補足）+ 右端に
  **次へ**（§4.3・フォーカス中のカードだけ）+ `│` + **サイドペインのトグル**（`変更 +N −M  ターミナル N  ファイル`・下の
  *2026-09-28*）+ `⤢` + `⋯`（片付けメニュー・既存 5 段のまま）。右側（次へ〜⋯）は 1 つの塊で、狭いカード（2〜3 列）で
  入りきらなければ塊ごと 2 行目の右へ折り返す。題名は 72px を残して縮み、ブランチ名は省略で縮む。
  旧ヘッダの `· ACP` 等の surface 名・phase の生文字列・Review / Integrate の 2 ボタン・`🗑` は廃止（`🗑` は ⋯ とサイドバーのホバーに残す）。
  **統合の下見で競合した時**（O19・2026-09-26）: 統合（`git merge-tree` の下見）で競合したら、競合したファイルを覚えて
  **次へ**を「競合を直させる」にする（管制のカードにも同じボタン）。押すと Task のいまのスレッドへ人間の発話として
  「統合先 `main` を取り込んで競合を解消し、コミットして」（競合したファイルの一覧つき・統合はしない）を送る（作業中なら
  終わってから流れる・スレッドが無ければ立てる）。送ったら覚えを消すので、次へは「統合」に戻り、直った後に押せば
  下見からやり直す（まだ競合していればまた覚える）。統合先は触らない。
- **スレッドタブ行とサイドペインのトグル**（*2026-09-28 作り直し*）。旧「Task タブ行」は *誰と話すか*（スレッド）と *何を見るか*
  （変更 / ターミナル / ファイル）という別の軸を 1 本のタブに混ぜていた。2026-09-19 の「ペインバー」は同じ行の左右に
  分けたが、**会話の AI と変更・ファイルが同じ段に同列に並ぶ**ことは変わらず、本人から「UX として最悪」と差し戻された
  （2026-09-16 に一度「変更やファイルも同じ様にタブになってる」、2026-09-27〜28 に再度）。2 つの軸を**別の段**に置く:
  - **スレッドタブ行**（高さ 30px）= **会話の列の頭**に置く、スレッド（AI との会話）だけの行。`● スレッド名`（色はスレッド色・
    選択は上線 2px）+ 直後に `＋`（= 会話を足す、だけ。同じ worktree に足すので Task は 1 枚のまま）。選択中のタブに `×`
    （既存 `close_thread`・⌘⇧T で戻せる）。最後の 1 本には出さない。`＋` で足した panel が空になったら一覧から外し、
    会話は残っている方へ移す。サイドペインを開いて分割しても、この行は会話の列の幅だけに収まる（サイドペインの上に掛からない）。
  - **サイドペインのトグル** = **Task 見出しの右**（`│ 変更 +N −M  ターミナル N  ファイル`・角丸のセグメント。タブの見た目に
    しない）。変更 / ターミナル / ファイルは Task（worktree）の持ち物で、会話の相手ではないので、Task の段に置く。
    `+N −M` = `RepositoryController.task_files` の合計（`git diff --numstat <base_oid>`・base が無い統合先は HEAD ＋ 追跡外の
    ファイルの行数。git 更新と同じ background で取る）。サイドバーの Task 行と変更ペインの見出しも同じ値を読む（0/0 と未取得は
    出さない）。*2026-09-28*: 追跡外の行数を 0 と数えていたので、見出しの数が変更レビューの見出しより少なく出ていた（新しく作った
    ファイルの分）。レビューと同じ規則（バイナリ・1MB 超は 0）で数える。読むのは 1 回の更新で 4MB まで（超えた分は 0 行）。
    押すと**会話の右に分割で開く**（マルチプレクサの split）。同じものをもう一度押すか、ペイン見出しの `×` で閉じる。
    開けるサイドペインは 1 つ（3 分割にはしない）。
  - カード幅が 900px 未満（2〜3 列）では分割せず、サイドペインが本体の全面に出る（会話の列ごと＝スレッドタブ行も隠れる）。
    会話へ戻るのは、見出しの同じトグルをもう一度押すか、ペイン見出しの `×`。
- **本体** = `会話の列（スレッドタブ行 + 会話）| 境 | サイドペイン`（サイドは既定 44%・境のドラッグで 25〜65%・全カード共通・保存はしない）。会話 = 既存 `AgentPanel` を **chrome を畳んで**埋め込む
  （自前のスレッドタブ行は描かない・トークンメーターは畳まない）。サイドペインは見出し 30px（名前 + 補足 + `×`・会話の列のスレッドタブ行と
  高さを揃える）+ 中身:
  - 変更 = **変更レビュー**（UI-SPEC §14・O6・Task の base が既定の比較）。左にファイルの木（状態の 1 文字 + 名前 +
    `+N −M`）、右に色付きの diff。行コメントと注記トレイ（O7）もそのまま使え、注記は隣の会話（スレッド）へ 1 通で送れる
    ＝ Fleet から出ずに読んで直させられる。ファイルを編集する操作（「エディタで開く」）だけ Editor へ出る。見出しの補足は
    `N ファイル` と `+N −M`（`git diff --numstat/--name-status <base_oid>` + untracked を git 更新の background で取り
    `RepositoryController.task_files` に置く。`+N −M` はその合計）。開いた時に `activate_review` が基準を渡して読み込ませる
    （描画中に git を叩かない）。*2026-09-20 の訂正*: F2 は `GitPanel` の Entity をそのまま埋めていたが、`GitPanel` は描画を
    持たない（本体は active session だけを描く `render_git_panel`）ので、**実機では中身が空だった**。隔離 offscreen の実画面で
    発覚し、一度はファイルの一覧（押すと Editor で diff）に替えた。*2026-09-27（統合）*: O6 の変更レビューをこのペインに
    載せ直した（ファイルの一覧はレビューの無い session の予備としてだけ残る）。
  - ターミナル = 見出しに Task 内の通番チップ `1 2 ＋`（**端末を足すのはここ**）と `⎇ branch`。PTY は
    `TerminalDock.detached` の名札で持ち回る（既存）。1 本も無ければトグルを押した時に 1 本作る。
  - ファイル = worktree のツリー（▸/▾・git の 1 文字と色はエクスプローラと同じ語彙。クリックで solo のエディタに開く。
    Fleet 内にエディタは持たない）。
- **composer**: 宛先チップ `● スレッド名 ／ プロジェクト ⎇ branch` + トークン `used / limit` + 入力枠（Task 色の枠）
  + ピル（Agent / model · effort / 承認モード）。既存そのまま。

### 3.6 ブリッジ（統合先のカード = Fleet の家）

*2026-09-20 作り直し*: 旧「Captain カード」（⌘0 の時だけ舞台に出る別実装のカード）は廃止し、**統合先（main）のカードそのものを
ブリッジにした**。Captain は「main に住むただの ACP スレッド」（§5.1）なので、スレッドタブとして座るのが素直で、
別のカード実装・`captain_space` / `captain_tab` の状態は要らなかった。レールで main を選んで Fleet に入ると最初に見えるのがここ。

- **スレッドタブ行**（会話の列の頭）: 先頭に `⚑ Captain`（accent 色）→ 続けて main の通常スレッド → `＋`。
  - 任命済み: `⚑ Captain` = main の panel の Captain スレッド（名前で再利用・通常スレッドの並びには二重に出さない）。
  - 未任命: 同じ位置が `⚑ Captain を任命`（fg2）。押すと**会話ペインに任命の面**が出る（§5.7）。設定画面へは飛ばさない。
  - ⌘0 / サイドバーの Captain バー / このタブはすべて同じ入口（`focus_captain`）。
- **見出しの右のトグル**: `│ 編隊図  采配ログ N  変更 +N −M  ターミナル N  ファイル`（*2026-09-28*: 旧ペインバーの右から移設）。
  - **編隊図**: 既存の 4 表示（ハブ / 扇形 / ツリー / カード）をペインの中に描く。表示切替はペイン見出しのチップ、補足に `N Tasks`。
    ノード / ラベルのクリックでその Task の舞台へ。**何も選んでいない統合先は編隊図を開いておく**（`stage_side` の既定）。
    × で閉じたら勝手に開き直さない。ハブはペインいっぱいに割合座標で描き、任命済みなら中心が `⚑ Captain · <agent> · N エージェント`。
  - **采配ログ**: ニュースの Captain 行だけを時系列で（旧 Captain カードのタブを移設）。補足に「采配のみ · 統合と承認は人間が操作」。
  - 旧「Task N」（名前だけの一覧）は編隊図とサイドバーで足りるので廃止。
- ヘッダの phase ピルは Task の phase ではなく役割（`統合先 · 保護`）。「次へ」は出さない。
- transcript には人間の発話・Captain の思考（✳）・fleet コマンドの実行（⏺ / ⎿）・**台帳イベントの受信**
  （「イベント · 14:05（台帳）」の灰色カード。人間の発話と区別する・未実装）・Captain の報告が時系列で出る。
  composer の宛先は `⚑ Captain ／ プロジェクト ⎇ main`。

### 3.7 下段

変更なし（ニュース / ターミナル・高さ共有・上縁ドラッグ）。ニュースの Captain 行は丸チップ（既存の coordinator 表示）。
ターミナルタブの見出しに `· ⎇ branch`（どの worktree の PTY かを常時）。

## 4. Task の一生

### 4.1 ＋ Task ダイアログ（1 プロンプト = 1 worktree）

- 入力は **1 つ**（複数行）。1 行目から `task/<slug>` を自動生成（ASCII 化・40 字・衝突時は `-2`）。クリックで編集可。
  1 行目に英数字が無い（日本語の依頼）と slug は `task`（`task/task`・`task/task-2`…）になるので、**最初のスレッドに名前が付いた時に改名する**（O23・A23・2026-09-26）: その名前に英数字があればそこから、無ければ既定のエージェントの一発生成（スレッドの自動命名と同じ口・`agent_auto_name` が on の時だけ）で英語の句をもらって `task/<slug>`（fan-out はエージェントの slug を後ろに残す）。名前を指定した Task・英語の依頼から作った名前は触らない。改名しないのは、worktree がもうそのブランチに居ない（人が切り替えた）時と、同じ名前で push した（`branch.<名前>.merge` が自分・どこかのリモートに同じ名前）時。起点（`origin/main` 等）を追跡しているだけなら改名する（git が追跡の設定ごと運ぶ）。worktree のフォルダ名は変えない（動いているエージェントの場所を動かさない）。予約は起動の間だけ。
- 表示行: ブランチ / worktree パス（`<repo の親>/<repo 名>-worktrees/<slug>`・設定 `worktree_dir` で変更可）/
  エージェント・model · effort・承認モードのピル（sticky 規則は 2026-07-27 のまま = `default_*`）/
  準備スクリプトの有無（✓ パス表示 / 無ければ「作る」→ テンプレを `.necoder/worktree-setup.sh` に書いて開く）。
- 「詳細 ▾」（O20・実装 2026-09-26）: **ブランチ**（1 行・空 = `task/<slug>`・**既にあるローカルブランチ名なら、そのブランチの worktree を作る**＝新しいブランチは切らない）と**起点**（1 行・空 = **リポジトリの既定の起点**、それも無ければ統合先の今の HEAD・ブランチ / タグ / コミット）。リポジトリの既定の起点 = 統合先の `.necoder/settings.json` の `task_base`（例 `"origin/develop"`・O20・2026-09-26）。起点を指定せずに**新しいブランチを切る**時は ＋Task・fan-out・`ne fleet create` のどれもそこから切る（既にあるブランチの worktree には使わない）。**sparse checkout**（O20 / A24・2026-09-26）= 同じファイルの `task_sparse`（例 `["web", "packages/ui"]`・根からのフォルダ）があれば、新しい Task の worktree はそのフォルダと根のファイルだけを取り出す（`git worktree add --no-checkout` → `git sparse-checkout set --cone` → `checkout`・worktree ごとの設定なので統合先は全部のまま・既にあるブランチの worktree も同じ）。`..` と `-` で始まる物は捨てる。sparse の設定に失敗したら全部を取り出して続ける（空の worktree を残さない）。ダイアログを開いた時に 1 回読み、起点欄の説明と `⎇` 行に出す。上の `⎇` 行は選んだ名前と `（起点 …）` を映す。ブランチ名は `git check-ref-format --branch` で確かめ、使えなければ作らずにトースト。worktree のフォルダはブランチ名の `/` を `-` にした名前。準備スクリプトがあれば **「今回は準備スクリプトを実行しない」**（チェック・`.worktreeinclude` は写す）。**並べて比べる（fan-out・O23）**: エージェントのチップ（複数選択・**ログイン済みのエージェントだけ**＝composer のエージェント選択と同じ `authenticated_agent_labels`。開いた時に 1 回読み、1 つも分からなければカタログ全部）と「各 1 / 2 / 3」。選んだエージェント × 本数の Task を**順に**切る（同じリポジトリへ `git worktree add` を並走させると ref の lock でぶつかる・1 本の失敗で残りを止めない・上限 6）。ブランチは `task/<slug>-<エージェントの slug>`（本数が 2 以上なら `-<n>`・詳細でブランチ名を決めていれば `<名前>-<エージェント>`）、Task 名は `<1 行目> · <エージェント>`（`#n`）。各 Task の最初のスレッドはそのエージェント（`acquire_thread(Some(agent))`）。切れたら舞台に並べる（`stage_pinned` に最大 3 枚・列数 = 枚数）。Fleet の外ならトーストで ⌘⇧M を案内。比べるのは各カードの「変更」サイドペイン（2〜3 列のカードは幅が 900px 未満なのでペインがカードの全面に出る・専用の比較画面は作らない）。未実装: 同じ worktree にスレッドを足す（隔離しない・読むだけの用途向けと明記）。
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
コードを書かず、**necoder が同梱する Captain 用の MCP の道具だけ**で采配する（§5.8）。**状態を持たない**（記憶は台帳）。
だから交代・再起動・エージェント変更が自由で、Claude Code / Codex / Gemini / OpenCode どれでも Captain になれる。
これが Claude Code の Agent Teams や Codex 内蔵のマルチエージェントとの差別化（Captain はエージェント非依存）。

*2026-09-24*: 道具を `necoder fleet` CLI（shell で実行）から **Captain 用の MCP**（`necoder mcp --captain`・セッションへ
necoder が自動で渡す）へ移した。shell を持たせる限り、git で worktree を直接切る・main のブランチを切り替える・
ファイルを書く、という抜け道が残るため。先に実機で確かめた（/tmp の練習用 repo・Opus 5・3 回・JOURNAL 2026-09-24）:
MCP だけを持たせた Captain は 3 回とも道具だけで Task を切り、編集も shell も試さなかった。Bypass で全道具を
持たせた回も「自分では編集しません」と止まった。一方で 1 行の誤字にも迷わず worktree を切った ＝ 切る前の
人間の承認（§5.5）が要の gate になる。

### 5.2 権限表

| 操作 | Captain | 人間 | 備考 |
|---|---|---|---|
| Task を切る | **提案のみ**（`fleet_propose_tasks`） | ○（＋ Task） | ブランチと worktree は人間が承認した分だけ necoder が切る（§5.5）。承認で作られた Task は `⚑` 帰属 |
| 既存 Task にエージェントを起こす（`fleet_spawn_agent`） | ○（確認なし） | ○ | worktree もビルドも増えないので自由 |
| Task に追撃する（`fleet_send`） | ○（確認なし） | ○（介入・§5.4） | |
| 依存を宣言（`fleet_set_depends`） | ○ | ○ | |
| 待つ（`fleet_wait_task`） | **×** | | 席を塞ぐ。結果は知らせで届く（§5.3） |
| レビュー実行（`fleet_review_task` = radar） | ○ | ○ | read-only |
| phase の報告（`fleet_update_task`） | **×** | ○ | phase は担当エージェントが報告する。Captain が「担当を起こせなかった」を blocked で表した実例がある（画面では承認待ちに見える） |
| Task を終了（archived） | × | ○ | worktree は消さない |
| **Integrate** | **×** | ○ | 人間 gate（不変） |
| **承認待ちへの応答** | **× （推薦のみ・`fleet_recommend`）** | ○ | §5.5。推薦は要対応カードに添えるだけで、許可・拒否は人間が押す |
| worktree / ブランチの削除 | × | ○ | |
| ファイルの編集・shell | **×** | ○ | necoder が席の決まりで断る（§5.8） |
| 設定の変更・Captain 自身の交代 | × | ○ | |

Herdr socket 直叩き等の迂回路は Captain のツールセットに含めない（FLEET-CONTROL-PLAN §0-8 のまま）。

### 5.3 起きる条件（イベント駆動・ポーリング禁止）

| イベント | 起きる | 渡すもの |
|---|---|---|
| 人間が Captain に書いた | 即時 | 発話 + 前置き（役割・現況・直近の采配・§5.9） |
| Task が Done / review_ready / Failed | 即時 | 台帳の未読 |
| Task が Blocked | 15 秒経過後 1 回 | 台帳の未読 + 今の承認待ちの要求（推薦の案内つき・§5.5） |
| 人間が Task に介入した（§5.4） | 次の wake に同乗（単独では起こさない） | 台帳の未読（`human_send`） |
| Task が integrated | 即時 | 台帳の未読 |
| 分解案が承認 / 却下された（§5.5） | 即時 | 台帳の未読（`proposal_approved` / `proposal_rejected`） |
| necoder の起動 | 未読があれば 1 回 | 台帳の未読 |

**渡すのは「台帳の未読」**（*2026-09-24*）。Captain がどこまで読んだかの位置（台帳の通し番号）をリポジトリごとに
DB（`captain_cursors`）へ保存し、起こす時はその位置より後の出来事をまとめて 1 通にする。位置を進めるのは Captain の
ターンが**終わってから**（途中で落ちたら同じ出来事をもう一度渡す）。以前はメモリ上の待ち行列（`captain_pending`）で、
再起動すると未配達の知らせが消えていた。統合先を開いていない間の出来事も、開いた時に届く。位置が無いリポジトリ
（初めての任命）は、その時点の末尾から始める（過去の全履歴は渡さない）。1 通に載せるのは新しい方から 40 件まで
（残りは `fleet_events` で読める、と 1 行添える）。

渡す出来事: `phase_changed`（blocked / review_ready / changes_requested / merge_ready / failed / integrated / archived）・
`human_send`・`task_created`・`proposal_approved`・`proposal_rejected`。working / planned などの途中経過と、
Captain 自身の `captain`・`tier2` は渡さない（現況表と digest で足りる）。

実行中は重ねない。起こす要求は 2 秒まとめてから 1 通にし（同じ Task の連続遷移を 1 通に畳む・旧 5 秒デバウンスの実装）、
Captain が実行中ならターン終了で未読を確かめて続けて渡す。

### 5.4 人間の 2 つの玄関

- **既定 = Captain に話す**（⌘0）。目標・優先順位・やめる指示。Captain が Task に分解して提案し、承認で起動する。
- **介入 = Task に直接話す**（サイドバー行 / 舞台のカード）。composer はそのまま。送信時に台帳へ
  `human_send`（原文）を積み、ニュースに載せ、次の Captain wake に同乗させる。Captain は介入を前提に采配を続ける
  （Captain が知らないまま進む状態を作らない）。
- 1 件だけの単純な作業は ＋ Task で直接切ってよい（毎回 Captain を通すと往復が 1 段増える）。

### 5.5 承認（gate は人間のまま）

**分解案の承認**（*2026-09-24*）: Captain が `fleet_propose_tasks` を呼ぶと、necoder は worktree をまだ作らず、
要対応に**分解案カード**を 1 枚出す（見出し `⚑ Captain の分解案 · N 本`・並びは承認待ちの次）。見出しの直下に
**承認（k 本を切る）/ 却下**、その下に理由（2 行まで）と行を並べる（ボタンを先頭に置くのは、要対応の欄が高さに上限を
持ってスクロールするので、行の下だとボタンが隠れるため・2026-09-24 の実画面で確認）。行ごとに題名・目的・完了条件・
範囲・エージェント（目的と完了は 1 行に畳む・全文は Captain の会話にある）。既定で全部に印が付き、印を外した行は切らない。
承認の後は necoder が担当を起こして最初の指示を送るので、Captain は起こし直さない（役割文・道具の返り値・承認の知らせの
3 か所で伝える。実験で Captain が「承認が届いたら担当を起こします」と書いたのを受けて足した）。

- **承認**: 印の付いた行だけ、＋ Task と同じ流れ（作成中の行・O20・§3.2）でブランチと worktree を 1 本ずつ切り、準備スクリプトを回し、
  担当を起こして、目的・完了条件・範囲をそのまま最初の指示として送る。作られた Task は `⚑` 帰属（サイドバー行の題名の前）。
  作れなかった行はサイドバーに「やり直す」で残り、その行だけやり直せる（成功した行は作り直さない）。舞台には並べない（fan-out ではない）。
  裁きを DB に書けなかった時はカードが印ごと戻る（押し直せる）。*残り*: 承認の後に終了した時の再開の記録（UX-CODE-REVIEW R09）。
- **却下**: 何も作らない。
- どちらも台帳に `proposal_approved` / `proposal_rejected` を積み、Captain を起こす（§5.3）。
- 道具の返り値は「承認待ち」。Captain はそのターンを終えてよい（待たない）。
- 提案は DB（`captain_proposals`）に置くので、再起動を跨いでカードが残る。titlebar の要対応バッジにも数える
  （Editor 画面にいても気づける）。

**承認待ちへの推薦**（*2026-09-28 実装*）: Blocked で起きた Captain は「許可してよい: cargo test の実行（worktree 内・読み取りのみ）」の形で
1 行の推薦を返せる。UI は要対応カードに ✳ 付きで添えるだけで、**応答はしない**。ポリシーによる自動承認は本文書の範囲外（後段の判断）。

- **道具** = `fleet_recommend(task_id, permission_id, verdict, reason)`（Captain 版の MCP にだけ出す・§5.8）。verdict は
  `allow`（許可してよい）/ `deny`（拒否を勧める）/ `ask_human`（判断はあなたに＝取り消せない・Task の範囲の外など、人間が中身を
  見るべき）。reason は 1 行（改行なし・200 字まで）。引数は GUI に触れる前に確かめ、GUI が居なければ断る（推薦は今の承認待ちに
  だけ意味があるので DB には置かない）。返り値は「カードに添えた・応答はしない・待たずにターンを終えてよい」。
- **どの要求か**: 承認要求には 1 件ごとに id がある（`PermissionCard::id`・リモート管制の `permission_id` と同じ相関キー）。Captain は
  起こされた 1 通の末尾の「承認待ち」（`- <Task-id> <名前>: 「<要求>」 permission_id=<id>`・まだ推薦していない要求だけ・台帳ではなく
  GUI の今の状態から足す）か、`fleet_digest` の `threads[].permission`（id・文・付けた推薦）で知る。今の要求と id が違えば断り、
  今の要求を添えて返す（読んだ後に解決・取り消し・別の要求に替わった＝古い要求への推薦を新しい要求に付けない）。
- **持ち方**: GUI は「どの Task の、どの要求への推薦か」と一緒に画面の上にだけ持つ（承認待ち自体が再起動を越えないので、推薦も
  越えない）。カードは今の要求の id で引くので、前の要求への推薦は出ない。解決・取り消し・別の要求に替わった推薦は、パネルの出来事
  （次の承認待ち・ターンの終わり）と要対応カードのボタンの後に捨てる。同じ要求への 2 度目の推薦は差し替える。
- **記録**: 台帳に `captain`（Task の id・payload に見立て・理由・要求の id と文・スレッド名）を積み、ニュースに Captain の丸チップの行
  （`推薦 · <Task>: 許可してよい — 理由`・押すとその Task へ）。Captain へ渡す知らせからは kind で外れる（§5.3）。
- **Captain の席だけ**: `necoder mcp`（Full 版・人が普通のスレッドや他のツールに登録して使う版）の一覧と `ne skills get --full` には出さず、名前で呼んでも断る
  （担当が自分の承認要求に「Captain:」の見立てを付ける道を道具として渡さない）。GUI も Captain が未任命なら受けない。統合先（main）の
  スレッドの承認待ちには推薦しない（Captain が見るのは Task）。CLI の対（`ne fleet recommend`）は作らない（分解案と同じく席の MCP から
  だけ使う道具）。
- **リモート管制には載せない**（今回）: スマホの承認カードは `remote_thread` の `permission`（agent_panel・Captain を知らない）から組み、
  許可は「実行内容と変更内容を確認しました」を経てからしか押せない（tool poisoning 対策）。推薦を並べるなら、その確認との並べ方
  （読まずに許可を誘わない）を決めて、Cloudflare に別に配っている PWA（`relay/public`）と一緒に出す。データの道は
  `handle_remote_control` の `remote_thread` で `permission` に 1 項目足すだけ（ホストの bridge は中身を素通しする）なので後から足せる。

### 5.6 コストの規律

- 渡すのは事実層 + Tier1 digest + 台帳の未読だけ（既存 `fleet digest` の 3 段圧縮）。transcript は渡さない。
- Blocked は 15 秒閾値、起こす要求は 2 秒まとめて 1 通（§5.3）。
- **会話の交代**（*2026-09-24*）: Captain は状態を持たないので、会話が膨らんだら捨ててよい。起こす前に、Captain スレッドの
  文脈が上限の 40% を超えたか、前回の交代から 20 回起きていたら、同じタブのまま新しい会話へ切り替える（`session/load`
  で引き継がず `session/new`）。次の 1 通には前置き（役割・現況・直近の采配 5 件・§5.9）と台帳の未読が付くので、続きから
  采配できる。transcript には区切りを 1 行出す。
- Captain のトークンは Task 行の右端と statusbar Σ に含めて常時見せる（見えないコストを作らない）。
- Captain の采配は毎回 `task_events` に `captain` として残す（監査可能・ニュースの丸チップ）。

### 5.7 任命

設定 `captain_agent`（旧 `coordinator_agent`・プロジェクト設定 `.necoder/settings.json` でも可・既定ドリフト禁止）。
未任命の間も要対応・Task 行・＋Task は全部そのまま使える（Captain 無しでも Fleet は成立する）。

*2026-09-20*: **任命の UI を足した**。それまでは settings.json の手書きが唯一の入口で（Captain バーの文言が
「settings.json の captain_agent」・押すと Captain 節の無い設定ホームへ飛ぶだけ）、Captain の機能（wake・采配ログ・⌘0）が
実質誰にも使えなかった。

- **任命**: ブリッジの `⚑ Captain を任命` タブ / Captain バー / ⌘0 → 会話ペインに任命の面（役割の説明 +
  サインイン済みエージェントのボタン = `acp_client::authenticated_agent_labels()`）。押すと `captain_agent` を書いて
  そのまま Captain の会話へ入る。
- **設定からの任命・交代・解任**: 設定 → AI エージェントの行ごとの `Captain にする` / `⚑ Captain` ボタン（PR #8）。任命中のボタンをもう一度押すと解任。
  user の `captain_agent` を更新し、既定エージェント（★）とは連動させない。解任は既存の永続化規約に従ってキーを削除する。
  プロジェクトの `.necoder/settings.json` に同じキーがある場合は project 層が優先される。オンボーディングには任命ボタンを出さない。
  作業中だった設定のセグメントは同じ操作の重複になるため、この行ごとのボタンへ統一した。Fleet 内の任命面はそのまま残す。

### 5.8 Captain の席（道具と決まり・*2026-09-24*）

Captain のスレッドは necoder が「席」として扱う。席は AgentPanel の汎用機能（`SeatPolicy`・Fleet を知らない）で、
workspace が Captain 用の中身を詰めて渡す。

1. **道具 = Captain 用の MCP だけ**。セッション開始時に `necoder mcp --captain <統合先>` を渡し、他の MCP を
   持ち込ませない。止め方はエージェントごとに違い、`acp_client::preset::restrict_mcp_to` に閉じ込めてある:
   **Codex** は codex-acp が Codex 自身の設定（`~/.codex/config.toml`・computer-use など）の MCP サーバも立ち上げるので
   （*2026-09-25* 偽のサーバで確認）、ACP の mcpServers は空にし、`CODEX_CONFIG` で necoder の分
   （`default_tools_approval_mode = "approve"`）と「止める」指定（登録済みの名前に `enabled = false`）を一緒に渡す
   （ACP で渡すと codex-acp が `mcp_servers` を丸ごと差し込み、止める指定が上書きで消える）。
   **Claude Code** は mcpServers で渡し、`strictMcpConfig` + `settingSources` 空（`~/.claude` の許可ルール・hooks を
   持ち込まない）+ claude.ai のコネクタを止める環境変数。組み込みの道具は絞らない（MCP を読み込む `ToolSearch` まで消えうる）。道具: `fleet_list_tasks` `fleet_digest` `fleet_events` `fleet_propose_tasks` `fleet_spawn_agent`
   `fleet_send` `fleet_set_depends` `fleet_review_task` `fleet_recommend`（承認待ちへの推薦・**この席だけ**の道具で Full 版には出さない・§5.5）と、
   読むだけの `list_files` `read_file` `search` `git_status`。
   `write_file` `fleet_create_task` `fleet_update_task` `fleet_wait_task` `fleet_integrate_task` は一覧に出さず、呼ばれても断る。
   型つきの引数（`fleet_propose_tasks` の目的・完了条件は必須）が、そのまま委任文の型になる。
2. **権限モードは「聞いてくる」モードに固定**（Claude Code = `default`・Codex = `read-only`・他は広告の既定）。
   ユーザーの既定（Bypass 等）を引き継がない。
3. **許可要求には necoder が代わりに答える**（人間には何も出ない＝体感は Bypass と同じ静かさ）。
   Codex の MCP の承認は「題名なし・種別 実行・要求全体の `_meta.is_mcp_tool_approval`」で届く（codex-acp 1.13.1）。
   種別だけでは shell と区別できないので、acp_client がこの印と呼び出しの id を運び、席は先に届いた同じ id の呼び出しの
   題名（`mcp.necoder.<道具>`）で裁く。読む・探す・考える・fetch と
   necoder の道具は許可、編集・削除・移動・shell・他の道具は拒否して transcript に 1 行残す。「常に許可」は返さない
   （エージェント側が権限モードを切り替えて以後聞いてこなくなるのを防ぐ＝裁定を necoder が持ち続ける。Chat と同じ作法）。
4. **見張り**: 席のスレッドで編集・削除・移動・shell の道具が**完了した**ら（許可を求めずに実行された＝決まりの漏れ）、
   necoder がそのターンを止め、transcript・ニュース・トーストに出す。前もっては止められないが、起きたらその場で必ず気づける。
5. **席はスレッドの名前ではなく席に付く**。⌘0 と wake が選んだスレッドに付け、同じパネルの他のスレッドからは外す。
   Captain スレッドを改名したら席と前置きを外す（普通のスレッドに戻る。次の ⌘0 で新しい Captain ができる）。
6. 席が付く前から走っていたセッションは、次の送信で席の決まり（モード・MCP）を持って立ち上げ直す（会話は `session/load` で引き継ぐ）。

### 5.9 前置き（役割・現況・直近の采配）

Captain スレッドへの送信には、役割（`captain.role`）+ 現況表（`captain.facts`）+ 直近の采配（`captain.recent`・5 件・あれば）が
前置きで付く（PR #12 で人間の発話にも付くようにした）。⌘0・wake・人間の送信の後に差し替える。slash コマンドには付けない。
役割文は道具の一覧を持たない（MCP の道具が自分の説明を持つ）ので、毎回付いても短い。

## 6. worktree の運用（1 branch = 1 worktree・準備スクリプト・ビルドコスト）

### 6.1 原則

- **1 Task = 1 branch = 1 worktree**。`git worktree add -b` で同時に作る（既存）。1 つの worktree に別ブランチを 2 体は
  git の仕様上不可能。同じブランチに 2 体は「詳細」からのみ（互いのファイルを踏むと明記）。
- worktree の置き場は `<repo の親>/<repo 名>-worktrees/<slug>`（設定 `worktree_dir`）。リポジトリの外に置くので
  探索・grep・LSP が別 worktree を舐めない。

### 6.2 準備スクリプト `.necoder/worktree-setup.sh`

worktree 作成直後に 1 回、Task の worktree を cwd にして実行する。環境変数 `NECODER_MAIN_ROOT`（統合先の絶対パス）
`NECODER_TASK_ROOT`（新 worktree）`NECODER_TASK_BRANCH`。失敗（非 0）は Task を `failed` にしてログを要対応に出す。
失敗した時の依頼は**送らずに控え**、要対応の失敗カードに **「準備をやり直す」**（`.worktreeinclude` と準備スクリプトを
もう一度流し、通ったら控えた依頼を送る・また失敗したら理由を差し替えて failed のまま）と **「準備を飛ばして始める」**
（流さずに送る）を出す（O20・2026-09-26）。控えは起動している間だけ（再起動後は「直す指示」から書き直す）。
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

### 6.3 `.worktreeinclude`（O20・2026-09-26）

統合先のルートに `.worktreeinclude`（`.gitignore` と同じ書き方）を置くと、そこに書かれ、**かつ git が無視している**
追跡外のファイルを、準備スクリプトより前に新しい worktree へ写す（Claude Code・Orca と同じ約束）。`.env` のように
commit しないが Task でも要る物を、スクリプトを書かずに持ち込むための宣言。

- 一致の判定は git（`git ls-files --others --ignored --exclude-from=.worktreeinclude` と `--exclude-standard` の両方に
  出るもの）。否定 `!` やディレクトリの書き方も `.gitignore` と同じ（親をディレクトリごと書くと中の否定は効かない）
- 追跡中のファイルは checkout で入るので対象外。**無視されていない**追跡外（書きかけのソース）は写さない
- 新しい worktree に既にある物は上書きしない。上限は 500 件・合計 64 MiB（`node_modules` のような大物は準備スクリプトで）。
  超えた分は写さず標準エラーに残す（Task は失敗にしない）
- 読み書きは Host 経由（SSH の Task でも同じ。書き込みは新しい worktree を開き直した host で行う）。失敗は準備スクリプトの
  失敗と同じく Task を `failed` にしてメッセージを残す（準備スクリプトは走らせる）

### 6.4 共有フォルダ `task_shared`（O20・A05・2026-09-26）

統合先の `.necoder/settings.json` の `task_shared`（例 `["node_modules", ".venv"]`・根からのフォルダ）に書いたフォルダは、
新しい Task の worktree に**統合先の同じフォルダへのリンク**として置く（Orca の `sharedDirectories`）。依存を Task ごとに
入れ直さない（時間とディスク）ための宣言。

- 置くのは、統合先にフォルダがあり、**統合先で git が無視している**物だけ（追跡しているフォルダは checkout で入る）。
  Task に既にある物・親のフォルダが無い物（`task_sparse` の外）は置かない（標準エラーに理由）。`..`・`-` で始まる物・
  `.git` の中は読まない
- 順番はリンク → `.worktreeinclude` → 準備スクリプト（スクリプトはリンクを前提にできる。`.worktreeinclude` がリンクの
  中へ写すことはない）。「準備をやり直す」も同じ関数（置いたリンクは置き直さない）
- **中身は統合先と同じ物**: Task の中で `npm install` すると統合先のも変わる。ブランチごとに依存が違うなら使わない
  （準備スクリプトで入れる）
- リンクは git にはフォルダでなくファイルに見えるので、`.gitignore` の `node_modules/`（末尾の `/`）では無視されず、
  そのままだと `git add -A` でリンクが commit される。git が無視しないリンクは、リポジトリの共有の `info/exclude`
  （commit されない・その機械だけ・見出しの行つき）に `/<フォルダ>` を足して無視させる。それでも無視されない
  （`.gitignore` の `!` が勝つ等）リンクは消して、準備の失敗として理由を出す＝ **git が無視しないリンクは残さない**
- 片付け（`git worktree remove`・force なしで消せる）はリンクだけを消し、統合先の中身には触れない。worktree の大きさにも
  リンク先は数えない（`du` はリンクをたどらない）
- SSH の Task は接続先で `ln -s -n`（置き場にフォルダへのリンクがあっても、その中へ置かない）。Task の worktree の
  中のコマンド・リンクは **Task を開き直した host** で、`info/exclude` は統合先の host で書く（SSH の host は開いた
  project の外を cwd にも書き先にもできない）。置いてあるかはリンクをたどらずに親の一覧で見る（接続先の metadata は
  リンクをたどり、Task の外を指すリンクを「無い」と答える）。Windows のローカルはシンボリックリンク（開発者モードか
  管理者の権限が要る。無ければ準備の失敗として知らせる）
- 途中でリンクを置けなかった物があっても、置けた物を git に無視させる確かめは済ませてから失敗を返す。やり直しは前の回に
  置いたリンクも確かめ直す（無視させられなければ知らせるだけで消さない）
- Rust の `target/` のようなビルドの出力は §6.5（`CARGO_TARGET_DIR`）で

### 6.5 ビルドコストの答え（necoder 自身 = Rust・`target/` 12GB）

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
| ⌘⇧G | 編隊図を開く / 閉じる（ブリッジへ行く・§3.3） |
| ⌘⌫ / delete（サイドバー） | 選んだ Task（無ければ前面の Task・⌂ は対象外）に印を付けて片付けの画面を開く（O21・A26。消すのは画面で確かめてから。Windows は ⌘⌫ が ctrl-shift-backspace に置き換わるので Delete が本線） |
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
`captain.dest` `captain.pill_scope` `captain.role` `captain.facts` `captain.spawned_by` `captain.human_send`
`captain.recommend_allow/deny/ask_human/card/news/reply/wake_header/wake_line` `captain.recommend_err_field/verdict/line/long/no_captain/task/integration/stale/stale_current/stale_none`（*2026-09-28*: 予定の `captain.recommend` を分けた）
`captain.recent` `captain.wake_header` `captain.wake_omitted` `captain.line_created/human_send/approved/rejected` `captain.rotated` `captain.retired_name`
`captain.seat_denied` `captain.seat_violation` `captain.seat_violation_news` `captain.proposal_title/goal/done_when/scope/approve/reject`
`captain.proposal_pending_reply` `captain.proposal_err_tasks/too_many/field/agent/no_integration/resolve` `captain.facts_proposal`
`captain.delegation` `captain.delegation_scope`（*2026-09-24*: `captain.event` は台帳の未読の見出し `captain.wake_header` に置き換えて削除）
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
| **F1 サイドバー ✅ 2026-09-12** | Task 行 3 段（last_prompt を `Thread` に持つ = P1 残の回収）+ Captain 行 + 統合先行 + ⌘0/⌘1..9。他プロジェクトのグループを出さない。**残（2026-09-19 に `+N −M` は実装）**: ピン `◫` は F2 / ⚑ 帰属は F6 / 要対応は F3** | 3 Task 並走で「頼んだこと / いま何を」が各行に出る。レール切替で編隊が切り替わる |
| **F2 舞台と Task カード ✅ 2026-09-16** | `fleet_stage.rs` 新設。列数は `chrome.stage_columns`（1..=3・⌘⇧1/2/3・中央上のトグル）+ `stage_pinned`（◫）+ `stage_tabs`（Task ごとの選択面）。Task カード = ヘッダ（● 名前 ⎇ branch・phase ピル・**次へ**・⤢・⋯）+ Task タブ行（スレッド / 変更 / ターミナル N / ファイル / ＋会話 / ＋端末）+ 本体。*実装時の訂正*: `StageLayout` enum ではなく列数の整数、`FleetPane` は残して `fleet_cells` を「Task の中身の実体一覧」として使う（配置の正は `stage_cards()` が毎 render 導出）。拡大 ⤢ = 1 列化（`fleet_maximized` とサムネイル列は削除）。旧 ＋ACP/＋Terminal/＋Worktree バーと `fleet.add_*` キーを削除。**残: サイドペイン幅の永続化**（`+N −M`・幅ドラッグ・スレッドの ×・列数トグルの同居は 2026-09-19 に実装。統合先のスレッド非表示は §3.2 の訂正で取り下げ）。*2026-09-19 作り直し*: Task タブ行を**ペインバー**に（§3.5。左 = スレッドタブ + ＋ / 右 = サイドペインのトグル・カード幅 900px 以上は「会話 \| サイドペイン 44%」の分割・未満は全面）。`＋▾` 案は廃止（足す場所を文脈に置いた）。状態は `stage_tabs` のまま（会話を指せばサイド無し）。Captain カードも同じ器（`thread_tab` / `side_toggle` / `side_header` / `render_card_body` を共用）（2026-09-19: タブ行をスレッド色の `●` + 区切り + 上線 2px に整え、ターミナルの数字を Task 内の通番にした。`fleet.stage_columns` の `%` 落ちも修正）。*2026-09-28 作り直し*: ペインバーを 2 つに割った — スレッドタブ行（`render_thread_tabs`）は会話の列の頭へ、サイドペインのトグル（`render_side_toggles`）は Task 見出しの右へ（§3.5・会話の AI と変更 / ファイルを同じ段に並べない）。見出しの右側は 1 つの塊で、狭いカードでは 2 行目へ折り返す | `fleet_panes_keep_their_threads_and_remote_targets` に「ピンしても同じ Task は 1 枚 / 幅不足で列を落とす / 端末を足してもカードが増えない」を追加 |
| **F3 要対応をサイドバーへ・中央タブ廃止 ✅ 2026-09-16** | `render_stage_attention`（Captain バーの直下・選択リポジトリの分だけ）が `render_attention_card` を再利用。サイドバー全体を `FleetControl` key context にして ⏎ / ⌘⇧U（`ControlNext`）が効く。管制タブ本体（ヘッダ / Captain バー / 稼働カード / パイプライン）・中央タブ帯・`ToggleControl`・`NECODER_CONTROL`・`fleet_mascot`・`control.*` の死キー 26 個を削除。*実装時の訂正*: `FleetCenterView` enum は `Work` が `workbench.rs` に残るため **F7 まで据え置き**（描画経路からは外れている） | 全 test green・`cargo check --workspace --all-targets` の警告は workbench 由来 5 件のみ（F7 で消える） |
| **F4 ＋ Task ダイアログ + 準備スクリプト + task.env ✅ 2026-09-16** | `new_task_dialog.rs`（⌘N / サイドバーの ＋Task）: 1 入力（EditorView・⌘⏎）→ `project::task_slug` → `create_named_task_on`（`<repo>-worktrees/<slug>`・衝突は `-2`）→ `run_task_setup_on`（`.necoder/worktree-setup.sh`・失敗は `failed` + 要対応）→ `ipc_spawn_into` で 1 行目を Task 名にしてプロンプト送信。`host::task_environment` が `.necoder/task.env` を ACP（`acp_client`）とターミナル（`terminal_view`）の環境に注入。ダイアログに branch / worktree パス / 準備スクリプトの有無（無ければ「作る」= §6.2 テンプレを書いてエディタで開く）。CLI の `fleet create` も同じ関数。*未実装*: エージェント/model ピル（sticky 既定が自動で効く）・「詳細 ▾」・設定 `worktree_dir` | `task_slug` / `task_worktree_dir` / `task_environment` の unit test。**necoder 自身での `CARGO_TARGET_DIR` 共有の体感（2 本目の `cargo check`）は本人の実機の手番** |
| **F5 系譜の帯 ✅ 2026-09-16** | 畳んだ系譜ヘッダの下に `render_lineage_strip`（Task 色の `┬━ 名前` / 統合済み `╰━` / main `━`・クリックで舞台の選択へ）。⌄ で従来 4 表示に展開・`ToggleLineage`（⌘⇧G・非 mac は ctrl-alt-g = Git パネルとの衝突回避）。*実装時の訂正*: 曲線描画の 64px 帯ではなくテキストの 1 行（34px）から始めた。既定は畳み | 帯の名前クリックで舞台が切り替わる（offscreen 目視は本人の手番） |
| **F6 Captain の玄関（一部 2026-09-16・*2026-09-20*: Captain カードはブリッジ（§3.6）に統合・任命 UI と設定の交代/解任を追加（§5.7））** | 済: Captain カード（⌘0 → `captain_space`・タブ = Captain スレッド / 采配ログ / Task 一覧）・`captain_pending` に台帳イベントを溜めて busy 明けにまとめて 1 通（`send_ledger_event`・`captain_facts` の現況表を同乗）・Task への直接発話を `human_send` として台帳 + ニュース + Captain へ・`integrated` でも wake。*2026-09-24*: **Captain の席（§5.8）と分解案の承認（§5.5）**・台帳の未読と読んだ位置（`captain_cursors`・§5.3・`captain_pending` は削除）・会話の交代（§5.6）・前置きの直近の采配（§5.9）・`⚑` 帰属・知らせが表示中のタブを奪わない（`send_ledger_event_to`）・ターン終了の通知に `completed`（失敗したターンで読んだ位置を進めない）。*2026-09-28*: **承認待ちへの推薦 ✳**（§5.5・Captain の席だけの `fleet_recommend`・要対応カードの 1 行・どの要求への推薦かを id で持つ・Blocked の知らせに今の要求と permission_id）。残: 台帳イベントの灰色カード表示・トークン表示 | storage / agent_panel / workspace / necoder の test（読んだ位置と分解案の往復・席の裁定・見張り・交代・MCP の Captain 版・台帳の未読が 1 通で届き表示を奪わず正常完了で位置が進む・承認で印の行だけ本物の worktree + `⚑`・推薦の道具は Captain 版だけで引数を GUI の前に確かめる・推薦は今の要求にだけ付き解決と差し替えで消え古い id は断る・Blocked の 1 通に permission_id）。隔離 offscreen で推薦つきの承認待ちカード（allow / deny / ask_human）を目視。実機: /tmp の練習用 repo で Opus 5 が「分けて提案 → 承認待ちで止まる → 承認の知らせで起こし直さない」。隔離 offscreen で分解案カードと `⚑` 行を目視。**necoder 本体での実 e2e（目標 1 つ → 承認 → 2 Task）は本人の目視待ち** |
| **F7 掃除 ✅ 2026-09-20** | `workbench.rs` / `work_layout.rs` / `FleetCenterView` / `work.*` の死キー 37 個 / `NECODER_WORKBENCH_PROBE` を削除。窓の状態の復元は `restore_window_state`（`persisted_state` の対・`explorer_controller.rs`）へ改名して残し、保存形式から `work_layout` / `fleet_view` を外した（旧 payload の余分なキーは serde が無視する）。端末の名札は `chrome.next_terminal_id`（プロセス内単調増加・PTY は再起動を越えないので保存しない）。生きていた 3 キーは `settings.pref_tabs_position` / `tabs_position_top` / `tabs_position_left` へ。設定キー `work_tabs_position` の名前は未決のまま据え置き。*2026-09-28*: UI-SPEC §11 を FLEET-V2 の要約へ全面置換し、FLEET-CONTROL-PLAN P3 に廃止の注記を足した（ついでに ⋯ の「セルを閉じる」と閉じた時のトースト「herd から戻せます」を「カード」「サイドバーの Task 行」へ） | `cargo check --workspace --all-targets` **と `--release`** で警告 0（`WorkAction` の未使用警告は debug 専用プローブからしか構築されず、release ビルドでだけ出ていた） |

順序は F0 → F1 → F2 → F3 → F4 → F5 → F6 → F7。F1 と F5 は独立。**各フェーズの終わりに本文書と JOURNAL を更新**。

*2026-09-27（parity の統合）*: この作り（F0〜F7 + Captain の席・分解案）を parity の統合ブランチへ合わせた。parity で足した Fleet の機能は
この作りの置き場へ載せ直した — 変更レビュー（O6）は「変更」サイドペイン、並べて比べる（O23）は列数トグル（最大 3）と各カードの「変更」、
ピンと列数の保存（O21）は `restore_window_state`、作成中の行（O20）は ＋ Task・fan-out・分解案の承認の共通の流れ。経緯と分担は
`ORCA-PARITY.md` §7.3、確かめたことは `UX-CODE-REVIEW.md` R09 / R10。

F0 で追加した i18n キーは `captain.title`（Captain バーの見出し・ニュースの帰属名・**Captain スレッドの表示名**を兼ねる）/
`captain.appoint`（未任命の行）/ `captain.prompt`（wake テンプレート）の 3 つ。§8 の残りは使う面（F1/F6）と同時に足す。
*2026-09-24 訂正*: `captain.prompt` は `captain.role`（役割・規律・道具）/ `captain.facts`（現況表の見出し）/ `captain.event`（wake のイベント行）に分割。
役割 + 現況は Captain スレッドの prompt context として**人間の発話にも**前置する（§5.3「発話 + 現況」。以前は wake にしか付かず、
人間から始めた Captain が役割を知らずに自分で作業していた）。道具一覧に `fleet create` を追加（無いと Task を切れない）。
実装は `/goal` の規律（現在地把握 → 1 歩 → 検証 → 文書）で進める。ドッグフーディング中はスクショを回さず本人目視。

## 10.5 実画面の検証（稼働中の本体に触れない隔離 offscreen）

**`cargo check` / `cargo test` では見た目のバグは出ない**。F2 の「変更」が実機で空だったこと・Captain に任命の入口が
無かったこと・展開した系譜がカードを潰すことは、どれも実画面を撮って初めて分かった（2026-09-20）。ドッグフーディング中は
別インスタンスを立てない決まりだが、**データ・ソケット・書類フォルダを全部隔離した offscreen 撮影**なら本体に触れない
（`CHAT.md` §7 と同じ流儀。窓は出ず、撮ったら自分で終了する）。

```sh
ISO=$(mktemp -d); mkdir -p $ISO/home $ISO/docs $ISO/project
(cd $ISO/project && git init -q -b main && echo 'fn main() {}' > main.rs && git add -A && git -c user.email=p@e -c user.name=p commit -qm init)
echo '{"onboarded":true,"agent_prewarm":false,"reduce_motion":true}' > $ISO/home/settings.json   # "captain_agent":"Claude Code" を足すと任命済み
cargo build -p necoder --features screenshot
NECODER_HOME=$ISO/home NECODER_GUI_SOCK=$ISO/gui.sock NECODER_DOCUMENTS_DIR=$ISO/docs \
  NECODER_SCREENSHOT=$ISO/fleet.png NECODER_SCREENSHOT_DELAY_MS=5000 NECODER_WINDOW_SIZE=1680x1000 \
  NECODER_CONTROL_PROBE=1 NECODER_FLEET_PROBE="graph;task:1;side:diff" ./target/debug/necoder $ISO/project
```

`NECODER_CONTROL_PROBE=1` が 5 本の擬似 Task（稼働 / 承認待ち / merge_ready / 失敗 / 待機）を仕込み、`NECODER_FLEET_PROBE` が
画面を組み立てる（`;` 区切り・debug ビルド限定）:

| 命令 | 何をするか |
|---|---|
| `graph` | Fleet に入り Task セルを seed する（最初に置く） |
| `task:<n>` | n 本目の Task を舞台の選択にする（無ければブリッジのまま） |
| `side:<diff\|terminal\|files>` | 選択中のカードのサイドペインを開く（それ以外の値で閉じる） |
| `formation` | ブリッジへ行って編隊図を開く / 閉じる（`NECODER_GRAPH=fan\|tree\|card` で表示を選ぶ・既定ハブ） |
| `captain` | ⌘0 と同じ（未任命なら任命の面・任命済みなら Captain の会話） |
| `proposal:<n>` | Captain の分解案を n 行（既定 2・2 行目は印を外した状態）で要対応に仕込む（DB には書かない） |
| `origin` | 1 本目の Task に `⚑` 帰属を付ける（分解案の承認で作られた Task の行の見た目） |
| `recommend[:allow\|deny\|ask_human]` | 承認待ちの最初の Task（CONTROL_PROBE の「バグ #412」の `cargo publish`）に Captain の推薦を仕込む（§5.5・既定 deny・DB には書かない。ニュースにも行が出る） |
| `editor` / `threads` | Fleet から Editor へ戻る（titlebar の `Editor` と同じ道）/ レールの AI スレッド一覧を押す（`graph;editor` で「戻った直後の左カラム」を撮れる） |
| `columns:<n>` / `pin` | 舞台の列数 / 選択中の Task をピン |
| `filter:<語>` / `select:<n>` | サイドバーの絞り込み欄に語を入れる / n 本目の Task を複数選択に足す（O21・`select:1;select:3` で帯が出る） |
| `creating` | ＋ Task の作成中の行を段ごとに仕込む（O20・worktree は作らない） |
| `compare:<n>` | 先頭から n 本（2〜3）の Task を舞台に並べ、各カードの「変更」を開く（O23 の並べて比べる） |
| `menu` / `rename` / `maximize` / `terminal` / `tall` / `close-all` | 従来の片付け UI・下段の検証 |

## 11. 今回確定した判断（DECISIONS に転記済み）

1. Fleet は Task を主語にした 1 画面（左 = 一覧 / 中央 = 1 枚 / 並べるのは明示で 3 まで）。
2. 「監督」は **Captain** に改名し、Fleet の中核に置く。人間の既定の玄関は Captain、Task への直接発話は介入として台帳に残す。
3. ターミナルは Task の中のタブ。エージェントの起動は ACP が既定で、CLI 形態は明示選択のみ（DECISIONS ⑥ のまま）。
4. 1 Task = 1 branch = 1 worktree。同じ worktree に 2 体は「詳細」からのみ。
5. サイドバーはレールで選んだ 1 プロジェクトの編隊だけ。他プロジェクトはレールのドットで足りる。
6. Rust のビルドコストは `CARGO_TARGET_DIR` 共有を既定（task.env）。並列が痛くなったら sccache。
7. 管制タブ・作業タブは廃止。要対応はサイドバー常設。管制の全画面版はリモート管制（P9）の正として残す。

**残る判断**（実装中に本人に聞く）: 承認のポリシー自動化（§5.5 の先）/ Captain の推薦を要対応カードで
ワンクリック適用にするか / 推薦をスマホ（リモート管制）の承認カードにも出すか / `worktree_dir` の既定パス / solo のタブ向き設定の名前。
