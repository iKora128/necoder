# ORCA-PARITY — Orca に全部勝つための実装計画（O フェーズ）

作成: 2026-09-26（案）。**本人の承認前**に、本人の指示（「起きたら出来上がっている状態に」）で Wave 1〜2 の一部に着手した（§7）。
根拠は全数調査 [`research/orca-gap-2026-09.md`](./research/orca-gap-2026-09.md)（226 項目）と、その証拠 [`research/orca-gap-2026-09-evidence.md`](./research/orca-gap-2026-09-evidence.md)。
この文書は「どの順で・どの単位で・どう作るか」を決める。受入条件は、承認後に要約を ROADMAP の M17 として転記する。

**この文書の立場**:
- 設計・UI・用語の正は従来どおり `ARCHITECTURE.md` / `UI-SPEC.md` / `GLOSSARY.md`。各 PR はそちらを先に直す。
- FEATURES のタグは本人の管理。この計画はタグを変えない。never とぶつかる項目は §6 で判断を仰ぐ。

## 0. 一言で

- 調査で「有」以外だった 209 項目（一部 105・無 100・方式差 4）を、**PR 45 本（O1〜O45）** に割り振った。行き先は付録 A。
  - 作るもの: 179 項目。うち 15 項目は、FEATURES のタグの判断が済んでから作る（O10・O31〜O33・O41・O42。§6）。
  - やらないもの: 19 項目（never・思想差・本人判断。§5）。
  - 本人の判断待ち: 6 項目（過去の決定とぶつかるもの。§6）。
  - 方式差のまま: 5 項目（調査の 4 項目と、PDF を OS の表示で見る D06）。
- 順番は「壊れている所 → 比較の土俵（S 群）→ 毎日の差（A 群）→ 判断待ちの大物」。
  - Wave 1（O1〜O4）: 不具合、`/` 補完、端末の基本、終了の確認。
  - Wave 2（O5〜O10）: 変更レビュー、行コメント、localhost プレビュー、Design Mode、Ports。
  - Wave 3（O11〜O30）: 通知、使用量、履歴、Git パネル、Task と Fleet、端末の続き、エディタ、設定。
  - Wave 4（O31〜O45）: GitHub、閉じても走る daemon、SSH、スマホ、CLI、Skills、定期実行ほか。
- PR にしたのは **O1・O2・O3・O4・O6・O7・O8・O9・O11・O12・O28・O30・O39・O40**（#14〜#28）、実装中は **O15・O18**（§7）。夜の間に 9 本を並行で走らせたが、全部が利用上限に当たって書き始める前に止まったので、2026-09-26 の昼から 3 本ずつ回している。
- 重くしない規律（§1.4）を全 PR に課す。常駐する部品を増やさず、ポーリングを入れず、既定は off にする。

## 1. 進め方（全 PR に共通）

### 1.1 着手の前に片付けること（O0・コードは書かない）

1. **main の作業ツリーの未コミット差分を main に入れる。**
   - main の作業ツリーには、別スレッドの作業（Captain の席・分解案・エージェントの導入など）が未コミットで残っている。規模は 52 ファイル・+4,702 / −1,691 行と、新規ファイル 4 本（2026-09-26 の `git diff --stat`）。
   - この計画の PR はすべて `origin/main` から切っている。その差分が後から main に入ると、`agent_panel.rs`・`workspace.rs`・`fleet_stage.rs`・`project.rs` などでぶつかる。
   - どちらを先に入れるかは本人が決める。入れた後に、残りの PR を統括（私）が rebase する。
2. **§6 の判断をもらう。** FEATURES のタグと、過去の決定とぶつかる項目。
3. **この計画を承認してもらってから、ROADMAP と CLAUDE.md に載せる。**
   - ROADMAP に M17 として要約と受入を足す（/goal が拾えるように）。
   - CLAUDE.md の一次資料の表に、この文書の行を足す。
4. **計測の基準を取り直す。** 起動時間・idle RSS・release バイナリの大きさ。手元の数字（122MB / 215ms）は 2026-07-17 のもので古い。

### 1.2 ブランチと PR

- **作業場所は、流れごとに別の worktree。** `../necoder-parity-<名前>`（例 `../necoder-parity-review`）。Chat モードの `../necoder-chat` と同じ流儀で、本人が necoder を動かしている main の作業ツリーでは checkout しない。
- **1 PR = 1 ブランチ = §2 の表の 1 行。** ブランチ名は `parity/o06-review` のように番号を付ける。
  - 流れの違う PR（例: 端末と Git）は、どれも `origin/main` から切る。どの順にマージしてもよい。
  - 同じ流れで前の PR に依る PR（例: O7 は O6 の上）は、前のブランチの上に積み、PR の base を前のブランチにする。前がマージされると GitHub が base を main に付け替える。
- **ぶつかりの扱い。** 流れの違う PR が同じファイル（`locales/*.yml`・`workspace.rs`・`agent_panel.rs`）を触ることはある。1 本マージされるたびに、統括が残りを rebase して CI を通し直す。
- **PR は統括が push して作り、本人がマージする。** CI の必須チェックが通ってから。main へ直接 push はしない（ruleset の迂回になる）。
- **ビルドの成果物は worktree ごと。** 本体の `target/` を触らない。necoder の debug ビルド（`--features screenshot`）は、28 コアで初回 1 分 15 秒だった（2026-09-26・`../necoder-parity`）。

### 1.3 1 本の PR の中身（/goal と同じ規律）

1. **文書を先に直す。** 見た目が変わるなら UI-SPEC と mock、呼び名が増えるなら GLOSSARY、キーが増えるなら UI-SPEC のキー表。
2. **実装する。** UI 文字列は `t!` を通し、ja / en の両方に書く。
3. **検証する。** 関係する crate の `cargo check` / `cargo test` / `cargo fmt --check`、最後に `cargo check --workspace --all-targets` で警告 0。
4. **実画面を撮る。** 隔離 offscreen（`NECODER_HOME` / `NECODER_GUI_SOCK` / `NECODER_DOCUMENTS_DIR` を一時ディレクトリへ・手順は FLEET-V2 §10.5）で撮り、目で確かめる。稼働中の本体には触れない。
5. **PR 本文を書く。** 何が変わったか・受入の確かめ方・撮った画面・確かめられなかったこと・残したこと。
6. **記録する。** マージの後に ROADMAP の受入にチェックを入れ、JOURNAL に 1 項目足す（JOURNAL と ROADMAP は複数の PR で同時に触るとぶつかるので、統括がまとめて書く）。
- 本人の実機への反映（`scripts/install-mac.sh`）は、本人が回す。
- 実エージェントへプロンプトは送らない（本人のサブスクに課金される）。`session/new` までの起動は可。

### 1.4 重くしない規律（「デカくなりすぎる」への答え）

- **常駐させない。** 新しい機能は、画面を開いた時かイベントが来た時にだけ動く。定期ポーリングは入れない。GitHub の状態も、画面を開いた時・push の後・手動の ↻ でだけ取りに行く。
- **既定は off。** ネットワークに出るもの（使用量・GitHub）、常駐するもの（daemon）、OS の権限が要るもの（Computer Use）は、設定で入れた人にだけ効く。
- **閉じ込める。** 大きい機能は新しい crate か周辺の crate に置き、エディタのコア（`editor_core` / `editor_view`）に機能を知らせない（CLAUDE.md の依存方向）。
- **測る。** 機能を足す PR は、起動時間・idle RSS・release バイナリの大きさを PR 本文に書く。O0 で取る基準から 5% を超えて悪くなったら、理由を書いて本人に判断してもらう。
- **常設の UI を増やさない。** 常に見える場所に足すのは S 群だけにする。ほかはパレット・右クリック・設定から入る。

### 1.5 ライセンス

- Orca は MIT。手法もコードも参考にしてよいが、参考にしたファイルの冒頭に出典（`stablyai/orca@646e9a5` のパス）を書く。コードを持ち込んだら、MIT の表示を `THIRD_PARTY_NOTICES.md` に足す。
- Zed の GPL アプリケーション crate は、今まで通り写さない（CLAUDE.md）。GPUI の API を読むのは可。
- 新しい依存は、ライセンスを確かめてから入れる（CI の `audit-deps` と `scripts/check-license-boundary.sh`）。

## 2. フェーズと PR

規模の目安: **S** = 〜300 行・1 日以内 / **M** = 〜1,000 行・2〜3 日 / **L** = 〜2,500 行・1 週間 / **XL** = それ以上・先に設計文書。
実績から見ると、Fleet v2（F0〜F7）は 8 日、Chat モード（C0〜C6）は 2 日で入った。律速は本人のレビューと実機確認になる。

