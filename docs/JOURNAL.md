# JOURNAL — 実装日誌

`/goal` の各セッションが末尾に1エントリ追記する（新しいものが下）。形式:

```
## YYYY-MM-DD — 見出し
- やったこと: （変更ファイル・マイルストーン進捗）
- 学び/罠: （次のセッションが踏まないために）
- 次: （次の1歩）
```

---

## 2026-07-11 — スキャフォールドとドキュメント駆動化
- やったこと: 調査3本+マトリクス、mock v0.3、workspace スキャフォールド（cargo check 通過）、ARCHITECTURE/UI-SPEC/ROADMAP 整備、/goal コマンド新設
- 学び/罠: macOS 26 は Metal Toolchain が別コンポーネント（導入済み）。`cargo check | tail` は tail の exit 0 に騙される — 終了コードは PIPESTATUS でなく出力の error 行で判定せよ。バックグラウンドエージェントの .output はほぼ空のままが正常
- 次: M0（ユーザーの週末テスト）→ M2 着手

## 2026-07-11 — M0 通過・きせかえ明文化・フォルダリネーム予定
- やったこと: **M0 通過**（hello_world 3m28s ビルド→表示、ユーザー確認）。テーマきせかえを明文化（FEATURES 2 / UI-SPEC §1 きせかえ契約 / ARCHITECTURE theme_core — テーマ×プロジェクト色の独立2軸）。/goal コマンド登録確認
- 学び/罠: なし
- 次: フォルダを `editor` → `shirushi` にリネーム予定。**リネーム後の整備タスク（次セッションでやること）**:
  1. docs 内の絶対パス一括置換（`~/Work/experience/editor` → 新パス。対象: MVP-PLAN.md・BACKGROUND.md・JOURNAL.md・memory）
  2. mock/index.html のターミナル表示パス修正
  3. `cargo check -p shirushi` で再ビルド確認（キャッシュ再構築で数分かかるのは正常）
  4. Claude Code メモリの移行: `cp -r ~/.claude/projects/-Users-daichi-Work-experience-editor/memory ~/.claude/projects/<新パスのスラッグ>/`（新フォルダで一度セッションを開くとスラッグが分かる）

## 2026-07-11 — M2 theme_core（デザイントークン）＋ リネーム後始末
- やったこと:
  - **M2 `theme_core` 実装**（`crates/theme_core`、`[lib] path = src/theme_core.rs`）。UI-SPEC §1 を型に写した:
    - §1.1 全トークンを `Theme`（`bg0..3`/`fg0..2`/`border`/`ok`/`warn`/`err` + `SyntaxColors` 8 種）に。`Theme::dark()` に dark 値、`Theme::light()` に light 値（値だけ用意・セレクタは M3）。色は `Hsla` 保持
    - §1.2 巡回パレット: `project_color(i)` / `thread_color(i)`（modulo 巡回で index 安全）、`accent_dim(c)`（16%）
    - §1.1 テーマ非連動の固定色: `editor_selection()`（#7d9bd8 α0.28）/ `Theme::folder_icon()`（#7d9bd8 を bg3 に 55% sRGB mix）/ `claude_bullet()`（#d97757）
    - ARCHITECTURE §3 の `ProjectIdentity` / `IconSource` / `ThemeSource` / `Theme::load`（User JSON は M3 なので明示エラー）
    - unit test 9 本 green（トークン値=spec 照合・巡回・α・mix 範囲・load 分岐）。ワークスペースに `anyhow`（zed に合わせ 1.0.86）追加
  - **リネーム後始末（前エントリの「次」1〜4 消化）**: docs/mock の絶対パス `experience/editor`→`shirushi` 一括置換（JOURNAL の歴史記述行は温存）。mock はヘッドレス Chrome で再描画確認。Claude メモリを新スラッグ `-...-shirushi/memory/` へ移行（`next-editor-project.md`→`shirushi-project.md` にリネーム・stale パス修正）
- 学び/罠:
  - **フォルダリネームで GPUI ビルドが壊れるのはキャッシュ由来**。`gpui::GPUI_MANIFEST_DIR = env!("CARGO_MANIFEST_DIR")` はコンパイル時に絶対パスを焼く。`zed/` 無変更だと Cargo は gpui を再コンパイルせず、**build-dep 側の stale rlib** が旧パスを返し `gpui_macos/build.rs` が `ParseCannotOpenFile`（scene.rs）で落ちる。対処は **`cargo clean -p gpui -p gpui_macos`**（`zed/` は絶対に触らない）→ 再ビルドで解消。`cargo check` は通っても `cargo build`/`run` で初めて出るので注意
  - theme トークンは `Hsla`（gpui ネイティブ・α操作が素直）。hex→Hsla は `rgb(x).into()`（const 化不可なので関数で構築）。フォルダ色だけ bg3 との mix が要る＝`Theme` のメソッドにした
  - UI-SPEC §1.4 の密度/寸法は theme_core に**入れていない**（色に集中）。`editor_view` が要求した時点で追加する
- 次:
  - M2 次項 `i18n`（rust-i18n・`t!` 規律開始・`locales/ja.yml`+`en.yml`）→ その後 `editor_core`（Buffer/Selection/undo、GPUI 非依存 unit test）
  - `theme_core` を bin で消費する配線は `editor_view` 着手時に（現状 `main.rs` はハードコード hex のまま。Theme::dark() 参照へ移すと重複が消える）
  - **M1 最終項（`cargo run -p shirushi` の窓）はビルド解消済み＝確認可能**。ユーザーが窓を見たらチェックを入れる（ユーザー確認ゲートなので当方ではチェックしない）

## 2026-07-11 — M2 一気通し（i18n→editor_core→editor_view→bin→perf）
- やったこと（M2 の受入条件を上から連続実装）:
  - **`i18n`**（`crates/i18n`）: 薄い自作。`i18n::t!("領域.キー")` を workspace 横断で。`locales/{ja,en}.yml` を `serde_yaml` でパース・`include_str!` 埋め込み・OS ロケール自動選択。ja/en parity テスト付き（5 test + doctest）
  - **`editor_core`**（`crates/editor_core`, GPUI 非依存）: ropey 2.0（byte 索引で Selection と一致）で Buffer・Selection・Transaction undo/redo・snapshot・save・UTF-16↔byte・複数カーソル。**unit test 19 本 + bench 2 本**
  - **`editor_view`**（`crates/editor_view`): custom `Element`（`request_layout`/`prepaint`/`paint`）で複数行描画・行仮想化・行番号ガター・キャレット（プロジェクト色 2px）・選択面・現在行ハイライト・縦スクロール・マウス・**IME（`EntityInputHandler`）**。zed `examples/input.rs` を複数行に拡張。仮想化数値は unit test
  - **`shirushi` bin**: 起動引数でファイルを開く→ヘッダ（ファイル名+変更ドット）+ エディタ + ステータスバー（行:列・追従）。キーマップ登録・IME ロケール初期化・起動計測
  - **perf**: `scripts/startup-time.sh`（env `SHIRUSHI_STARTUP_LOG` で初回 render に `startup_ms` 出力）+ editor_core bench
- 実測（headless で取れたもの）: 起動 debug ~100ms（cold→first render）/ editor_core insert 1.5M ops/s・undo 6.8M/s・**~1MB バッファ上の編集 40.4万 cycle/s（≈2.5µs/編集）** / 1.6MB ファイルを開いて **idle CPU 0.0%**（＝可視行のみ shape＝仮想化が効いている）/ 全 crate **35 test green・警告0**
- 学び/罠:
  - **rust-i18n は多 crate に不適**。`t!` が `crate::_rust_i18n_t!` 展開で、`i18n!` を呼んだ crate 内でしか効かない。レイヤ化 UI（ui/editor_view/…が各々 t!）では自作の薄い実装が正解（ARCHITECTURE §6 を更新）。境界（`t!`）さえ守れば後で差し替え可
  - **ropey 2.0 は 1.x と非互換で byte 索引**（`insert(byte,..)`/`remove(byte_range)`）。ARCHITECTURE の byte-offset Selection 設計と綺麗に一致。行 API は `LineType` 引数（feature `metric_lines_lf` 要）、UTF-16 は `metric_utf16` feature 要
  - **GPUI Element は3相**（`request_layout`→`prepaint`→`paint`、`layout`/`prepend` は無い）。IME は `EntityInputHandler`（UTF-16 で話す）を `paint` 内で `window.handle_input` 登録。`examples/input.rs` が最良の最小雛形
  - **`screencapture` はこのセッションから使えない**（画面収録権限）。代わりに **gpui の `render_to_image` で自前 PNG** を実装（`crates/shirushi` の `--features screenshot`・`SHIRUSHI_SCREENSHOT=<path>`）。test-support 経由なので**通常ビルドには入れない**よう feature gate 済み
  - **offscreen `render_to_image` はグリフ（テキスト）を写さない**（矩形・色・キャレットは写る）。ただし診断で `shape_line` の幅を出したところ **12 行が非ゼロ幅で shape 済み（1 行目 140.4px・日本語含む）** → 実ウィンドウではテキスト描画される。offscreen 画像で確認できたのは**レイアウト/色/キャレット位置**（ヘッダのプロジェクト色ドット・エディタ面 bg1・現在行ハイライト・キャレットがプロジェクト色で行 0 の正位置・ガター幅・ステータスバー — すべて設計通り）
- **検証の限界（重要）**: ロジック（editor_core/i18n/仮想化数値）は full test。UI は「compile + 無クラッシュ smoke（多言語/絵文字/サロゲート/1.6MB）+ idle CPU で仮想化確認 + 起動計測 + **offscreen PNG でレイアウト/色/キャレット目視** + shape 幅で**グリフ生成を確認**」まで。**未確認 = (1) グリフの実描画目視（offscreen が写さないため。実窓 or screencapture 権限が要る）(2) IME の対話（実際に日本語変換）(3) スクロールの体感**。本人が `cargo run -p shirushi <file>` で最終確認するのが早い
- 次:
  - **UI 目視 & IME 対話検証**（screencapture 権限＋再起動、または本人実行）。ここが M2 完了ゲート
  - editor_view の見た目微調整（キャレット点滅 1.1s・パンくず・選択の複数行・ソフトラップ無し確認）
  - `input_latency_ui` の key→frame ヒストグラム移植（今は時期尚早・UI が育ってから）
  - その後 M3（settings_core / Picker / workspace / レール）へ

## 2026-07-12 — M3 ワークスペースシェル（レール/エクスプローラ/Picker/永続化）
- やったこと（受入「2 プロジェクトを色区別 + 再起動復元」達成）:
  - **`settings_core`**（6 test）: default→user→`.shirushi/settings.json` の深い JSON マージ・型付き `Settings`。bin でテーマ/ロケール反映
  - **`keymap_core`**（3 test）: JSON keymap を `App::build_action`（名前解決）+ `KeyBinding::load` で `KeyBinding` 化。既定 keymap を bin で読込 → 全アクション解決を実機確認
  - **`project`**（4 test）: `ignore` crate で gitignore 準拠の遅延 `read_dir` + 再帰 `all_files`（`.git` 除外・`require_git(false)`）
  - **`ui`**（2 test）: 再利用 `Picker`（fuzzy・キー操作・`EventEmitter`）。全モーダル共用
  - **`workspace`**: レール（プロジェクト巡回色・切替）+ 左ドックのツリーエクスプローラ + 中央エディタ + ステータスバー。⌘P ファイルファインダ・⌘O プロジェクトスイッチャー（Picker）。`state.json` で状態永続化・復元。bin のルートを `Workspace` に差し替え
  - 実測: 全 11 crate **~95 test green・警告0**。offscreen スクショで **2 色のレール**（indigo/teal リング）を目視。state.json 往復で復元確認
- 学び/罠:
  - **gpui の JSON keymap は `build_action(name)` + `KeyBinding::load(.., keyboard_mapper)`**。`actions!(ns, [..])` は `ns::Name` で自動登録され、別 crate から名前解決できる（keymap_core は具体アクション型に非依存）
  - **`ignore::WalkBuilder` は既定で `.git` が無いと .gitignore を無視する** → `require_git(false)` が要る（テスト用 scratch dir で発覚）
  - Picker→ホストのイベントは `cx.subscribe_in(&picker, window, handler)`（window 付き handler で確定時にファイル open/フォーカス）。オーバーレイは root の最後の child に `.absolute().inset_0()` で最前面
  - render_to_image のオフスクリーンはレイアウト/色/矩形を写す（グリフは写らない）＝レールの色区別の検証に有効
- 未（M3 の残り・honest）: テーマセレクタ UI / ユーザーテーマ JSON・タブ/分割/右下ドック・⌘⇧P コマンドパレット（要 CommandRegistry）・⌘1..9 / ⌘⏎ 新窓・カスタムアイコン・ファイル監視
- 次: M7 の tree-sitter ハイライト（theme の syn-* に接続・視覚的に大きい・自己完結）→ M6 検索 → M4 ACP（要 claude-agent-acp）/ M8（terminal・git）は外部統合が重い

## 2026-07-12 — M6 検索モデル + M7 tree-sitter ハイライト + 環境の再確認
- やったこと:
  - **オフライン思い込みの訂正**: crates.io は HTTP 200（**ネットあり**）、`claude-agent-acp` バイナリは実在（Zed の npx キャッシュ）、`claude` は PATH、git2 はキャッシュ済み。＝「不可能」ではなく「規模 + 実行時検証」の問題と判明
  - **M6 `search`**（7 test）: literal/regex・大小トグル・`search_text`（バッファ）/`search_files`（横断）。位置は byte・多バイト行のオフセットもテスト。ripgrep 子プロセスでなくインプロセス走査
  - **M7 `lang`**（3 test）: tree-sitter 0.25 + tree-sitter-rust 0.24 + tree-sitter-highlight。`HighlightKind`→theme の syn-* に接続。editor_view で行ごとに色 run 生成（`build_line_runs`・`spans_in_range` で行に重なる span だけ・編集で再解析・512KB 超スキップ）。実 .rs（editor_core.rs 900 行）を無クラッシュで開ける
- 学び/罠:
  - **`tree-sitter` は `links="tree-sitter"` の native crate＝1 版のみ**。tree-sitter-highlight 0.25 は tree-sitter 0.25 を要求 → 0.26 と衝突。**0.25 に揃える**（gpui は tree-sitter を使わないので競合なし）
  - `tree-sitter-highlight` の `HighlightEvent`（Source/HighlightStart/End）をスタックで畳んで**非重複・start 昇順の span** にすると、描画側は `partition_point` で行ごとに O(log n) で引ける
  - ハイライトの色は offscreen スクショに写らない（グリフ非表示）→ 私は span をテストで検証・実描画は要実機
  - **ACP（M4）の API を実地調査**: `agent-client-protocol` 1.2.0 は `futures` ベース（tokio 非依存＝gpui executor と噛む）・role/`ConnectTo`/`Stdio`/session builder。zed の `agent_servers/src/acp.rs` は**4000 行超**（+acp_thread 14k）。＝実装は可能だが**単独で大きな focused 作業**、かつ会話の実動作は Claude 認証つき実機でないと検証不能
- このセッションの到達点: **M2 完了・M3 完了・M6 検索モデル・M7 ハイライト**（12 crate・61 unit test green・本番ビルド警告0）
- 次（残り・全て実装可能。availability 確認済み）: M4 ACP（最大・要実機検証）・M7 LSP（rust-analyzer プロセス）・M8 git(git2 済)/terminal(alacritty 要取得)・M5 エクスプローラ 3 ビュー/右クリック・M6 検索結果 UI・コマンドパレット/⌘⇧P（要 CommandRegistry）

## 2026-07-12 — 【重大】文字が全く描画されない = font-kit 未有効 + フォント同梱
- 症状: 実ウィンドウで**四角（quad）は出るのにグリフ（文字）が1つも出ない**（エディタ/エクスプローラ/ステータスバー全部）。offscreen 撮影・私のテストでは検出不能で、**13 crate 積むまで気づかなかった**（daichi さんの実機目視で発覚）
- **根本原因: `gpui_platform` の `font-kit` feature が無効だった**。font-kit = macOS のグリフ**ラスタライザ**。無いと shape（幅計算）はできてもラスタライズ0＝文字が完全に不可視。quad は font-kit 不要なので出る。gpui の example が文字を出せるのは dev-dep で `gpui_platform = { features = ["font-kit", ...] }` してるから
  - **修正**: `crates/shirushi/Cargo.toml` の `gpui_platform = { workspace = true, features = ["font-kit"] }`。**gpui_platform を使う bin は font-kit 必須**（これが無いと文字ゼロ）。最重要トラップ
- 切り分け手法（有効だった）: `crates/shirushi/src/bin/hello.rs`（gpui 素の hello_world 相当を**同じビルドの gpui** で）→「緑だけ/文字なし」で「ビルド or gpui 側」と確定。`cargo clean` でも直らず＝古ビルドでない、と絞れた
- フォント同梱（OFL・GPL 本体に同梱可）: `assets/fonts/` に **IBM Plex Sans JP（UI）+ PlemolJP（コード・IBM Plex Mono+Plex Sans JP の等幅）**。`include_bytes!` + `cx.text_system().add_fonts()`（bin 起動時）。family 名は `"IBM Plex Sans JP"` / `"PlemolJP"`（fontTools で確認）。~15MB
- 併せて: コピペ ⌘C/⌘X/⌘V 追加、`default-run = "shirushi"`（hello bin 追加で `cargo run` が曖昧化したため）
- 学び: **UI に文字を出す変更は、必ず実機の目視確認を早期に1回入れる**。offscreen render_to_image はグリフを写さない・screencapture は権限で私から不可＝私単独では文字描画を検証できない。ここは daichi さんの目が要る（今後も）

## 2026-07-12 — フォント差替（Guguru Sans Code）+ 見た目の作り込み + M4 エージェントパネル
- やったこと（font 同梱 → 見た目 → M4 パネル、を1セッションで走破）:
  - **コードフォント差替**: PlemolJP → **Guguru Sans Code**（Google Sans Code + IBM Plex Sans JP の等幅・作者 yuru7・SIL OFL・v0.0.3）。UI の IBM Plex Sans JP と血統が揃う。family 名 `"Guguru Sans Code"`（`isFixedPitch`）。標準 1:2 幅版を採用（`*35`=3:5・`*Console*`=端末は不使用）。`assets/fonts/` + OFL 本文更新
  - **titlebar（UI-SPEC §3）**: `TitlebarOptions { appears_transparent: true, traffic_light_position: point(13,13) }` でシステム titlebar を隠し信号機だけ残す。左 78px を空けて自前描画。プロジェクトピル（左縁3pxプロジェクト色+「名前 ▾」+「⎇ branch」）・ドックトグル3アイコン（左/下/右）・空き領域は `window.start_window_move()` のドラッグハンドル
  - **タブ列/パンくず**: アクティブタブ上線2px=プロジェクト色・変更ドット(warn)・×で閉じる。パンくずはルート相対 ` › ` 連結。エクスプローラのヘッダも上位階層ブレッドクラム（末尾=現在フォルダ太字）
  - **statusbar 拡充**: ⎇branch（`.git/HEAD` を祖先まで辿る暫定）・診断 ✗0▲0・カーソル `行:列`（列は文字数）・UTF-8・言語名。レールに ＋（⌘O）と `⌘O` フット
  - **M4 `agent_panel`（右ドックまるごと・差別化の本丸）**: スレッド色タブ（上線+ドット）・メタ行（model/think ピル + **トークンメーター常時**）・transcript（msg-user 箱+左縁 / ✳Thinking 斜体 / ⏺ステップ+⎿結果 mono / 本文）・composer（宛先チップ ●+名前+`project ⎇ branch` / ⌘Enter 送信ボタン）。**スレッド色貫通**（タブ切替で下線/バー/左縁/ドット/枠/ボタンが一斉切替）
  - **ACP 1往復を配線**: `acp_client::prompt_once`（initialize→`build_session().block_task().start_session()`→`send_prompt`→`read_to_string`）。パネルは ⌘Enter→`agent::SubmitPrompt`→`cx.background_executor().spawn(prompt_once)`→`cx.spawn`+`WeakEntity::update` で応答を transcript へ（UI 非ブロッキング）
  - 実測: **15 crate・全 unit test green（~95本）・本番ビルド警告0**。offscreen で全レイアウト+**グリフ**を目視確認
- 学び/罠:
  - **【前回の訂正】font-kit 有効化後は offscreen `render_to_image` に *グリフも* 写る**。前回「offscreen はグリフ非表示」と書いたが、font-kit ラスタライザが有効なら写る＝**私単独で文字・フォント・ハイライトを自己検証できるようになった**（screencapture 権限は依然不要）
  - **gpui の border 色は全辺共通**（`border_color` は1色）。「枠+左縁だけ別色」は `items_stretch` + 先頭に幅3px の子 div（＝ピル左縁・タブ上線・msg-user 左縁は全部この手法。`absolute` 不要）
  - **composer は `EditorView` の平坦モード再利用**（`plain: true` でガター/行番号/現在行なし・UI フォント）。IME・複数カーソル・undo を本体エディタと共通化。`cmd-enter`→`agent::SubmitPrompt` は "Editor" context に bind＝本体エディタでは無ハンドラで無視・composer では祖先の AgentPanel が拾う（アクションはフォーカスパスを上昇）
  - **ACP session は `build_session(cwd).block_task().start_session()`**（`start_session` は `Blocking` 状態限定）。`read_to_string` は StopReason まで `AgentMessageChunk` を集約。`prompt_once` の future は **Send**＝gpui の background executor で回せる
  - `TitlebarOptions.appears_transparent=true` でも macOS 信号機は残る＝位置指定で自前 titlebar 内へ。窓ドラッグは空き領域の `on_mouse_down`→`start_window_move()`
- 未（honest・次の focused 作業）: **ACP 逐次ストリーミング**（`read_update` を回して chunk/thought/tool を都度反映＝現状は応答一括表示）・永続セッション（毎回プロセス起動をやめる）・権限リクエスト/diff レビュー・titlebar beacon + statusbar スレッドドット・分割ペイン・下ドック(ターミナル)・⌘⇧P パレット。transcript 初期内容は mock 会話例のプレースホルダ
- 実機で確かめたいこと（daichi さん）: `cargo run -p shirushi <file>` で Guguru の見え方 / 右パネルでスレッドタブ切替の色貫通 / composer に日本語入力→⌘Enter で claude-agent-acp から応答が返るか（`live_prompt` テスト: `cargo test -p acp_client -- --ignored --nocapture live_prompt`）

## 2026-07-12 —（続き）ACP を live で通す → ストリーミング化（エージェント自己検証で達成）
- ユーザーが `live_prompt` を実行 → `claude-agent-acp が PATH に無い` で失敗。**これが本当のブロッカーだった**（バイナリは PATH 上の単体ではない）
- **バイナリ探索を解決**: Zed は `npx @agentclientprotocol/claude-agent-acp@0.58.1` で起動していた（`~/Library/Application Support/Zed/external_agents/registry/registry.json` が根拠）。実体は Zed 専用 npx キャッシュ `~/Library/Application Support/Zed/node/cache/_npx/<hash>/node_modules/.bin/claude-agent-acp`（→ dist/index.js・shebang `#!/usr/bin/env node`）。`AgentCommand::claude` を **PATH → Zed キャッシュ → npx フォールバック**に強化
- **`run_session`（永続セッション + 逐次ストリーミング）**: `connect_with` 内で `build_session(cwd).block_task().start_session()` → prompt_rx から届く各 prompt を `send_prompt` → `read_update()` ループで `SessionMessage::SessionMessage(dispatch)` を `MatchDispatch::if_notification` で `SessionNotification` に開き、`SessionUpdate::AgentMessageChunk/AgentThoughtChunk` のテキストを [`AgentEvent`] に簡約して `event_tx` へ。`StopReason` = `TurnEnded`。`read_to_string` の実装を種に拡張
- **パネル配線**: `Thread.prompt_tx: Option<UnboundedSender<String>>`（初回送信で遅延起動＝以後常駐で文脈が続く）。⌘Enter→`send_prompt_text`→`start_session`（`cx.background_executor().spawn(run_session)` + `cx.spawn` で `AgentEvent` を `on_event` に逐次適用）。増分テキストは直前の同種エントリへ連結（新ターン先頭の User が自然な区切りになる）
- **エージェント自身が実機で全部検証**（今回はユーザーの手を借りず）:
  - `live_prompt`: 「1+1は？」→ `Ok("2")`（5.7s）
  - `live_stream`: 「3の倍数5個」→ `3, 6, 9, 12, 15` を逐次チャンクで受信 + TurnEnded
  - **アプリ内フルパス**: `SHIRUSHI_ACP_PROBE` で起動時に空スレッドへ自動送信 → offscreen スクショに「1+1は？」→「2」がスレッド色付きで表示。composer→常駐セッション→claude-agent-acp→ストリーミング→描画 が実機で動くことを PNG で確認
- 学び/罠:
  - **ACP エージェントは PATH 上の単体バイナリではなく npx パッケージ**（Zed も npx 起動）。探索は Zed キャッシュ直叩き（ネット不要・即時）を優先、無ければ npx。`node` が PATH に要る（shebang）
  - **`SessionMessage` は `#[non_exhaustive]`** → `match` に `_ => {}` が要る（外部 crate enum）
  - GPUI 非同期橋渡し（実証済みパターン）: `cx.background_executor().spawn(future)` で ACP を回し、`cx.spawn(async move |weak, cx| { while let Some(ev)=rx.next().await { weak.update(cx, ..).is_err()→break } })` で結果をフォアグラウンド反映。`run_session` の future は **Send**（ACP は Send 設計）
  - **`SHIRUSHI_ACP_PROBE` + `SHIRUSHI_SCREENSHOT_DELAY_MS`**: 起動時自動送信 + スクショ遅延延長で、**非対話な私でも実機の会話ストリーミングを自己検証**できる（font-kit で offscreen にグリフが写るのと合わせ、UI 検証の自立度が上がった）
- 未（M4 の残り）: **tool_call の transcript 表示**（⏺ ステップ化）・**UsageUpdate でトークンメーターを実値に**・権限リクエスト UI・ファイル編集の diff/accept・statusbar スレッドドット

## 2026-07-12 —（続き）Agent パネルの UX 改善（折り返し・可変幅・モデルセレクタ・タブ系ショートカット）
- ユーザー要望に対応: 「はみ出す・デフォもっと大きく・横幅ドラッグ・モデル変更 UI・Cmd+T/W」
- **はみ出し修正**: transcript の msg-user / thinking のテキスト側 flex 子に **`min_w_0()`**（flexbox は既定 `min-width:auto` で内容幅を割らない＝折り返さない。`min_w_0` で折り返し許可）。色バーは `flex_none()` 固定。clean スクショで長文が箱内で折り返すのを確認
- **既定幅 340→440** + **左縁ドラッグで可変**: 幅は workspace が保持（`agent_width`）。AgentPanel は `size_full()` で親任せに。workspace が可変幅コンテナ（`render_agent_dock`）に 6px のリサイズハンドル（`CursorStyle::ResizeLeftRight`）を置き、ハンドル `on_mouse_down` で開始、**root の `on_mouse_move`/`on_mouse_up`** で追従（ドラッグ中にカーソルがパネル外へ出ても root なら拾える）。左縁を左へ→広がる。clamp [320,900]
- **モデルセレクタ（Zed 風）**: composer の model ピルをクリック式に（`▾`）。`model_menu_open` で絶対配置ドロップダウン（`MODELS` 一覧）を composer 上にポップ。選択で `thread.model` を切替。**現状はラベル切替のみ（ACP エージェントへの実反映は継続課題＝session の model 指定 API を要調査）**
- **タブ/スレッド系ショートカット**: `⌘W`=`workspace::CloseTab`（アクティブエディタを閉じる）、`⌘⇧A`=`workspace::NewThread`（workspace が agent_panel に転送・右ドックを開く）。keymap の全域セクションに追加。既存: ⌘S/⌘Z/⌘⇧Z/⌘C/⌘X/⌘V/⌘A/⌘P/⌘O/⌘Enter/⌘Q。**⌘T（新規エディタタブ）は複数タブ編集が未実装のため保留**（現状 1 ペイン 1 エディタ）
- 学び/罠:
  - **GPUI で flex 子のテキストを折り返すには `min_w_0()` が要る**（CSS の `min-width:0` と同じ。これが無いと横にはみ出す）
  - **ドックの可変幅ドラッグは root スコープで mouse move/up を拾う**のが定石（ハンドル要素だけだとカーソルが外れた瞬間に追従が切れる）。ハンドルで開始 x/幅を記録し、root で delta を反映
  - offscreen プローブを長め（13〜15s）に待つと、前面化した実ウィンドウに**環境のキーイベントが漏れ込む**ことがある（⌘O/⌘⇧A が偶発発火して switcher / 新スレッドが出た）。＝逆にショートカットが動く傍証だが、検証は短時間 or プローブ無しのクリーン撮影が確実
- 設計メモ（未決）: ユーザーは「AI パネルの UI は完全に Zed で・モデル変更等」を希望。現状は**色スレッド（差別化の核・BACKGROUND 由来）を維持しつつ Zed 風コントロール（モデルセレクタ）を足す**方針で実装。色スレッドを捨てて純 Zed にするかは要確認（勝手に核を捨てない）

## 2026-07-12 —（続き）窓操作を Zed 準拠に（最大化・ドラッグの不具合修正）
- 症状（ユーザー報告）: 最大化やウィンドウの扱いが変。原因＝自前 titlebar（`appears_transparent`）にした際、Zed がやっている窓操作の作法が抜けていた
- **修正（`crates/platform_title_bar/src/platform_title_bar.rs` と `zed.rs` を参照）**:
  - `WindowOptions.is_movable = false`（macOS）。**gpui のコメント通り**: custom titlebar で `start_window_move` を使う場合、true のままだと AppKit が titlebar を system 所有扱いして**クリック遅延・ダブルクリック判定の不具合**になる。Zed も macOS では `is_movable: cfg!(not(target_os="macos"))`＝false
  - titlebar を **`.window_control_area(WindowControlArea::Drag)`** + **ドラッグ状態機械**に: `on_mouse_down`→`should_move=true`、`on_mouse_move`→`should_move` なら `start_window_move()`（＝**down後に動いて初めて**ドラッグ開始・クリックと区別）、`on_mouse_up`/`on_mouse_down_out`→`false`。以前は spacer で down 即 `start_window_move` していてクリックと混ざっていた
  - **ダブルクリックで最大化**: titlebar に `.on_click(|e,w,_| if e.click_count()==2 { w.titlebar_double_click() })`（macOS はシステム設定の zoom/minimize を尊重。`window.titlebar_double_click()`）
  - titlebar 内の対話子（プロジェクトピル・ドックトグル）は `on_mouse_down` で **`cx.stop_propagation()`**＝ドラッグ/ダブルクリックを起こさない（Zed も同様）
- 学び/罠: **`is_movable:false` は「動かせなくする」ではなく「system の titlebar 掴みを無効化して自前ドラッグに委ねる」設定**。custom titlebar では必須。ドラッグは down→move の状態機械にしないとクリック/ダブルクリックと競合する
- 未検証（対話操作のため offscreen 不可・ユーザー確認向け）: 実際の最大化/ドラッグ/ダブルクリックの体感。信号機（赤黄緑）の close/min/zoom は OS 任せで従来通り

## 2026-07-12 —（続き）composer 下部を Zed 風コントロール列に（権限モード/モデル/effort）
- ユーザー: 「Zed に bypass permissions, default(model selection), effort(High), speed(あるやつは) みたいになっている」＝下部コントロール列がほしい
- **実装**: composer 下部に 3 つの選択ピル（Zed のエージェント下部相当）:
  - **権限モード** `default / accept edits / bypass permissions / plan`（Claude Code の permission mode。実体は ACP の `SessionMode`／`set_mode` だが今は UI ラベル）
  - **モデル** `claude-opus-4-8 / sonnet-5 / haiku-4-5 / fable-5`
  - **effort** `low / medium / high / max`
  - 各ピルクリックで絶対配置ドロップダウン（`Selector` enum + `open_menu: Option<Selector>` + `menu_left()` で各ピル下に出す）。値はスレッド単位（`permission_mode`/`model`/`effort`）。**offscreen で dropdown 展開まで自己検証**（`SHIRUSHI_OPEN_MODE_MENU` フックで一時的に開いて撮影→確認後 None に戻す）
  - 上部メタ行は重複解消でトークンメーターのみに（`model_pill` 撤去）
