# Zed 機能全列挙

調査日: 2026-07-11。ソース: `/Users/daichi/Work/experience/shirushi/zed`（2026-07-10 時点のクローン。調査開始時は `/Users/daichi/Work/zed` にあったものと同一）。一次情報は crates/ 配下 237 crate のソース、`docs/src/`、`assets/settings/default.json`（2792 行）、`assets/keymaps/default-macos.json`（1683 行）。推測で書いた箇所には「とみられる」と付けた。

## 概要

Zed は 237 個の Rust crate からなるモノリシックなワークスペースで、拡張機構（WASM）に切り出されているのは言語・テーマ・スニペット・デバッグアダプタ・MCP/エージェントサーバといった宣言的な資産だけ。機能本体はすべて静的リンクされ、`zed` crate の初期化コードで結線される。つまり VSCode のような「コア + 拡張」ではなく「全部入りバイナリ + 資産拡張」という構成になっている。

層構造は下から順にこう積み上がっている。

1. **データ構造基盤**: `sum_tree`（並行フレンドリーな B+ 木）の上に `rope`（テキスト）、その上に `text`（CRDT バッファ。操作履歴、アンカー、undo）。`clock` の Lamport/ベクタクロックが CRDT の土台。コラボ機能はオプションだが、CRDT はバッファの標準表現としてコア編集そのものに焼き込まれている。ここは分離不能。
2. **GPUI**: 自前の GPU 描画 UI フレームワーク（`gpui` + プラットフォーム別バックエンド 6 種 + `scheduler`）。Metal/DirectX/Wayland/X11 に加え、`gpui_wgpu`（wgpu + cosmic-text）と `gpui_web`（ブラウザ）という新しいバックエンドがソースに存在する。
3. **プロジェクトモデル**: `fs` → `worktree` → `project`。`project` がバッファ、LSP、Git、タスク、設定を束ねる中枢で、ローカルとリモート（SSH/WSL/Dev Container）を同一 API で透過的に扱う。リモート開発が後付けでなくこの層に組み込まれているのが特徴。
4. **エディタ**: `multi_buffer` + `editor`。Zed のエディタは常にマルチバッファ（複数ファイルの断片を 1 つのバッファとして合成表示）の上に乗っており、プロジェクト検索、診断一覧、Git 差分、参照一覧がすべて「編集可能なマルチバッファ」として実装される。これが Zed 設計の中心で、専用の読み取り専用ビューをほとんど持たない理由。
5. **ワークスペースシェル**: `workspace`（ペイン、3 方向ドック、タブ、永続化）+ `ui`/`theme`/`picker` などの部品群。
6. **機能 crate 群**: Git、ターミナル、デバッガ、コラボ、AI などがパネル/モーダルとしてぶら下がる。

コアとオプションの分離線は crate の依存関係にはっきり出る。`editor` は `lsp` や `git` に依存するが、`vim`、`collab_ui`、`agent_ui`、`debugger_ui`、`repl` には依存しない（逆方向のみ）。設定面でも `vim_mode`、`disable_ai`（AI 全機能の一括無効化）、`enable_language_server` のようにオプション側だけスイッチが用意されている。ゼロから作る場合の最小核は「gpui 相当 + rope/text + editor + workspace + fs/worktree」で、そこに tree-sitter（`language`）、LSP、ターミナル、Git 表示を足した段階で「現代のエディタ」の期待値に届く、というのが本ソースから読める線引き。

区分の定義（以降の表で使用）:

- **中核**: これが無いとテキストエディタとして成立しない
- **準中核**: 現代のコードエディタとして実質必須。全ユーザーが期待する
- **オプション**: あれば強いが、無くてもエディタとして成立する

## 機能一覧（レイヤ別）

### 1. コア編集

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| ロープによるテキスト保持 | 中核 | `rope`, `sum_tree` | 全テキストが sum_tree 上のロープ。行/オフセット/ポイント変換もここ |
| CRDT テキストバッファ（操作、アンカー、バージョン） | 中核 | `text`, `clock` | undo 履歴と位置アンカーの実装基盤。コラボはこの上の応用 |
| undo/redo（操作履歴、トランザクション） | 中核 | `text`, `editor` | トランザクション単位のグルーピングあり |
| 基本編集操作（挿入/削除/行複製/行移動/行結合/大文字小文字変換など） | 中核 | `editor` | アクション数は非常に多い（docs/src/all-actions.md 参照） |
| カーソル移動・スクロール（垂直マージン、高速スクロール、マウスホイールズーム） | 中核 | `editor` | `vertical_scroll_margin`, `fast_scroll_sensitivity`, `mouse_wheel_zoom` 設定 |
| 複数選択・マルチカーソル | 準中核 | `editor` | `multi_cursor_modifier` 設定。列選択、選択の上下追加、全一致選択 |
| multibuffer（複数ファイルの断片を 1 バッファに合成表示・直接編集） | オプション | `multi_buffer`, `editor` | 一般エディタ基準ではオプションだが Zed 設計の中心。検索/診断/差分/参照が全部これ |
| クリップボード（コピー/カット/ペースト、中クリックペースト） | 中核 | `gpui`, `editor` | `middle_click_paste`（Linux 系）。ペースト時自動インデント `auto_indent_on_paste` |
| 自動インデント | 準中核 | `editor`, `language` | tree-sitter のインデントクエリ駆動 |
| 括弧の自動クローズ/自動サラウンド/対応括弧色付け | 準中核 | `editor`, `language_core` | `use_autoclose`, `use_auto_surround`, `colorize_brackets` |
| コメントトグル（言語設定駆動、改行時コメント継続） | 準中核 | `editor`, `language` | `extend_comment_on_newline` |
| スニペット（tabstop、プレースホルダ、choice） | 準中核 | `snippet`, `snippet_provider`, `snippets_ui` | LSP スニペットとユーザー定義（`~/.config/zed/snippets`）と拡張提供の 3 系統 |
| コード折りたたみ（構文/LSP folding range、ギャッター操作） | 準中核 | `editor`, `language` | `document_folding_ranges` 設定で LSP 由来に切替可 |
| ソフトラップ、折り返しガイド | 準中核 | `editor` | `soft_wrap`, `wrap_guides`, `preferred_line_length` |
| 空白文字の可視化 | オプション | `editor` | `show_whitespaces`, `whitespace_map`（表示文字も変更可） |
| 行番号/相対行番号 | 中核/オプション | `editor` | 相対行番号（`relative_line_numbers`）は vim 用途のオプション |
| ファイルエンコーディング選択 | 準中核 | `encoding_selector`, `fs` | ステータスバーから切替 |
| 改行コード（LF/CRLF）選択 | 準中核 | `line_ending_selector` | `line_ending` 設定（auto 検出含む） |
| 保存時処理（フォーマット、末尾空白除去、最終改行保証） | 準中核 | `editor`, `project` | `format_on_save`, `remove_trailing_whitespace_on_save`, `ensure_final_newline_on_save` |
| 自動保存（フォーカス喪失/遅延/ウィンドウ切替） | 準中核 | `workspace` | `autosave` |
| 外部変更の検知・再読み込み・競合表示 | 準中核 | `fs`, `worktree`, `editor` | キーマップに InvalidBuffer コンテキストあり（壊れたバッファの扱い） |
| rewrap（コメント幅での再折り返し） | オプション | `editor` | `allow_rewrap` |
| Vim モード（モーダル編集、テキストオブジェクト、レジスタ、マーク、ex コマンド） | オプション | `vim`, `vim_mode_setting` | tree-sitter/LSP/Git と統合した独自拡張あり。プラグイン相当（surround 等）を同梱 |
| Helix モード | オプション | `vim`（内部で共用） | `helix_mode` 設定 |
| modeline（ファイル先頭/末尾の設定行解釈） | オプション | `editor` | `modeline_lines` 設定。docs/src/modelines.md |
| 選択のドラッグ&ドロップ移動 | オプション | `editor` | `drag_and_drop_selection` |
| 秘匿値のマスク表示（private files の redact） | オプション | `editor` | `redact_private_values`, `private_files` |
| インライン入力（IME）対応 | 中核 | `gpui`, `editor` | プラットフォーム層で input handler を実装 |

