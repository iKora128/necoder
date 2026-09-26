# Orca にあって necoder に無いもの（2026-09-25 全数調査）

> **2026-09-26追記**: 本文の有無判定は下記の調査対象版についての記録。別worktreeで変更レビュー・行コメント・Design Mode・端末基本機能などが実装されているため、現在の未実装一覧としてそのまま使わない。[UX・コードレビュー台帳](../UX-CODE-REVIEW.md)にworktree横断の訂正、残る懸念、確認条件を記録した。「軽さで勝つ」「状態の誤検出がない」等の評価も、設計上の利点と未計測の比較を分けて読む（同台帳R14）。

Orca（stablyai/orca・MIT・Electron）に負けているのは、エディタではなく**エージェントの周り**だ。
エディタとしての深さ（LSP・hunk・blame・⌘I）、ACP による承認と巻き戻し、プライバシー、SSH の作りは、**設計の上では** necoder が有利だ（軽さは未計測。§4 の注のとおり、同じ条件の比較実測はまだ無い・R14）。
一方で次の 6 つが丸ごと無いか使い物にならない。ツイートや Zenn 記事が Orca を推す理由とそのまま重なる。

1. **diff の行に ＋ でコメントを付けて、まとめてエージェントへ送る**（Annotate AI Diff）が無い
2. **worktree 単位の差分レビュー画面**が弱い（1 ファイルずつ HEAD 比較のみ。ソース管理パネルの行を押しても diff が開かない等の不具合つき）
3. **内蔵ブラウザと Design Mode**（要素をクリック → HTML・計算済み CSS・切り抜きスクショをエージェントへ）が無い
4. **ターミナルが未成熟**（分割・検索・URL リンク・OSC 52・Shift+Enter・フォント設定が無い。アプリを閉じると PTY もエージェントも死ぬ）
5. **GitHub / Linear / Jira がアプリ内に無い**（PR・Checks・Issue から worktree を作る流れ）
6. **使用量・レート制限・アカウント切替**が無い（Claude / Codex の 5 時間枠・週枠）

加えて、Orca が CLI エージェントを 35 種以上ワンクリックで起動できるのに対し、necoder が起動できるのは ACP の 7 種だけだ。

関連: 采配役（Captain）の比較は [`captain-orchestrators-2026-09.md`](./captain-orchestrators-2026-09.md)、Fleet の UX 比較は [`fleet-ux-2026-09.md`](./fleet-ux-2026-09.md)。

## 調べ方

**証拠は別ファイルに全部ある**: [`orca-gap-2026-09-evidence.md`](./orca-gap-2026-09-evidence.md)。
- 全 226 項目について、Orca 側の出典（ドキュメントの引用・ソースのパス・PR 番号）と、necoder 側の `file:line` を載せている。
- 「無」の判定には、使った検索と件数を付けている。
- 再現手順、不具合のコードの抜粋、引用の機械検査の結果も入っている。
- 検索の正規表現は [`orca-gap-2026-09-searches.tsv`](./orca-gap-2026-09-searches.tsv) に置いた。

**比べた版**:
- **Orca**: `stablyai/orca@646e9a5b02514795af5139961ccca225dfa01b12`（2026-09-25 の main。同日の最新リリースは v1.4.211）。
  - リポジトリ直下の `orca/` に参照用としてクローンした（`.gitignore` 済み・`zed/` と同じ扱い）。
  - 公式ドキュメントは 55 ページ全部を読んだ。
  - 安定版のリリースノート 74 本（v1.4.128〜v1.4.211）から `feat` 269 件を拾った。
  - ソースの構成（`src/main`・`src/renderer/src/components`）も見た。
- **necoder**: `97e866648281874e53f9001c9b612a6415f0ee8d` と、2026-09-25 時点の未コミットの作業ツリー。行番号はこの作業ツリー基準。

**判定**: 226 項目を 8 領域に分け、necoder のコードに実装があるかを判定した。
- 判定値は 有 / 一部 / 無 / 方式差（別の方式で目的を達している）の 4 つ。
- ドキュメントに「予定」「残」とあるだけのものは、実装とみなさない。
- FEATURES の never タグが付いたものも「無」とし、備考に never と書いた。
- 主要な判定と脇道の発見（§3）は、書き手がコードを開いて確かめ直した。
- 「無」の 100 項目は、すべて検索を流し直して件数を記録した。

| 領域 | 項目数 | 有 | 一部 | 無 | 方式差 |
|---|---:|---:|---:|---:|---:|
| A ワークツリー・サイドバー | 28 | 1 | 15 | 11 | 1 |
| B エージェント・セッション | 32 | 7 | 17 | 7 | 1 |
| C ターミナル・分割・復元 | 24 | 2 | 7 | 15 | 0 |
| D エディタ・ビューア・ナビ | 29 | 2 | 17 | 10 | 0 |
| E レビュー・Git・連携 | 35 | 1 | 12 | 22 | 0 |
| F ブラウザ・Design Mode | 16 | 0 | 2 | 14 | 0 |
| G リモート・モバイル・CLI | 30 | 4 | 17 | 7 | 2 |
| H アプリ全体・通知・配布 | 30 | 0 | 18 | 12 | 0 |
| X 追加（書き手が確認） | 2 | 0 | 0 | 2 | 0 |
| **計** | **226** | **17** | **105** | **100** | **4** |

「一部」が多いのは、necoder が骨格を持っていて周辺の作り込みが足りない、という形が大半だからだ。

## 1. 負けている所（利用者から見える順）

### 1.1 ツイートで名指しされた点の実態

| Orca の売り（ツイート） | necoder の現状 | 判定 |
|---|---|---|
| IDE 内でターミナルを開ける | 下ドックに端末あり。ただし分割・検索・URL リンクが無く、アプリを閉じると PTY が終わる（§1.2 S5） | 一部 |
| 左バーでセッション管理（Herdr 相当） | Fleet サイドバー・herd・要対応キュー。状態は ACP 由来で、推定に頼る Orca より誤検出が起きにくい作り（設計上の見込み・実運用の比較は未・R14）。フィルタ・一括操作・並べ替えは Orca が上（A11 / A13 / A14） | 有 |
| 右バーでファイル・Git 差分を選んで見る | エクスプローラ・ソース管理は左ドックに排他表示。**ソース管理の行を押しても diff が開かない**（§3-2）。Fleet の「変更」から開く diff はコミット済みだと空（§3-3） | 一部（不具合あり） |
| diff に ＋ でコメント → まとめて Claude へ送る | **無い** | 無 |
| Markdown / HTML 表示 | ⌘⇧V で閲覧プレビュー。Markdown の編集は raw のみ、Mermaid なし。SSH 先の HTML は不可 | 一部 |
| Docker やアプリの起動 | Orca は worktree ごとに起動中のポートを検出して開く / 止める Ports パネルを持つ。necoder には無い | 無 |
| 1 つのウィンドウで全部できる | Editor / Fleet / Chat の 3 モードで 1 窓に収まる。ただしブラウザと PR は外に出る | 一部 |