- **speed は保留**（モデル依存＝一部のみ。ACP の config option を読んで出すのが筋）
- 未（次段・重要）: **これらを ACP に実反映**する。権限モード = `session/set_mode`（`AgentSessionModes` 相当）、モデル/effort/speed = セッションの advertised models / config options（`NewSessionResponse` と `SessionUpdate::{CurrentModeUpdate, ConfigOptionUpdate}`）を読んでピルを動的生成し、選択を送る。今はラベルのみ
- 設計判断（ユーザー確認待ち）: 色スレッド維持(A) か 完全 Zed 化(B) か。現状は (A)（色スレッド + Zed 風コントロール）

## 2026-07-12 —（続き）エージェント切替（Claude/Codex/…）+ Add context
- ユーザー: 「add context も完全コピー」「Codex のパターンもあるのでエージェントに寄って切り替えたい」
- **エージェント切替**:
  - Zed の `external_agents/registry/registry.json` から起動方法を採取: Claude=`@agentclientprotocol/claude-agent-acp@0.58.1`、Codex=`@agentclientprotocol/codex-acp@1.1.2`、Gemini=`@google/gemini-cli --acp`、Copilot=`@github/copilot --acp`、Qwen=`@qwen-code/qwen-code --acp --experimental-skills`。**Zed キャッシュに落ちているのは claude-agent-acp のみ**（他は初回 npx DL + 各サービス認証）
  - `acp_client` を **`AgentKind` 表**に一般化（id/label/bin/package/extra_args）。`command(cwd)` = PATH → Zed npx キャッシュ `.bin/<bin>` → `npx <package> <extra_args>`。`AgentCommand::claude` は `AGENTS[0].command` に委譲（互換）
  - パネル: `Selector::Agent` 追加（`AGENT_LABELS`）。`Thread.agent` を持ち、`start_session` は `AgentKind::by_label(&thread.agent).command(cwd)` で起動。**エージェント変更で `prompt_tx=None`** ＝次送信で新エージェントのセッションを張り直す。ドロップダウンに Claude/Codex/Gemini/Copilot/Qwen を確認
- **Add context**:
  - composer に **`＋ context`** ボタン + 添付チップ（× で外す）。クリックでプロジェクトのファイル候補ドロップダウン（workspace が `worktree.all_files(60)` を `set_destination` 経由で渡す）。送信時に添付を **`@path`** として prompt 先頭へ付ける（Claude Code が @参照でファイルを読む）。表示は素の prompt のまま
  - 未: ファイル検索（今は先頭18件のみ）・シンボル/スレッド/URL 等の他コンテキスト源
- **composer レイアウト**: 下部を2段化（セレクタ列 [エージェント/モード/モデル/effort] + 送信行）。メタ上部はトークンメーターのみ
- 未（重要・次段）: これら（agent 以外の mode/model/effort/context）を **ACP に実反映**。特に権限モード=`set_mode`、モデル/effort=advertised models/config options。Codex/Gemini 等の live 検証はユーザーの各認証が要る（Claude は検証済み）

## 2026-07-12 —（続き）エージェント名を registry 準拠に + 実行中 pulse アニメ
- ユーザー: 「Gemini, Qwen とかおかしい」「Agent なので Codex / Claude Code / Gemini CLI / GitHub Copilot?」「アニメーション・見た目もしっかり」
- **エージェント名を Zed の registry 表示名に**: Claude→**Claude Code**、Gemini→**Gemini CLI**、Copilot→**GitHub Copilot**、Qwen→**Qwen Code**（Codex はそのまま）。`AGENTS`/`AGENT_LABELS` と `Thread.agent` 既定を更新（`by_label` 一致）
- **実行中スレッドの pulse アニメ**（mock: 1.6s の breathing）: gpui の `Animation::new(dur).repeat().with_easing(pulsating_between(0.35,1.0))` + `with_animation(id, .., |el,delta| el.opacity(delta))`。適用先: titlebar beacon（`beacon_dot`）・スレッドタブのドット・composer 宛先ドット（`pulsing_dot`）。**実行中(running)のみ pulse・停止中は静止**（＝「今どのスレッドが動いているか」が動きで分かる＝差別化の核が生きる）
- 学び: gpui の要素アニメは `AnimationExt::with_animation`（全 `IntoElement` に実装）。`pulsating_between(min,max)` は gpui から直接 import・sine 系の自然な breathing。running 分岐で型が変わる（`AnimationElement` vs `Div`）ので `into_any_element()` で `AnyElement` に揃える小ヘルパにする
- 未（次の見た目仕上げ候補）: キャレット点滅（editor の custom Element に blink task）・hover のトランジション・メッセージ fade-in・token メーターの実値（UsageUpdate）

## 2026-07-12 —（続き）ホバー拡充 + メッセージ fade-in + トークン実値
- ユーザー: 「UX はホバーアニメ中心。そこら辺解決したら全部やりきって」
- **ホバー**: gpui のホバーは即時（CSS トランジション無し・`on_hover(bool)` で state 追跡は可能）。主要 interactive を crisp に: スレッドタブ（inactive→bg1）・送信ボタン（opacity 0.85）・プロジェクトピル（枠 fg2）。既存（ツリー行・レール・ドックボタン・各ピル・チップ×・メニュー項目）と合わせ全 interactive にホバー
- **メッセージ fade-in**: transcript 各エントリを `div().child(entry).with_animation(("transcript-entry", index), Animation::new(200ms), |el,delta| el.opacity(delta))` で**出現時に一度だけ** opacity 0→1。id 固定なのでストリーミングの増分再描画では再発火しない＝**常時再描画を起こさない（idle 0% を保つ）**。ラッパ div に掛けるだけで render_entry は無改修
- **トークンメーター実値**: `SessionUpdate::UsageUpdate{used,size}` → `AgentEvent::Usage` → `thread.tokens_used/max`。ターン中に実コンテキスト使用量が常時メーターに出る（これまで 23.4k/200k は種の固定値だった）
- **エージェント名**: Claude Code / Codex / Gemini CLI / GitHub Copilot / Qwen Code（registry 準拠・前セッションで修正）
- 設計判断（perf 配慮でこの turn は見送り）: **キャレット点滅**は素朴な perpetual timer だと idle 0% を壊すので、focus in/out で start/stop する設計が要る（次 turn）。**pulse/fade-in は一過性 or 実行中のみ**なので idle を壊さない
- 未（次の大物）: **選択値の ACP 実反映**（特に権限モード=`session/set_mode`）。run_session に prompt とは別の**制御チャネル**を足し、`futures::select` で prompt/mode を捌く refactor が要る（session_id を保持して `SetSessionModeRequest::new(session_id, mode_id)`）。model/effort は agent が advertise する config option 次第

## 2026-07-12 —（続き）キャレット点滅 + 権限モードの ACP 実反映（両大物・live 検証済み）
- **キャレット点滅（focus 連動・idle 0% 維持）**: editor_view に `blink_visible` + `_blink_task: Option<Task<()>>`。530ms ごとに反転 notify。**focus in で start / blur で stop**（`_blink_task=None` で drop＝停止）。start/stop の判定は paint（focus 情報あり）で `self.editor.update(cx, |v,cx| if focused { start } else { stop })`。編集/移動/クリックで `blink_visible=true`（点滅 OFF フェーズで消えないよう）。**blur 中は止まる＝背景で常時再描画しない**
- **権限モードの ACP 実反映**（`session/set_mode`）:
  - run_session を **単一 `SessionCommand` チャネル**（`Prompt(String)` / `SetMode(String)`）に一般化＝`select!` 不要で prompt とモード変更を1ループで捌く。SetMode は `connection.send_request(SetSessionModeRequest::new(session.session_id().clone(), mode_id)).block_task().await`
  - セッション開始時に `session.modes()`（`SessionModeState`）を読んで **`AgentEvent::Modes{modes:(id,name), current}`** を UI へ。`SessionUpdate::CurrentModeUpdate` → `ModeChanged`
  - パネル: `Thread.available_modes:(id,name)` を保持。モードセレクタは**広告モードがあればそれを表示**（無ければ既定）。選択で name→id を引いて `SessionCommand::SetMode(id)` を送る。`command_tx`（旧 prompt_tx）は `SessionCommand` 送信に
  - **live 検証**: Claude Code は実モードを広告 = `[Auto, Manual(=default), Accept Edits, Plan Mode, Don't Ask, Bypass Permissions]`。プローブでピルが「default」→実際の現在モード「**Manual**」に更新されるのを実機スクショで確認。会話（3,6,9,12,15）も無回帰
  - **トークンメーターも実値化**: `UsageUpdate{used,size}` を配線 → 実機で「**22.8k/1000k**」（1M ctx）表示
- 学び: **claude-agent-acp は権限モードを ACP の SessionMode で広告する**（id は camelCase: `bypassPermissions` 等・name は表示用）。ハードコードせず広告を読むのが正。run_session の「単一 Command チャネル + match」は `select!` より単純で堅い（prompt を送ってから StopReason まで読み切る間はモード変更が queue されるが、モード変更はターン間で十分）
- 未（次）: model/effort の ACP 反映（agent の config option 次第）・権限リクエスト UI（許可/拒否ダイアログ）・diff レビュー・hover の“本物のトランジション”（gpui は即時なので要素毎の on_hover+state+animation）

## 2026-07-12 —（続き）残り4大物を全部やりきり: Enter設定化 / 権限リクエスト+diff / model・effort反映 / hover遷移（全 live 検証）
- ユーザー: 「（残り列挙）これ全部、そのうえで日本語にありがちな enter で送られちゃう問題も設定で変更できるように」
- **① Enter 送信の設定化（日本語 IME 誤送信対策）**:
  - `settings_core::Settings.submit_on_enter: bool`（既定 false = Enter 改行・⌘Enter 送信＝IME 変換確定 Enter で誤送信しない安全側）+ `persist_user_value(path,key,value)`（user 設定の 1 キーだけ書き換え保存・他キー保持）
  - editor_view: `EventEmitter<ComposerEvent>`（`Submit`）。`newline` は **`submit_on_enter && marked_range.is_none()`** の時だけ `cx.emit(Submit)`（**IME 変換中は絶対に送信しない**のが肝）。`InsertNewline` アクション（`shift-enter`）は常に改行。`plain(theme,accent,submit_on_enter,cx)` + `set_submit_on_enter`
  - agent_panel: `cx.subscribe(&composer, |p,_,ev,cx| Submit => p.submit(cx))`。composer 下部にトグル（「⏎ 改行 / ⌘⏎ 送信」⇄「⏎ 送信 / ⇧⏎ 改行」）+ 送信ボタン文言も連動。トグルは runtime 反映 + `persist_user_value` で永続化。設定は main→Workspace::new→AgentPanel::new と貫通
- **② 権限リクエスト UI + diff レビュー**（`session/request_permission`）:
  - acp_client: read ループに **`.if_request(async |req: RequestPermissionRequest, responder| ...)`** を追加（`.if_notification(...).await` の後）。**ハンドラ内で respond_rx.next().await＝ユーザーの決定を待つ間ターンをブロック**（agent 側も待つので正しい）。UI は `AgentEvent::PermissionRequest{title,diffs,options,respond:UnboundedSender<usize>}` を受け、選んだ**添字**を respond に送る→`RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id))`、drop で `Cancelled`
  - 差分は `req.tool_call.fields.content` の `ToolCallContent::Diff{path,old_text,new_text}` を抽出。`compact_line_diff`（共通 prefix/suffix を刈って中央だけ ±・cap 14 行）で表示。**追加=`theme.ok`(緑)/削除=`theme.err`(赤) をテキスト色のみ**（面塗りしない＝UI-SPEC 色規律。ok/err は「診断・git（追加/エラー）」用に定義済＝許可色）
  - agent_panel: `Thread.pending_permission`。composer 直上に承認カード（「● 承認が必要 <tool>」+ diff + 許可/常に許可/拒否ボタン。種類でスタイル分け）。**live 検証**: Write プローブで緑 +diff とカード表示、`SHIRUSHI_AUTO_ALLOW` で round-trip→**実ファイル生成を確認**（許可応答が Claude に届き実行された）
- **③ model / effort の ACP 反映**（Claude が広告する: model=Default/Opus/Fable/Sonnet/Haiku・effort=Default/Low/Medium/High/Xhigh/Max）:
  - **重要**: model/effort に専用 API は無い。**session config options**（`SessionConfigOption` の `category: Model | ThoughtLevel`）で来る。かつ crate の `start_session()` は応答の `config_options` を **`..` で捨てる** → 手動で `send_request(NewSessionRequest::new(cwd))` して `response.config_options` を取ってから `attach_session(response, Vec::new())`。initialize で **config_options 能力の広告が必須**（`ClientCapabilities::new().session(...config_options(...boolean(...)))`）
  - `AgentEvent::Configs(Vec<ConfigOption{config_id,category,current,choices}>)`（開始時 + `SessionUpdate::ConfigOptionUpdate`）。`SessionCommand::SetConfig{config_id,value_id}` → `SetSessionConfigOptionRequest`（応答の更新後一覧を再 emit）
  - agent_panel: `Thread.configs`。Model/Effort セレクタは**広告があれば実選択肢に置換**（無ければ静的ラベル）。選択で name→value_id を引いて SetConfig。**live 検証**: モデルドロップダウンに Opus/Sonnet/Haiku/Fable、effort ピルが実際の現在値「Xhigh」（種の「high」でなく）を表示
- **④ hover の“本物のトランジション”**: 調査結論＝**gpui に CSS 的トランジションは無い / `with_animation` は forward-only・oneshot/loop・ElementId キー・可逆不可 / Zed は全 hover を即時 `.hover()`**（＝可逆 hover は非慣用）。折衷: `hovered: Option<HoverKey>`（`on_hover` で追跡・同時 1 要素）+ **hover-in だけ `with_animation`(120ms, ease_in_out) でフェード / hover-out は即時**。oneshot で settle 後は再描画要求しない＝**idle 0% 維持**。送信ボタン（opacity 沈み）・スレッドタブ（bg1 フェードイン）に適用。汎用 `hover_fade<E>(el,id,hovered,animator)`
- 学び: **Claude Code は mode/model/effort を全部 config options でも広告する**（mode は modes() と二重）。`attach_session` の `..` が config_options を捨てるので `build_session().start_session()` を使うと model が取れない→手動 NewSessionRequest。**権限応答は if_request ハンドラ内 await で待つのが自然**（別チャネル不要・ターンブロックが正しい）
- 結果: 全 crate `cargo build` 警告 0・`cargo test --workspace` **68 passed / 0 failed**。live: model ドロップダウン・effort=Xhigh・権限カード+緑diff・round-trip ファイル生成・Enter トグル、すべて実機スクショ/実ファイルで確認
- 未（任意の磨き込み）: 権限カードの複数 diff スクロール・diff の赤緑を色規律として UI-SPEC に正式追記するか判断・model/effort 変更の実効き（set_config_option）を live で最終確認

## 2026-07-12 —（続き）hover は Zed 流即時に確定 + 設定 live-reload 基盤（global+watcher）+ config CLI
- **hover 方針転換**: ユーザー「Zed の精神（即時 hover）が好き、同じにしたい」→ 前 turn の hover-in フェード（with_animation）を**全撤去して素の `.hover()` に戻す**（HoverKey/hovered/set_hover/hover_fade/on_hover 配線/force-hover 環境フックを削除）。Zed も全 hover が即時＝キビキビ・idle 0%・非慣用を避ける。コードも状態も減ってクリーン
- **設定アーキテクチャ確定（ユーザー合意）**: 「UI か json か」で選ばず、**settings.json を唯一の真実**にして UI トグル / CLI / MCP / 手編集を全部その「書き手」にする。3つ別々に作るとズレる。前提の欠けピース = **live-reload**（起動時1回読みでは CLI/手編集/MCP が再起動まで効かない）
- **`settings` crate 新設（gpui-aware レイヤ）**: `settings_core`（純ロジック・GPUI 非依存）はそのまま、その上に反応レイヤ。
  - `SettingsGlobal`（gpui `Global`）= 真実の共有。`init(user_path, project_dir, cx)` で load + `set_global` + watcher 起動。`get(cx)->Settings`、`set_user_value(cx,key,value)`（in-proc 即時反映+永続化）
  - **watcher = poll**（`background_executor().timer` 1.2s で user/project の settings.json の mtime 差分）。**解決値が実際に変わった時だけ** `update_global`＝無変化では observer を起こさない（`Settings: PartialEq` で比較）。＝**idle 0% を保つ**（notify/FSEvents へは後で差し替え可能）
  - ビューは `cx.observe_global::<SettingsGlobal>` で追従。agent_panel が observe→composer に submit_on_enter を反映。`submit_on_enter` の起動時スレッド（Workspace::new→AgentPanel::new 引数）は**廃止**し global 読みに一本化。toggle も `settings::set_user_value` に一本化（UI/CLI/MCP と同経路）
  - **live 検証済み**: `settings.json` を手編集で `submit_on_enter:false→true` に書き換え→**再起動なしで** composer が「送信 ⌘⏎」→「送信 ⏎」/「⏎ 送信 / ⇧⏎ 改行」に変化（watcher→global→observer→composer をデバッグ trace でも確認: `change detected differs=true` → `panel-observe fired global=true current=false`）
- **config CLI**（2つ目の書き手）: `shirushi config <list|get <key>|set <key> <value>>`。GUI を開く前に処理して終了。`set` は値を JSON 解釈（true/数値/文字列）して user settings.json に保存＝**起動中アプリは watcher で live 適用**。smoke: `set tab_size 2`→`get`→`2`
- 学び: **AI に設定を投げる目標は基盤だけでほぼ達成**＝Claude Code は**ファイル編集ツールで settings.json を直接書ける**、それが watcher で live 適用される。MCP は「構造化 API（list+スキーマ+検証）」と「ファイルで表せないエディタ操作（open_file/new_thread/set_model）」の付加価値。gpui: `update_global`（`BorrowAppContext`）は `end_global_lease` で `NotifyGlobalObservers` を push＝observer 発火。`App::spawn(async |cx: &mut AsyncApp|)` + `background_executor().timer` は window 生成前でも回る（screenshot task と同型）
- 結果: 警告 0・`cargo test --workspace` **68 passed / 0 failed**。crate 16 に
- （MCP は後述の hover pass の後に着手予定）

## 2026-07-12 —（続き）ホバー体験の総合強化（cursor / 色 / 影 / tooltip）= Zed 級の"気持ちよさ"
- ユーザー: 「アニメーションにはとことんこだわって。Zed が強い＝ファイルホバーで影・タブは指アイコン・説明はホバーで**すっと**出る・**ホバーで色が変わるのは絶対**。この気持ちよさは絶対必要」。前 turn で hover fade を撤去したが、本質は"即時 vs アニメ"でなく **cursor 変化 + ホバー色 + 影 + tooltip の総合**だった（全部 idiomatic）
- 調査（gpui/Zed）: `cursor_pointer()`/`shadow_*()` は **`Styled` トレイト**（id 不要・`.hover()` 内でも使える）。`tooltip`/`active` は **`StatefulInteractiveElement`**（`.id()` 必須）。**tooltip は 500ms 遅延あり・アニメは無し**（Zed も静的）。`.transition` は**無い**（確定）
- 監査結果: **cursor_pointer がどこにも無い**（クリック要素で指カーソルが出ない＝痛点）・**tooltip ゼロ**・tree 行は hover 無し
- 実装:
  - **`ui::Tooltip`**（新規・fade-in 付き）: `Tooltip::text(text, theme)` が `.tooltip(...)` 用の `Fn(&Window,&App)->AnyView` を返す。中身は bg2・罫線・**浮遊影**（`BoxShadow`）・テキスト。gpui は tooltip をアニメしないので**出現時に一度だけ opacity 0→1 の fade-in**（`with_animation` 110ms ease_out_quint・oneshot＝settle 後は再描画しない）＝"すっと出る"。`Tooltip::preview` で見た目をヘッドレス撮影確認済み（浮遊影付きの箱）
  - **cursor_pointer を全クリック要素へ**: レール project/＋・ツリー行（ファイル/フォルダ）・project pill・ドックトグル・タブ×・スレッドタブ・＋新規スレッド・セレクタピル4種・メニュー項目・＋context・context chip×・権限ボタン・送信ボタン・Enter トグル
  - **ホバー色を補完**: **ツリー行は hover で bg3 + fg0**（Zed 流＝ファイルが立つ。従来 hover 無し）・レール項目は hover で色濃く枠色化・pill/ボタン類も bg/色変化
  - **tooltip を説明が要る所へ**: レール＋（プロジェクトを開く ⌘O）・project pill（切替 ⌘O）・ドックトグル（左/下/右パネル）・タブ×（閉じる ⌘W）・beacon（スレッド名 — 実行中/待機中）・セレクタピル（エージェント/権限モード/モデル/effort の説明）・＋context・＋新規スレッド（⌘⇧A）・Enter トグル（挙動説明 + IME 注記）
- 学び: **Zed の"気持ちよさ"の実体は CSS トランジションではなく cursor_pointer + 即時ホバー色 + tooltip(500ms 遅延) + 浮遊影**。tree 行の"影"は Zed でも bg ハイライト（flush 行に影は不自然）＝面で表現。tooltip だけ fade を足すと上質になる
- 結果: 警告 0・`cargo test --workspace` 68 passed / 0 failed。`ui` に `Tooltip`、agent_panel が `ui` 依存に
- 未（要判断）: **自前 MCP サーバ**（transport: Stdio=ファイル経由 / in-proc HTTP=editor 操作可 / unstable ACP）。claude-agent-acp の mcp_capabilities 確認後に決定

## 2026-07-12 —（続き）hover が効かない根本原因を特定・修正（**要 `.id()`**）+ セレクタを Zed 流に
- ユーザー: 「hover 全然うまく行ってない、しっかりやりきって」「claude code 選ぶところのチップ、ダサい。Zed のシンプルさに」
- **根本原因（gpui の重要な罠）**: `.hover(|s| …)` のスタイル適用自体は hitbox で動く（id 不要）が、**hover 変化時の再描画（`cx.notify`）は要素に `element_state` がある時だけ発火する**（div.rs:2592 `element_state…hover_state`）。`element_state` は **`.id()` を付けた要素にしか無い**。＝**id の無い要素は、hover しても再描画がトリガされず、hover スタイルが視覚反映されない**（他の何かが再描画した時だけ偶発的に出る）。Shirushi は idle 0% 設計で mouse move では再描画しない（`on_resize_move` は resize 中以外 return）ため、**id 無し hover は完全に無反応**だった＝ユーザーの「全然効かない」の正体
- **修正**: **hover を持つ全要素に `.id()` を付与**（Zed も全 hoverable に id を付ける＝これが必須作法だった）。ツリー行・スレッドタブ・送信ボタン・権限ボタン・context chip×・各メニュー項目・resize ハンドル 等。workspace/agent_panel の `.hover(` 全 20 箇所が id 付き要素になっていることを grep で照合
- **セレクタを Zed 流に**（研究: Zed の model/mode selector = `Button` default = `ButtonStyle::Subtle`）: **枠も塗りも無し・透明背景**、hover でだけ薄い面（ghost_element_hover 相当）。muted テキスト ~12px + 小さいシェブロン ▾（8px）。open 中はラベルをスレッド色（accent）に。**チップ（rounded+bg3 塗り）を撤去**。実機スクショで「Claude Code ▾ default ▾ claude-fable-5 ▾ high ▾」が**素のテキスト+シェブロン**（チップでない）になったのを確認
- **ヘッドレスで hover を撮る試み**: `window.simulate_mouse_move`（test-support）を screenshot 経路に注入→ **reentrancy パニック**（`window.update` 内で dispatch_event すると leased entity を触って entity_map:142 で abort）。撤去。hover の視覚確認は live 必須（at-rest のセレクタ見た目は撮れる）
- 学び: **gpui で hover/active を使うなら要素に `.id()` は必須**（スタイルは id 無しでも「当たる」が、idle 0% アプリでは再描画が来ず反映されない）。これは今後の全 UI で守る鉄則。`simulate_mouse_move` は `window.update` 内から呼ぶと reentrancy で落ちる
- 結果: 警告 0・test 68 passed。at-rest の Zed 流セレクタ確認済み。**hover の効き自体はユーザーが live 検証**（cursor 変化・行の面立ち・tooltip）
- 未: 送信ボタンは今も色付き（ユーザーの指摘はセレクタ限定なので保留・Zed は icon + accent glyph の subtle）・MCP サーバ

## 2026-07-12 —（続き）セレクタのドロップダウン位置ズレ修正 + ファイルタブのアニメ
- ユーザー: 「セレクタ、上手く機能してない部分ある」「ファイルのタブもアニメして」「この後はファイルエクスプローラ？」
- **セレクタ不具合の正体**: ドロップダウンを **AgentPanel 基準の絶対配置 + ピル毎ハードコード `menu_left`（11/82/170/238）** で出していたが、ピルを Zed 流（透明・narrow）に作り直した結果ピル位置が変わり、**メニューがピルの下に来ない**（ズレ）＝「機能してない」。
- **修正（堅牢化）**: ドロップダウンを **各ピルの子**にして `.relative()` ピル基準で絶対配置（`menu_left` 撤去）。ピルの真上に開く（`.bottom(24)`）。右寄りの Model/Effort は `.right(0)` で右端揃え＝パネル外にはみ出さない。メニュー項目クリックは `cx.stop_propagation()`（ピルの toggle 再発火＝開き直しを防止）。**出現時に fade-in + 6px せり上がり**（with_animation 120ms oneshot）+ 浮遊影。open 中はピルのラベルを accent（スレッド色）に。実機スクショで model ドロップダウンがピル真上に整列・"Fable ▾" が accent 表示を確認
- **ファイルタブのアニメ/hover**: エディタタブに `.id("editor-tab")` + `cursor_pointer` + hover(bg2)。**新しいファイルを開くとタブが fade-in**（key=ファイル名＝別ファイルで再発火・with_animation 200ms oneshot）。× は既に cursor+hover+tooltip
- 結果: 警告 0・test 68 passed。次は **ファイルエクスプローラ**（Finder 的カラム/アイコン・本物のブラウザ挙動 = 本人ビジョンの核）に着手予定

## 2026-07-12 —（続き）ファイルエクスプローラ: アイコン + 3表示（ツリー/カラム/アイコン）+ 上位ナビ
- ユーザー: 「これやりましょう！！」（エクスプローラの Finder 感）
- **ファイルアイコン（拡張子別）**: `file_icon`（**フォルダ=横長・ファイル=縦長**のシルエットで一目で判別）。色は `file_type_color`（rs=橙/toml・json=青/md=灰/ts=黄/py=緑/html=紫/画像=シアン、theme.syntax パレット流用＝色による方向感覚）。`icon_large` はアイコングリッド用の2倍版
- **3表示モード + 左下スイッチャー**（`ExplorerView{Tree,Columns,Icons}`・`render_explorer_footer` の ☰▥▦。active は bg3）:
  - **Tree**: 従来 + アイコン（`render_tree`）
  - **Icons**: 現在フォルダ直下のアイコングリッド（`render_icons`・flex_wrap・84px セル・大アイコン+名前中央）。フォルダ click=中に入る・ファイル click=開く
  - **Columns**: Finder の Miller columns（`render_columns`）。ルート→`current_dir` の連鎖を各カラムに。末尾3段だけ見せる（460px 幅に収める・`overflow_x_scroll` は gpui に無いのでクリップ）。フォルダに `›`。カラム表示時だけ dock 幅を 460 に広げる
  - 状態: `ProjectSlot.current_dir`（カラム/アイコンの「現在フォルダ」）+ `enter_dir`（中に入る）
- **上位ナビ**: `render_explorer_header` = プロジェクト名→current_dir の**クリック可能ブレッドクラム**（各段 `enter_dir`＝上へ戻れる）。カラム/アイコンの「戻る」手段。従来の `ancestor_crumbs`（FS 祖先表示）は撤去
- 開発フック: `SHIRUSHI_EXPLORER_VIEW=icons|columns` で初期表示モード指定（撮影確認用）
- 実機スクショ: ツリー（アイコン付き）・アイコングリッド（2列）・カラム（crates の各 crate に ›）すべて確認。警告 0・test 68 passed
- 未: **右クリックメニュー**（フォルダ=新規ウィンドウでプロジェクトとして開く 等・task 25 の残り）。gpui は `overflow_x_scroll` 無し（カラムは末尾3段クリップで対処）

## 2026-07-12 —（続き）エクスプローラ: 幅可変（ドラッグ）+ 右クリックメニュー（新規ウィンドウで開く）
- ユーザー: 「右クリック大事。エクスプローラも幅可変（ドラッグで大きく）したい」
- **幅可変**: `explorer_width` + `resizing_explorer`（Agent ドックと同型）。エクスプローラ右縁に絶対配置のリサイズハンドル（`CursorStyle::ResizeLeftRight`・hover で border 色）。`on_resize_move` を両ドック対応に（Agent=左縁 dx 負で増 / エクスプローラ=右縁 dx 正で増・clamp [150,640]）。カラム表示に切替時、幅が狭ければ 440 に自動拡張（以後ドラッグ調整）。DOCK_WIDTH の固定 460 override は撤去
- **右クリックメニュー**: `ExplorerContextMenu{path,is_dir,position}` state。全3表示（tree/icons/columns）の各エントリに `on_mouse_down(Right, …)` → `show_context_menu(path,is_dir,event.position)`。メニュー = 位置に絶対配置・浮遊影。**フォルダ**: 「新規ウィンドウで開く」（`open_folder_as_window` = `cx.open_window` で新 Workspace＝**ウィンドウモデルの核**）/「このフォルダを開く」（enter_dir）/「パスをコピー」。**ファイル**: 「開く」/「パスをコピー」。背後に透明バックドロップ（`size_full`）を敷き外側クリックで閉じる（子のメニューは最前面）。root render の最前面に `.children(render_explorer_context_menu)`
- 開発フック: `SHIRUSHI_CONTEXT_MENU=1`（ルートの右クリックメニューを開いた状態で撮る）。実機スクショで「新規ウィンドウで開く/このフォルダを開く/パスをコピー」の浮遊メニュー確認
- 罠: 構造体リテラルはフィールド記述順に評価＝`projects,`（move）の後に `explorer_context_menu:` で `projects.first()` を使うと moved value error。リテラル前にローカルへ計算してから shorthand
- 結果: 警告 0・test 68 passed。**M5 ファイルエクスプローラ = アイコン/3表示/上位ナビ/幅可変/右クリック 一通り完成**。新規ウィンドウ開くの round-trip は live 検証待ち（コードは main.rs と同じ open_window パターン）

## 2026-07-13 — 「全部やりきる」Phase A: M3/M5/M6 残り（⌘1-9・新窓・root上ブラウズ・検索パネル・テーマセレクタ）
- ユーザー: 「全部やりきってください。基本のエディタ機能はZedを参考に」。ROADMAP の実装可能な残り全量に着手。まず重い Zed 移植3本（git / terminal / LSP）を並行調査に出し、要点を `docs/research/porting-git-terminal-lsp.md` に永続化（git=CLI直叩き+`imara-diff` / terminal=`alacritty_terminal`（EventLoopが読取+parser）/ LSP=`lsp-types 0.97`+自前JSON-RPC封筒+ropey UTF-16変換）
- **⌘1..9 / 新窓**: `ActivateProject1..9`（レール切替）+ `NewWindow`。新窓は **⌘⇧N**（⌘⏎ は composer 送信と衝突するため変更）。keymap 全解決を実機 stderr で確認
- **検索パネル（M6 受入）**: `SearchState`（クエリ+大小/正規表現トグル+ファイル別結果）オーバーレイ。`⌘⇧F`。マッチは先頭空白除去してアクセント色強調（`whitespace_nowrap`・`.inline()` は gpui に無い）。クリック/Enter で `EditorView::reveal_position`（**pending_reveal**＝初回描画前でも viewport 確定後の prepaint で対象行を中央へ・one-shot で idle 0%）。offscreen で「fn」440件のファイル別結果を目視
- **root 上ブラウズ（M5 受入）**: `project::Worktree::read_any_dir`（ルート外は gitignore 無しで列挙）。ブレッドクラムに **⤴上へ** + ルート外は **⌂プロジェクト戻る**。`enter_dir` がルート外へ出たら Tree→Columns 自動切替。offscreen で隣接 repo 一覧を目視
- **テーマセレクタ（M3）**: `theme_core` にユーザーテーマ JSON（トークン上書き・`appearance` を土台に欠けは組み込みへ）・`available_themes`/`resolve`。`⌘⇧T` で Picker、**`PickerEvent::Highlighted` で即ライブプレビュー**、確定で settings.json へ theme 名保存・中止で戻す。`apply_theme` がクローム/エディタ/Agentパネル/Picker へ波及（`EditorView::set_theme`/`AgentPanel::set_theme`/`Picker::set_theme` 追加）。light 全体反映を offscreen で目視
- 学び/罠: **raw 文字列に hex 色 `"#..."` を含めると `r#"..."#` が `"#` で早期終了** → `r##"..."##`。gpui に `.inline()` 無し（inline テキストは flex 行 + `whitespace_nowrap`）。`Picker` は汎用なので Highlighted を全モードで emit するが workspace 側でモード判定して無視
- 結果: 警告 0・**test 67 passed**（workspace 全体）。M3 テーマ/⌘1-9/新窓・M5 root上ブラウズ・M6 検索ジャンプ = 受入達成
- 次: Phase B = git status 色（ツリー/タブ）+ gutter diff（`imara-diff`）+ branch/worktree メニュー

