# 同梱フォント（bundled fonts）

necoder は以下のフォントを `crates/necoder` にバイナリ埋め込み（`include_bytes!` + `add_fonts`）している。
いずれも **SIL Open Font License 1.1（OFL）**。OFL フォントは AGPL-3.0 の本体に同梱・再配布してよい
（フォント自体は OFL のまま。OFL 全文は `IBMPlexSansJP-OFL.txt` を参照＝同一ライセンス本文）。

| ファイル | family 名（`.font_family()`） | 用途 | 出自 |
|---|---|---|---|
| `IBMPlexSansJP-Regular.ttf` / `-SemiBold.ttf` | `IBM Plex Sans JP` | UI（プロポーショナル） | IBM Plex（© IBM Corp.）/ Google Fonts `ofl/ibmplexsansjp` |
| `GuguruSansCode-Regular.ttf` / `-Bold.ttf` | `Guguru Sans Code` | コード（等幅・Google Sans Code + IBM Plex Sans JP） | Guguru Sans Code v0.0.3（© 2025 Yuko Otawara）/ github.com/yuru7/guguru-sans-code |

- **Guguru Sans Code** = Google Sans Code（等幅ラテン）に IBM Plex Sans JP（日本語）を合成したコーディングフォント。
  UI 用の IBM Plex Sans JP と血統が揃い、日本語コメントも桁が揃う。`isFixedPitch` あり（等幅）。
  ※ 標準の 1:2 幅版を採用（`GuguruSansCode35` = 3:5 幅・`*Console*` = 端末向けは不使用）。
- 更新時は各リポジトリの最新 OFL 版を取得し、この表と `*-OFL.txt` を更新すること。
- ファイル総量は ~15MB（バイナリに埋め込むためビルド成果物が同分増える）。