### Wave 1 — 壊れている所と、すぐ効く所

| # | 中身（調査 ID） | 作り方の要点 | 受入 | 規模 |
|---|---|---|---|---|
| **O1** 不具合 | 調査 §3-1〜7・9。Git 5 件（フックが走らない・行を押しても diff が開かない・Fleet の diff が空・失敗が見えない・コミット欄の ⌘V と IME）、設定ファイルの上書き（下記）、MANUAL のずれ 2 件、死にコード | 利用者が押したコミット / push だけフックを走らせる経路に分ける（自動の status 系の防御は残す）。Fleet の「変更」は Task の base と比べる。失敗はトースト。コミット欄は EditorView の平坦モードへ。**settings.json が JSON として読めない時に、設定画面のトグルがファイル全体を 1 キーだけで上書きする**（`crates/settings_core/src/settings_core.rs` の `persist_user_value`）のを、上書きせずエラーにする | フックの付いた repo でパネルからのコミットが止まり、理由が出る。行を押すと diff。コミット済みの Task でも diff が出る。壊れた settings.json は上書きされない（test） | M |
| **O2** `/` 補完と会話名 | B18・B31・B27（`/goal`）・調査 §3-8 | ACP の `AvailableCommandsUpdate` と `SessionInfoUpdate` を捨てずに拾い、**待機中も update を読む**（今はターンの間は読まない）。composer の行頭 `/` で候補（名前・説明・hint）。slash を送る時は `/...` を先頭ブロックにする（今は `@path` と画像が前に付き、Claude が slash と判定しない）。エージェントの会話名に追従（手で改名したものは上書きしない） | Claude Code のスレッドで `/` を打つと組み込み・自作・MCP の prompt が出る。Codex では `/goal` や skills | M |
| **O3** 端末の基本 I | C01・C07・C08・C09・C10・C12・C17（と C19 の一部） | Shift+Enter（kitty keyboard protocol と、それ以外は `\x1b\r`）・Alt・F キー・修飾付き矢印。`"Terminal"` のキーコンテキスト。端末内検索（`RegexSearch`）。OSC 52。URL と OSC 8 のリンク（`localhost:3000` と `[::1]` の検出も直す）。下線・取り消し線・DIM・カーソル形状。マウス報告（SGR）。ファイルのドロップ。右クリックメニュー。OSC のタイトルをタブ名に | TUI のエージェントで Shift+Enter が改行になる。⌘F で端末内を検索できる。`http://localhost:5173` を押すと開く | L |
| **O4** 終了の確認と「隠して動かし続ける」 | C21 と S5 の一部 | 実行中のエージェント（Working / Blocked）か、前面でプロセスが動いている端末がある時の ⌘Q・最後の窓閉じ・端末タブの × で確認する。選択肢は「隠して動かし続ける（`App::hide`）/ 終了 / キャンセル」。何も動いていなければ聞かない。設定で切れる | エージェントの実行中に ⌘Q を押しても、確認から「隠す」を選べば止まらない | S〜M |

### Wave 2 — 比較の土俵（S 群）

| # | 中身 | 作り方の要点 | 受入 | 規模 |
|---|---|---|---|---|
| **O5** Ports とポート転送 | X1・G08・F10 | 端末の pid を控え（alacritty の `Pty::child()`）、**パネルを開いた時と端末に URL が出た時だけ** `lsof -nP -iTCP -sTCP:LISTEN` を走らせ、pid の cwd で Task に割り当てる。止める = 所有を確かめて SIGTERM。SSH は remote server に一覧の request を足し、ControlMaster の `-O forward` で転送（再接続で張り直す）。O3・O8 の後 | Task の端末で dev server を立てると Ports に出て、開く（O8）/ 止めるができる。SSH の Task でも同じ | M〜L |
| **O6** 変更レビュー画面 | E01〜E04・D12・D13 | EditorView を拡張せず新しいビューにする（可変高の仮想リスト）。`project` に base 指定のファイル単位パッチ・`git show <rev>:<path>`・merge-base を足す。+/- の行背景・旧 / 新の行番号・シンタックスハイライト・変更の無い領域の畳み・空白の無視・左のファイルツリー。基準は Task の base / HEAD / ブランチ / コミット。入口は Fleet の「変更」・ソース管理・パレット | 3 ファイル変えた Task で、全 diff が 1 画面に色付きで並び、基準を切り替えられる | L |
| **O7** 行コメントと注記トレイ | E06〜E08（S1） | 行の ＋ か `c` で行・範囲に Markdown のコメント（入力は EditorView の平坦モード）。注記は storage に残す（再起動を越える）。トレイから宛先のスレッドを選び、`path:行` と抜粋つきの **1 通**にまとめて、表示を奪わずに送る（実行中ならキュー）。解決 / 未解決だけ再送 | 3 か所にコメントして「送る」で Task のスレッドに 1 通で届く | L |
| **O8** localhost プレビュー | F01（localhost に限る）・F02・F04・F08・F14・F16・D10・E33 | 新しいタブの種類。読めるのは `localhost` / `127.0.0.1` / `[::1]` だけで、ほかは既定のブラウザへ回す（汎用ブラウザにしない）。戻る / 進む / 再読込 / ズーム / ビューポート / DevTools。URL を開く関数を 1 つにまとめ、トランスクリプト・Markdown プレビュー・（統合時に）端末のリンクをそこへ通す | dev server の URL を押すと necoder のタブで開き、外部サイトは既定のブラウザで開く | M |
| **O9** Design Mode | F06・F07 | 調査 §2 のとおり。picker を初期化スクリプトで入れて nonce で有効化。IPC は Web タブにだけ付け、Design 中かつ nonce 一致のものだけ受ける。WKWebView の `takeSnapshotWithConfiguration` で切り抜き。要素チップ（PNG + HTML・計算済みスタイル・ソース位置）を composer に付ける。React 19 は `_debugStack` | ページのボタンを選ぶと、スクショと HTML・CSS がスレッドの composer に付く | L |
| **O10** プレビューを操作する道具 | F09 | necoder の MCP に `preview_open / snapshot / click / fill / console / screenshot` を足し、GUI の制御 IPC 経由で localhost の Web タブだけを操作させる。通常のスレッドにも necoder の MCP を渡す設定を足す（今は Captain だけ）。**§6 の判断待ち** | エージェントがログイン画面を操作して、スクショ付きで答える | M |

### Wave 3 — 毎日の差（A 群）