## 2026-07-13 —（続き）Phase B: M8 git（status色・gutter diff・branch/worktree）
- **git モデル（`project`）**: Zed 準拠で git2/gix 不使用。`git_status`（`git status --porcelain=v1 -z` → XY を Added/Modified/Deleted/Untracked/Conflicted に畳む・絶対パス）/ `buffer_diff`（`git show HEAD:./<name>`〔cwd 相対で subdir も正〕+ **imara-diff 0.1.8** Histogram）/ `git_branches`/`git_worktrees`/`switch_branch`/`add_worktree`。dep 追加は imara-diff 1つだけ。test 6（diff 分類 + 一時 repo で status/diff）
- **ツリー/タブ色（workspace）**: `git_status: HashMap<PathBuf,StatusKind>` を switch_project/open_file/起動時に読み直す。ツリー行=ファイル名を状態色 + 末尾バッジ（M/A/U/D/!）・フォルダは配下変更で琥珀 ●（`keys().any(starts_with)` ロールアップ）。タブ名も同色貫通
- **gutter diff（editor_view）**: `diff_hunks: Vec<project::DiffHunk>`。**編集の choke は `after_edit` だけだと IME/入力を取り逃す** → **prepaint で `buffer.version()` 変化を検知**して一様に捕捉。`schedule_diff` = 250ms デバウンス（世代番号）+ `background_executor().spawn` で git 実行＝**idle 0%**。EditorPrepaint に `diff_marks: Vec<PaintQuad>` 追加、行ループで左端バー（追加=ok/変更=warn の全高・削除=err の上境界小マーカー）を積み paint。初回は `diff_scheduled_version=u64::MAX` で必ず計算
- **branch/worktree メニュー（workspace）**: titlebar ⎇ クリックで overlay（context menu と同型・背面クリックで閉じる）。ブランチ行=in-place 切替（`switch_branch`→`reload_active_project`＝ツリー再構築+開ファイル再読込+git更新）、**⧉=worktree で新窓**（`add_worktree` → `open_folder_as_window`＝当初ビジョンの入口）。worktree 一覧セクションも別窓で開ける。⎇ の子は `stop_propagation` でピル（⌘O）を抑止
- 学び/罠: `imara_diff::Sink::process_change(before,after)` の after=現在バッファ行域＝gutter キー。CRLF は diff 前に LF 正規化（さもないと全行 Modified）。git `rev-parse --show-toplevel` は realpath 返す→テストは canonicalize で比較。editor_view→project 依存を追加（view→model は正方向・lang 依存と同型）
- 結果: 警告 0・**test 全 suite ok**。offscreen で ①ツリー Cargo.toml=M 琥珀 + gutter 緑バー ②branch メニュー「● main」+ ⧉ を目視
- 次: Phase C = 分割ペイン + 下ドック + 統合ターミナル（alacritty_terminal）

## 2026-07-13 —（続き）Phase C 前半: 統合ターミナル + 下ドック（M8）
- 新 crate **`terminal_view`**（crates.io `alacritty_terminal 0.26` = Zed fork 0.26.1-dev とほぼ同一 API）。移植ガイド通り。
- **設計の肝**: alacritty の `EventLoop::spawn()` が**読取スレッド + vte parser**（自前で書かない）。PTY 出力→parse→`Term`(FairMutex)→`EventListener::send_event(Wakeup)`→`UnboundedSender`→**pump（`cx.spawn`）が `next().await`**→`sync`（term.lock→renderable_content スナップショット→notify）。**タイマー無し＝idle 0%**。カーソル blink は捨てた（静止ブロック）
- **サイズ**: `TerminalSize`(impl Dimensions)。prepaint で 'M' を shape してセル幅、bounds/セル寸から行列算出→変化時のみ `term.resize` + `Msg::Resize`（PTY winsize）
- **描画**: custom Element。①bg全面 ②非デフォ bg セルの quad ③各セル `shape_line(c,size,run,Some(cell_width))`（等幅強制）④ブロックカーソル（focus=塗り/非focus=outline・下地文字は bg 色で再描画）。色=16色 ANSI 固定パレット + 256(cube/grayscale) + truecolor、既定 fg/bg はテーマ。`Flags::INVERSE/BOLD/ITALIC/WIDE_CHAR_SPACER/HIDDEN` 対応
- **入力**: v1 は `on_key_down` 一本化（IME 前編集は捨てた）。`keystroke_to_bytes`＝Ctrl+英字→制御バイト・Enter/BS/Tab/Esc・矢印（APP_CURSOR で `\x1bO`）・Home/End/Del/PgUp/Dn・印字は key_char。⌘系は素通し（None）
- **workspace 配線**: `terminal: Option<Entity<TerminalView>>` 遅延生成（初回表示・cwd=プロジェクトルート）。`⌘J` / 下ドックボタン / × で開閉（`toggle_terminal`＝生成+フォーカス）。`render_center` を flex_col 化しエディタ列の下に積む（サイドドックに被らない）。`apply_theme` がターミナルにも波及。Drop で `Msg::Shutdown`
- 学び/罠: `gpui::outline(bounds, color, BorderStyle)` は3引数。`event_loop.spawn()` の JoinHandle は detach（型名 `State` を書かずに済む）。`tty::Options { .., ..Default::default() }` でフォーク差分フィールドを吸収。struct リテラルは記述順評価＝`exited: notifier.is_none()` は `notifier,` move の前にローカルへ
- 結果: 警告 0・**test 32 suite ok**。offscreen で **実シェル**（`Last login… / daichi@… shirushi % / ブロックカーソル`）を目視＝PTY・グリッド・cwd 全て動作
- 次: 分割ペイン（M3 残り）→ Phase D = LSP（rust-analyzer・M7）→ MCP サーバ（#21）

## 2026-07-13 —（続き）Phase D: LSP 診断（rust-analyzer・M7）
- `lang/src/lsp.rs`（`pub mod lsp`）= 最小 LSP クライアント。**JSON-RPC 封筒は自前・型は必要分だけ手書き**（lsp-types のバージョン差異回避）。transport = `std::process` + **読取スレッド**（Content-Length フレーム parse）+ `futures::channel`（oneshot=応答相関 / unbounded=通知）で GPUI 前景へ。stdin は `Mutex<ChildStdin>`。サーバ→client 要求には -32601 を返す
- lifecycle: `initialize_request`→（pump で応答 await）→`initialized`+`did_open`→編集で `did_change`（FULL）。**診断は行番号だけ使うので UTF-16 変換不要**
- **editor_view**: `diagnostics: Vec<(u32, Severity)>` + `set_diagnostics`。prepaint 行ループで診断行に**下線 quad**（error=err/warn=warn/他=fg2）を積み、テキストの上に paint。**workspace**: `lsp/lsp_root/lsp_initialized/diagnostics(map)/_lsp_pump`。open_file で rust なら `ensure_lsp`（遅延起動）、pump が publishDiagnostics を受けて map 格納 + アクティブファイル分を editor へ push。statusbar の ✗N ▲N を実件数に
- **罠1（致命）**: `~/.cargo/bin/rust-analyzer` は **rustup プロキシ**で、GUI 起動（cwd=/tmp・RUSTUP_TOOLCHAIN 無し）だと "Unknown binary in stable" で応答せず。cargo test では 1.95.0 が設定され通っていた → **`~/.rustup/toolchains/*/bin/rust-analyzer` の実体**を探して起動に修正
- **罠2**: editor の `observe` は focus/blink の notify でも発火 → 初期 `lsp_sent_version=MAX` と version 不一致で **initialized 前に didChange** を送り ra が落ちる → `on_editor_changed` を `lsp_initialized` でゲート
- 検証: transport の unit test 3 + **実 ra の ignored test 2**（initialize handshake で completionProvider 受領 / `/tmp/lsp-test` で診断「[2行] expected expression」を 2s で受信）。app では **行2/3 に赤下線 + statusbar ✗4** を offscreen で目視
- 次: LSP 補完/hover/定義の UI 配線（transport 済）→ MCP サーバ（#21）→ 分割ペイン（M3）

## 2026-07-13 —（続き）LSP 補完+定義（M7 受入）+ MCP サーバ（#21）
- **補完/定義（M7）**: editor_view に `cursor_lsp_position`/`reveal_lsp_position`（byte↔UTF-16）・`caret_window_position`・`apply_completion`（識別子プレフィクス置換）。workspace: `go_to_definition`(F12)＝Location/LocationLink 解析→別ファイルは開いて中央へ（**`window.window_handle().downcast::<Workspace>()` + `WindowHandle::update` で非同期タスクから window 取得**）。`trigger_completion`(Ctrl-Space)＝キャレット直下ポップアップ（種別バッジ+detail・上下/Enter・Tab/Esc・textEdit→insertText→label）。parser の unit test 2。offscreen で補完ポップアップ + 診断赤下線を目視
- **MCP サーバ（#21・差別化の核）**: `shirushi mcp [root]`（`crates/shirushi/src/mcp.rs`・`mod mcp`）。MCP 標準の **stdio・改行区切り JSON-RPC**（Content-Length ではない）同期ループ。`initialize`/`tools/list`/`tools/call`。ツール = `list_files`/`read_file`/`write_file`/`search`/`git_status`（project/search を reuse）。config CLI と同じ「GUI を開かず処理して return」の口。unit test 2 + **実 stdio smoke**（initialize→serverInfo・list_files→ファイル列）。ライブ GUI 制御（起動中の窓へ「開く」）は IPC ソケットが要るので後続
- 学び/罠: 補完 popup はフォーカスを取り上下/確定/中止（typing で閉じて再トリガ＝v1）。テストの scratch dir は tag 付き（cargo test は並列で共有 dir を削除し合う）
- 結果: 警告 0・**test 32 suite ok**。M7 受入（補完+診断）達成・MCP は AI エージェントから叩ける
- 次: 分割ペイン（M3 の最後の残り）

