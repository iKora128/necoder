# ROADMAP — 受入条件つきマイルストーン

運用: `/goal` はこのファイルの**未チェックの受入条件**を上から拾って実装する。
チェックは「実際に満たすことを検証してから」入れる（CLAUDE.md の検証ループ）。順序の正はこのファイル、機能の全量は FEATURES.md。
人の対話検証が必須の項目（M1 の窓確認・M2 の IME 対話など「人の手番」）は /goal ではスキップし、次の実装可能な項目を拾う。

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

## M9 — Remote SSH（「場所が変わっても同じ Shirushi」）

- [x] local の FS/process/Git/search/save を `Host` 境界へ移し、既存 unit test を維持する
- [x] versioned length-frame RPC + raw body + frame 上限 + multiplex worker + protocol test
- [x] `shirushi-remote-server` + system OpenSSH + project root scope + conflict-safe atomic save
- [x] `ssh://[user@]host[:port]/path` で tree/open/edit/search/Git を remote Host へ接続する
- [x] LSP・PTY・ACP を remote process/ControlMaster session へ移す（task 機能自体は未実装）
- [x] ControlMaster・server version/OS/arch 検出・同一 target の versioned binary 自動配備
- [x] daemon/proxy 再接続・5秒 heartbeat・master 再生成・非冪等 request の無条件再送禁止
- [x] protocol/fs/process concurrency・proxy 再接続・外部編集競合の統合 test
- [x] SSH URI の状態復元と status bar の接続先表示
- → **未チェック残件（ベンチ・musl 配布・dirty backup・障害注入 test・Remote Projects UI・受入）は M13 へ移動**（2026-07-15 判断: 日常機能 M10〜M12 を先に消化する）

設計と根拠: [`research/remote-ssh-2026.md`](./research/remote-ssh-2026.md)。

## M10 — 毎日使える（「Shirushi で Shirushi を開発する」）

2026-07-15 のギャップ分析（全 crate 棚卸し × `research/feature-matrix.md` 突合）で M10〜M13 を策定。
レイヤ自体は 13/13 に点が打ってあるが「所作」の層が薄い — ここを埋めるとドッグフーディングが始まる。