### 2. 描画・UI

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| GPU 描画 UI フレームワーク | 中核 | `gpui`, `gpui_macros`, `gpui_platform`, `gpui_shared_string`, `gpui_util`, `gpui_tokio` | ウィンドウ、要素ツリー、レイアウト（taffy）、入力、アクションディスパッチまで全部ここ |
| プラットフォームバックエンド | 中核 | `gpui_macos`(Metal), `gpui_windows`(DirectX), `gpui_linux`(Wayland/X11), `gpui_wgpu`(wgpu+cosmic-text), `gpui_web`(ブラウザ) | wgpu と web は新設。web は実験段階とみられる |
| 非同期実行基盤（foreground/background executor、決定的テスト） | 中核 | `scheduler`, `gpui_tokio` | GPUI のテストはスケジューラ差し替えで決定的に走る |
| テキストシェーピング・フォントレンダリング・合字 | 中核 | `gpui`（text system） | `buffer_font_features` で OpenType 機能切替。`text_rendering_mode` 設定 |
| ワークスペースシェル（ペイン分割、タブ、3 方向ドック、ズーム） | 中核 | `workspace`, `panel` | ペイン分割は準中核。ドック位置/サイズ、`bottom_dock_layout`, `centered_layout` |
| タブバー（ピン留め、プレビュータブ、タブ上限、並び替え） | 準中核 | `workspace` | `tabs`, `tab_bar`, `preview_tabs`, `max_tabs` |
| ステータスバー | 準中核 | `workspace` + 各 crate | 各機能がアイテムを登録する方式。`status_bar` 設定 |
| ツールバー + パンくず | 準中核 | `workspace`, `breadcrumbs` | パス + シンボル階層表示 |
| タイトルバー（プロジェクト名、コラボ状態、システムウィンドウタブ） | 準中核 | `title_bar`, `platform_title_bar` | `use_system_window_tabs`（macOS） |
| テーマシステム（light/dark/system 追従） | 準中核 | `theme`, `theme_settings`, `syntax_theme`, `theme_selector` | テーマ切替モーダルはライブプレビュー付き |
| VSCode テーマのインポート | オプション | `theme_importer` | 開発ツール寄り |
| アイコンテーマ（ファイルアイコン差し替え） | オプション | `file_icons`, `icons` | 拡張で追加可能 |
| UI コンポーネントライブラリ | 中核（内部基盤） | `ui`, `ui_input`, `ui_macros`, `component` | ボタン、リスト、モーダル等の部品。`component_preview` はギャラリー（開発用） |
| ピッカー（モーダル型ファジーリスト）基盤 | 準中核 | `picker`, `picker_preview` | file finder やテーマ選択の共通基盤 |
| アプリ内ダイアログ/パスプロンプト（OS ダイアログ代替） | 準中核 | `ui_prompt`, `open_path_prompt` | `use_system_prompts`, `use_system_path_prompts` で切替 |
| 通知・トースト | 準中核 | `workspace`, `notifications`（status_toast） | |
| アクティビティインジケータ（LSP ダウンロード/起動状況） | 準中核 | `activity_indicator` | ステータスバー左下 |
| ネイティブメニュー/共通メニューアクション | 準中核 | `zed`（メニュー定義）, `menu` | |
| ミニマップ | オプション | `editor` | `minimap` 設定 |
| スクロールバー（診断/検索/差分/カーソルのマーク表示） | 準中核 | `editor` | `scrollbar` 設定で表示物を制御 |
| sticky scroll（スクロール中の親スコープ固定表示） | オプション | `editor` | `sticky_scroll` |
| インデントガイド | 準中核 | `editor` | `indent_guides`（アクティブ強調含む） |
| ギャッター（折りたたみ、実行ボタン、ブレークポイント、Git hunk） | 準中核 | `editor` | `gutter` 設定 |
| Markdown レンダリング（ホバー/ドキュメント表示用） | 準中核 | `markdown` | エージェントパネルでも使用 |
| Markdown プレビュー | オプション | `markdown_preview`, `mermaid_render` | Mermaid 図の SVG 描画対応 |
| SVG プレビュー | オプション | `svg_preview` | |
| CSV テーブルプレビュー | オプション | `csv_preview` | ソート等を持つ table_data_engine 内蔵 |
| 画像ビューア | オプション | `image_viewer` | `image_viewer` 設定（単位表示） |
| カーソル形状/点滅、マウス自動非表示、フォーカス追従 | オプション | `editor`, `gpui` | `cursor_shape`, `cursor_blink`, `hide_mouse`, `focus_follows_mouse` |
| 選択ハイライト、現在行ハイライト、角丸選択 | オプション | `editor` | `selection_highlight`, `current_line_highlight`, `rounded_selection` |
| 最低コントラスト保証（ハイライト上の文字色補正） | オプション | `editor` | `minimum_contrast_for_highlights` |
| GPUI 要素インスペクタ | オプション（開発用） | `inspector_ui` | 実行中 UI の要素を調査 |
| フレームプロファイラ/入力レイテンシ計測 | オプション（開発用） | `miniprofiler_ui`, `input_latency_ui` | 入力→フレームのヒストグラム表示アクションあり |
| ウィンドウ装飾/透過等のプラットフォーム調整 | オプション | `gpui`, `workspace` | `window_decorations`（Linux）, `active_pane_modifiers` |

### 3. ナビゲーション

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| ファジーファイル検索（file finder） | 準中核 | `file_finder`, `fuzzy` | ファイル名 + パスのスコアリング。`file_finder` 設定（プレビュー等） |
| ファジーマッチングエンジン | 準中核（内部基盤） | `fuzzy`, `fuzzy_nucleo` | 自前実装と nucleo ベースの 2 系統が併存。移行中とみられる |
| コマンドパレット | 準中核 | `command_palette`, `command_palette_hooks` | hooks で vim 等がアクションの表示/挙動を差し替える |
| 行:列ジャンプ | 中核 | `go_to_line` | |
| シンボルアウトライン（モーダル） | 準中核 | `outline` | tree-sitter のアウトラインクエリ駆動。LSP 不要で動く |
| アウトラインパネル | オプション | `outline_panel` | マルチバッファ（検索結果や差分）の目次としても機能 |
| ワークスペースシンボル検索 | 準中核 | `project_symbols` | LSP workspace/symbol |
| プロジェクトパネル（ファイルツリー） | 準中核 | `project_panel` | 作成/リネーム/削除/ドラッグ移動、Git 状態表示、`hidden_files` 表示切替 |
| タブスイッチャー（ctrl-tab、MRU 順） | オプション | `tab_switcher` | |
| 最近のプロジェクト/リモート接続ピッカー | 準中核 | `recent_projects` | WorktreePicker、SSH 接続先もここから |
| ナビゲーション履歴（戻る/進む） | 準中核 | `workspace`, `editor` | ペイン単位の履歴スタック |
| パンくずによるシンボル階層ジャンプ | オプション | `breadcrumbs` | |
| ブックマーク | 該当なし | なし | Zed に無い。vim モードのマークで代替（「Zed に無いもの」参照） |

