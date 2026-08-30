# WINDOWS-PORT.md — Windows 対応（W フェーズ）

necoder の **Windows 版**を作るための一次資料。`/goal-win` はこの文書を上から消化する。
調査日 **2026-08-22**（対象 rev: gpui `b2d9c2e1`）。

**この文書の立場**:
- ROADMAP の Windows 部分の**詳細版**。ROADMAP には要約だけを置き、受入条件の正はここ
- 設計・UI・用語の正は従来どおり `ARCHITECTURE.md` / `UI-SPEC.md` / `GLOSSARY.md`。ここはその**プラットフォーム差分**だけを扱う
- **鉄則: mac の挙動を 1 ミリも変えない**。W フェーズの全変更は「mac では従来と同一のパス・同一の挙動」を unit test で固定してから入れる（既存ユーザーの設定・DB・スレッド履歴が引っ越すのは事故）

---

## 0. 現在地（2026-08-22 の調査結果）

### GPUI は Windows で動く

固定 rev に **`gpui_windows` crate が実在**する（`gpui_platform/Cargo.toml` が `cfg(target_os = "windows")` で正式に引いている）:

```
directx_renderer.rs   68KB   DirectX レンダラ
direct_write.rs       68KB   DirectWrite（フォント整形・グリフラスタライズ）
window.rs / events.rs 58/64KB  ウィンドウ・入力・IME・DirectManipulation
directx_atlas.rs / vsync.rs / clipboard.rs / system_notifications.rs …
```

Zed 自身が Windows 版を配布している＝実証済み。「GPUI が動かないかも」は**誤り**で、正しくは
「**3 プラットフォームの中で一番荒い**」（`BACKGROUND.md` の記述どおり）。

### 実測結果（2026-08-22・Windows 実機・W0 完了）

`cargo check --workspace --all-targets --keep-going` を実機で回した結果。**推測ではなく計測値**:

| 測ったこと | 結果 |
|---|---|
| necoder 側 20 crate | **19 が green**。落ちるのは `workspace` の **1 エラーだけ** |
| その 1 エラー | `crates/workspace/src/workspace/control_ipc.rs:22`（E0433 `os::unix`） |
| `necoder`（bin） | **未到達＝まだ測れていない**（`workspace` に依存するため。`fleet.rs:86` は残っているはず） |
| `gpui_windows` | **コンパイル成功**（DirectX + DirectWrite が実際に通る） |
| `turso` | **成功** = §7 の `rusqlite` 退避は**不要**だった |
| `alacritty_terminal` | 成功（ConPTY バックエンド） |
| tree-sitter grammar | **17 本すべて成功**（cc → MSVC 検出が効いている） |
| `font-kit` | **Windows ではビルドされない**（gpui が DirectWrite を使う＝mac の font-kit 相当は不要） |
| `host` crate | **green** = 既存の `#[cfg(unix)]` ゲートが効いて Remote SSH が素直に除外されている（§D6 の前提が裏付いた） |

**W2 の「警告 0」に向けた現存警告 5 件**（Windows のみ・unix 経路が cfg で消えた副作用）:
`shell_env.rs:27,28,29`（未使用 import）/ `updater.rs:177`（不要な `mut`）/ `host.rs:2438`（`connect_io` が never used）

> **測定の作法（重要）**: `--keep-going` が無いと **cargo は最初に落ちた crate で停止**する。
> さらに `--keep-going` を付けても**壊れた crate の下流は測れない**（上流のメタデータが要るため）。
> ＝**W0 のインベントリは本質的に反復**（1 個直すと次の層が見える）。
> 受入条件の「エラー全量」は「**その時点で見える全量**」と読むこと。

### 詰まるのは necoder 側の unix 前提

ビルドが落ちる箇所・動かない箇所は §5 のインベントリに全量。要約すると:

1. **無条件の Unix domain socket 使用 2 ファイル** → そもそもコンパイルできない
2. **`~/Library/Application Support/necoder/` のハードコード 9 ファイル・`HOME` 直読み 19 箇所**
3. **`sh -c` 前提 10 箇所**（Windows に `sh` は無い）
4. mac 専用の外部コマンド 8 種（`open` / `osascript` / `trash` / `afplay` / `sw_vers` / `hdiutil` / `ditto` / `spctl`）
5. keymap 既定が全部 `cmd-`、メニューが `cx.set_menus`（mac ネイティブ）前提
6. 検証ループの `scripts/*.sh` が mac 専用

### 追い風（すでに移植可能な状態にあるもの）

- ~~**ヘッドレススクショが OS 非依存**: `--features screenshot` + `NECODER_SCREENSHOT=<path>` は gpui の
  `render_to_image` を使う＝**Windows でもそのまま検証ループが回る見込み**（`screencapture` 不要）~~
  → **2026-08-22 実機で誤りと判明**。`render_to_image not implemented for this platform` で落ちる
  ＝ gpui の `render_to_image` は **macOS にしか実装が無い**。
  代わりに **Win32 の `PrintWindow`（`PW_RENDERFULLCONTENT`）で実ウィンドウを捕捉**する方式にした
  （`scripts/screenshot-app.ps1`）。**結果的に mac 版より良い**: 画面全体を撮る `screencapture` と違い
  **necoder の窓だけ**が撮れるので他のウィンドウが写り込まず、座標にも依存しない。
  ただし**実ウィンドウが要る**＝ヘッドレス CI では回せない（mac 版と同じ制約に戻った）
- **検索は in-process**: ripgrep バイナリではなく `ignore` crate → 外部依存なし
- **Git は CLI + imara-diff**: `git.exe` が PATH にあれば動く
- **ターミナルは alacritty_terminal**: ConPTY バックエンドを持つ（W4 で最初に実証する）
- **remote server（`host` crate）は GPUI 非依存**で、すでに Linux 実機実証済み

---

## 1. 開発環境（なぜ WSL では駄目なのか）

### WSL でビルドすると「Linux 版」ができる

| | WSL でビルド | 必要なもの |
|---|---|---|
| 成果物 | ELF（Linux 実行ファイル） | **PE（`necoder.exe`）** |
| リンカ | GNU ld | **MSVC `link.exe`**（Windows SDK 同梱・Windows 側にしか無い） |
| 描画 | `gpui_linux`（Wayland/X11） | **`gpui_windows`（DirectX + DirectWrite）** |
| GUI | WSLg には出る（が Linux 版の絵） | Windows ネイティブウィンドウ |

WSL からの `--target x86_64-pc-windows-msvc` クロスビルドは、理屈の上では `xwin` 等で
Windows SDK を落とせば可能だが、**HLSL シェーダのビルドと font-kit が絡んで茨**。
**採らない**（時間を溶かす割に得るものが無い）。

### リポジトリの置き場（再 clone は不要）

WSL から見えている `/mnt/c/...` は **Windows のディスクそのもの**。同じ作業ツリーを
PowerShell から `C:\...` として開けば、そのまま Windows ネイティブビルドができる。
**別の場所に clone し直す必要は無い**（むしろ 2 本持つと「どっちを直したか」で事故る）。

