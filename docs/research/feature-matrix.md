# 3エディタ横断マトリクス — 中核/オプションの整理と Next Editor の採否

元データ: [`vscode-features.md`](./vscode-features.md) / [`zed-features.md`](./zed-features.md) / [`cursor-features.md`](./cursor-features.md)（いずれも 2026-07-11 調査）。
この文書は3本の「横断で見たときに何が言えるか」と、[`../../FEATURES.md`](../../FEATURES.md) のタグ付け根拠。

---

## 1. 各エディタは「コア/オプション」の線をどう引いているか

同じ問いに3つの全く違う答えがある。ここが一番の収穫。

| | VSCode | Zed | Cursor |
|---|---|---|---|
| 分離の仕組み | **6段の層**: editor core → editor/contrib(59) → workbench core → workbench/contrib(99) → built-in拡張(~90) → marketplace。ESLint が層違反を機械的に禁止 | **全部入りバイナリ**: 237 crate を静的リンク。分離は crate 依存方向（`editor` は vim/collab/agent を知らない）と設定スイッチ（`disable_ai` 等） | VSCode fork で第1層を**タダ取り**。発明は AI 層に全振り。線引きは技術でなく**料金**（機能はほぼ全プラン開放、モデル消費量で課金） |
| コアの純度 | 極端に高い。editor core には **find すら無い**（全部 IEditorContribution 登録式。コアは contrib の存在を知らない） | 中くらい。ただし **CRDT がバッファ標準表現としてコアに焼き込み**（コラボはその上の応用） | VSCode に同じ |
| 拡張にできること | ほぼ何でも（contribution points 53種 + webview + カスタムエディタ + FileSystemProvider）。ただし拡張は**別プロセス**で UI スレッドに触れない | **資産のみ**（言語/テーマ/アイコン/スニペット/デバッグアダプタ/MCP/エージェントサーバ）。capability は process:exec / download_file / npm:install の3種だけ。**UI 拡張は不可** | VSCode 相当（ただし Marketplace は Open VSX） |
| その帰結 | 拡張 API の性能が足りない領域は**本体化**していった: terminal, notebook(244ファイル), chat(802ファイルで最大) 。「git は拡張なのに terminal は本体」が限界線の実例 | notebooks / problem matcher / Settings Sync / テストエクスプローラ等、**VSCode では拡張が担う機能が1個ずつ自前実装**になる | fork 故に upstream 追従が遅延。MS 専有拡張（Pylance 等）は使えない |

**Next Editor への教訓**:
- VSCode の「コアが機能を知らない登録境界」は GPUI でも最初に切る価値がある（150個機能を足しても本体が肥大しなかった主因）。
- Zed の「拡張は資産のみ、機能は本体」は開発速度の勝ち筋だが、UI 拡張という魂を諦める判断でもある（うちはそこが差別化点なので真似できない）。
- 両者の中間 = **「宣言的 UI 拡張」**（webview もフルプロセス分離も持ち込まず、ネイティブ部品の宣言的合成を拡張に許す）が狙うべき空白。VSCode の contribution points 53種のうち views / viewsContainers / statusBarItems / menus / commands / themes あたりは宣言型で安全に提供できることが実証済み — このリストが実装カタログになる。

## 2. 最小核 — 3エディタが揃って「中核/準中核」に置くもの

「エディタとして成立する」ための parity ライン。ここは発明不要、粛々と作る領域。

| レイヤ | 3者共通の中核（＝MVP〜v1 の必須集合） |
|---|---|
| 1 コア編集 | テキストバッファ（VSCode=piece tree / Zed=rope+CRDT。**行配列はどこも使っていない**）、位置/範囲/選択モデル、カーソル編集、undo/redo（トランザクション単位）、IME、クリップボード、自動インデント、multi-cursor（VSCode は中核、Zed は準中核） |
| 2 描画・UI | **行の仮想化描画**（可視行しか描かない）、デコレーション基盤（検索/diff/デバッグ全部の土台）、タブ+ペイン分割、ステータスバー、テーマ、通知、コンテキストメニュー |
| 3 ナビゲーション | fuzzy ファイル検索（Ctrl+P）、コマンドパレット、ファイルツリー、行ジャンプ、ナビ履歴（戻る/進む） |
| 4 言語知能 | シンタックスハイライト（VSCode=TextMate→tree-sitter 実験中 / Zed=tree-sitter）、LSP（補完/診断/ホバー/定義/リネーム/フォーマット）。**言語機能は全部プロバイダ型**（本体は言語実装を持たない）も共通 |
| 5 検索・置換 | バッファ内検索/置換（regex 含む）、プロジェクト横断検索（VSCode=ripgrep 子プロセス / Zed=自前走査） |
| 6 VCS | git status（ツリー/タブの色）、ガター diff（hunk）、ブランチ操作。※本格 UI は分かれる（VSCode は拡張、Zed は本体 git_ui） |
| 7 ターミナル | 統合ターミナル（VSCode=xterm.js+pty host / Zed=alacritty_terminal）、パスリンク検出 |
| 10 設定 | JSON 設定の階層マージ（default→user→project）、JSON keymap + **コンテキスト述語**（when 句 / Zed のコンテキスト式）、JSON スキーマによる補完検証 |

## 3. レイヤ別の要点比較と Next Editor 方針