## 2026-07-13 —（続き）分割ペイン（M3）— 「全部やりきる」完了
- **低リスク方針**: `self.editor`(18箇所) を全面リファクタせず、主ペイン=`editor`（LSP/保存/診断/カーソル 全機能そのまま）+ 右分割=`split_editor`（独立エディタの比較・参照ビュー）。`⌘\` でトグル（開=現ファイルを独立バッファで複製・focus / 再押下=閉じる）。主ペインを閉じると分割も畳む
- `render_editor_pane(editor, is_split)` を主/分割で共用。`render_tabstrip` に `is_split` 追加＝ElementId を `(name, pane)` で一意化・× は分割なら `close_split`。`render_center` は `editor` があれば flex 行 `[主 | 仕切り | 分割?]`。下ドックはその下
- 開発フック `SHIRUSHI_SPLIT=1`。offscreen で theme_core.rs の左右2枚（各タブ+×・仕切り線）を目視
- 結果: 警告 0・**test 32 suite ok**
- **★「全部やりきる」完了**: Phase A（⌘1-9/新窓・root上ブラウズ・検索パネル・テーマセレクタ）→ Phase B（git色・gutter diff・branch/worktree）→ ターミナル → LSP（診断+補完+定義）→ MCP サーバ → 分割ペイン。ROADMAP の実装可能な残りを全消化。**残りは人の手番の対話検証のみ**（M1 窓確認・M2 IME/編集保存の一往復・LSP 補完/定義の live 体感・worktree 並行運用の体感）

## 2026-07-15 — ギャップ分析 → ROADMAP M10〜M13 策定 + 文書同期
- やったこと:
  - **完成までのギャップ分析**: 全 18 crate（約1.8万行・test 97 本）の実装棚卸しと docs/research（feature-matrix の 13 レイヤ）を突合。結論 =「レイヤは 13/13 に点が打ってあるが“所作”の層が薄い」。実使用の五大壁: ①複数タブ無し（1ペイン=1ファイル）②⌘F/置換 UI 無し ③補完が手動トリガのみ ④ファイル監視無し ⑤hot exit 無し。quick win: hover（`lang/src/lsp.rs` 実装済み・未配線）・`.shirushi` 色（`ProjectIdentity` 型定義済み・未配線）
  - **ROADMAP に M10〜M13 を追記**（/goal 消化用・各項目に受入条件つき）: M10 毎日使える（ドッグフーディング開始）→ M11 言語×Git parity → M12 AI の唯一無二（色×並行×信頼）→ M13 公開準備。**M9 の未チェック残件は M13 へ移動**（日常機能優先の判断）。FEATURES の later タグ一部（checkpoint/@mention/⌘K/Todos）を M12 に前倒し採用（タグ自体は不変更）
  - **CLAUDE.md を現状と DECISIONS §5 に同期**: ①「Zed GPL crate 移植可」→「移植禁止・手法参考のみ」（旧記述は GPL-3.0 決定時代の残骸）②gpui は path 依存 → git rev 固定済みへ ③マイルストーン一覧を M13 まで更新・順序の正を ROADMAP に統一
  - **ライセンス確定（本人判断・同日）**: **AGPL-3.0 で確定**（park→確定・「最終的に Apache」は撤回。理由: 単独コミッタ＝Apache の利得が効かない・クローズドコピーへの心理的抵抗・remote/cloud 展開に §13 が効く。DECISIONS §5 に追記）。鉄則（Zed GPL crate 移植禁止・貢献時 CLA）は「再ライセンスの自由の保全」として維持
- 学び/罠:
  - **7/13〜7/15 の実装が JOURNAL 未記載**（git 基礎操作+ソース管理パネル・git graph・gitignore dimming・Host 抽象+Remote SSH+多言語 LSP+GitHub 連携=M9・アプリアイコン/マスコット・スレッドタブ Chrome 風・D&D @メンション・AI コミットメッセージ・レールのアクティビティバー化・既定エージェント設定・CI 署名リリース）。詳細は git log 参照。**セッション終わりの日誌追記を忘れない**
  - i18n は仕組み（t!/parity テスト）だけ先行し locales は 5 キーのみ = UI 文字列はほぼハードコード日本語。規律はあるが回収は M13
  - FEATURES.md のチェックボックスは実装済み分も未チェックのまま（あれはバックログでタグはユーザー管理 = 触らない）。実装状況の正は ROADMAP+コード
  - **Todo ボード発案（本人）→ M12 に追記**: エディタ本体に人間所有の Todo 板（`.shirushi/todos.md` が真実・▶ でスレッドへ送信・エージェントが完了時に自分でチェック→watch で板に即反映）。= /goal+ROADMAP の開発フローの製品化。エージェント内部 Todos とは別物として共存
- 次: /goal は M10 先頭「複数タブ」から。着手時に Pane/Item の型契約（multibuffer 前提・FEATURES §1）を ARCHITECTURE に一筆入れてから実装する

## 2026-07-16 — M10-1 複数タブ（1ペイン=1ファイルの撤廃）
- やったこと（M10 先頭・ドッグフーディング最大の壁）:
  - **データモデル**: `workspace.rs` の `editor: Option<Entity<EditorView>>`（+単一 `_editor_observation`）を **`tabs: Vec<EditorTab{path,editor,_observation}>` + `active_tab: usize`** に置換。`active_editor()`/`active_tab_path()` アクセサで従来 `self.editor` を読んでいた ~30 箇所を一様に移行（借用が素直・call-site の変更は最小）
  - **タブ操作**: `select_tab`（クリック/⌘{⌘}）・`close_tab_at`（⌘W/×＝active を隣へ寄せる・閉じたら履歴へ積む・LSP didClose・最後の1枚で分割も畳む）・`move_tab`（ドラッグ並べ替え・index 追従）。ロジックは `agent_panel` の `remove_thread/move_thread` と同型（実績あり）。**重複オープンは既存タブへ切替**（`open_file` 冒頭で `position(|t| t.path==path)`）
  - **UI**: `render_main_tabstrip`（全タブを loop・active 上線=プロジェクト色・dirty ドット・git 色貫通・× 閉じる・`DraggedEditorTab` でドラッグ並べ替え）。分割ペインは単一比較ビューのまま `render_split_tabstrip`。`render_center` を `tabs.is_empty()` 分岐に
  - **LSP**: didOpen（タブ開）/ didClose（タブ閉・`lang/src/lsp.rs` に追加）追従。**`lsp_sent_version: u64` → `lsp_sent_versions: HashMap<Path,u64>`**（複数ファイルは version 番号が衝突しうる＝単一カウンタだと別ファイルの didChange を誤スキップするバグ。path 別 map で修正）
  - **永続化**: `PersistedProject`/`SavedProject`/`ProjectSlot` の `open_file: Option` → **`open_files: Vec` + `active_file`**（旧 `open_file` は読み込み後方互換のみ・`files()` で移行）。bin は `RestoredTabs{files,active}` で復元（args 単一ファイル/dir/前回状態）。非アクティブプロジェクトのタブは slot に記録して**遅延復元**（レール切替・ブランチ切替は `open_slot_files` で開き直す）
  - **キー**: `cmd-}`/`cmd-{`（=⌘⇧] ⌘⇧[）を `SelectNextTab`/`SelectPrevTab` に。Zed の `pane::Activate*Item` と同字面（gpui は `}`/`{` を shift 込みの key として解釈）。⌘⌥←→ は AI スレッドタブが既に使用＝衝突回避
  - **ARCHITECTURE §3**: Pane/Item 初版の型契約を明記（`EditorTab` の Vec から始め、多態は必要時に `enum PaneItem→trait TabItem` へ育てる。multibuffer 本体は later）
  - **検証フック**: `SHIRUSHI_OPEN_TABS=a,b,…` で起動時にアクティブプロジェクトへ複数タブを開く（offscreen 検証用・`pub fn open_paths`）
- 検証: `cargo check --workspace` 警告0・`cargo test --workspace` 全 green（回帰なし）。offscreen で **5 タブ描画**（active=lsp.rs の下線・各タブ ×）・**state.json 往復**（open_files 5件・active_file:4）・**無引数再起動で 5 タブ復元**（active 追従）を目視（font-kit でグリフも写る）
- 学び/罠:
  - **単一 `lsp_sent_version` は複数タブで壊れる**: buffer version はファイル毎に 0 から増えるので別ファイルが同じ番号を持ちうる → 「送信済み version と一致＝スキップ」で他ファイルの didChange を握り潰す。**path 別 map が正**。複数タブ化で最初に踏む地雷
  - **タブ列の Vec 化は accessor で吸収**: `editor: Option` を読む箇所が多い（~30）が、`active_editor()->Option<Entity>` を1本足せば `let Some(e)=self.active_editor() else{...}` へ機械的に移行でき、借用エラーも出ない（Entity clone は cheap）。大リファクタでも call-site を荒らさない定石
  - **タブ操作ロジックは agent_panel と完全同型**: `close_tab_at`/`move_tab` の active-index 追従は `remove_thread`/`move_thread` のコピー。既に live 実績のあるコードを写すのが安全（Chrome 風スレッドタブが先に育っていた恩恵）
  - **構造的状態分離**: 「状態が混ざらない」はタブ毎に独立した `Entity<EditorView>`（undo/カーソル/スクロール/診断を各自保持）で構造保証。offscreen で描画・往復・復元まで確認したので、残りは編集→保存→× の対話体感のみ（人の手番）
- 次: M10 の 2 番目「**⌘F バッファ内検索/置換**」（インライン検索バー・`search` crate 再利用・⌥⌘F 置換・全置換は 1 Transaction）。または「補完の自動トリガ」（`.`/`::`/識別子で自動ポップアップ）。依存順では ⌘F が先

## 2026-07-16 —（続き）M10-2 ⌘F バッファ内検索/置換
- やったこと（M10 の 2 番目・エディタの「所作」の核）:
  - **search crate**: `SearchQuery::find_in(text, max)` 新設 = テキスト全体を `find_iter` して byte レンジだけ返す（`search_text` と違い**行分割しない**＝改行跨ぎの literal も見つかる。上限打ち切り付き）。既存の regex/大小トグル基盤をそのまま再利用
  - **editor_view**: ①`search_ranges: Vec<Range>` + `set_search_ranges`（同値なら no-op = observe 再入で notify しない）②prepaint の行ループで **`ranges_overlapping_line`（二分探索・unit test 付き）** から warn 16% 面の quad を積み、**選択面の下**に paint ③`select_byte_range` = 範囲選択 + pending_reveal ④**pending_reveal を賢く**: 対象行が可視域に丸ごと入っていればスクロールしない（インクリメンタル巡回で画面が跳ねない。F12/検索ジャンプにも効く改善）⑤`PositionSnapshot`（選択+scroll_top の不透明型）= `position_snapshot`/`restore_position` ⑥`replace_ranges` = `Buffer::edit` 複数レンジ（**1 Transaction = undo 一発**）
  - **workspace**: `BufferSearchState` + エディタ右上のフローティングバー `render_buffer_search_bar`（▸/▾ 置換行開閉・クエリ/置換欄・n/m・Aa・`.*`・‹›・×。アクティブ欄 = アクセント枠 + 末尾キャレットバー）。キー処理は検索パネル/git パネルと同じ手書き流儀（escape/enter/⇧enter/⌘enter/tab/backspace/⌘V/印字）。`refresh_buffer_search` は **(バッファ version, クエリ, 大小, 正規表現) タプルで再計算ガード**（editor observe は blink でも発火する = M10-1 の地雷の応用）。現在マッチは「anchor 以降で最初」= partition_point（末尾で先頭へ回る）。**1件置換は挿入末尾+1 を anchor に再計算**（置換文字列がパターンに再マッチしても足踏みしない）。全置換は表示上限(20k)と独立に `find_in(usize::MAX)` で全件。タブ/プロジェクト切替・タブ閉じでは `dismiss_buffer_search`（マッチはエディタ毎の状態なので持ち越さない）
  - **配線**: keymap に `cmd-f`/`cmd-alt-f`（global 節）。locales に search.* 13 キー（ja/en 両方・parity green）。開発フック `SHIRUSHI_BUFFER_SEARCH=<query>`（+`SHIRUSHI_BUFFER_REPLACE`）で offscreen 撮影可。UI-SPEC §5 にバー仕様・§9 キー表に ⌘F/⌥⌘F/⌘⇧F を追記（文書が先）
- 検証: `cargo check --workspace` 警告0・`cargo test --workspace` 全 green。offscreen: 「render_」で **1/54 + 置換行**（クエリ種込み・アクセント枠・Replace/Replace all ボタン）、「resizing」で **1/12 + 他マッチの琥珀ハイライト + 現在マッチ選択が中央 + statusbar 545:13 追従**を目視。**置換→undo の対話一往復は人の手番**（複数レンジ edit + undo は editor_core の既存 test が保証）
- 学び/罠:
  - **ハイライトの再入ループは「同値 no-op」と「キー付きガード」の 2 段で切る**: refresh → set_search_ranges → notify → observe → refresh… は (version,query,トグル) ガードで止まるが、set_search_ranges 側も同値 no-op にしないと無駄描画が残る
  - **`ranges_overlapping_line` の境界**: start 昇順・非重複なら end も昇順 = partition_point が使える。「range.end == 行頭」は含まれるが零幅 quad ガード（x_end > x_start）で無害。テストの期待値を書き間違えて 1 回落とした（10..20 は end=9 の行に重ならない）
  - **1件置換の anchor は「挿入末尾+1」でなく「挿入末尾」**: `range.start + replacement.len()` を anchor にすれば、置換結果がパターンに再マッチしても partition_point が次のマッチを拾う（同位置で足踏みしない）
  - **クロージャに `&replacement` で渡せば move されず後段で使える**（`editor.update(|e,cx| e.replace_ranges(&[..], &replacement, cx))` → 直後に `replacement.len()`）。String をうっかり move すると借用エラー
  - **バーの t! はこのシェルだと英語で出る**（LANG が en → init_from_os_locale が en 解決）= i18n 配線が効いている証拠。既存 UI のハードコード日本語との混在が可視化された（M13 回収の実感）
  - IME 変換はバー入力欄では不可（key_char 手書き方式 = ⌘⇧F/git パネルと同じ制約）。日本語クエリは ⌘V 貼り付けで可。入力欄の EditorView(plain) 化は後続の共通課題
- 次: M10 の 3 番目「**補完の自動トリガ**」（識別子/`.`/`::` で自動ポップアップ・Esc 直後は同語で再表示しない）。その次は hover 配線（`lang/src/lsp.rs` 実装済み・未配線）

## 2026-07-16 —（続き）同期ブロッキング監査 + ローカル DB 方針決定（実装なし・調査と文書化のみ）
- **ローカル DB = Turso 採用（本人指定）**: hot exit / スレッド永続化 / トークン台帳 / checkpoint メタに限定。薄い `storage` crate に隔離・rusqlite 退避可能に。ARCHITECTURE §7 / DECISIONS 決定ログ / ROADMAP 該当4項目に反映済み。自動アップデートも自前（GitHub Releases + 署名検証・velq/karui 同型）で確定
- **同期ブロッキング監査（本人依頼「詰まりやすいので見て」）**: Host trait は完全同期・remote は 1 呼び出し最大 REQUEST_TIMEOUT=30s ブロック。UI スレッドから直接呼んでいる箇所を棚卸し:
  - **済（確立パターン）**: git push/pull・PR・AI コミットメッセージ（bg + git_busy）/ LSP 全経路（読取スレッド+channel）/ 横断検索 / gutter diff（250ms デバウンス+bg）/ ターミナル / ACP。パターン = `host.clone()` → `background_executor().spawn` → 前景反映
  - **S1（render 内 FS/RPC・最悪）**: エクスプローラの**アイコン表示・カラム表示が render のたび `read_any_dir`**（`workspace.rs:3421` / `:3511`）。ツリー表示は `slot.rows` キャッシュ済みなのに非対称。remote だと描画毎 RPC（切断時 30s×カラム数）。→ enter_dir/切替時に読んで slot にキャッシュ（tree と同型）
  - **S2（操作時ブロック・remote 重大）**: ①⌘S 保存 = `buffer.save()` 同期 write（`editor_view.rs` save アクション。remote 切断で 30s フリーズ）②`open_file`/分割 = `Buffer::from_host` 同期 read（`workspace.rs:2120,2267`）③**`refresh_git_status()` 同期 git CLI/RPC を約15箇所から**（タブ切替・タブ閉じ・open_file・stage/commit 後…大リポジトリで 50-200ms が体感に直結）④⌘P = `all_files(5000)` 同期 walk/RPC（`:2351`）⑤ブランチメニュー開 = `git_branches`+`git_worktrees` 同期（`:948`）・`switch_branch`（checkout は秒単位ありうる）・`add_worktree`・`git_commit/stage`（フックで長引く）
  - **S3（軽微・当面放置可）**: `save_state()` 同期小 JSON 書き（タブ操作毎・ms 未満）/ `update_agent_destination` の all_files(60) / explorer クリック展開の read_dir（1回きり・local は可）
  - **規律化**: 「UI スレッドで Host を呼ばない・render 内で FS/RPC 列挙しない」を ARCHITECTURE §9 に明記
- 学び: 非同期化は「性能改善」でなく **remote では正しさ**（切断 = 30s フリーズは事実上のハング）。local だけ見ていると refresh_git_status の 50ms に気づかない
- 次: /goal は M10-3「補完の自動トリガ」から再開。**S1+S2 の async 化を M10 に 1 項目足すか提案中**（ユーザー承認待ち。hot exit の前に消化するのが筋が良い — 保存/open の async 化は hot exit のスナップショット経路と同じ形になるため）

## 2026-07-16 —（続き）M10-3 補完の自動トリガ（type-through + クライアント側絞り込み）
- やったこと: `EditorInputEvent::Typed`（editor_view・確定入力のみ emit）→ workspace がタブ毎 subscribe → `classify_completion_trigger`（識別子/`.`/`::`）で自動 `request_completion`。ポップアップは type-through 化（印字キー=エディタへ挿入+絞り込み継続・backspace=prefix 縮小・非印字=閉じる）。絞り込みは `filter_completion_indices`（大小無視前方一致・クライアント側=LSP 再要求なし）。Esc 抑止（語頭 offset 記憶）・世代番号で古い応答破棄・prefix/位置は応答時点のキャレットで確定
- 検証: 全 test green（+trigger 分類 9 ケース・filter 4 ケース）。**実 ra で `t.the` → theme/thermal のみの自動ポップアップをキャレット直下に offscreen 目視**（`SHIRUSHI_TYPE_PROBE="row:col:text"` フック新設・LSP 初期化待ちの delay 付き）
- 学び/罠:
  - **ポップアップ位置は要求時点でなく応答時点のキャレットで**: caret_bounds は直近 paint 由来なので、要求と同フレームだと 1 文字（プローブでは数行）ズレる。応答時に active editor から取り直すと自然（要求時点の値は fallback）
  - **type-through は「ポップアップが挿入を代行」でなく「エディタに挿入させて Typed イベントに委ねる」**と一本道になる（挿入経路が popup 経由/直接タイプの 2 本あっても後段は同じ subscription が処理）
  - **`::` の 1 個目で出さない**: classify は挿入後のキャレット前 2 文字（`text_before_caret(4)`）で判定。`:` 単発は None
  - FocusHandle を要するステート構造体のテストは**ロジックを自由関数に切り出す**（zeroed FocusHandle は UB。一度書きかけて捨てた）
- 次: M10-4 hover の配線（`lang/src/lsp.rs` の hover() 実装済み・未配線 = quick win）

## 2026-07-16 —（続き）M10-4 hover の配線（dwell + ⌘K ⌘I）
- やったこと: editor_view に **dwell 検知**（mouse_move → 500ms タイマー・世代番号・`EditorHoverEvent::Dwell/Cancel`。Cancel = クリック/スクロール/30px 超離脱）。workspace がタブ毎 subscribe → `lang::lsp::hover()`（実装済みを配線）→ `parse_hover_lines`（コードフェンス行のみ除去のプレーン表示・24 行 truncate）→ アンカー直上のコード字カード（occlude・フォーカス取らない）。⌘K ⌘I（`workspace::ShowHover`）でキャレット位置にも。補完表示中は hover 抑止・タイプ/タブ切替で消す
- 検証: 全 test green。**実 ra で `Thing` の struct シグネチャがポップアップ表示されるのを offscreen 目視**（`SHIRUSHI_HOVER_PROBE="row:col"` フック。キャレット矩形は paint 由来なので移動後 300ms おいて発火）
- 学び/罠: **hover の生存管理は「editor が Cancel を emit」に寄せる**と workspace 側の条件分岐が消える（on_editor_changed は blink でも発火するので「変更で閉じる」には使えない — 補完と同じ地雷）。occlude でポップアップ上のマウスは editor に届かない＝ポップアップに乗っている間は消えない、が 1 行で手に入る
- 次: M10-5 ファイル監視（notify/FSEvents・watch 基盤）

## 2026-07-16 —（続き）M10-5 ファイル監視（watch 基盤）
- やったこと: `project::watch_root`（notify 6.1/FSEvents・opaque `Watch` で型を封じ M13 remote 差し替え面を確保）→ workspace pump（200ms 合流）→ ①開バッファ: `disk_probably_unchanged`（len+mtime 比較で**自分の保存は無視**）→ クリーンなら `Buffer::reload`（選択クランプ・undo 履歴リセット）/ dirty なら**警告バー**（再読込/このまま）②ツリー slot 再構築 ③git 色 + `EditorView::refresh_diff`。`.git/` は index/HEAD/refs だけ合図。gitignore はノイズ除去。切替で張り替え・remote は張らない
- 検証: unit（reload 追従・クランプ・履歴リセット・無題/削除は None）+ live offscreen ×2（クリーン: echo 追記 4 行が自動反映 + newfile.rs が U でツリー出現 + 緑 gutter / dirty: ⚠バー表示・外部行の混入なし・ユーザー編集温存）
- 学び/罠:
  - **「動かない」の犯人は自分の古いバイナリ**: cargo check だけして build せず `./target/debug/shirushi` を実行 → watch 実装が入っていない旧バイナリで 2 回無駄撮り。**スクショ検証の前は必ず build**（check≠build）。デバッグ用 `SHIRUSHI_WATCH_DEBUG` は今後も有用なので常設化
  - **外部変更判定は content_hash 抜きの len+mtime で足りる**（FileRevision の hash は読まないと出ない。自分の保存を外部変更と誤認しない、が目的なので安価比較が正）
  - **undo 履歴は reload でリセット**が正（外部変更を跨ぐ undo は嘘の状態を作る。VSCode は保持するが v1 はリセットを選択）
  - futures の `try_next` は deprecated → `try_recv`（返り値は `Result<T, TryRecvError>` で `Option` が剥がれてる）
- 次: M10-6 = ROADMAP に追加した「UI スレッド非ブロッキング化」（監査 S1+S2・承認済み）

## 2026-07-16 —（続き）M10-6 UI スレッド非ブロッキング化（監査 S1+S2 の回収）
- やったこと（承認済み・ROADMAP 化した監査項目の実装）:
  - **S1**: render 内 `read_any_dir`（アイコン/カラム表示・毎フレーム FS/RPC）→ `ProjectSlot::listed_dir` = RefCell<HashMap> キャッシュ。初回 render だけ読み、watch の `refresh()` で無効化（= FS 変化で取り直し）
  - **S2 保存**: `Buffer::prepare_save`（host/path/全文/競合条件/version の snapshot）→ 背景 `PendingSave::write` → `complete_save`（**保存開始時 version と一致した時だけ dirty を下ろす** = 書き込み中の編集を失わない）。editor_view の ⌘S ハンドラを spawn 化
  - **S2 open**: `open_file` = 背景 read → `open_loaded_file` 合流（読み込み中の重複 open は再 dedup）。**起動復元は `open_file_sync` を温存**（背景だと完了順でタブ順が崩れる）
  - **S2 git**: `refresh_git_status` = 背景 + 世代番号（status・branch・パネル用 changes/log/slug を一括）。ブランチメニュー列挙・switch（busy）・worktree add（busy）・commit（busy・stage 込み）・stage/unstage/branch 作成/削除（`run_git_index_op` 共通化）を全部背景へ。⌘P は新設 `project::all_files_on` で背景化
- 学び/罠:
  - **Rc<Worktree> は Send できない** → 背景へは `host.clone() + root.to_path_buf()` を渡し `*_on` 自由関数を呼ぶ（この形にするために M9 で全 git fn に `_on` 版が既にあったのが効いた）
  - **保存の非同期化は「version 比較で dirty を残す」が肝**（無条件に dirty=false にすると保存中のタイプが「保存済み」の顔をする）
  - **復元経路まで非同期化すると順序が壊れる**（タブ列 = Vec push 順）。対話経路だけ async、復元は同期のまま、が正しい切り分け
  - futures `try_next` → `try_recv`（deprecated 対応・Option が剥がれる）
- 検証: `cargo check --workspace` 警告 0・`cargo test --workspace` 全 green。offscreen（複数タブ + icons ビュー + git 色）撮影済み — 目視は Read ツール復旧待ち（インフラ断・Bash は生存）
- 次: M10-7 hot exit（Turso `storage` crate 新設）

## 2026-07-16 —（続き）M10-7 hot exit（Turso 初導入・crates/storage 新設）
- やったこと: **`crates/storage`** = Turso 0.7（決定どおり）。async API を**専用ワーカースレッド + チャネル + block_on** で包み、外へはブロッキング API（GPUI に runtime を持ち込まない・呼び出しは background executor から）。Turso 型は crate 外に出さない（rusqlite 退避面）。hot_exit テーブル（path PK・全文・saved_at）。workspace: 2s デバウンス背景スナップショット・復元/破棄バー・⌘Q でクリア・`SHIRUSHI_DB`/`SHIRUSHI_HOTEXIT_DEBUG`/`SHIRUSHI_HOTEXIT_AUTORESTORE` フック
- 検証: storage unit ×2（turso 0.7 が**初回コンパイルで API 一致**・round trip・drop→再オープン残存）。**sqlite3 CLI で .tables が読めた** = SQLite 互換フォーマット実証。live: タイプ→tick→**kill -9**→dump で全文残存→再起動「復元候補 1 件」→自動復元→dirty 再スナップショット、をログで一部始終確認
- 学び/罠（3 連発・全部「検証で見つけて直した」）:
  - **blink の notify がデバウンス世代を永遠に流す**: 2s デバウンスは「2s の静寂」が前提だが、focus 中は blink が 530ms 毎に notify → 世代が無限に進んで tick が一度も発火しない。**バッファ version ガード**（lsp_sent_versions と同型）が必須。M10-1 の地雷の再演
  - **再起動直後の tick が復元候補を消す**: クリーンなバッファ → 「clean=行削除」で、ユーザーが復元を決める前に候補行を消してしまう。**hot_exit_pending が Some の間は tick を丸ごと止める**
  - **テストハーネスの kill が早すぎた**: `$SECONDS` がビルド時間込みで、アプリ生存 3 秒で kill していた（2 回「書かれない！」と空騒ぎ）。**プロセス起動直後に SECONDS=0 リセット**。watch の「古いバイナリ」に続き、検証スクリプト自体を疑う教訓 2 個目
- 次: M10-8 ツリーのファイル操作（新規/リネーム/削除/複製）

## 2026-07-16 —（続き）M10-8 ツリーのファイル操作
- やったこと: project に local ファイル操作 5 種（create/create_dir/rename〔上書き拒否〕/duplicate〔name copy.ext・再帰〕/trash〔/usr/bin/trash→Finder fallback〕）+ unit test。workspace は右クリックメニュー 5 項目（local のみ表示）+ **インライン命名行**（rename=行置換・New*=親の直後に挿入・手書きキー入力流儀）。開いてるタブの rename/trash は先にタブを閉じる（旧パス保存の復活事故防止）。render_tree の行クロージャを `render_tree_row` 関数に切り出して入力行と共存
- 検証: unit green + live で命名→Enter→実ファイル生成をエンドツーエンド確認（SHIRUSHI_NAMING/SHIRUSHI_NAMING_CONFIRM）。スクショ 3 枚（メニュー/命名行/生成後）撮影済・目視は Read ツール復旧後
- 学び/罠: ゴミ箱は macOS 14+ の `/usr/bin/trash` が素直（無い環境は Finder AppleScript）。「開いてるファイルの rename」はタブ/バッファ/LSP の張り替えが必要で v1 は close-first が安全（VSCode 同等は後続）
- 次: M10-9 編集の所作一式（⌥←→・行操作・⌘/・自動インデント・括弧ペア・Tab インデント）

## 2026-07-16 —（続き）M10-9 編集の所作一式
- やったこと: editor_core に単語境界（2 クラス・256B 窓）/行移動・複製・削除/コメントトグル（1 Transaction）/自動インデント改行/インデント増減/**ペア分類 `classify_pair_input`**（Pair/Wrap/SkipOver/Insert）を実装 + unit test 8 群。editor_view はアクション 16 本 + `handle_pair_input`（入力ハンドラ介入・IME/明示レンジ/composer は素通し）。lang に `comment_prefix` 最小表。keymap 17 本追加
- 学び/罠: **ペアの誤爆防止 2 則**（クォートは単語隣接で無効 = `don't`/lifetime `'a` 事故防止・開き括弧は直後が識別子なら素通し）はロジック層でテストしておくと UI 層が何も考えなくてよい。行移動は「最終行に改行が無い」ケースの改行付け替えが唯一の罠（test が最初に落とした）。gpui の keystroke 表記は `cmd-/`・`cmd-]` がそのまま通る
- 次: M10-10 multi-cursor の UI 配線（⌘D・⌥⌘↑↓・⌥クリック・Esc 単一化）

## 2026-07-16 —（続き）M10-10 multi-cursor の UI 配線
- やったこと: editor_core に select_next_occurrence（⌘D）/add_cursor_vertically（⌥⌘↑↓）/add_cursor_at（⌥クリック）/collapse_to_primary（Esc）+ test 3 群（受入「⌘D×3 で 3 箇所同時書き換え」をそのままテスト化）。view はアクション 4 本・⌥クリック分岐・⌘D 後の reveal。描画/同時編集は M2 から全選択対応済みだったので**配線だけで完成**
- 学び/罠: Esc は「複数→1 個」だけにする（単一選択で no-op）と、検索バー/補完ポップアップの Esc と衝突しない。⌥クリックは is_selecting を立てない（直後のドラッグで選択を上書きしない）
- 次: M10-11 ナビゲーション履歴（⌃- / ⌃⇧-）

## 2026-07-16 —（続き）M10-11 ナビゲーション履歴
- やったこと: back/forward スタック（(path, offset)・上限 100）。記録 = F12 着地直前・⌘P 確定・検索ジャンプ・50 行以上のクリック（editor が CaretJumped を emit）。⌃-/⌃⇧- で往復・閉じたファイルは背景読みで開き直し。EditorInputEvent が 2 variant になったので on_editor_typed の irrefutable let を match 化
- 学び/罠: gpui の keystroke 表記は `ctrl--`（ハイフンキー）と `ctrl-shift--` がそのまま通る（起動時の全アクション解決で警告なしを確認）。「大距離クリック」は editor 側で検知して emit する方が、workspace がクリックを盗み見るより素直
- 次: M10-12 soft wrap + 行ジャンプ（着手前に設計を書く — 論理行↔表示行マップ×行仮想化）

## 2026-07-16 —（続き）M10-12 soft wrap 設計（実装前・約束の設計先行）
- **方式**: 論理行→表示行の **WrapMap** を editor_view に持つ（コアは無知のまま = 依存方向不変）
  - `segments: Vec<Vec<usize>>` = 論理行ごとの「セグメント開始 byte 列」（折返し無し行は `[0]`）
  - `prefix_rows: Vec<usize>` = 論理行 i の先頭**表示行**番号の累積（len = 行数+1・末尾 = 総表示行数）→ 表示行→論理行は partition_point・逆は prefix + セグメント内 partition_point。O(log n)
  - 再計算 = (buffer version, 折返し列数, on/off) が変わった prepaint で全行 O(総文字数)。1.6MB でも数 ms（増分化は M11 の増分パースと同輪郭で後続）
- **折返し計算は等幅前提のセル数**: ASCII=1・東アジア全角=2（`unicode-width`・既に依存木にある）。列数 = content_width / 'M' 幅（ターミナルと同じ）。**単語境界優先**（直近の空白で切る・1 語が幅超過なら文字で切る）。バンドルフォント（Guguru Sans Code）は等幅 CJK=2 セル設計なのでピクセル shaping なしで正確
- **描画**: 仮想化は**表示行**で回す。可視表示行 → (論理行, セグメント byte 範囲) を解決し、セグメント部分文字列だけ shape。行番号/診断/diff マークは**先頭セグメントのみ**。選択/検索/キャレットの x はセグメント相対 offset
- **キャレット移動**: ↑↓ は表示行単位（wrap 中の行内移動）。offset↔表示行のヘルパを EditorView に生やし、scroll/reveal/クリック位置も表示行系に統一
- **トグル**: 設定 `soft_wrap`（settings_core に追加・既定 false）+ ⌥Z（editor::ToggleSoftWrap）。off は恒等マップ（挙動不変を構造で保証）
- **⌃G 行ジャンプ**: workspace の小オーバーレイ入力（手書きキー流儀）→ Enter で reveal_position(n-1, 0)（論理行番号のまま）

## 2026-07-16 —（続き）M10-12 soft wrap + ⌃G 実装
- 設計どおり実装（上のエントリ参照）。要点: **仮想化ループを表示行で回し、セグメントの絶対 byte 範囲（line_start/line_end）を既存ロジックにそのまま流す**と、選択・検索ハイライト・IME 下線・キャレット x が無改修で正しく動く（範囲交差で書いてあったものは折り返しに強い）
- 罠: キャレットのセグメント帰属（境界 offset は次セグメント側・論理行末だけ最終セグメント）を決めておかないと折返し点でキャレットが二重描画/消失する。マップ再構築前の 1 フレーム（after_edit → prepaint 間）は version ガードで論理行フォールバック
- ⌃G はミニオーバーレイ（数字のみ受理）。ジャンプはナビ履歴（M10-11）にも積む
- 次: M10-13 設定の実効化（font_size/tab_size 配線 + ユーザー keymap.json + live reload）

## 2026-07-16 —（続き）M10-13 設定の実効化 — ★M10 の実装可能項目を全消化
- やったこと: font_size/tab_size を editor_view のフィールド化（行高 23/13 比・shape/ヒットテスト/wrap 列数まで可変）+ `apply_editor_settings` を observe_global に接続（全タブ+分割へ live 配布）。ユーザー keymap.json = 既定の後に bind（後勝ち）+ 専用 watcher で live reload（200ms 合流・再 bind）
- 検証: live で `shirushi config set font_size 19` → 起動中アプリに反映（settings watcher→observe_global→set_typography の全経路）。ユーザー keymap「1 束」適用ログ確認。全 suite green・警告 0
- **★M10 総括**: 3〜13 の実装項目を全消化（複数タブ・⌘F・補完自動・hover・watch・非ブロッキング化・hot exit/Turso・ツリー操作・所作一式・multi-cursor・ナビ履歴・soft wrap・設定実効化）。**残りは受入（総合）「Shirushi で Shirushi を丸 1 日開発」= 人の手番のみ**
- 次: PNG 目視の消化（Read ツール復旧待ちの 9 枚）→ M11 へ（フォーマット/rename/code actions/参照検索/シンボル/診断一覧/tree-sitter 多言語/増分パース/diff エディタ/hunk 操作/blame）

## 2026-07-17 — M11-1 フォーマット（⌥⇧F + 保存時）
- `Buffer::edit_batch`（異テキスト一括・1 Transaction）を新設し、LSP TextEdit 適用の共通基盤に（rename/code actions も同じ道を通る）。⌘S は workspace::SaveActive へ移し format_on_save のフック地点に。キャレットは (行,列) で復元・スクロール維持
- live: 崩れた .rs がプローブ（フォーマット→保存）でディスクごと rustfmt 品質に。応答中のタブ切替ガード付き
- 次: M11-2 rename（F2・WorkspaceEdit 複数ファイル）

## 2026-07-17 —（続き）M11-2 rename（F2）
- `apply_workspace_edit` を共通経路として整備（rename と code actions で共用）。開タブ=バッファ反映・未オープン=ディスク直書き（revision 条件付き = 競合安全）。live で 2 ファイル rename（バッファ+ディスク）を実 ra 確認
- 罠: dev フックの `cx.spawn` は Context<Workspace> だと 2 引数クロージャ（3 回目の同じ地雷 — window.update 内の cx を素の App と勘違いする）。**ビルド成功を確認してから実行**（旧バイナリ実行、これも 2 回目）
- 次: M11-3 code actions（⌘.）+ M11-4 参照検索（⇧F12）

## 2026-07-17 —（続き）M11-3 code actions（⌘.）+ M11-4 参照検索（⇧F12）
- ⌘. = 生診断（新設 raw_diagnostics）を context に渡す → 一覧ポップアップ → edit or resolve → apply_workspace_edit（rename と共通）。⇧F12 = Location[] → ファイル別集約（背景でプレビュー行読み）→ ⌘⇧F パネル再利用
- **最大の学び: ra は client capabilities を見て機能を黙って無効化する**。codeActionLiteralSupport/resolveSupport/dataSupport 無しだと codeAction は**無応答**（エラーですらない）。LSP 機能を足すときは initialize の capability 宣言をセットで疑う
- live: unused import が ⌘.（resolve 経由）→ 保存でディスクから消滅。locations_to_file_matches は unit test
- 次: M11-5 シンボル（⌘⇧O アウトライン = tree-sitter / ⌘T = workspace/symbol）+ M11-6 診断一覧 + F8

## 2026-07-17 —（続き）M11-5 シンボル + M11-6 診断一覧/F8 + 回帰修正
- ⌘⇧O = lang::outline（tree-sitter クエリ・LSP 不要）→ Picker。⌘T = workspace/symbol → Picker（空クエリ 200 件 + ローカル fuzzy）。診断一覧 = statusbar ✗▲ クリック → 検索パネル UI 再利用（メッセージがプレビュー）。F8/⇧F8 = 次/前の診断行
- **回帰発見・修正**: open_file 非同期化（M10-6）で「開いてからジャンプ」が未オープンファイルで旧バッファに誤 reveal（検索ジャンプ/F12/⌘T）。`open_file_then`（背景読み完了後にエディタへ FnOnce 適用）へ 3 箇所を統一。**async 化の追跡調査は「その後に書かれたコード」も対象**という教訓
- 次: M11-7 tree-sitter 多言語（TS/JS/Python/Go/JSON/TOML/YAML 等）

## 2026-07-17 —（続き）M11-9/10/11（diff タブ・hunk 操作・blame）— ★M11 実装項目を全消化
- diff タブ = transient タブ基盤（永続化/⌘⇧T/LSP 除外）+ Buffer::set_read_only（コアでガード）+ unified_diff_on（imara-diff UnifiedDiffBuilder）+ F7 で @@ 間移動。hunk = gutter クリック検知 → ポップオーバー（stage/巻き戻し/コピー/diff）。stage は 1 hunk パッチ生成 → `git apply --cached`（一時ファイル経由 = host 汎用）。blame = `-L n,n` porcelain + 400ms デバウンス + 行末 dim
- **受入をテストにする**流儀を継続: 「2 hunk の片方だけ stage」round trip・blame の実履歴合成・diff round trip、全部 unit/integration test 化
- ★M11: 11/11 実装完了（総合受入の通し体感のみ人の手番）。積み残しメモ: アウトラインの tree-sitter クエリは Rust のみ（他言語は ⌘T で代替）・diff タブは +/- の色なし（unified テキスト素通し）
- 次: M12。最初に **checkpoint 比較表**（約束）を出してから実装に入る

## 2026-07-17 —（続き）M12-1〜5（永続化・checkpoint・生中継・色リンク・通知）
- **実 Claude Code で end-to-end 検証**: ①送信 → turns テーブルに user/agent/step が追記・トークン実測が threads へ ②「元の内容です」のファイルをエージェントが「工業」に書換 → **checkpoint blob に変更前内容が保存** → restore → **ディスクが元に戻る**（受入の完全往復）
- 設計の勘所:
  - **checkpoint は PermissionRequest 受信時（応答前）に切る** — 手動承認と AUTO_ALLOW の一本道。answer_permission（クリック）に置くと自動許可経路が素通りする（1 回踏んだ）
  - **Claude の Write は diff に old_text を含まない** → ディスクの現内容を「書かれる前」に背景で読む。**自動許可の応答をスナップショット完了後に遅延** = エージェントの書き込みとのレースを構造的に防ぐ
  - PanelEvent（TurnEnded/PermissionWaiting/FilesTouched）で workspace 疎結合（トースト・statusbar pulse・ツリー色ドット・gutter スレッド色）
- 既知の限界（注記）: bypass モードは権限リクエストが来ない = checkpoint/色リンク対象外（リロードのみ）。Dock バッジ・タブ側ドット・±行サマリーは後続
- 次: M12-6 diff レビュー本体化 → 7 @mention Picker 化 → 8 ⌘K（キーは ⌘I 採用予定・⌘K はチョードプレフィクスと衝突）→ 9 Todos → 10 Todo ボード → 11 .shirushi 色 → 12 ⌘O 2 階層 → 13 台帳 UI
## 2026-07-17 —（続き）M12-6/7/11/13（diff タブ本体化・@mention fuzzy・.shirushi 色・Σ台帳）
- 承認カード「エディタで開く」= `PanelEvent::OpenDiffRequest` → `pending_transient_tab`（subscribe に window が無い GPUI 制約は「次の render 冒頭で消化」で迂回）。＋context は fuzzy 絞り込み（all_files 60→2000）。レール右クリック色ピッカー（12 色 → `.shirushi/settings.json` へ persist）。Σ 累計チップ = turns 集計クエリ
- 次: M12-9 Todos（プラン）→ M12-8 ⌘I → M12-10 板 → M12-12 ⌘O

## 2026-07-17 —（続き）M12-9 Todos（プラン）常設チェックリスト
- ACP schema crate（agent-client-protocol-schema 1.4.0）に `SessionUpdate::Plan(Plan)` あり（本体 crate を grep しても出ない — **v1 の型は別 crate re-export**。探すときは `agent-client-protocol-schema` を見る）。PlanEntry{content, priority, status} を UI 非依存の `PlanItem` に写して `AgentEvent::Plan`
- プランは**毎回全量置換**（ACP 仕様）→ Thread.plan を置換するだけで常設チェックリストが追従。● 進行中はスレッド色（UI-SPEC の Todos 節を「ステップ内」→「常設」へ文書先行で更新）
- 検証は `SHIRUSHI_PLAN_PROBE`（実 ACP 不要の直接注入）で offscreen 目視。status は non_exhaustive なので未知値は Pending 扱い
- 次: M12-8 ⌘I インライン編集

## 2026-07-17 —（続き）M12-8 ⌘I インライン編集（claude -p 型・live 全往復）
- **キー確定 ⌘I**（⌘K はコードプレフィクス衝突）・グローバル bind でターミナルにも効く。**方式は ACP でなく `claude -p`**（ai_commit_message と同型）: セッション起動ゼロ・権限フロー不要で「チャットへ行かない最短経路」が最短で立つ。ROADMAP の文言も方式変更を明記（文書を直す方が先）
- **shell 引用問題はファイルで殺す**: 指示+コードを一時ファイルに書いて `claude -p "固定プロンプト" < tmp`（ユーザー入力を sh に埋めない）。出力はコードフェンス剥がし+末尾改行を元コードへ正規化（LLM は末尾改行を付けがち = diff ノイズ）
- InlineEditTarget::{Editor{range,old,version}, Terminal} の 2 相。適用は apply_lsp_edits = 1 Transaction（⌘Z 一発）・version 不一致は安全側破棄。ターミナルは insert_text（改行を落とす = 実行しない）
- live: 「Result<u16, String> を返すように」→ busy 表示 → **-/+ diff プレビュー**（unified からヘッダ落とし・@@→···・中央省略）→ 適用+保存 → ディスク書換を確認（`SHIRUSHI_INLINE_PROBE`/`SHIRUSHI_INLINE_ACCEPT`）
- 謎が 1 件: 初回走行のみ「提案が承諾前に適用されたように見えた」（ディスク不変・以後 3 走で再現せず）。監視中
- 次: M12-10 Todo ボード

## 2026-07-17 —（続き）M12-10 Todo ボード（板の一部始終を live 実証）★AI の唯一無二の本丸
- `project::todos`: 行番号保持パース + 「該当行の [ ]↔[x] だけ書き換え」（他は 1 バイトも動かさない）。設計は settings と同じ「**ファイルが真実・UI/CLI/AI は全部ただの書き手**」— 反映は watch 任せにすることで書き手が何人いても板が正しい
- **live 実証（全部実 Claude）**: ▶ 送信（「完了したら [x] にせよ」自動付与）→ Claude が parse_port を 92 行改善 → **自分で todos.md をチェック** → watch → **板のチェックがひとりでに入る**（4→3・pulse 解除）→ git diff に `- [ ]`→`- [x]` の一部始終。外部プロセス追記の watch 反映・✨今日の計画（claude -p → 今日見出しへ追記）も live
- ✨の学び: `claude -p` は agentic にファイルを読みに行き前置き+注記を混ぜてくる → プロンプトで「ファイルは開かない・タスク行のみ」+ `parse_plan_lines`（。終わり/※/60字超を捨てる）の**二段防御**。実測出力をそのまま unit test に
- 権限待ちで 1 回目 110s では足りず（Model fallback: fable-5 が cyber 判定で decline → opus-4-8 再試行という珍事も観測）。2 回目 280s で完走
- 残: 逐次消化モード（checkpoint 済で解禁条件は満たす・UI トグルは次スレッド）・手動チェックの対話確認は人の手番
- 次: M12-12 ⌘O 2 階層

## 2026-07-17 —（続き）M12-12 ⌘O 2階層 + worktree ダッシュボード — ★M12 全消化
- UI-SPEC §7 の解釈: 「2 階層」= 画面遷移でなく**1 リストにプロジェクト行+配下 ⎇ 行をインデントで並べる**（一望が受入の本質なので遷移させない）
- 「どこで何が走っているか」の実体 = **`RunningRegistry`（GPUI Global）**: 各窓の AgentPanel が dest_cwd（= その窓の worktree root）キーで (名前,色,running) を上書き。全窓横断の台帳を ⌘O が読む。書き手は窓ごとに排他なので lock 不要
- 開く速さを守る 2 段構え: 即プロジェクト行のみで開く → bg で全 project の `git worktree list`+`git status --short --branch` → `Picker::set_items` 差し込み（Picker に set_items/accent/dots を追加）
- live: 実 Claude 実行中に ⌘O → `⎇ main ✓●` に実行中スレッド色ドット・`⎇ feature ●`(dirty) を offscreen 目視
- **M12 これで全項目チェック**（総合受入は機構完成・3 worktree 並走の体感は人の手番）。次: M13（⌘⇧P・i18n 回収・ベンチ・自動更新・Linux・初回体験）
## 2026-07-17 —（続き）M13-1/2（⌘⇧P パレット・i18n 全回収）
- パレット = `command_entries()` 登録表 + Picker 再利用。**確定は「閉じてから dispatch」**（フォーカスがエディタへ戻った後に `build_action`→`window.dispatch_action` = keymap と同じ解決経路）。live で Terminal コマンド確定 → 実シェルが開くのを実証
- キー併記は `key_for_action` 逆引き + `pretty_keystroke`（⌘⇧P 記号化・unit test）。ユーザー keymap の上書き反映は既定 keymap 固定（軽微な乖離・残件）
- i18n 回収は **98 箇所を Python 一括置換**（完全一致ペア + 一意性 assert）→ 型エラーだけ手直し（&'static str 前提のクロージャ/タプルを String 化）。**教訓: リテラル→t! の一括置換は「取り違え」より「型」で落ちる**ので、コンパイラを検証器に使うのが速い
- ロケール依存になったテスト 1 本（「他 16 行」）は数字ベースの検証へ。en 実機スクショで Source control / Send ⌘⏎ / Terminal 1 を確認
- 次: M13-3 ターミナルリンク

## 2026-07-17 —（続き）M13-3/4/5（ターミナルリンク・自動更新・welcome/ベンチ/CI）— ★M13 実装分を全消化
- **ターミナル file:line**: リンク検出は prepaint（セル→行再構成 + 手書きパーサ）・クリックは **paint 内 `window.on_mouse_event` 登録**（TerminalView への layout 書き戻し不要 = 再入の心配なし）。`--> sample.rs:28:9` 下線 + 28 行目着地を offscreen 実証。IME は `EntityInputHandler` 最小（確定→PTY・selected_text_range が Some でないと IME セッションが始まらない点に注意）
- **自動更新**: Apple 公証済み dmg なので**署名検証は `spctl --assess` に委ねる**（自前 ed25519 鍵の配布・管理が丸ごと消える）。差し替えは hdiutil+ditto（実行中 .app 置換可）。GitHub API は curl 委任 = 依存ゼロ。実 Release での E2E は初回リリース時
- **ベンチ**: criterion を入れず examples + Instant 直測（**予算超過 exit 1 を CI ジョブに**）。編集コアは全項目 ~1µs。key→frame は GUI 実機が要るので CI 外（残件）
- **CI**: ci.yml 新設（mac test + コアベンチ・linux check は continue-on-error で追従待ち）
- **M10〜M13 の実装項目をこれで全消化**。残 = 実環境必須群（Remote 障害注入/実機受入/Linux 実行/実 Release E2E/key→frame）と対話確認（⌘I ターミナル・板の手動チェック・3 worktree 並走の体感）
## 2026-07-17 —（続き）terminal-stack 文書の §4 消化 + titlebar ピル余白
- **⌘P 実測**（bench_fuzzy 新設）: zed 4,194 件 ~0.85ms・50k 合成 ~10ms/refilter = 1 フレーム内 → **in-process 続行を確定**し、列挙上限 5,000→50,000 へ（fuzzy がボトルネックでなく列挙 limit がボトルネックだった）。CI には載せない（ui は GPUI 依存でビルドが重い — editor_core ガードのみ CI）
- **メモリ実測**（memory-usage.sh 新設）: idle RSS 122MB・起動 ~215ms → CLAUDE.md の性能予算に数字として記録（terminal-stack 層への武器）
- titlebar のプロジェクトピル: gap 7→11 / px 9→11・ブランチ側に px7/py2・⎇ を fg2 で分離 = 詰まり解消（ユーザー指摘）
## 2026-07-17 —（続き）UX 直し（transcript スクロール・titlebar・SSH の GUI 導線）
- **transcript がスクロール不能だった**（overflow_hidden のまま）→ `overflow_y_scroll` + ScrollHandle。追従は「**底に居る時だけ** scroll_to_bottom」（遡り読み中は動かさない・offset.y は下スクロールで負）。初期表示/復元/スレッド切替も末尾へ
- テキストのドラッグ選択は GPUI の素のテキストでは不可（zed は独自選択実装）→ **エントリ hover の ⧉ コピー**で代替・本文選択は残件
- **アイコンが出ない罠**: `Assets::load` は明示 match 表 — SVG を assets/ に置くだけでは**出ない**（square-check も登録漏れで、レールの ☑ は空ボタンだった）。「SVG 追加 = match に 1 行」をセットで
- titlebar: ピル左縁の 3px 色チップ廃止（色はレール等の許可箇所に集約）・信号機との間 78→92px・dock ボタン gap 2→6
- **SSH の GUI 導線**: titlebar 右に server アイコン → 入力バー（ssh://…）→ 背景で ControlMaster+server 配備 → **新窓で開く**。失敗はトースト。system OpenSSH 委任なので ~/.ssh/config（エイリアス/鍵/ProxyJump/agent）がそのまま効く
## 2026-07-17 —（続き）composer の折り返し（plain = 常時 wrap へ）
- **composer で長文が右へはみ出す**: WrapMap の構築条件が `soft_wrap && !plain` — plain（composer）は M10-8 時点で意図的に対象外だった。**plain は常時 wrap** に変更（`soft_wrap || plain`・ガター無し幅で columns 計算）。描画/座標変換（offset_for_position）は元から wrap_map 経由なので条件 1 箇所の修正で全部整合
- composer のフォントは Sans（プロポーショナル）なので 'M' 幅の等幅見積もりはややズレる → はみ出しより**早折れ**の安全側で許容（目視 OK）
- ドラッグ複数行選択: エディタ/composer は is_selecting 機構で実装済み（wrap 対応も確認）。**transcript は GPUI に選択プリミティブが無く**（InteractiveText はクリック/ホバーのみ・Zed の markdown 選択は GPL 独自実装）自前実装が要る = 残件。当面は各エントリ hover の ⧉ コピー
- `SHIRUSHI_COMPOSER_PROBE` 新設（composer へ下書き流し込み → offscreen 目視）
## 2026-07-17 —（続き）transcript ドラッグ選択（自前実装）+ ブランチ切替 fallback + レール＋
- **transcript のドラッグ選択をやりきった**（GPL 移植なしの自前）: 素材は GPUI の `StyledText`（`with_highlights` = 親スタイル継承で範囲背景だけ変えられる）と `TextLayout`（`index_for_position` が**絶対座標→byte** のヒットテスト・`bounds()` 付き・Rc 共有で render 後も引ける）
- 構造: render 毎に `SelectableRegion { entry, text, layout }` を registry へ再構築 → コンテナ 1 箇所の mouse down/move/up で **エントリ跨ぎ選択**（隙間は直前リージョン末尾へ丸め）→ 選択は各エントリの highlight 背景（**スレッド色 30%**）→ **⌘C** はパネルルートの on_key_down で「選択がある時だけ」composer より先に拾って stop_propagation。Esc/外クリックで解除
- 対象は User/Thinking/Agent の本文（Step の result はラベル混在のため対象外 = ⧉ コピーで代替）。offset は index_for_position 由来なので char 境界保証（プローブ注入時だけ boundary に注意）
- 検証: `SHIRUSHI_TRANSCRIPT_SEL_PROBE` で entry0:3〜entry4:9 を注入 → **327 bytes のエントリ跨ぎコピー**（Step スキップ・空行区切り）と**行単位のハイライト描画**を offscreen 実証
- **ブランチ切替の git 仕様バグ**: 他 worktree にチェックアウト済みのブランチへの `git switch` は fatal（'feature' is already used by worktree）→ 事前に worktree 一覧を見て、**あればその worktree を開く**（レールに居れば切替・無ければ新窓 = ⌘O と同経路）へ倒した。エラーは eprintln → トーストへ格上げ
- **レール ＋ をネイティブのフォルダ選択ダイアログに**（`cx.prompt_for_paths`）: 選んだフォルダを現在窓のレールへ slot 追加（既存なら切替）。ダイアログ経由は window が無いので pending_project_switch → render 消化パターン（3 例目）

## 2026-07-18 — LP 制作（lp/）+ スクショ連写モード
- やったこと: `lp/index.html` 新設（Cursor 風・日本語・実機素材のみで構成）。screenshot 機能に連写を追加（`SHIRUSHI_SCREENSHOT_FRAMES` / `SHIRUSHI_SCREENSHOT_INTERVAL_MS` + 保存ログに経過 ms・main.rs）。ACP 実ストリーミングは **release ビルドで 60ms 間隔 ×1100 コマ（≒12fps・92 秒）連写 → 実時間タイムスタンプで mp4 に組んで → GIF 化**（思考区間のみ 16 倍速編集。`lp/assets/gif/stream.{mp4,gif}`）。マスコットは確立パイプライン通り `mock/mascot/neko-anim/video/*.mp4`（Kling: tl/thk/cel/doze）から GIF 化 — `*8/` ディレクトリの 8 コマ版は旧世代なので宣材に使わない。素材採取は `~/Library/Application Support/Shirushi` をバックアップ → デモ用 state.json / shirushi.db（threads・turns を手組み）を配備 → プローブ撮影 → byte 一致で復元、の手順。リポジトリ直下の一時 `.shirushi/todos.md` も削除済み
- 学び/罠: main.rs の「オフスクリーンはグリフが写らない」コメントは古かった（font-kit で写る・修正済み）。連写は debug ビルドだと PNG エンコードが ~700ms/コマで律速 — release なら ~85ms/コマ。⌘F プローブ（SHIRUSHI_BUFFER_SEARCH）は SHIRUSHI_OPEN_TABS だと非同期オープンに先行して不発 — state.json の open_files に事前投入すれば効く。スレッドのタブ順は `updated_at DESC`、ACP_PROBE は index 1 に送る。zsh は `"$VAR[x]"` を添字展開する — ffmpeg のフィルタ変数は `${VAR}` で書く。この端末シェルには画面収録権限が無く screencapture は不可（オフスクリーン連写で代替）
- 次: LP の公開（GitHub Pages 等・todos の「LP を公開する」）。README のヒーロー画像を新素材へ差し替え検討

## 2026-07-19 — LP を操作アニメ化 + necoder へリブランド（LP のみ）
- やったこと: LP の静止スクショを「操作 → 出現」の CSS アニメに置換。整合した 1 セッション（state-tabs.json）から base（オーバーレイ無し）+ 各機能の操作後（todos/palette/switcher/diff/search）を撮り、`lp/assets/demo/*.png` に配置。`.player` コンポーネント = base/active の 2 レイヤをスクロールインで crossfade（active フェードイン＝オーバーレイだけ出現して見える）+ キーキャップ（⌘⇧P 等）or カーソル+波紋（Todo/diff のクリック）。IntersectionObserver で可視時のみ `.run` 付与＝画面外は停止。prefers-reduced-motion では active を静止表示。エディタ節の 4 枚カードを大きな showcase 3 本（palette/search/diff）+ LSP テキスト帯へ再構成。**公開ブランドを Shirushi → necoder（necoder.com）にリネーム**（title/meta/og/nav/hero/footer）。favicon とブランドマークを白髪アイコンから相棒の茶髪ネコ（`mock/mascot/gpt/01-neko.png` の顔クロップ）へ差し替え。未参照になった旧 GIF/PNG を削除（lp 13MB）。
- 学び/罠: リネームは**表示ブランドのみ** — repo/crate/`.shirushi/` 設定 dir/clone コマンドは実体が shirushi なので据え置き（LP のスクショ titlebar の "shirushi" はデモで開いているプロジェクト名なので不整合ではない）。screenshot 連写は debug ビルドで大 PNG 保存が ~700ms/コマ・kill で 0 バイト truncate する → **アプリは保存後に自分で quit する**ので kill せず自然終了を待つのが正。ユーザーが main.rs に同梱フォント（IBM Plex Sans JP / Guguru Sans Code・OFL）を追加済み＝日本語グリフがオフスクリーンでも綺麗に出るようになった。
- 次: necoder への本体リネーム（crate 名・ウィンドウタイトル・`~/Library/Application Support/Shirushi` の設定パス）は未着手＝別途本人判断。マスコットの固有名は未定（候補提示済み）。LP 公開。

## 2026-07-19 — プロジェクト色を Peacock 相当に（パレット一本化 + 任意 hex + ⌘K⌘C）
- やったこと: 既存の色ピッカー（M12-11・レール右クリック）を Peacock 水準へ。①`theme_core::IDENTITY_PALETTE_HEXES` 新設（巡回5色 + 予約色非衝突の厳選5色 cyan/chartreuse/violet/magenta/graphite）→ ピッカーの寄せ集め12色（Claude バレット `#d97757`・スレッド色 `#61afef`/`#c678dd`・選択面 `#7d9bd8` と衝突していた）を撤去し出所を1本化。②ピッカーに任意 hex 入力行（rename 式の生 `on_key_down` + 16進フィルタ・`parse_hex_color`→`apply_project_color`）。③`ProjectColor` アクション + ⌘K⌘C（⌘K⌘T に並置）+ `command_entries` に1行（コマンドパレット自動化）。キーボード起動は `open_project_color` がアクティブなレール項目（`RAIL_WIDTH` + index*38）にアンカー。④`apply_project_color` の波及をアクティブ1枚→全ペイン（`tabs`+`split_editor`・`apply_editor_settings` と同じ集約）へ。i18n ja/en に `cmd.project_color`/`color.hex_placeholder`。検証用 `SHIRUSHI_COLOR_PICKER` プローブ追加。変更: theme_core / workspace / keymap_core / locales。
- 学び/罠: **ユーザーが並行で workspace.rs（窓縁 2px 枠 L11599・UI-SPEC §1.3）を同時実装していた** — Edit の text-match のおかげで無衝突だったが、大ファイルの同時編集は line 番号が飛ぶ（apply_project_color が 4809→4835 と移動）ので「編集直前に再 Read して exact 一致で当てる」のが安全。私の色ピッカー変更（`apply_project_color`→`cx.notify()`）は窓縁ボーダー（`self.accent()` 参照）も自動追従 = 合流できた。**remote 別色は入れない**判断（project=slot が固有色・窓縁が「どのマシンか」を担う）。パレットの厳選は offscreen スウォッチ目視で確定（magenta が rose とやや近いが識別可で据え置き＝本人確認済み）。
- 次: ライブの ⌘K⌘C / コマンドパレット / hex 確定の対話確認は人の手番。ユーザー実装の窓縁 2px 枠との統合の体感確認も。

## 2026-07-19 — M10-2 ブランチ/worktree を既定でレールに開く + レール右クリックメニュー
- やったこと: ユーザー報告2件（①delete branch が効かない ②別ブランチが新窓で開くのを止めてレール＋右クリックメニューに）に対応。**ウィンドウモデルを転換**（DECISIONS/ARCHITECTURE §5 改訂）: 1窓=1worktree・新窓に開く → **1窓に複数 project×branch のレール・既定はレール内**。
  - `open_folder_in_rail(host, path, branch)` 新設（既存なら切替・無ければスロット追加）。`open_branch_worktree`(⧉) / `open_worktree_target`(⌘O・switch_branch_to フォールバック) / `open_worktree_window`(⎇ worktree 行) の3経路を `open_folder_as_window`→レールへ差し替え。新窓は右クリック明示 + ⌘⇧N のみ残す。
  - `ProjectSlot.worktree_branch: Option<String>` 追加（Some=リンク worktree タブ）。同一リポジトリ別ブランチの identity 色衝突を `next_free_color`/`color_in_use`/`colors_close` で回避（同色2枚を防ぐ）。
  - **レール右クリック = コンテキストメニュー**（`render_rail_menu` + `RailMenuState`）: 色スウォッチ＋「その他の色…」（→ユーザー実装のフル hex ピッカー `open_color_picker` を再利用）／新しいウィンドウで開く／レールから外す／(worktree のみ) worktree を削除・worktree ごとブランチを削除。破壊的操作は二段確認（`arm_rail_confirm`→`confirm` armed で実行）。旧「右クリック=色ピッカー直開き」はメニュー最上段に吸収。
  - `remove_project_slot`（active index 詰め + 最後の1枚ガード + `load_active_slot` へ張り替え。`switch_project` からロード部を抽出して共有）。`remove_slot_worktree`/`delete_slot_branch` は背景で `git worktree remove`（メイン作業ツリーの dir から実行）→ 任意で `git branch -D` → スロット外し。
  - **delete branch のバグ**: `git branch -d/-D` は「他 worktree に checkout 中のブランチ」を拒否する仕様（report の `asdfa` は scratchpad の `inline_probe` worktree が握っていた）。旧実装は失敗が `eprintln` に消えていた → 事前に `git_worktrees_on` で検知し**分かるトースト**（`git.branch_used_by_worktree`）+ 成否とも `push_toast` で可視化。
  - project に `remove_worktree_on`。i18n ja/en に rail メニュー7語 + git 3語。検証プローブ `SHIRUSHI_RAIL_MENU`。変更: project / workspace / locales / docs。
- 学び/罠: **また別セッションが並行で workspace.rs（直前の色ピッカー Peacock 化）を編集中**だった（`find -mmin -5` / `ps` で複数 claude + cargo の file lock で確認）。Edit の exact-match と「編集直前に再 Read」で無衝突。line 番号は 200 行超ずれる。`asdfa`/`inline_probe` は別セッション(925098de・稼働中)の scratchpad worktree 由来で、このリポジトリには既に無い（`git worktree list`=main のみ）ので再現不可＝コード側のハンドリングを直す方針にした。`git worktree remove` は**対象ツリーの中からは実行不可** → 一覧先頭（対象以外）のメイン作業ツリー dir で叩く。
- 追記（同セッション・ユーザー指摘）: **レール下部のアクティビティアイコンをアクティブ＝プロジェクト色・非アクティブ＝fg2 に**（VSCode の Activity Bar 準拠）。explorer/search/git/todos/agent/terminal の6つ、それぞれ表示中のビュー/ドックだけ accent（`rail_icon` の color 引数を条件式に）。todos の旧「running だけ accent」は「板を開いていれば accent」に一般化（running は板が開いている前提なので損失なし）。UI-SPEC §1.3 許可リスト + §2 に明記。スクショ目視で explorer+agent が accent・他 gray を確認。
- 追記（同セッション・実コード駆動の検証）: クリックできない代わりに**起動時プローブで実コードパスを叩き結果をオフスクリーン撮影**して4挙動を全確認。①`active_index_after_removal` を純関数化し6ケース単体テスト（末尾/先頭/自身/前後）②右クリックメニュー描画＋二段確認 armed（`SHIRUSHI_RAIL_MENU=confirm-worktree` → 「⚠ Click again to delete」赤）③**ブランチ→レール**（temp git repo に feature ブランチ→`SHIRUSHI_RAIL_PROBE=open-branch:feature` → 実 `git worktree add` が走り `ProbeRepo-feature` が**別色(cyan)のレールタブ**として追加・新窓ではない・親と color 衝突回避も確認）④**アクティブ slot をレールから外す→隣へ張り替え**（2スロット起動→`remove-active` → AltProject がアクティブになりツリー/ピル/宛先チップ全更新）。新プローブ `SHIRUSHI_RAIL_PROBE`（open-branch:/remove-active）+ `debug_rail_probe` は他の SHIRUSHI_* プローブと同じく dev ツールとして残置。
- 次: remote(SSH) worktree のレール追加の実地確認（ローカルは検証済み）。コミットは作業ツリーが多セッション混在のためユーザーがステージ範囲を決めて実施。

## 2026-07-19 — Finder 多重ガード + SSH config ピッカー + 窓縁色枠 + リモートのホスト別色（別セッション）
- やったこと: ユーザー依頼を上から消化。①**Finder 多重**: レール＋の `add_project_via_dialog`（`cx.prompt_for_paths` は全体で1箇所）に多重起動ガード `add_project_dialog_open`（`ssh_connecting` と同型・`await` の全経路で false へ戻す）。②**SSH config ピッカー**（ROADMAP M13「SSH config picker」実質達成）: `host::ssh_config_hosts()`/`parse_ssh_config`（`~/.ssh/config` の Host 列挙・`*?!` 除外・`=`区切り・複数 alias・unit test）→ `PickerMode::SshHosts`（既存 Picker 再利用）+ titlebar server アイコン/⌘⇧P「Remote: Connect to SSH Host」→ 選択で `ssh://alias/` seed → 既存 `connect_ssh_and_open`(M9) で新窓。行に **user@実IP** 併記（VSCode 超え）。i18n ja/en。③**窓縁 2px プロジェクト色枠**（ルート render・`self.accent()`）= Peacock 相当の「窓ごと識別」。UI-SPEC §1.3 許可リストに追記（面塗りは禁止のまま＝縁の線）。④**リモートのホスト別色(#3b)**: `.shirushi` はリモート側で使えないので `storage` に `host_colors`(host→0xRRGGBB) 新設（round-trip test）+ `theme_core::color_from_hex` + `apply_remote_host_colors`（storage セット後・`display_name` キー・初回は IDENTITY パレットからハッシュで焼付け＝開き順で色が変わらない）。⑤**footer にプロジェクト色●スウォッチ**（statusbar 左端・クリックで `open_project_color`＝ユーザー実装の色ピッカーを開くだけ）。⑥**#2d SSH 磨き**: `host_last_path`(host→前回パス) を storage 追加 → ピッカー選択で前回パスがあれば即接続（打たない）・行に `→path` 併記・接続成功で記録。⑦**手動 override**: `apply_project_color` のリモート分岐で `.shirushi` の無駄書きを止め `set_host_color` へ。変更: host / storage / theme_core / workspace / locales / docs。cargo check 全 green・storage/workspace test green・footer スウォッチ と SSH ピッカーは offscreen 目視。
- 学び/罠: **ユーザーが複数の並行セッションで workspace.rs（色ピッカー Peacock 化・レールメニュー等）を同時編集中**（mtime が数分ごとに進む）。Edit の exact-match ＋「編集直前に再 Read」で無衝突を維持（line 番号は数百行ずれる）。色ロジックは別 crate（storage/theme_core）中心に置き、workspace.rs は「新メソッド + 呼び出し1点」に留めてユーザーの色割当行を避けた。**要すり合わせ**: ユーザーの色ピッカー JOURNAL（本日2件目）は「**remote 別色は入れない**（窓縁が『どのマシンか』を担う）」判断だが、本会話で明示的に #3b を依頼された → 「窓縁色を機械ごとに安定させる」方向として実装（矛盾ではなく補完だが、方針は本人のセッション間で統一が要る）。#3b の色キーは `display_name`、#2d の前回パスキーは config alias（別テーブル・別用途で不整合なし）。
- 次: **live 検証（実 SSH 接続が要る・人の手番）**: リモート窓にホスト色枠が出るか / 再接続で同色 / ピッカーの前回パス即接続 / 手動 override（リモートで色選択→`host_colors` に残る）。ROADMAP M13「SSH config picker」はコード達成・実機受入待ち。remote 色を入れるか否かの最終方針はユーザーの色ピッカー方針と統一する。

## 2026-07-19 — スレッド改名/AI命名 + 履歴ビュー + レール SSH 導線 + Linux musl 配布（別セッション）
- やったこと: ユーザー4件（SSH機能不全・レール導線 / タブ改名 / 履歴 / AI命名）を4フェーズで消化。
  **#4 タブ改名**: `Thread.name_is_custom` 追加・タブ ダブルクリック（`on_click` の `click_count()==2`、既存 `on_mouse_down` 切替と共存）→ `EditorView::plain`（IME正しい）のインライン入力（アクセント枠）→ Enter 確定 / Esc 取消 → `persist_thread`。改名中は on_mouse_down ガードで切替/フォーカス移動を止める（入力を邪魔しない）。offscreen 目視（タブがアクセント枠入力に差替）。
  **#6 AI自動命名**: `project::name_thread_on`（`inline_command_on` 同型 = `claude -p` 一時ファイル経由・shell 引用回避）→ 初回 `TurnEnded` 後、既定名（"スレッドN"）かつ未手動改名なら会話冒頭（最初の user + agent 応答先頭）から18字タイトルを背景生成→差替。`settings.agent_auto_name`（既定 on）。`is_placeholder_name` で再命名防止・手動改名尊重。
  **#5 履歴**: `storage::load_all_threads`（archived 含む・unit test）+ `entry_from_turn`/`thread_from_storage`（set_storage から切出）+ `AgentPanel::open_thread_from_history`。導線 = Agent タブ列の 🕘 ボタン → `PanelEvent::OpenHistoryRequest` → workspace が pending で消化（subscribe に window 無し迂回・OpenDiffRequest 同型）→ `PickerMode::ThreadHistory`（`open_picker` 再利用・行頭●スレッド色 + Σトークン detail）。⌘⇧H / パレット「AI: スレッド履歴」も。offscreen 目視（3スレッドが色付きで一覧）。
  **#1/#2/#3 SSH**: 診断 = コードは M9 で完全だが mac→Linux で Linux musl バイナリ未生成/未同梱 → `ensure_remote_server` が bail が実体（バグでなく配布の穴）。①レール `server.svg` アイコン（`RailSettings.remote`）→ 既存 `open_ssh_host_picker`（~/.ssh/config ワンクリック・#2d 前回パス即接続は既存）。②リモート slot に server バッジ（`is_remote()` 分岐・絶対配置）。③**CI に `build-remote-server` ジョブ**（cargo-zigbuild で x86_64/aarch64 musl → upload-artifact）。④`ensure_remote_server` を **platform 先検出 + per-target 自動発見**（`find_remote_server_for`: `~/.local/share/shirushi/remote/artifacts/<triple>/` → .app 同梱 `Resources/remote/<triple>/` → same-platform dev の順。env 指定と same-platform は不変＝厳密な superset。`remote_target_triple` unit test）。⑤`bundle-mac.sh` が server バイナリ（同OS=MacOS隣・別OS musl=Resources/remote/）を同梱。**Docker で x86_64 musl バイナリ生成済み**（3.9MB static-pie ELF）。
  変更: settings_core / project / agent_panel(+Cargo project 依存) / storage / workspace / keymap_core / host / main.rs(検証プローブ 3本) / ci.yml / bundle-mac.sh / locales。cargo test --workspace 全 green・i18n parity green。新UI 5点 offscreen 目視（レール SSH アイコン / 履歴🕘 / タブ改名入力 / SSH ホストピッカー / 履歴 Picker）。
- 学び/罠: **offscreen で storage 依存状態（履歴 Picker）はユーザーの起動中アプリが DB をロックしていると開けない**（`Locking error` → `storage=None` → seed スレッド表示）→ `SHIRUSHI_DB=<一時>` で回避（初回起動で seed が永続化され load_all_threads が拾う）。**cargo test を `-p A -p B` 複数指定すると feature 統合で `Availability` 未解決の偽エラー**（`--workspace` や単独 -p では出ない＝CI 無害）。musl クロスは `rust-toolchain.toml`(1.95.0) が messense image 既定を上書き → `rustup target add` を挟む必要。ユーザーが workspace.rs/agent_panel.rs を並行編集中（rustfmt hook も走る）＝ 編集直前 Read で追従。
- 次: **人の手番**: ①実 Linux 接続（`~/.ssh/config` に ubuntu@AWS / azureuser@Azure = x86_64 濃厚 → ビルド済み musl を `SHIRUSHI_REMOTE_SERVER_BINARY` 指定 or `~/.local/share/shirushi/remote/artifacts/x86_64-unknown-linux-musl/shirushi-remote-server` に配置で開通確認）②ダブルクリック改名の IME 実操作 ③実 claude での自動命名 live ④リモート slot バッジの実接続確認。askpass（パスワード認証）は鍵/agent 運用なら不要のため後続。CI の musl ジョブは初回実行で追従修正あり得。

## 2026-07-19 — Agent タブ横スクロール + markdown 描画（pulldown-cmark 借用・自作レンダラ）+ Thinking 折り畳み（別セッション）
- やったこと: ユーザー依頼を上から。①**Agent タブ横スクロール**: `render_thread_tabs` を Zed `tab_bar` 構造へ（Σ台帳/＋を両端 `flex_none` 固定・中央タブ列だけ `flex_1`+`overflow_x_scroll`+`track_scroll(&tabs_scroll)`・各タブ `flex_none`）。GPUI div.rs で y 非スクロール時に縦ホイール→横送りにマップ＝**ホイールもトラックパッドも両対応**。②**markdown 描画（M4「継続」を消化）**: パーサは permissive な **pulldown-cmark 0.13**（借用）、**イベント列→Block モデルは自作**（`markdown.rs`・GPUI 非依存・unit test 5本）、**Block→GPUI 描画も自作**（Zed の markdown crate は GPL で不使用・DECISIONS §5＝git=CLI/terminal=alacritty と同じ路線）。見出し/段落/箇条書き・番号・タスクリスト（ネスト深さ）/フェンスコード/水平線/インライン（強調・斜体・打消し・コード・リンク）。③**transcript 選択(M13)を region インデックス化**: 1 エントリが複数 markdown ブロック＝複数リージョンに割れても選択が壊れないよう `SelectableRegion`/`TranscriptPoint` を entry→region index へ一般化。装飾+選択背景は `gpui::combine_highlights`（端点スイープ・active_styles fold）で合成（入れ子強調も安全）。プレーンは 1 ブロック=1region で無改変。④**Thinking 折り畳み**（Claude Code 風・印っぽく）: 既定は `✳ Thought ▸` + 1 行プレビュー、クリックで全文展開（`expanded_thoughts: HashSet<(thread.id, entry index)>`）。実行中の最終ブロックは自動展開 + `✳` pulse（`pulsating_between`・idle 静止＝CPU 0%）で「動いてる」を明示。reveal タイプライタ維持。変更: agent_panel(+`markdown.rs` 新設) / Cargo(root+agent_panel に pulldown-cmark)。cargo test -p agent_panel 全 green(9)・警告0・fmt 済み。**offscreen 目視: seed の markdown（見出し/太字/インラインコード=syn-mac 色+bg3/箇条書き/rust コードブロック枠）+ Thinking 折り畳み（✳ Thought ▸ + 1行プレビュー）を撮影確認**。
- 学び/罠: **GPUI `with_highlights` は重なり非対応**（`compute_runs` はソート済み非重複前提）→ 重なるハイライト（選択背景×markdown 装飾・入れ子強調）は `gpui::combine_highlights`（style.rs）を通してから渡す。**`HighlightStyle` に font-family が無い**＝インラインコードを mono にできない（コードブロックは container の `font_family` 継承で mono 可）→ インラインは色+薄背景で表現。pulldown-cmark は部分 markdown も落ちない＝ストリーミングは全文再パースで十分（増分不要）。offscreen で seed を出すには**起動中アプリの DB ロック回避**（別 `HOME` or `SHIRUSHI_DB`）+ 先頭撮影用に `SHIRUSHI_SCROLL_TOP` を新設。**ユーザーが workspace.rs/agent_panel.rs を並行編集中**（`name_is_custom`・rename・エージェント設定 UI・rustfmt hook）＝編集直前 Read + exact match で無衝突維持。**相談の確定**: エージェント見分けアイコンは Zed 同様「ACP レジストリ実行時取得（repo にロゴを持たない＝再ライセンス自由を保つ）＋未知は sparkles フォールバック」で合意。
- 次: **人の手番/後続**: ①実 claude ストリーミングで markdown 逐次描画の体感 ②Thinking 展開/折り畳みのクリック実操作 ③**完了 Ring 音**（新規・ROADMAP 未載・`TurnEnded` で短チャイム・`rodio` or `afplay`・設定でオフ・idle 影響なし） ④**タブ view モード**（Bar/縦 List/プロジェクト⎇ブランチ Grouped の登録式）⑤**エージェントアイコン**（ACP レジストリ DL 実装）⑥markdown の表・引用装飾。dev probe 追加: `SHIRUSHI_TABS_PROBE=<n>`（タブ溢れ検証）/ `SHIRUSHI_SCROLL_TOP`（transcript 上部撮影）。

## 2026-07-19 — composer キャレット可視化バグ + 完了音 + タブ view モード（Bar/List）（別セッション続き）
- やったこと: ①**composer のキャレット可視化バグ修正**（`editor_view`）: 長い1行を貼って入力すると入力位置が見えない件。原因は**編集ハンドラが `scroll_caret_into_view` を呼ぶ時点で `wrap_map` が旧 version（stale）** → `display_row_for_offset` が論理行へフォールバックし、折り返した行のキャレット表示行を「行0」と誤認 → 縦スクロールせず画面外のまま。修正: **reveal を prepaint（wrap_map 再構築後）へ遅延**する `pending_caret_reveal` フラグ導入（編集/カーソル移動で立て、prepaint で `viewport_height>0 &&` 短絡で消化＝viewport 未確定なら保持）。`set_plain_text` も末尾キャレットを reveal。**wrap-off の code editor は display=logical で挙動不変**（回帰なし）。offscreen 目視: 長行流し込みで**末尾（fox…1234567890.）まで自動スクロールしキャレットが見える**のを確認。②**完了音**: `settings.completion_sound`（既定 on・`#[serde(default)]` で既存 config 安全）+ `TurnEnded` 成功時に macOS system sound（`/usr/bin/afplay /System/Library/Sounds/Glass.aiff`）を**短命スレッドで status() まで待って再生**（zombie 刈り取り・UI 非ブロック・イベント駆動で idle 影響なし）。失敗（Failed）では鳴らさない。既存 shell-out（open/osascript/trash）と同じ依存ゼロ方式。独自チャイム同梱は後続。③**タブ view モード**: `AgentTabsView{Bar,List}`（explorer の登録式ビューと同型）。**Bar**=色付き横タブ（既定）、**List**=縦リスト（色ドット+名前+トークン・active は左2px色バー+bg1・行クリックで switch・× で閉じ・`max_h(220)`+`overflow_y_scroll`）。切替は小トグル `render_tabs_view_switcher`（explorer footer idiom・svg 色直接指定・遷移先アイコンを表示・list.svg⇄columns-3.svg）で Bar/List 両ヘッダに配置。i18n `agent.view_list`/`view_bar`（ja/en）。変更: editor_view / settings_core / agent_panel / locales。cargo test 全 green（agent_panel 9・editor_view 5・settings_core 8・i18n parity）・fmt 済み。offscreen 目視: composer 末尾スクロール / List ビュー（3スレッド縦・active 左バー）を撮影確認。
- 学び/罠: **soft-wrap は編集ハンドラ時点で `wrap_map` が stale** = 「編集直後にキャレット行を計算する」処理は wrap-on だと誤る。折り返しのある editor で caret reveal は**必ず wrap_map 再構築後（prepaint）**に。`scroll_caret_into_view` は縦のみ・横スクロールは無い設計（composer は `wrap_on = soft_wrap || plain` で常に折り返す＝横スクロール不要）。afplay を `spawn` しっぱなしは zombie が溜まる → 短命スレッド + `status()` で刈る。**ユーザーがエージェント設定 UI（レターバッジ→実ロゴアイコン化）を並行実装中**（本セッションのスクショに反映）。dev probe 追加: `SHIRUSHI_TABS_VIEW=list`（List 起動）。
- 次: **人の手番**: ①長行 composer の実操作（貼り付け→入力でキャレット追従） ②完了音の実ターン（実 claude・`SHIRUSHI_ACP_PROBE`）で発火確認 ③List ビューの多数スレッド時スクロール（`SHIRUSHI_TABS_PROBE=12 SHIRUSHI_TABS_VIEW=list`）。**後続**: タブ view の永続化（settings or `.shirushi`）・Grouped（プロジェクト⎇ブランチ束ね）・完了音の window 非フォーカス時のみ化 or 独自チャイム同梱・設定画面に completion_sound/view トグル。

## 2026-07-20 — Picker クリック選択 + Todo ボード追加 + SSH 履歴（最近のプロジェクト・別セッション）
- やったこと: ①**Picker がクリックで選べないバグ修正（共通）**: `crates/ui` の Picker アイテム行に **on_mouse_down（選択＋確定）が元から無く**（`git show HEAD` で確認・回帰でなく初めからキーボード専用設計）→ SSH/⌘O/パレット/テーマ/シンボル/履歴 全 Picker がクリック不能だった。行に click（`selected=row`→`confirm`・stop_propagation・hover）を追加＝全 Picker がクリックで選べる。ブランチ ⎇ メニュー / Agent セレクタは元から on_mouse_down 有り（別コンポーネント・無事）。②**Todo ボードから追加**: `project::todos::add_todo_on`（今日の見出し下へ 1 項目追記・既存 `add_todos_on`/`append_todos` 流用・`date +%F`・unit test は append 側で担保）+ ボードヘッダに **＋**（IME 正しい `EditorView::plain` インライン入力・Enter 確定/Esc 取消・追記後 watch で自動リロード）。③**SSH 履歴（最近のリモートプロジェクト）**: `storage::remote_projects`（host+path で upsert・opened_at 降順・round-trip test）+ `connect_ssh_and_open` 成功時に `record_remote_project`（name=パス末尾フォルダ）+ **SSH ピッカーを2階層化**（上=最近のプロジェクト〔行頭●・host:path・クリックで直接接続〕/下=config ホスト〔従来の前回パス即接続 or 入力〕/末尾=手入力。id は recent 分 offset）。変更: ui / project / storage / workspace / locales。cargo test --workspace 全 green・i18n parity green。offscreen 目視: Todo ＋ボタン / SSH 2階層（最近3件●+config3件+手入力）を撮影確認。
- 学び/罠: **Picker のクリック欠落は回帰でなく初期からの穴**（HEAD の Picker は on_mouse_down が背景/箱の2箇所のみ）＝キーボード ↑↓Enter 専用だった。左カラムのテキスト入力（git message/branch）は手書き String＝**IME 非対応** → Todo は日本語必須なので EditorView::plain を採用（composer/タブ改名と同じ判断）。offscreen で storage 依存 UI（SSH 最近）は**別 DB を seed**して撮る（実 DB はユーザーの起動中アプリがロック→ `sqlite3` で remote_projects に直 INSERT）。ユーザーが agent_panel transcript(pulldown-cmark)/onboarding/完了音/タブ view を並行実装中＝編集直前 Read で追従（ビルドは赤→緑を確認してから UI 配線）。
- 次: **SSH home 既定接続は未実装**（`SshProject.path` 必須の緩和 + connect 時に remote $HOME 解決＝「ホスト選ぶ→home をブラウズ→目的フォルダを開くと履歴に残る」の初手。untestable connect + URI/path 波及のため別パスで慎重に）。**人の手番**: Picker 実クリック選択 / Todo ＋の IME 入力→追記 / 実 SSH 接続→記録→次回ピッカーに最近として出るか。

## 2026-07-20 — Workspace ownership refactor 完了

- やったこと: 約 13,000 行・約 95 直接フィールドだった `workspace.rs` を、開始 checkpoint `ca46f40` から小さい checkpoint で再編。`src/lib.rs` facade / persistence / `ProjectSource` / typed LSP parser・server registry を先に分離し、TerminalDock・SearchPanel・Explorer・GitPanel・TodoPanel・SettingsView を長寿命 Entity へ移した。編集面は `EditorArea`（tabs/pane、LSP/diagnostics/completion、navigation、diff/inline edit、hot exit）、project 単位の実行状態は `ProjectSession`（Agent/Terminal/Search/Explorer/Git/Todo、repository snapshot、watcher）、rail metadata と session 配列は非公開 `ProjectSessions` が所有する。`Workspace` は 8 直接フィールド。`workspace.rs` は合計 1,399 行だが production body は 975 行で、残りは完了条件を固定する test。通常の project switch は pure helper で active index だけを変え、loaded session を再生成しない。command/panel 結線は `CommandRegistry` / `PanelRegistry`、`debug_*` は debug build のみに隔離。root Render に残っていた pending command 消化も effect-cycle 末尾へ移した。
- 設計判断: `EditorArea` 自身は `Entity` にせず、長寿命 aggregate の中に実体の `Entity<EditorView>` を保持する。二重 Entity 化は session 内の同期 editing command を不必要な cross-entity update にするため。当初案の Explorer/Git 全 Render 移動は、active ProjectSlot と rail/picker/notification/window を横断する callback に project state の複製か巨大な event protocol を要求するため改めた。project model / interaction state / typed event 契約は独立 crate、実描画 callback は shell adapter (`explorer_view.rs` / `git_view.rs`) が所有する。実際に child Entity から上がる Search/Agent/Terminal/Settings/Todo は typed event で結線し、Explorer/Git の現操作は adapter 内で完結する。
- 検証: 最終監査 checkpoint `d83578a`。`cargo check --workspace` / `cargo test --workspace` / `cargo check -p shirushi --release` / `git diff --check` が成功。開始 checkpoint との rustdoc 公開項目比較で top-level item / Workspace method の削除 0、actions 定義と ja/en locale の不変、旧/現 `state.json` の公開 loader round-trip を確認。GPUI test で A→B→A の dirty text・undo と Agent/Explorer/Git/Todo/TerminalDock/TerminalView Entity 保持、local/remote root draw 中の全 Host call 0 を実証した。release 実 cfg / `.rlib` に debug probe はなく、feature crate の workspace 逆依存もない。default / Explorer 3 view / Git / Search / Settings / Todo / Picker の 9 状態を 2560×1600 offscreen で目視し、起動 error なし。詳細は [`REFACTOR-WORKSPACE.md`](./REFACTOR-WORKSPACE.md)。
- 学び/罠: Rust の `Deref` は公開型の Target に private owner を漏らせない。`Workspace -> ProjectSessions -> ProjectSession` の公開 Deref 連鎖ではなく、`ProjectSessions` を完全非公開にして `Workspace -> ProjectSession` だけを公開境界にし、project metadata access は `project_sessions` と明示した。また「ファイルを include 分割した」だけでは ownership は変わらないため、先に session のライフサイクルを常駐化してから feature state を移す順序が安全だった。
- 次: この refactor の追加作業はなし。将来 Explorer/Git を shell chrome から完全に独立描画できる共通 Dock API が必要になった時だけ、現在の adapter を child Render へ移す。StatusItemRegistry も同様に、実際の拡張 consumer が生まれた時点で導入する。

## 2026-07-20 — Workspace refactor の独立検証（別セッション）+ flaky SIGABRT 修正

- やったこと: 完了報告の第三者検証。①構造の主張を実測で追認（8 直接フィールド・workspace.rs 1,399 行・`cargo tree -i workspace` 逆依存なし・checkpoint 列・監査 test の実体）。②検証初回の `cargo test --workspace` で workspace lib が **SIGABRT**（notify fsevents スレッドの destructor panic）→ 原因は両 gpui 監査 test の「監視中 temp root を削除してから teardown」順序 × per-session watcher（A→B→A 後は 2 本生存）。teardown で全 session の `_watch`/`_watch_pump` を root 削除より先に落とす修正 → フルスイート 2 連続 green（隔離ループ 32 回も green）。③オフスクリーン目視（default・2560×1600）崩れなし。④実態の註記を REFACTOR-WORKSPACE.md §7 へ固定（include! は module ではない / 8 フィールドは Deref 連鎖と併読 / dev_probes に本番コード同居 / editor_area/mod.rs 規約違反 / explorer・git_ui crate は薄い契約層）。
- 学び/罠: **パイプは exit code を握り潰す**（`cargo test | tail` は tail の 0 が返り、初回の SIGABRT を「green」と誤読しかけた）→ 検証コマンドはリダイレクト + 直接 `$?` で。**flaky は隔離ループでは出ないことがある**（今回 0/32）＝フルスイート並列の負荷が race 窓を開く。watch 系テストの teardown は「watcher を殺してから watch 先を消す」を定石に。screenshot-app.sh（簡易版）は実デスクトップに起動して全画面 screencapture する＝検証は `SHIRUSHI_SCREENSHOT` のオフスクリーン経路を使う。
- 次: REFACTOR-WORKSPACE.md §7.5 のフォローアップ（mod.rs 改名 → dev_probes 移設 → include!→mod 化 → notify 更新 → Deref 明示化。軽い順）。

## 2026-07-20 — §7.5 フォローアップ全 5 項目を実施（include! 全廃 = 真のモジュール化）

- やったこと: ① `editor_area/mod.rs` → `editor_area.rs`（mod.rs 禁止規約に復帰）。② dev_probes の本番コード移設（SSH 一式→`remote_ssh.rs` 新設 / `on_terminal_dock_event`→panels.rs / `open_thread_history`→overlays.rs / updater 2 本→chrome.rs）＝ dev_probes は全 item cfg(debug_assertions) の `debug_*` のみに。③ **include! 24 ファイルを実モジュール化**: `src/workspace/` 配下の子モジュール 18 + editor_area 孫 7。鍵は (a) hub の import 束を `pub(crate) use` 化 → 子は `use crate::workspace::*;` 1 行、(b) **descendant 可視性**（子は親 hub の private 型に触れる）で hub 側の型変更ほぼ不要、(c) hub が `pub(crate) use 子::*` で共有型を再フラット化、(d) cross-module メソッドは一括 pub(crate)。B1〜B4 の 4 バッチ・各バッチ check+test green でコミット。④ notify 6.1.1 → **8.2.0**（API 互換・コード変更なし）。⑤ `session()`/`session_mut()` 明示アクセサ + project_watcher/project_switch を明示形へ。
- 学び/罠: sed の一括 `pub(crate) fn` 化は **trait impl の fn に当たると E0449**（Deref×2・Render×1 を個別復旧）＝先に trait impl の位置を洗ってから。子→親は private でも見えるが、**親→子・兄弟→兄弟は pub(crate) が要る**＝「共有語彙型は hub に置く」が最小 churn。`private_interfaces` 警告（private 型が pub(crate) シグネチャに露出）は hub 型を pub(crate) に揃えて解消。include! 時代のファイルは rustfmt 未整形なので、mod 化直後の一括 fmt は diff 爆発する → 整形は別 PR に分離（今回はしない）。
- 次: pub(crate) の絞り込みと残モジュールの session() 明示化は「触った機能から」。fmt 一括適用は独立コミットで判断。

## 2026-07-20 — Agent タブ3件: 改名バグ / エージェントアイコン / herdr風 状態可視化（別セッション）
- やったこと: ユーザー3件を消化。**①ダブルクリック改名バグ**: 根因はタブ同居の並べ替え `on_drag` の 2px 閾値がクリック合成を握り潰し `on_click` の `click_count==2` が発火しないこと + **List モードに改名配線が無い**こと。両モードとも判定を押下即発火の `on_mouse_down` の `event.click_count`（メソッドでなくフィールド）へ移し、List 行にも `renaming` 差替え描画を展開。**②タブにエージェントアイコン**: `settings.rs` private の `agent_brand` を **`acp_client::AgentKind` のフィールド**（`icon`/`brand_color`/`monogram`）へ昇格＝カタログが単一の出所（settings/agent_panel 両方が acp_client 依存で逆依存を作らない）。`agent_badge(label,size)` を `Thread.agent`（ラベル）→`by_label`→`brand()` で引き、タブ/List に描画（Claude=brand-claude.svg のテラコッタ sunburst が実機で出た）。**③herdr風 5状態**（**ACP ネイティブ＝ヒューリスティック不要**が肝）: `ThreadActivity{Idle,Working,Blocked,Done{interrupted}}` を新設し `render_meta` の導出を `Thread::activity()` に formalize（Blocked=`pending_permission`・これまで `running:bool` に埋もれていた「待ち」を分離）。Done は `done: Option<TurnEnd>` ラッチ（TurnEnded で立て・裏スレッドのみ・`switch_thread`/submit でクリア）。`acp_client` の `StopReason(_)` 破棄をやめ `AgentEvent::TurnEnded{reason:TurnEnd}` 化（Refusal/Cancelled=中断）。横断台帳 `RunningRegistry` のタプルと `beacons()` を `bool`→`ThreadActivity` に拡張。
- 見せ方の決定（ユーザー確認）: **色は識別に集約（§1.3）を守り、状態は色相でなく「リング×動き」**。共有部品 `activity_dot`（Idle=淡静止 / Working=満脈動 / Done=リング静止・中断=中空 / Blocked=リング＋速い脈動）。フッターは**常設ロールアップ**（herdr 本来＝ニュースティッカーではない・中央スペーサーに「N 実行·M 承認待ち·K 完了」+代表スレッド・click で該当 project+Agent）。**レールの他プロジェクト状態ドット**（ARCHITECTURE §5 設計済みを実装・右上絶対配置・リモートバッジ右下と非衝突）。beacon/⌘O も状態化。表示範囲は「既存サーフェスに溶け込ませる」（専用パネルは作らない＝将来 `RunningRegistry` の上に安く足せる）。
- 検証: 全 crate `cargo test --workspace` green（i18n parity 含む）・`cargo check -p shirushi` 警告0。offscreen 3枚を目視（`SHIRUSHI_ACTIVITY_PROBE` 新設で各状態注入）: **(a) Bar**=タブに Claude アイコン+3状態ドット・titlebar beacon 状態化・レール右上ドット・フッター「1 waiting·1 working·1 done」、**(b) List**=行に アイコン+状態ドット、**(c) rename**=List/Bar とも入力欄がアクセント枠で差替わる。**改名トリガの実挙動（実ダブルクリック→IME 入力→Enter/Esc）はユーザーの手番**（offscreen はクリック合成を再現できない）。
- 学び/罠: **`MouseDownEvent.click_count` はフィールド**（`ClickEvent` はメソッド `click_count()`）。gpui のクリックは `on_drag` 同居 + 2px 移動 + `pending_mouse_down` クリアで合成が飛ぶ → 確実な二重クリックは `on_mouse_down` 側で見る。`ThreadActivity` は agent_panel に置き workspace が読む（workspace→agent_panel 依存は既存）。ブランドアイコンは Claude ロゴがそもそも sunburst＝「✳ っぽい」が実 svg（fallback の ✳ ではない）。offscreen は probe 2s 発火 → `SHIRUSHI_SCREENSHOT_DELAY_MS=3200` で撮る。DB ロック回避に `SHIRUSHI_DB=<一時>`。
- 次: **人の手番**: ①実ダブルクリック改名（Bar/List・日本語 IME）②実 claude で送信→Working→（承認カード）Blocked→完了 Done の遷移が beacon/フッター/レールで一斉に動くか③中断（Cancel）で Done が注意表示か。**後続**: フッター左 `waiting_thread` ドットとロールアップの重複を整理するか（現状は左=アクティブの承認待ちアフォーダンス/中央=集計で併存）・⌘O やレールドットに状態形（今は色ドットのみ）を持たせるか・専用「全エージェント一覧」パネル（②案）を足すか。

## 2026-07-20 — マルチエージェント編隊を設計（mock ＋ UI-SPEC §11 ＋ ROADMAP M14・実装は次）
- やったこと: 「herdr を参考にした編隊モード」をユーザーと対話設計（実装はせず /goal に拾わせる前提）。①**mock 編隊モード**を新設（`mock/index.html`・`#fleet` で開く）: herd 状態サイドバー ＋ 系譜グラフ ＋ N セルグリッド ＋ ニュースフィード ＋ focus-follows。グラフは **4 表示（リバー既定・扇形・縦ツリー・カード）**をアイコン＋語スイッチャーで切替・⌄ で折り畳み。セル数 1..8 の自動レイアウト・＋タイルで Agent/Terminal/Editor 追加・× で閉じる。プロバイダバッジ（monogram・実機は `AgentKind.icon`）。②別セッションが同日入れた「**状態に色相を使わない**（色=識別・状態=形と動き）」決定に mock を**全整合**（herd グリフ・グラフ先端・approve カード・news を全部スレッド色化 ＋ `activity_dot` 相当の形/脈動へ。承認待ち=速い脈動リング）。③**herdr 記事を分析**（zenn・AIエージェント multiplexer）→ 検知/状態一覧/会話復元は ACP で既にカバー・**唯一の実質ギャップ = エージェントが編隊を操作する API（#3-4・`wait agent-status`）**と判断。④**UI-SPEC §11 編隊** ＋ **ROADMAP M14**（6 フェーズ）＋ **DECISIONS**（採否ログ）＋ **ARCHITECTURE**（`graph_view` crate・§5 編隊注記）を追記。M13-7「Grouped ビュー」は②モデル拡張で決着。
- 学び/罠: **状態に色を足すと「色=識別」の掟が崩れる** — herdr の 🔴🟡🔵🟢 は不採用、状態は `activity_dot` の形と動きで（別セッションが同日 §1.3/§6/§8 に明文化＋実装済み。ACP ネイティブ＝ヒューリスティック不要）。**mock の CSS クラス衝突**: `.rail` がプロジェクトレール既存クラスと衝突 → スイムレーンの線が 46px ブロック化 ＝ 新クラスは既存 grep してから（`.lrail` に改名で解決）。**SVG の `transform-origin` 既定は svg 原点** → 脈動リングがズレる（`transform-box:fill-box; transform-origin:center` で解決）。ヘッドレス Chrome の連続起動はプロファイルロックで稀に 1 枚落ちる。
- 次: **/goal で M14 を上から消化**（実装はユーザーが走らせる）。Phase 1 の ACP 状態基盤は 2026-07-20 別セッションで実装済み ＝ 残 = **herd サイドバー（状態一覧の常設パネル・`RunningRegistry` の上に）**。整合の残: mock 通常ビューの statusbar を中央ロールアップ表示へ（§3・任意）。