遅いのは「**WSL 側から `/mnt/c` を触るとき**」だけ（DrvFs 越しで数倍〜10 倍）。
Windows 側から `C:\` を触る分はネイティブ速度。

**ただし `target/` は必ず分ける**。WSL ビルド（Linux）と Windows ビルド（msvc）は
どちらも既定で `target/debug/` を使うため、**同じディレクトリを奪い合って毎回フルリビルドになる**。

```powershell
# Windows 側（PowerShell）: プロファイルか .env で固定しておく
$env:CARGO_TARGET_DIR = "$PWD\target-win"
```

再 clone した方がよいのは次の場合だけ:
- **リポジトリのパスが深い / 空白や日本語を含む**（MAX_PATH と quoting の事故が増える）→ `C:\dev\necoder` へ
- WSL 側でも高速にビルドしたい（Linux 版の作業）→ WSL の ext4（`~/dev/necoder`）に**別途** clone する

### 結論: 役割分担

| やること | どこで |
|---|---|
| コード編集・エージェント実行・grep・文書更新 | WSL でも Windows でもよい |
| **`cargo check` / `test` / `run` / スクショ検証** | **Windows のネイティブ PowerShell（必須）** |
| mac 版の非退行確認 | 従来どおり Mac |

### Windows 側セットアップ（一度だけ）

```powershell
# 1. Visual Studio Build Tools（C++ ビルドツール + Windows SDK）
#    tree-sitter の grammar が cc でビルドされる & gpui のリンクに必要
winget install --id Microsoft.VisualStudio.2022.BuildTools --override `
  "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"

# 2. Rust（msvc ツールチェーン）— 版は rust-toolchain.toml が固定するので自動で合う
winget install --id Rustlang.Rustup

# 3. Git（git.exe が PATH に居ること = git 機能の前提）
winget install --id Git.Git

# 4. リポジトリは既にある作業ツリーをそのまま使う（再 clone 不要・前節参照）
#    例: cd C:\Users\<user>\Desktop\dev\necoder
#    target/ の奪い合いを避けるため CARGO_TARGET_DIR を分けること

# 5. 長パス有効化（管理者 PowerShell・cargo の深い依存ツリーで効く）
git config --system core.longpaths true
Set-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" LongPathsEnabled 1
```

初回ビルドは GPUI の依存で**数分〜十数分**かかる（mac と同じ。壊れたと早合点しない）。

---

## 2. 設計決定（着手前に固定する）

### D1. `paths` crate を新設する（**最優先・W1**）

foundation 層に**依存ゼロの `paths` crate** を置き、パスを決める処理を全部そこへ集約する。
`ARCHITECTURE.md §1` の層構造では `i18n` / `theme_core` と同じ最下層。

```rust
// crates/paths/src/paths.rs — 依存: std のみ
pub fn home_dir() -> Option<PathBuf>;        // HOME / USERPROFILE
pub fn config_dir() -> Option<PathBuf>;      // 人が編集するもの
pub fn data_dir() -> Option<PathBuf>;        // 機械が読み書きするもの
pub fn state_dir() -> Option<PathBuf>;       // ログ・クラッシュ・セッション状態
pub fn runtime_socket() -> Option<PathBuf>;  // 制御 IPC の口

// 用途別（呼び出し側はこれだけ使う）
pub fn settings_file() / keymap_file() / db_file() / blobs_dir()
     / state_file() / logs_dir() / crashes_dir() / shell_path_cache() -> Option<PathBuf>;
```

| 用途 | macOS（**変更禁止**） | Windows | Linux |
|---|---|---|---|
| settings.json / keymap.json / テーマ | `~/Library/Application Support/necoder` | `%APPDATA%\necoder`（Roaming） | `$XDG_CONFIG_HOME/necoder`（既定 `~/.config/necoder`） |
| necoder.db / blobs | 同上 | `%LOCALAPPDATA%\necoder` | `$XDG_DATA_HOME/necoder` |
| state.json / logs / crashes | 同上 | `%LOCALAPPDATA%\necoder` | `$XDG_STATE_HOME/necoder` |
| shell-path キャッシュ | 同上 | **不要**（Windows に login shell PATH の概念が無い） | `$XDG_CACHE_HOME/necoder` |
| 制御 IPC | `~/.necoder/gui.sock` | `\\.\pipe\necoder-gui-<user>` | `$XDG_RUNTIME_DIR/necoder/gui.sock` |

**なぜ Windows で Roaming と Local を分けるか**: Roaming プロファイルはドメイン環境で
サーバへ同期される。**数百 MB になりうる DB と blob を Roaming に置くとログオンが死ぬ**（実害）。
「人が編集する設定 = Roaming / 機械が書くもの = Local」が Windows の作法。

**テスト差し替え口**: `NECODER_HOME` を 1 つ用意し、全部これで根から差し替えられるようにする
（既存の `NECODER_LOG_DIR` / `NECODER_SHELL_PATH_CACHE` / `NECODER_GUI_SOCK` / `NECODER_CRASH_DIR` は互換のため残す）。

### D2. IPC は「Unix socket / 名前付きパイプ」を抽象する

`control_ipc.rs`（GUI 側の listener）と `fleet.rs`（CLI 側の client）が `std::os::unix::net` を
**cfg 無しで**使っている＝ここが Windows ビルドの最初の壁。

- 方針: `workspace` に `control_transport` を切り、`ControlListener` / `ControlStream`（`Read + Write`）に抽象化
- unix: 現状どおり `UnixListener`（0600 パーミッション維持）
- windows: **名前付きパイプ**（`\\.\pipe\necoder-gui-<user>`）。TCP loopback は**採らない**
  （他ユーザー・他プロセスから叩けてしまう＝`control_ipc.rs` 冒頭が守っている「守るべき操作の単一経路」が崩れる）
- 実装は依存を増やさず Win32 API 直叩き（`windows-sys`）か、permissive な既存 crate を検討。
  **どちらにせよ `ARCHITECTURE` の依存方向は変えない**

### D3. 外部コマンドは「意図」で包む

`sh -c "..."` が 10 箇所、mac 専用コマンドが 8 種。**呼び出し側を書き換えるのではなく、
意図の名前で関数を切って中で分岐する**（`#[cfg]` を業務ロジックに散らさない）。

| 意図 | mac | Windows |
|---|---|---|
| シェルで 1 行実行 | `sh -c <script>` | `cmd.exe /C` もしくは `powershell -NoProfile -Command` |
| ファイルを OS で開く | `/usr/bin/open` | `explorer.exe` / `start` |
| Finder/エクスプローラで表示 | `open -R` | `explorer.exe /select,<path>` |
| ゴミ箱へ | `/usr/bin/trash` | Recycle Bin（`SHFileOperation` or permissive crate） |
| 完了音 | `afplay Glass.aiff` | ROADMAP の「独自チャイム同梱 + `rodio`」を前倒しするのが筋 |
| OS バージョン | `sw_vers` | `cmd /c ver` / WinAPI |

**注意**: `sh -c` を使っている 10 箇所は「複数コマンドのパイプ」を組み立てている可能性が高い。
移植時は**シェル構文に頼らない形へ直す**方が安全（`cmd.exe` の quoting は地雷）。

**最重要の注意 — 分岐キーは `cfg!` ではなく「実行先 host の OS」**:
`sh -c` 10 箇所のうち **9 箇所は `host::CommandSpec::new("sh", …)` → `run_command` 経由**
（`project.rs:907,999,1038,1104` / `todos.rs:211` / `acp_client.rs:621` / `host.rs:3235` /
`remote_ssh_live.rs` ×3）。**この経路はローカルにもリモート（Linux）にも同じ呼び出しで飛ぶ。**

上の表を `cfg!(target_os = "windows")` で実装すると、**Windows クライアントからリモート Linux ホストへ
`cmd.exe /C` を送る**ことになる（当然動かない）。分岐は「**コンパイル先の OS**」ではなく
「**そのコマンドを実行する host の OS**」に属する情報で行うこと＝判定材料は `CommandSpec` の実行先が持つ。

§D6 により Windows 初回リリースでは Remote SSH を cfg 除外する＝**Windows では host が必ずローカル**なので、
W2 の時点で実害は出ない。**だがその前提に暗黙に乗る形になる**ため、ラッパを切る時点で
「ローカル前提」を型か関数名で明示しておくこと。Remote SSH が Windows に戻る時にここが効く。

### D4. keymap は既定をプラットフォーム別にする

`keymap_core` の既定が全て `cmd-`、表記も `⌘` 固定（`pretty_keystroke`）。

