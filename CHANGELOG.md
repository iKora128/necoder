# Changelog

[Keep a Changelog](https://keepachangelog.com/) 形式 / [Semantic Versioning](https://semver.org/)。
リリース手順は [`docs/RELEASE.md`](docs/RELEASE.md)（タグを切ると CI が署名済み .dmg を Releases へ添付する）。

## [Unreleased]

### Added
- 初回公開リリース（v0.1.0）に向けた整備:
  - Finder/Dock からのファイル/フォルダオープン（`CFBundleDocumentTypes` + `on_open_urls` 配線）
  - GUI 起動（Finder/Dock）時のアプリログ `~/Library/Application Support/Shirushi/logs/`（20 本保持）
  - 依存ライセンス監査（cargo-deny）を CI に常設
  - CLA / CONTRIBUTING / SECURITY / Code of Conduct を整備
- エディタ本体はここまでの M0〜M14 で実装済み（色レール・ACP スレッド・LSP・tree-sitter 多言語・
  Git hunk/blame・統合ターミナル・Remote SSH・マルチエージェント編隊 — 詳細は `docs/ROADMAP.md`）

### Fixed
- リリースのバージョン配線を一本化（`Cargo.toml` を唯一の出所に。タグ不一致は CI が拒否 —
  旧: Info.plist ベタ書きで、上げ忘れると更新チップが無限に出る実害）
- 更新・管制 IPC のエラーメッセージを i18n 化（英語ロケールに日本語エラーが出ていた）

<!-- リリース時: Unreleased を [0.1.0] - YYYY-MM-DD へ繰り上げ、新しい Unreleased 節を上に作る -->