## 2026-07-20 — M14 #1 herd サイドバー（状態一覧の常設パネル）
- やったこと: M14 の 1 歩目。状態基盤（`ThreadActivity`/`activity_dot`/`RunningRegistry`・別セッション実装済み）を**左ドックの専用ボード**として常設。
  - **`crates/workspace/src/workspace/herd_view.rs` 新設**（`mod herd_view;`）: `render_herd_sidebar` ＋ `toggle_herd_sidebar`。左カラムの排他チェーン（`show_herd` → todo → git → explorer）に最優先で差し込み。開閉は git パネルと同流儀（開くと他ボードを畳み `show_left=true`・閉じるとエディタへフォーカス復帰）。
  - **プロジェクト別グループ**: `self.project_sessions.{projects,sessions}` を同添字で走査し、各 `ProjectSession.agent_panel.read(cx).statuses()` を集約（**非アクティブ project も含む**・read はメモリ参照のみ＝描画中に Host 呼ばない規約を守る）。行 = 左 2px スレッド色バー ＋ `activity_dot`（形と動き）＋ `agent_badge`（エージェント種別アイコン）＋ 名前 / `⎇ branch · 状態` ＋ トークン（`human_tokens`・0 は「—」）。ヘッダに実行中/承認待ち/完了ロールアップ、末尾に中立色（fg2）の 4 状態凡例（**形の説明**）。
  - **クリック = focus-follows の入口**: 行クリックで `switch_project` → 当該 `agent_panel.focus_thread(index)` → Agent ドックを開く（M14 #3 で左ファイル/下ターミナル追従へ拡張予定）。
  - **下段 = focus 中 worktree ツリー**（mock `.fleet-files`・当初先送り→**同セッションでやりきり**）: `render_tree(active_slot)` を再利用（git 色つき・行クリックで開く/展開も既存経路）＋ ヘッダ「● Files · <スレッド名>'s worktree」（`active_thread_name()`/`active_color()` 追加）。herd は explorer と**排他**なので herd ビュー中はここが唯一のファイルツリー＝重複しない。行クリックの `switch_project` でツリーがフォーカス追従する（＝現モデルでの focus-follows）。上下 2 段は 50/50（`flex_1`・flex-basis % は当 crate 未使用のため）。

  - **agent_panel の公開 API 追加**: `AgentStatus`（name/color/activity/agent/tokens）＋ `statuses()`（`beacons()` のリッチ版）＋ `focus_thread()`。既存 `human_tokens`/`agent_badge` を `pub` 化（workspace から再利用）。`activity_label` を `pub(crate)` に。
  - **導線**: レール ⚡ アイコン（新規 `icons/activity.svg` = Lucide activity・脈動＝状態の隠喩・main.rs へ AssetSource 登録）＋ `RailSettings.herd`（既定 on）＋ アクション `ToggleHerdSidebar` ＋ コマンドパレット「表示: 編隊」＋ `SHIRUSHI_HERD` プローブ。i18n: `herd.title`/`herd.empty`/`rail.herd`/`cmd.toggle_herd`（ja/en・parity green）。行状態は既存 `agent.state_*` を再利用。
