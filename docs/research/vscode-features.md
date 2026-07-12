# VSCode 機能全列挙

調査対象は /Users/daichi/Work/vscode（main ブランチの shallow clone、commit 6479c9a、2026-07-11 時点、version 1.129.0 開発版）。ディレクトリ構造と実ファイルを直接読んで作成した一次情報ベースの列挙。安定版に未搭載の実験機能（vs/sessions、browserView、onboarding 実験系など）も含まれる点に注意。

## 概要

VSCode は Electron アプリだが、コードの大半は Electron 非依存で、ブラウザ単体（vscode.dev）でも動く構成になっている。規模は src/vs 配下で TypeScript 7,431 ファイル、約 65.8 万行（テスト込み、実測値）。

src/vs 直下の層構造。下の層は上の層を import できず、ESLint のレイヤ規則で機械的に強制されている。

| 層 | 中身 |
|---|---|
| base | 汎用ユーティリティと UI ウィジェット（仮想 list/tree、scrollbar、sash、grid など）。エディタと無関係に使える |
| platform | DI コンテナ（instantiation）とサービス 104 ディレクトリ。files、configuration、keybinding、contextkey、quickinput、telemetry など |
| editor | テキストエディタ本体（Monaco）。common（DOM 非依存のモデル層）、browser（DOM/GPU 描画）、contrib（後付け機能 59 個）、standalone（Monaco 単体配布用） |
| workbench | IDE シェル。browser/parts（UI シェル）、services（93 ディレクトリ）、api（拡張ホスト実装）、contrib（機能 99 個） |
| code | エントリポイント。Electron main プロセス、CLI、ブラウザ版起動コード |
| server | リモート開発と Web 版のためのヘッドレスサーバ（code-server 相当） |
| sessions | Agents Window。エージェント作業専用の簡易ワークベンチ。2026 年に追加された、workbench と並列の新しいトップレベル層 |

プロセスモデルは、Electron main / renderer（workbench UI）/ extension host（Node、Web Worker、またはリモート）/ pty host（ターミナル）/ shared process / utility process に分かれる。拡張コードは renderer プロセスでは一切動かない。UI がフリーズしない設計の根幹。

デスクトップ固有コードは各所の electron-browser / electron-main サブディレクトリに隔離されていて、browser / common だけで Web 版が成立する。

機能の物量が集中している場所（TS ファイル数、実測）は chat が 802、terminal + terminalContrib が 334、notebook が 244、debug が 101。chat が workbench/contrib 最大の機能領域になっている。

## VSCode のコア/オプション分離線

VSCode には「どこまでが本体か」を分ける線が 6 段階ある。この段階構造そのものが、自作エディタの実装順序の参考になる。

1. **editor/common + editor/browser（エディタコア）**。テキストバッファ、カーソル、undo、トークン保持、diff 計算、描画パイプライン。ここには検索 UI も折りたたみも補完も入っていない。純粋な「テキストを表示して編集する箱」だけ。
2. **editor/contrib（エディタ機能 59 個）**。find、folding、suggest、hover、multicursor など。各機能は IEditorContribution として登録され、コアは contrib の存在を知らない。Monaco Editor はこの層まで込みで配布される。
3. **workbench コア（browser/parts + services）**。ウィンドウ、レイアウト、エディタグループとタブ、クイック入力、設定、キーバインド、ファイルサービス。IDE の骨格。
4. **workbench/contrib（IDE 機能 99 個）**。ターミナル、デバッグ、SCM、検索ビュー、notebook、chat。1 ディレクトリ = 1 機能領域で、contrib 同士は原則直接 import しない。
5. **built-in 拡張（extensions/ 配下、約 90 ディレクトリ）**。git、emmet、typescript-language-features、markdown プレビューなど。本体と同じリポジトリで開発されるが、公開拡張 API 経由でしか本体に触れない。
6. **marketplace 拡張**。リモート開発（SSH / WSL / Dev Containers）、Live Share、Copilot の推論部分、js-debug、Python や C++ の言語拡張。本体リポジトリには存在しない。

自作エディタが学べること。

- 「テキストエディタとして成立する」最小線は 1 の全部と 2 の一部（find、multicursor、clipboard、indentation あたり）、それに 3 のごく一部（ファイルを開く、保存する、コマンドを実行する）。以降の表の「中核」区分はこの線を基準にした。
- git ですら拡張である一方、terminal と notebook と chat は本体 contrib になっている。プロセス分離された拡張 API では描画性能と UI 統合が足りない領域がどこか、という線引きの実例になっている。
- コアが機能を知らない登録式の contrib パターンは、機能を 150 個以上足しても本体が肥大しなかった主因。GPUI で作る場合も、エディタコアと機能群の間にこの種の登録境界を最初に切っておく価値がある。
- Web 対応を「browser / common と electron-* のディレクトリ分離」で最初から強制している点も、プラットフォーム抽象の参考になる。

## 機能一覧（レイヤ別）

区分の意味。**中核** = これが無いとテキストエディタとして成立しない。**準中核** = 現代のコードエディタとして実質必須。**オプション** = あれば強いが無くても成立する。