### 1.2 優先度つきギャップ

**S = Orca が選ばれている理由そのもの。** 無いと比較の土俵に上がれない。

| # | ギャップ | Orca | necoder の現状（根拠） |
|---|---|---|---|
| S1 | **diff 行コメント → 一括送信** | diff のガター ＋（`c` キー）で行・複数行範囲に Markdown コメント。「Send to agent」で行アンカー付きの 1 プロンプトに束ね、送り先のエージェントを選ぶ。Resolve と、未解決分の再送。Markdown のレンダ済みテキストへの注記、ブラウザページへの注記も同じ器 | 無。gutter から出るのは hunk メニューだけ（`crates/workspace/src/workspace/editor_area/diff.rs:239-317`）。添付はファイル単位（`crates/agent_panel/src/agent_panel.rs:4928-4974`） |
| S2 | **worktree の差分レビュー画面** | staged / unstaged / untracked を 1 本にした combined diff。比較基準を start-from / 任意コミット / ブランチに切替。横にファイルツリー、変更の無い領域の折り畳み、空白表示、画像 diff、3-way conflict UI | diff は 1 ファイルずつ、HEAD との unified diff を読み取り専用の一時タブに出すだけで、+/− の色も付かない（`crates/workspace/src/workspace/editor_area/diff.rs:24-66`）。まとめて見られるのは Fleet カードのフラットな変更一覧だけ |
| S3 | **内蔵ブラウザ + Design Mode** | worktree ごとの Chromium（タブ・履歴・DevTools・プロファイル・ビューポート）。Design Mode は要素クリックで HTML・計算済み CSS・切り抜きスクショ・ソース位置をエージェントへ。ブラウザ自動化 CLI（snapshot / click / fill） | 無（FEATURES never の範囲とぶつかる・§5）。WebView が読み込むのは `file://` と `necoder-artifact://` だけ（`crates/webview_view/src/webview_view.rs:68,105-113`）。**§2 に necoder 流の実装案** |
| S4 | **エージェントの幅と Claude Code の機能** | 35 種以上の CLI をワンクリック起動（任意 CLI も可）。`/` のスラッシュコマンドと skills の補完。サブエージェントを子行で表示 | ACP の 7 種（claude / codex / copilot / qwen / opencode / kimi / grok、`crates/acp_client/src/acp_client.rs:386-477`）。公開 ACP レジストリ 39 件は版の決定に使うだけで一覧に出さない。**AvailableCommandsUpdate を捨てている**ので `/` 補完が作れない（`crates/acp_client/src/acp_client.rs:1972`）。サブエージェント表示は無い |
| S5 | **閉じても走り続けるエージェント** | PTY をバックグラウンドの daemon が持つ。⌘Q・クラッシュ・更新再起動を越えて warm reattach し、閉じていた間の出力も scrollback に残る。SSH 先も relay の lease で PTY を残す | PTY はアプリ本体が持ち、Drop で終わる（`crates/terminal_view/src/terminal_view.rs:252-257,871-877`、`crates/host/src/host.rs:208-213`）。会話は session/load で冷再開できるが、実行中のターンは失う |
| S6 | **GitHub / Linear / Jira** | PR ビュー（checks・review・コメント返信・リアクション・auto-merge・stacked PR）。Actions の失敗ログ、「Fix broken checks」。Issue / Linear / Jira から worktree を作る。PR のレビューコメント（CodeRabbit 含む）を選んで AI に直させ、自動返信して resolve | `gh pr create --web` と `gh pr view --web`（ブラウザを開くだけ・`crates/project/src/project.rs:1084-1116`）。FEATURES §6 の「ホスティング連携 never」とぶつかる（§5） |

**A = 毎日の使い心地の差。** 「necoder で丸一日」の穴。

| # | ギャップ | 中身（根拠は付録） |
|---|---|---|
| A1 | ターミナルの基本 | 分割（C02）・検索（C07）・OSC 52（C08）・URL リンクとポップオーバー（C09）が無い。**Shift+Enter も `\r` なので TUI エージェントで改行できない**（`crates/terminal_view/src/terminal_view.rs:936-937`、C12）。フォント・サイズ・scrollback・ANSI 色・シェルは固定（C11/C22/C24）。Quick Commands（C14）・閉じる前の確認（C21）・ファイルのドロップ（C17）も無い |
| A2 | Git パネルの基本 | discard・amend・force-with-lease・Sync・状態で変わる主ボタンが無い（E10/E13/E14）。conflict の解決 UI も無い（E05）。加えて §3 の不具合 4 件（hook が走らない・diff が開かない・失敗が見えない・⌘V が効かない） |
| A3 | 使用量・アカウント | Claude / Codex の 5 時間枠・週枠・reset までの時間・80% 警告（B15）。アカウントの hot-swap（B14）。日別チャートと推定コスト（B16） |
| A4 | 通知 | OS のデスクトップ通知・Dock バッジ・通知履歴（ベル）・「未読に戻す」が無い（H01-H03）。スマホの Web Push は承認待ちと質問だけで、完了は通知しない（G19） |
| A5 | Ports | worktree ごとに起動中のポートを検出し、開く / 止める（X1）。SSH のポート転送（自動検出・1 クリック・再接続後も維持）（G08） |
| A6 | worktree 作成の自由度 | 起点（任意ブランチ・コミット・リモートブランチ）・ブランチ名・repo ごとの base ref（A01/A02/A04）。`.worktreeinclude` と共有パス（A05）。作成中の進捗・キャンセル・Retry（A03）。一括操作と片付け画面（A11/A16）。課題から作る（A04/E28） |
| A7 | エディタの所作 | 自動保存（D01・FEATURES v1 のまま未実装）・プレビュータブ（D02）・Markdown のリッチ編集と Mermaid（D03/D05）・CSV 表（D08）・画像 diff（D07）・パス:行のコピー（D15）・フォルダ内検索（D18）・Open in 外部エディタ（D29） |
| A8 | セッション履歴 | `~/.claude`・`~/.codex` など CLI 自身の過去セッションを走査して再開・resume コマンドをコピー（B09）。全スレッドの全文検索（B10）。「新しいセッションで続ける」（B11） |
| A9 | 設定 | 設定の検索（H08）・キー割り当ての GUI（H09）・UI ズームとフォント（H10）・言語切替の UI（H12） |