| # | 中身 | 作り方の要点 | 規模 |
|---|---|---|---|
| **O11** 使用量・レート制限・Stats | B15・B16 | **資格情報に触れない。** Claude は ACP の usage_update に付く `_meta["_claude/rateLimit"]`（status・resetsAt・rateLimitType・utilization）と PromptResponse の `_meta.quota`。Codex は `_meta.quota` と、画面を開いた時だけ `codex app-server` の `account/rateLimits/read`。statusbar に「5h 42% · 週 18%」（80% で警告）。Stats は ACP の `cost` を turn ごとに台帳へ + ローカルの JSONL の集計（既定 off） | M〜L |
| **O12** 通知 | H01・H03・B29 の一部 | GPUI の `show_system_notification`（完了・承認待ち・質問待ち。窓がアクティブでない時だけ・ミュートを守る・押すとそのスレッドへ）。**質問（elicitation）が音だけで要対応にもトーストにも出ない**のを直す。ミュートを保存する（今は再起動で消える）。Dock バッジ（objc2-app-kit） | M |
| **O13** 通知の続き | H02・B30・G19・H04・B17・H06・H27 | ベル（通知の履歴・未読に戻す）・ニュースのクリック移動。スマホへ完了の push（relay の host の絞り込みに完了を足す）。音量と音の選択の UI。エージェント実行中のスリープ抑止。メニューバーの項目（NSStatusItem）。macOS の権限の案内 | M |
| **O14** アカウント切替 | B14・G27 | 口座ごとのディレクトリを作り、ログインは公式 CLI（`CLAUDE_CONFIG_DIR=… claude auth login` / `CODEX_HOME=… codex login`）。切替は新しいセッションの起動時の env（既存の `agent_servers.<id>.env`）。資格情報はコピーも上書きもしない | M |
| **O15** セッション履歴 | B09・B10・B11・B23・G30 | ACP の `session/list`（Claude・Codex は CLI で作った会話も出る）を履歴に出し、`session/load` で再開して**再生された履歴を描く**（今は捨てている）。全スレッドの全文検索。「新しいセッションで続ける」（Captain の会話の交代を一般化） | M〜L |
| **O16** エージェントの設定 | B02・B07・B28・E16 | 有効 / 無効、権限の既定と起動引数の UI、エージェント別の新規スレッドのキー、repo ごとの AI アクションのレシピ（`.necoder/` の定型プロンプト） | M |
| **O17** transcript の仕上げ | B12・B19・B20・B22・B27 | サブエージェントを子の行に（Claude は `_meta.claudeCode.parentToolUseId`）。質問カードの自由入力。message rail。ツールのまとめ表示と個別停止。Codex の goal の表示 | M〜L |
| **O18** Git パネルの基本 | E10・E12〜E14・E31・E32・E34 | discard・amend・force-with-lease・Sync・状態で変わる主ボタン・フック失敗を AI に直させる・ブランチ行の ↑↓ と +/-・署名 | M |
| **O19** conflict の解決画面 | E05・E15 | 3-way の画面・Abort・conflict を AI に渡す | L |
| **O20** ＋Task の自由度 | A01〜A06・A08・A24・A28 | 起点（ブランチ・コミット・リモート）・ブランチ名・絵文字の名前・repo ごとの base・`.worktreeinclude`・準備スクリプトの run / skip・sparse checkout・進捗 / キャンセル / Retry・どこで動かすか（local / SSH） | L |
| **O21** Fleet サイドバーの整理 | A07・A09〜A15・A25・A26・B04 の一部 | ピンの保存・手動の休眠・複数選択の一括操作・フィルタ・並べ替え・検索・外部 worktree の表示切替・削除キーと名前のコピー・外で消えた worktree の片付け・詳細・親子の表示 | L |
| **O22** 片付け画面と Resource Manager | A16・H21 | 状態・活動・サイズで一括削除。Task ごとのプロセスとメモリ | M |
| **O23** fan-out と比較・ブランチの自動改名 | A27・A23 | 1 つの依頼を N 体へ（エージェントを選べる）→ O6 のレビュー画面で並べて比べる。自動命名のブランチを作業内容から改名 | M〜L |
| **O24** 端末の基本 II | C02・C03・C13・C18・C19 | 分割（入れ子）・ドラッグで分割・エディタ領域の端末タブ・フローティング端末・キー・改名 | L |
| **O25** 端末の設定とシェル統合 | C11・C14・C16・C20・C22・C24・B25 | フォント / 大きさ / scrollback / カーソル・ANSI 16 色とテーマの取り込み・シェルと引数・Quick Commands・OSC 133（vte が捨てるので PTY のバイト列を自前で読む）・TUI エージェントの状態推定 | L |
| **O26** エディタの所作 | D01・D02・D15・D28・D29・H22 | 自動保存・プレビュータブ・パス:行のコピー・全部閉じる / ピン・Open in（VS Code / Cursor / Zed / ターミナル） | M |
| **O27** 設定 | H08〜H12・H23・H25・H26・H29・D14・D22・D23 | 設定の検索・キー割り当ての GUI と衝突の警告（keymap.json から消した束が再起動まで残る不具合も）・UI ズーム（GPUI の `rem_size`。`px(` が 2,348 か所あるので段階的に）・UI フォント・density・言語の切替・statusbar の項目・アプリアイコン・ヘルプ | L |
| **O28** エクスプローラと検索 | D16・D18〜D21・D25・H30 | 自然順・右クリックの stage / discard・フォルダ内検索・⌘P の recency と ignored・omnibox / jump palette・ツリーの仮想化・ファイル操作の undo | L |
| **O29** Markdown と表 | D03・D04・D05・D08・D17 | 横並びのライブプレビュー（WYSIWYG は作らない）・スラッシュメニュー・表 / 目次 / front matter・Mermaid・CSV / TSV の表・画像のドロップ・Markdown の注記（O7 のトレイ） | L |
| **O30** 言語・ミニマップ・画像 diff・行単位の帰属 | D26・D11・D07・E09 | tree-sitter の言語を足す（Java・C#・Ruby・PHP・Swift・Kotlin・SQL・Dockerfile など。バイナリの大きさを測りながら）・ミニマップ・画像の diff（swipe / onion）・エージェントが書いた行の帰属 | M〜L |

### Wave 4 — 判断待ちと大物

| # | 中身 | 作り方の要点 | 規模 |
|---|---|---|---|
| **O31** GitHub I | A20・E17・E19・E21・E23・H05 | **全部ユーザーの `gh` 経由・トークンを保存しない・ポーリングしない**（画面を開いた時・push の後・手動の ↻）。Task カードに PR の状態と Checks。「Checks を直させる」は失敗ログの末尾を Task のスレッドへ（「データは信用しない・この branch が原因のものだけ直す」の注記つき）。PR は AI が書いた文面を確かめてから `gh pr create --body-file`。**§6 の判断待ち** | L |
| **O32** GitHub II: PR / Issue / URL から Task | E24・E28・A04・E26・E27 | PR は `git fetch origin +refs/pull/N/head:…` で worktree に。Issue は名前をタイトルから作り、プロンプトには URL だけ。**Linear / Jira は作り込まない**: ＋Task に URL を貼れば、エージェントが自分の MCP で読む。**§6 の判断待ち** | M |
| **O33** GitHub III: レビューコメントを直させる | X2・E20 | レビューの thread を読み、選んだものをスレッドへ。返信と resolve は **エージェントが終わった後に、文面を見せて本人が押す**（Orca はプロンプトが届いた時点で自動投稿する）。auto-merge は `gh pr merge --auto`。**§6 の判断待ち** | M |
| **O34** 閉じても走る II: 端末の daemon | C05・C06 | `necoder-remote-server` の daemon を土台に、PTY の request（spawn / attach / detach / write / resize / snapshot）と出力の Event を足し、ローカルでも Unix ソケットで起動する。TerminalView に PTY ハンドルの抽象。daemon 側にも alacritty の `Term` を持たせ、reattach でスナップショット。scrollback をディスクへ（上限つき）。端末 id を台帳に残す | XL |
| **O35** 閉じても走る III: SSH の切断を越える | G06・G07 | リモート端末を `ssh -tt` の中ではなく remote server の daemon に持たせ、切断後に reattach する | L |
| **O36** 閉じても走る IV: ACP のターンを続ける | S5 のエージェント部分 | ACP を daemon 側で持つか、stdio をバッファして中継する。アプリ不在中の承認は、方針で裁くかスマホへ回す。**O34 の後に判断** | XL |
| **O37** SSH の UI | G01〜G04・G09 | 接続先の登録と接続テスト・ホスト鍵の案内・パスフレーズ（ssh-agent / Keychain の案内）・ダウンロード / アップロード | M |
| **O38** スマホ | G15〜G18 | ファイル・端末（読み取り）・stage / commit・Task の作成・画像の添付 | L |
| **O39** CLI | G20〜G22 | selector・任意の端末の read / send / wait・`ne` での diff | M |
| **O40** Skills | G25 | `necoder skills get` が版に合った使い方を出し、設定のボタンで `~/.claude/skills/necoder/SKILL.md` などへ薄い入口を書く。入っている skill の一覧 | S〜M |
| **O41** 定期実行 | G24 | 台帳に表を足し、GUI の中で「次の予定時刻まで 1 本のタイマー」。時刻が来たら既存の Task 作成を呼ぶ。precheck。アプリの起動中だけ動く。**§6 の判断待ち** | M |
| **O42** Cloud VM レシピ | G13 | 作成スクリプトが SSH の接続先を出力 → その host に Task を作る → 削除で破棄スクリプト。O20 の「どこで動かすか」の後。**§6 の判断待ち** | L |
| **O43** Computer Use・エミュレータの案内 | F12・F13 | ネイティブは作らない（§4）。MCP の案内と macOS の権限の案内 | S |
| **O44** レールのグループ | A18 | 複数の repo を 1 つの束にしてレールに置く | M |
| **O45** 仕上げの残り | H13・H14・H17・H20・C15・H12 | オンボーディングのツアー・初回の設定の取り込み・更新のチャネルと changelog・ペット・端末のインライン画像（vte が捨てるので自前の解析）・UI 言語の追加（機械翻訳の質と保守は本人判断） | 各 M |

## 3. 大物の設計メモ（夜の調査で分かったこと）

調査の詳細（行番号つき）は、PR ごとの本文と、調査の証拠ファイルに残した。ここには計画を左右した事実だけを書く。

### 3.1 変更レビューと注記トレイ（O6・O7）