### 4. 言語知能

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| tree-sitter シンタックスハイライト | 準中核 | `language`, `language_core`, `grammars` | 増分パース。injection（埋め込み言語）、ローカル変数スコープ対応 |
| 同梱 tree-sitter 文法 | 準中核 | `grammars` | bash/c/cpp/css/diff/gitcommit/go/gomod/gowork/js/jsdoc/json/jsonc/markdown/python/regex/rust/tsx/ts/yaml + キーマップコンテキスト述語用の専用文法 |
| 言語レジストリ・言語ごとの設定 | 準中核 | `language`, `language_core` | 言語設定（コメント文字、括弧、インデント等）は宣言的 config |
| 組み込み言語 + LSP アダプタ | 準中核 | `languages` | Rust(rust-analyzer)/C/C++(clangd)/Python/TS/JS(vtsls, eslint)/Go/JSON/YAML/CSS/Bash/Tailwind。他言語は拡張（自動インストール可） |
| LSP クライアント（マルチサーバ、サーバ自動ダウンロード） | 準中核 | `lsp`, `project` | 1 言語に複数サーバ可（`language_servers` で順序/有効化制御）。`global_lsp_settings` |
| 補完（LSP + スニペット + バッファ内単語） | 準中核 | `editor`, `project` | `completions` 設定に words/LSP のモード。ドキュメント表示、詳細の配置調整 |
| ホバー（ドキュメント表示） | 準中核 | `editor` | `hover_popover_*` 設定群 |
| シグネチャヘルプ | 準中核 | `editor` | `auto_signature_help` |
| 定義/型定義/実装/宣言ジャンプ | 準中核 | `editor`, `project` | `go_to_definition_fallback`（失敗時に参照検索へフォールバック） |
| 参照検索（マルチバッファ表示） | 準中核 | `editor` | 結果がそのまま編集可能 |
| リネーム（シンボル） | 準中核 | `editor` | インライン編集 UI（renaming コンテキスト） |
| リンク編集（linked editing range、JSX タグ連動） | オプション | `editor` | `linked_edits`, `jsx_tag_auto_close` |
| コードアクション/クイックフィックス | 準中核 | `editor` | ギャッターまたはインライン表示（`inline_code_actions`） |
| code lens | オプション | `editor` | `code_lens` 設定 |
| 診断（インライン、波線、プロジェクト診断ビュー） | 準中核 | `diagnostics`, `editor` | 診断ビューはマルチバッファ。`diagnostics` 設定でインライン表示等を制御 |
| インレイヒント | 準中核 | `editor` | `inlay_hints`（型/パラメータ名、トグルあり） |
| セマンティックトークン | オプション | `editor`, `language` | `semantic_tokens` 設定（tree-sitter との優先関係を選択） |
| ドキュメントカラー（色値のスウォッチ表示） | オプション | `editor` | `lsp_document_colors` |
| ドキュメントリンク | オプション | `editor` | `lsp_document_links` |
| ドキュメントハイライト（同一シンボルの出現強調） | オプション | `editor` | `lsp_highlight_debounce` |
| フォーマッタ（LSP/外部コマンド/Prettier、範囲フォーマット、on-type） | 準中核 | `project`, `prettier`, `editor` | `formatter` は配列指定可。`use_on_type_format`, `code_actions_on_format` |
| Prettier 同梱 | オプション | `prettier`, `node_runtime` | プロジェクトの prettier を優先、無ければ内蔵版 |
| Node.js ランタイム自動管理 | オプション（内部基盤） | `node_runtime` | LSP/Prettier 用に Node を自動取得。`node` 設定で自前 Node 指定可 |
| ツールチェイン選択（Python venv 等） | オプション | `toolchain_selector`, `language_core` | ステータスバーから切替。terminal の venv 自動検出と連動 |
| 言語モード切替 | 準中核 | `language_selector` | ファイルタイプの手動指定。`file_types` でパターン割当 |
| LSP ログ/シンタックスツリービューア | オプション（開発用） | `language_tools` | デバッグ用パネル |
| 言語別オンボーディング | オプション | `language_onboarding` | 現状 Python 向けのみ |
| 未使用コードのフェード表示 | オプション | `editor` | `unnecessary_code_fade` |

### 5. 検索・置換

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| バッファ内検索（インクリメンタル、正規表現、大小文字、単語単位） | 中核 | `search` | 検索バー UI。`search` 設定にデフォルトオプション、`use_smartcase_search` |
| バッファ内置換 | 中核 | `search` | in_replace コンテキストで置換フィールド展開 |
| プロジェクト全体検索（結果はマルチバッファ、直接編集可） | 準中核 | `search`, `multi_buffer` | include/exclude グロブ、開いているファイル限定等のフィルタ |
| プロジェクト全体置換 | 準中核 | `search` | マルチバッファ上で一括置換 |
| 検索履歴・カーソル位置からのクエリ初期化 | オプション | `search` | `seed_search_query_from_cursor`, `search_wrap` |
| ファイル走査の制御（除外/包含/シンボリックリンク） | 準中核 | `worktree`, `project` | `file_scan_exclusions`, `file_scan_inclusions`, `scan_symlinks` |

### 6. VCS / Git

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| Git リポジトリ検出・ステータス追跡 | 準中核 | `git`, `worktree`, `project` | ファイルツリーとタブに状態色を表示 |
| ギャッターの差分表示（hunk）+ インライン展開 + hunk 単位の stage/restore | 準中核 | `buffer_diff`, `editor` | エディタ内で hunk を展開して編集/取り消し。単語単位差分 `word_diff_enabled` |
| Git パネル（変更一覧、ステージング、コミット） | 準中核 | `git_ui` | コミットメッセージエディタは専用コンテキスト。AI によるコミットメッセージ生成もここ |
| プロジェクト差分ビュー（全変更を 1 つのマルチバッファで） | オプション | `git_ui`, `multi_buffer` | Zed の Git 体験の中心。unified/split 表示 `diff_view_style` |
| コミット履歴/ファイル履歴、Git グラフ | オプション | `git_ui` | キーマップに GitGraph コンテキストあり（グラフ UI は新しめ） |
| ブランチ作成/切替/削除 | 準中核 | `git_ui`（GitBranchSelector） | |
| スタッシュ（一覧、差分表示） | オプション | `git_ui`（StashList, StashDiff） | |
| fetch / push / pull、リモート管理 | 準中核 | `git`, `git_ui` | push 時の認証は askpass 経由 |
| git blame（インライン + 詳細） | オプション | `git`, `editor` | インライン blame は設定でオフ可 |
| マージコンフリクト解消 UI | 準中核 | `git_ui`, `editor` | ours/theirs/両方の選択ボタン |
| Git worktree（複数ワークツリー）対応 | オプション | `git`, `git_ui` | docs に専用節。マルチルートワークスペースと連動 |
| ホスティング連携（permalink 生成、PR 参照） | オプション | `git_hosting_providers` | GitHub/GitLab/Bitbucket/Gitea/Forgejo/Gitee/SourceHut/Azure DevOps/Chromium。`git_hosting_providers` 設定でセルフホスト対応 |
| SSH/Git 認証プロンプト仲介 | 準中核（内部基盤） | `askpass` | リモート開発とも共用 |

### 7. ターミナル

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| 統合ターミナル（エミュレーション） | 準中核 | `terminal` | alacritty_terminal ベース |
| ターミナル UI（下ドックのパネル + センター領域のタブ、分割） | 準中核 | `terminal_view` | 複数ターミナル、パネル/エディタ領域どちらでも開ける |
| シェル/作業ディレクトリ/環境変数の制御 | 準中核 | `terminal`, `project` | `terminal` 設定。direnv 読み込み `load_direnv`、Python venv 自動アクティベート |
| パスのハイパーリンク化（クリックでファイル:行へ） | 準中核 | `terminal_view` | ビルドエラーからのジャンプ手段（problem matcher の代替） |
| ターミナル内検索、copy on select、vi モード | オプション | `terminal_view` | |
| タスク実行（task templates、変数展開、再実行、oneshot） | 準中核 | `task`, `tasks_ui` | `.zed/tasks.json` + VSCode `tasks.json` 互換読み込み。実行はターミナル上 |
| runnables（tree-sitter タグ→ギャッターの実行ボタン） | オプション | `language`（runnable クエリ）, `tasks_ui` | テスト関数の横に再生ボタンを出す仕組み |
| タスクのフック/カスタム Git コマンド/キーバインド起動 | オプション | `task`, `tasks_ui` | docs/src/tasks.md に一覧 |

