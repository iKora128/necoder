# Changelog

[Keep a Changelog](https://keepachangelog.com/) 形式 / [Semantic Versioning](https://semver.org/)。
リリース手順は [`docs/RELEASE.md`](docs/RELEASE.md)（タグを切ると CI が署名済み .dmg を Releases へ添付する）。

## [Unreleased]

### Added
- macOS アプリに `necoder` CLI ランチャーを同梱。`necoder .` / `necoder <file>` は起動中の
  ウィンドウを再利用し、`--new-window`、`ssh://`、既存の `config` / `fleet` / `mcp`
  サブコマンドにも対応。初回画面と設定から macOS の認証ダイアログ経由で導入でき、
  `Contents/Resources/install-cli.sh` からの手動導入も可能

## [0.1.2] - 2026-08-25

### Fixed
- **配布物が起動しなかった致命的な不具合を修正**（v0.1.0 / v0.1.1 の `.dmg` と `.zip` が該当）。
  マスコット画像の読み込みが `env!("CARGO_MANIFEST_DIR")` を**実行時のパス**として開いていたため、
  **ビルドした機械以外では起動直後に panic** していた:

  ```text
  mascot asset D:\a\necoder\necoder\crates\agent_panel/assets/mascot\idle.png:
  指定されたパスが見つかりません。 (os error 3)
  ```

  （`D:\a\necoder\necoder` は GitHub Actions ランナーのパス）
  フォント・アイコンと同じく `include_bytes!` でバイナリに埋め込むよう修正した。
  **v0.1.0 / v0.1.1 は使えません。本版へ更新してください。**
- クラッシュログが**二重 panic のとき 1 度目 = 本来の原因を消していた**。起動クロージャ内の
  panic は GPUI の extern "C" 境界を unwind できず 2 度目の panic（cannot unwind）になり、
  同一秒・同一 PID の `crash-<unix秒>-<pid>.log` を上書きしていた（上記の調査で実害）。追記式に変更

### Added
- 上記の再発を機械で止める回帰テスト（`crates/necoder/tests/no_runtime_manifest_paths.rs`）。
  ビルド時のパスを実行時に開いている箇所を全 crate から検出する。**このバグは開発機では
  原理的に再現しない**（そのパスが手元には実在するため）ので、人間の目では防げない
- release CI に**最終成果物側の裏取り**も追加: 配布バイナリに `strings` でビルド機のパス
  （`$GITHUB_WORKSPACE/crates`）が残っていたら fail。併せてリモートサーバ探索の開発用
  fallback（`target/debug/…`）を debug ビルド限定にし、release バイナリからビルド機の
  絶対パスを全廃（ガードの期待値を 0 件にできる）
- `release.yml` の Windows ジョブで VCRUNTIME 依存の検証が実際に動くようになった
  （`Program Files` 側の VS を見ておらず、これまで一度も検証していなかった）

## [0.1.1] - 2026-08-24

### Added
- **Windows ネイティブ対応**（`docs/WINDOWS-PORT.md` の W0〜W6）。`necoder-windows-x64.zip` を配布物に追加:
  - 起動・編集・保存・検索・Git・統合ターミナル（ConPTY + PowerShell）が Windows で動作
  - `paths` crate を新設し、設定/DB/ログの置き場を 1 箇所に集約。Windows は Roaming（設定）と
    Local（DB・ログ）を分ける。**Linux で `~/Library/Application Support/` を作っていた既存バグも解消**
  - 制御 IPC を Unix socket / 名前付きパイプで抽象化
  - Windows 既定 keymap（VSCode 準拠の `Ctrl-` 系）とキー表記の出し分け（`⌘S` ↔ `Ctrl+S`）
  - タイトルバーのキャプションボタン（最小化・最大化・閉じる）
  - CI に `check-windows` を常設（`-D warnings` + `cargo test --workspace`）

### Fixed
- ターミナルのドックが**中身を描画しない**不具合（`.cached()` した view の root は `flex_1()` が
  効かず高さ 0 に潰れる）。mac にも影響していた可能性がある
- 初回起動で設定ディレクトリが無いときにファイル監視が失敗していた（3 プラットフォーム共通）
- git gutter が CRLF の作業ツリーで全行 Modified になり得た問題に回帰テストを追加

### Notes
- Windows 版は**未署名**（初回起動で SmartScreen の警告が出る）
- **アプリ内更新は macOS のみ**。Windows は zip の再ダウンロードで更新する

## [0.1.0] - 2026-08-22

### Added
- 初回公開リリース（v0.1.0）に向けた整備:
  - Finder/Dock からのファイル/フォルダオープン（`CFBundleDocumentTypes` + `on_open_urls` 配線）
  - GUI 起動（Finder/Dock）時のアプリログ `~/Library/Application Support/necoder/logs/`（20 本保持）
  - 依存ライセンス監査（cargo-deny）を CI に常設
  - CLA / CONTRIBUTING / SECURITY / Code of Conduct を整備
- エディタ本体はここまでの M0〜M14 で実装済み（色レール・ACP スレッド・LSP・tree-sitter 多言語・
  Git hunk/blame・統合ターミナル・Remote SSH・マルチエージェント編隊 — 詳細は `docs/ROADMAP.md`）

### Fixed
- リリースのバージョン配線を一本化（`Cargo.toml` を唯一の出所に。タグ不一致は CI が拒否 —
  旧: Info.plist ベタ書きで、上げ忘れると更新チップが無限に出る実害）
- 更新・管制 IPC のエラーメッセージを i18n 化（英語ロケールに日本語エラーが出ていた）

<!-- リリース時: Unreleased を [x.y.z] - YYYY-MM-DD へ繰り上げ、新しい Unreleased 節を上に作る -->