### 1. コア編集

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| テキストバッファ | 中核 | editor/common/model/pieceTreeTextBuffer | piece tree 実装。行配列ではない。巨大ファイルでも編集が対数時間 |
| 位置/範囲/選択モデル | 中核 | editor/common/core (position, range, selection) | 全 API がこの型の上に立つ |
| カーソルと編集オペレーション | 中核 | editor/common/cursor | タイプ、削除、改行、自動インデント、自動閉じ括弧をここで解決 |
| undo/redo | 中核 | editor/common/model/editStack.ts + platform/undoRedo | ファイル横断 undo（リネームのリファクタ等）に対応 |
| マルチカーソル | 中核 | editor/contrib/multicursor | 上下追加、全一致選択、Ctrl+D 系。矩形選択はカーソル列生成で実現 |
| キーボード基本コマンド | 中核 | editor/browser/coreCommands.ts | 移動、選択、ページ送り等の土台 |
| IME / 入力処理 | 中核 | editor/browser/controller/editContext | EditContext API ベース。マウスは mouseHandler / pointerHandler |
| クリップボード | 中核 | editor/contrib/clipboard | コピー時のシンタックスハイライト保持も |
| 単語単位操作 | 中核 | editor/contrib/wordOperations | 単語移動/削除。単語境界は言語ごとの wordPattern 依存 |
| インデント処理 | 中核 | editor/contrib/indentation + common/model/indentationGuesser.ts | tab/space 変換、再インデント、既存ファイルからの自動検出 |
| エンコーディング / EOL | 中核 | workbench/services/textfile | 自動推定、CRLF/LF 変換 |
| サブワード操作 (camelCase) | 準中核 | editor/contrib/wordPartOperations | |
| 行操作 | 準中核 | editor/contrib/linesOperations, caretOperations | 行移動/複製/削除/ソート/join/ケース変換/文字転置 |
| 括弧マッチング | 準中核 | editor/contrib/bracketMatching + model/bracketPairsTextModelPart | ネイティブ括弧ペア色付けも同じモデル部品 |
| コメントトグル | 準中核 | editor/contrib/comment | 言語設定の lineComment/blockComment を参照 |
| スニペットエンジン | 準中核 | editor/contrib/snippet | tabstop、placeholder、変数、ネスト対応 |
| 折りたたみ | 準中核 | editor/contrib/folding | インデントベース + 言語プロバイダの 2 段構え |
| 選択の構文的拡張/縮小 | 準中核 | editor/contrib/smartSelect | |
| 保存時処理 | 準中核 | workbench/contrib/codeEditor (saveParticipants) | 末尾空白除去、最終改行、format on save、code action on save |
| 自動保存 / hot exit | 準中核 | workbench/browser/parts/editor/editorAutoSave.ts + services/workingCopy | 未保存内容のバックアップとクラッシュ復元 |
| 巨大ファイル保護 | 準中核 | codeEditor/largeFileOptimizations + editor/contrib/longLinesHelper + workbench/contrib/limitIndicator | 長大行での tokenization 打ち切り等 |
| スニペット管理 | オプション | workbench/contrib/snippets | ユーザスニペット、tab 補完、snippet picker |
| ペースト/ドロップ変換 | オプション | editor/contrib/dropOrPasteInto + workbench/contrib/dropOrPasteInto | paste as JSON 等、プロバイダ型 |
| 選択テキストの D&D | オプション | editor/contrib/dnd | |
| アンカー選択 | オプション | editor/contrib/anchorSelect | |
| カーソル位置 undo | オプション | editor/contrib/cursorUndo | 移動だけを戻す soft undo |
| 行選択コマンド | オプション | editor/contrib/lineSelection | |
| 値の巡回置換 | オプション | editor/contrib/inPlaceReplace | true→false 等の入れ替え |
| 最終行改行挿入 | オプション | editor/contrib/insertFinalNewLine | |
| 挿入/上書きモード切替 | オプション | codeEditor (toggleOvertype) + editor/common/inputMode.ts | |
| 異常行終端の検出 | オプション | editor/contrib/unusualLineTerminators | LS/PS 混入の警告と除去 |
| 読み取り専用時メッセージ | オプション | editor/contrib/readOnlyMessage | |