- 既定 keymap を `default_macos` / `default_windows`（Linux は Windows 側を共有）に分ける
- Windows 既定は **VSCode 準拠**（`ctrl-` 系・`ctrl-shift-p` 等）
- 記号化はプラットフォーム別（`⌘S` ↔ `Ctrl+S`）。パレット・メニューのキー併記もここを経由するので 1 箇所直せば波及する
- ユーザー keymap の JSON 形式は共通のまま（`cmd-` は Windows では `ctrl-` に読み替えない＝**明示が正**）

### D5-b. キャプションボタン（最小化・最大化・閉じる）は自前で描く

**2026-08-24 に実機を触ったユーザーの指摘で判明した穴**。当初の計画には無かった。

macOS は GPUI がネイティブの信号機を左上に描くが、**Windows でカスタム titlebar を使う以上、
最小化・最大化・閉じるは自前で描かないと存在しない**＝ **窓を閉じる手段が無い**。

| | macOS | Windows / Linux |
|---|---|---|
| 位置 | 左上（OS が描く） | **右上**（自前で描く） |
| 並び | 閉じる・最小化・最大化 | 最小化・最大化・閉じる |
| 寸法 | OS 任せ | **46x32**（Windows 標準） |
| 左の余白 | `TRAFFIC_LIGHT_INSET`（92px） | **詰める**（信号機が無い） |

実装は `chrome.rs` の `render_window_controls()`。**mac では何も描かない**（§D8）。

**クリックは自前で処理する**（`window.minimize_window()` / `zoom_window()` / `remove_window()`）。
gpui の `window_control_area`（`WM_NCHITTEST` で `HTCLOSE` 等を返す仕組み）は
**この構成では発火しなかった** — ボタンは描かれるのに押しても何も起きない、という状態になる。
引き換えに **Windows 11 のスナップレイアウト**（最大化ボタンのホバーで出る配置メニュー）は出ない。
後日 hit-test 経路が動く形が分かったら戻す価値がある。

**`zoom_window()` は「元に戻す」ができない**。gpui の Windows 実装は `ShowWindowAsync(SW_MAXIMIZE)` を
投げるだけで復元の口が無い（`gpui_windows` の `zoom()`）。最大化済みなら `SW_RESTORE` を自前で
呼ぶ（`restore_maximized_window()`・`workspace` は名前付きパイプで既に `windows-sys` に依存している）。

**色は中立**（`fg2` → hover で `fg0` + `bg2`）。UI-SPEC §1.3 の許可リストに titlebar の
キャプションボタンは無いため。**Windows 標準の「閉じるだけ hover が赤」は色の掟と衝突する**ので
採っていない — 採るなら UI-SPEC 側に例外を明記する必要がある（**ユーザー判断**）。

### D5. メニューは Windows ではアプリ内に持つ

`crates/necoder/src/menus.rs` は `cx.set_menus`（mac のネイティブメニューバー）。
Windows にはグローバルメニューバーが無い。

- W3 では**最小**（メニュー無しでも全機能はパレット `⌘⇧P`/`Ctrl+Shift+P` から到達可能な設計になっている）
- 本実装は「タイトルバー内のアプリ内メニュー」= VSCode 方式。**UI-SPEC への追記が必要**（色の許可リストに触れないこと）
- 既存の 8 メニュー・72 個の i18n キー（`menu.*`）はそのまま流用できる＝**データは既にある**

### D6. Remote SSH（`host` crate）は当面 Windows 非対応とする

`id -u` / `mkfifo` / Unix socket / `sh -l` 前提が深く、ControlMaster にも依存している。
**Windows 版の初回リリースでは cfg で機能ごと落とし、UI で「Windows では未対応」と明示**する。
（`OpenSSH for Windows` には ControlMaster が無いため、素直な移植ができない）

### D7. 配布は「zip → Authenticode」の順

- W6 初手: **署名なし zip**（`necoder.exe` + 必要 DLL）。SmartScreen の警告は README に明記
- 次: **Authenticode 署名**（EV 証明書は年額が高い。OV でも SmartScreen の評判は時間で育つ）
- MSIX / winget は**その後の判断**。updater（現状 .dmg + `spctl` 前提）は Windows 分岐が要る
- **やらない**: Microsoft Store（AGPL との整合と審査コストを公開初期に背負わない）

### D8. mac 互換の鉄則（W フェーズ全体に適用）

- mac のパス文字列は**1 文字も変えない**。`paths` crate の mac 分岐は既存文字列をそのまま返し、それを unit test で固定する
- `#[cfg(target_os)]` を **view 層・業務ロジックに散らさない**。分岐は foundation（`paths` / transport / command）に閉じる
- 新しい UI 文字列は当然 `t!` 経由で ja/en 両方（i18n parity テストが落ちる）

---

## 3. フェーズと受入条件（`/goal-win` はここを上から消化）

### W0 — 現在地の確定（Windows 実機）

- [x] **エラー全量の取得**: Windows 実機で **`cargo check --workspace --all-targets --keep-going`** を回し、**コンパイルエラーを全件** `docs/JOURNAL.md` に記録する。**2026-08-22 実施済み**（JOURNAL 同日エントリ・結果は §0 の表）
  - **`--keep-going` は必須**。無いと cargo は**最初に落ちた crate で停止**し、その先が未検査のまま終わる（2026-08-22 実測: `workspace` の 1 エラーで止まり、`necoder` 本体＝`fleet.rs` まで到達しなかった）
  - 実機を触らずに当たりを付けるなら `.github/workflows/release.yml` の windows ジョブを `workflow_dispatch` で回す（**`cargo build --release -p necoder` だけでは `necoder` とその依存しか見ない**ので、`--keep-going` 付きの check ステップを先頭に置いてある＝2026-08-22 対応済み）
- [x] **依存の Windows 適性を確認**: `turso`（DB）・`alacritty_terminal`（ConPTY）・`tree-sitter` grammar（cc/MSVC）・`gpui_platform` の `font-kit` / `test-support` feature が Windows で解決するか。**落ちるものがあればここで代替を決める**（`turso` が駄目なら `rusqlite` へ退避できる面は ARCHITECTURE §7 が保証済み）
  - **2026-08-22 実測: 全部通った。代替は不要**（§0 の表）。`font-kit` は Windows では**そもそもビルドされない**（DirectWrite 経路）
- [x] 受入: 「あと何をすれば通るか」がファイル単位のリストになっている
  - **残り 1 ファイル（その時点で見える全量）**: `crates/workspace/src/workspace/control_ipc.rs:22`
  - **その先で出るはず**: `crates/necoder/src/fleet.rs:86`（`workspace` が通るまで測れない）
  - **警告 5 件**: `shell_env.rs:27,28,29` / `updater.rs:177` / `host.rs:2438`

### W1 — `paths` crate（**mac 挙動不変**）— **2026-08-22 完了**

- [x] `crates/paths` 新設（依存ゼロ）。§D1 の API と 3 プラットフォームの対応表を実装
- [x] **mac の戻り値が現行と完全一致**することを unit test で固定（9 ファイル分の全パス）
- [x] `HOME` 直読み **19 箇所**と `Library/Application Support` ハードコード **9 ファイル**を全て `paths` 経由へ置換
- [x] `NECODER_HOME` で全体を差し替えられる（テストが実ユーザーのデータを触らない）
- [x] 受入: `grep -rn 'Library/Application Support' crates --include=*.rs` が**コメント以外 0 件**（残るのは Zed 自身の npx キャッシュ探索＝別アプリの置き場）、`grep -rn 'var_os("HOME")' crates` が `paths` crate のみ