### 8. デバッグ

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| DAP クライアント | オプション | `dap` | IDE 級では期待されるが、エディタ成立には不要という位置付け |
| 組み込みデバッグアダプタ | オプション | `dap_adapters` | CodeLLDB/GDB/Delve(Go)/debugpy(Python)/vscode-js-debug(JS)。他言語は拡張で追加 |
| デバッガ UI（スタック、変数、ウォッチ、コンソール、ブレークポイント一覧） | オプション | `debugger_ui` | 専用パネル + ペイン分割対応。RunModal（セッション開始）、DebugConsole、VariableList、BreakpointList |
| ブレークポイント（行、ログポイント、条件） | オプション | `debugger_ui`, `editor` | ギャッター操作。保存設定 `debugger`（Save Breakpoints） |
| ビルドタスクからのデバッグシナリオ自動生成（locator） | オプション | `debugger_ui`, `task` | cargo 等のタスクを起動構成に変換 |
| インライン値表示 | オプション | `debugger_ui`, `editor` | 変数値をコード横に表示 |
| DAP 通信ログビューア | オプション（開発用） | `debugger_tools` | |
| 拡張提供のデバッグアダプタ | オプション | `debug_adapter_extension` | 拡張機構との橋渡し |

### 9. 拡張モデル

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| WASM 拡張ランタイム（wasmtime、WIT インターフェース） | オプション | `extension`, `extension_host` | 拡張はサンドボックス内 WASM。VSCode のような JS API 全開放ではない |
| 拡張開発 API（Rust → WASM） | オプション | `extension_api` | 公開 crate。process:exec 等は capability 宣言制 |
| 拡張マーケットプレイス UI + 自動インストール | オプション | `extensions_ui` | `auto_install_extensions` 設定。ローカル dev 拡張のインストールも可 |
| capability 制御（process:exec / download_file / npm:install） | オプション | `extension_host` | `granted_extension_capabilities` 設定。docs/src/extensions/capabilities.md |
| 言語拡張（文法 + 言語設定 + LSP 起動） | オプション | `language_extension` | 拡張の主用途。公式 docs の言語の大半はこれ |
| テーマ/アイコンテーマ拡張 | オプション | `theme_extension` | |
| スニペット拡張 | オプション | `snippet_provider` | |
| デバッグアダプタ拡張 | オプション | `debug_adapter_extension` | |
| MCP サーバ拡張 / エージェントサーバ拡張 | オプション | `extension`, `context_server`, `agent_servers` | AI 系も拡張ポイント化されている |
| 拡張パッケージング CLI | オプション | `extension_cli` | 拡張レジストリ登録用 |

### 10. 設定・キーマップ

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| JSON 設定の階層マージ（default → user → プロジェクト `.zed/settings.json`） | 中核 | `settings`, `settings_content`, `settings_json`, `settings_macros` | コメント付き JSON。保存即反映（ファイル監視） |
| 言語別オーバーライド / OS 別 / リリースチャンネル別設定 | 準中核 | `settings` | `languages`、`macos`/`linux`/`windows`、`stable`/`preview`/`nightly`/`dev` キー |
| 設定プロファイル（名前付き設定セットの切替） | オプション | `settings_profile_selector` | `profiles` 設定 |
| GUI 設定エディタ | オプション | `settings_ui` | SettingsWindow。JSON と併存 |
| 設定/キーマップの自動マイグレーション | オプション | `migrator` | 旧形式を検出して書き換え提案 |
| JSON スキーマ提供（設定ファイルの補完・検証） | 準中核 | `json_schema_store`, `schema_generator` | `$schema: zed://schemas/settings`。スキーマは自動生成 |
| キーマップ（JSON、コンテキスト述語付きバインド） | 中核 | `gpui`（keymap）, `zed` | コンテキスト式（例 `Editor && mode == full`）は専用文法。100 超のコンテキストを確認 |
| ベースキーマッププリセット | 準中核 | `assets/keymaps` | VSCode/Atom/JetBrains/SublimeText/TextMate/Cursor/Emacs + vim.json |
| GUI キーマップエディタ（キーストローク録音） | オプション | `keymap_editor` | KeystrokeInput コンテキスト |
| which-key（押下途中のキー候補ポップアップ） | オプション | `which_key` | `which_key` 設定 |
| コマンドエイリアス | オプション | `zed` | `command_aliases` 設定 |
| フィーチャーフラグ（段階的ロールアウト） | オプション（内部基盤） | `feature_flags`, `feature_flags_macros` | サーバ駆動 |
| 環境変数の扱い（direnv、CLI 環境の継承） | オプション | `zed_env_vars`, `env_var`, `project` | |

### 11. コラボレーション

Zed のアイデンティティ機能だが、エディタとしては全部オプション。CRDT バッファ（レイヤ 1）が前提のため追加実装コストが低い、というのが Zed の構造上の勝ち筋。

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| リアルタイム共同編集（プロジェクト共有、ゲスト編集） | オプション | `client`, `rpc`, `proto`, `collab`（サーバ） | CRDT により競合レス。サーバは同リポジトリに同梱（k8s マニフェスト付き、GPL） |
| following（他者のカーソル/ビューへの追従） | オプション | `workspace`, `collab_ui` | ペイン単位で追従 |
| 音声通話・画面共有 | オプション | `call`, `livekit_client`, `livekit_api`, `audio`, `media` | LiveKit ベース。`calls`/`audio` 設定（デバイス選択、自動ミュート等） |
| チャンネル（ツリー状の常設ルーム）+ チャンネルノート（共有バッファ） | オプション | `channel`, `collab_ui` | ノートは CRDT バッファの永続化版 |
| チャット | オプション | `channel`, `collab_ui` | チャンネル内テキストチャット |
| コンタクト・プライベートコール | オプション | `collab_ui` | |
| コラボパネル | オプション | `collab_ui` | `collaboration_panel` 設定 |
| 通知（招待、メンション等） | オプション | `notifications`, `collab_ui` | |
| zed.dev 認証（GitHub サインイン） | オプション | `client`, `oauth_callback_server` | AI のクラウド機能とアカウント共用 |

### 12. AI / エージェント