- 学び/罠: **左ドックの排他は「散らばったフラグ」パターン**（todo=Entity.open / git=Entity.open / herd=`chrome.show_herd` / explorer=フォールバック）。4 つ目を足すには開く側で他 3 を畳み、explorer のアクティブ色判定に `!show_herd` を足すのを忘れない（1 箇所でも漏れると 2 ボード同時アクティブ表示になる）。**`RunningRegistry` は他窓分も持つがリッチ情報（agent/branch/token）は無い**（(name,color,activity) のみ）→ herd の詳細はこの窓の `agent_panel` から引く設計に（他窓分はレールドット/⌘O/statusbar が担う既存分担のまま）。**グループ見出しに集約状態語を出すと誤解を生む**（初版「3 Waiting」＝ 1 件しか待っていないのに）→ 件数だけに。offscreen は `SHIRUSHI_HERD=1`＋`SHIRUSHI_ACTIVITY_PROBE=1`＋`SHIRUSHI_SCREENSHOT_DELAY_MS=2700`（probe 2s より後）＋一時 `SHIRUSHI_DB` で撮る。
- 次: **M14 #3（編隊グリッド＋worktree 帰属・focus-follows の完全版）** — herd 行/セルのフォーカスで**左エクスプローラ/エディタタブ/下ターミナル**まで別 worktree へ張り替える（本項の herd 下段ツリーは現モデル＝アクティブ project 追従で先取り済み・#3 は別ブランチ dest への拡張）。その前提の**スレッド↔worktree 帰属モデル拡張**（M13-7 ②）が #3/#4 の土台。**残の軽い磨き**: herd 行のアクティブ強調（現在 focus 中スレッドの枠）・凡例に中断 Done（中空）を足すか・行に最終触りファイル名（mock の「— buffer.rs」）を出すか。**人の手番**: 実 claude 送信で Working→Blocked→Done が herd/beacon/statusbar で一斉に動くか、レール ⚡ クリックの開閉体感、下段ツリーの実クリックで開く。

## 2026-07-20 — M14: herd を「worktree spaces 一覧」へ（ユーザー指摘 → ① 採用）
- 経緯: #1 完了報告後、ユーザーが「herdr の**worktree を spaces として一覧**（worktree ごとに terminal 開かなくて済む）が良い、これは達成された？」と指摘。**#1 は達成していなかった** — herd は「プロジェクト別グループ」で、同一プロジェクトの全スレッドが同じブランチを共有＝worktree ごとに並んでいなかった（私はそれを「やり切った」と誤って報告していた。判断ミスを謝罪）。**方式を対話決定 = ①「worktree = 切替 space」**（worktree を同ウィンドウのレール/herd に space として並べ、切替でファイル/エディタ/**下ターミナル**が丸ごと追従。②スレッド帰属拡張は見送り）。
- **大きな発見**: ①の**機構は既に存在**していた。`open_worktree_window`（⎇ メニューの worktree 行）は名前と裏腹に `open_folder_in_rail` を呼び、**worktree を新窓でなくこのウィンドウのレール slot（＝独立 `ProjectSession`＝独立 terminal/editor/explorer）として追加**し、`worktree_branch` も記録していた。`switch_project` で space 丸ごと切替＝ファイル/エディタ/下ターミナル（`self.terminal_dock` は active session を Deref）が追従。つまり「worktree ごとに terminal 開かなくて済む」は機構としては成立済み。**足りなかったのは herd がそれを「ブランチ = worktree」として見せていなかったこと**。
- やったこと（legibility ＋ 正確さ）: ①**herd グループ見出しに ⎇ ブランチ**（`slot.branch` ∥ `worktree_branch`）を出し「プロジェクト一覧」→「**worktree space 一覧**」に読めるように。行の重複 ⎇ は落とし状態のみに（ブランチは space が持つ）。②**`refresh_all_git_status`** 新設 = 起動時に**全 slot**の現在ブランチを背景で埋める（従来 `refresh_git_status` は active のみ＝非アクティブ space のブランチが空だった）。
- 検証（offscreen・**実 git worktree**で）: scratchpad に `git worktree add … -b feat/auth` → `shirushi(main)` と `shirushi-authwork(feat/auth)` の**2 worktree を2 space として起動** → herd に **`● shirushi ⎇ main` / `● shirushi-authwork ⎇ feat/auth`** の2グループ（レールも2色 slot）が出るのを目視。各 space は独立 `ProjectSession`＝独立ターミナル（構造保証・`terminal_dock` は per-session）。worktree は後始末（remove + branch -D、main 1本へ復帰）。`cargo test` 38 green・警告0。
- 学び/罠: **「受入(項目)を満たす」と「ユーザーの目的を満たす」は別物**。#1 の受入「状態一覧が形と動きで左に出る」は満たしたが、ユーザーの目的（worktree spaces）は #3 側で、そこを見ずに「やり切った」と言ったのが失敗。`open_worktree_window` の命名が「window」で実体は「rail に追加」＝**名前に騙されず実体を追う**（危うく「新窓だから未達」と二重に誤診しかけた）。`refresh_git_status` が active-only なのは idle 0% のための設計だが、横断表示（herd/⌘O）には全 slot 版が要る。
- 次: **#3 の残り** = (a) herd/レールから「**＋ worktree を space として開く**」導線（今は ⎇ メニュー経由のみ・mock fleet の ＋ タイル相当）、(b) **編隊グリッド**（複数 space を同時に並べる N セル・今は1 space ずつ切替）、(c) space 切替で下ターミナルが実際に別 cwd になる**可視デモ**（構造は保証済み・offscreen で2枚撮る）。**人の手番**: ⎇ メニュー→worktree を space 化→レール/herd 切替でファイル＋ターミナルが追従する体感。

## 2026-07-21 — 編隊モードを実装（mock の「編隊」ビュー・ネイティブ系譜グラフ）
- 経緯: ユーザーの真の要件が判明 — **mock の「編隊」ビュー（herdr 風＝herd + 系譜グラフ + N分割グリッド + ニュース）にモードで切り替えたい。今ない**。私は #1 で左 herd 板だけ作って的外れだった（要件取り違えを謝罪）。「graph rag／歯車っぽいグラフ」＝ mock の系譜グラフ（扇形）のこと。
- やったこと: **編隊モード**（`chrome.fleet_mode` + レール ⚡ トグル〔`toggle_herd_sidebar`→`toggle_fleet_mode` に転用〕 + `ToggleFleet` アクション + コマンドパレット + `SHIRUSHI_FLEET` プローブ）。ON で root render が通常の center/right dock を `render_fleet` に置換: レール | herd サイドバー（既存 `render_herd_sidebar` 流用）| 中央（系譜グラフ + グリッド + ニュース）。**新モジュール `fleet_view.rs`**。
  - **系譜グラフ（ネイティブ描画・最重要ビジュアル）**: `gpui::canvas` + `PathBuilder`（cubic bezier）で main 幹 + 各レーン（枝）を base から扇状に描く（スレッド色・Idle は破線 `dash_array`）。ノード（`activity_dot`）とラベルは overlay の絶対配置で重ね、canvas の px 座標と `relative(FRAC)` を揃えて一致させる。データは全 space × スレッド（`fleet_lanes`・上限 8）。
  - グリッド = エージェント状態カード（色×状態×トークン・`agent_badge`・`relative(0.485)` の2列）。ニュース = 現在シグナルのティッカー。
- 検証: offscreen（`SHIRUSHI_FLEET=1`+`SHIRUSHI_ACTIVITY_PROBE`）で **main 幹から青/琥珀/紫の3曲線が扇状に分岐 → rope設計(Working)/tab色分け(Waiting)/gpui起動(Done) のノードに着地**するのを目視。左 herd・グリッド・下 Activity・レール ⚡ 点灯も一致。`cargo test` 21 green（i18n parity 含む）・警告 0。
- 学び/罠: **`Pixels.0` は private → `f32::from(pixels)`**。canvas の paint 座標は `bounds.origin` 基準・overlay は container 基準なので、canvas を `.absolute().top_0().left_0().size_full()` で container に重ねると px が一致。overlay の x は `relative(frac)`、canvas 側は `width*frac` で幅未知でも一致する。`PathBuilder::stroke().dash_array()` は Self を返す（`move_to`/`cubic_bezier_to` は `&mut self`）→ `let mut b = PathBuilder::stroke(..); if dashed { b = b.dash_array(..) }; b.move_to(..)`。
- 次（編隊モードの残り）: (a) グラフ表示スイッチャー（リバー/縦ツリー/カード — 今は扇形のみ）(b) グラフのデータを `git log --all --parents` のレーン計算へ（今はスレッド由来＝全 main）(c) グリッドを実ペイン（Agent/Terminal/Editor の実体）に（今は状態カード）(d) ニュースを時系列イベントログに（`PanelEvent` 追記）(e) グラフノード click → focus-follows。**人の手番**: レール ⚡ で通常⇄編隊の切替体感・脈動・実 worktree で枝が別ブランチに分かれる様子。

## 2026-07-21 — M13 remote SSH を Docker で「実環境ブロッカーごと」消化（別セッション・M14 とは非干渉）
- 経緯: ユーザー「M13 やりきって（SSH 系）」+「リモート環境くらい Docker で立てられないのか」。ROADMAP の remote 残件は多くが「実環境必須」で棚上げされていたが、**Docker に sshd + git のコンテナを立てれば end-to-end がローカルで回る**と判明 → 検証ブロッカーごと消化した。M14（herd/fleet）は別 Agent が担当中のため `workspace.rs`/chrome/herd/fleet には触れず、変更は **host / settings / locales / main.rs / docs** に限定。
- Docker ハーネス: `ubuntu:24.04`（arm64 native）+ openssh-server + git + サンプル git repo。テスト鍵と隔離 `ssh_config`/`known_hosts` は scratchpad（ユーザーの `~/.ssh` は汚さない）。
- **配布**: `cargo-zigbuild`（pip の ziglang 同梱 zig をリンカに）で **aarch64-unknown-linux-musl** の `shirushi-remote-server` を建てた（**2.4MB static-pie ELF・15s**）。CI と同じ経路＝配布アーティファクトの実証も兼ねる。配備は `ensure_remote_server` が `~/.local/share/shirushi/remote/artifacts/<triple>/` から自動アップロード。
- **host の変更**（`crates/host/src/host.rs`）:
  - `ssh_command()` ヘルパ + **`SHIRUSHI_SSH_CONFIG`（`-F` 透過）** = ssh 呼び出し 4 箇所（start_master/ensure_master/command/Drop）を集約。CI/サンドボックスで ~/.ssh を汚さず remote テストを回す seam。
  - `start_master` に **`ConnectTimeout=10`** = 死んだ/到達不能ホストの無限ハングを断つ（既定は OS の TCP タイムアウト任せ＝分単位。sleep/VPN 断の復帰性）。
  - **checksum 検証** = 配備後に両端 `sha256sum`/`shasum` でハッシュ突合（crypto 依存を足さず CLI 委譲・不一致は bad binary を消して bail・`parse_sha256_hex` unit test）。
  - **古い server cleanup** = `cleanup_old_servers`（`servers/` 直下の現行 version 以外を rm・容量肥大防止）。
  - **構造化接続ログ** = `ssh_log` で `connect_ssh` の各フェーズを stderr へ所要 ms 付き（`[ssh dest] ControlMaster 確立 (63ms)` …）+ 各フェーズに `.context()`＝失敗時にどの段かがトーストの `{:#}` に出る。
  - `RemoteHost::debug_stop_master`（`#[doc(hidden)]`・障害注入テスト用）。
- **統合テスト**（新規 `crates/host/tests/remote_ssh_live.rs`・`SHIRUSHI_REMOTE_TEST_URI` 未設定なら skip＝CI 無害）: ①**CRUD/検索/command**（list/read_dir/read/write〔revision 一致更新 + stale 拒否〕/search TODO/uname）②**ControlMaster kill → 再接続回復** ③**server kill → 新 daemon + project 開き直しで回復** ④**巨大 tree**（5000 ファイル → `list_files(limit=500)` が **1.3ms** で打ち切り）⑤**benchmark**（round-trip read avg **0.352ms** / remote server 常駐 **RSS 6.5MB** / **idle CPU 0.00%**・localhost 予算で assert）。**全 5 green**。checksum/cleanup は「偽 `0.0.0-p0` を仕込む→再配備で消え現行だけ残る・remote/local sha256 完全一致」を Docker で実証。
- **設定画面**（`crates/settings/src/settings.rs`・非SSH・M13「render_settings 完成」）: エージェント選択に **「動作とエディタ」セクション**を追加＝settings.json を唯一の真実に UI から直接操作（`set_user_value`→永続化 + observe_global 波及）。**トグル5**（submit_on_enter/soft_wrap/format_on_save/agent_auto_name/completion_sound・スライドスイッチ）+ **ステッパー2**（font_size 8–32 / tab_size 1–16）+ **セグメント**（agent_tabs_view）+ テーマ hint。i18n ja/en 追加（parity green）。**検証: offscreen で全コントロールが seed 値を正しく反映**（ON/OFF/16/2/List 選択）を目視（隔離 HOME + `SHIRUSHI_SETTINGS=1` + 新 env `SHIRUSHI_WINDOW_SIZE` で縦長パネル全体を撮影）。
- ドリフト解消: SSH config ピッカー（2026-07-19）・最近のリモートプロジェクト履歴（2026-07-20）は既に done だったが ROADMAP は `[ ]` のままだった → チェック + 実績を追記。
- 学び/罠: **`cargo run` は rustup 解決に実 HOME が要る** → HOME 差し替えのスクショは「先に実 HOME でビルド → ビルド済みバイナリを隔離 HOME で直接実行」。**pkill の out-of-band ssh は exit 255**（自分の ssh セッションも切るため）だが `|| true` + 目的（server kill）は達成＝テストの assert（再接続回復）が真。ヘッドレス Chrome ならぬ Docker sshd で「実環境必須」は多くが**近似 or 実証可能**（sleep/VPN 断だけは実マシン）。同マシンで別プロジェクトの `cargo-tauri tauri dev`（apps/desktop）が並走しビルドが CPU 競合で遅くなる場面あり＝他プロジェクトの cargo は殺さない。
- 残（**honest**・GUI/session 層の深い機能で今回は未実装。ただし「実環境必須」の検証ブロッカーは Docker ハーネスで解消済み）: **remote hot exit（未保存 remote バッファの crash backup・reconnect-on-restore が要る）** / **remote watch push（protocol Event frame 配線 + daemon inotify + subscription/cancel）** / **再接続後の LSP/PTY handle 再同期** / retry-cancel ボタン・port forwarding / GUI askpass（鍵運用なら不要寄り）。総合受入「remote Linux で一日運用」は実マシン長時間が前提。
- 次: 上記 remote 深堀り 3 件（hot exit/watch/handle 再同期）は `remote_ssh_live.rs` の土台の上に載せられる。実 GitHub Release での自動更新 E2E は初回リリース時。