- 今の diff タブは、unified diff の**文字列**を読み取り専用のバッファに入れているだけ（`editor_area/diff.rs` の `open_diff_tab_for`）。hunk や行のモデルが無く、色も無く、行番号は diff テキストの行番号になる。比較の相手は常に HEAD。
- `EditorView` は子要素の無い単一の Element で、行の間にブロックを差し込む API も、任意の色で行背景を塗る API も無い。**レビュー画面は EditorView を拡張せず、別のビューにする**（コアに機能を知らせない）。
- `project` に足す API: base を指定したファイル単位のパッチ・`git show <rev>:<path>`・merge-base。今あるのは `git_diff_files_on(base)` の一覧と行数だけ。
- composer と送信: 外から特定のスレッドへ「人間の発話として」送る公開 API が無い（あるのはアクティブ宛ての `send_prompt_text` と、台帳イベント表示の `send_ledger_event_to`）。O7 で「指定のスレッドへ送る（実行中ならキュー）」を足す。
- 注記の型は、O9 の「ページの要素」と O29 の「Markdown の注記」も入る形にしておく。

### 3.2 localhost プレビューと Design Mode（O8・O9）

- wry 0.56.1 には、初期化スクリプト・IPC・`evaluate_script_with_callback`・遷移の handler・各 OS の WebView のハンドルが全部ある。
- **IPC の handler は生成時にしか付けられない。** 「Design 中だけ受ける」は、Rust 側で Design の状態と nonce を確かめて捨てる形にする。`window.ipc` はページ内のどのスクリプトからも呼べるので、nonce が要る。
- WKWebView の `takeSnapshotWithConfiguration` は objc2-web-kit の feature `WKSnapshotConfiguration` が要る（wry は有効にしていない）。
- Chat の artifact の隔離（IPC 無し・`connect-src 'none'`）は変えない。
- 端末の URL は今は捨てている。`ui::links` は `localhost:3000`（スキーム無し）を path:line と誤認し、`http://[::1]:5173` は `]` で切れる（O3 で直す）。

### 3.3 閉じても走る（O4 → O34〜O36）

- 今は PTY も ACP の子プロセスもアプリ本体が持ち、⌘Q や最後の窓閉じで全部終わる（`terminal_view.rs` の Drop、`host.rs` の `HostProcess` の Drop）。
- remote server は PTY を持っておらず、リモート端末は、ローカルの PTY の中で動く `ssh -tt` にすぎない。daemon は 1 接続ずつの逐次ループで、600 秒接続が無いと終わる。session id は接続ごとの乱数。
- Orca の daemon は、テストを除いて約 3 万行（SSH の relay も約 3 万行）ある。
- そこで 2 段にする。
  - まず O4（安い）で「⌘Q の確認 → 隠して動かし続ける」。クラッシュや更新の再起動は守れないが、うっかり閉じる事故は防げる。
  - 本格版は O34 以降。remote server の daemon・`SHRS` フレーム・再接続の骨組みが流用できる。

### 3.4 使用量・レート制限（O11・O14）

- **Orca は非公開のエンドポイントを使っている。** Claude では、Keychain の `Claude Code-credentials` から本人の OAuth トークンを読み、`claude-code/2.1.0` を名乗って `api.anthropic.com/api/oauth/usage` を叩く。予備として、見えない PTY で `/usage` を打って画面を読む。アカウント切替は `~/.claude` と Keychain の上書き。
  - docs には「No API calls」と書いてあるが、実装は外部に HTTP を送っている。
- **necoder は資格情報に触れずに作れる。**
  - Claude: ACP の claude-agent-acp 0.81.1 は、`rate_limit_event` を受けると `_meta["_claude/rateLimit"]` 付きの usage_update を送る。中身は status・resetsAt・rateLimitType・utilization。
  - ターンの結果には `cost` と `_meta.quota` が付く。
  - Codex: CLI 本体に `account/rateLimits/read` と `account/rateLimits/updated` がある（この Mac の codex 0.144.6 の `strings` で確認）。
  - 弱点: Claude の値はイベントが来た時にしか更新されない。常に最新にするには非公開の経路が要る。これは §6 で判断を仰ぐ（推奨は使わない）。
- アカウント切替は、口座ごとの `CLAUDE_CONFIG_DIR` / `CODEX_HOME` を起動時の env で渡す。Claude 2.1 以降は Keychain の項目がディレクトリごとに分かれるので、これで分離できる。

### 3.5 GitHub（O31〜O33）

- **Orca の GitHub 連携は、全部ユーザーの `gh` 子プロセス**で、トークンを保存しない（18,355 行）。一方で、状態によって 10 秒〜30 分間隔のポーリングと、レート制限のブレーカを持つ。
- **Orca の「PR コメントを AI で直す」は、プロンプトがエージェントに届いた時点で自動投稿する。** 中身は thread の resolve と「Fixing. Will be in the next commit」の返信（bot にも mention）で、修正を待たず、止める設定も無い。
- necoder はこうする。
  - `gh` だけで済む範囲（状態・Checks・失敗ログ・PR から worktree・Issue から Task）を、**ポーリングせずに**作る。
  - GitHub への書き込み（返信・resolve・auto-merge・PR 作成）は、エージェントが終わった後に文面を見せて、本人が押した時だけ行う。
  - Linear / Jira はトークンを預からず、エージェントの MCP に読ませる（＋Task に URL を貼るだけ）。

### 3.6 `/` 補完と ACP の取りこぼし（O2・O15・O17）

- `acp_client` は `AvailableCommandsUpdate`・`SessionInfoUpdate`、ToolCall の `_meta`、UsageUpdate の `cost`、PromptResponse の `_meta` を捨てている。
- **待機中（ターンの間）は update を読まない。** claude-agent-acp はコマンド一覧を session/new の直後に送るので、今のままでは最初のプロンプトまで届かない。
- CLI の過去の会話: ACP の session id は Claude Code の UUID そのもの。claude-agent-acp は `~/.claude/projects/<dir>/<uuid>.jsonl` を探して履歴を再生する。`session/list` には CLI 製の会話も出る。Codex も同様（cursor つき）。
- サブエージェント: ACP の標準には親 ID が無い。Claude は子の update に `_meta.claudeCode.parentToolUseId` を付ける。`subagents` の capability は広告しない（Rust のスキーマに無い variant で、逆直列化に失敗する恐れ）。

## 4. 「無理だと思っていた」5 つ — できるのか

結論: **5 つとも技術的にはできる。** ただし重さはまるで違う。Orca と同じ作り方をすると大きいが、necoder がすでに持っている部品（Remote SSH・MCP・Task の作成・`necoder fleet`）の上に載せると、多くは中くらいで済む。

| 項目 | Orca の作り | necoder で作るなら | 規模 | 推奨 |
|---|---|---|---|---|
| **スケジュール実行**（G24・never） | cron / RRULE・precheck・実行履歴・ホストをまたぐ一覧（`src/main/automations` テスト除き 4,580 行） | 台帳に「定期実行」の表を足し、GUI の中で「次の予定時刻まで 1 本のタイマー」で待つ（ポーリングしない）。時刻が来たら、既存の Task 作成（準備スクリプト → プロンプト送信）を呼ぶだけ。precheck は `sh -c` の終了コード。**アプリが起動している間だけ動く**（O4 の「隠して動かし続ける」と相性がよい）。閉じていても動かすのは O34 の daemon の後 | M | タグを変えるなら作る（O41） |
| **Skills の配布**（G25） | `npx skills add` で SKILL.md の薄い入口を配り、本文は `orca skills get` が版に合わせて出す。更新の通知と一括更新 | `necoder skills get` が版に合った使い方（`necoder fleet` と MCP の道具）を出し、設定のボタンで `~/.claude/skills/necoder/SKILL.md` などへ薄い入口を書く。入っている skill の一覧（`~/.claude/skills`・`~/.codex/skills`・プロジェクトの `.claude/skills`）も見せる | S〜M | 作る（O40） |
| **Computer Use**（F12） | OS ごとのネイティブ補助（macOS だけで `native/computer-use-macos` 6,888 行）＋ main 側 4,515 行。アクセシビリティ木・クリック・入力・スクショ | **本人の環境では、Codex のスレッドですでに使える**。`~/.codex/config.toml` に computer-use の MCP サーバが登録されていて、codex-acp がそれを立ち上げる（JOURNAL 2026-09-25）。necoder が止めているのは Captain の席だけ（`crates/agent_panel/src/seat.rs` の `restrict_mcp_to`）。necoder が足すのは、Claude Code にも同じ MCP を渡す案内と、macOS の権限（アクセシビリティ・画面収録）の案内くらい。ネイティブ補助を自作すると XL | 案内だけなら S | ネイティブは作らない（O43） |
| **iOS / Android エミュレータ**（F13） | iOS は serve-sim（MJPEG の映像とアクセシビリティ木）、Android は scrcpy（H.264）を使い、アプリ内のペインに映して操作を送る（`src/main/emulator` テスト除き 4,818 行） | エージェントに操作させるだけなら、`xcrun simctl` / `adb` を包んだ MCP の道具で M。ただし `xcrun simctl` にはタップの命令が無く（この Mac の `xcrun simctl help` で確認）、iOS の操作には補助ツールが要る。**画面を necoder の中に映す**のは、iOS は MJPEG の復号、Android は H.264 の復号が要り、L〜XL。シミュレータの画面は OS の Simulator.app で見れば足りる | M（道具）/ L〜XL（映像） | 当面は MCP サーバを渡す案内だけ（O43）。映像は作らない |
| **Cloud VM レシピ**（G13・never 相当） | repo の `orca.yaml` に create / suspend / resume / destroy のスクリプトを書き、出てきた SSH 先かペアリング URL につなぐ。プロバイダ（Vercel Sandbox・Fly・Modal・Docker）はユーザー持ち | necoder は Remote SSH（system OpenSSH・server の自動配備）をすでに持つ。「作成スクリプトが SSH の接続先を出力 → その host に Task を作る → Task の削除で破棄スクリプト」を足せば成り立つ。前提は O20 の「どこで動かすか」 | L | タグを変えるなら作る（O42・O20 の後） |