全機能が `disable_ai: true` で一括無効化できる。ゼロから作る場合、この層は丸ごと後回しにできる構造。

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| ネイティブエージェント（ツールループ、スレッド管理） | オプション | `agent` | ツール: read/write/edit file, grep, find_path, list_directory, terminal, fetch, web_search, diagnostics, go_to_definition, find_references, code actions, rename, スキル実行、サブエージェント生成（spawn_agent/create_thread） |
| エージェントパネル | オプション | `agent_ui` | スレッド表示、メッセージエディタ、モデル/プロファイル切替、@メンション（ファイル等のコンテキスト添付） |
| インラインアシスタント（エディタ内の選択範囲を指示で書き換え） | オプション | `agent_ui`（inline_assistant, buffer_codegen）, `streaming_diff` | ターミナル内インラインアシストもあり |
| エージェント編集のレビュー UI（AgentDiff、accept/reject） | オプション | `agent_ui`, `action_log` | エージェントの全編集を差分で追跡し個別に採否 |
| 外部エージェント統合（ACP: Agent Client Protocol） | オプション | `agent_servers`, `acp_thread`, `acp_tools` | Claude Code/Codex/Gemini CLI/OpenCode/Copilot/Cursor/Pi + レジストリ + カスタム。UI は同一スレッド画面を共用 |
| 並列エージェント（複数スレッド同時実行、スレッドサイドバー） | オプション | `sidebar`, `agent_ui` | ThreadsSidebar/ThreadSwitcher コンテキスト。ターミナルスレッド（CLI エージェントをタブで並走）も docs に記載 |
| エージェントのサンドボックス実行 | オプション | `sandbox`, `shell_command_parser`, `http_proxy` | コマンドを OS サンドボックスで隔離し、ネットワークはホスト許可リスト付きプロキシ経由に限定。shell_command_parser が実行プログラムを解析して権限判定 |
| ツール権限（許可/確認/拒否のルール） | オプション | `agent`（tool_permissions） | プロファイル（Write/Ask/読み取り専用等）と連動 |
| スキル（SKILL.md） | オプション | `agent_skills`, `agent_ui`（SkillCreator） | スコープ付きスキル読み込み |
| ルール/プロンプトライブラリ | オプション | `prompt_store` | rules → skills への移行コードあり |
| MCP（Model Context Protocol）クライアント | オプション | `context_server` | 設定 `context_servers` + 拡張提供 + エージェントからのツール利用 |
| edit prediction（次編集予測、tab 適用） | オプション | `edit_prediction`, `edit_prediction_types`, `edit_prediction_ui`, `zeta_prompt` | Zed 独自モデル Zeta（クラウド）が既定。eager/subtle の 2 モード、除外パス設定 `edit_predictions_disabled_in` |
| edit prediction の代替プロバイダ | オプション | `copilot`, `codestral`, `edit_prediction`（mercury.rs, ollama.rs, open_ai_compatible.rs, fim.rs） | GitHub Copilot/Codestral/Mercury/Ollama/OpenAI 互換(FIM)。ローカルモデルも可 |
| 予測コンテキスト収集 | オプション（内部基盤） | `edit_prediction_context` | BM25 検索、git log、編集可能領域の組み立て。かなり作り込まれている |
| LLM プロバイダ抽象 + レジストリ | オプション | `language_model`, `language_model_core`, `language_models` | リクエスト/ツールスキーマ/レート制限の共通化 |
| LLM プロバイダ実装 | オプション | `anthropic`, `open_ai`, `google_ai`, `bedrock`, `mistral`, `deepseek`, `ollama`, `lmstudio`, `llama_cpp`, `open_router`, `x_ai`, `copilot_chat`, `opencode`, `language_models_cloud` | provider/ ディレクトリには他に anthropic_compatible, open_ai_compatible, api_compatible, openai_subscribed（ChatGPT サブスク）, vercel_ai_gateway。Zed ホストモデルは cloud_llm_client 経由 |
| Zed クラウド API（アカウント、課金、ホストモデル） | オプション | `cloud_api_client`, `cloud_api_types`, `cloud_llm_client` | |
| Web 検索ツール | オプション | `web_search`, `web_search_providers` | エージェントのツールとして提供 |
| HTML→Markdown 変換 | オプション（内部基盤） | `html_to_markdown` | fetch ツールの整形に使用 |
| コミットメッセージ生成/スレッド要約などの単発 LLM 利用 | オプション | `git_ui`, `agent` | |
| AI オンボーディング/プラン案内 | オプション | `ai_onboarding`, `onboarding` | ZedPredictModal 等 |
| 評価基盤（エージェント/予測の品質測定） | オプション（開発用） | `eval_cli`, `eval_utils`, `edit_prediction_cli`, `edit_prediction_metrics` | ヘッドレスでエージェントを走らせる CLI まで同梱 |

### 13. その他（プラットフォーム基盤・周辺機能）

| 機能 | 区分 | 実装 (crate等) | 備考 |
|---|---|---|---|
| アプリ本体（初期化、結線、メニュー、URL スキーム処理） | 中核 | `zed`, `zed_actions`, `assets` | zed_actions は循環依存回避のためのアクション置き場 |
| プロジェクト中枢モデル | 中核 | `project` | バッファストア/LSP ストア/Git ストア/タスク/設定を束ね、ローカル・リモートを透過化。実装上の最重要 crate の一つ |
| ワークツリー（ディレクトリ走査、gitignore、変更監視） | 中核 | `worktree` | 巨大リポジトリ向けに増分スキャン |
| ファイルシステム抽象（実 FS + テスト用 FakeFs、ゴミ箱、監視） | 中核 | `fs` | 決定的テストの土台 |
| 状態永続化（開いていたウィンドウ/タブ/レイアウトの復元） | 準中核 | `db`, `sqlez`, `sqlez_macros`, `workspace`（persistence） | SQLite。`restore_on_startup`, `restore_on_file_reopen` |
| セッション管理（クラッシュ/再起動をまたぐウィンドウ復元） | 準中核 | `session` | セッション ID で前回の異常終了を検出 |
| CLI（`zed` コマンド、行:列指定オープン、diff 起動） | 準中核 | `cli`, `install_cli`, `net` | net は CLI↔アプリの IPC（Unix ソケット/名前付きパイプ） |
| 自動アップデート | 準中核 | `auto_update`, `auto_update_helper`, `auto_update_ui` | helper は Windows 用の差し替え補助バイナリ |
| リリースチャンネル（Stable/Preview/Nightly/Dev） | オプション | `release_channel` | チャンネル別設定オーバーライドあり |
| リモート開発（SSH/WSL、ヘッドレスサーバ常駐） | オプション | `remote`, `remote_connection`, `remote_server` | UI はローカル、project がリモートで動く方式。`ssh_connections`/`read_ssh_config` 設定 |
| Dev Containers | オプション | `dev_container` | devcontainer.json ベースの環境起動 |
| REPL / Jupyter カーネル統合（コード横のインライン出力、セルモード） | オプション | `repl` | kernelspec 検出、`jupyter`/`repl` 設定。`repl/src/notebook/` に .ipynb 向けノートブック UI が実装中（キーマップに NotebookEditor コンテキストあり） |
| ジャーナル（日次 Markdown ノート） | オプション | `journal` | `journal` 設定 |
| オンボーディング（ようこそ画面、ベースキーマップ/テーマ選択） | オプション | `onboarding` | multibuffer の使い方ヒント等も |
| テレメトリ（opt-out 可）とイベント定義 | オプション | `telemetry`, `telemetry_events`, `client` | `telemetry` 設定で診断/メトリクス別に制御 |
| クラッシュレポート（minidump） | オプション | `crashes`, `system_specs` | |
| フィードバック送信 | オプション | `feedback` | |
| ログ/トレーシング | オプション（内部基盤） | `zlog`, `zlog_settings`, `ztracing`, `ztracing_macro`, `etw_tracing` | etw_tracing は Windows ETW。`log`/`instrumentation` 設定 |
| HTTP クライアント基盤 | 中核（内部基盤） | `http_client`, `http_client_tls`, `reqwest_client`, `aws_http_client` | aws_http_client は Bedrock 用署名対応 |
| 資格情報保存（キーチェーン） | 準中核（内部基盤） | `credentials_provider`, `zed_credentials_provider` | API キーやトークンの保存先 |
| プロキシ設定 | オプション | `http_client`（`proxy` 設定） | |
| OS 統合（Windows） | オプション | `windows_resources`, `explorer_command_injector` | エクスプローラ右クリック「Open with Zed」（COM DLL） |
| 汎用ユーティリティ | 中核（内部基盤） | `util`, `util_macros`, `collections`, `paths`, `time_format`, `refineable`, `watch` | refineable はカスケード設定用 derive マクロ |
| ドキュメントビルド/ベンチマーク | オプション（開発用） | `docs_preprocessor`, `benchmarks`, `editor_benchmarks`, `fs_benchmarks`, `project_benchmarks`, `worktree_benchmarks` | 製品機能ではない |

## Zed に無いもの

VSCode と比べたときの欠落。自作エディタのスコープ判断の参考として、ソースと公式 docs から確認できた範囲で書く。