## 2026-07-21 —（続き）編隊: ヘッダートグル + 「増やせる」グリッド（＋/×・自動レイアウト）
- ユーザー指摘（3点）: ①グリッドで「増やせない」（mock には ＋ で Agent/Terminal/Editor を最大8追加がある）②「mock 作りきってるのに実装が空」③入口はレール ⚡ でなく **titlebar 右上に「Multi Agent mode」**。全部対応。
- やったこと:
  - **入口を titlebar 右上へ**: `render_fleet_toggle`（`layout-grid` アイコン + "Multi Agent" ラベル・ON でプロジェクト色ハイライト・`i18n titlebar.multi_agent`）。レールの ⚡ は廃止（前回の「⚡ が見当たらない」も解消）。
  - **編隊グリッドを対話化**（mock `.acell`・M14 #3）: `chrome.fleet_cells: Vec<FleetPane{Agent(lane)|Terminal|Editor}>`。**＋ タイルで Agent/Terminal/Editor を追加・× で閉じる**（上限 8）。列数はセル数で自動（1→1 / 2→2 / 3→3 / 4→2×2 / 5-6→3列 / 7-8→4列）。初回（起動プローブ含む）は lanes で自動配置（`seed_fleet_cells`）。Agent セルはクリックで `switch_project`+`focus_thread`（focus-follows 入口）。セル上線 = 帰属色（Agent=スレッド色 / Terminal・Editor=中立 fg2＝§1.3）。
- 検証: offscreen（`SHIRUSHI_FLEET=1`+`SHIRUSHI_ACTIVITY_PROBE`）で **titlebar 右上 "Multi Agent" トグル点灯 + 3 Agent セル（各 × 付き・上線がスレッド色）+ ＋タイル（Agent/Terminal/Editor + "Add a panel (max 8)"）が 2×2 自動レイアウト**を目視。`cargo test` 21 green・警告0。
- 学び/罠: **ElementId タプルは `(&str, usize)`**（`i32` 不可 → id インデックスは `0usize` 等）。gpui の border 色は全辺共通 → セル上線は先頭子 `div().h(px(2.)).bg(color)` で表現（`border_t_2()` + `border_color()` は全辺を上書きしてしまう）。`border_dashed()` は存在（styled.rs）。
- 次（編隊グリッドの残り）: Terminal/Editor セルに**実ペイン**（`terminal_dock`/editor の実体）を埋める・Agent セルに transcript 数行・フォーカス強調・グラフ表示スイッチャー（リバー/縦ツリー/カード）。**人の手番**: 実機で ＋→セル追加→× で減る→列数が変わる体感。

## 2026-07-21 —（続き・/loop 自走）編隊の作り込み: 4 グラフ表示 / 実端末 / フル画面セル / Editor 削除
- ユーザー指摘（/loop で「最後までやれ」）: ①Editor セルは不要 ②Terminal のプレースホルダは不可＝実体化 ③配置が小さい＝フル画面を使え・セル大きく ④色味/状態管理 ⑤＋ を見た目良く ⑥グラフ4表示（扇形/リバー/ツリー/カード）を全実装 + ON/OFF。全部対応。
- やったこと（`fleet_view.rs` 大改修）:
  - **系譜グラフ 4 表示をネイティブ実装 + スイッチャー**（`GraphView{Fan,River,Tree,Card}` + `graph_collapsed`）。ヘッダに 4 ボタン（扇形/リバー/ツリー/カード）+ ⌄ 折り畳み。`graph_layout()` が view 別にノード（割合座標）と枝（cubic bezier 制御点）を生成 → 共通 canvas paint + overlay。**Fan**=base から扇状 / **River**=同形で太帯（5.5px）/ **Tree**=main 上・枝が下へ扇 / **Card**=base 左・曲線で右のノードへ + 幹を HEAD まで。`SHIRUSHI_GRAPH=river|tree|card` で初期表示指定（撮影用）。
  - **Editor セル削除**（`FleetPane{Agent|Terminal}` の 2 種に）。
  - **Terminal セルに実端末を埋め込み**（`self.terminal_dock.clone()` を flex_1 で・プレースホルダではない）。offscreen で `⎇ main zsh` ヘッダ + 実端末ウィジェットを確認。
  - **フル画面セル**: グリッドを「行×列の flex」に（各セル `flex_1` で幅・高さとも均等に伸びる・列数はセル数で自動）。従来の固定 148px から画面全体を使う大セルへ。
  - **Agent セルに実 transcript**: `agent_panel::transcript_lines(thread_index, 6)`（▸user/✳thinking/⏺step/⟲checkpoint）を追加し、セル本文にスクロール表示（本物の会話・rope設計 が ropey/sum-tree の検討を表示）。
  - **＋ タイル磨き**: 円形 ＋ + Agent/Terminal ボタン + 「パネルを追加（最大 8）」。
- 検証: offscreen で **Fan/River/Tree/Card の 4 表示**（スイッチャー点灯）・**実端末セル**・**フル画面の大セル + 実 transcript** を目視。`cargo test` 30 green・警告 0。
- 学び/罠: canvas を割合座標（0..1）で統一すると 4 view を 1 つの paint で描ける（overlay は `relative(frac)`+`px(yf*body_h)` で一致）。`gpui::canvas` の `Styled` で `.absolute().size_full()` 可。同一 `terminal_dock` Entity を編隊セルに描くのは通常ビューの下ドックが非表示なので競合しない。
- 次: グラフのデータを `git log --all --parents` の実レーンへ（今はスレッド由来＝全 main・worktree を切れば別ブランチ枝に）・複数 Terminal セルの独立端末化・完了ノードの main 統合アニメ。

## 2026-07-21 —（続き）編隊: ＋Agent の複製バグ修正（herdr 式=新エージェント）+ セル拡大表示
- ユーザー指摘: ①「＋Agent で tab色分けがどんどん複製される」＝バグ（モックではない）②「herdr をよく調べてほぼ同じ挙動に」③「あるセルを拡大＝他は小さく避ける・拡大中は系譜グラフを消してそこに出す」。
- 原因（①）: `FleetPane::Agent(lane_index)` が**既存レーンの添字**を指し、スレッドが 3 本しかないので 4 個目以降が既存を複製していた。
- 直したこと:
  - **`FleetPane::Agent { space, thread }`（安定参照）**へ変更（レーンの並び替え/追加で添字がズレない）。ルックアップは `lanes.iter().find(space,thread)`。
  - **＋Agent = 新しいエージェントを起動**（herdr 式）: `agent_panel::new_thread_index()` を追加し、アクティブ space に**新スレッドを作って**セルを足す（複製ではない）。offscreen で `SHIRUSHI_FLEET_ADD=2` → **スレッド4/スレッド5 が新規に増える**（tab色分け の複製でない）ことを実証（herd/グラフ/グリッド/beacon すべて 5 本に）。
  - **セル拡大表示**（ユーザー案どおり）: 各セル頭に ⤢（maximize.svg）/ ⤡（minimize.svg）。拡大中は **`fleet_maximized: Option<usize>`** で ①**系譜グラフを隠し**②拡大セルをそこに大きく出し③**他セルは上のサムネイル列**（クリックで拡大先を切替・アクティブは accent 枠）へ「避ける」。閉じ/範囲外は自動解除。`SHIRUSHI_FLEET_MAX=i` で撮影。offscreen で rope設計 を拡大＝全 transcript が大画面 + 上に 3 サムネイルを実証。
- herdr 整合（クリーンルーム・記事/挙動から）: 「エージェント＝各セッション（worktree/スレッド）、＋で新エージェント、状態一覧、切替」。今回 ＋Agent を新エージェント化してこの中核挙動に一致。worktree ごとの space は既存（M14 #3 の①）。
- 検証: `cargo test`（実行中）・警告0（my crates）。offscreen 2 枚（＋Agent 複製なし / 拡大表示）を目視。
- 次: ＋Agent 起動時に worktree も切る選択（herdr の worktree=space に寄せる）・拡大中のサムネイルに状態ドット追加・複数 Terminal の独立端末。

## 2026-07-21 — M13 remote SSH の深掘り3機能（remote watch push / hot exit host-scoped / handle 再同期・別セッション続き）
- 経緯: 前段（Docker で remote 受入・障害注入・checksum/cleanup/benchmark を消化）に続き、ユーザー「GUI/session 層の深い3機能もしっかり」。M14（herd/fleet）は別 Agent が並行編集中のため、変更は **host / project / storage / main.rs** 中心に置き、workspace は `project_watcher.rs`（remote skip 撤去）と `editor_area/hot_exit.rs`・`tabs.rs`（scope 配線）のみ。途中 M14 の `fleet_view.rs:584`（`i18n::t!` 構文ミス）で workspace が一時ビルド不能 → 触らず待ち、直った後に自分の変更を確認（＝並行編集の作法どおり）。
- **① remote watch push**（最重要・protocol 変更）: `PROTOCOL_VERSION 1→2`。protocol に `Request::Watch/Unwatch` + `WatchEvent`（`FrameKind::Event` の meta）。**daemon**: `serve_stream_with_state` の接続スコープに watch マネージャ thread — 無 watch 時は `recv()` で完全ブロック（idle 0%）、使用中のみ 700ms で `watch_snapshot`（ignore 走査で mtime/len map）→ `watch_diff` → Event frame を**共有 writer** へ push。proxy は**バイト透過**（`io::copy`）なので Event はそのまま client へ流れる。**client**: `RpcClient` reader が `matches!(kind, Event)` を `WatchEventSink` へ demux（id ルーティングと分離）。sink は `ReconnectingClient` が Arc で保持＝**RpcClient を再接続で差し替えても購読が生き続ける**。`RemoteHost::watch()`→`HostWatch`（recv_timeout）、keeper thread が `generation`（再接続で ++）の変化を見て**自動再購読**（`send_watch` は scoped_request 経由で daemon 再起動の project_id 再マップも吸収）。**workspace**: `project::watch_root` を `(host, root, on_paths)` に host 対応化（`Watch` を enum 化: Local=notify / Remote=pump スレッドが HostWatch 所有・drop で stop）。remote は相対→絶対化して既存 pump へ流す＝`handle_watch_events`（host 非依存）をそのまま流用。`project_watcher.rs` の `if is_remote { return }` を撤去。**live 実証**: out-of-band 編集→`["README.md"]` 通知 / ControlMaster kill→再接続→再購読→`["docs/notes.md"]` 通知。
- **② remote hot exit（host-scoped）**: storage `hot_exit` に **`scope` 列**（PK=(scope,path)）+ 旧スキーマからの作り直しマイグレーション（`SELECT scope` の成否で検知・transient なので安全）。`save/remove_hot_exit(scope, path, ...)`・`load_hot_exit_all()→(scope,path,content)`。workspace: `active_hot_exit_scope()`（host.id()）で snapshot/close を scope 付き保存、restore は**アクティブ host の scope に一致する候補だけ**適用（local と remote の同一絶対パスの取り違え＝データ破壊を防ぐ）。**unit test: 同一パス×別 scope が別レコード・片方消しても他方残る**。
- **③ LSP/PTY handle 再同期**: `spawn_process`/`terminal_launch` は元から `ensure_master` を呼ぶ＝再接続後の再 spawn は成立していた。`HostProcess::is_alive()`（`child.try_wait()`）を追加＝consumer が死を検知して再 spawn する土台。**live 実証: spawn(echo;sleep)→stdout 受領→ControlMaster kill→再 spawn で stdio が通る**。
- テスト: `remote_ssh_live.rs` に watch×2 + process 再spawn を追加＝**実 SSH live 8本 green**（CRUD/ControlMaster kill/server kill/巨大tree/bench/watch×2/process）。storage 9・host unit 8・workspace/shirushi ビルド green。
- 学び/罠: **daemon の writer は元から `Arc<Mutex<&mut>>` 共有**＝watcher thread から Event push が自然に載る（設計の勝ち）。**proxy がバイト透過**なので Event frame 用の proxy 変更ゼロ。**event sink は ReconnectingClient に置く**のが肝（RpcClient に置くと再接続で購読が切れる）。keeper の再購読は `generation` 監視で「再接続が起きた時だけ」＝poll 毎の baseline リセット spam を避ける。`Arc<Self>` receiver の trait メソッド（`fn watch(self: Arc<Self>)`）は object-safe＝`Arc<dyn Host>` から呼べる。**PROTOCOL_VERSION を上げると socket path と install dir が変わり p1/p2 が自然分離**（旧 daemon と衝突しない・cleanup で旧 server 消える）。並行 M14 のビルド破損は「待って触らない」。
- 残（honest・consumer/GUI 層）: LSP の死亡→自動再 initialize（didOpen 再送）・terminal の自動再起動・remote 未保存の**起動時 reconnect 復元**（scope 衝突は解消済み＝あとは「起動時に該当 remote へ自動再接続して書き戻す」フロー）。いずれも今回の host 土台（watch/is_alive/scope）の上に載る。

## 2026-07-21 —（続き）編隊: ＋Agent に worktree 選択（ハイブリッド B ＋ 設定で A 化）+ herd 追従の穴修正
- ユーザー議論の決着: 「＋Agent 起動時に worktree を切るか」を **③ハイブリッド = 自分で選ぶ**、その上で **B（明示オプトイン）＋ 設定で A 化**（既定=素の ＋Agent は同 space・設定 on で常に worktree）を採用（「ハイお願いします！」）。worktree 生成は **ACP エージェントに切らせず、アプリが所有**（herdr 式の worktree=space に寄せつつ、どのエージェントでも成立させる）。
- やったこと:
  - **＋タイルを 3 ボタン化**: `Agent（ここ）` / `Agent＋⎇worktree（隔離）` / `Terminal`（`fleet.add_agent`/`add_agent_worktree` i18n・ja/en parity）。
  - **`add_worktree_agent`**: アクティブ worktree の親に `git worktree add <repo>-agent-<n>`（branch `agent/<n>`・既存 space のブランチ集合と衝突しない番号）を **background_executor で作成** → `open_folder_in_rail(host, target, Some(branch))` で **space（`ProjectSession`）として開き** → その space の `agent_panel.new_thread_index()` で**新エージェント起動** → セル追加。失敗はトースト（`{error:#}`・`push_toast`）。既存機構（`project::add_worktree_on` / `open_folder_in_rail`）の再利用＝移植なし。
  - **設定 `fleet_agent_worktree`（既定 off）**: on で **素の ＋Agent（`add_fleet_agent_default`）も worktree を切る**＝herdr 純度 A に寄せられる（`settings::get(cx).fleet_agent_worktree` を見て分岐）。`settings_core` に bool 追加・`settings.json` 唯一の真実。
  - **穴①「Fleet 列タップで無反応（× で消すと戻せない）」修正**: 編隊モードでは herd 行クリック = `reveal_agent_in_fleet(space, thread)` = **該当エージェントのセルが無ければ追加し、あれば最大化**（通常モードは従来どおり `switch_project`+`focus_thread`+`show_right`）。閉じたセルもここから復帰可。
  - **穴②「新規エージェントの ー 表示」修正**: トークン 0 のセルも `0/200k` メーター表示（旧 `—`）。
- 検証: host.rs（M13 remote-watch の並行 WIP）が `handle_request` の `Request::Watch/Unwatch` 非網羅で一時ビルド不能 → **触らずユーザーに修正を委ね**、直った後にビルド。`cargo build -p shirushi --features screenshot` green（8.9s・警告0）。**offscreen（`SHIRUSHI_FLEET=1 SHIRUSHI_FLEET_ADD=2`）で目視**: titlebar「Multi Agent」点灯・**＋タイル 3 ボタン（Agent（ここ）/Agent＋⎇worktree/Terminal・max 8）**・＋Agent が **スレッド4/5 を新規生成（複製なし）**・全セル `0/200k` メーター・Fan グラフ ＋ 5 ノード・rope設計 セルに実 transcript・左 Fleet 一覧 ＋「スレッド5's worktree」フォーカスツリー。`cargo test -p i18n -p workspace -p agent_panel` green（workspace 15 / i18n parity 5 ＋ doctest 1 / 0 failed）。
- 学び/罠: worktree 生成は **同期 API を `cx.background_executor().spawn` に逃がして UI をブロックしない**（`add_worktree_on` は fs/git を叩く）。`open_folder_in_rail` は名前に反して**新窓でなく現ウィンドウのレールに space 追加**＝そのまま編隊 space の追加口に使える（`open_worktree_window` は誤称）。**並行編集中の他 crate（host.rs）が壊れても自分の crate は触らず待つ**の作法をまた実践（前回の `fleet_view.rs:584` 破損の逆の立場）。
- 次: worktree エージェント完了時の `main` への merge 導線（#3-4 の「完了を待って merge」）・拡大中サムネイルに状態ドット・系譜グラフを `git log --all --parents` の実レーンへ（worktree を切ったので**別ブランチ枝**が出せる素地ができた）。

## 2026-07-22 — 編隊: 承認待ち＝半円（mock ◐ 準拠）+ herd 行からエージェント削除
- ユーザー指摘: ①「waiting を半円にして」②「セル（スレッド）を作って × 押しても左のリストから消えない＝どう消す？」③「Fleet って書き方合ってる？この波アイコンも意味不明（看板みたい）」。①②を実装、③は下記の判断待ち。
- ① **承認待ち（Blocked）= 半円**（`activity_dot`）: mock `index.html:622` が `glyph 形 = ● 作業 / ◐ 承認待ち / ✓ 完了 / ○ 待機` と定義済み＝**実装がモックに追従**。状態を「色」でなく「形」で見せる原則（§1.3）に、Blocked が今までリング（Done と枠が同形で紛らわしい）だったのを**独立の形**に。**罠**: 高さ=直径/2・上角を半径 直径/2 で丸める素朴な実装は、7px（凡例）で角丸半径が高さでクランプされ**横カプセル（▬）に潰れる**。→ **フル円を高さ半分の箱で `overflow_hidden` clip** して上半分＝ドームにする方式へ（どの径でもくっきり半円）。offscreen で凡例 4 状態（満/半円/リング/淡）を 4 倍拡大目視。`activity_dot` は全所（タブ/List/beacon/レール/フッター/⌘O/herd/グラフノード/セル上線）共用なので一括で半円化＝一貫。
- ② **herd 行からエージェント削除**: 「セルの × ＝グリッド表示から外すだけ（エージェントは生存）」と「エージェント自体の削除」を分離。**左 herd 一覧の各行にホバー × を追加**＝これが本当の削除（herdr の「エージェントを閉じる」）。`agent_panel::close_thread(index)`（アーカイブ＋`remove_thread`・`close_active_thread` もこれを呼ぶ形に集約）を公開し、workspace `close_agent(space, thread)` が呼ぶ。**罠（重要）**: `fleet_cells` は**絶対 thread 添字**を持つので、スレッド削除で `threads.remove` が 1 つ詰まる → 削除後に ①該当セルを落とし ②同 space の thread>削除位置 のセルを 1 つデクリメント（前回の「Agent(lane_index) 複製バグ」と同じ添字ズレの罠）。アーカイブなので ⌘⇧T で復元可。ホバー × は agent_panel タブと同じ `group("herd-row")`+`group_hover(..visible())` パターン・`cx.stop_propagation()` で行本体（reveal/switch）へ伝播させない。offscreen で ×（一時常時表示）が各行右端に出るのを目視 → ホバー限定に戻す。
- ③ **「Fleet」見出し＋波アイコンを削除（ユーザー選定＝モック準拠）**: mock の herd パネルには「Fleet」見出しも波アイコン（`activity.svg`）も**無い**（`.ghead`＝プロジェクト群見出しに直行）＝あの見出しは実装の過剰追加だった。加えて語彙も不整合（titlebar トグル ja「編隊」/en「Multi Agent」・左パネル ja「編隊」/en「Fleet」＝ja で二重・en で不一致）。→ `render_herd_sidebar` の見出し（総計計算＋`header` div）を撤去し、パネルは**群見出しから直開始**。モード名はタイトルバー「Multi Agent」トグル、稼働数ロールアップは下ステータスバー中央が既に担うので情報損失なし。未使用になった i18n `herd.title` を ja/en 両方から除去（parity 維持）。`activity.svg` の登録（main.rs）は無害な死アセットとして残置（並行編集中の main.rs は触らない）。offscreen で「● AltProject 2」群見出しから直行するのを目視。
- 検証: `cargo build -p shirushi --features screenshot` green（警告0）。`cargo test -p i18n -p workspace -p agent_panel` green（agent_panel 9 / workspace 15 / i18n parity 5 ＋ doctest 1 / 0 failed）。
- 学び/罠: gpui の角丸は**小径で min 寸法にクランプ**＝ドーム/半円は clip で作る（角丸ではなく）。`overflow_hidden` は子を矩形 scissor で切る＝フル円の上半分＝きれいなドーム。`fleet_cells` の絶対添字はスレッド増減のたびに補正が要る（追加＝新添字・削除＝後続デクリメント）。**「mock に無い＝作らない」**を徹底（見出しは実装が勝手に足したもの＝ユーザーの違和感が正しかった）。
- 次: 拡大中サムネイルに状態ドット。系譜グラフを実 git レーンへ。完了エージェントの main merge 導線（#3-4）。

## 2026-07-22 — M13 ベータ前の信頼性: panic hook→バグ報告導線 + editor_core ファズ + データ完全性
- やったこと:
  - **panic hook + クラッシュログ + バグ報告導線**（`workspace::crash` 新設・ROADMAP M13 に記録）: main() 冒頭で hook → `crashes/crash-<unix秒>-<pid>.log`（backtrace 付き・20 本保持）+ `pending` マーカー → 次回起動で 1 回だけ statusbar ⚠ チップ（`theme.warn`）→ クリックで GitHub new issue（環境+ログ抜粋を事前記入・自動送信なし = telemetry never 維持）。⌘⇧P「ヘルプ: バグを報告」（`ReportBug`）と `.github/ISSUE_TEMPLATE/bug_report.yml` も。**実 E2E 検証** = `SHIRUSHI_PANIC_PROBE` で背景スレッドを panic → ログ生成 → 再起動チップを offscreen 目視 → マーカー 1 回消費まで確認
  - **editor_core ランダム操作ファズ**（`tests/random_edits.rs`・依存ゼロ SplitMix64）: 厳密系（insert/delete/undo/redo × 参照実装 String+状態スタックと毎手突合）+ 全部系（18 操作乱打 × 大域不変条件）。`SHIRUSHI_FUZZ_SEED/ITERS` で再現・強化。**初回実行で実バグ 1 件検出** → `move_lines`/`duplicate_lines`/`indent_lines` の手動選択シフトを `set_selections`（クランプ）経由に修正
  - **データ完全性テスト**: prepare/write 間競合・削除後 reload・save_as の競合束縛を追加、既存 round-trip に自己保存の `disk_probably_unchanged`=true と 2 周目保存を追記。**保存意味論を 1 点改善** = 外部削除は競合にせず ⌘S で作り直し（作業救出・`write_file_local` 一箇所 = local/remote 共通・VSCode 同挙動）
- 学び/罠:
  - **ファズは書いた直後に元が取れる**: 全部系の初回実行で選択シフト系の boundary バグを即検出（multi-cursor × マルチバイト × 行操作の交差 = 手書きテストでは書かない組合せ）。失敗メッセージにシードを埋めておくと再現が 1 コマンドになる
  - ファズの「redo 全進め = 最終文」は**ウォークが undo で終わると偽陽性**（redo スタックに未来が残る）→ undo した回数だけ redo する
  - テスト追加前に**既存テストの重複チェック**（今回 prepare/complete round-trip・reload 系が既存にあり、temp_path のタグ衝突で並列実行が踏み合う事故も）。同タグの temp ファイルは pid が同じ = 衝突する
  - `WriteCondition::Matches` の削除判定は「読めない」でひとまとめだった → `path.exists()` で「消えた（作り直し可）」と「読めない（判定不能 = 拒否）」を分ける
- 次: RELEASE-CHECKLIST.md（ROADMAP の「人の手番」21 箇所の回収リスト）→ ドッグフーディング週間 → v0.0.1 タグ（repo public 化 + Apple secrets が前提・前セッションの整理どおり）

## 2026-07-22 — 編隊: 履歴からの会話復元 + 開始/最終入力時刻の常時表示（ユーザー要望 2 件）
- 要望: ①セル/エージェントを消せるようになった今、Agent パネル同様に**履歴（セッション管理）で前の会話を復元**できるべき ②**いつスタートして最終いつ入力したか**をサクッと見たい（multimode で）。
- ① 履歴復元を編隊に配線: ＋タイルに「**履歴から復元**」ボタン（既存 ThreadHistory Picker をそのまま開く — overlay は編隊モードでも root render の最前面に乗る構造だったので追加コストほぼゼロ）。`agent_panel::open_thread_from_history` が**開いた thread index を返す**ようにし、編隊モード中の確定は `reveal_agent_in_fleet(active space, index)` で**復元した会話をセルとして前面（拡大）に出す**（通常モードは従来どおり右ドック）。herd 行 × の削除はアーカイブなので、履歴（·閉 表示）からいつでも戻せる＝セッション管理が閉じた。
- ② 時刻: `Thread.created_at_ms / last_input_at_ms`（送信時に更新）を新設し `AgentStatus` → `FleetLane` へ露出。**DB 往復はスキーマ変更なし** — `threads.created_at` と `MAX(turns.created_at WHERE role='user')` のサブクエリで導出（`load_threads` / `load_all_threads` に列追加・末尾追加なので既存タプル添字は不変）。表示 3 箇所: 編隊セル状態行の右肩「開始 6分前 · 入力 5分前」/ herd 行は幅が狭いので**最新の 1 つ**（入力があれば入力・なければ開始）＋ホバー tooltip で両方 / 履歴 Picker の detail。整形は `agent_panel::relative_time_label`（i18n `time.*`）。
- 鮮度: 相対時刻は描画時評価なので、編隊/herd 表示中だけ回る **30 秒時計**（`ensure_fleet_clock`・両モード閉じたら次 tick で自停止＝idle 0% 予算を守る。多重起動は `chrome.fleet_clock` ガード）。
- 検証: storage round-trip テスト拡張（created_at>0・user turn の有無で last_input Some/None）・i18n parity 含む全対象 crate green・警告 0。offscreen 3 枚目視（en/ja/`SHIRUSHI_HISTORY_PROBE`）: セル右肩の時刻・herd 行「実行中 · 開始 6分前」・＋タイル 4 ボタン・Picker detail「Σ 23.4k / started 5m」。**再起動往復も実証**: probe DB の 2 回目起動で「started 4m ago」が DB から復元された。
- 学び/罠: herd 行の 2 行目に**両時刻を並べると en で確実に溢れる**（"Working · started 2h ago · last input 5m ago"）→ 行は最新 1 つ＋tooltip に逃がし、en の相対語も「4m ago→4m」へ短縮（ja「4分前」は元々短い）。種スレッドは**メタのみ永続**（mock 会話は turns に書かない既存設計）なので復元後の last_input=None は正しい挙動。`fetch_add(1)` を 0 と書き間違える凡ミスをリファクタ中にやった（id 連番が全部 0 になる）— 抽出リファクタでも定数は目視確認。
- 残/人の手番: グリッド満杯（8 セル）時は履歴から復元してもセルは出ない（スレッドは開くので herd 行から reveal 可・toast 通知は未）。実クリックで「履歴から復元」→ 拡大表示・herd 行ホバーの tooltip・30 秒での時刻更新の体感。

## 2026-07-23 — 「＋」統一オープン + リモート browse-first（現レールに開く）

- 経緯: ユーザー「＋ で履歴/ファイルを選べるように・remote は新フォルダをどう開く・**プロジェクトでない接続は単なるファイルブラウザーで開いて『ここで開く』させたい**（cd 連打→開き直しを消す）」。承認済み mock（Artifact）を実装しきる方針。
- やったこと:
  - **「＋」統一オープン**（`PickerMode::OpenLauncher`）: 狭い popover に履歴を詰めず、⌘O と同じ広い `.palette` に **固定アクション（フォルダ/ファイル/リモート）＋ 最近（local + remote を opened_at 降順でマージ・上位 20）** を 1 枚で。最近行は **●識別色 ＋ 実行中スレッドのドット**（色による方向感覚）、リモートは `host:path` で識別。⏎ = 現レールに開く。`storage` に **`local_projects`**（path PK・`record_local_project`/`recent_local_projects`/`forget_recent_project` + round-trip test）を新設し remote 側 `remote_projects` と混ぜる。「＋」は従来のローカル native ダイアログ限定から統一ランチャーへ（tooltip `rail.add_tip` も更新）。
  - **リモート接続の既定を「現レールに開く」へ**: `connect_ssh_and_open` の成功を `open_source_as_window`（新窓）→ **`open_folder_in_rail`（現ウィンドウのレール・SSH 接続を再利用）** に。ユーザー要件「勝手に窓を開かない」。新窓は explorer 右クリック「新しいウィンドウで開く」で明示。
  - **remote browse-first**: home（path 未指定）接続は「ブラウズ入口」＝履歴に残さず、接続後に **hint toast**（`ssh.browse_hint`）「フォルダを右クリック→『ここをプロジェクトとして開く』」。explorer のフォルダ右クリックに **「ここをプロジェクトとして開く」（`open_dir_in_rail`）** を新設 = 現レールに再ルート・`host_for_project` で**同じ SSH 接続を再利用（再接続なし）**。= home に繋ぐ→ツリーを辿る→このフォルダを開く、の最後の一歩。local/remote の「開いた」を `record_recent_project`（remote host key は display_name から復元し `connect_ssh_and_open` の記録キーと一致）で最近に記録。
- 学び/罠: **別セッション（ユーザー手動 or 別 Agent）が同じ launcher を同時にライブ実装**していて衝突した。私と相手が**互いに自分の `open_launcher` を削除**（相手の版が残ると仮定）＝一時ビルド不能、メソッド名も私の 2 回の読み取りの数秒間に `open_file_via_dialog`→`open_file_from_launcher` へ変化。**収束点 = 私の `open_launcher`（●/ドット付き）＋ 相手の `open_file_from_launcher`/dispatch/＋配線 ＋ 私の storage 土台**。教訓どおり「編集直前 re-read・exact match」で着地したが、**ライブ churn を検知したら盲目編集を止めユーザーに分担を確認**するのが正解（duplicate method は soft でなく hard conflict）。`open_folder_in_rail` は M14 の `fleet_view` も呼ぶのでシグネチャは変えず、browse は struct フラグでなく hint toast で非侵襲に。
- 検証: `storage` 11 / `workspace` 21 test green・i18n parity（ja/en 322 = 322・diff 0）・`cargo check` 警告 0。**残/人の手番**: ライブの「＋→最近→現レールに開く」体感・Docker 実 SSH での「home 接続→右クリック『ここで開く』→接続再利用でレールに出る」実地・mock 反映。

## 2026-07-23 — M13 macOS ネイティブメニューバー（メニュー非連動の解消）
- やったこと:
  - **メニューバー全体を実装**（ROADMAP M13 に記録）: `shirushi/src/menus.rs`（Zed と同じ bin crate 置き場）に 8 メニュー（Shirushi/File/Edit/View/Go/AI/Window/Help・計 84 項目）+ Dock メニュー。全部**既存アクションの参照**で、keymap/パレットと同じ dispatch 系＝キー表記も gpui が keymap から自動付与。Cut/Copy/Paste/Undo/Redo/SelectAll は `OsAction` 対応付き
  - **新設アクション 7 + バインド 4**: `OpenSettings`（⌘,）/ `About` / `Hide`（⌘H）/ `HideOthers`（⌥⌘H）/ `ShowAll` / `Minimize`（⌘M）/ `Zoom`。handlers は workspace root の on_action 鎖（cx は App へ deref するので cx.hide() 系がそのまま呼べる・Minimize/Zoom は window メソッド）
  - i18n `menu.*` 72 キーを ja/en 両方に（parity green）。ユーザー keymap live reload 時に `set_menus` を呼び直してキー表記を追従
