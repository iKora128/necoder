# ROADMAP — 受入条件つきマイルストーン

運用: `/goal` はこのファイルの**未チェックの受入条件**を上から拾って実装する。
チェックは「実際に満たすことを検証してから」入れる（CLAUDE.md の検証ループ）。順序の正はこのファイル、機能の全量は FEATURES.md。

## M0 — 週末テスト（ユーザー自身の関門）

- [x] 環境: Metal Toolchain 導入（2026-07-11、`xcodebuild -downloadComponent MetalToolchain`）
- [x] `cd zed && cargo run -p gpui --example hello_world` で窓が出る — **2026-07-11 通過**（3m28s ビルド後に hello_world 表示。daichi さん自身で確認）

## M1 — 骨組み ✓（2026-07-11）

- [x] workspace スキャフォールド（crates/shirushi・rust-toolchain 1.95・AGPL-3.0 LICENSE・CLAUDE.md）
- [x] `cargo check -p shirushi` 通過
- [ ] `cargo run -p shirushi` で骨組み窓が出ることをユーザーが確認

## M2 — エディタコア（「テキストが編集できる」）

- [x] `theme_core`: UI-SPEC §1 のトークンを型と dark テーマ値で実装（light は値だけ用意）— 2026-07-11、`crates/theme_core`、unit test 9 本 green
- [x] `i18n`: `t!` 規律開始・`locales/ja.yml`+`en.yml` — 2026-07-11、`crates/i18n`（rust-i18n はマクロが crate 局所で多 crate に不適 → 薄い自作に。ARCHITECTURE §6 更新済み）、ja/en parity テスト付き
- [x] `editor_core`: Buffer(ropey 2.0・byte 索引)・Selection・edit/undo/redo（Transaction 単位）・snapshot — 2026-07-11、`crates/editor_core`、GPUI 非依存・unit test 18 本 green
- [x] `editor_view`: 行仮想化描画・スクロール・行番号ガター・キャレット描画 — 2026-07-11、`crates/editor_view`（custom Element・`shape_line`・`uniform_list` 相当の仮想化）。1.6MB 開いても idle CPU 0%（＝可視行のみ shape）。目視: font-kit 有効化後は offscreen（`render_to_image`）でもグリフが写り自己検証可能に。ユーザーが実機で描画確認済み（2026-07-12）
- [x] キー入力で挿入/削除/改行、⌘Z/⌘⇧Z、⌘S 保存、ファイルは起動引数で開く — 2026-07-11、`crates/shirushi` に配線（`EntityInputHandler` 経由の入力・アクション/キーマップ）。smoke（多言語+絵文字+1.6MB）で無クラッシュ
- [ ] **IME**: 日本語入力（変換中表示・確定）が正しく動く — 受入: この README を Shirushi で開いて日本語の1段落を書ける ← **コードは実装済み**（`replace_and_mark_text_in_range` 等・UTF-16↔byte）。**IME 対話検証が未**（人が変換入力する必要・headless 不可）
- [ ] 受入（総合）: `shirushi <file>` で開く→編集→undo→保存 が一往復できる。1MB のファイルでスクロールがもたつかない ← ロジックは全て test 済・1.6MB は idle 0% だが、**対話/目視の一往復確認が未**
- [x] 性能ベンチ導入（一部）: 起動時間計測 `scripts/startup-time.sh`（debug 実測 ~100ms cold→first render）+ editor_core スループット bench（insert 1.5M/s・1MB上 edit 40万cycle/s）。**`input_latency_ui` の key→frame ヒストグラム移植は後回し**（要 UI・時期尚早／JOURNAL 参照）

## M3 — ワークスペースと方向感覚（「色で迷わなくなる」）