| 無い/弱い機能 | 状況 | 根拠 |
|---|---|---|
| 拡張による UI 拡張 | 不可。拡張が追加できるのは言語/テーマ/アイコン/スニペット/デバッグアダプタ/MCP/エージェントサーバのみで、webview やカスタムパネル、ステータスバーアイテム、ツリービューは作れない | docs/src/extensions/capabilities.md（capability は process:exec, download_file, npm:install の 3 種のみ） |
| Jupyter Notebook（.ipynb）の本格編集 | 弱い。REPL（コード横インライン出力）は完成しているが、ノートブック UI は `repl/src/notebook/` に実装途上のものがある段階 | crates/repl/src/notebook/notebook_ui.rs、キーマップの NotebookEditor コンテキスト |
| タスクの problem matcher | 無い。タスク実行はあるがタスク出力を診断に変換する仕組みが無く、ターミナルのパスリンクで代替 | crates/task と docs/src/tasks.md に problem matcher 該当なし（grep で確認） |
| Settings Sync（アカウントによる設定同期） | 無い。設定はローカル JSON のみ | 設定サーフェスに同期関連キーなし |
| Local History / Timeline（未コミット変更の時系列履歴） | 無い。Git のファイル履歴はあるが、コミット外の編集履歴は保持しない | git_ui に File History はあるが editor/workspace に該当機能なし |
| テストエクスプローラ | 無い。runnables（ギャッターの実行ボタン）+ タスクで代替する設計 | 該当 crate なし |
| 組み込みブラウザ/ライブプレビュー | 無い。プレビューは Markdown/SVG/CSV/画像のみで、HTML のライブプレビューや簡易ブラウザは無い | preview 系 crate は markdown_preview, svg_preview, csv_preview, image_viewer のみ |
| Git 以外の SCM（SVN, Mercurial, Perforce） | 無い。VCS 層は Git 専用 | `git` crate のみ、SCM 抽象なし |
| Remote Tunnels / Codespaces 相当 | 無い。リモートは SSH/WSL/Dev Container の直接接続のみで、中継サービス経由のトンネルは無い | remote_connection の実装と docs/src/remote-development.md |
| Web 版エディタ | 製品としては無い。`gpui_web` バックエンドが入り基盤は動き始めているが、vscode.dev のような提供形態は無い | crates/gpui_web の存在（実験段階とみられる） |
| ブックマーク | 無い。vim モードのマークが実質の代替 | 該当アクション/裏付け crate なし |
| プロファイル（VSCode の Profiles 相当） | 部分的。設定プロファイル切替（`profiles`）はあるが、拡張や UI 状態まで丸ごと切り替えるものではない | settings_profile_selector の実装範囲 |
| アクセシビリティ（スクリーンリーダー） | 初期段階。AccessKit の統合は始まっているが、DOM ベースの VSCode との差は大きいとみられる | Cargo.lock に accesskit / accesskit_atspi_common |
| 印刷 | 無い | 該当なし |

要するに Zed が捨てているのは「拡張に何でもやらせる」路線で、その代わりに Git/ターミナル/デバッガ/AI をすべて本体に焼き込んでいる。自作するなら、この「拡張 API を最小に保ち、機能は本体に持つ」判断が最も真似しやすい部分だと思う。逆に notebooks や problem matcher のように VSCode の拡張エコシステムが担っている機能は、Zed 方式では 1 個ずつ自前実装になる。

## crate 全対応表

全 237 crate。レイヤ番号は上の「機能一覧」の節番号（1 コア編集 / 2 描画・UI / 3 ナビゲーション / 4 言語知能 / 5 検索・置換 / 6 VCS / 7 ターミナル / 8 デバッグ / 9 拡張 / 10 設定・キーマップ / 11 コラボ / 12 AI / 13 その他）。