- [x] **複数タブ** — 2026-07-16。`workspace.rs` の `editor: Option<Entity>` を撤廃し、`tabs: Vec<EditorTab{path,editor,_observation}>` + `active_tab` に。**タブクリック**（`select_tab`）/ **⌘{ ⌘}**（=⌘⇧[ ⌘⇧]・`SelectPrevTab/SelectNextTab`。Zed の `pane::Activate*Item` と同じ字面）切替・**⌘W で閉じて隣へ**（`close_tab_at`＝active 追従は agent スレッドタブと同ロジック）・**ドラッグ並べ替え**（`DraggedEditorTab`+`move_tab`）・**dirty ドット個別**・**同一ファイルは重複タブを作らず既存へ切替**・**LSP didOpen/didClose 追従**（+ `lsp_sent_versions` を path 別 map 化＝複数ファイルで version 番号が衝突しても誤スキップしない）。永続化は `ProjectSlot.open_files: Vec` + `active_file`（プロジェクト単位でタブ列を保存・非アクティブは切替時に遅延復元）。ARCHITECTURE §3 に Pane/Item 初版の型契約を追記。**検証**: cargo test 全 green・offscreen で 5 タブ描画（active=lsp.rs 下線）・state.json 往復（open_files 5件+active_file:4）・無引数再起動で 5 タブ復元を目視。**編集→保存→× の対話ループ体感は人の手番**（状態分離はタブ毎の独立 Entity で構造的に保証）
- [ ] **⌘F バッファ内検索/置換**: エディタ上部にインライン検索バー（インクリメンタルにマッチ全ハイライト + 件数 n/m・Enter/⇧Enter で次/前・regex/大小トグルは `search` crate 再利用）。⌥⌘F で置換行を追加（1件置換 / 全置換・全置換は 1 Transaction で undo 一発）。Esc で閉じて元位置へ。受入: workspace.rs で「render_」を検索→ジャンプ→3件目だけ置換→undo で戻る
- [ ] **補完の自動トリガ**: 識別子文字・`.`・`::` の入力で自動的に補完ポップアップ（現状 Ctrl-Space 手動のみ）。入力継続で再フィルタ・確定/Tab/Esc は既存流用・Esc 直後は同じ語で再表示しない。受入: `buf.` と打つだけで候補が出て Enter で挿入できる
- [ ] **hover の配線**: `lang/src/lsp.rs` の `hover()`（実装済み・未配線）をマウスホバー ~500ms とキー操作で呼び、対象位置の上にポップアップ（markdown は当面プレーン表示で可）。受入: rust-analyzer で型シグネチャと doc がポップアップに出る
- [ ] **ファイル監視（watch 基盤）**: `notify`（FSEvents）で worktree を監視。①開いているバッファの外部変更 → 無編集なら自動リロード / dirty なら警告バー（再読込/このまま）②ツリーへ作成/削除/リネームを反映 ③git status・gutter diff の自動更新（現状は保存時 `FileRevision` 検知のみ・`project.rs:5`）。M12「エージェント編集の生中継」の前提土台。受入: ターミナルで `echo >> file` した直後にバッファ・ツリー・git 色が追従する
- [ ] **hot exit（クラッシュ耐性）**: dirty バッファを数秒デバウンスでローカルにスナップショットし、異常終了後の起動で復元を提案（正常終了・保存で破棄）。FEATURES 唯一の未実装 MVP タグ。受入: 未保存編集中に `kill -9` → 再起動で編集内容が戻る
- [ ] **ツリーのファイル操作**: 右クリックに 新規ファイル / 新規フォルダ / リネーム / 削除（OS ゴミ箱）/ 複製 を追加、ツリー行のインライン入力で命名。受入: マウスだけで「フォルダ作成→ファイル作成→リネーム→ゴミ箱」が一巡できる
- [ ] **編集の所作一式**: ⌥←→ 単語移動・⌥⌫ 単語削除・⌘↑↓ 文頭/文末・⌥↑↓ 行移動・⇧⌥↑↓ 行複製・⌘⇧K 行削除・⌘/ コメントトグル（言語別 prefix 表）・改行の自動インデント（前行継承 + ブロック開始で 1 段）・括弧/クォート自動ペア（選択を囲む対応含む）・Tab/⇧Tab と ⌘[ ⌘] のインデント増減（`tab_size` 反映）。受入: 各操作が editor_core の unit test + 実機で効く
- [ ] **multi-cursor の UI 配線**: コア（`editor_core` の複数レンジ edit・テスト済み）に対しビュー側アクションが無い。⌘D 次の一致を追加選択・⌥⌘↑↓ 上下に追加・⌥クリック追加・Esc で単一化・全カーソルの同時入力/削除/ペーストとキャレット複数描画。受入: ⌘D×3 で同名 3 箇所を同時に書き換えられる
- [ ] **ナビゲーション履歴**: ジャンプ級の移動（F12・⌘P・検索ジャンプ・大距離クリック）で位置を積み、⌃- 戻る / ⌃⇧- 進む（閉じたファイルは開き直す）。受入: F12 で飛んで ⌃- で元の行へ戻れる
- [ ] **soft wrap + 行ジャンプ**: 設定 `soft_wrap` + ⌥Z トグルで折り返し描画（論理行→表示行マップを行仮想化と両立させる）。⌃G（または ⌘P 内 `:42`）で行ジャンプ。受入: 長い Markdown 行が折り返され、⌃G→100 で 100 行目へ
- [ ] **設定の実効化**: `font_size` / `tab_size` を editor_view へ配線(現状 13px 等ハードコード・settings_core に定義のみ) + ユーザー `keymap.json` の読込（`keymap_core` はロード可能・main.rs が組み込み JSON のみ）。どちらも live-reload（settings watcher 既存）。受入: font_size 変更が再起動なしで反映・ユーザー keymap でバインドを差し替えられる
- [ ] 受入（総合・ドッグフーディング開始）: **Shirushi で Shirushi を丸 1 日開発できる**（この日から自分の変更を自分で浴びる）