- 学び/罠:
  - **メニューバーは offscreen スクショに写らない**（OS 側の描画・render_to_image はウィンドウのみ）。screencapture / AppleScript(System Events) はこのセッション文脈では画面収録・補助アクセス権限で不可 → **`cx.get_menus()` 読み戻しが機械的証拠になる**（gpui_macos の set_menus は `setMainMenu_` と同時に owned copy を保持）。`SHIRUSHI_MENU_PROBE=1` として恒久化
  - `cx.set_menus` は**呼び出し時点の keymap スナップショット**でキー等価を解決 → 既定+ユーザー keymap の bind より後に呼ぶ・live reload 後は再設定が要る
  - gpui の git rev（5f8a741）はプラットフォーム実装が `gpui_macos`/`gpui_platform` に分離済み（ローカル zed/ クローンとはレイアウトが違う）— API 調査は `~/.cargo/git/checkouts/` の実 rev を見るのが確実
  - 新設アクションの dispatch 検証は**パレット実駆動**（SHIRUSHI_PALETTE_PROBE + CONFIRM）が使える＝メニュークリックと同じ `dispatch_action` 経路
- 次: 実メニューのクリック一巡（人の手番・1 分）。RELEASE-CHECKLIST.md → ドッグフーディング → v0.0.1

## 2026-07-23 — 承認待ちマスコット 2 種（祈る / 頬に手）を生成 → agent panel に配線（M14・別セッションの mascot 続き）
- 経緯: 編隊モードに猫が居ない → 「各セルに猫は散る」ので不採用、代わりに**承認待ちの気分マスコット**を作る流れ。既存 4 モーション（打鍵/考える/バンザイ/うとうと）に無い「承認待ち」用を、確立パイプライン（基準絵 `gpt/01-neko.png` → Higgsfield `kling3_0` image→video・start=end ループ → `video_to_sprite.py` 共通窓切り出し＋K=32量子化＋透過 → 60×72 strip）で追加。
- **モーション設計の学び（ユーザー FB）**: 初回「微動作の worried squint」は**他の座りモーションと被る**と却下 → **万歳級の大きな動き**へ。さらに Higgsfield は別アカウント接続で残高 97→1186 に増えたので、被らない 4 種（頭を抱える/PC覗き込み/祈る/いやいや）を各 2 テイク生成して見比べ。**PC覗き込み・祈るは座り姿勢に近く地味／頭を抱える・いやいや・頬に手はダイナミック**。
- **採用＝2 種（ユーザー選定）**: **祈る（plead・手を組んで頼む）** ＋ **頬に手（worry・あわあわ）**。頬に手は初回テイクに上端アーティファクトが出たので、プロンプトに「手は頬の高さまで・上端に余白・浮遊物なし」を足して 4 テイク作り直し→クリーンな 1 本（D）を採用。
- **配線（`crates/agent_panel/src/agent_panel.rs`）**: `MascotMotion::{Plead,Worry}` 追加 ＋ `render_mascot` に `plead/worry-strip.png` の anim/frame0 アーム。`PendingPermission` に **`since: Instant`** を足し、motion 決定を **承認待ち（`pending_permission.is_some()`）→ 祈る、`since.elapsed() >= 15s` → 頬に手であわあわ**の段階変化へ（`render_meta` は active 中アニメで毎フレーム再評価されるので閾値跨ぎで自然切替）。status 行も Blocked 時「承認待ち」表示に（従来は誤って「待機中」）。**検証 env `SHIRUSHI_MASCOT=plead|worry|…`**（`cfg!(debug_assertions)` ゲート＝release 未評価）でモーション固定してスクショ可。
- 検証: `cargo build -p shirushi --features screenshot` green（警告0）。`cargo test -p agent_panel -p i18n` green（9 / parity 5+1）。offscreen で agent panel のマスコットが崩れず描画（承認待ちの**半円ドット◐**も thread list に出る）。**罠: オフスクリーンは window 非アクティブ＝`frame0`(rest ポーズ)固定**＝祈る/頬に手の中割り（見せ場）は静止スクショに写らない → gif（アーティファクト公開済み）と実機で確認。
- 素材: strip 2 本 ＋ ソース `mock/mascot/neko-anim/{video/{worry,plead}.mp4, vid-worry, vid-plead}`。プレビュー artifact 3 枚（編隊ムード / バリエ5案 / 頬に手クリーン4案）。
- 次: 編隊の集約"気分"1 匹（Activity 隅・誰か承認待ち→祈る/長引き→頬に手）＋ maximized セル内の 1 体表示（今回の panel 配線が土台）。名前（necoder/shirushi）は保留中でも nyaco は残す方針。

## 2026-07-23 — 管制 P0: 縫い目の一本化（TaskPhase 単一定義・遷移コードパス統一）
- やったこと（`FLEET-CONTROL-PLAN.md` P0 完了・UI 変更なし）:
  - **`TaskPhase`/`SpaceKind` を storage crate へ単一定義**（10 phase・`ALL`/`as_str`/厳密 `from_str`）。`fleet.rs::PHASES`（文字列配列）と `workspace::TaskPhase`（11 値 enum）の二重定義を解消。workspace は `pub use storage::{SpaceKind, TaskPhase}` の再輸出（子モジュールは `use crate::workspace::*` なので無修正で追従）
  - **Integration を phase から分離**: `TaskSpace`/`TaskSpaceRecord` に `kind: SpaceKind` を追加し `phase` は Task lifecycle 専用に。DB は phase 列に `integration` sentinel を書く互換表現（migration 不要・storage 内に封じ、`parse_phase_column` で復元）
  - **遷移の唯一の入口 `Storage::commit_task_transition`**: snapshot upsert + `task_events` への `phase_changed` 追記を同一 transaction で行い、IntegrationSpace の遷移は ensure で拒否。GUI `transition_task_space`（payload source="gui"）と CLI/MCP `update_task`（source="orchestration_api"）の両方がここを通る＝受入条件「同一コードパス」達成。旧 `transition_task`（UPDATE のみ・upsert と別 job）は廃止
  - 境界の文字列 parse は `fleet::parse_phase`（不正値は有効値一覧つきエラー）に集約。MCP の `fleet_update_task`/`fleet_wait_task` inputSchema に `"enum"` として `TaskPhase::ALL` を埋め込み（エージェントが正しい値を選べる）
  - 検証: `cargo check` 警告 0・storage 11 / workspace 21 / shirushi 2 test 全 green（storage テストに Integration 拒否 + sentinel 復元の 2 ケース追加）。CLI 実地: scratch repo で `fleet create`→`status working`→不正 phase 拒否 を確認
- 学び/罠:
  - `cargo run --manifest-path` を**リポジトリ外の cwd から叩くと rustup がその cwd でツールチェインを解決**し、rust-toolchain.toml（1.95.0）が効かず gpui_util の `slice_as_array` で落ちる。リポジトリ内から実行するのが正
  - restore 時の真実の分担を明文化: **lifecycle（phase）は台帳が正・kind は worktree の現実（branch が task/ か）が正**。台帳の kind を盲信すると branch 付け替え後に鮮度が逆転する
- 次: P1 遷移スナップショット Tier 1（digest・LLM なし）— `Thread.digest` + `AgentStatus` 拡張 + herd 行/編隊セルへの表示

## 2026-07-23 — 管制 P1: 遷移スナップショット Tier 1（digest・LLM なし）
- やったこと（`FLEET-CONTROL-PLAN.md` P1 完了）:
  - **`Thread.digest`**: 状態遷移時のみ更新する決定論スナップショット。素材の優先順位は計画どおり — ① Blocked=`PermissionRequest.title`（承認カード表示と同時に記録）② Done=最終 `Entry::Agent` の末尾 1〜2 文（`digest_tail`・素材なしターンは直前値を保持）③ Failed=エラー文（`fail_turn` と `AgentEvent::Failed` の両経路）
  - **`digest_tail`（純関数）**: 最終段落 → 文区切り（。．!?！？ + 「. 」）→ 末尾 2 文 → 空白畳み → 140 字超は先頭 … 切り。パス中の `.`（src/main.rs）で切らない。単体テスト 5 ケース
  - **Working はライブ素材** `live_digest()`: 最新 `Entry::Step` のツール説明 + plan の in_progress 項目を「tool · step」で合成。保存せず `statuses()` で毎回組む＝「今なにをしているか」が常に生きた値
  - **表示**: herd 行の sub を digest に差し替え（無ければ従来の「状態 · 時刻」・時刻はツールチップに退避済み）。編隊 Task セルはヘッダ直下に digest 帯 + `plan_meter`（▰▰▰▱▱ 3/5・色相を使わず文字グリフ＝規律どおり）
  - **永続化**: `PanelEvent::TurnEnded{digest}` / `PermissionWaiting{title}` を追加 → `transition_task_space(…, digest)` が task_events の payload に digest を載せる。TurnEnded の `result_summary` は digest を優先（従来の「経過秒 · N files」は fallback）＝ review_ready 遷移時に確定値が record へ載る
  - 検証: 全 test green（agent_panel 10 / workspace 21 / storage 11）。offscreen（`SHIRUSHI_HERD=1 SHIRUSHI_ACTIVITY_PROBE=1`）で 3 状態の digest が herd 行に出るのを目視。probe（`debug_set_activities`）にも digest 素材を仕込んで実経路と同じ見え方を再現可能にした
- 学び/罠:
  - digest は**遷移時に書く**のが肝（poll しない・Working だけはライブ合成でコストゼロ）。`statuses()` は描画パスから呼ばれるのでメモリ参照のみを厳守
  - herd の sub 行は幅が狭い＝digest は 1 行 nowrap + overflow_hidden で自然に切れる。全文はセル/カード側で読む設計（P3 の管制キューが本席）
- 次: P2 ニュース常設（task_events ソースの時系列フィード・下ドック・ミュート）

## 2026-07-23 — 管制 P2: ニュース常設（task_events の鏡・ミュート・通知音の非フォーカス限定）
- やったこと（`FLEET-CONTROL-PLAN.md` P2 完了 = ROADMAP M14「ニュースフィード ＋ 通知の細部」）:
  - **ニュース = 台帳の鏡**という一本化: `NotificationCenter.news`（上限 100・新しい順）へ、`transition_task_space` が task_events へ書くのと**同じ場所**で 1 行積む（`news_text_for_phase` 写像: Blocked→「承認待ち — 内容」/ Failed→「失敗 — detail」/ 他→「→ phase — digest」）。起動時は新設 `Storage::load_recent_task_events(60)` で backfill（閉じた Task は中立色チップ＝不明に色を発明しない）。イベント種別語彙 `NewsKind { PhaseChange, Permission, Digest, Integration, Coordinator }` — **監督（P6）の采配も同じログに載る**前提を先に確保
  - **フィード UI**: 編隊下ドックの旧「動き」ストリップ（lane 現在値の横並び）を、mock `fleet-dashboard.html` 下段の時系列フィードへ置換（h118・col-head「ニュース」+ hint・行 = 相対時刻 + 7px 角丸チップ（coordinator は丸）+ 太字名 + イベント文・スクロール・30 行上限表示）
  - **エージェント別ミュート**: `Thread.muted` + `toggle_thread_mute` + herd 行の 🔔（ホバー）/🔕（muted 中常時表示）。muted 中はトースト・完了音を抑止、**ニュースには載る**（見えるが鳴らない）。`PanelEvent::{TurnEnded,TurnFailed,PermissionWaiting}` に muted を載せ workspace 側でトースト分岐
  - **通知音は window 非アクティブ時のみ**: `AgentPanel.window_active` を render で更新（GPUI は activation 変化で再描画するので追従）し、完了音の条件に `!window_active` を追加（見ている画面に音は要らない）
  - 検証: 全 test green（i18n parity 含む）。offscreen（FLEET+ACTIVITY_PROBE・probe に `news_text_for_phase` 経由のデモ行を追加）で「承認待ち/review_ready/failed/working」4 行が時系列で下フィードに出るのを目視
- 学び/罠:
  - ニュースの時刻は mock の絶対 HH:MM でなく**アプリ既存の相対時刻**（`relative_time_label`・30 秒 fleet clock で更新・i18n 済み）を採用。ローカル TZ の HH:MM は std だけでは組めず、フィードの鮮度感には相対の方が合う。mock 側をあとで寄せるか要ユーザー判断
  - GUI 外（CLI/MCP）の遷移は live には映らない（DB を watch しない設計・poll 禁止）— 再起動 backfill では映る。GUI 起動中の CLI 遷移の反映は P5 の IPC（fleet_events）で解消する
- 次: P3 管制タブ（`FleetCenterView { Control, Graph }`・mock/fleet-dashboard.html 本体・要対応キュー + 稼働カード + 統合パイプライン + ⏎）

## 2026-07-23 — 管制 P3: 管制タブ本体（編隊統括ダッシュボード・mock 準拠）
- やったこと（`FLEET-CONTROL-PLAN.md` P3 完了・新規 `crates/workspace/src/workspace/control_view.rs`）:
  - **中央タブ 2 面**: `FleetCenterView { Graph, Control }` + タブ帯（管制/グラフ）。既定 Graph（計画どおり・ドッグフーディング後に再判断）。`ToggleControl`（パレット「表示: 管制」）・`SHIRUSHI_CONTROL=1` で管制起動
  - **6 部構成**（mock/fleet-dashboard.html の書式に一致・全て render 内 memory 読みのみ）: ヘッダ（stat チップ ◐要対応は >0 で err ボーダー・Σトークン+メーター）/ 監督バー（`fleet_mood_mascot` 集約気分 1 匹 = render_mascot をサイズ可変化 `render_mascot_sized`・anim id は tag 名前空間化で panel の 1 匹と共存。総括は**事実文**＝✳ は Tier2 の印なのでまだ出さない）/ 要対応キュー（Blocked 経過順→Failed→Review→Done 未確認）/ 稼働カード / 統合パイプライン 7 列 / ニュース（P2 常設をそのまま下段に）
  - **インライン操作が主役**: 許可/常に許可/拒否 = `AgentPanel::respond_permission(thread, option)`（既存 `answer_permission` を任意スレッド化・同一の一本道）+ 応答直後に台帳を Working へ（turn end が最終確定）。Radar=`review_task_for_merge` / `Integrate`=`integrate_task` / **確認 = `mark_done_seen`（Done→Idle の確認済み遷移・P3 の新設・herdr の done/idle 区別の採用）** / 破棄=Archived。カード本体クリック / ⏎ = 没入（`reveal_agent_in_fleet` + Graph 面へ）
  - **⏎ キー**: keymap に `FleetControl` context（enter → `workspace::ControlNext`）。`ChromeState.control_focus` を管制 root が track_focus し、開いた時/クリックでフォーカス
  - **受入プローブ** `SHIRUSHI_CONTROL_PROBE=1`: 5 擬似 TaskSpace（Working/Blocked 45s/MergeReady/Failed/Planned）を現 root の worktree 共有で合成（SpaceId は probe 専用・storage は渡さない＝**Git/DB に一切書かない**）。panel 側は `debug_set_state`（plan 3/5・実行中ツール・承認カード 3 択・digest）
  - 検証: offscreen で mock 同等の 1 画面を目視（キュー 3 カードのボタン列・稼働カードの「Bash(cargo test…) · テスト修正 ▰▰▱▱▱ 2/5 · 42.1k · 12m」・パイプラインのチップ配置・ニュース連動・herd/レールも 6 project で整合）。全 test green（keymap 4 セクション化・i18n parity 含む）
- 学び/罠:
  - YAML の値に「: 」を含む日本語（`目標: 未設定`）は**必ず引用符**（i18n parity テストが即検出してくれた）
  - `.id()` 後の Div は `Stateful<Div>` — ボタンビルダーの戻り型注釈に注意
  - keymap の既定にセクションを足したら `parses_sections_and_bindings` の位置 assert を必ず追従（4 セクション目 = 全域）
  - GPUI の `with_animation` id はグローバル衝突しうる＝同じ部品を 2 箇所で使うなら id を tag で名前空間化（マスコット panel/管制）
- 次: P4（Tier 2 ✳ 要約 = oneshot 1 行・監督バー総括のデバウンス生成）→ P5 fleet API 拡張（spawn 断絶解消・IPC）

## 2026-07-23 — 管制 P4: Tier 2（✳ 1 行要約）+ 監督バー総括のデバウンス生成
- やったこと（`FLEET-CONTROL-PLAN.md` P4 完了）:
  - **`project::oneshot_line_on` 切り出し**: `name_thread_on` の共通実体（テンプレ埋め・一時ファイル・fence 剥がし・先頭非空行・clamp）を汎用化し、スレッド命名と Tier 2 要約が同じ機構を共有。プロンプトは sh -c 二重引用符埋めなので**引用符/$/バッククォート禁止**の約束をドキュメント化
  - **タスクレベル ✳**（`maybe_tier2_summary`）: turn_finished（Done/Failed）で最後の指示 + 最終応答末尾 1200 字を素材に oneshot → `Thread.tier2` + `PanelEvent::SummaryReady` → workspace が task_events（kind `tier2`）へキャッシュ。管制キューカードの digest 下に ✳ テラコッタ + イタリック行。新ターン送信でクリア・生成完了時に running なら破棄（古い要約を出さない）
  - **編隊レベル ✳**（`schedule_control_summary`）: `transition_task_space` と SummaryReady から蹴り、5s デバウンス（`control_summary_gen` 世代比較）→ `control_summary_facts`（稼働/要対応/完了未確認の数字 + キュー先頭 4 件の事実だけ・**フル transcript は渡さない**）→ oneshot 60 字 → 監督バーの ✳ イタリック文。生成前・失敗時は従来の決定論事実文のまま＝ UI が欠けない
  - **規律の実装**: ✳=LLM 生成の印はここで初めて出す（Tier 1 には付けない）。要約は状態を上書きしない（AttentionKind/stats は常に事実層から別導出）。設定 `tier2_summaries` 既定 on・oneshot 非対応エージェントは自然フォールバック
  - 検証: 全 62 test green・offscreen（CONTROL_PROBE に tier2 素材追加）で監督バー + Failed/MergeReady カードの ✳ 行を目視
- 学び/罠:
  - oneshot の宛先は**既定 Agent の CLI**（Claude 決め打ちしない・auto-name と同じ判断）。編隊総括の実行 cwd は IntegrationSpace root（無ければ先頭 slot）
  - デバウンスは「sleep 後に世代比較 → 最新だけ生成」が最小構成。facts 収集は sleep **後**に main thread で行う（5 秒間の変化を織り込む）
- 次: P5 fleet API 拡張 — GUI ライブ制御 IPC（Unix socket 0600）+ `fleet_spawn_agent / fleet_send / fleet_digest / fleet_events`（spawn 断絶の解消・監督の道具）

## 2026-07-24 — 管制 P5: fleet API 拡張（GUI ライブ制御 IPC・spawn 断絶解消・単一 writer の確立）
- やったこと（`FLEET-CONTROL-PLAN.md` P5 完了・新規 `crates/workspace/src/workspace/control_ipc.rs`）:
  - **IPC**: `~/.shirushi/gui.sock`（0600）・1 接続 1 リクエスト・1 行 JSON。accept は std スレッド（I/O なし・解析のみ）→ futures channel → GUI 主スレッドで memory 解決 → 足りない時だけ background executor + **GUI の storage ハンドル**で読む（UI スレッドで Host/DB を呼ばない規律を維持）
  - **新 CLI/MCP 4 種**（MCP 15 tools に）: `spawn-agent`（record 解決 → `open_folder_in_rail` → `acquire_thread`（空スレッド使い回し）→ prompt 送信 → Task セル追加）/ `send` / `digest`（事実層+Tier1+Tier2 キャッシュのみ・**transcript は返さない**）/ `events`（`load_task_events_since` の差分・GUI 不在は DB 直読み）
  - **【重要発見】Turso は排他ロック** — GUI 稼働中は headless プロセスが DB を開けない（`Locking error`）。従来の「別プロセスが DB を poll」前提が GUI 併走で成立しないことが e2e で発覚 → **単一 writer アーキテクチャに確立**: `is_lock_error` 検出で create/status/wait/list/events を IPC へフォールバック（GUI method: `task/tasks/update_task/record_task/events`）。update_task は開いている slot なら GUI の遷移入口（`transition_task_space`）を通す＝ニュース/総括デバウンスにも自然に載る
  - **受入 e2e（実運転）**: scratch repo + scratch DB + `SHIRUSHI_GUI_SOCK` で GUI 起動 → headless `fleet spawn-agent <id> "Claude Code" "1行で ok と返答"` → **実 claude ターンが GUI 内で実行** → `fleet wait <id> review_ready 90` 成功（result_summary = "ok" = エージェントの返答が digest 経由で確定値に）→ `fleet digest` が GUI メモリの live thread（activity/digest/tokens 24.4k）を返す＝「GUI に thread が現れる」をデータで実証
- 学び/罠:
  - **Unix socket パスは SUN_LEN（macOS ~104B）制限** — 深い scratch パスは bind できない。既定 `~/.shirushi/gui.sock` は安全・テストは `/tmp` の短パスで
  - window 構築クロージャ内の即時 `handle.update` は届かない — probe 群と同じく **spawn + 短 delay で defer** が正解
  - `--features screenshot` 無しビルドでは SHIRUSHI_SCREENSHOT が無効（e2e で PNG が出ず数分悩んだ）。ビジュアル証拠が要る時は feature を忘れない
  - Turso の WAL（`e2e.db-wal` 350KB）はプロセス kill でも残って正常（次回 open で追いつく）
- 次: P6 監督席（任命制 pinned thread + fleet ツールセット + イベント駆動 wake + `depends_on`）＝ M14 総合受入「B の完了を待って merge」の自走

## 2026-07-24 — 管制 P6: 監督席 + 依存待ち（「B の完了を待って merge」の自走を実運転で実証）
- やったこと（`FLEET-CONTROL-PLAN.md` P6 完了・新規 `crates/workspace/src/workspace/coordinator.rs`）:
  - **depends_on**: `task_deps` 別テーブル（列追加せず既存 DB 無 migration）+ record + `set_task_depends`。CLI `fleet depend` / `wait-deps <id> <phase>`（全依存が揃うまで）/ `fleet wait` は phase と **activity（GUI live）** の両対応。IPC `set_depends`（単一 writer 経由）
  - **監督席**: settings `coordinator_agent`（None=未任命・**プロジェクト設定 `.shirushi/settings.json` でも任命可**＝scratch 検証で発見した綺麗な道）。IntegrationSpace の panel に pinned thread「監督」（`ensure_named_thread` = 名前で再利用・`name_is_custom` で自動命名から保護）。wake は Done/Failed 即時・Blocked 15s 閾値（`wake_coordinator_for_blocked` が 15 秒後に「まだ Blocked か」を確認）・`thread_busy` なら重ねない（次のイベントで最新状態ごと読む）。管制の監督バーは任命済みエージェント名を表示
  - **プロンプトテンプレート**: 役割・規律（自分でコードを書かない・**integrate の最終承認は人間**）・fleet CLI 全道具（`std::env::current_exe()` の実パス埋め込み＝dev の target/debug でもそのまま動く）・「静観」も明示的な選択肢に
  - **采配の監査**: 監督ターン完了（TurnEnded・integration slot・thread 名一致）→ `record_coordinator_decision` = ニュース（NewsKind::Coordinator・丸チップ）+ task_events（task_id="coordinator", kind="coordinator"）
  - **受入 e2e（実運転・自動化済み）**: scratch repo の `.shirushi/settings.json` に coordinator_agent を書いて GUI 起動 → `fleet spawn-agent` で task A に実 claude ターン → A が review_ready → **監督が wake され、実際に fleet CLI で現況を調べ、A と B の両方を review（radar clean）→ merge_ready へ進め、「あとは統合を人間が承認する段階」と采配を台帳に残して停止**。task list で両タスク merge_ready + integration 無傷を確認＝ M14 総合受入の自走部分を実挙動で達成
- 学び/罠:
  - `cargo check` 通過ではバイナリは古いまま — CLI サブコマンド追加後の実地テストは `cargo build` を忘れない（旧 usage が出て数分迷った）
  - 監督の自己 wake ループは「integration slot のイベントでは wake しない」の 1 条件で消える（監督は integration に住むので、自分のターン完了は Task 遷移を起こさない）
  - プロジェクト設定のマージ（`.shirushi/settings.json`）は検証にもプロダクトにも効く: リポジトリごとに監督を変える運用が自然に可能
- 次: P7 Herdr sidecar（AgentRuntime trait・常駐ランタイム・再起動を跨ぐ AgentRun）— 常駐の最重要価値。P8 observer / P9 リモート管制が残り

## 2026-07-24 — 管制実装ループの区切り（P0〜P6 完了・P7 は herdr 導入待ち）
- やったこと: P3 残件の**編隊 goal** を回収 — settings `fleet_goal`（プロジェクト設定 `.shirushi/settings.json` でリポジトリごと・**ファイルが真実**の原則に寄せ、計画の「ledger に持つ」は settings で満たすと判断）→ 管制ヘッダに「目標: …」表示（未設定時は設定方法つきプレースホルダ）
- **ループ全体の到達点**（このセッション連鎖・全 P0〜P6）:
  - P0 縫い目一本化 / P1 digest Tier1 / P2 ニュース常設 / P3 管制タブ / P4 ✳ Tier2 / P5 IPC + 単一 writer / P6 監督席 + 依存待ち — 各フェーズ受入実証つき（offscreen 目視 or 実運転 e2e）
  - M14 総合受入の自走部分（「B の完了を待って merge」）を**実 claude の監督で実挙動達成**
- **P7（Herdr sidecar）が着手ブロック**: `herdr` バイナリが未導入。計画 §P7 は「Apache 2.0 の正式リリースを version 固定・checksum 固定」＝**導入方法とバージョンはユーザー判断**（zed/ の pull と同じ扱い）。API も実物の `herdr api schema --json` から取る設計のため、目隠し実装はしない。P8（observer）も Herdr 前提・P9 は「M14 完了後・設計から」
- 次（ユーザーの手番）: ① herdr の導入（バージョン選定・`--session shirushi` 運用の確認）→ P7 着手 ② 日常ドッグフーディング（管制タブ・監督任命・ニュース/ミュート）→ M14 総合受入のチェック ③ 未コミットの作業ツリー（P0〜P6 全部 + 前セッション分）のコミット方針

## 2026-07-24 — UX 是正（ユーザーレビュー C 対応・4 点）
- 経緯: 7/22〜の未コミット設計（TaskSpace-first）+ 管制ループ実装をユーザーがレビューし、4 点の UX 逸脱を指摘。方針 C（コードは残して UX を直す）で対応
- **① solo の横タブ消失** — 原因はコードでなく **ユーザー settings.json に `agent_tabs_view: "list"` が保存されていた**（7/20 の検証時ドリフト）。`"bar"` へ戻して復旧。教訓: 検証でスイッチャを触ると保存される＝「自分で決めた既定はドリフトしない」原則は検証手順にも適用すべき
- **② レールに Task を載せない** — レール = プロジェクト（リポジトリ）単位に戻した。**同じリポジトリが色違いで並ぶのは「色による方向感覚」の自己破壊**（ユーザー指摘・正しい）。`render_rail` が Task slot を除外し、アクティブが Task の時は同リポジトリの Integration 枠を点灯。Task worktree は編隊モード（herd/セル）の中だけに住む。7/22 の「Task = worktree = レール 1 枠」設計判断は**取り消し**
- **③ ＋ を 1 択に** — 編隊の＋タイルを「＋ エージェント（新しい worktree で並走）」の単一ボタンへ（7 ボタン → 1）。Terminal/Editor/Diff/Tests セルと履歴復元は機能として保持（dead_code 許可・入口は後日控えめに再接続）
- **④ 「IntegrationSpace は保護されています」トースト撤廃** — main もセルに出せるように。守るべきは Git 側（radar + 人間 gate・台帳の遷移拒否）であって**画面の取り締まりではない**
- 検証: 警告 0・test green・offscreen で solo（横タブ復活）と編隊（レール 1 枠・管制正常）を目視
- 次: 動作確認いただき OK なら退避 or 本コミットで確定（未コミットの全量が消えない状態を早く作る）

## 2026-07-25 — Working 表現 = 点字スピナーで確定（ユーザー選定）
- mock/working-anim-patterns.html の 10+1 案比較から **① 点字スピナー（herdr 型）** をユーザーが選定。実装は前日に `working_spinner`（activity_dot の Working アーム差し替え・フィルムストリップ手法・スレッド色維持）で導入済みのため変更なし。UI-SPEC §6/§11 の記述を実装に同期（満・脈動 → 点字スピナー）。カタログは今後の再検討用に残す（差し替えは working_spinner 1 箇所）

## 2026-08-08 — ACP ハンドシェイクの無言ハング可視化 + カタログ pin + ファイルアイコン
- やったこと（ユーザー依頼の 3 件）:
  - **① ACP initialize のタイムアウト（無言ハング → エラー化）**: `acp_client` の initialize 3 サイト（connect_and_initialize / prompt_once / 常駐 run_session_on）は無制限に応答待ちしていた＝エージェントが生きたまま黙ると無言でスピナーが回り続けた。`with_timeout`（タイマは既存 `blocking` プールで寝るスレッド＝新規依存なし・ランタイム非依存）で `HANDSHAKE_TIMEOUT`=30s を噛ませ、時間切れは `acp::Error::new(InternalError, "… が 30 秒応答しません（無言ハング）")`（理由を message に直接）に。agent_panel は `AgentEvent::Failed(format!("{error:#}"))` で anyhow 原因鎖まで出す（`to_string()` だと上位 context だけでハング理由が埋もれる）。検証: ユニット2本（速い応答は素通り / pending は timeout でエラー・0.03s）＋ **実プロセス統合 `connect_times_out_on_silent_hang`（#[ignore]・`sleep` に実パイプ越しで当て 30.00s でエラー復帰を実挙動確認）**
  - **② カタログ pin 更新**: `claude-agent-acp 0.58.1→0.66.0` / `codex-acp 1.1.2→1.1.14`（npm 最新を実確認）。**ただし現環境では未反映の可能性大**（下記・罠）
  - **③ ファイルアイコンをわかりやすく**: `file_icon`（ツリー）/`icon_large`（グリッド）が無地の角丸長方形だったのを、mock の icon-grid 準拠のシルエットへ。新規 `folder.svg`（フォルダ形・landscape viewBox）/`file.svg`（折れ角のある書類形・portrait viewBox）を main.rs の AssetSource に登録。ツリー=シルエット＋色（幅が無いのでラベル無し）、グリッド=書類（中立 fg2 の紙面）＋**型ラベル**（RS/TOML/MD…）。ラベル色は型色、ただし muted 型（md/txt＝色が fg2＝紙面と同色で埋もれる）は濃いインク bg0 に落として可読に。UI-SPEC §4 ツリー行にアイコンを明記（§4 アイコン節は元から SVG40×34+拡張子タグ＝実装のドリフトを spec 側へ戻した形）。offscreen（`SHIRUSHI_EXPLORER_VIEW=tree/icons` + `--features screenshot`）で両ビュー目視
  - 全 test green（acp_client / agent_panel 10 / workspace 21）・警告 0
- 学び/罠:
  - **pin 更新は現環境では効かない**: `zed_cached_agent` は Zed の npx キャッシュを **bin 名だけで拾いバージョンを見ない**。実機は claude-agent-acp が 0.58.1 と 0.59.0 の2つ + codex 1.1.2 がキャッシュ済み（＝どれが動くかは read_dir 順で非決定的）。pin は npx フォールバック（キャッシュ無しの新規/CI）だけに効く。「pin を効かせる」には (A) 古いキャッシュ削除 or (B) `zed_cached_agent` をバージョン一致優先化（pin 版が無ければ npx で取得・npx も無い時だけ既存キャッシュにフォールバック）。**ユーザー判断待ち**（resolution 挙動変更のため独断でやらない）
  - `connect_with(transport, main_fn)` の main_fn は `Result<R, acp::Error>` 固定＝timeout エラーも `acp::Error` で返す必要（`acp::util::internal_error` は message=「Internal error」+ data に文字列＝Display が `Internal error: "…"` と quote 付き。`acp::Error::new(code, msg)` で message 直書きすると綺麗）
  - offscreen スクショはグリフも SVG マスクも写る（font-kit 経由）＝アイコン検証に有効。新規 SVG は main.rs の match 登録を忘れると無音で出ない（既知の罠どおり）
- 次: ② の (A)/(B) をユーザーが選定 → (B) なら version 一致 resolution を実装。必要ならコミット方針（今セッションの作業ツリーは未コミット）