| crate | レイヤ | 説明 |
|---|---|---|
| acp_thread | 12 | ACP（Agent Client Protocol）スレッドの共有モデル。エントリ、差分、ターミナル、メンション、サンドボックス許可 |
| acp_tools | 12 | ACP 通信ログビューア（開発用） |
| action_log | 12 | エージェントによるバッファ編集の追跡と accept/reject/undo 管理 |
| activity_indicator | 2 | ステータスバーの動作表示（LSP のダウンロード/起動状況等） |
| agent | 12 | ネイティブエージェント本体。スレッド、ツール群、権限、サンドボックス統合 |
| agent_servers | 12 | 外部 ACP エージェントの起動と管理（Claude Code, Codex, Gemini CLI, カスタム, レジストリ） |
| agent_settings | 12 | エージェント設定の型定義 |
| agent_skills | 12 | SKILL.md 形式のスキル読み込みとスコープ管理 |
| agent_ui | 12 | エージェントパネル、インラインアシスタント、編集レビュー（AgentDiff）等の UI |
| ai_onboarding | 12 | AI 機能のオンボーディング UI（プラン案内、API キー設定カード） |
| anthropic | 12 | Anthropic API クライアント |
| askpass | 6 | Git/SSH の認証プロンプト仲介（askpass 実装） |
| assets | 13 | 埋め込みアセット（フォント、アイコン、デフォルト設定/キーマップ） |
| audio | 11 | 通話用音声パイプライン |
| auto_update | 13 | 自動アップデート本体 |
| auto_update_helper | 13 | Windows 用アップデート補助バイナリ |
| auto_update_ui | 13 | アップデート関連 UI |
| aws_http_client | 13 | AWS 署名対応 HTTP クライアント（Bedrock 用） |
| bedrock | 12 | AWS Bedrock クライアント |
| benchmarks | 13 | ベンチマークハーネス |
| breadcrumbs | 3 | ツールバーのパンくず（パス + シンボル階層） |
| buffer_diff | 6 | バッファと HEAD/index 間の差分状態管理 |
| call | 11 | 通話/ルーム管理、画面共有 |
| channel | 11 | チャンネル、チャンネルノート（共有バッファ）、チャットストア |
| cli | 13 | `zed` コマンドラインバイナリ |
| client | 11 | zed.dev 接続クライアント（認証、RPC、テレメトリ送信） |
| clock | 1 | Lamport/ベクタクロック（CRDT の基盤） |
| cloud_api_client | 12 | Zed クラウド API クライアント（アカウント、課金、LLM） |
| cloud_api_types | 12 | Zed クラウド API の型定義 |
| cloud_llm_client | 12 | Zed ホスト LLM への通信クライアント |
| codestral | 12 | Mistral Codestral クライアント（FIM edit prediction プロバイダ） |
| collab | 11 | コラボサーバ本体（API、認証、DB、RPC。k8s マニフェスト同梱、GPL） |
| collab_ui | 11 | コラボパネル、チャット、通知パネル等の UI |
| collections | 13 | 標準コレクション型の再エクスポート |
| command_palette | 3 | コマンドパレット |
| command_palette_hooks | 3 | パレットへのフック（vim 等によるアクション制御） |
| component | 2 | UI コンポーネント登録基盤（プレビュー用メタデータ） |
| component_preview | 2 | コンポーネントギャラリー（storybook 相当、開発用） |
| context_server | 12 | MCP（Model Context Protocol）サーバ接続 |
| copilot | 12 | GitHub Copilot 連携（サインイン、補完系） |
| copilot_chat | 12 | Copilot Chat の LLM プロバイダ化 |
| copilot_ui | 12 | Copilot 関連 UI |
| crashes | 13 | クラッシュハンドラ（minidump 生成/送信） |
| credentials_provider | 13 | 資格情報保存の抽象（キーチェーン） |
| csv_preview | 2 | CSV テーブルプレビュー |
| dap | 8 | DAP クライアント（セッション、トランスポート、レジストリ） |
| dap_adapters | 8 | 組み込みデバッグアダプタ（CodeLLDB, GDB, Delve, debugpy, JS） |
| db | 13 | SQLite ベースの永続化（ワークスペース状態） |
| debug_adapter_extension | 9 | 拡張提供デバッグアダプタの橋渡し |
| debugger_tools | 8 | DAP 通信ログビューア |
| debugger_ui | 8 | デバッガ UI（パネル、ブレークポイント一覧、変数、コンソール） |
| deepseek | 12 | DeepSeek API クライアント |
| dev_container | 13 | Dev Containers（devcontainer.json）対応 |
| diagnostics | 4 | プロジェクト診断ビュー（マルチバッファ） |
| docs_preprocessor | 13 | 公式ドキュメント（mdBook）の前処理ツール |
| edit_prediction | 12 | edit prediction 本体（Zeta/Copilot/Codestral/Mercury/Ollama/OpenAI 互換の統合、FIM、データ収集） |
| edit_prediction_cli | 12 | 予測品質評価用 CLI（開発用） |
| edit_prediction_context | 12 | 予測用コンテキスト収集（BM25、git log、編集可能領域の組み立て） |
| edit_prediction_metrics | 12 | 予測品質メトリクス計算（開発用） |
| edit_prediction_types | 12 | edit prediction の型定義 |
| edit_prediction_ui | 12 | 予測のステータスバーボタン、レート付けモーダル等 |
| editor | 1 | エディタ本体。表示、カーソル、選択、編集操作、LSP UI 統合の中心 |
| editor_benchmarks | 13 | editor のベンチマーク |
| encoding_selector | 1 | 文字エンコーディング選択 UI |
| env_var | 13 | 環境変数の型（UI 編集用） |
| etw_tracing | 13 | Windows ETW トレーシング |
| eval_cli | 12 | エージェント評価用ヘッドレス CLI |
| eval_utils | 12 | 評価ユーティリティ |
| explorer_command_injector | 13 | Windows エクスプローラ右クリックメニュー（COM DLL） |
| extension | 9 | 拡張の共通型（マニフェスト等） |
| extension_api | 9 | Rust 製 WASM 拡張の公開 API（WIT） |
| extension_cli | 9 | 拡張パッケージング CLI |
| extension_host | 9 | WASM 実行ホスト（wasmtime）と拡張ストア連携 |
| extensions_ui | 9 | 拡張のブラウズ/インストール UI |
| feature_flags | 13 | フィーチャーフラグ（サーバ駆動） |
| feature_flags_macros | 13 | feature_flags 用マクロ |
| feedback | 13 | フィードバック送信 |
| file_finder | 3 | ファジーファイル検索モーダル |
| file_icons | 2 | ファイル種別→アイコンのマッピング（アイコンテーマ） |
| fs | 13 | ファイルシステム抽象（実 FS/FakeFs、監視、ゴミ箱） |
| fs_benchmarks | 13 | fs のベンチマーク |
| fuzzy | 3 | ファジーマッチングエンジン（自前実装） |
| fuzzy_nucleo | 3 | nucleo ベースの代替ファジーマッチャ |
| git | 6 | Git 基盤（ステータス、リポジトリ操作、blame、hunk） |
| git_hosting_providers | 6 | ホスティング連携（GitHub/GitLab/Bitbucket/Gitea/Forgejo/Gitee/SourceHut/Azure/Chromium） |
| git_ui | 6 | Git パネル、プロジェクト差分、コミット、ブランチ/スタッシュ/グラフ UI |
| go_to_line | 3 | 行:列ジャンプモーダル |
| google_ai | 12 | Google AI（Gemini）クライアント |
| gpui | 2 | GPU アクセラレーテッド UI フレームワーク本体 |
| gpui_linux | 2 | GPUI の Linux バックエンド（Wayland/X11） |
| gpui_macos | 2 | GPUI の macOS バックエンド（Metal） |
| gpui_macros | 2 | GPUI 用 derive/attribute マクロ |
| gpui_platform | 2 | プラットフォーム抽象層の分離 crate（バックエンド選択） |
| gpui_shared_string | 2 | SharedString 型 |
| gpui_tokio | 2 | GPUI への tokio 統合 |
| gpui_util | 2 | GPUI 向け小型ユーティリティ（ArcCow 等） |
| gpui_web | 2 | GPUI の Web（ブラウザ）バックエンド。実験段階とみられる |
| gpui_wgpu | 2 | wgpu レンダラ（cosmic-text によるテキスト描画） |
| gpui_windows | 2 | GPUI の Windows バックエンド（DirectX） |
| grammars | 4 | 同梱 tree-sitter 文法（rust/ts/tsx/js/python/go/c/cpp/css/json/yaml/markdown/diff/gitcommit/regex ほか） |
| html_to_markdown | 12 | HTML→Markdown 変換（fetch ツール等で使用） |
| http_client | 13 | HTTP クライアント抽象 |
| http_client_tls | 13 | HTTP クライアントの TLS 設定 |
| http_proxy | 12 | サンドボックス用のホスト許可リスト付き HTTP/HTTPS プロキシ |
| icons | 2 | アイコン名 enum（IconName） |
| image_viewer | 2 | 画像ビューア |
| input_latency_ui | 2 | 入力レイテンシのヒストグラム表示（開発用） |
| inspector_ui | 2 | GPUI 要素インスペクタ（開発用） |
| install_cli | 13 | `zed` CLI のインストール（PATH 登録） |
| journal | 13 | ジャーナル（日次 Markdown ノート） |
| json_schema_store | 10 | 設定/キーマップ等の JSON スキーマ提供 |
| keymap_editor | 10 | GUI キーマップエディタ（キーストローク録音付き） |
| language | 4 | 言語基盤（バッファの言語層、tree-sitter 統合、言語レジストリ） |
| language_core | 4 | 言語のコア型（言語設定、文法、LSP アダプタ型、ツールチェイン） |
| language_extension | 9 | 拡張提供言語の橋渡し |
| language_model | 12 | LLM 抽象とレジストリ |
| language_model_core | 12 | LLM のコア型（リクエスト、ロール、ツールスキーマ、レート制限） |
| language_models | 12 | 全 LLM プロバイダの登録と設定（provider/ に 19 実装） |
| language_models_cloud | 12 | Zed ホストモデルのプロバイダ |
| language_onboarding | 4 | 言語別オンボーディング（現状 Python） |
| language_selector | 4 | 言語モード切替モーダル |
| language_tools | 4 | LSP ログビューア、シンタックスツリービュー（開発補助） |
| languages | 4 | 組み込み言語定義 + LSP アダプタ（Rust/C/C++/Python/TS/JS/Go/JSON/YAML/CSS/Bash/Tailwind 等） |
| line_ending_selector | 1 | 改行コード（LF/CRLF）切替 UI |
| livekit_api | 11 | LiveKit サーバ API SDK |
| livekit_client | 11 | LiveKit クライアント統合（GPUI 向け） |
| llama_cpp | 12 | llama.cpp サーバクライアント（ローカルモデル） |
| lmstudio | 12 | LM Studio クライアント |
| lsp | 4 | LSP クライアント（JSON-RPC、プロセス管理、プロトコル型） |
| markdown | 2 | Markdown レンダリング要素（ホバー、エージェント UI 等で使用） |
| markdown_preview | 2 | Markdown プレビュー |
| media | 11 | macOS メディア API バインディング（画面共有映像） |
| menu | 2 | 共通メニューアクション（Confirm/Cancel/SelectNext 等） |
| mermaid_render | 2 | Mermaid 図の SVG レンダリング |
| migrator | 10 | 設定/キーマップの自動マイグレーション |
| miniprofiler_ui | 2 | アプリ内フレームプロファイラ（開発用） |
| mistral | 12 | Mistral API クライアント |
| multi_buffer | 1 | マルチバッファ（複数バッファの断片を 1 バッファに合成） |
| net | 13 | ソケット/名前付きパイプ抽象（CLI↔アプリの IPC 等） |
| node_runtime | 4 | Node.js ランタイム管理（LSP/Prettier 用の自動インストール） |
| notifications | 11 | 通知ストア（コラボ通知）とステータストースト |
| oauth_callback_server | 13 | サインイン用ループバック OAuth 2.0 サーバ |
| ollama | 12 | Ollama クライアント |
| onboarding | 13 | 初回オンボーディング（ようこそ画面、ベースキーマップ/テーマ選択） |
| open_ai | 12 | OpenAI API クライアント |
| open_path_prompt | 3 | アプリ内パス入力プロンプト（OS ダイアログ代替） |
| open_router | 12 | OpenRouter クライアント |
| opencode | 12 | OpenCode（Zen API）連携 |
| outline | 3 | シンボルアウトラインモーダル（tree-sitter ベース） |
| outline_panel | 3 | アウトラインパネル（マルチバッファの目次にもなる） |
| panel | 2 | ドックパネルの共通部品 |
| paths | 13 | Zed が使う標準パスの定義 |
| picker | 2 | ピッカー（モーダル型ファジーリスト）基盤 |
| picker_preview | 2 | ピッカーのプレビュー（開発用） |
| platform_title_bar | 2 | プラットフォーム別タイトルバー実装（システムウィンドウタブ対応） |
| prettier | 4 | Prettier 統合フォーマッタ |
| project | 13 | プロジェクト中枢モデル（バッファ/LSP/Git/タスク/設定の束ね役、リモート透過） |
| project_benchmarks | 13 | project のベンチマーク |
| project_panel | 3 | ファイルツリーパネル |
| project_symbols | 3 | ワークスペースシンボル検索（LSP） |
| prompt_store | 12 | プロンプト/ルールライブラリの保存（rules→skills 移行含む） |
| proto | 11 | Zed アプリと zed.dev サーバ間のプロトコル定義（protobuf） |
| recent_projects | 3 | 最近のプロジェクト/リモート接続ピッカー |
| refineable | 13 | カスケード設定向けの refinement 型 derive マクロ |
| release_channel | 13 | リリースチャンネル（Stable/Preview/Nightly/Dev） |
| remote | 13 | リモート編集のクライアント側サブシステム |
| remote_connection | 13 | SSH/WSL 接続処理 |
| remote_server | 13 | リモート先で常駐するヘッドレスデーモン |
| repl | 13 | REPL/Jupyter カーネル統合（インライン出力、ノートブック UI 実装中） |
| reqwest_client | 13 | reqwest 実装の HTTP クライアント |
| rope | 1 | ロープ（sum_tree 上のテキスト構造） |
| rpc | 11 | RPC ピア実装（コラボ/リモート共用） |
| sandbox | 12 | エージェント実行コマンドのクロスプラットフォームサンドボックス |
| scheduler | 2 | 実行スケジューラ抽象（executor、テスト用決定的スケジューラ） |
| schema_generator | 10 | 設定 JSON スキーマの生成ツール |
| search | 5 | バッファ検索/プロジェクト検索と検索バー UI |
| session | 13 | アプリセッション管理（再起動/クラッシュ後のウィンドウ復元） |
| settings | 10 | 設定システム（階層マージ、ファイル監視） |
| settings_content | 10 | 設定内容の型定義 |
| settings_json | 10 | 設定 JSON の編集操作（キー書き換え等） |
| settings_macros | 10 | 設定用マクロ |
| settings_profile_selector | 10 | 設定プロファイル切替 |
| settings_ui | 10 | GUI 設定ウィンドウ |
| shell_command_parser | 12 | シェルコマンド解析（エージェントの権限判定用） |
| sidebar | 12 | エージェントスレッドのサイドバー/スレッド切替 |
| snippet | 1 | スニペット本体（tabstop 解析） |
| snippet_provider | 1 | スニペットファイルの読み込み（拡張提供含む） |
| snippets_ui | 1 | スニペット管理 UI |
| sqlez | 13 | SQLite ラッパ |
| sqlez_macros | 13 | sqlez 用マクロ |
| streaming_diff | 12 | LLM 出力のストリーミング差分適用（インラインアシスト用） |
| sum_tree | 1 | Sum tree（サマリ付き並行フレンドリー B+ 木） |
| svg_preview | 2 | SVG プレビュー |
| syntax_theme | 2 | シンタックスハイライトテーマの型 |
| system_specs | 13 | システム情報収集（フィードバック/クラッシュ用） |
| tab_switcher | 3 | タブ切替モーダル（MRU 順） |
| task | 7 | タスク定義（テンプレート、変数展開） |
| tasks_ui | 7 | タスク実行 UI（モーダル、再実行、runnables 連携） |
| telemetry | 13 | テレメトリ送信 |
| telemetry_events | 13 | テレメトリイベントの型定義 |
| terminal | 7 | ターミナルエミュレーション（alacritty_terminal ベース） |
| terminal_view | 7 | ターミナル UI（パネル/センタータブ、検索、vi モード） |
| text | 1 | CRDT テキストバッファ（操作、アンカー、undo 履歴、選択） |
| theme | 2 | テーマシステム本体 |
| theme_extension | 9 | 拡張提供テーマの橋渡し |
| theme_importer | 2 | VSCode テーマのインポートツール |
| theme_selector | 2 | テーマ切替モーダル |
| theme_settings | 2 | テーマ/フォント設定の解決 |
| time_format | 13 | 日時フォーマット |
| title_bar | 2 | タイトルバー（プロジェクト/コラボ表示統合） |
| toolchain_selector | 4 | ツールチェイン選択（Python venv 等） |
| ui | 2 | UI コンポーネントライブラリ |
| ui_input | 2 | フォーム系入力コンポーネント |
| ui_macros | 2 | UI 用マクロ |
| ui_prompt | 2 | アプリ内モーダルプロンプト（OS ダイアログ代替） |
| util | 13 | 汎用ユーティリティ |
| util_macros | 13 | ユーティリティマクロ |
| vim | 1 | Vim/Helix エミュレーション |
| vim_mode_setting | 1 | vim/helix モード設定（依存切り離し用の小 crate） |
| watch | 13 | 単一値の watch チャンネル |
| web_search | 12 | Web 検索の抽象 |
| web_search_providers | 12 | Web 検索プロバイダ実装 |
| which_key | 10 | which-key（キー入力途中の候補ポップアップ） |
| windows_resources | 13 | Windows リソース（アイコン、マニフェスト）埋め込み |
| workspace | 2 | ワークスペースシェル（ペイン、ドック、タブ、永続化） |
| worktree | 13 | ワークツリー（ディレクトリ走査、ignore、Git 検出） |
| worktree_benchmarks | 13 | worktree のベンチマーク |
| x_ai | 12 | xAI（Grok）クライアント |
| zed | 13 | メインバイナリ。全機能の初期化と結線、メニュー、URL スキーム |
| zed_actions | 13 | crate 横断で共有するアクション定義（循環依存回避） |
| zed_credentials_provider | 13 | 資格情報プロバイダの Zed 実装 |
| zed_env_vars | 13 | Zed 関連の環境変数定義 |
| zeta_prompt | 12 | Zeta（edit prediction モデル）のプロンプト構築 |
| zlog | 13 | ロガー |
| zlog_settings | 13 | ログ設定 |
| ztracing | 13 | トレーシング基盤 |
| ztracing_macro | 13 | ztracing 用マクロ |

（内訳: レイヤ1 = 13、レイヤ2 = 46、レイヤ3 = 13、レイヤ4 = 16、レイヤ5 = 1、レイヤ6 = 6、レイヤ7 = 4、レイヤ8 = 5、レイヤ9 = 9、レイヤ10 = 12、レイヤ11 = 14、レイヤ12 = 52、レイヤ13 = 46。合計 237）