> **設計を 1 点、文書より踏み込んだ**: D8 の「mac の戻り値を unit test で固定」を
> `#[cfg(target_os = "macos")]` なテストで書くと **mac 上でしか検証されない**＝
> 「mac を壊していないこと」を Windows/Linux の CI が保証できない。そこで分岐の核を
> **`Platform` と環境変数取得関数を引数で受け取る内部関数**（`*_on` 系）にしてある。
> ＝ **Windows 上でも mac のパス生成をテストできる**（23 テストが全プラットフォームで走る）。
>
> **その設計が即座に自分を救った**: 最初の実装は XDG の絶対パス判定に `Path::is_absolute()` を
> 使っていたが、あれは**実行中 OS の規則**で判定するため Windows 上では `/custom/config` が
> 「相対パス」になりテストが落ちた。`is_posix_absolute()`（先頭が `/` か）を自前で持つよう修正。
> **この crate が防ごうとしているバグそのもの**を、この crate のテストが捕まえた。

**追加で入れたもの（W1 の想定外・だが実害があった）**: `paths::canonicalize`。
Windows の `std::fs::canonicalize` は **verbatim 形式**（`\\?\C:\Users\…`）を返すが、
`git rev-parse --show-toplevel` は通常形（`C:/Users/…`）を返す。この 2 つは `Path` として
**等しくない**（`VerbatimDisk` と `Disk`）＝ **同じファイルが別物扱いになり、git status の色が
付かない・タブが二重に開く**。verbatim を剥がして揃える。区切り文字（`/` と `\`）の違いは
`Path` のコンポーネント比較で吸収されるので問題にならない。**17 箇所の `std::fs::canonicalize`
を全部これへ置換済み**。

### W2 — Windows でコンパイルを通す — **2026-08-22 完了**

- [x] `control_ipc.rs` / `fleet.rs` の transport 抽象化（§D2・名前付きパイプ）
  - `crates/workspace/src/workspace/control_transport.rs` 新設。`ControlListener` / `ControlStream`
  - Windows は `windows-sys` の Win32 直叩き（`CreateNamedPipeW` / `ConnectNamedPipe` / `PeekNamedPipe`）。
    定数は版で置き場が動くのでローカル定義し、**関数だけ**引く
  - **`FILE_FLAG_FIRST_PIPE_INSTANCE` で二重 bind を検出**＝unix の「socket ファイルが残っているだけか
    生きているか」判定より確実。`PIPE_REJECT_REMOTE_CLIENTS` でネットワーク越しを拒否
  - 読み取りタイムアウトは `PeekNamedPipe` のポーリングで実装（overlapped I/O を持ち込まない）
  - **往復・二重 bind 拒否・listener 不在・読み取りタイムアウトの 4 テストが Windows 実機で green**
    ＝ FFI が机上でなく実際に動くことを機械で確認済み
- [x] `sh -c` の意図別ラッパ化（§D3）
  - `Host::shell_script()` / `Host::has_posix_shell()` を新設。**分岐は Host が持つ**（`cfg!` ではない）
  - `LocalHost` だけが `cfg!(windows)` を見る＝ここが唯一 `cfg!` を見てよい場所
  - `acp_client.rs:621` は `if !host.is_remote()` の内側＝**必ずリモート**なので `sh` のまま。
    コメントで明示（次の人が `cfg!` で「直さない」ように）
- [x] `logging.rs` の Windows 実装 — `log_dir()` が全プラットフォームで実パスを返すようになった
      （以前は `cfg(not(unix))` で `None`＝置き場すら決まらなかった）
- [x] `shell_env` は Windows で no-op。**未使用 import 3 件も cfg で潰した**（受入条件の警告 0）
- [x] `host` crate の Remote SSH は Windows で cfg 除外（§D6）— 既存の `#[cfg(unix)]` が効いていた
- [x] 受入: **Windows で `cargo check --workspace --all-targets` がエラー 0・警告 0**

**W2 の範囲を超えて green にしたもの**（`cargo test --workspace` を Windows で通すため）:

