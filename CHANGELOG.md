# Changelog

[Keep a Changelog](https://keepachangelog.com/) 形式 / [Semantic Versioning](https://semver.org/)。
リリース手順は [`docs/RELEASE.md`](docs/RELEASE.md)（タグを切ると CI が署名済み .dmg を Releases へ添付する）。

## [Unreleased]

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