- [x] `settings_core`: 3層マージ（default→user→`.shirushi/`）— 2026-07-12、`crates/settings_core`、深い JSON マージ・型付き Settings、6 test。保存即反映（監視）は後続
- [x] テーマきせかえ: light 値の実装 + テーマセレクタ（Picker 上・ライブプレビュー）+ ユーザーテーマ JSON 読み込み — 2026-07-13。`theme_core` に**ユーザーテーマ JSON（トークン上書き・欠けは appearance の組み込みへフォールバック）**・`available_themes`/`resolve` を実装（unit test）。`⌘⇧T` でテーマセレクタ（Picker）＝**ハイライト移動で即ライブプレビュー**（`PickerEvent::Highlighted`）・確定で settings.json へ theme 名保存（再起動でも効く）・中止でプレビューを戻す。`apply_theme` がクローム/エディタ/Agent パネル/Picker へ波及。light 全体反映を offscreen で目視確認
- [x] `keymap_core`: JSON keymap + コンテキスト述語 — 2026-07-12、`crates/keymap_core`、`build_action` で名前解決・`KeyBinding::load`、3 test。既定 keymap を bin で読込（全アクション解決確認）
- [x] `ui`: Picker 基盤（1個のファジーリストを全モーダルで使い回す）— 2026-07-12、`crates/ui`、fuzzy・キー操作・イベント通知、2 test。**CommandRegistry/StatusItemRegistry は未**
- [x] `workspace`: タブ・単一ペイン→分割・左右下ドック・statusbar — titlebar ✓・タブ列 ✓・パンくず ✓・左ドック(explorer) ✓・右ドック(Agent) ✓・statusbar ✓（2026-07-12）。**分割ペイン**（`⌘\` で右分割・独立エディタの比較ビュー・各ペインにタブ+×）✓・**下ドック**（統合ターミナル・`⌘J`）✓（2026-07-13）
- [x] `project`: worktree 走査・gitignore — 2026-07-12、`crates/project`、`ignore` crate で gitignore 準拠の遅延 read_dir + all_files、4 test。**ファイル監視は未**
- [x] エクスプローラ（ツリーのみ先行）+ ファイル開く — 2026-07-12、workspace の左ドック。クリックで展開/ファイル open
- [x] **レール**: プロジェクト登録・切替（窓内）・プロジェクト色（自動巡回）— 2026-07-12。**カスタムアイコン / `.shirushi` 色指定は未**（頭文字 + 巡回色）
- [x] **⌘O スイッチャー** + ⌘1..9 + **新窓** — 2026-07-13。⌘O プロジェクトスイッチャー ✓（Picker）。⌘1..9 でレールのプロジェクト N 番へ切替（`ActivateProject1..9` アクション）。**新窓は ⌘⇧N**（当初案 ⌘⏎ は composer の `agent::SubmitPrompt` と衝突するため慣例的な ⌘⇧N に変更。Editor コンテキストの ⌘⏎ 送信は温存）＝アクティブプロジェクトを新ウィンドウで開く（右クリック「新規ウィンドウで開く」と同じ経路）
- [x] 状態永続化 — 2026-07-12、`state.json` に (プロジェクト群・アクティブ・開ファイル) を保存、引数無し起動で復元
- [x] ファイルファインダ ⌘P — 2026-07-12（Picker + `all_files`）。**コマンドパレット ⌘⇧P は未**（要 CommandRegistry）
- [x] 受入: **2プロジェクトをレールで色区別 + 再起動で状態復元** — 2026-07-12 達成（offscreen で 2 色レール目視・state.json 往復確認）

## M4 — ACP（「Claude Code が中で動く」）

- [x] `acp_client` 接続・initialize・prompt・**ストリーミング** — 2026-07-12、`crates/acp_client`。claude-agent-acp を子プロセス起動 + `blocking::Unblock` パイプ + `ByteStreams` + `Client.builder().connect_with`。バイナリ探索を **PATH → Zed の npx キャッシュ(.bin) → `npx @agentclientprotocol/claude-agent-acp@0.58.1`** に強化（単体は PATH に無いのが普通・これが `live_prompt` の当初失敗要因だった）。`prompt_once`（一括）に加え **`run_session`（永続セッション + `read_update` ループで `AgentMessageChunk`/`AgentThoughtChunk` を [`AgentEvent`] に簡約し逐次流す）** を実装。`ToolCall` も [`AgentEvent::ToolStarted`] にマップ。**エージェント自身が実機検証済み**: `live_prompt`（「1+1」→「2」5.7s）・`live_stream`（「3の倍数5個」→`3, 6, 9, 12, 15` を逐次）・ツール誘発プローブ（「ls して」→ ⏺ Terminal ステップ + 応答）。**権限リクエスト・`UsageUpdate` でトークン実値・markdown 描画は継続**
- [x] `agent_panel`: スレッド色タブ・メタ行（モデル/トークン常時メーター）・transcript（⏺/⎿・✳ Thinking 常時展開）・composer（宛先チップ・⌘Enter 送信・IME 安全）— 2026-07-12、`crates/agent_panel`（右ドックまるごと）。composer は editor_view 平坦モード再利用で IME/undo 共通。⌘Enter→`agent::SubmitPrompt`→**スレッド毎の常駐セッション**（`run_session` をバックグラウンド + `cx.spawn` フォアグラウンドで `AgentEvent` を逐次 transcript 反映）。**実機フルパス検証済み**: 起動プローブ（`SHIRUSHI_ACP_PROBE`）で composer→claude-agent-acp→ストリーミング→描画を offscreen スクショで確認（「1+1は？」→「2」がスレッド色付きで表示）。transcript の初期内容は mock 会話例（デモ・実送信で本物が積まれる）
- [x] 権限リクエスト UI（許可/拒否）・エージェント編集の diff レビュー（accept/reject）— 2026-07-12、`crates/acp_client`+`agent_panel`。`session/request_permission` を read ループの **`.if_request`** で受け、ハンドラ内 await でユーザー決定待ち（ターンブロックが正）。`ToolCallContent::Diff` を `compact_line_diff` で緑（追加=`theme.ok`）/赤（削除=`theme.err`）表示、composer 直上に承認カード（許可/常に許可/拒否）。**live: Write プローブで diff カード表示 + `SHIRUSHI_AUTO_ALLOW` で実ファイル生成まで round-trip 確認**
- [x] スレッド色貫通（UI-SPEC §6）+ titlebar beacon + statusbar ドット ← 色貫通 ✓（タブ切替で下線/トークンバー/msg-user 左縁/宛先ドット/composer 枠/送信ボタンが一斉切替・実機スクショで確認）。titlebar beacon ✓（実行中スレッドが窓上部に色ドット+名で常時表示＝BACKGROUND の原点痛点）。**statusbar スレッドドットは未**
- [x] 受入: **Claude Code サブスクで会話→ファイル編集を diff で確認→accept、を Shirushi 内で完結。トークン使用量が常に見えている** — 2026-07-12 達成。会話ストリーミング ✓・トークン実値メーター ✓・**ファイル編集の diff 表示→承認→実行（accept）を live で完結**（round-trip でファイル生成確認）。加えて **model/effort/権限モードの ACP 実反映**（session config options / set_mode）・**Enter 送信の設定化**（IME 誤送信対策）も同日実装

## M5 — エクスプローラ完成（Finder 感）

- [x] カラム / アイコンビュー + 左下ビュー切替 — 2026-07-12、`ExplorerView{Tree,Columns,Icons}` + `render_explorer_footer` の ☰▥▦。拡張子別アイコン（フォルダ横長/ファイル縦長・型色）。カラムは Miller columns（末尾3段・幅可変）
- [x] 上位階層ブレッドクラム — 2026-07-12、`render_explorer_header` のクリック可能パンくず（プロジェクト内 up-nav）。**プロジェクト外へ出るブラウズは未**（root 上へは行かない）
- [x] 右クリックメニュー（新しいウィンドウでプロジェクトとして開く 含む）— 2026-07-12、全3表示のエントリに右クリック。フォルダ=新規ウィンドウで開く/開く/コピー、ファイル=開く/コピー。**ファイル操作 undo は未**
- [x] エクスプローラ幅可変（右縁ドラッグ）— 2026-07-12
- [x] 受入: マウスだけで「上へ辿る→隣のリポジトリを新窓でプロジェクトとして開く」— 2026-07-13。ブレッドクラムの **⤴（上へ）** で current の親へ辿れ、ルート直上へ出ると Finder カラム表示へ自動切替（`project::Worktree::read_any_dir` がルート外を gitignore 無しで列挙）。ルート外では **⌂プロジェクト（戻る）** + 末尾数段のパンくずを出す。隣リポジトリを右クリック→新規ウィンドウで開く（既存）。offscreen で `~/Work/experience` の隣接 repo 一覧を目視確認

## M6 — 検索

- [x] バッファ内検索（regex/literal・大小トグル）— 2026-07-12、`crates/search` の `search_text`、7 test。**置換・インクリメンタル UI は未**
- [x] プロジェクト横断検索 — 2026-07-12、`search_files`（`ignore` の走査結果を渡してインプロセス検索。ripgrep 子プロセスは未採用）
- [x] 受入: `TODO` をプロジェクト横断で列挙→ジャンプ — 2026-07-13。`⌘⇧F` で検索パネル（オーバーレイ）＝クエリ + 大小/正規表現トグル + **ファイル別グルーピング結果**（行番号 + マッチ強調プレビュー）。クリック / Enter で**該当ファイルを開き対象行を中央へジャンプ**（`EditorView::reveal_position` = viewport 未確定でも効く pending_reveal・one-shot で idle 0%）。↑↓ 選択・Esc 閉じる。offscreen で「fn」440 件のファイル別結果を目視確認

## M7 — 言語知能

- [x] tree-sitter ハイライト（Rust）— 2026-07-12、`crates/lang`（tree-sitter 0.25 + tree-sitter-rust）。`HighlightKind`→theme の syn-* に接続、editor_view で行ごとに色 run 生成（編集で再解析・512KB 超はスキップ）。3 test。実 .rs を無クラッシュで開ける。**色の目視は要実機**（offscreen はグリフ非表示）。インクリメンタル解析は後続
- [x] LSP: rust-analyzer（**診断・補完・定義ジャンプ**。hover/format は後続）— 2026-07-13。`lang::lsp` に最小 LSP クライアント（JSON-RPC 封筒は自前・型は手書き・transport は std::process + 読取スレッド + futures channel）。initialize→initialized→didOpen→didChange の lifecycle。①**診断**=publishDiagnostics を gutter 下線（error=赤/warn=琥珀）+ statusbar 件数（✗N ▲N）にライブ反映（live 確認）。②**補完**（Ctrl-Space）=キャレット直下ポップアップ（種別バッジ+detail・上下/Enter・Tab/Esc・textEdit→insertText→label で挿入・識別子プレフィクス置換）。③**定義ジャンプ**（F12）=Location/LocationLink を解析→別ファイルなら開いて中央へ。位置は byte↔UTF-16 変換。`~/.rustup/toolchains/*/bin/rust-analyzer` 実体を起動。parser の unit test + 実 ra の handshake/診断 ignored test。offscreen で診断赤下線・補完ポップアップを目視
- [x] 受入: この repo の Rust を補完と診断つきで書く — 2026-07-13 概ね達成。ハイライト ✓・診断 ✓（live）・補完 ✓（ポップアップ+挿入）・定義ジャンプ ✓。**補完/定義の live round-trip 体感は人の手番**（transport・parser・UI は各々検証済み）

## M8 — Git とターミナル、ブランチ横断の完成

- [x] git status（ツリー/タブ色）+ gutter diff — 2026-07-13。`project` に **git CLI 直叩き**の `git_status`（porcelain→5分類）+ `buffer_diff`（`git show HEAD:./name` + **imara-diff** Histogram・純Rust）。ツリー行/タブ名を状態色（追加=緑/変更=琥珀/削除=赤）+ 末尾バッジ（M/A/U/D/!）、フォルダは配下変更で ●。gutter は追加/変更/削除の左端バー（editor_view・**バッファ version 変化を prepaint で検知→250msデバウンス→背景スレッドで git**＝idle 0%）。offscreen で目視（Cargo.toml=M・gutter 緑バー）
- [x] branch/worktree メニュー（切替・worktree 作成）→ スレッドの (project, branch) 帰属 — 2026-07-13。titlebar の ⎇ クリックで開く（`project::git_branches`/`git_worktrees`/`switch_branch`/`add_worktree`）。ブランチ行クリック=in-place 切替（git switch→再読込）、**⧉=worktree として新しい窓で開く**（既存 worktree 優先・無ければ `<repo親>/<repo名>-<branch>` に作成）＝**並行ブランチ×別窓×スレッド色**の入口。worktree 一覧も別窓で開ける。宛先チップの (project, branch) は切替で追従。**⌘O 2階層化は未**（⎇ 直開きで代替）
- [x] 統合ターミナル（alacritty_terminal）— 2026-07-13。新 crate `terminal_view`（crates.io `alacritty_terminal` 0.26）。**EventLoop が読取+vte parser**、idle 0% は出力時のみ Wakeup→pump(`cx.spawn`)→sync→notify（**タイマー無し**）。下ドックに配置（`⌘J` / 下ドックボタン開閉・cwd=プロジェクトルート）。custom Element でグリッド描画（16/256色 ANSI + truecolor・INVERSE/BOLD/ITALIC・ブロックカーソル）。入力は `on_key_down` 一本化（印字 + 特殊キー→エスケープ・矢印は APP_CURSOR 対応）。offscreen で実シェル（`Last login… / プロンプト / カーソル`）を目視。**パスリンクは未**（後続）
- [x] 受入: **worktree で並行ブランチを開き、それぞれ別スレッドのエージェントを走らせ、色で追える**（＝当初ビジョンの完成形）— 2026-07-13。機構が揃った: ⎇メニューの ⧉ で **ブランチ→worktree→新ウィンドウ**、各窓は自分の (project, branch) の色付きスレッド + git 色 + プロジェクト色で方向づけ、下ドックに cwd 一致のターミナル。**live のマルチウィンドウ並行運用の体感確認は人の手番**（機構は完成）

---

以降（v1 後半〜later）: FEATURES.md のタグに従う（拡張モデル ADR・minimap・multibuffer・vim・追加言語パック配布 等）。