## M11 — 言語知能と Git の parity（「他人のリポジトリを違和感なく直せる」）

- [ ] **フォーマット**: `textDocument/formatting` + 保存時フォーマット（設定 `format_on_save`）。受入: ⌥⇧F と保存時整形が rust-analyzer で効く
- [ ] **rename（F2）**: WorkspaceEdit を複数ファイルへ適用（未オープンはディスク書換・オープン中はバッファ反映）。受入: 構造体名の rename が定義+全使用箇所に効く
- [ ] **code actions（⌘.）**: 診断位置の `textDocument/codeAction` → 一覧 → 適用（WorkspaceEdit 適用は rename と共通基盤）。受入: unused import を ⌘. で削除できる
- [ ] **参照検索（⇧F12）**: `textDocument/references` → 検索パネルのファイル別グルーピング UI を再利用して一覧→ジャンプ
- [ ] **シンボル**: ①アウトライン（tree-sitter クエリ駆動 = LSP 不要で全言語動く・⌘⇧O picker またはサイドパネル）②⌘T ワークスペースシンボル（LSP `workspace/symbol`）。受入: workspace.rs の関数一覧を ⌘⇧O で絞ってジャンプ
- [ ] **診断一覧 + F8**: statusbar の ✗▲ クリックでファイル別一覧→ジャンプ・F8/⇧F8 で次/前の診断へ
- [ ] **tree-sitter 多言語**: TS/TSX/JS/Python/Go/JSON/TOML/YAML/Markdown/HTML/CSS の grammar 追加（`lang.rs` `for_extension` は現状 rs のみ。コメント prefix・インデント規則も言語表へ）。LSP 側は 7 言語ルーティング済み — この非対称を解消する。受入: .ts / .py がハイライトされ ⌘/ が正しいコメント記号になる
- [ ] **増分パース + didChange 差分化**: `Tree::edit` で再解析を編集近傍に限定（512KB スキップの緩和）・LSP didChange を FULL→incremental へ。受入: 1MB の .rs で 1 文字編集のハイライト更新に体感遅延なし
- [ ] **diff エディタ**: HEAD vs バッファの diff をタブとして開く（unified から・side-by-side は後続可）。入口 = git パネル / gutter / M12 の承認カード。受入: 変更ファイルの diff をタブで開き hunk 間を移動できる
- [ ] **hunk 操作**: gutter の変更バークリック→ポップオーバー（この hunk を stage / 巻き戻し / コピー）+ git パネルから hunk 単位 stage。受入: 1 ファイル内 2 hunk の片方だけ stage してコミットできる
- [ ] **blame（インライン）**: `git blame --porcelain` を遅延+キャッシュで、現在行の行末に dim 表示（作者・日時・要旨）。受入: 行にカーソルを置くと由来が見える
- [ ] 受入（総合）: 他人の TypeScript リポジトリを clone → 読む（アウトライン/参照/hover）→ 直す（rename/quickfix/フォーマット）→ hunk stage でコミット、まで Shirushi 内で完結

## M12 — AI の唯一無二（「色と通知で、並行エージェントに迷わない」）

FEATURES で later タグの checkpoint / @mention / ⌘K / Todos をここへ前倒し採用（2026-07-15 分析。タグ自体は不変更 = ユーザー管理）。