**大きくしすぎないための線引き**: 5 つのうち、necoder の中に常駐する部品が増えるのはスケジュール実行のタイマー 1 本だけ。ほかは「必要な時に外のプロセスを呼ぶ」か「エージェントに道具を渡す」形にする。

## 5. やらないもの（理由つき）

| 調査 ID | 何 | 理由 |
|---|---|---|
| B01 | 35 種以上の CLI エージェント | 本人判断（2026-09-26「ACP が 7 種だけは流石に良いだろう」） |
| B13 | Claude Agent Teams | ACP から見えない（teammate を別の窓で出す Claude Code の TUI 機能） |
| B21 | 会話の巻き戻し | ACP に「N 通目まで戻す」手段が無い。ファイルは checkpoint、会話は O15 の「新しいセッションで続ける」で代える |
| D09 | Jupyter | FEATURES §13 never |
| E18 / E22 / E25 / E29 / E30 | stacked PR・GitLab 等・GitHub Projects・API の予算・プロジェクト別の gh アカウント | GitHub を解禁しても入れない（`gh` の既定のアカウントだけ・ポーリングしないので予算も要らない） |
| F03 / F05 / F15 | ブラウザのプロファイル・ダウンロード・Web 検索 | FEATURES §9 never（汎用ブラウザ）。プレビューは localhost に限る |
| G23 | Orchestration の残り | Captain の計画（FLEET-V2 F6 の残り）で進める |
| H15 | テレメトリ | FEATURES §13 never（思想差として残す） |
| H16 | minidump | 重い（crashpad 等）。panic のログから Issue の下書きを作る今の仕組みで足りる |
| H18 / H28 | 配布の形態・Windows / Linux の正式対応 | 別トラック（WINDOWS-PORT の W フェーズ・ROADMAP M13） |
| H19 | プラグイン | FEATURES §9 never（Marketplace）/ later（WASM） |
| H24 | GPU の表示 | necoder は常に GPU で描くので、見せるものが無い |

## 6. 本人の判断が要ること

1. **main の未コミット差分（Captain の作業）と、この計画の PR のどちらを先に入れるか**（§1.1）。
2. **FEATURES のタグ**（タグは本人の管理。推奨を添える）:
   - §6 never「ホスティング連携」→ **GitHub だけ、`gh` 経由・ポーリングなし・書き込みは本人が押した時だけ**、を許すか（O31〜O33）。推奨: 許す。GitLab 等・Projects・stacked PR は never のまま。Linear / Jira は作り込まず MCP に任せる。
   - §9 never「拡張 / API 向けの汎用 webview」→ **localhost 限定のプレビュー（O8）と Design Mode（O9）** はこの never に当たらないと読んで、夜のうちに PR にした。違うと判断したら O8・O9 を閉じる。**エージェントがプレビューを操作する道具（O10）** は「API 向け」に近いので、許すかどうかを聞きたい。推奨: localhost 限定なら許す。
   - §12 never「Automations 相当」→ **ローカルの定期実行（O41）**。推奨: 許す（Cursor の Cloud Agents と違い、手元のサブスクで手元で動く）。
   - §12 never「Cloud Agents 相当」→ **Cloud VM レシピ（O42）**。推奨: 保留（O20 と O34 の後に改めて）。
   - §6 never「graph / stash UI」→ **stash（E35）**。コミットグラフはすでに実装済みで、タグの方が実装とずれている。推奨: stash は作らない・タグの graph を外す。
3. **過去の決定とぶつかる項目**:
   - 状態別のボード・カンバン（A22・B04）は、F3 で本人が消した「管制の統合パイプライン」と同種。推奨: 作らない（サイドバーのフィルタと検索で足りる）。
   - ヘッドレスのサーバ運用・Web クライアント（G12・G28）は、FEATURES never「Web 版」と、Tailscale 不採用の決定（DECISIONS）にぶつかる。推奨: O34（daemon）の後に改めて。
   - artifact の公開リンク（F11）は、中身を外に出す機能で、リレーが何も覚えない設計（DECISIONS）とぶつかる。推奨: 作らない。
4. **使用量の取り方**（O11）: Claude の値を常に最新にしたいなら、Orca と同じ非公開の経路（本人の OAuth トークンを Keychain から読む）が要る。推奨: 使わない（ACP で来る値だけにする）。
5. **複数アカウント**（O14）: 仕事用と個人用を分ける用途で作る。上限を伸ばすために使うかどうかは、各サービスの規約に沿って本人が判断する。
6. **音声入力**（H07）: macOS の音声入力（fn を 2 回）が composer で効くなら、それで足りる。本人の実機で確かめてほしい（IME と同じ経路なので効く見込みだが未確認）。
7. **UI 言語の追加**（H12・O45）: 機械翻訳で 6 言語にできるが、質と保守の手間がある。
8. **PR の push と作成を、今後も統括がやってよいか**（今夜は本人の「branch を切って PR」の指示で行った）。

## 7. 進捗

状態・確かめたこと・確かめられなかったことは、各 PR の本文にある。夜の間は利用上限で担当が止まったため、同時に動かすのは 3 本までにしている。

