# Git・ターミナル・LSPの独立実装ノート

作成: 2026-07 / 来歴と表現を2026-08-27に訂正。

この文書は、necoderの実装が依拠する公開仕様とpermissiveなライブラリ、およびZedとの設計比較を記録する。
ZedのソースはGPUI APIの利用例と比較調査のために閲覧しているため、厳密なclean-room実装とは称さない。
一方、ZedのGPLアプリケーションcrateからソースコードを複製、翻訳、改変して取り込まない。
実装の正はnecoderのコードと、以下に挙げる公開仕様・ライブラリのAPIである。

## 1. Git statusとgutter diff

- Git状態は公開されたGit CLIのporcelain形式を使う。repo-local設定からの意図しないコード実行を避けるため、
  自動実行経路ではhooks、fsmonitor、外部transportを無効化する。
- 行差分はcrates.ioの`imara-diff`を使い、necoderの`StatusKind` / `DiffHunk`へ変換する。
- CRLF/LFを比較前に正規化し、UIスレッド外でGitとdiff計算を実行する。
- 実装: `crates/project/src/project.rs`、表示: `crates/git_ui/`と`crates/workspace/`。

比較調査では、ZedもGit CLIと`imara-diff`を利用していることを確認した。ただしこれは採用技術の比較情報であり、
Zedの`git` / `buffer_diff`実装をnecoderへ移す指示または出自を表すものではない。

## 2. 統合ターミナル

- crates.ioの`alacritty_terminal`がPTY、VTE parser、terminal state、event loopを提供する。
- necoderはその公開APIをGPUIの描画・入力・dockモデルへ接続する。
- 出力イベントが来たときだけsnapshotと再描画を行い、idle pollingは行わない。
- キー入力、IME、selection、link検出、配色はnecoder側の要件と型で実装する。
- 実装: `crates/terminal_view/src/terminal_view.rs`。

ZedもAlacritty系terminalを利用しているためAPI利用例として比較したが、Zedの`terminal` / `terminal_view`の
ソースコードやkey mapping実装をnecoderへ取り込まない。

## 3. Language Server Protocol

- protocolの正は公開Language Server Protocol仕様とJSON-RPC 2.0仕様。
- necoderは必要なJSON-RPC envelopeとLSP型を最小限定義し、`HostProcess`のstdin/stdoutへ接続する。
- `Content-Length`はUTF-8 byte length、LSPの`Position.character`はUTF-16 code unitとして扱う。
- 診断、補完、hover、定義、format、rename等はserver capabilityを確認して送る。
- reader/writerは背景で待機し、channel経由でUI側へ通知する。
- 実装: `crates/lang/src/lsp.rs`と`crates/workspace/src/workspace/editor_area/language.rs`。

Zedを含む複数のeditor実装をレイヤ分割の比較対象にしたが、Zedの`lsp` / `project` / `language` crateの
ソースコードをnecoderへ取り込まない。

## 4. ライセンス境界

- necoder固有コード: AGPL-3.0-or-later。
- `imara-diff`、`alacritty_terminal`、tree-sitter、ACP SDK等: 各上流のpermissiveライセンス。
- `gpui` / `gpui_platform`: Zed側でApache-2.0表示。
- 現在固定するGPUI revisionの推移依存`ztracing` / `ztracing_macro` / `zlog`: GPL-3.0-or-later。

最後の3 crateはGPLv3/AGPLv3 §13の組み合わせ規定に基づき、necoderのAGPL配布物に含める。
詳細と第三者通知は`docs/DECISIONS.md` §5と`THIRD_PARTY_NOTICES.md`を正とする。