| 落ちていたもの | 原因 | 直し方 |
|---|---|---|
| `lang` の LSP URI 3 件 | `url` crate の `to_file_path` は**実行中 OS の規則**で判定する。Windows の file URI はドライブレターが要るのに、テストの固定値が `file:///x/lib.rs`（POSIX 前提）だった | **実装ではなくテストの固定値が偏っていた**。`file_fixture()` でプラットフォーム別の組を作る |
| `project` の git status 2 件 | `std::fs::canonicalize` の verbatim（`\\?\`）と git の出力が一致しない | `paths::canonicalize`（W1 の追加分） |
| `project` の worktree 統合 1 件 | `core.autocrlf=true` で checkout が CRLF になり `"done\n"` ≠ `"done\r\n"` | テスト用リポジトリで `core.autocrlf=false` を固定（**mac でも周囲の global 設定に依存しなくなる**） |
| `project` の `all_files` 1 件 | 相対パスが `src\main.rs` になる | **`/` 区切りに正規化**（VSCode と同じ流儀。⌘P はユーザーが `/` で打つ・mac/Linux と同じ文字列になる） |

**結果: Windows 実機で `cargo test --workspace` = 235 テスト green・警告 0。**

### W3 — 起動して編集・保存ができる

- [x] Windows で起動し、ウィンドウが出て**文字が描画される**（DirectWrite 経路 = mac の font-kit 相当が効いているか）
  - **2026-08-22 実機で確認**。`title='necoder'` 1295x807 のウィンドウが出て、**日本語（「ようこそ」「未導入」「導入」）も絵文字（🎨）も正しく描画**される。同梱フォント（IBM Plex Sans JP / Guguru Sans Code）が DirectWrite 経路で効いている
  - レール（プロジェクト色つき）・宛先チップ（`necoder ▾ ⎇ main`）・エクスプローラの **git status（黄点と `M`）**・ウェルカム画面のエージェント一覧まで表示される
  - **`paths` crate が実アプリで効いていることも確認**: `%APPDATA%\necoder`（設定）と `%LOCALAPPDATA%\necoder`（`necoder.db` / `necoder.db-wal` / `state.json`）が分かれて作られ、`~\Library` は**作られない**。`necoder.db` が実在する＝**turso が実行時にも動く**
- [x] keymap の Windows 既定（§D4）。`Ctrl+S` で保存、`Ctrl+Shift+P` でパレット
  - `KeymapPlatform` + `default_keymap_json()` を新設。**mac 版を唯一の正として機械変換**する
    （2 本の JSON を手で並べると必ず片方だけ直されて腐るため）。差分は 3 つの表にだけ持つ:
    `NON_MAC_DROPPED` / `NON_MAC_REPLACEMENTS` / `NON_MAC_ADDITIONS`
  - **衝突検出テストが要**: `ctrl-a`（mac の emacs 風 行頭）と `cmd-a`→`ctrl-a`（SelectAll）は
    同じキーに落ちる。`BTreeMap` は後勝ちで**黙って片方を消す**ので、テストが無いと
    「なぜか効かないキーがある」という形でしか表に出ない
  - `pretty_keystroke_for()` で表記も分岐（`⌘⇧P` ↔ `Ctrl+Shift+P`）
  - **10 テスト green**。`cmd-` が 1 つも残っていないこと・macOS 専用概念（Hide / HideOthers / Minimize）を
    意図的に落としていること・**mac 側が 1 文字も変わらないこと**を機械で固定
  - **残: 実キー入力での確認は人の手番**（`Ctrl+S` を実際に押して保存されるか）
- [ ] **実キー入力の確認**（`Ctrl+S` 保存・`Ctrl+Shift+P` パレット）は実機で人が押す必要がある
  - → 2026-08-23 に `SendKeys` で自動化して確認済み（W3 の受入欄参照）。人の手番は **IME だけ**
- [ ] **locale に埋まった mac 記号 60 文字列**（2026-08-23 に発見・**D4 の積み残し**）
  - `locales/ja.yml` / `en.yml` に **各 30 行**、`⌘`×24 / `⇧`×11 / `⌃`×3 / `⌥`×2 が
    文字列の中に直接書かれている（例: `close_thread_tip: スレッドを閉じる  ⌘W` /
    `send_cmd: 送信 ⌘⏎` / `hint_submit_cmd: ⏎ 改行 / ⌘⏎ 送信`）
  - Windows / Linux では **mac の記号がそのまま出る**。ウェルカム画面と同じ不具合が、
    ツールチップ・コンポーザ・空状態メッセージに残っている
  - **推奨する直し方**（この codebase の作法に合う）: 記号を locale から抜き、`t!` の
    名前付き引数で埋める。`t!` は既に `"n" => index + 1` の形を持っている:
    ```yaml
    close_thread_tip: スレッドを閉じる  {key}
    ```
    ```rust
    i18n::t!("agent.close_thread_tip", "key" => keymap_core::keystroke_label("cmd-w"))
    ```
    表記の分岐は `keystroke_label()` が既に持っているので、**locale と呼び出し側を機械的に直すだけ**
  - 規模が大きい（~30 箇所）ので独立したタスクにしてある。**i18n parity テストが効くので
    ja/en の片方だけ直すと CI が止まる**（＝安全に進められる）
- [x] **CRLF ファイルの round-trip**: CRLF のファイルを開いて保存しても git diff が出ない
  - **2026-08-22 実機で実証**。CRLF のファイル（13 bytes / CRLF=2 / LF=0）を開いて文字を追加し `Ctrl+S`
    → **18 bytes / CRLF=2 / LF=0**。改行が 1 つも壊れていない。LF のファイルも LF のまま保存される
- [ ] **日本語入力（IME）**が編集領域で通る — **人の手番**。`SendInput` で合成できるのは仮想キーだけで、
      IME の変換・確定は実 IME の状態が絡むため自動化しても「通った」証明にならない
- [x] 受入: 実機で**開く → 編集 → 保存**の一巡 + スクショを Read して目視
  - 2026-08-22 実機で通した。`edit.txt` が 12 bytes → 22 bytes（`hello<LF>world<LF>WINDOWS-OK`）
  - 検証は `AttachThreadInput` で前面化 → `SendKeys` で打鍵 → `PrintWindow` で撮影 → **Read で目視**
  - **`SetForegroundWindow` は単体では効かない**。Windows のフォアグラウンド制限で弾かれ、
    打鍵が**別のウィンドウへ飛ぶ**（ファイルが変わらないので気づける）。
    `AttachThreadInput` で入力キューを繋いでから呼ぶこと

**実機でしか見つからなかったもの（この一巡で 3 件）**:

| 見つかったもの | 中身 |
|---|---|
| 初回起動でウォッチャが落ちる | 設定ディレクトリが無いのに watch していた。**mac は既存ユーザーのディレクトリが在るので表に出ていなかっただけ**で、まっさらな環境なら 3 プラットフォームとも起きる |
| ウェルカム画面が `⌘O` を直書き | Windows / Linux に **mac の記号が出る**。`keymap_core::keystroke_label()` を通すよう修正（表記の分岐は 1 箇所に集約） |
| キーバッジのレイアウト崩れ | バッジが `w(52px)` 固定で、mac の `⌘⇧A`（3 文字）に合わせてあった。**Windows の `Ctrl+Shift+A`（12 文字）が枠から溢れる**。`min_w` へ変更 |

**教訓**: 3 件とも「**mac だけを見ていては絶対に気づけない**」類で、しかも 2 件目・3 件目は
**キー表記が長くなること自体**が原因。文字数が変わる i18n / プラットフォーム表記は、
固定幅レイアウトを必ず壊すと思っておくこと。

### W4 — ターミナル・Git・ACP

- [x] ターミナルが ConPTY で開く。既定シェルは PowerShell（`pwsh` があれば優先、無ければ `powershell`）
      — **2026-08-23 完了。実機で `Ctrl+J` → PowerShell 起動 → `git status --short` の色付き出力まで確認**
  - **経過（残した理由は §「切り分けの記録」）**
  - ✅ **ConPTY は動く**（実機で確認）: necoder の子プロセスに
    `conhost.exe --headless --width 80 --height 24 --signal … --server …`（これが ConPTY の姿）と
    `powershell.exe` が居る
  - ✅ **既定シェルの選択を実装**: `Host::terminal_launch()` が Windows でだけ launch を返す。
    `pick_windows_shell()` が `pwsh` → `powershell` の順に PATH を見る（**PATHEXT 対応**）。
    unix は従来どおり `None` を返して alacritty の `$SHELL` 既定に任せる（mac 挙動不変・§D8）。4 テスト green
  - ✅ **`terminal_launch_for` の cwd 落ち**を修正。`Ok(Some(launch))` を **remote 専用**と決め打ちして
    cwd を捨てていたため、ローカル Windows で project root に開けなかった。host が remote か否かで分ける
  - ✅ `Ctrl+J` でドックが開く（Windows 既定 keymap が実機で効いている）
  - ❌ **端末の中身が描画されない**（＝受入条件未達）。ドックとタブ（「ターミナル 1」）は出るが**中身が真っ黒**
  - **2026-08-23 切り分け完了: PTY は無実。疑いは `TerminalView` の pump / 描画側。**
    - `crates/terminal_view/tests/pty_smoke.rs` を新設（**UI を通さない** PTY スモークテスト）。
      `TerminalView` と同じ組み立て（`Config` / `WindowSize` / `EventLoop`）でシェルを起こし、
      `echo` した文字列が `Term` に届くかを見る。**Windows 実機で green（1.53s）**
      ＝ **ConPTY・シェル起動・`Wakeup` の発火まで全部動いている**
    - キー入力も届いている（`Ctrl+J` の後に打った文字がエディタに入らない＝端末が消費している）
    - **残る疑いは `Wakeup` → `on_alac_event` → `sync()` → 描画の経路だけ**
    - 調査済みで**シロ**だったもの: gpui の Windows ディスパッチャは `dispatch_on_main_thread` で
      `PostMessageW(WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD)` を投げ、`wake_posted` も
      `platform.rs` で正しくリセットされる／`resize()` はグリッドを 0 にしない（`.max(2)` / `.max(1)` の下限あり）
  - **2026-08-23（続き）: 原因を「レイアウト」まで特定した。パイプラインは全部無実。**
    - `NECODER_TERM_PROBE=1`（`terminal_view.rs` に新設した計測口）で実機を 1 回動かした結果:
      ```
      pty: 起動
      pump: 1 件目 Wakeup          ← pump は回っている
      sync: 24 セル（うち非空白 10） ← sync も呼ばれている
      pump: 2 件目 Title(…powershell.exe)
      layout: bounds 164x0 / cell 6.6x17 → 24列 1行
      ```
    - **端末要素の bounds が `164x0`**。80×24 なら 1920 セルのはずが **24 セル**しか無い＝
      **グリッドが潰れているだけ**で、PTY も pump も sync も描画も動いている
    - **確証**: 端末の親に固定サイズ（`h(200) w(600)`）を与えると
      `bounds 600x200 → 90列 11行 / sync: 990 セル（非空白 162）`
      ＝ **PowerShell のバナーとプロンプトが実際に入っている**（実験後に元へ戻した）
    - **さらに二分**: `flex_1().min_h_0()` → `h_full().w_full()` に替えると **bounds 0x0**。
      ＝ **親（`TerminalDock` の外枠）が確定サイズを受け取れていない**。原因は body より**上**
    - **原因確定（4 通り試した）**:

      | 実験 | 変更 | bounds |
      |---|---|---|
      | A | `chrome.rs` から `.cached(...)` を外す | **572x206 ✅** |
      | B | `cached` を残し `flex_1` の親で包む | 164x0 ❌ |
      | C | `cached` に definite な高さを渡す | 164x0 ❌ |
      | **D（採用）** | **`dock.rs` の root を `flex_1().min_h_0()` → `size_full()`** | **572x206 ✅** |

    - **真因**: gpui の `.cached()` は **children を `None` にして layout を要求する**
      （"caching skips rendering the contents to measure them" — `gpui/src/view.rs`）。
      ＝ **cached の subtree は隔離してレイアウトされる**ので、その root で `flex_1()` は効かない
      （flex 親が居ない）→ 高さ 0 → グリッドが 24 セルに潰れて中身が真っ黒に見えた。
      **cached する view の root は `size_full()` で書く**のが正しい。置いた側が領域を決める、という
      元の意図もそのまま保たれる
    - 採った修正は **`dock.rs` の 1 箇所（2 行）**。`chrome.rs` と `fleet_view.rs` の
      `.cached(...)` はどちらも無変更のまま効く（両方とも同じ形で差し込んでいる）
    - ⚠️ **mac 未確認**（mac 実機が無い）。`flex_1()` は cached subtree の root では
      どのプラットフォームでも no-op のはずなので mac も同じく壊れている可能性があるが、
      **push する前に mac でターミナルが従来どおり出ることを確認すること**
- [x] Git 機能（gutter diff / status / ブランチ）が `git.exe` で動く。**改行の autocrlf 設定でも gutter が全行 Modified にならない**
      — **2026-08-23 確認**
  - 実機の証跡: エクスプローラの `M` マーカーと status ドット / エディタの gutter（追加行だけ緑・**全行が赤くならない**）/
    タイトルバーとステータスバーの `⎇ main` / ターミナルで `git status --short` が色付きで通る
  - **回帰テストで固定**: `project.rs::gutter_ignores_line_ending_differences_but_not_real_edits`。
    **`core.autocrlf=true` を明示的に立てて罠を再現**し、①改行しか違わなければ gutter が 1 本も出ない
    ②本物の変更は 1 hunk として出る（正規化が変更を隠していない）の両方を検証。
    **プラットフォームを問わず走る**（autocrlf は明示指定なので mac / Linux でも同じ状況を作れる）
  - `normalize_newlines()` は元から 3 経路（`diff_hunks` / `unified_diff_on` / もう 1 箇所）に入っていたが、
    **CRLF のテストが 1 本も無かった**。実装は正しく、守りが無かっただけ
- [x] ACP の実行ファイル解決が Windows でも効く（**`claude` は Windows では `claude.cmd`**）
      — **2026-08-23 修正**。エージェントとの 1 往復は**この機械に CLI が無いため未検証**（下記）
  - **§4 の「`CreateProcess` は `.cmd` を直接実行できない」は necoder には当たらない**。
    necoder は生の `CreateProcess` ではなく Rust の `std::process::Command` を使っており、
    **これは `.bat` / `.cmd` を検出して `cmd.exe` 経由で起動する**（CVE-2024-24576 の対応以降）。
    `host::cmd_scripts_can_be_spawned_directly` テストで固定した
  - **本当の問題は「探せるか」だった**: `acp_client::find_in_path` が PATH に素朴に join していて
    **`claude` を探して `claude.cmd` を見つけられない**。`host::executable_names()` /
    `host::find_in_path()`（PATHEXT 対応）へ集約し、`acp_client` と `lang::lsp` の両方をそこへ寄せた
    （同じロジックが 3 箇所に散り始めていたため）
  - npx キャッシュ探索の `~/Library/...` 前提は W1 で `zed_npx_cache_roots()` として対応済み
- [ ] **エージェントに 1 往復させてトランスクリプトが出る** — **この機械に `claude` / `node` / `npx` が無いため未検証**。
      CLI を入れれば検証できる状態にはなっている
- [ ] 受入（2 条件のうち**片方は達成**）:
  - [x] **ターミナルで `git status` が打てる** — 2026-08-23 実機で確認。`Ctrl+J` → PowerShell →
        `git status --short` の**色付き出力**をスクショで目視。cwd も project root
  - [ ] エージェントに 1 往復させてトランスクリプトが出る — **この機械に `claude` / `node` / `npx` が
        無いため未検証**。解決経路（PATHEXT）と起動経路（`.cmd`）は直してテストで固定済みなので、
        **CLI を入れれば確認できる状態**

### W5 — 検証ループと CI の常設

- [x] `scripts/screenshot-app.ps1`（PowerShell 版・`--features screenshot` のヘッドレス経路を使う）— **2026-08-22 追加**。mac 版の `screencapture` と違い**アプリの描画結果そのもの**が撮れる（他の窓が写り込まない・解像度が安定・CI でも回せる）
- [x] `scripts/startup-time.ps1` / `memory-usage.ps1` — **2026-08-22 追加**。**mac の数値と直接比べないこと**（DirectX/DirectWrite は Metal/font-kit と別経路。予算はプラットフォームごとに持つ）。メモリは `WorkingSet64` と `PrivateMemorySize64` を併記する
- [x] **実機で数値を取る** — **2026-08-23 実測**（release ビルド・NucBox M5PLUS / 3840x2160@150%）

  | 指標 | Windows（今回） | mac の記録（2026-07-17） |
  |---|---|---|
  | 起動（cold start → first render） | **522.7 ms**（5 回平均・501〜574） | ~215 ms |
  | idle ワーキングセット | **169 MB** | — |
  | idle private commit | **290 MB** | — |
  | idle RSS | — | 122 MB |

  - **mac の数値と直接は比べられない**。描画は DirectX + DirectWrite と Metal + font-kit で別物、
    メモリも `WorkingSet64` と RSS は定義が違う。
  - **起動の内訳を測った**（`NECODER_STARTUP_LOG=1` が段ごとの累積を出す・3 回の代表値）:

    | 段 | 累積 | 増分 | 中身 |
    |---|---|---|---|
    | `app_run_entered` | ~187 ms | **187 ms（36%）** | GPUI のプラットフォーム初期化（DirectX デバイス + DirectWrite） |
    | `fonts_loaded` | ~199 ms | 12 ms | 同梱フォント 4 本の読み込み |
    | `projects_resolved` → `before_open_window` | ~207 ms | 8 ms | プロジェクト解決・設定・テーマ |
    | `startup_ms` | ~519 ms | **312 ms（60%）** | `open_window` → 初回描画完了 |

  - **結論: 遅いのは necoder のコードではない。** necoder 自身の仕事は **519ms のうち ~20ms** しかなく、
    **96% が gpui のプラットフォーム初期化（187ms）と窓生成＋初回フレーム（312ms）**。
    「同梱フォントの読み込みが重いのでは」という当初の見立ては**外れ**だった（12ms）
  - 初回フレームの 312ms は DirectX のシェーダ準備・スワップチェーン生成が有力。
    **gpui は rev 固定で改変対象外**なので、necoder 側で縮められる余地は小さい。
    予算を決めるときはこの内訳を前提にすること（＝ Zed も同じ土台なので同じコストを払っているはず）
- [ ] **Windows の性能予算を決める** — **まだ決められない**。CLAUDE.md の予算は「**Zed 比 ~80%**」で、
      この機械に Zed が入っていないため比較対象が無い。**Windows 版 Zed を入れて同じ 2 本を測れば決まる**
- [x] **`ci.yml` に `check-windows` ジョブを新設**（`runs-on: windows-latest`・`cargo check --workspace --all-targets`（`RUSTFLAGS=-D warnings`）+ `cargo test --workspace`）— **2026-08-22 追加**。checkout **より前**に `core.autocrlf=false` を置く（既定のままだと作業ツリーが CRLF になり、CRLF 起因の差異を CI が見逃す）
- [x] `release.yml` の windows ジョブから `continue-on-error` を外す（リリース成果物の担保）— **2026-08-25**
  - ジョブ側の `continue-on-error: true` を削除。**付けたままだと Windows のビルドが落ちても
    Release だけが作られ、zip の付かないリリースが公開される**（＝配布物の欠落に気づけない）
  - 併せて W0 用の `cargo check --workspace --all-targets --keep-going` ステップも削除した。
    移植が終わって `ci.yml` の `check-windows` が同じ検査を毎 push で回しているので用済みで、
    しかも下の `build` とは `RUSTFLAGS` が違う（`+crt-static`）＝**フィンガープリントが別＝
    キャッシュが効かず丸ごと 2 回ビルドしていた**
- [x] 受入: **`ci.yml` の windows ジョブが green**（＝以後の退行を機械が止める）— **2026-08-24**、
      main への push で `check-windows` を含む **全 6 ジョブ green**（run 32748471930）
  - **残: branch protection の必須チェックに `check-windows` を入れる**（リポジトリ設定側の作業。
    現状 push は「2 of 2 required status checks」を bypass しており、必須集合に入っていない）

> **なぜ `ci.yml` に新設なのか（`release.yml` の `continue-on-error` を外すだけでは足りない）**:
> windows ジョブが今あるのは **`release.yml`（`.github/workflows/release.yml:128`）だけ**で、これは
> **タグ push か手動実行でしか走らない**。`continue-on-error` を外しても **PR は止まらない**＝
> 「以後の退行を機械が止める」という W5 の狙いを満たさない。
> 一方 `ci.yml` は `pull_request` トリガを持つが、ジョブは `test-macos` / `audit-deps` /
> `check-linux` / `build-remote-server` の 4 つで **windows が無い**。だから新設が要る。
> （`release.yml` 側は「リリース成果物が作れること」の担保なので、そちらも別途 `continue-on-error` を外す）

### W6 — 配布

- [x] release.yml で `necoder-windows-x64.zip` を作って GitHub Release に添付 — **2026-08-24**
  - **静的 CRT が必須**（この節で一番大事）。素の `cargo build --release` で作った exe は
    **`VCRUNTIME140.dll` / `VCRUNTIME140_1.dll` に依存する**（`dumpbin /DEPENDENTS` で実測）。
    これは **VC++ 再頒布可能パッケージ**の DLL で、**まっさらな Windows には無い**
    ＝ そのまま配ると「VCRUNTIME140.dll が見つかりません」で起動できない人が出る。
    開発機は Build Tools を入れた副作用で入っているので**手元では絶対に気づけない**
  - `RUSTFLAGS=-C target-feature=+crt-static` で解決。依存は **26 個すべて Windows 標準**になる
    （`kernel32` / `user32` / `d3d11` / `dwrite` / `icuuc` …。`api-ms-win-crt-*` は
    Universal CRT ＝ Windows 10 以降に標準搭載）
  - **配る前に機械で検証する**: CI に「VCRUNTIME 依存が無いことを確認」ステップを入れてあり、
    残っていたらビルドを落とす。手元用は `scripts/bundle-windows.ps1` が同じ検証をする
  - 実測: zip **28 MB** / exe 75.9 MB。タグ push で Release に添付（手動実行では artifact どまり）
- [ ] **AGPL の義務を満たす**（バイナリ配布の前提）。AGPL-3.0 は**バイナリを配ったら対応するソースを
      提供する義務**が生じる。リポジトリが private のままだと満たせない。
      **公開するか、別途ソース提供の導線を用意するかを決める必要がある**（ユーザー判断）
- [ ] updater の Windows 分岐（`.dmg`/`spctl`/`hdiutil` 経路を通らない）。当面は「新版あり → ブラウザで Release を開く」でもよい
      — **実装済み 2026-08-27・実機未確認のためチェック保留**。`updater::UpdateAction` で経路分岐:
      Windows は zip 付きリリースのみ「⬆ vX.Y.Z を入手」チップ（`update.get`）→ クリックで
      `crash::open_url`（`rundll32 url.dll,FileProtocolHandler`。cmd `start` は URL 中の `&` を
      解釈するので不採用）→ Release ページを既定ブラウザで開く。zip の入れ替えは手動。
      アプリ内適用（zip 展開 → 実行中 exe の差し替え）は後続。
      **実機確認**: チップ表示（旧版起動 or `NECODER_UPDATE_PROBE`）→ クリックでブラウザが開く
- [ ] README に Windows の導入手順（SmartScreen 警告の説明込み）
- [ ] 受入: **まっさらな Windows マシンで DL → 起動 → プロジェクトを開いて編集できる**

### やらないこと（当面・意図的に）

- Remote SSH の Windows クライアント（§D6）
- ARM64 Windows ビルド（x64 が安定してから）
- MSIX / Microsoft Store（§D7）
- Windows 固有の外観追随（Mica / アクリル）— **`UI-SPEC.md` の色の許可リストが正**であり、OS の流儀に寄せない

---

## 4. Windows 固有の罠（実装前に必ず読む）

| 罠 | 中身 | 効く場所 |
|---|---|---|
| **パス区切り** | `\` と `/` が混ざる。文字列連結で組んだパスは壊れる | 全域。`Path::join` を徹底 |
| **大文字小文字** | ファイルシステムが非区別。`Foo.rs` と `foo.rs` が同一 | エクスプローラ・検索・gitignore・タブの同一判定 |
| **MAX_PATH 260** | 長パスは `\\?\` プレフィクスか長パス有効化が要る | 深い node_modules を持つプロジェクト |
| **予約名** | `CON` `PRN` `AUX` `NUL` `COM1`〜 はファイル名にできない | ファイル作成・リネーム |
| **ファイルロック** | 開いているファイルは削除・リネームが**失敗する**。mac と挙動が違う | 保存（temp + rename）・watch・git 操作 |
| **CRLF** | 既定の改行が CRLF。git の autocrlf も絡む | editor_core・保存・git gutter |
| **`.cmd` / `.bat`** | `CreateProcess` は直接実行できない（PATH 解決も `PATHEXT` 依存） | ACP エージェント起動・外部ツール |
| **コンソール窓** | GUI アプリから子プロセスを起こすと黒窓が一瞬光る | ターミナル以外の全 shell-out（`CREATE_NO_WINDOW` を付ける） |
| **`windows_subsystem`** | 付けないと GUI アプリでもコンソールが開く。付けると stdout が消える | `main.rs`・ログ設計（logging の Windows 実装と一緒に決める） |
| **環境変数** | 大文字小文字非区別。`HOME` は**存在しない**（`USERPROFILE`） | paths crate |

### スクショを撮るプロセスは **DPI 認識**を宣言すること（2026-08-23 に誤診まで踏んだ）

**これを忘れると「機能が動いていない」と誤診する。** 実際に一度誤診した。

DPI 非対応のプロセスは Windows に座標を仮想化される:

| 呼ぶもの | 返る値（150% スケーリングの場合） |
|---|---|
| `GetWindowRect` | **論理** 1295x807 |
| `PrintWindow` の描画 | **物理** 1942x1211 |

＝ ビットマップが足りず **ウィンドウの左上 2/3 しか写らない**。下ドック（ターミナル）と
右ドック（Agent パネル）が丸ごと画面外になり、「`Ctrl+J` を押してもターミナルが開かない」
と読めてしまう。**実際は開いていた。**

対策は `SetProcessDPIAware()` を **`GetWindowRect` / `PrintWindow` より前に**呼ぶこと
（`scripts/screenshot-app.ps1` は対応済み）。手で撮る道具を書くときも必ず入れること。

**教訓**: スクショで「無い」ことを確認したときは、**撮影範囲がウィンドウ全体を覆っているか**を
先に疑う。撮影サイズと `GetWindowRect` の値をログに出しておくと一発で分かる。

### `.ps1` は UTF-8 **BOM 付き**でなければならない（2026-08-22 に実際に踏んだ）

**Windows PowerShell 5.1 は BOM の無いファイルをシステム ANSI**（日本語環境では Shift-JIS）
**として読む。** 日本語コメントの入った `.ps1` を BOM 無しで置くと、文字が化けるだけでなく
**構文ごと壊れて実行できない**（`Unexpected token '竊・縺薙・'` のようなエラーになる）。

厄介なのは 2 点:

- **エディタや編集ツールは BOM を落とすことがある**。一度直しても次の編集で再発する
- **`pwsh`（PowerShell 7）は BOM 無しでも UTF-8 として読める**。手元で pwsh を使っていると気づけず、
  5.1 しか無い環境で初めて壊れる

対策は `ci.yml` の `check-windows` に入れてある — **`shell: powershell`（5.1）で `scripts/*.ps1` を
パースする**ステップ。既定の `pwsh` で回すと BOM 無しでも通ってしまい**チェックの意味が消える**ので、
シェルの指定を外さないこと。

PowerShell 5.1 由来の落とし穴は他にも 2 つある。どちらも 2026-08-23 に実際に踏んだ:

- **`Start-Process -Environment` は 7.4 以降にしか無い。** 5.1 で子プロセスへ環境変数を渡すには、
  呼び出し側の `$env:` に置いて継承させる
- **ネイティブ exe に `2>&1` を付けてはいけない。** 5.1 は exe の stderr を **1 行ずつ ErrorRecord に包む**ため、
  `cargo` が終了コード 0 で成功していても `NativeCommandError` で落ちる
  （`& cargo build 2>&1 | Out-Null` がまさにこれで失敗した）。**素通しでコンソールに流し、
  成否は `$LASTEXITCODE` で見る**こと

### CRLF は「起きるかも」ではなく「確定で起きる」（2026-08-22 実機で確認）

Windows 実機で確認した事実:

- Git for Windows の**システム gitconfig が `core.autocrlf=true`**（`C:/Program Files/Git/etc/gitconfig`・**インストーラ既定**）
- necoder のリポジトリに **`.gitattributes` が無い**

この組み合わせだと、checkout 後の working copy は **CRLF**・index の blob は **LF** になる。
necoder の gutter diff は「git CLI + imara-diff」で **index の blob と working のバイト列**を突き合わせるので、
**W4 の「gutter が全行 Modified」は条件が揃えば必ず起きる**（可能性の話ではない）。

**対策は 2 つあるが、採れるのは片方だけ**:

| 案 | 中身 | 可否 |
|---|---|---|
| (a) necoder 側で比較前に改行を正規化する | gutter の比較経路に LF 正規化を効かせる | **必須。これしかない** |
| (b) リポジトリに `.gitattributes`（`* text=auto eol=lf`）を置く | 改行を LF に固定する | **不可**（下記） |

(b) が採れない理由は 2 つ。**① necoder が開くのは「他人のリポジトリ」**であって、
そこに `.gitattributes` を置かせることはできない＝ユーザーの手元で普通に起きる。
**② necoder 自身のリポジトリに入れると mac 側の working copy にも renormalize が走る**＝§D8 の鉄則
（mac の挙動を 1 ミリも変えない）に触れる。入れるかどうかはユーザー判断であって、W フェーズが勝手に決めない。

したがって **W4 の受入条件「gutter が全行 Modified にならない」は (a) の実装を指す**。
なお `project.rs` の既存 LF 正規化は **git 差分用であって保存経路ではない**（W3 参照）ので、
gutter 経路に効いているかは別途確認が要る。

---

## 5. 該当箇所インベントリ（2026-08-22 時点・調査済み）

### コンパイルが落ちる（無条件の unix API）

| 場所 | 内容 |
|---|---|
| `crates/necoder/src/fleet.rs:86` | `std::os::unix::net::UnixStream::connect`（cfg 無し） |
| `crates/workspace/src/workspace/control_ipc.rs:22` | `use std::os::unix::net::{UnixListener, UnixStream}`（cfg 無し） |

### パス（W1 の対象・全量）

`Library/Application Support` ハードコード（9 ファイル）:
`storage.rs:155,161` / `settings_core.rs:261` / `persistence.rs:40` / `crash.rs:29` /
`logging.rs:27` / `shell_env.rs:50` / `brand_migration.rs:23,29` / `acp_client.rs:751`（Zed の npx キャッシュ）

`HOME` 直読み（19 箇所）:
`acp_client.rs:699,707,750` / `host.rs:1652,2113` / `lsp.rs:81,108,988,1017` /
`settings_core.rs:260` / `storage.rs:154,160` / `persistence.rs:39` / `shell_env.rs:49` /
`brand_migration.rs:22,28` / `crash.rs:28` / `logging.rs:26` / `control_ipc.rs:30`
（+ `panels.rs:140` の `std::env::home_dir()`）

### 外部コマンド（W2/D3 の対象・全量）

- **mac 専用**: `/usr/bin/afplay`（agent_panel）/ `/usr/bin/open`・`/usr/bin/osascript`・`/usr/bin/trash`（project）/ `sw_vers`（crash）/ `hdiutil`・`ditto`・`spctl`（updater）
- **unix 前提**: `sh -c` ×10（`project.rs` ×4 / `todos.rs` / `acp_client.rs` / `host.rs` 他）/ `id -u`・`mkfifo`・`ssh`（host）/ `$SHELL`（shell_env）
- **そのまま使える**: `git`（`git.exe`）/ `curl`（Windows 10 以降 同梱）
- **CLI シム（2026-08-30 追加・W フェーズまで非対応）**: `crates/cli_shim`（`/usr/local/bin/ne` + `osascript` 昇格）と `crates/necoder/src/cli.rs` の `open -n`。Windows でもコンパイルは通る（stub + cfg 済み）が `supported()=false` ＝設定画面のセクション非表示・`install-cli` は明示拒否。Windows の PATH 投入（per-user bin + PATH 追記等）は W フェーズで設計

### プラットフォーム分岐が要る UI

- `keymap_core.rs:153-` 既定 keymap 全体（`cmd-`）と `pretty_keystroke` の `⌘` 表記
- `crates/necoder/src/menus.rs`（`cx.set_menus` = mac ネイティブメニュー）
- `agent_panel.rs:7212` 完了音

### スクリプト

- `scripts/screenshot-app.sh` / `bundle-mac.sh` / `install-mac.sh` / `startup-time.sh` / `memory-usage.sh` — 全て mac 専用。W5 で PowerShell 版

---

## 6. 検証コマンド（PowerShell）

```powershell
cd C:\Users\<user>\Desktop\dev\necoder   # WSL の /mnt/c/... と同じ実体
$env:CARGO_TARGET_DIR = "$PWD\target-win"   # WSL ビルドと target を分ける（必須）

cargo check -p necoder            # 速い整合性確認
cargo test --workspace            # ロジック

# 移植中の「残りエラー全量」を取る（W0〜W2 で使う）
# --keep-going が無いと最初に落ちた crate で停止して先が見えない
cargo check --workspace --all-targets --keep-going

# ターミナルが「開くのに真っ黒」のとき: PTY → pump → sync → layout のどこで
# 止まっているかを出す（2026-08-23 の W4 で実際にこれで原因を特定した）
$env:NECODER_TERM_PROBE = "1"   # 各段の到達とセル数・bounds を stderr へ
$env:NECODER_TERMINAL = "1"     # 下ドックを開いた状態で起動
cargo run -p necoder

# PTY 単体（UI を通さない）。これが green なら PTY は無実で、疑うのは pump より下流
cargo test -p terminal_view --test pty_smoke

# UI 検証（OS 非依存のヘッドレス経路。出た PNG を Read して目視する）
$env:NECODER_SCREENSHOT = "$PWD\shot.png"
cargo run -p necoder --features screenshot
Remove-Item Env:\NECODER_SCREENSHOT

# 実機で触る
cargo run -p necoder
```

**mac 側の非退行確認を忘れない**: W フェーズの変更は共有コードに入るので、
mac で `cargo test --workspace` + 起動確認まで通って初めて「1 歩完了」。