| # | ブランチ | PR | 状態 |
|---|---|---|---|
| O0 | `parity/o00-plan` | #13（この文書） | レビュー待ち |
| O1 | `parity/o01-fixes` | #17 | レビュー待ち |
| O2 | `parity/o02-slash-commands` | #14 | レビュー待ち |
| O3 | `parity/o03-terminal-basics` | #20 | レビュー待ち（判断 3 点あり） |
| O4 | `parity/o04-quit-guard` | #22 | レビュー待ち |
| O6 | `parity/o06-review` | #15 | レビュー待ち |
| O7 | `parity/o07-annotations`（O6 の上） | #16 | レビュー待ち |
| O8 | `parity/o08-localhost-preview` | #18 | レビュー待ち |
| O9 | `parity/o09-design-mode`（O8 の上） | #19 | レビュー待ち |
| O11 | `parity/o11-usage`（O2 の上） | #24 | レビュー待ち |
| O12 | `parity/o12-notifications`（O4 の上） | #23 | レビュー待ち |
| O28 | `parity/o28-explorer-search` | #28 | レビュー待ち |
| O30 | `parity/o30-languages` | #27 | レビュー待ち（バイナリ +11% の判断） |
| O39 | `parity/o39-cli`（O40 の上） | #26 | レビュー待ち |
| O40 | `parity/o40-skills` | #25 | レビュー待ち |
| — | `parity/integration` | #21（draft） | 全部を重ねた確認用。ぶつかりを解いた状態でテストが通る |
| O15 | `parity/o15-session-history`（O11 の上） | | 実装中（Mac の worktree・未 push） |
| O18 | `parity/o18-git-panel`（O1 の上） | | 実装中（Mac の worktree・未 push） |
| O5（手元） | `claude/sleepy-hamilton-gesxiq` | #30 | 実装済み・実機未確認（SSH 転送・Windows は未） |
| O21（一部） | 同上 | #30 | 外部の worktree・取り込み・統合先の取り違え・消えた worktree・Task の絞り込み・パス / ブランチ名のコピー・ピンの保存・手動の休眠（⋯「休ませる」）・並べ替え（レール / 最近 / 要対応）・複数選択（⌘ / ⇧ クリック → まとめて休ませる・舞台に並べる・片付けへ）。詳細・親子は未 |
| O26 | 同上 | #30 | 済: `path:行`・全部閉じる・外部アプリ・自動保存・プレビュータブ・ピン留め。実機未確認 |
| O20（一部） | 同上 | #30 | ブランチ名・起点・既にあるブランチ・`.worktreeinclude`・準備の skip（＋ Task の詳細）・準備が失敗した Task の「準備をやり直す」/「準備を飛ばして始める」・repo ごとの base（`task_base`）・作成中の行（段の表示・取り消し・やり直し）・sparse checkout（`task_sparse`・cone）。絵文字名・どこで動かすかは未 |
| O13（一部） | 同上 | #30 | 作業中はスリープさせない・通知の履歴（ベル）・通知音の大きさ・ニュースのクリック移動・スマホへの完了 push（relay の host）。メニューバー・TCC の案内は未。完了 push は実機 iPhone で未確認 |
| O22 | 同上 | #30 | リソース（メモリをプロジェクトごと・使っていないエージェントを止める）・片付け（まとめて終了 / 失うものが無い worktree をまとめて削除・worktree の大きさと大きい順）。SSH 先のプロセスのメモリは未 |
| O16（一部） | 同上 | #30 | repo ごとのレシピ（`.necoder/recipes/*.md` を `/` 補完へ）・エージェント別の新規スレッド（`workspace::NewThreadCodex` 等・パレットと keymap.json）。エージェントの有効 / 無効・権限の既定・起動引数の UI は未 |
| O14 | 同上 | #30 | アカウントのフォルダ・切替（`agent_servers.<id>.env`）・公式 CLI でログイン。mac 実機未確認・Windows は未 |
| O23（一部） | 同上 | #30 | fan-out（1 つの依頼をエージェントごとの Task へ・舞台に並べて比べる）。自動命名ブランチの改名は未 |
| O17（一部） | 同上 | #30 | 質問カードの自由入力（Other 欄に書いて答える・選んだ案に添える。スマホの質問も）・サブエージェントの手順を親の Task の下に畳む・目標の一時停止 / 再開 / 取り消し（Codex の goal の操作）・message rail。ツールのまとめ表示と個別停止は未（個別停止は `_session/async_task/stop` があるが、`async_task_*` の更新は JetBrains AIR 拡張 `_meta.jetbrains.air.capabilities` を名乗った時だけ来る。SDK が未知の sessionUpdate をどう扱うかを実機で確かめてから） |
| O25（一部） | 同上 | #30 | ターミナルの文字の大きさ・フォント・遡れる行数・カーソルの形（設定と設定画面・開いている端末にもその場で効く）・シェルと引数（`terminal_shell`・手元の端末だけ・settings.json）・Quick Commands（`quick_commands`・ドックの ▶ から新しい端末で走らせる）。テーマの取り込み・シェルを選ぶ画面・OSC 133・TUI の状態推定は未 |
| O19（一部） | 同上 | #30 | conflict を AI に渡す（統合の下見で競合したら Task の次へが「競合を直させる」・競合したファイルの一覧つきで Task のエージェントに頼む）。3-way の画面・Abort は未 |
| O29（一部） | 同上 | #30 | CSV / TSV の表（⌘⇧V・RFC 4180・見出しの固定・行番号・仮想リスト）・画像のドロップ（Markdown に落とすと隣へコピーして落とした所に `![]()`・ほかのファイルはタブで開く）。横並びのライブプレビュー・スラッシュメニュー・目次・front matter・Mermaid・Markdown の注記は未 |
| O27（一部） | 同上 | #30 | 設定の検索（全ページから一致した行を集める・キー / 選択肢の名前でも当たる）・表示言語の切替（その場で）・UI とコードの書体（`ui_font_family` / `code_font_family`・settings.json・直書きしていた 30 か所を `ui::ui_font` / `ui::code_font` に）。キー割り当ての GUI・UI ズーム・density・statusbar の項目・書体を選ぶ画面は未 |
| O37（一部） | 同上 | #30 | リモートのダウンロード / アップロード（G09・Finder からエクスプローラへ落とす / 右クリックの「ダウンロード…」「このフォルダへアップロード…」・上書きしない・背景で送り、終わりの知らせから Finder で見せる）・繋がらない理由の案内（G02 / G04・OpenSSH のログを `-E` で読み、指紋・鍵・名前・拒否・応答なし・権限・暗号を見分けて次にすることを出す・パスフレーズの問いに `ssh-add` の案内）。接続先の登録画面・接続テストは未。SSH 実機未確認 |

### 7.1 クラウドでの続き（2026-09-26・`claude/sleepy-hamilton-gesxiq`）

本人の指示（「全て統合して、できるところまで走って」）で、クラウドの Linux 環境で続けた。

- **土台**: `parity/integration`（#21 相当・PR 14 本を重ねたもの）+ この計画（`parity/o00-plan`）+
  UX・コードレビュー台帳（`docs/ux-code-review-2026-09`）をマージ。衝突なし。
- **台帳の修正**: R01〜R08・R14 を修正、R11 は一部（使用量の記録を背景へ）、R15 は表現だけ。各項目の
  コミットと検証は `UX-CODE-REVIEW.md` の各項目の末尾。R09・R10 は Mac の main の未コミット差分が対象で、
  このブランチからは見えないので触っていない。
- **計画の続き**: O5（手元の Ports）・O21 の一部（外部の worktree・絞り込み・並べ替え・コピー・ピンの保存・休ませる・複数選択）・O26（エディタの所作・全部）・
  O20 の一部（ブランチ名・起点・`.worktreeinclude`・準備の skip / やり直し・repo ごとの base・作成中の行と取り消し / やり直し）・O13 の一部（スリープ抑止・通知の履歴・音量・スマホへの完了 push）・
  O22（リソース・片付け）・O23 の一部（fan-out）・O14（アカウント切替）・O16 の一部（レシピ・エージェント別の新規スレッド）・
  O17 の一部（質問カードの自由入力・サブエージェントの手順・目標の操作・message rail）・O27 の一部（設定の検索・表示言語）・
  O25 の一部（ターミナルの文字の大きさ・フォント・遡れる行数・カーソルの形・シェルと引数・Quick Commands）・
  O19 の一部（統合で競合したら Task のエージェントに直させる）・O29 の一部（CSV / TSV の表・画像のドロップ）・O37 の一部（リモートのダウンロード / アップロード・繋がらない理由の案内）。O15・O18 は Mac の worktree に途中があるので触っていない（push されれば取り込める）。
- **確かめたこと**: Linux で `cargo check --workspace --all-targets`（警告 0）と `cargo test`。落ちるのは
  Linux で元から落ちる 2 件（PDF のネイティブビューア・Web タブの surface。どちらも macOS / Windows の
  ネイティブ部品が要る）だけ。**実画面・macOS 実機・SSH 実機は未確認**（隔離 offscreen の撮影は macOS が要る）。
- **push**: 最初は GitHub への書き込み権限が無く 403（git bundle で本人に渡した）。本人が GitHub App を
  直した後に push し、**#30**（base `parity/integration`・1 本にまとめた）を作った。分けたくなったら、台帳の修正は
  元の PR（O4・O6/O7・O8/O9・O11）のブランチへ、O5・O13・O20・O21・O22・O26 は新しいブランチへ切り出すのが
  素直（コミットは項目ごとに分けてある）。
- **#30 の初回 CI**: この PR の物は 3 つ（Windows だけの型推論 E0282・macOS の temp が `/var`→`/private/var` の
  リンクで消した worktree のパス比較がずれるテスト・Windows では流せない準備スクリプトのテスト）で、直した。
  Windows は手元で `x86_64-pc-windows-gnu` 向けの `cargo check --workspace --all-targets`（`-D warnings`）まで
  確かめられる（mingw と、turso の build.rs が呼ぶ `windres` の別名が要る）。relay の WebKit のカメラの
  テストは main の v0.1.18〜v0.1.20 でも同じ形で落ちている（この PR の物ではない・提案は #30 のコメント）。
  CLA は Claude の名前のコミットが allowlist に無いので本人の判断待ち。
  2 回目（8be0711）は Windows のテストで 3 件（O2 の待機中の更新・O11 の codex の偽 app-server 2 件）。どれも
  偽物（python）の起動込みで 10 秒の上限を持つテストで、元の PR（#14・#24）の Windows では通っていた。
  テストが増えて並ぶと Windows のランナーでは python の起動だけで上限に届くので、上限を 60 秒にした
  （条件がそろえばすぐ抜けるので、通る時は遅くならない）。

## 付録 A: 調査の全 226 項目の行き先