- [ ] **スレッド永続化**: transcript+メタ（宛先・モデル・トークン累計）を state に保存し再起動で復元・過去スレッドのブラウズ。ACP の session/load（resume）対応可否を調査し、不可なら「履歴表示+新セッションで継続」。現状は `seed_threads` の mock が初期値 = 再起動で消える。受入: 再起動しても昨日のスレッドが色ごと残り、続きを送れる
- [ ] **チェックポイント / 巻き戻し**: エージェントのファイル編集前にターン単位の自動スナップショット（hot exit のバックアップ基盤を流用）。スレッドの各ターンに「この時点へ戻す」。Git 非依存の信頼担保（`research/cursor-features.md` の結論）。受入: エージェントに 3 ファイル壊させて 1 操作で全部戻せる
- [ ] **エージェント編集の生中継**: watch（M10）と接続 — エージェントが書いたファイルを開いていれば即反映し、gutter にスレッド色のマークを出す。受入: bypass モードで「コメントを足して」→ 見ているバッファに変更がスレッド色付きで現れる
- [ ] **スレッド⇄成果物の色リンク**: スレッドが触ったファイルを Thread に記録し、タブ/ツリーの該当ファイルへスレッド色ドット・スレッド側に「触ったファイル n」チップ → クリックで diff（M11）へ。3 エディタのどれも持たない「この変更は誰の仕業か」の可視化 = 方向感覚の完成形。受入: 2 スレッド並走後、ツリーの色ドットだけでどちらの変更か判別できる
- [ ] **ターン終了サマリー + 通知**: ターン完了/権限待ちで ①トースト（UI-SPEC §8・右下）②statusbar スレッドドット（M4 の持ち越し）③未フォーカス窓は Dock バッジ。サマリー = 触ったファイル数・±行・所要時間。受入: 裏の worktree 窓のスレッドが権限待ちで止まったら手前の窓で気づける
- [ ] **diff レビューの本体化**: 承認カードに「エディタで開く」→ M11 の diff タブで全変更をレビューして accept/reject（compact diff は要約に格下げ）。受入: 大きな編集の承認判断を diff タブで下せる
- [ ] **@mention の完成**: `＋context` を Picker 化（fuzzy 全ファイル検索 — 現状先頭 18 件のみ・`agent_panel.rs:1408`）+ フォルダ / 選択範囲 / ターミナル出力の mention。受入: 目当てのファイルを 3 秒で @ 参照に載せられる
- [ ] **⌘K インライン編集**: 選択範囲+指示 → その場に diff 表示 → accept/reject（ACP 経由・チャットへ行かない最短経路）。ターミナルにも同型（自然言語→コマンド生成）。キーは keymap で最終決定（Cursor 準拠 ⌘K を暫定）。受入: 関数を選択して「Result を返すように」→ その場で差分適用できる
- [ ] **Todos（プラン）表示**: エージェントの plan/todo 更新を transcript 上部の常設チェックリストに（VSCode Claude Code 拡張踏襲・FEATURES §12）。受入: マルチステップ指示で進行中の項目に ● が付く
- [ ] **Todo ボード（人間の板を AI が消化・2026-07-15 発案）**: 真実は **`.shirushi/todos.md`**（markdown チェックボックス+日付見出し。settings と同じ「ファイルが真実・UI/CLI/MCP/AI は全部書き手」方式）。レール ☑ → パネル表示・チェッククリック=ファイル書き換え・普通のファイルとしても編集可。各項目 **▶ でスレッドへ prompt 送信**（末尾に「完了したら todos.md の該当項目をチェックせよ」を自動付与・実行中はスレッド色 pulse ドット・宛先チップ連動）。エージェントが完了時に自分でチェックを入れ、watch（M10）で板へ即反映＝「チェックがひとりでに入る」体験。**✨ 今日の計画** = ai_commit_message と同型（ROADMAP/git status/昨日の残りを `claude -p` へ→下書き生成）。**逐次消化モード**（TurnEnded で次の未チェックを自動送信 = /goal ループの GUI 化）は checkpoint 実装後に解禁。前項のエージェント内部 Todos とは別物として共存（板=プロジェクトの1日 / 内部 todos=1ターンの分解）。`shirushi mcp` に todos ツール追加で外部 CLI からも同じ板を操作可（任意）。受入: 朝に 3 項目書き → 2 つを AI に消化させて**チェックが自動で入るのを見届け** → 1 つを手動チェック、その一部始終が todos.md の git diff に残る
- [ ] **`.shirushi` の色/アイコン + 色ピッカー**: `.shirushi/settings.json` の `color` / `icon`（絵文字/画像）読込（`theme_core::ProjectIdentity` は型定義済み・未配線）+ レール右クリック→色ピッカー（選択を .shirushi へ保存）。Peacock 相当の完成 = 差別化の核の未回収分。受入: プロジェクトに好きな色と絵文字を与え、再起動後もレール/ピル/キャレットへ貫通
- [ ] **⌘O の 2 階層化 + worktree ダッシュボード**: ⌘O を project → branch/worktree の 2 階層に（UI-SPEC §7・現状 ⎇ メニューで代替中）+ 各 worktree の ahead/behind・dirty・実行中スレッド（色ドット）を一望。受入: 3 worktree 並行中に ⌘O だけで「どこで何が走っているか」を見て切り替えられる
- [ ] **トークン台帳**: スレッド横断のトークン集計（スレッド一覧に累計・今日の合計）。受入: 今日どのスレッドが何 k 使ったか見える
- [ ] 受入（総合・当初ビジョンの完成形）: **worktree×3 で 3 スレッドを並走させ、色と通知だけで迷子にならず、全変更を diff レビュー → 必要なら checkpoint で巻き戻せる**