### 2. 描画・UI

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| 行レンダリング（仮想化） | 中核 | editor/browser/view + viewParts/viewLines | 可視行だけ DOM 化。viewParts は全 24 個 |
| 折返しと座標変換 | 中核 | editor/common/viewModel + viewLayout | 論理行と表示行の変換層（modelLineProjection） |
| デコレーション基盤 | 中核 | editor/common/model（interval tree）+ viewParts/decorations | 検索、diff、デバッグ等すべての装飾の土台 |
| カーソル/選択の描画 | 中核 | viewParts/viewCursors, selections | 点滅、スムーズキャレット、複数カーソル |
| 行番号とマージン | 中核 | viewParts/lineNumbers, glyphMargin, margin | |
| スクロールバー + overview ruler | 中核 | viewParts/editorScrollbar, overviewRuler | ruler にエラー/検索位置/diff を集約表示 |
| ウィジェット挿し込み基盤 | 中核 | viewParts/viewZones, contentWidgets, overlayWidgets | 行間ゾーンと浮動ウィジェット。peek や hover の土台 |
| フォント計測 | 中核 | editor/browser/config | 文字幅実測でレイアウトする |
| クイック入力ウィジェット | 中核 | platform/quickinput | コマンドパレット等すべての入力 UI の土台 |
| 仮想 list/tree ウィジェット | 中核 | base/browser/ui + platform/list | explorer から補完リストまで全部これ |
| ダイアログ | 中核 | workbench/browser/parts/dialogs + platform/dialogs | |
| コンテキストメニュー | 中核 | editor/contrib/contextmenu + platform/contextview | |
| WebGPU レンダリング | オプション | editor/browser/gpu + viewParts/viewLinesGpu, rulersGpu + editor/contrib/gpu | DOM と並存する実験的 GPU パス。GPUI で作るなら最重要の参照実装 |
| ミニマップ | 準中核 | viewParts/minimap | canvas 描画 |
| 現在行ハイライト | 準中核 | viewParts/currentLineHighlight | |
| 空白/制御文字の可視化 | 準中核 | viewParts/whitespace + codeEditor (toggleRenderWhitespace) | |
| インデントガイド | 準中核 | viewParts/indentGuides + common/model/guidesTextModelPart | ブラケットペアガイド込み |
| テーマシステム | 準中核 | platform/theme + workbench/contrib/themes + services/themes | カラーテーマ、ファイルアイコン、product icon の 3 種 |
| ワークベンチレイアウト | 準中核 | workbench/browser/layout + parts/* | grid ベース。パーツの D&D 移動、パネル位置切替 |
| タイトルバー | 準中核 | parts/titlebar + platform/menubar | カスタムタイトルバー、コマンドセンター、メニューバー |
| アクティビティバー / サイドバー / 補助バー / パネル / ステータスバー | 準中核 | parts/activitybar, sidebar, auxiliarybar, panel, statusbar | 補助サイドバーは chat の主戦場 |
| エディタグループとタブ | 中核（グループ）/ 準中核（タブ） | parts/editor | 分割 grid、複数行タブ、ピン留め、プレビュータブ、modalEditorPart（sessions 用） |
| 通知 | 準中核 | parts/notifications | トースト + 通知センター |
| ホバーサービス | 準中核 | platform/hover | UI 全域の tooltip 統一 |
| 進捗表示 | 準中核 | services/progress + editor/contrib/inlineProgress | |
| 出力パネル | 準中核 | workbench/contrib/output + services/output | 拡張のログ出力先 |
| インライン一時メッセージ | 準中核 | editor/contrib/message | |
| Unicode ハイライト | 準中核 | editor/contrib/unicodeHighlighter | 紛らわしい文字のなりすまし対策 |
| 補助ウィンドウ | オプション | services/auxiliaryWindow + parts/editor/auxiliaryEditorPart.ts | エディタやビューを別 OS ウィンドウへ |
| フォントズーム | オプション | editor/contrib/fontZoom | |
| ルーラー（桁線） | オプション | viewParts/rulers | |
| Zen モード / センタリング | オプション | workbench/browser/layout | |
| スプラッシュ | オプション | workbench/contrib/splash | 前回のレイアウト概形を先に描く起動演出 |
| バナー | オプション | parts/banner + workbench/contrib/welcomeBanner | |
| スクロール同期 | オプション | workbench/contrib/scrollLocking | |
| 中ボタンオートスクロール | オプション | editor/contrib/middleScroll | |
| 空エディタのプレースホルダ | オプション | editor/contrib/placeholderText + codeEditor/emptyTextEditorHint | |
| フローティングメニュー | オプション | editor/contrib/floatingMenu | |
| セクション見出し (MARK:) | オプション | editor/contrib/sectionHeaders | |
| シンボルアイコン配色 | オプション | editor/contrib/symbolIcons | |
| サッシュ設定 | オプション | workbench/contrib/sash | |
| リスト列幅調整 | オプション | workbench/contrib/list | |
| スタイル実験上書き | オプション | workbench/contrib/styleOverrides | フォントランプ等の実験 CSS |

### 3. ナビゲーション

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| Quick Open（ファイル名 fuzzy） | 中核 | workbench/contrib/search (anythingQuickAccess) + platform/quickinput | Ctrl+P。エディタ史的にも最重要 UI のひとつ |
| コマンドパレット | 中核 | workbench/contrib/quickaccess | 全コマンドがここから叩ける。キーバインド表示付き |
| 行移動 (Ctrl+G) | 中核 | editor/contrib/quickAccess | |
| ファイルツリー（エクスプローラ） | 中核 | workbench/contrib/files | 作成/リネーム/削除/D&D/ファイル操作 undo、Open Editors ビュー |
| シンボル移動（@ / #） | 準中核 | editor/contrib/quickAccess + gotoSymbol + workbench/services/search | ファイル内とワークスペース全体 |
| 定義/型定義/実装/参照ジャンプ + peek | 準中核 | editor/contrib/gotoSymbol + peekView | プロバイダは言語側（レイヤ 4） |
| エラー間移動 (F8) | 準中核 | editor/contrib/gotoError | |
| ナビゲーション履歴 | 準中核 | workbench/services/history | 戻る/進む、直前の編集位置へ |
| アウトラインビュー | 準中核 | workbench/contrib/outline + services/outline | |
| ブレッドクラム | 準中核 | parts/editor/breadcrumbs* | パス + シンボル階層。ピッカーで横移動できる |
| リンク検出 | 準中核 | editor/contrib/links | URL/ファイルパスの Ctrl+クリック |
| 最近開いた項目 | 準中核 | workbench/contrib/workspaces + platform/workspaces | |
| sticky scroll | オプション | editor/contrib/stickyScroll | 現在スコープの見出しを上部固定。ツリーとターミナルにも波及 |
| 参照ツリービュー | オプション | extensions/references-view | 参照/実装/呼び出し階層をサイドバーに常駐表示 |

### 4. 言語知能

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| 言語機能レジストリ | 中核（この設計自体が） | editor/common/languageFeatureRegistry + languageFeaturesService | 全機能がプロバイダ型。エディタ本体は言語実装を 1 つも持たない |
| 言語設定 | 中核 | editor/common/languages + languageConfigurationRegistry | 括弧、コメント記号、インデントルール、wordPattern、onEnter ルール |
| 構文ハイライト (TextMate) | 準中核 | workbench/services/textMate | WASM 版 oniguruma。ワーカースレッドでの非同期トークン化あり |
| Tree-sitter トークン化 | オプション | services/treeSitter + editor/common/services/treeSitter | 実験中。TextMate と並存 |
| セマンティックトークン | オプション | editor/contrib/semanticTokens + common/services/semanticTokensStyling | LSP のセマンティックハイライト |
| 補完 (IntelliSense) | 準中核 | editor/contrib/suggest | 単語ベースのフォールバックは editorWorker が提供 |
| パラメータヒント | 準中核 | editor/contrib/parameterHints | |
| ホバー | 準中核 | editor/contrib/hover | 診断 + 言語ホバーの合成表示 |
| 診断と問題パネル | 準中核 | platform/markers + workbench/contrib/markers | フィルタ、テーブル表示、ファイル装飾 |
| コードアクション / クイックフィックス | 準中核 | editor/contrib/codeAction + workbench/contrib/codeActions | 電球 UI、on-save 実行、種別 (refactor.extract 等) の体系 |
| リネーム | 準中核 | editor/contrib/rename + workbench/contrib/bulkEdit | 複数ファイルに跨る場合はプレビュー付き一括編集 |
| フォーマット | 準中核 | editor/contrib/format + workbench/contrib/format | document/range/on-type。複数フォーマッタの選択 UI、変更行のみのフォーマット |
| ドキュメントシンボル | 準中核 | editor/contrib/documentSymbols | アウトライン/ブレッドクラム/シンボル検索へ供給 |
| 出現箇所ハイライト | 準中核 | editor/contrib/wordHighlighter | プロバイダ無し言語ではテキスト一致に退化 |
| 一括編集とリファクタプレビュー | 準中核 | workbench/contrib/bulkEdit | WorkspaceEdit の適用エンジン。適用前に diff で確認できる |
| Web Worker オフロード | 準中核 | editor/common/services/editorWebWorker | diff、リンク検出、単語補完を UI スレッド外で |
| CodeLens | オプション | editor/contrib/codelens | |
| Inlay hints | オプション | editor/contrib/inlayHints + workbench/contrib/inlayHints | |
| カラーデコレータ + ピッカー | オプション | editor/contrib/colorPicker | |
| 連動編集 | オプション | editor/contrib/linkedEditing | HTML 開閉タグの同時リネーム等 |
| 呼び出し階層 / 型階層 | オプション | workbench/contrib/callHierarchy, typeHierarchy | peek UI |
| Emmet | オプション | workbench/contrib/emmet + extensions/emmet | 略記法展開。実装は拡張側 |
| 言語自動判定 | オプション | workbench/contrib/languageDetection + services/languageDetection | ML モデル（guesslang 系）で無題ファイルの言語を推定 |
| 言語ステータス | オプション | workbench/contrib/languageStatus | ステータスバーの言語別ヘルスレポート |
| 組み込み言語サポート | オプション | extensions/*（文法のみ約 47 言語 + language-features 6 種 + TypeScript） | TS は tsserver との独自プロトコル統合で最大規模。css/html/json/markdown は LSP サーバ同梱 |

### 5. 検索・置換

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| エディタ内検索/置換 | 中核 | editor/contrib/find | 正規表現、大小文字、単語一致、選択範囲内、キャプチャグループ置換、複数行 |
| モデル内検索 API | 中核 | editor/common/model/textModelSearch.ts | バッファ上の検索プリミティブ |
| ワークスペース横断検索 | 準中核 | workbench/contrib/search + services/search | ripgrep を子プロセスで実行。Web 版はローカル実装に切替 |
| 横断置換（プレビュー付き） | 準中核 | workbench/contrib/search | 置換前に diff で確認 |
| 検索エディタ | オプション | workbench/contrib/searchEditor + extensions/search-result | 検索結果をテキスト文書として保存/再実行。専用文法拡張あり |
| クイック検索 | オプション | workbench/contrib/search (quickTextSearch) | Quick Open から本文検索 |

### 6. VCS / Git

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| diff 計算 | 準中核 | editor/common/diff | legacy（Myers）と defaultLinesDiffComputer の 2 実装。移動検出と単語内差分も |
| diff エディタ | 準中核 | editor/browser/widget/diffEditor | side-by-side / inline、moved code blocks 表示 |
| SCM フレームワーク | 準中核 | workbench/contrib/scm | プロバイダ型の汎用ソース管理ビュー。git 非依存。コミット入力欄、リソースグループ、SCM 履歴グラフ |
| quick diff（ガター差分） | 準中核 | workbench/contrib/scm (quickDiff) | 未コミット変更を行番号脇に表示、インライン展開 |
| Git 本体 | 準中核 | extensions/git + workbench/contrib/git + platform/git | 機能実装は拡張（stage/commit/branch/stash/rebase/worktree/submodule）。近年 workbench/contrib/git と platform/git に本体側の Git 読み取りサービスが追加された |
| マージエディタ (3-way) | オプション | workbench/contrib/mergeEditor | |
| インラインコンフリクト解決 | オプション | extensions/merge-conflict | conflict マーカー上に CodeLens |
| マルチファイル diff | オプション | workbench/contrib/multiDiffEditor + editor/browser/widget/multiDiffEditor | 複数ファイルの diff を 1 画面に。AI 編集レビューでも使用 |
| GitHub 連携 | オプション | extensions/github + github-authentication | publish、認証。PR/issue の本格 UI は marketplace 拡張 |
| リモートソース基盤 | オプション | extensions/git-base | clone 元プロバイダの抽象 |
| diff エディタのパンくず | オプション | editor/contrib/diffEditorBreadcrumbs | |

### 7. ターミナル

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| 統合ターミナル | 準中核 | workbench/contrib/terminal + platform/terminal | xterm.js。pty は専用プロセス（pty host）。タブ、分割、プロファイル管理 |
| シェル統合 | 準中核 | terminal + terminalContrib | シェルにスクリプト注入してコマンド境界/終了コードを検出。成功失敗の装飾、コマンド単位ナビゲーション |
| リンク検出 | 準中核 | terminalContrib/links | ファイルパスをエディタで開く、行番号付きも解決 |
| ターミナル内検索 | 準中核 | terminalContrib/find | |
| アクセシビリティバッファ | 準中核 | terminalContrib/accessibility | スクリーンリーダ向けの読み上げ用ビュー |
| 補完（シェル IntelliSense） | オプション | terminalContrib/suggest + extensions/terminal-suggest | |
| クイックフィックス | オプション | terminalContrib/quickFix + 拡張点 terminalQuickFixes | 失敗コマンドへの修正提案 |
| コマンド履歴 | オプション | terminalContrib/history | 最近のコマンド/ディレクトリの再実行 |
| sticky scroll | オプション | terminalContrib/stickyScroll | 実行中コマンドを上部固定 |
| type ahead | オプション | terminalContrib/typeAhead | SSH 越しの入力を予測エコー |
| 自動応答 | オプション | terminalContrib/autoReplies | 特定プロンプトへ自動返答 |
| シーケンス/シグナル送信 | オプション | terminalContrib/sendSequence, sendSignal | |
| ターミナル内 chat / エージェント実行 | オプション | terminalContrib/chat, chatAgentTools | エージェントがコマンドを実行する際の承認 UI を含む |
| 音声入力 / ズーム / 画面サイズ表示 | オプション | terminalContrib/voice, zoom, resizeDimensionsOverlay | |
| 環境変数変更の検出 | オプション | terminalContrib/environmentChanges | 拡張による環境変数注入の通知 |
| 外部ターミナルで開く | オプション | workbench/contrib/externalTerminal | |

terminalContrib は計 25 サブ機能。ターミナルだけでミニ contrib 構造を持つ。

### 8. デバッグ

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| DAP クライアント | 準中核 | workbench/contrib/debug | Debug Adapter Protocol。どの言語のデバッガも同一 UI |
| launch.json と変数解決 | 準中核 | debug + services/configurationResolver | ${workspaceFolder} 等の変数展開、入力プロンプト |
| ブレークポイント | 準中核 | debug | 条件付き、ヒットカウント、ログポイント、関数、データ、インライン |
| 実行制御 | 準中核 | debug | step in/out/over、restart frame、複数セッション同時 |
| 変数 / ウォッチ / コールスタック | 準中核 | debug | |
| デバッグコンソール (REPL) | 準中核 | debug/browser/repl* | 式評価、補完付き |
| デバッグホバー / インライン値 | 準中核 | debug | 実行中は言語ホバーよりデバッグ値を優先 |
| デバッグツールバー / ステータス | 準中核 | debug | |
| 逆アセンブリビュー | オプション | debug (disassemblyView) | |
| compound 起動 | オプション | debug | 複数構成の一括起動 |
| デバッグ可視化拡張点 | オプション | 拡張点 debugVisualizers | |
| Node 自動アタッチ | オプション | extensions/debug-auto-launch | |
| サーバ起動検出でブラウザ起動 | オプション | extensions/debug-server-ready | |
| JS デバッガ本体 | オプション | 本リポジトリ外 (js-debug、ビルド時同梱) | デバッガ実装自体はすべて拡張である証左 |

### 9. 拡張モデル

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| 拡張ホストのプロセス分離 | 準中核 | workbench/services/extensions + workbench/api | local Node / Web Worker / remote の 3 形態。拡張は UI スレッドに触れない |
| vscode.d.ts API 実装 | 準中核 | workbench/api/common/extHost*（約 80 モジュール） | extHost.api.impl.ts だけで 2,315 行。文書/エディタ/言語/デバッグ/SCM/認証/LM まで |
| contribution points | 準中核 | 各 contrib の registerExtensionPoint | package.json 宣言で UI へ静的登録。grep 実測で 53 個（下記） |
| activation events | 準中核 | services/extensions | 遅延起動。起動時間を守る要 |
| 拡張管理 | 準中核 | workbench/contrib/extensions + platform/extensionManagement | インストール、更新、無効化、依存解決、署名検証、VSIX |
| Marketplace 接続 | 準中核 | platform/extensionManagement (gallery) | 検索、評価、おすすめ (extensionRecommendations) |
| webview | 準中核 | workbench/contrib/webview, webviewPanel, webviewView | サンドボックス化 iframe。パネルにもサイドバービューにも置ける |
| カスタムエディタ | オプション | workbench/contrib/customEditor + 拡張点 customEditors | 任意ファイル形式のエディタ差し替え（webview ベース） |
| ツリービュー / ビューコンテナ拡張 | 準中核 | 拡張点 views / viewsContainers | |
| 仮想文書 / FileSystemProvider | 準中核 | services/textmodelResolver + platform/files | 拡張が仮想ファイルシステムを丸ごと提供できる |
| 認証プロバイダ | 準中核 | services/authentication + workbench/contrib/authentication + extensions/github-authentication, microsoft-authentication | |
| LSP / DAP | 準中核 | プロトコル実装は拡張側ライブラリ | 本体はプロバイダ API のみを定義。LSP を本体に持たない設計 |
| 拡張の性能診断 | オプション | workbench/contrib/extensions（bisect、実行プロファイル） | 拡張半減法で問題拡張を特定 |
| proposed API 機構 | オプション | workbench/services/extensions | 安定化前 API を Insiders 限定で試す仕組み |

grep 実測の contribution points 53 個（拡張が package.json で宣言できる面）。authentication, breakpoints, chatContext, chatOutputRenderers, chatParticipants, chatPlugins, chatSessions, chatViewsWelcome, colors, commands, configuration, configurationDefaults, continueEditSession, css, customEditors, debugVisualizers, debuggers, grammars, iconThemes, icons, jsonValidation, keybindings, languageModelChatProviders, languageModelToolSets, languageModelTools, languages, localizations, mcpServerDefinitionProviders, menus, notebookPreload, notebookRenderer, notebooks, problemMatchers, problemPatterns, productIconThemes, remoteCodingAgents, remoteHelp, resourceLabelFormatters, semanticTokenModifiers, semanticTokenScopes, semanticTokenTypes, snippets, speechProviders, statusBarItems, submenus, taskDefinitions, terminal, terminalQuickFixes, themes, views, viewsContainers, viewsWelcome, walkthroughs。これに加えて built-in 拡張が独自に定義するものが若干ある。

### 10. 設定・キーマップ

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| 設定レジストリ + JSON スキーマ | 中核 | platform/configuration + platform/jsonschemas | 全設定が型とデフォルト値と説明を持ち、settings.json 編集時に補完/検証される |
| 設定スコープ階層 | 準中核 | workbench/services/configuration | default < user < remote < workspace < folder。言語別上書き、企業ポリシーによる強制も |
| 設定エディタ GUI | 準中核 | workbench/contrib/preferences | 検索、変更済みフィルタ、GUI と JSON の双方向 |
| キーバインドシステム | 中核 | platform/keybinding + contextkey + services/keybinding | keybindings.json、when 句のコンテキストキー、キーボードレイアウト検出、chord 対応 |
| キーバインドエディタ | 準中核 | workbench/contrib/preferences (keybindings editor) | 競合検出、録音入力 |
| プロファイル | オプション | workbench/contrib/userDataProfile + services/userDataProfile | 設定/拡張/UI 状態/スニペットのセット切替とエクスポート |
| Settings Sync | オプション | workbench/contrib/userDataSync + platform/userDataSync | アカウント同期。マージと競合解決を持つ |
| 複数コマンド実行 | オプション | workbench/contrib/commands | runCommands で 1 キーに複数コマンド |
| キーバインドのエクスポート | オプション | workbench/contrib/keybindingsExport | |
| システム全域ショートカット | オプション | workbench/contrib/keybindings (systemWideKeybindings) | OS グローバルなキー登録（実験） |
| 企業ポリシー | オプション | platform/policies + workbench/contrib/policyExport | グループポリシー/MDM で設定を固定 |

### 11. コラボレーション

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| Live Share | オプション | 本体に実装なし（marketplace 拡張） | リアルタイム共同編集は本体機能ではない、という事実自体が重要 |
| コメントスレッド基盤 | オプション | workbench/contrib/comments | 行コメント UI の汎用フレームワーク。GitHub PR 拡張などが利用 |
| 共有リンク | オプション | workbench/contrib/share | vscode.dev リンクや permalink をプロバイダ型で生成 |
| Cloud Changes (Edit Sessions) | オプション | workbench/contrib/editSessions | 未コミットの作業状態を別デバイスへ持ち運ぶ |
| Remote Tunnels | オプション | workbench/contrib/remoteTunnel + extensions/tunnel-forwarding + platform/tunnel | 自分のマシンへどこからでも接続 |
| ポート転送ビュー | オプション | workbench/contrib/remote (ports) | 転送ポートの公開/共有 |

### 12. AI / エージェント

2026 年 main では chat 関連が workbench/contrib 最大の機能領域（802 TS ファイル）。UI とセッション管理は本体、モデル推論は copilot 拡張という分業。

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| Chat ビュー | オプション | workbench/contrib/chat | @参加者、/コマンド、添付(ファイル/画像/コンテキスト)、モデル切替、モード (ask/edit/agent) |
| Inline chat | オプション | workbench/contrib/inlineChat | エディタ内 Ctrl+I。差分プレビューして採否 |
| Chat editing | オプション | chat/browser/chatEditing | 複数ファイルへの AI 編集を multiDiffEditor でレビューし承認/巻き戻し |
| Agent mode | オプション | chat（tools、chatAgentTools ほか） | ツール呼び出しループ、承認フロー、ターミナル実行統合 |
| MCP クライアント | オプション | workbench/contrib/mcp + platform/mcp | サーバ起動/検出、ツール、sampling、リソース FS、ゲートウェイ、サンドボックス。拡張点 mcpServerDefinitionProviders |
| Language Model API | オプション | chat/common/languageModels | 拡張がモデルを提供/消費する API。BYOK、クォータ管理 |
| インライン補完 (ghost text / NES) | オプション | editor/contrib/inlineCompletions + workbench/contrib/inlineCompletions | エンジンは editor 層。供給者は拡張（Copilot 等）。編集予測 (next edit) も同系 |
| Copilot 同梱拡張 | オプション | extensions/copilot | copilot-chat が built-in 化している（頭脳はサービス側） |
| エージェントセッション管理 | オプション | chat (agentSessions, chatSessions) + welcomeAgentSessions | ローカル CLI エージェントやクラウドエージェントのセッション一覧/再開 |
| Agents Window | オプション | src/vs/sessions（トップレベル層） | エージェント専用の簡易ワークベンチ。固定レイアウト、モーダルエディタ、モバイルレイアウト仕様まで持つ。workbench から独立した新層 |
| リモートコーディングエージェント | オプション | workbench/contrib/remoteCodingAgents + 拡張点 remoteCodingAgents | GitHub 側などで非同期に走るエージェントへ委任 |
| 音声対話 | オプション | workbench/contrib/speech + agentsVoice + terminalContrib/voice | 拡張点 speechProviders。dictation、voice chat、エージェント音声 UI |
| プロンプトファイル / instructions | オプション | chat/common/promptSyntax + extensions/prompt-basics | .prompt.md 等の文法とカスタム指示の読み込み |
| AI 編集の計測 | オプション | workbench/contrib/editTelemetry | AI 由来編集率の計測 |
| チャット画像カルーセル | オプション | workbench/contrib/imageCarousel | chat が生成/添付した画像群の閲覧エディタ |

### 13. その他

| 機能 | 区分 | 実装 (ディレクトリ等) | 備考 |
|---|---|---|---|
| Notebooks | オプション | workbench/contrib/notebook + extensions/ipynb, notebook-renderers | 244 ファイルの大型 contrib。カーネル、出力レンダラ、セル diff、アウトライン、検索まで本体に深く統合。拡張 API では性能が出ず本体化した代表例 |
| Interactive Window / REPL | オプション | workbench/contrib/interactive, replNotebook | notebook 基盤の派生 |
| タスクシステム | 準中核 | workbench/contrib/tasks + extensions/npm, gulp, grunt, jake | tasks.json、problem matcher でビルド出力を診断化、タスク自動検出 |
| リモート開発 | オプション | workbench/contrib/remote + services/remote + src/vs/server | 拡張ホストごとリモートで動かす思想。SSH/WSL/Containers の各拡張は marketplace（クローズドソース） |
| Web 版 | オプション | src/vs/code/browser + server | デスクトップ機能は electron-* に隔離されているため成立する |
| テスト | オプション | workbench/contrib/testing | Test Explorer、インライン結果、カバレッジ表示、continuous run |
| アクセシビリティ | 準中核 | workbench/contrib/accessibility, accessibilitySignals + editor/contrib/toggleTabFocusMode | アクセシブルビュー、ヘルプダイアログ、操作音/アナウンス、スクリーンリーダ最適化 |
| i18n | オプション | workbench/contrib/localization + platform/languagePacks | 表示言語パック |
| テレメトリ | オプション | workbench/contrib/telemetry, tags, bracketPairColorizer2Telemetry + platform/telemetry | 製品運用要件。エディタ機能ではない |
| 自動更新 | オプション | workbench/contrib/update, relauncher | リリースノート表示込み |
| Workspace Trust | 準中核 | workbench/contrib/workspace | 未信頼フォルダでタスク/デバッグ/一部拡張を無効化する制限モード。任意コード実行対策 |
| マルチルートワークスペース | オプション | workbench/contrib/workspaces + configuration | .code-workspace |
| タイムライン + ローカル履歴 | オプション | workbench/contrib/timeline, localHistory | ファイル単位の履歴ビュー。git 履歴もプロバイダとして合流 |
| ファイル監視 | 準中核 | platform/files (watcher) | ネイティブ watcher の分離実行 |
| ログ管理 | 準中核 | workbench/contrib/logs + services/log | ログレベル操作、出力チャネル連携 |
| 問題レポータ / プロセスエクスプローラ / 性能計測 | オプション | workbench/contrib/issue, processExplorer, performance + services/timer | |
| Welcome / オンボーディング | オプション | welcomeGettingStarted, welcomeWalkthrough, welcomeViews, welcomeBanner, welcomeOnboarding, onboarding | walkthrough は拡張も提供可能（拡張点 walkthroughs） |
| URL プロトコル処理 | 準中核 | workbench/contrib/url + platform/url | vscode:// の受け口、拡張の URI ハンドラ、信頼ドメイン確認 |
| 外部 URI オープナ | オプション | workbench/contrib/externalUriOpener, opener | |
| 組み込みブラウザ | オプション | workbench/contrib/browserView + extensions/simple-browser | browserView はネイティブ WebContents ベースの新実装（実験）。エージェントのブラウザ操作用途も視野 |
| メディアプレビュー | オプション | extensions/media-preview | 画像/音声/動画の閲覧 |
| Markdown プレビュー | オプション | extensions/markdown-language-features + markdown-math + mermaid-markdown-features | 本体側 workbench/contrib/markdown は設定 UI やリリースノート描画用（KaTeX 対応） |
| シークレット保管 | 準中核 | platform/encryption + secrets + workbench/contrib/encryption | OS キーチェーン連携 |
| ネットワーク | オプション | platform/request + workbench/contrib/meteredConnection | プロキシ対応、従量課金接続の検出 |
| 緊急アラート / アンケート | オプション | workbench/contrib/emergencyAlert, surveys | 配信型の告知と NPS |
| 拡張開発支援 | オプション | extensions/extension-editing, configuration-editing | package.json や settings.json 系ファイルの補完/検証 |

## ディレクトリ全対応表

### workbench/contrib 全 99 ディレクトリ

レイヤ番号は上の機能一覧の節番号。

| ディレクトリ | レイヤ | 説明 |
|---|---|---|
| accessibility | 13 | アクセシブルビューと a11y ヘルプダイアログ |
| accessibilitySignals | 13 | 操作音とスクリーンリーダ向けアナウンス |
| agentsVoice | 12 | エージェントとの音声対話 UI（トランスクリプトビュー付き） |
| authentication | 9 | 認証プロバイダ管理。拡張へのサインイン提供 |
| bracketPairColorizer2Telemetry | 13 | 旧括弧色付け拡張の利用検出テレメトリ |
| browserView | 13 | 組み込みブラウザエディタ（ネイティブ WebContents、実験） |
| bulkEdit | 4 | ワークスペース一括編集の適用とリファクタプレビュー |
| callHierarchy | 4 | 呼び出し階層 peek |
| chat | 12 | チャット/エージェント UI 一式。802 TS ファイルで contrib 最大 |
| codeActions | 4 | code action の contribution 定義と設定 |
| codeEditor | 1 | エディタと workbench の接着。保存時処理、表示トグル、大ファイル対策、dictation |
| commands | 10 | runCommands（複数コマンドの一括実行） |
| comments | 11 | コメントスレッド UI 基盤（PR レビュー等が利用） |
| customEditor | 9 | webview ベースのカスタムエディタ |
| debug | 8 | デバッガ UI 一式（DAP クライアント） |
| dropOrPasteInto | 1 | paste as / drop as のプロバイダ設定 |
| editSessions | 11 | Cloud Changes。作業状態のデバイス間持ち運び |
| editTelemetry | 13 | 編集の由来（AI か手入力か）の計測 |
| emergencyAlert | 13 | 緊急告知バナー |
| emmet | 4 | Emmet コマンドの本体側接着（実装は拡張） |
| encryption | 13 | シークレット暗号化の接着 |
| extensions | 9 | 拡張管理 UI。検索、インストール、おすすめ、bisect |
| externalTerminal | 7 | OS のターミナルで開く |
| externalUriOpener | 13 | URI を外部アプリで開く仕組み |
| files | 3 | エクスプローラ、Open Editors、ファイル操作全般 |
| folding | 1 | 折りたたみの workbench 側設定 |
| format | 4 | フォーマッタ選択、変更行のみフォーマット |
| git | 6 | 本体側 Git サービスの窓口（refs/diff の読み取り。実装主体は git 拡張） |
| imageCarousel | 12 | チャット画像のカルーセル閲覧エディタ |
| inlayHints | 4 | inlay hint のアクセシビリティ支援 |
| inlineChat | 12 | エディタ内チャット |
| inlineCompletions | 12 | インライン補完の workbench 統合（設定/ステータス） |
| interactive | 13 | Interactive Window（notebook ベースの REPL） |
| issue | 13 | issue レポータ |
| keybindings | 10 | キーバインド contribution とシステム全域ショートカット |
| keybindingsExport | 10 | キーバインドのエクスポート |
| languageDetection | 4 | 内容からの言語自動判定（ML） |
| languageStatus | 4 | 言語ステータス（ステータスバー項目） |
| limitIndicator | 2 | 機能上限到達の表示（折りたたみ数上限等） |
| list | 2 | リスト/テーブル操作の追加コマンド（列幅調整） |
| localHistory | 13 | ローカル履歴（タイムライン統合） |
| localization | 13 | 表示言語パック管理 |
| logs | 13 | ログレベル操作とログビューア |
| markdown | 13 | workbench 内 Markdown 描画（設定 UI、リリースノート、KaTeX） |
| markers | 4 | 問題パネル |
| mcp | 12 | MCP クライアント。サーバ管理、ツール、sampling、ゲートウェイ、サンドボックス |
| mergeEditor | 6 | 3-way マージエディタ |
| meteredConnection | 13 | 従量課金接続の検出と表示 |
| multiDiffEditor | 6 | 複数ファイル diff の単一ビュー |
| notebook | 13 | ノートブックエディタ本体（カーネル、レンダラ、diff） |
| onboarding | 13 | 新規ユーザ向けスポットライト UI（実験） |
| opener | 13 | リンクを開く際の確認と委譲 |
| outline | 3 | アウトラインビュー |
| output | 2 | 出力パネル |
| performance | 13 | 起動性能の計測/レポート |
| policyExport | 13 | 企業ポリシー定義の書き出し |
| preferences | 10 | 設定エディタ GUI とキーバインドエディタ |
| processExplorer | 13 | プロセスエクスプローラ |
| quickaccess | 3 | コマンドパレットとビュー切替 Quick Access |
| relauncher | 13 | 設定変更時の再起動/再読み込み促し |
| remote | 13 | リモート開発 UI（Remote Explorer、接続、ポート） |
| remoteCodingAgents | 12 | 非同期リモートコーディングエージェント連携 |
| remoteTunnel | 11 | Remote Tunnels（自マシンの公開） |
| replNotebook | 13 | REPL エディタ（notebook 派生） |
| sash | 2 | サッシュ（境界ドラッグ）の設定 |
| scm | 6 | ソース管理ビュー基盤、quick diff、SCM 履歴グラフ |
| scrollLocking | 2 | エディタ間スクロール同期 |
| search | 5 | ワークスペース検索ビュー |
| searchEditor | 5 | 検索エディタ |
| share | 11 | 共有リンク生成メニュー |
| snippets | 1 | スニペット管理と補完統合 |
| speech | 12 | 音声入力プロバイダ統合 |
| splash | 2 | 起動スプラッシュ |
| styleOverrides | 2 | UI スタイルの実験的上書き |
| surveys | 13 | NPS と言語別アンケート |
| tags | 13 | ワークスペース内容のタグ付けテレメトリ |
| tasks | 13 | タスクシステム |
| telemetry | 13 | テレメトリ設定と送信の接着 |
| terminal | 7 | 統合ターミナル本体 |
| terminalContrib | 7 | ターミナル追加機能 25 個（find、links、suggest、chat 等） |
| testing | 13 | テストエクスプローラとカバレッジ |
| themes | 2 | テーマ選択 UI |
| timeline | 13 | タイムラインビュー基盤 |
| typeHierarchy | 4 | 型階層 peek |
| update | 13 | 自動更新とリリースノート |
| url | 13 | vscode:// URL 処理と信頼ドメイン |
| userDataProfile | 10 | プロファイル切替 |
| userDataSync | 10 | Settings Sync |
| webview | 9 | webview 基盤 |
| webviewPanel | 9 | webview のエディタ統合 |
| webviewView | 9 | サイドバー等への webview ビュー |
| welcomeAgentSessions | 12 | エージェントセッション向けウェルカム画面 |
| welcomeBanner | 13 | 初回バナー |
| welcomeGettingStarted | 13 | Getting Started / walkthrough エディタ |
| welcomeOnboarding | 13 | 新オンボーディングフロー（実験、テーマ選択等） |
| welcomeViews | 13 | 空ビューへの案内テキスト基盤 |
| welcomeWalkthrough | 13 | エディタプレイグラウンド解説文書 |
| workspace | 13 | Workspace Trust UI |
| workspaces | 13 | ワークスペースの保存/切替/最近の項目 |

### editor/contrib 全 59 ディレクトリ

| ディレクトリ | レイヤ | 説明 |
|---|---|---|
| anchorSelect | 1 | 選択アンカーを置いた範囲選択 |
| bracketMatching | 1 | 対応括弧の強調と括弧間ジャンプ |
| caretOperations | 1 | キャレット移動コマンドと文字転置 |
| clipboard | 1 | cut/copy/paste コマンド（リッチコピー含む） |
| codeAction | 4 | クイックフィックス/リファクタの実行 UI（電球） |
| codelens | 4 | コード行上のレンズ表示 |
| colorPicker | 4 | カラーデコレータとピッカー |
| comment | 1 | 行/ブロックコメントのトグル |
| contextmenu | 2 | エディタ右クリックメニュー |
| cursorUndo | 1 | カーソル移動だけを戻す soft undo |
| diffEditorBreadcrumbs | 6 | diff エディタへのパンくず統合 |
| dnd | 1 | 選択テキストのドラッグ移動 |
| documentSymbols | 4 | ドキュメントシンボル取得の共通化 |
| dropOrPasteInto | 1 | ドロップ/ペースト時の形式選択 |
| editorState | 1 | 操作中のエディタ状態検証とキャンセル（内部基盤） |
| find | 5 | エディタ内検索/置換ウィジェット |
| floatingMenu | 2 | エディタ上のフローティングメニュー |
| folding | 1 | コード折りたたみ |
| fontZoom | 2 | フォントズーム |
| format | 4 | フォーマット実行 |
| gotoError | 3 | 次/前のエラーへ移動 |
| gotoSymbol | 3 | 定義/型定義/実装/参照ジャンプと peek |
| gpu | 2 | GPU レンダリングのデバッグ用アクション |
| hover | 4 | ホバー表示 |
| inPlaceReplace | 1 | 値の巡回置換 |
| indentation | 1 | インデント種別の変換と再インデント |
| inlayHints | 4 | インレイヒント表示 |
| inlineCompletions | 12 | ゴーストテキスト補完エンジン（AI 補完の受け口） |
| inlineProgress | 2 | インライン進捗スピナー |
| insertFinalNewLine | 1 | 最終行改行の挿入 |
| lineSelection | 1 | 行単位選択 |
| linesOperations | 1 | 行の移動/複製/削除/ソート/join/ケース変換 |
| linkedEditing | 4 | 連動編集（開閉タグ同時変更） |
| links | 3 | リンク検出と Ctrl+クリック |
| longLinesHelper | 2 | 超長行での処理打ち切り保護 |
| message | 2 | エディタ上の一時メッセージ |
| middleScroll | 2 | 中ボタンオートスクロール |
| multicursor | 1 | マルチカーソルと全一致選択 |
| parameterHints | 4 | 引数ヒント |
| peekView | 4 | インライン埋め込みビューの共通基盤 |
| placeholderText | 2 | 空エディタのプレースホルダ |
| quickAccess | 3 | 行/シンボル移動の Quick Open プロバイダ |
| readOnlyMessage | 2 | 読み取り専用編集時の警告 |
| rename | 4 | シンボルリネーム |
| sectionHeaders | 2 | MARK: コメントのセクション見出し |
| semanticTokens | 4 | セマンティックハイライト適用 |
| smartSelect | 1 | 選択の構文的拡張/縮小 |
| snippet | 1 | スニペット挿入エンジン |
| stickyScroll | 3 | スコープ見出しの上部固定 |
| suggest | 4 | 補完ウィジェット |
| symbolIcons | 2 | シンボル種別アイコンの配色 |
| toggleTabFocusMode | 13 | Tab キーのフォーカス移動切替（a11y） |
| tokenization | 1 | 強制再トークン化アクション |
| unicodeHighlighter | 2 | 紛らわしい Unicode 文字の強調 |
| unusualLineTerminators | 1 | 異常行終端の検出と除去 |
| wordHighlighter | 4 | カーソル下シンボルの出現ハイライト |
| wordOperations | 1 | 単語単位のカーソル移動/削除 |
| wordPartOperations | 1 | camelCase 部分単位の移動/削除 |
| zoneWidget | 2 | 行間挿入ウィジェット基盤（peek の土台） |

### 同梱 built-in 拡張一覧

extensions/ 配下。言語系はグループ化した。

| グループ / 拡張 | 内容 |
|---|---|
| 言語 basics（47 個） | 文法（TextMate grammar）と言語設定のみ。bat, clojure, coffeescript, cpp, csharp, css, dart, diff, docker, dotenv, fsharp, go, groovy, handlebars, hlsl, html, ini, java, javascript, json, julia, latex, less, log, lua, make, markdown-basics, objective-c, perl, php, powershell, pug, python, r, razor, restructuredtext, ruby, rust, scss, shaderlab, shellscript, sql, swift, typescript-basics, vb, xml, yaml |
| *-language-features（6 個） | リッチ言語支援（LSP サーバ同梱等）。css, html, json, markdown, php, typescript。typescript-language-features は tsserver 統合で同梱拡張中最大 |
| テーマ（11 個） | theme-defaults, abyss, kimbie-dark, monokai, monokai-dimmed, quietlight, red, seti（ファイルアイコン）, solarized-dark, solarized-light, tomorrow-night-blue |
| タスク自動検出（4 個） | npm, gulp, grunt, jake |
| git | Git 統合本体。SCM プロバイダとして実装された最大級の機能拡張 |
| git-base | リモートソースプロバイダの共通基盤 |
| github | GitHub 連携（publish、リモート解決） |
| github-authentication / microsoft-authentication | 認証プロバイダ |
| copilot | GitHub Copilot（copilot-chat）。AI チャットが built-in 拡張化 |
| prompt-basics | プロンプトファイル (.prompt.md 系) の文法 |
| emmet | Emmet 展開 |
| merge-conflict | インラインコンフリクト解決 |
| search-result | 検索エディタ用の文法とジャンプ |
| references-view | 参照/呼び出し階層のツリービュー |
| simple-browser | webview ベースの簡易ブラウザ |
| media-preview | 画像/音声/動画プレビュー |
| markdown-math / mermaid-markdown-features | Markdown プレビューの KaTeX / Mermaid 対応 |
| notebook-renderers | ノートブック標準出力レンダラ |
| ipynb | .ipynb のシリアライズ対応 |
| debug-auto-launch / debug-server-ready | Node 自動アタッチ / サーバ起動検出 |
| terminal-suggest | ターミナル補完データ |
| tunnel-forwarding | トンネル転送 |
| configuration-editing / extension-editing | settings.json 系 / 拡張開発ファイルの編集支援 |
| テスト用（4 個） | vscode-api-tests, vscode-colorize-tests, vscode-colorize-perf-tests, vscode-test-resolver（製品には入らない） |
| types | 拡張向け共有型定義 |

なお marketplace 側にしか存在しない主要機能は、リモート開発 3 拡張（SSH / WSL / Dev Containers）、Live Share、js-debug、Copilot の推論サービス側、そして大半の言語拡張（Python、C/C++、Java 等）。「本体に見える機能」のかなりの部分がこの層にある。