「済」は調査の時点で necoder が持っていた（有）。「方式差」は別の方式で目的を達している。「§5」はやらない、「§6」は判断待ち。
### A. ワークツリー・ワークスペース・サイドバー

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| A01 | リポジトリ追加と repo ごとの base ref | 一部 | O20 |
| A02 | 作成ダイアログ（空なら自動命名・start-from） | 一部 | O20 |
| A03 | 背景作成（進捗行・キャンセル・Retry） | 一部 | O20 |
| A04 | ブランチ名の指定・課題から命名 | 無 | O20・O32 |
| A05 | 共有パス・`.worktreeinclude`・`sharedDirectories` | 一部 | O20 |
| A06 | 作成後の setup hook | 一部 | O20 |
| A07 | 親子 worktree（ネスト表示・子孫ごと Sleep / Delete） | 無 | O21 |
| A08 | 絵文字の名前（:rocket:） | 無 | O20 |
| A09 | ピン留め | 一部 | O21 |
| A10 | Sleep / Archive / Delete | 一部 | O21 |
| A11 | 複数選択の一括操作 | 無 | O21 |
| A12 | インライン改名・詳細ダイアログ | 一部 | O21 |
| A13 | サイドバーのフィルタ（Sleeping / 既定ブランチ / CLI 作成 / detached 等） | 一部 | O21 |
| A14 | グループ・並べ替え・サイドバー内検索 | 一部 | O21 |
| A15 | 外部 worktree の検出と表示切替 | 一部 | O21 |
| A16 | 片付け画面（状態・活動・サイズで一括削除） | 無 | O22 |
| A17 | 残ったブランチのレビュー | 方式差 | 方式差 |
| A18 | マルチリポの project group | 無 | O44 |
| A19 | repo アイコン・バッジ色 | 一部 | O27 |
| A20 | worktree カードに課題を表示 | 無 | O31 |
| A21 | 進捗コメントとカード状態をエージェントが更新 | 有 | 済 |
| A22 | 状態別ボード | 無 | §6 |
| A23 | 自動命名ブランチを作業内容から改名 | 無 | O23 |
| A24 | sparse checkout プリセット | 無 | O20 |
| A25 | 外で消された worktree の自動掃除 | 一部 | O21 |
| A26 | 削除ショートカット・名前のコピー | 無 | O21 |
| A27 | 同じ依頼を N 体へ fan-out して比べる | 一部 | O23 |
| A28 | Run on（local / SSH / サーバ / VM）を選ぶ | 一部 | O20・O42 |

### B. エージェント・セッション・状態・使用量・チャット

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| B01 | 35 種以上の CLI エージェント + 任意 CLI | 一部 | §5 |
| B02 | 検出・自動導入・有効 / 無効 | 一部 | O16 |
| B03 | 状態の検出と表示 | 有 | 済 |
| B04 | Agent Dashboard（カンバン・検索・フィルタ・pop-out） | 一部 | O21・§6 |
| B05 | Agent map | 有 | 済 |
| B06 | Restart chip | 有 | 済 |
| B07 | 権限の既定（Yolo / Manual 一括）・起動引数の上書き | 一部 | O16 |
| B08 | hibernation | 有 | 済 |
| B09 | 各 CLI のディスク上 transcript を走査して再開 | 一部 | O15 |
| B10 | 全エージェント横断の全文検索 | 一部 | O15 |
| B11 | Continue in New Session（handoff） | 一部 | O15 |
| B12 | サブエージェントの子行 | 無 | O17（済: 親の Task の下に畳む） |
| B13 | Claude Agent Teams | 無 | §5 |
| B14 | アカウント hot-swap | 無 | O14 |
| B15 | 使用量・レート制限（5h / 週・reset・80% 警告） | 無 | O11 |
| B16 | Stats（日別チャート・推定コスト） | 無 | O11 |
| B17 | Keep computer awake | 一部 | O13 |
| B18 | composer（添付・`/` コマンドと skills・ピル） | 一部 | O2 |
| B19 | 構造化質問カード | 有 | 済（自由入力も O17 で済・スマホも） |
| B20 | message rail | 一部 | O17（済） |
| B21 | rewind・/clear・/compact | 一部 | §5 |
| B22 | inline diff・plan・ツールのバッチ・BG タスク停止 | 一部 | O17 |
| B23 | 履歴から構造化チャットで再開 | 一部 | O15 |
| B24 | Chat ⇄ TUI の切替 | 方式差 | 方式差 |
| B25 | 端末で直接起動したエージェントの状態検出 | 無 | O25 |
| B26 | `.claude` / `.codex` のフックと CLAUDE.md を尊重 | 有 | 済 |
| B27 | Codex goal | 無 | 済（表示 O2・操作 O17） |
| B28 | エージェント別の新規タブのキー | 一部 | O16（済: エージェントごとの action・既定のキーは無し） |
| B29 | 完了通知・未読 | 一部 | O12・O13 |
| B30 | Agents feed（横断フィード） | 一部 | O13 |
| B31 | 会話名の同期 | 一部 | O2 |
| B32 | モデル一覧を動的に取得 | 有 | 済 |

### C. ターミナル・ペイン・セッション復元

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| C01 | 描画品質（xterm + WebGL） | 一部 | O3 |
| C02 | 端末の分割（無限・ネスト） | 無 | O24 |
| C03 | タブのドラッグで分割・異種タブの混在 | 一部 | O24 |
| C04 | worktree ごとのレイアウト保持 | 有 | 済 |
| C05 | アプリ終了を越えて PTY が生きる（daemon） | 無 | O34 |
| C06 | scrollback の永続化 | 無 | O34 |
| C07 | 端末内検索（regex・件数） | 無 | O3 |
| C08 | OSC 52 | 無 | O3 |
| C09 | リンクのポップオーバー・URL・OSC 8 | 無 | O3 |
| C10 | Copy Context | 無 | O3 |
| C11 | 端末テーマのライブラリ・Ghostty / Warp の取り込み | 一部 | O25 |
| C12 | kitty keyboard・Shift+Enter・¥ → \ | 無 | O3 |
| C13 | Floating terminal | 一部 | O24 |
| C14 | Quick Commands | 無 | O25 |
| C15 | インライン画像 | 無 | O45 |
| C16 | Windows のシェル選択 | 一部 | O25 |
| C17 | 端末へのファイルドロップ | 無 | O3 |
| C18 | 端末のショートカット（⌘T・⌘⇧\ 等） | 一部 | O24 |
| C19 | 端末タブの改名 | 一部 | O3・O24 |
| C20 | シェル統合（OSC 133） | 無 | O25 |
| C21 | 実行中の端末を閉じる前の確認 | 無 | O4 |
| C22 | シェルの引数・既定のシェル | 無 | O25 |
| C23 | file:line のクリック | 有 | 済 |
| C24 | 端末のフォント・scrollback・カーソル | 無 | O25 |

### D. エディタ・ビューア・エクスプローラ・ナビゲーション

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| D01 | 自動保存 | 無 | O26 |
| D02 | プレビュータブ | 無 | O26 |
| D03 | リッチ Markdown 編集（スラッシュメニュー・表・目次・front matter） | 一部 | O29 |
| D04 | Markdown のレビュー注記 | 無 | O29 |
| D05 | Mermaid | 無 | O29 |
| D06 | PDF | 一部 | 方式差 |
| D07 | 画像・画像 diff（swipe / onion） | 一部 | O30 |
| D08 | CSV / TSV の表 | 無 | O29 |
| D09 | Jupyter notebook | 無 | §5 |
| D10 | HTML プレビュー（SSH・隔離・横に） | 一部 | O8 |
| D11 | ミニマップ | 無 | O30 |
| D12 | タブ内の Changes view | 一部 | O6 |
| D13 | Word wrap（diff 用は別設定） | 一部 | O6 |
| D14 | フォント・UI ズーム・density | 一部 | O27 |
| D15 | パス:行のコピー（⌘⌥C） | 一部 | O26 |
| D16 | 自然順・git 色・右クリックで discard / stage | 一部 | O28 |
| D17 | ドロップ（MD へ画像・OS クリップボードへファイル） | 一部 | O29 |
| D18 | フォルダ内検索 | 無 | O28 |
| D19 | ⌘P（recency・gitignored の 2 パス目） | 一部 | O28 |
| D20 | 新規タブの omnibox | 無 | O28 |
| D21 | Jump palette（タブ検索・最近のセッション・PR #・作成行） | 一部 | O28 |
| D22 | 設定の検索 | 無 | O27（済） |
| D23 | キー割り当ての GUI | 一部 | O27 |
| D24 | 言語機能 | 有（優位） | 済 |
| D25 | 大規模での性能 | 一部 | O28 |
| D26 | 言語の幅 | 一部 | O30 |
| D27 | Find の種込み・F7 | 有 | 済 |
| D28 | 全部閉じる・タブのピン留め | 一部 | O26 |
| D29 | Open in（VS Code / Cursor / Zed / ターミナル） | 一部 | O26 |