| レイヤ | VSCode | Zed | Cursor | **Next 方針** |
|---|---|---|---|---|
| 1 コア編集 | piece tree。ファイル横断 undo | rope + CRDT 焼き込み | 継承 | **ropey + トランザクション undo**。CRDT は焼き込まない（コラボ=never とセットの判断） |
| 2 描画 | DOM + **WebGPU パス実験中**（editor/browser/gpu — GPUI 自作の直接の参照実装） | GPUI（Metal/DX/Wayland + wgpu/web 新バックエンド） | 継承 | GPUI。VSCode の viewParts 24種 = 描画機能の仕様書として使う |
| 3 ナビ | Quick Open が中核 | file_finder + picker 共通基盤 | 継承 | v1 で picker 基盤ごと作る（Zed 方式: 1つの fuzzy picker を全モーダルで使い回す） |
| 4 言語 | プロバイダレジストリが中核設計 | tree-sitter 中核 + LSP | 継承 | tree-sitter 先行 → LSP。**最初からプロバイダ型に**（言語実装を editor に書かない） |
| 5 検索 | ripgrep 子プロセス | マルチバッファに結果表示 | 継承 | v1: ripgrep 子プロセス（最安で最強）。multibuffer 化は later |
| 6 VCS | 本体は SCM 枠組みのみ、git は拡張 | 本体に full 実装（graph/stash まで） | 継承 + AI コミットメッセージ | v1: status + gutter まで。それ以上は later |
| 7 term | pty host 分離プロセス | alacritty_terminal | 継承 + Cmd+K | v1: alacritty_terminal（Zed と同じ crate が使える） |
| 8 debug | DAP が準中核 | DAP 一式あるが全部オプション | 継承 | **later**。3者とも「無くてもエディタは成立」扱いが明確 |
| 9 拡張 | 53 contribution points + webview + 別プロセス | 資産のみ WASM、UI 不可 | 継承 | **差別化の本丸**。§4 参照 |
| 10 設定 | 5段階スコープ + Profiles + Sync | 3段階 + JSON スキーマ自動生成 | 継承 | MVP: default→user→project の3段。スキーマ生成は Zed 方式を真似る |
| 11 collab | 本体に無い（Live Share は拡張）! | CRDT 前提のフル実装（アイデンティティ） | 無し | **never**。「本体に無くても一流エディタ」を VSCode と Cursor が証明済み |
| 12 AI | chat が最大 contrib（802ファイル）+ **Agents Window という新層** | 52 crate。ACP で外部エージェント統合 | 全振り。3.0 で Agents Window | §5 参照。**ACP クライアントから入る** |
| 13 他 | notebooks/tasks/remote/trust… | 状態永続化(SQLite)/CLI/自動更新 | Cloud/Automations/Bugbot | 状態永続化と CLI だけ v1。残り later/never |

## 4. 差別化点の検証 — 「色による方向感覚」と「UI 拡張」

調査で確認できた空白地帯:

1. **per-project / per-thread の色識別は3エディタとも持っていない**。
   VSCode は Peacock（サードパーティ拡張）頼み、Zed は構造的に不可（BACKGROUND.md の出発点）、Cursor はエージェント UI を2度刷新（2.0, 3.0）してなお色による識別は入れていない。→ 出発点の不満は 2026-07 現在も未解決のまま。**空白は実在する**。
2. ただし方向感覚の需要自体は市場が証明し始めている: Cursor 3.0 の Agents Window、VSCode の `vs/sessions`（workbench と並列の新トップレベル層！）、Zed の ThreadsSidebar。**「複数エージェント並走時代の方向感覚」は各社が UI 刷新で追いかけている真っ最中**で、色はその最短の解の1つ。急ぐ価値がある。
3. UI 拡張の空白（速い×拡張できる）も変わらず未解決。VSCode は性能が要る機能を本体化し続けることで問題を回避、Zed は拡張 UI を許さないことで回避。

## 5. AI 層の戦略 — 作らずに繋ぐ

3本を横断すると、AI 層は「自前で作る部分」と「プロトコルで繋ぐ部分」に綺麗に割れる:

- **ACP（Agent Client Protocol）が決定打**。Zed は Claude Code / Codex / Gemini CLI / OpenCode / Copilot / **Cursor** までも ACP で外部エージェントとして統合し、UI は同一スレッド画面を共用している。Cursor も JetBrains 対応を ACP 経由で出した。→ **Next Editor は最初から「ACP クライアント」として作る**。自前エージェントループ・ツール実装・モデルプロバイダ 19 種（Zed が抱えている物量）を一切持たずに、Claude Code が動く。そもそもの出発点（Zed で Claude Code を使う UX の不満）に最短で刺さる。
- 自前で作る価値があるのは **UI 契約**の方: スレッド=色付きタブ、トークン常時表示（ACP アダプタの `MAX_THINKING_TOKENS` 問題で学んだ通り、ここはクライアント側 UI の責任範囲）、thinking 常時展開、diff レビュー、チェックポイント表示。
- Cursor から盗む概念: **チェックポイント/巻き戻し**（Git 非依存。信頼の担保としてほぼ必須）、@-mention の型（file/folder/diff/terminal/過去会話）、AGENTS.md 対応（事実上の業界標準、対応コスト低）。
- 後回しでよいと確信できたもの: edit prediction（Cursor は専用モデル+オンライン RL で成立させている＝個人では土俵に乗れない）、Cloud Agents / Bugbot / Automations（第4層＝プラットフォーム事業）。

## 6. FEATURES.md タグ付けの規則

- **MVP** = §2 の最小核のさらに芯（1人で日常使用が始められる線）+ 差別化の種（プロジェクト色）
- **v1** = §2 の残り（準中核 parity）+ ACP クライアント + スレッド色
- **later** = 3者のどこかで「オプション」扱いのもの
- **never（当面）** = リアルタイムコラボ、自前 LLM、クラウド実行基盤、telemetry
- 迷ったら「VSCode で editor/contrib か workbench/contrib か」「Zed で editor crate が依存しているか」を判定基準にする（両者の判断は §2 の通りほぼ一致している）
