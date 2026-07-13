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