**B = あれば勝てるが、今すぐでなくてよい。**
スケジュール実行（G24・never）、Skills の配布（G25）、Computer Use（F12）、iOS / Android エミュレータ（F13）、Cloud VM レシピ（G13・never 相当）。
ヘッドレスのサーバ運用（G12）、モバイルでのファイル・端末・stage/commit・添付（G15-G18）、音声入力（H07。Orca は日本語のオンデバイス STT も持つ）。
プラグイン（H19・never / later）、6 言語 UI（H12）、配布の幅（Intel mac・Homebrew・Linux・Windows インストーラ、H18）、Resource Manager（H21）、オンボーディングのツアー（H13）、artifact の公開リンク（F11）。

## 2. Design Mode を necoder に入れる道筋

Chromium を同梱しなくても作れる。necoder はすでに `webview_view`（wry 経由で macOS は WKWebView、Windows は WebView2）を持っている。
Design Mode に要る「スクリプト注入・ページからの通知・切り抜きスクショ」は、wry と各 OS の WebView API でそろう。

**Orca の作り**（MIT なので手法もコードも参照してよい。参考にしたらファイル冒頭に出典を残す運用）:

- 注入スクリプトは、テストを除いて 971 行（`os:src/main/browser/grab-guest-*.ts`）。ホバー枠を出し、クリックで確定する。
- 集める情報は次のとおり（`os:src/shared/browser-grab-types.ts`）。
  - 要素の HTML（予算で切る）と近傍テキスト、祖先パスとセレクタ。
  - 計算済みスタイル 16 項目（display / position / width / height / margin / padding / color / background / border / border-radius / font-family / font-size / font-weight / line-height / text-align / z-index）。
  - role / aria と矩形。
- React の開発ビルドなら、fiber からコンポーネント名と `_debugSource` のファイル:行を取る（`os:src/main/browser/grab-guest-react-script.ts:73`）。
- スクショはホスト側でページを撮り、矩形で切り抜く（`os:src/main/browser/browser-grab-screenshot.ts`・106 行）。
- 複数の要素を溜めて、送り先を選んで送る画面側の注記 UI は、テストを除いて 4,390 行ある（`os:src/renderer/src/components/browser-pane/annotate/`）。
- 送信前に URL の query を落とし、secret らしい値を伏せる。

**necoder での部品**:

1. **localhost 限定のプレビュータブ**: 読み込み先を `http://localhost|127.0.0.1|[::1]:<port>` まで広げる。任意 URL には広げない（汎用ブラウザにしない）。
   - 入口 1: ターミナル出力の URL をクリック。`ui::links` は既に `localhost:port` を 1 本の URL として拾える（`crates/ui/src/links.rs:368`）。
   - 入口 2: パレット「プレビュー: URL を開く」。
   - 入口 3: 将来は Ports 検出（A5）から開く。
2. **注入と切替**: wry の `with_initialization_script` で picker を入れておき、パンくずの再読込ボタンの隣に置く Design Mode ボタンから `evaluate_script` で有効化する。Esc で抜ける。
3. **ページ → necoder**: `with_ipc_handler`（`window.ipc.postMessage`）で受ける。
   - IPC を持つのは localhost プレビュータブの Design Mode 中だけにする。スキーマを検証し、サイズの上限を設ける。
   - Chat の artifact は「IPC を付けない」隔離のまま触らない（`crates/webview_view/src/sandbox.rs:11-13`）。
4. **切り抜きスクショ**: macOS は WKWebView の `takeSnapshotWithConfiguration`（矩形指定可・画面収録の権限は要らない）。Windows は WebView2 の `CapturePreview` を切り抜く。wry は各 OS の WebView ハンドルを出すので、objc2 / webview2-com で書ける。
5. **composer へ添付**: 画像添付（`PromptWithImages`・`chat-paste/`）とチップは既にある。
   - 「要素チップ」は PNG と文脈テキスト（ページ・要素ラベル・セレクタ・HTML・スタイル・ソースの file:line）の組にし、アクティブなスレッド（Fleet なら Task のスレッド）へ付ける。
   - 複数の要素を溜めて 1 通で送る。これは S1 の「diff 行コメントをまとめて送る」と同じ器で作れる。
6. **確認ループ**: dev server の HMR がページを更新するので、necoder 側は何もしなくてよい。

**Orca より良くできる所**:

- **React 19 でもソース位置を取る。** React 19 は fiber の `_debugSource` を廃止しており、Orca の方式では取れない（[facebook/react #32574](https://github.com/facebook/react/issues/32574)）。`_debugStack` を source map で解決するか、ビルド時に `data-*` 属性を埋める Vite / Babel プラグイン方式（[xray](https://github.com/ivanstnsk/xray) など）に最初から対応する。
- **ACP の画像ブロックで送る。** 端末への貼り付けを介さない。
- **ブラウザエンジンを常駐させない。** 隠れた WebView は既存の仕組みで 15 分後に破棄する（`crates/webview_view/src/webview_view.rs:139-201`）。

**規模**: 要素を 1 つ選んで添付して送る最小版なら、注入 JS と Rust 側（URL タブ・IPC・2 OS のスナップショット・チップ）で数日だろう。
Orca 並みの注記トレイ（複数の要素を溜める・編集する・送り先を選ぶ）まで作ると、1〜2 週間規模になる。
ただし S1（diff の行コメント）とトレイを共有できるので、両方を別々に作るより安く済む。
cmux も同じ種類の機能を持っている（[cmux #8533](https://github.com/manaflow-ai/cmux/pull/8533)）。

**判断が要ること**: FEATURES §9 の never は「拡張 / API 向けの汎用 webview」、[x] 項目は「Chromium を同梱しない」で、localhost 限定のプレビューはどちらにも正面からは当たらない。
ただし「URL を開くタブ」をどこまで許すかはタグ管理者（本人）の判断になる。任意の URL まで広げると、汎用ブラウザ（never）に踏み込む。

## 3. ついでに見つかった不具合・文書のずれ（すべてコードで裏取り済み）

コードの抜粋と確認に使ったコマンドは、証拠ファイルの §12 にある。

| # | 何が起きるか | 根拠 |
|---|---|---|
| 1 | **パネルからのコミットで pre-commit / commit-msg フックが走らない**（push の pre-push も同じ）。未信頼 repo を開いた時の自動 `git status` 対策として、全 git 呼び出しに `-c core.hooksPath=/dev/null` を付けている巻き添え。Orca はフックを普通に走らせる | `crates/project/src/project.rs:414-432`（`run_git`）→ `commit_on`（:951） |
| 2 | **ソース管理パネルの変更行を押しても何も起きない**。`GitPanelEvent::OpenDiff` の受け側はあるが、送り手が 1 か所も無い。行に click ハンドラも無い | 受け側 `crates/workspace/src/workspace/panels.rs:91`・定義 `crates/git_ui/src/lib.rs:27`・行 `crates/workspace/src/workspace/git_view.rs:462-537` |
| 3 | **Fleet の「変更」一覧から開く diff が、エージェントがコミット済みだと空になる**。一覧は Task の base 起点だが、押すと HEAD 比較の diff を開き、差分が無ければ stderr に出して何も開かない | `crates/workspace/src/workspace/fleet_stage.rs:437-441` → `crates/workspace/src/workspace/editor_area/diff.rs:24-58`（「差分なし（HEAD と同一）」） |
| 4 | **コミット・push・pull の失敗が画面に出ない**（`eprintln!` だけ） | `crates/workspace/src/workspace/git_controller.rs:366,476-480` |
| 5 | **コミットメッセージ欄で ⌘V が効かない**。EditorView ではない自前入力で、⌘ / ⌃ 付きのキーを捨てている。input handler も無いので、日本語 IME での入力も怪しい（実機未確認） | `crates/workspace/src/workspace/git_controller.rs:307-309` |
| 6 | **MANUAL §4「ファイル操作は取り消せる」が実装と合わない**。エクスプローラのファイル操作に undo は無い（ゴミ箱へ移すだけ） | `docs/MANUAL.md:131`。keymap にエクスプローラ用の割当なし |
| 7 | **MANUAL §13 の `density`（compact / cozy）が効かない**。型と既定値だけで、描画から参照されていない | `crates/settings_core/src/settings_core.rs:173,279` 以外に参照なし |
| 8 | ACP の `AvailableCommandsUpdate`（スラッシュコマンド一覧）と、エージェント側の会話名の更新を捨てている | `crates/acp_client/src/acp_client.rs:1972`（`_ => {}`） |
| 9 | 死にコード: 設定 `fleet_goal`（参照 0）、`Storage::token_ledger()`（テストのみ）、`Picker::set_query_action`（呼び出し 0） | `crates/settings_core/src/settings_core.rs:211-213`、`crates/storage/src/storage.rs:1637`、`crates/ui/src/ui.rs:203` |

## 4. necoder が既に勝っている所

> **注（2026-09-26・UX-CODE-REVIEW R14）**: この節は**設計上の利点**の整理で、Orca との**同条件の比較実測ではない**。
> 「軽い」の数字（idle RSS 122MB・起動 約 215ms）は 2026-07-17 の necoder 単体の記録で、今の全機能版の値でも、
> Orca と同じ端末・repo・エージェント数で測った値でもない。「誤検出が無い」も ACP の構造からの推定で、実運用での
> 比較はしていない。優位を断定する前に、R14 の確認条件（1 / 5 / 10 Task・入力 p95・RSS・起動・本体とエージェント等を
> 分けた報告）で測り直す。

- **本物のエディタ。**
  - 7 系統の LSP（定義・参照・rename・code action・整形・診断・シンボル。SSH 先でも動く）と、tree-sitter の増分ハイライトがある。
  - multi-cursor、hunk 単位の stage・巻き戻し（undo 可）、行末の blame、⌘I のインライン編集（端末ではコマンド生成）もある。
  - Orca は Monaco 標準のハイライトだけで、型検査は「ターミナルで回せ」の方針（`od:editing/monaco.mdx` §Language support）。
- **軽い。**
  - GPUI ネイティブで、idle RSS は 122MB、起動は約 215ms（**2026-07-17 の necoder 単体の実測・今の全機能版ではない**）。WebView は必要な時だけ作って 15 分で捨て、端末は出力がある時だけ起きる。
  - Orca は Electron で、worktree ごとに Chromium を持つ。Orca への批判は「編集が遅い」「重い」の 2 本に集まる（captain-orchestrators 調査）。
- **ACP の構造化。**
  - ツール単位で承認でき、書き込み前の内容を content-addressed の checkpoint に残して巻き戻せる。
  - 質問カード（elicitation）、エージェントが広告したモデル候補、認証切れや切断からの自動回復がある。
  - Orca は既定で全エージェントを permission bypass で起動し、状態は OSC タイトルと hook から推定して誤検出と戦っている。
- **Fleet の統制。**
  - Task の状態は自動で遷移し、監査ログに残る。Conflict Radar（統合前の衝突予測）と、人間が押す Integrate がある。削除前には「失うコミット数」を数える。
  - Captain は、権限を necoder がコードで強制し、分解案を人間が承認した分だけ worktree を切り、ACP のターンで起こす。
  - Orca の coordinator は、idle の PTY に Enter を打つ方式の誤配送が未解決。
- **Remote SSH。**
  - system OpenSSH と token 付き askpass で、秘密を保存しない。static-musl の server を checksum 検証つきで自動配備するので、ツールチェインの無い Linux でも端末まで動く。
  - リモート端末の `ne` は接続元の GUI で開くが、開く以外の権限は渡さない。
- **スマホ連携。** アカウント不要で端末間を E2E 暗号化するので、relay は中身を読めない。承認は差分を確かめてから行う。
- **プライバシー。** テレメトリを持たない。Orca は PostHog へ送る opt-out 方式。
- **通知の細かさ。** 完了と入力待ちで音を分け、スレッド単位で判定し、エージェント別にミュートできる。
- **artifact の隔離。** CSP `connect-src 'none'`、IPC 無し、`..` やシンボリックリンクでの脱出は 403。

## 5. FEATURES の never / later とぶつかる項目（本人の判断待ち）

「全部で勝つ」には、いくつかのタグを見直す必要がある。タグはユーザー管理なので、ここでは並べるだけにする。

| タグ（FEATURES.md） | ぶつかる Orca 機能 | 補足 |
|---|---|---|
| §6 never「ホスティング連携」 | PR ビュー・Checks・auto-merge・stacked PR・Issues・GitHub Projects・GitLab / Bitbucket（E17-E25, E29, E30, A20） | すでに `gh pr create --web` / `gh pr view --web` が never の中で最小限だけ入っている |
| §6 never「graph / stash UI」 | stash（E35） | コミットグラフ（30 件）はすでに実装済み（`crates/workspace/src/workspace/git_view.rs:542-696`）で、タグの方が実装とずれている |
| §9 never「拡張 / API 向けの汎用 webview」 | 内蔵ブラウザ一式（F01-F16）、Design Mode | §2 の localhost 限定なら正面からは当たらない |
| §9 never「Marketplace」 / later「WASM ホスト」 | プラグイン（H19） | |
| §12 never「Cloud Agents / Automations 相当」 | スケジュール実行（G24）、Cloud VM レシピ（G13） | |
| §13 never「telemetry・notebooks・Web 版」 | テレメトリ（H15）、Jupyter（D09）、Web クライアント（G12） | テレメトリは思想差として残す方が自然 |
| §2 / §7 later | ミニマップ（D11）、端末の分割 / tasks.json / シェル統合（C02 / C14 / C20） | 端末の分割は A1 に入れるべき重さ |

## 付録: 全 226 項目の判定

この表は要約だ。各行の根拠（Orca の出典・necoder の `file:line`・「無」の検索と件数）は、[`orca-gap-2026-09-evidence.md`](./orca-gap-2026-09-evidence.md) の同じ ID にある。
パスはリポジトリのルートからの相対で書いた。
- `os:` は、Orca のリポジトリのルートからの相対。
- `od:` は、Orca の公式ドキュメント（`docs/site/content/docs/`）からの相対。
- どちらも `stablyai/orca@646e9a5` 版。

### A. ワークツリー・ワークスペース・サイドバー

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| A01 | リポジトリ追加と repo ごとの base ref | 一部 | 追加（フォルダ / 最近 / SSH）はある。base は統合先 worktree の HEAD に固定（`crates/project/src/project.rs:767-788`） |
| A02 | 作成ダイアログ（空なら自動命名・start-from） | 一部 | 名前入力と -2 / -3 の連番のみ。起点ブランチ・SHA・リモートブランチは選べない（`crates/workspace/src/workspace/new_task_dialog.rs:64-112`） |
| A03 | 背景作成（進捗行・キャンセル・Retry） | 一部 | 即閉じと背景実行はある（`crates/workspace/src/workspace/fleet_view.rs:506-518`）。進捗・キャンセル・Retry は無い |
| A04 | ブランチ名の指定・課題から命名 | 無 | 常に `task/<slug>`（`crates/project/src/project.rs:3139-3150`） |
| A05 | 共有パス・`.worktreeinclude`・`sharedDirectories` | 一部 | `worktree-setup.sh` の雛形で `.env` のコピー等は書ける。宣言的な共有・clone・symlink は無い |
| A06 | 作成後の setup hook | 一部 | `.necoder/worktree-setup.sh`（`crates/project/src/project.rs:3157-3170`）。実行ごとの run / skip 切替と設定 UI は無い |
| A07 | 親子 worktree（ネスト表示・子孫ごと Sleep / Delete） | 無 | depends_on は待ち合わせ専用で表示されない |
| A08 | 絵文字の名前（:rocket:） | 無 | slug 化で非 ASCII は潰れる |
| A09 | ピン留め | 一部 | 舞台（多列）へのピンのみ。再起動で消える（`crates/workspace/src/workspace.rs:1151`） |
| A10 | Sleep / Archive / Delete | 一部 | Archive・確認つき削除・idle 自動停止はある。手動の休眠は無い |
| A11 | 複数選択の一括操作 | 無 | |
| A12 | インライン改名・詳細ダイアログ | 一部 | ダブルクリック改名はある（`crates/workspace/src/workspace/fleet_sidebar.rs:583-592`） |
| A13 | サイドバーのフィルタ（Sleeping / 既定ブランチ / CLI 作成 / detached 等） | 一部 | 選択リポジトリへの自動絞り込みのみ |
| A14 | グループ・並べ替え・サイドバー内検索 | 一部 | プロジェクト別の表示と ⌘O はある |
| A15 | 外部 worktree の検出と表示切替 | 一部 | ⌘O と ⎇ に全 worktree が出る。出所別の表示切替は無い |
| A16 | 片付け画面（状態・活動・サイズで一括削除） | 無 | |
| A17 | 残ったブランチのレビュー | 方式差 | 削除前に未統合コミット数を見せる（`crates/workspace/src/workspace/worktree_delete.rs:28-42`） |
| A18 | マルチリポの project group | 無 | |
| A19 | repo アイコン・バッジ色 | 一部 | 色（プリセット + hex）は同等。アイコンは `.necoder/icon.png` か設定の手書き |
| A20 | worktree カードに課題を表示 | 無 | never（ホスティング連携） |
| A21 | 進捗コメントとカード状態をエージェントが更新 | 有 | `fleet status` / MCP `fleet_update_task`。状態は自動で遷移する（`crates/workspace/src/workspace/notifications.rs:38-46`） |
| A22 | 状態別ボード | 無 | 旧管制のパイプラインは F3 で削除 |
| A23 | 自動命名ブランチを作業内容から改名 | 無 | |
| A24 | sparse checkout プリセット | 無 | |
| A25 | 外で消された worktree の自動掃除 | 一部 | git の変化には追従する。消えた worktree の片付けは無い |
| A26 | 削除ショートカット・名前のコピー | 無 | ホバーの 🗑 はある |
| A27 | 同じ依頼を N 体へ fan-out して比べる | 一部 | ＋Task の連打・舞台 3 列・Captain の分解案はある。1 操作での fan-out と比較画面は無く、GUI の ＋Task では担当エージェントを選べない（`crates/workspace/src/workspace/fleet_view.rs:484`） |
| A28 | Run on（local / SSH / サーバ / VM）を選ぶ | 一部 | 元プロジェクトの host に自動で作る |

### B. エージェント・セッション・状態・使用量・チャット

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| B01 | 35 種以上の CLI エージェント + 任意 CLI | 一部 | ACP の 7 種（`crates/acp_client/src/acp_client.rs:386-477`）。レジストリ 39 件は一覧に出ず、任意の CLI は足せない。Gemini は後継の Antigravity が ACP 非対応のため除外（`crates/acp_client/src/acp_client.rs:413-415`） |
| B02 | 検出・自動導入・有効 / 無効 | 一部 | PATH 検出と 1 クリック導入・ログインはある（`install.rs`）。有効 / 無効の切替は無い |
| B03 | 状態の検出と表示 | 有 | Idle / Working / Blocked / Done を形と動きで表し、全画面で共用（`crates/agent_panel/src/agent_panel.rs:785-811`） |
| B04 | Agent Dashboard（カンバン・検索・フィルタ・pop-out） | 一部 | herd・要対応キュー・titlebar バッジはある |
| B05 | Agent map | 有 | 編隊図 4 表示（`crates/workspace/src/workspace/fleet_view.rs:1219-1263`） |
| B06 | Restart chip | 有 | 「再開」チップと session/load（`crates/agent_panel/src/agent_panel.rs:3682-3748`） |
| B07 | 権限の既定（Yolo / Manual 一括）・起動引数の上書き | 一部 | モードはピルで選び次回へ持ち越す。起動引数の上書きは `agent_servers` の JSON だけ |
| B08 | hibernation | 有 | 既定 ON（15 分）。次の送信で session/load（`crates/agent_panel/src/idle.rs:46-80`） |
| B09 | 各 CLI のディスク上 transcript を走査して再開 | 一部 | ⌘⇧H は necoder の DB にあるスレッドだけ |
| B10 | 全エージェント横断の全文検索 | 一部 | 会話内の ⌘F と Chat の横断検索のみ（`crates/storage/src/storage.rs:1283-1327`） |
| B11 | Continue in New Session（handoff） | 一部 | Captain 席だけが自動で交代する（`crates/agent_panel/src/seat.rs:188-229`） |
| B12 | サブエージェントの子行 | 無 | |
| B13 | Claude Agent Teams | 無 | |
| B14 | アカウント hot-swap | 無 | |
| B15 | 使用量・レート制限（5h / 週・reset・80% 警告） | 無 | トークンメーターは文脈窓の使用量で、Σ はスレッドの合計 |
| B16 | Stats（日別チャート・推定コスト） | 無 | |
| B17 | Keep computer awake | 一部 | スマホ連携中の caffeinate のみ（`relay/host/bridge.mjs:307-316`） |
| B18 | composer（添付・`/` コマンドと skills・ピル） | 一部 | 添付・画像・ピルはある。`/` 補完は無い（§3-8） |
| B19 | 構造化質問カード | 有 | elicitation に対応（`crates/agent_panel/src/agent_panel.rs:8885-8899`）。自由入力の Other は未対応 |
| B20 | message rail | 一部 | 「前の指示へ」と固定帯（`crates/agent_panel/src/agent_panel.rs:7543-7546`） |
| B21 | rewind・/clear・/compact | 一部 | checkpoint が戻すのはファイルだけ。/compact はボタンがある |
| B22 | inline diff・plan・ツールのバッチ・BG タスク停止 | 一部 | inline diff と plan はある |
| B23 | 履歴から構造化チャットで再開 | 一部 | necoder のスレッドのみ |
| B24 | Chat ⇄ TUI の切替 | 方式差 | ACP 一本で、TUI は無い |
| B25 | 端末で直接起動したエージェントの状態検出 | 無 | 端末は Title / OSC を捨てている（`crates/terminal_view/src/terminal_view.rs:352-360`）。ROADMAP M14 の未チェック項目 |
| B26 | `.claude` / `.codex` のフックと CLAUDE.md を尊重 | 有 | `crates/acp_client/src/preset.rs:33-35` |
| B27 | Codex goal | 無 | |
| B28 | エージェント別の新規タブのキー | 一部 | ⌘⇧A（既定のエージェントのみ） |
| B29 | 完了通知・未読 | 一部 | 音・トースト・ミュート・未確認 Done はある。OS 通知・Dock・ベルは無い |
| B30 | Agents feed（横断フィード） | 一部 | Fleet 下段のニュース（`crates/workspace/src/workspace/fleet_view.rs:2535-2614`）。クリックでの移動・未読・フィルタは無い |
| B31 | 会話名の同期 | 一部 | necoder が claude -p / codex exec で命名する。エージェント側の会話名は捨てている |
| B32 | モデル一覧を動的に取得 | 有 | ACP の広告値を使う（`crates/agent_panel/src/agent_panel.rs:842-853`） |

### C. ターミナル・ペイン・セッション復元

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| C01 | 描画品質（xterm + WebGL） | 一部 | GPUI ネイティブ。下線・取消線・DIM・カーソル形状・リガチャ・マウス報告は無い（`crates/terminal_view/src/terminal_view.rs:1300-1324`） |
| C02 | 端末の分割（無限・ネスト） | 無 | タブの列だけ（`crates/terminal_view/src/dock.rs:23-34`） |
| C03 | タブのドラッグで分割・異種タブの混在 | 一部 | ドラッグは並べ替えのみ |
| C04 | worktree ごとのレイアウト保持 | 有 | ProjectSession が保持（`crates/workspace/src/workspace/project_session.rs:67-95`） |
| C05 | アプリ終了を越えて PTY が生きる（daemon） | 無 | §1.2 S5 |
| C06 | scrollback の永続化 | 無 | メモリ上の 10,000 行のみ |
| C07 | 端末内検索（regex・件数） | 無 | |
| C08 | OSC 52 | 無 | ClipboardStore を捨てる（`crates/terminal_view/src/terminal_view.rs:352-363`） |
| C09 | リンクのポップオーバー・URL・OSC 8 | 無 | リンクになるのは file:line だけ |
| C10 | Copy Context | 無 | 右クリックメニューが無い |
| C11 | 端末テーマのライブラリ・Ghostty / Warp の取り込み | 一部 | 前景・背景・カーソルだけテーマに追従し、ANSI 16 色は固定 |
| C12 | kitty keyboard・Shift+Enter・¥ → \ | 無 | Shift+Enter も `\r`（`crates/terminal_view/src/terminal_view.rs:936-937`） |
| C13 | Floating terminal | 一部 | repo 外の Chat モードはあるが端末が無い |
| C14 | Quick Commands | 無 | FEATURES later（tasks.json） |
| C15 | インライン画像 | 無 | |
| C16 | Windows のシェル選択 | 一部 | pwsh を自動で選ぶだけ |
| C17 | 端末へのファイルドロップ | 無 | |
| C18 | 端末のショートカット（⌘T・⌘⇧\ 等） | 一部 | ⌘J。⌘\ はエディタのみ |
| C19 | 端末タブの改名 | 一部 | 「ターミナル N」で固定 |
| C20 | シェル統合（OSC 133） | 無 | FEATURES later |
| C21 | 実行中の端末を閉じる前の確認 | 無 | ⌘Q で即終了（`crates/necoder/src/main.rs:723-744`） |
| C22 | シェルの引数・既定のシェル | 無 | 端末に設定の入口が無い |
| C23 | file:line のクリック | 有 | `crates/terminal_view/src/terminal_view.rs:48-91` |
| C24 | 端末のフォント・scrollback・カーソル | 無 | 12.5pt・10,000 行で固定 |

### D. エディタ・ビューア・エクスプローラ・ナビゲーション

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| D01 | 自動保存 | 無 | FEATURES v1 のまま。hot exit は DB 退避のみ |
| D02 | プレビュータブ | 無 | |
| D03 | リッチ Markdown 編集（スラッシュメニュー・表・目次・front matter） | 一部 | ⌘⇧V の閲覧プレビューのみ（`editor_view/src/markdown_preview.rs`） |
| D04 | Markdown のレビュー注記 | 無 | |
| D05 | Mermaid | 無 | |
| D06 | PDF | 一部 | OS のビューアでタブ表示。SSH 先もローカル複製で見られる（`pdf_view.rs`） |
| D07 | 画像・画像 diff（swipe / onion） | 一部 | 画像タブはある。diff と拡大は無い |
| D08 | CSV / TSV の表 | 無 | |
| D09 | Jupyter notebook | 無 | never |
| D10 | HTML プレビュー（SSH・隔離・横に） | 一部 | ローカルの .html と自動再読込のみ |
| D11 | ミニマップ | 無 | FEATURES later |
| D12 | タブ内の Changes view | 一部 | 別タブの unified diff |
| D13 | Word wrap（diff 用は別設定） | 一部 | ⌥Z と既定値の設定はある |
| D14 | フォント・UI ズーム・density | 一部 | エディタの font_size のみ。density は効いていない（§3-7） |
| D15 | パス:行のコピー（⌘⌥C） | 一部 | パス・相対パスのみ |
| D16 | 自然順・git 色・右クリックで discard / stage | 一部 | git 色と即時反映はある |
| D17 | ドロップ（MD へ画像・OS クリップボードへファイル） | 一部 | Finder からツリーへのコピーのみ |
| D18 | フォルダ内検索 | 無 | |
| D19 | ⌘P（recency・gitignored の 2 パス目） | 一部 | 5 万件を 16ms 以内（ベンチあり） |
| D20 | 新規タブの omnibox | 無 | |
| D21 | Jump palette（タブ検索・最近のセッション・PR #・作成行） | 一部 | ⌘O の 2 階層と ⌘⇧U |
| D22 | 設定の検索 | 無 | |
| D23 | キー割り当ての GUI | 一部 | keymap.json は保存で即反映 |
| D24 | 言語機能 | 有（優位） | LSP 7 系統・tree-sitter（`crates/lang/src/lsp.rs:36-74`） |
| D25 | 大規模での性能 | 一部 | rope・仮想化・CI ベンチ。ツリーの行は仮想化していない |
| D26 | 言語の幅 | 一部 | tree-sitter 15 言語。Orca は 53 言語 ID（Java・C#・Ruby・PHP・Swift・Kotlin・SQL・Dockerfile 等が necoder では無色） |
| D27 | Find の種込み・F7 | 有 | |
| D28 | 全部閉じる・タブのピン留め | 一部 | 「他を閉じる」「右側を閉じる」はある |
| D29 | Open in（VS Code / Cursor / Zed / ターミナル） | 一部 | 既定アプリと Finder のみ |

### E. レビュー・Git・連携

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| E01 | combined diff | 一部 | 1 ファイルずつ HEAD 比較（`crates/workspace/src/workspace/editor_area/diff.rs:24-66`） |
| E02 | 比較基準の切替 | 一部 | 固定（Fleet の一覧は base、diff タブは HEAD） |
| E03 | diff 横のファイルツリー | 一部 | Fleet カードのフラットな一覧 |
| E04 | 変更の無い領域の折り畳み・空白表示 | 一部 | 前後 3 行で固定 |
| E05 | 3-way の conflict UI・Abort | 無 | 事前の Conflict Radar は necoder の優位 |
| E06 | diff の行コメント | 無 | §1.2 S1 |
| E07 | まとめてエージェントへ送る | 無 | |
| E08 | Resolve・再レビュー | 無 | |
| E09 | AI 帰属（行単位） | 一部 | ファイル単位のスレッド色（`crates/workspace/src/workspace/notifications.rs:290-306`） |
| E10 | Source Control パネル（discard・Sync・主ボタン） | 一部 | stage・commit・push・pull はある（§3-2 / 4 / 5） |
| E11 | AI のコミットメッセージ | 有 | claude -p（`crates/project/src/project.rs:1126-1152`） |
| E12 | フック失敗時の Fix with AI | 無 | そもそもフックが走らない（§3-1） |
| E13 | force-with-lease | 無 | |
| E14 | amend | 無 | |
| E15 | conflict を AI へ渡す | 無 | |
| E16 | AI アクションのレシピ（repo ごと） | 無 | |
| E17 | PR 作成（アプリ内・AI で詳細生成） | 一部 | `gh pr create --web` |
| E18 | stacked PR | 無 | never |
| E19 | PR ビュー | 無 | never |
| E20 | auto-merge・merge queue | 無 | never |
| E21 | CI 失敗・Actions のログ | 無 | never |
| E22 | GitLab・Bitbucket・Azure・Gitea | 無 | never |
| E23 | worktree ごとの PR 状態 | 一部 | ↗ でブラウザを開く |
| E24 | Issues | 無 | never |
| E25 | GitHub Projects | 無 | never |
| E26 | Linear | 無 | |
| E27 | Jira | 無 | |
| E28 | 課題から worktree を作る | 無 | |
| E29 | GitHub API budget | 無 | never。ポーリングしないので実害は小さい |
| E30 | プロジェクトごとの gh アカウント | 無 | never |
| E31 | ブランチ行と +/- 行数 | 一部 | Task base 起点の +N −M と ⌘O の ↑↓ |
| E32 | 変更行のパスコピー | 一部 | タブとツリーにはある |
| E33 | diff から HTML を横にプレビュー | 一部 | ツールカードの ▣ プレビュー |
| E34 | コミット署名・外部エディタ | 無 | gitconfig の gpgsign はおそらく効く |
| E35 | Git 操作の総量 | 一部 | ブランチ・worktree・hunk・blame・履歴 30 件・Radar・Integrate。stash は無い（never） |

### F. 内蔵ブラウザ・Design Mode・自動化

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| F01 | worktree ごとの内蔵ブラウザ | 無 | never（WebView は file:// と necoder-artifact:// のみ） |
| F02 | DevTools | 一部 | プレビューの Web Inspector（`crates/webview_view/src/webview_view.rs:287-289`） |
| F03 | プロファイル・cookie の取り込み・passkey | 無 | never |
| F04 | ビューポート・デバイスのエミュレーション | 無 | never |
| F05 | ダウンロード | 無 | never |
| F06 | Design Mode | 無 | §2 |
| F07 | ページへの注釈 | 無 | |
| F08 | リンクの開き先の選択 | 無 | Markdown プレビューのリンクはクリックできない（`crates/editor_view/src/markdown_preview.rs:285-293`） |
| F09 | ブラウザ自動化 CLI | 無 | never |
| F10 | リモート経由のブラウザ通信 | 無 | |
| F11 | artifact の公開リンク | 無 | ROADMAP M16 で後回し |
| F12 | Computer Use | 無 | |
| F13 | iOS / Android エミュレータ | 無 | |
| F14 | HTML を横にプレビュー | 一部 | ローカルの .html のみ |
| F15 | Web 検索 | 無 | never |
| F16 | ページのズーム | 無 | never |

### G. リモート・モバイル・CLI・オーケストレーション

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| G01 | SSH ターゲットの登録 UI・接続テスト | 一部 | `~/.ssh/config` のピッカー・履歴・手入力（`crates/workspace/src/workspace/remote_ssh.rs:46-112`） |
| G02 | ホスト鍵の検証と案内 | 一部 | OpenSSH 任せで、指紋は askpass に出る。失敗の詳細は出ない |
| G03 | ProxyJump・多重化・GSSAPI・FIDO2 | 一部 | ControlMaster が常時有効で、config に従う |
| G04 | パスフレーズの保持 | 一部 | ControlMaster が生きている間だけ |
| G05 | SSH 上の worktree とエージェント | 有 | host 経由（`crates/workspace/src/workspace/fleet_view.rs:491-515`）。CLI の `fleet create` だけ local 固定 |
| G06 | 切断を越えて PTY が生きる | 一部 | 会話は session/load で続く。PTY は切断で終わる |
| G07 | 接続状態の表示と自動再接続 | 一部 | statusbar のチップと heartbeat |
| G08 | ポート転送 | 無 | |
| G09 | リモートのダウンロード・アップロード | 無 | |
| G10 | VS Code Remote-SSH で開く | 方式差 | necoder 自体が Remote SSH エディタ |
| G11 | ツールチェインの無い Linux でも動く | 有（優位） | static-musl の server で端末まで動く |
| G12 | Remote Orca Server・`orca serve` | 一部 | スマホ PWA 向けのペアリングのみ |
| G13 | Cloud VM レシピ | 無 | never 相当 |
| G14 | ネイティブのモバイルアプリ | 方式差 | PWA + Web Push（DECISIONS） |
| G15 | モバイル: 全ホスト一覧・ファイル・端末 | 一部 | スレッドの一覧と会話 60 項目 |
| G16 | モバイル: 返信・添付・音声 | 一部 | 返信・中断・承認・質問への回答 |
| G17 | モバイル: ソース管理 | 一部 | 読み取り専用の git diff |
| G18 | モバイル: アカウント・workspace 作成 | 無 | |
| G19 | モバイル: 完了の push 通知 | 一部 | 承認待ちと質問だけ |
| G20 | CLI: worktree / repo と selectors | 一部 | `necoder fleet create/list/status/digest`・`ne <path>` |
| G21 | CLI: 任意の端末の read / send / wait | 一部 | エージェント向けの spawn / send / wait のみ |
| G22 | CLI: file open / diff | 一部 | `ne <path>` のみ |
| G23 | Orchestration（Run・DAG・メッセージ・gate・別ホスト） | 一部 | DAG・events・Captain・承認はある |
| G24 | スケジュール実行（cron / RRULE） | 無 | never |
| G25 | Skills の配布と更新 | 無 | |
| G26 | MCP サーバの登録 | 有 | 設定の MCP ページと、他ツールの設定の自動発見（`acp_client/src/mcp.rs`） |
| G27 | ヘッドレスでのアカウント登録 | 無 | |
| G28 | ヘッドレス運用・QR ペアリング | 一部 | `ne remote pair` の QR（GUI の起動が必須） |
| G29 | リモートの状態のリアルタイム反映 | 有 | ACP はローカル側で動き、状態は同じ経路を通る |
| G30 | リモートでの履歴 | 一部 | necoder のスレッドは見られる |

### H. アプリ全体・通知・設定・配布

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| H01 | 完了通知（system・音・チップ） | 一部 | 音・トースト・Web Push（承認待ち）。OS 通知は無い |
| H02 | ベル（通知履歴）・未読に戻す | 一部 | 要対応バッジとキュー |
| H03 | Dock バッジ | 無 | |
| H04 | カスタム音・音量 | 一部 | 完了と入力待ちで別の音。同梱の猫 3 声と任意ファイル（JSON で指定） |
| H05 | PR check 失敗・更新の通知 | 一部 | 更新のみ |
| H06 | トレイ | 無 | |
| H07 | 音声入力（オンデバイスの日本語 STT を含む） | 無 | |
| H08 | 設定の検索 | 無 | |
| H09 | キー割り当ての UI | 一部 | keymap.json。⌘K⌘S は既定しか見せない |
| H10 | UI ズーム・フォント・density | 一部 | テーマ・プロジェクト色・エディタの font_size |
| H11 | アプリアイコンの切替 | 無 | |
| H12 | UI 言語（6 言語） | 一部 | ja / en。設定画面に切替は無い |
| H13 | オンボーディング（チェックリスト・ツアー） | 一部 | ようこそ画面・空状態の案内・Fleet の案内トースト |
| H14 | 初回の設定取り込み | 一部 | MCP の自動発見のみ |
| H15 | テレメトリ | 無 | never（思想差） |
| H16 | クラッシュ・ログ・フィードバック | 一部 | panic のログから Issue の下書きを作る。minidump は無い |
| H17 | 更新チャネル・changelog | 一部 | stable のみ。spctl で検証する |
| H18 | 配布の形態 | 一部 | Apple Silicon の dmg と Windows の zip のみ |
| H19 | プラグイン | 無 | Marketplace は never、WASM は later |
| H20 | ペット | 一部 | 猫のマスコット（6 状態・パネル内に固定） |
| H21 | Resource Manager | 一部 | 状態の集計と idle 停止 |
| H22 | Open in 外部アプリ | 一部 | 既定アプリと Finder |
| H23 | star・Discord への導線 | 無 | |
| H24 | GPU の表示 | 無 | 実害は小さい |
| H25 | statusbar の項目の切替 | 無 | |
| H26 | OS ショートカットとの衝突の警告 | 無 | |
| H27 | TCC 権限の案内 | 無 | |
| H28 | Windows / Linux の正式対応 | 一部 | Windows は zip。Linux GUI は未配布 |
| H29 | アプリ内ヘルプ | 一部 | ⌘K⌘S とバグ報告 |
| H30 | ファイル操作の undo | 一部 | ゴミ箱へ移すのみ（§3-6） |

### X. 書き手が追加で確認したもの

| ID | Orca の機能 | 判定 | necoder の現状 |
|---|---|---|---|
| X1 | Ports パネル（worktree ごとに起動中のポートを検出し、開く / 止める。ターミナル出力の URL から worktree を特定） | 無 | ポート検出・転送のコードは無い（SSH の reverse forward は `ne` 用のみ） |
| X2 | PR のレビューコメント（CodeRabbit 含む）を選んで AI に直させ、自動で返信・resolve | 無 | PR ビューそのものが無い |