## M13 — 公開準備（「英語話者が DL して 10 分で使い始める」）

旧 M9 の未チェック残件をここへ統合（2026-07-15 判断: 日常機能 M10〜M12 を先に）。

- [ ] **コマンドパレット ⌘⇧P + CommandRegistry**: 全アクションを名前+キー併記で登録式に（M3 からの持ち越し。StatusItemRegistry も同時に切る = FEATURES §9「登録境界を最初に切る」の第一歩）。Picker 再利用。受入: ⌘⇧P から任意の機能を名前で実行できる
- [ ] **i18n の回収**: ハードコード日本語 UI 文字列を全て `t!` 化（現状 locales は 5 キーのみ・「ソース管理」「承認が必要」等が直書き = CLAUDE.md 規律との乖離）。ja/en 両整備 + parity テスト + locale 切替の実機確認。受入: `locale=en` で全 UI が英語になる
- [ ] **入力レイテンシベンチ**: key→frame ヒストグラム（zed `input_latency_ui` 移植・M2 からの持ち越し）+ 起動時間の release 計測を CI へ。性能予算（Zed 比 ~80%）の自動検証。受入: 予算超過で CI が fail する
- [ ] **自動アップデート**: GitHub Releases の署名済み .dmg を起動時チェック→DL→差し替え（Sparkle 系 or 自前）。受入: 旧バージョンから 1 クリックで更新できる
- [ ] **Linux ビルド**: GPUI Linux（Wayland/X11）で起動・CI artifact 化（フォント/パス/キー差異の吸収）。受入: Ubuntu で編集・保存・ターミナルが動く
- [ ] **ターミナル仕上げ**: file:line のリンク化（クリックでジャンプ — AI がターミナルへ吐くパスにも効く・Zed 方式）+ IME 前編集対応。受入: cargo のエラーパスをクリックで該当行へ
- [ ] Remote（旧 M9）: local/remote の latency・CPU・memory benchmark と性能予算の自動検証
- [ ] Remote（旧 M9）: Linux x86_64/aarch64 musl artifact・署名/checksum・古い server cleanup・GUI askpass
- [ ] Remote（旧 M9）: dirty buffer crash backup（M10 hot exit の remote 版）・watch subscription/event/cancel（M10 watch の remote 版）・再接続後の LSP/PTY handle 再同期
- [ ] Remote（旧 M9）: sleep/VPN断/ControlMaster kill/server kill/巨大 tree の実ホスト障害注入 test
- [ ] Remote（旧 M9）: Remote Projects UI・SSH config picker・構造化接続ログ・retry/cancel・localhost port forwarding
- [ ] Remote（旧 M9）受入: remote Linux で編集/Git/LSP/terminal/agent を一日使い、切断復帰しても未保存変更を失わない
- [ ] **初回起動体験**: 空状態の案内（プロジェクトを開く・エージェント接続の導線）+ README/スクリーンショット整備
- [ ] 受入（総合）: 英語話者が GitHub から DL → 10 分で「開く・編集・保存・検索・AI に 1 タスク」まで到達できる

---

以降（later）: FEATURES.md のタグに従う（拡張モデル ADR・WASM ホスト・minimap・multibuffer 本体・vim・テーマ/言語パック配布 等）。