### E. レビュー・Git・連携

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| E01 | combined diff | 一部 | O6 |
| E02 | 比較基準の切替 | 一部 | O6 |
| E03 | diff 横のファイルツリー | 一部 | O6 |
| E04 | 変更の無い領域の折り畳み・空白表示 | 一部 | O6 |
| E05 | 3-way の conflict UI・Abort | 無 | O19 |
| E06 | diff の行コメント | 無 | O7 |
| E07 | まとめてエージェントへ送る | 無 | O7 |
| E08 | Resolve・再レビュー | 無 | O7 |
| E09 | AI 帰属（行単位） | 一部 | O30 |
| E10 | Source Control パネル（discard・Sync・主ボタン） | 一部 | O18 |
| E11 | AI のコミットメッセージ | 有 | 済 |
| E12 | フック失敗時の Fix with AI | 無 | O18 |
| E13 | force-with-lease | 無 | O18 |
| E14 | amend | 無 | O18 |
| E15 | conflict を AI へ渡す | 無 | O19 |
| E16 | AI アクションのレシピ（repo ごと） | 無 | O16 |
| E17 | PR 作成（アプリ内・AI で詳細生成） | 一部 | O31 |
| E18 | stacked PR | 無 | §5 |
| E19 | PR ビュー | 無 | O31 |
| E20 | auto-merge・merge queue | 無 | O33 |
| E21 | CI 失敗・Actions のログ | 無 | O31 |
| E22 | GitLab・Bitbucket・Azure・Gitea | 無 | §5 |
| E23 | worktree ごとの PR 状態 | 一部 | O31 |
| E24 | Issues | 無 | O32 |
| E25 | GitHub Projects | 無 | §5 |
| E26 | Linear | 無 | O32 |
| E27 | Jira | 無 | O32 |
| E28 | 課題から worktree を作る | 無 | O32 |
| E29 | GitHub API budget | 無 | §5 |
| E30 | プロジェクトごとの gh アカウント | 無 | §5 |
| E31 | ブランチ行と +/- 行数 | 一部 | O18 |
| E32 | 変更行のパスコピー | 一部 | O18 |
| E33 | diff から HTML を横にプレビュー | 一部 | O8 |
| E34 | コミット署名・外部エディタ | 無 | O18 |
| E35 | Git 操作の総量 | 一部 | §6 |

### F. 内蔵ブラウザ・Design Mode・自動化

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| F01 | worktree ごとの内蔵ブラウザ | 無 | O8（localhost のみ） |
| F02 | DevTools | 一部 | O8 |
| F03 | プロファイル・cookie の取り込み・passkey | 無 | §5 |
| F04 | ビューポート・デバイスのエミュレーション | 無 | O8 |
| F05 | ダウンロード | 無 | §5 |
| F06 | Design Mode | 無 | O9 |
| F07 | ページへの注釈 | 無 | O9 |
| F08 | リンクの開き先の選択 | 無 | O8 |
| F09 | ブラウザ自動化 CLI | 無 | O10 |
| F10 | リモート経由のブラウザ通信 | 無 | O5 |
| F11 | artifact の公開リンク | 無 | §6 |
| F12 | Computer Use | 無 | O43 |
| F13 | iOS / Android エミュレータ | 無 | O43 |
| F14 | HTML を横にプレビュー | 一部 | O8 |
| F15 | Web 検索 | 無 | §5 |
| F16 | ページのズーム | 無 | O8 |

### G. リモート・モバイル・CLI・オーケストレーション

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| G01 | SSH ターゲットの登録 UI・接続テスト | 一部 | O37 |
| G02 | ホスト鍵の検証と案内 | 有（#30・指紋を確かめていない / 変わったを見分けて案内・確認そのものは askpass） | O37 |
| G03 | ProxyJump・多重化・GSSAPI・FIDO2 | 一部 | O37 |
| G04 | パスフレーズの保持 | 一部（#30・askpass に `ssh-add --apple-use-keychain` の案内。necoder 自身は保存しない） | O37 |
| G05 | SSH 上の worktree とエージェント | 有 | 済 |
| G06 | 切断を越えて PTY が生きる | 一部 | O35 |
| G07 | 接続状態の表示と自動再接続 | 一部 | O35 |
| G08 | ポート転送 | 無 | O5 |
| G09 | リモートのダウンロード・アップロード | 有（#30・Finder から落とす / 右クリック・上書きしない） | O37 |
| G10 | VS Code Remote-SSH で開く | 方式差 | 方式差 |
| G11 | ツールチェインの無い Linux でも動く | 有（優位） | 済 |
| G12 | Remote Orca Server・`orca serve` | 一部 | §6 |
| G13 | Cloud VM レシピ | 無 | O42 |
| G14 | ネイティブのモバイルアプリ | 方式差 | 方式差 |
| G15 | モバイル: 全ホスト一覧・ファイル・端末 | 一部 | O38 |
| G16 | モバイル: 返信・添付・音声 | 一部 | O38 |
| G17 | モバイル: ソース管理 | 一部 | O38 |
| G18 | モバイル: アカウント・workspace 作成 | 無 | O38 |
| G19 | モバイル: 完了の push 通知 | 一部 | O13 |
| G20 | CLI: worktree / repo と selectors | 一部 | O39 |
| G21 | CLI: 任意の端末の read / send / wait | 一部 | O39 |
| G22 | CLI: file open / diff | 一部 | O39 |
| G23 | Orchestration（Run・DAG・メッセージ・gate・別ホスト） | 一部 | §5 |
| G24 | スケジュール実行（cron / RRULE） | 無 | O41 |
| G25 | Skills の配布と更新 | 無 | O40 |
| G26 | MCP サーバの登録 | 有 | 済 |
| G27 | ヘッドレスでのアカウント登録 | 無 | O14 |
| G28 | ヘッドレス運用・QR ペアリング | 一部 | §6 |
| G29 | リモートの状態のリアルタイム反映 | 有 | 済 |
| G30 | リモートでの履歴 | 一部 | O15 |

### H. アプリ全体・通知・設定・配布

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| H01 | 完了通知（system・音・チップ） | 一部 | O12 |
| H02 | ベル（通知履歴）・未読に戻す | 一部 | O13 |
| H03 | Dock バッジ | 無 | O12 |
| H04 | カスタム音・音量 | 一部 | O13 |
| H05 | PR check 失敗・更新の通知 | 一部 | O31 |
| H06 | トレイ | 無 | O13 |
| H07 | 音声入力（オンデバイスの日本語 STT を含む） | 無 | §6 |
| H08 | 設定の検索 | 無 | O27（済） |
| H09 | キー割り当ての UI | 一部 | O27 |
| H10 | UI ズーム・フォント・density | 一部 | O27 |
| H11 | アプリアイコンの切替 | 無 | O27 |
| H12 | UI 言語（6 言語） | 一部 | O27・O45 |
| H13 | オンボーディング（チェックリスト・ツアー） | 一部 | O45 |
| H14 | 初回の設定取り込み | 一部 | O45 |
| H15 | テレメトリ | 無 | §5 |
| H16 | クラッシュ・ログ・フィードバック | 一部 | §5 |
| H17 | 更新チャネル・changelog | 一部 | O45 |
| H18 | 配布の形態 | 一部 | §5 |
| H19 | プラグイン | 無 | §5 |
| H20 | ペット | 一部 | O45 |
| H21 | Resource Manager | 一部 | O22 |
| H22 | Open in 外部アプリ | 一部 | O26 |
| H23 | star・Discord への導線 | 無 | O27 |
| H24 | GPU の表示 | 無 | §5 |
| H25 | statusbar の項目の切替 | 無 | O27 |
| H26 | OS ショートカットとの衝突の警告 | 無 | O27 |
| H27 | TCC 権限の案内 | 無 | O13 |
| H28 | Windows / Linux の正式対応 | 一部 | §5 |
| H29 | アプリ内ヘルプ | 一部 | O27 |
| H30 | ファイル操作の undo | 一部 | O28 |

### X. 書き手が追加で確認したもの

| ID | Orca の機能 | 調査の判定 | 行き先 |
|---|---|---|---|
| X1 | Ports パネル（worktree ごとに起動中のポートを検出し、開く / 止める。ターミナル出力の URL から worktree を特定） | 無 | O5 |
| X2 | PR のレビューコメント（CodeRabbit 含む）を選んで AI に直させ、自動で返信・resolve | 無 | O33 |
