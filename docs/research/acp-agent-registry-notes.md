# ACPエージェント・レジストリの独立実装ノート

作成: 2026-09-02。

この文書は、necoderが外部ACPエージェント（`claude-agent-acp` / `codex-acp` など）の**版と起動方法を
どこから得るか**について、依拠する公開仕様と、Zedとの設計比較の境界を記録する。
ZedのソースはGPUI APIの利用例と比較調査のために閲覧しているため、厳密なclean-room実装とは称さない。
一方、Zedの`agent_servers` / `agent_registry_store` / `agent_server_store`などGPLアプリケーションcrateから
ソースコードを複製、翻訳、改変して取り込まない。実装の正はnecoderのコードと、以下の公開仕様である。

## 0. なぜ必要になったか

necoderは`crates/acp_client/src/acp_client.rs`の`const AGENTS`に、各エージェントのnpmパッケージ版を
完全一致で焼き込んでいた。この形はエージェントを1つ上げるたびにnecoderの再ビルドとリリースを要求する。
実際に`codex-acp`は`1.1.14`のまま止まり、その間にupstreamは`1.8.0`まで進んでいた（2026-09-02実測）。
`codex-acp`は`@openai/codex`本体を依存に含むため、**アダプタの版がvendor CLI本体の版まで決めてしまう**
という二重の固定になっていた。

## 1. 依拠する公開仕様

- **レジストリURL**: `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`
- **提供元**: Agent Client Protocolプロジェクト自身が配布する**ベンダー中立の公開CDN成果物**。
  特定エディタの成果物ではないため、necoderは第三者の配布物に相乗りするのではなく、
  プロトコルの公開資産を直接読む。
- **スキーマ**（2026-09-02時点で実物から確認・39項目）:
  - トップレベル `{ version, agents[] }`
  - 各項目 `{ id, name, version, description, authors, license, distribution, icon,
    repository?, website?, license_url? }`
  - `distribution` は `npx: { package, args?, env? }` と
    `binary: { "<os>-<arch>": { archive, cmd, args?, env?, sha256? } }` の任意の組み合わせ
  - プラットフォームキーの語彙: `darwin-aarch64` / `darwin-x86_64` / `linux-aarch64` /
    `linux-x86_64` / `windows-aarch64` / `windows-x86_64`
- 実装: `crates/acp_client/src/registry.rs`。パースは`serde_json::Value`からnecoderの型
  （`Registry` / `RegistryAgent` / `Distribution`）へ写す。ネットワークは`curl`に委ねる
  （`crates/workspace/src/updater.rs`と同じ依存ゼロの流儀）。

## 2. necoder側の設計（Zedとは別に決めたこと）

- **解決順は「設定 → レジストリ → 組み込みカタログ」**（`AgentKind::resolve_command`）。
  `const AGENTS`は削除せず、**necoderが検証した既定値**として最下層に残す。
  レジストリが落ちていても、オフラインでも、起動できる状態を保つため。
- **キャッシュ先は`paths` crateが決める**（`paths::acp_registry_cache`）。
  necoderの他の置き場と同じ規律に従い、`HOME`直読みをしない（WINDOWS-PORT.md §D1）。
- **起動時はキャッシュだけを読む**（`registry::load_cached`）。ネットワークは起動12秒後に背景で
  後追いする（`Workspace::schedule_agent_registry_refresh`）。アップデート確認と同じ抑止スイッチ
  （`NECODER_NO_UPDATE_CHECK` / `NECODER_SCREENSHOT`）に従う。
- **取り直した結果を走行中のスレッドへ適用しない**。次に起動するセッションから効く。
- **1件の不備で全体を捨てない**。`id` / `version` / 起動方法のいずれかを欠く項目だけを落とす。
  レジストリは他人が更新するので、知らないキーや壊れた項目が混ざる前提で読む。
  実物39件のうち2件（`fast-agent` / `minion-code`）は`distribution`が空で、この規則により落ちる。
- **バイナリ配布は「在ることが分かる」ところまで**。ダウンロード・sha256検証・展開・版別ディレクトリ・
  GCは未実装で、`Launch::BinaryNotSupportedYet`という型で明示的に表す（黙ってnpxへ落とさない）。

## 3. Zedから比較参照した設計上の判断

以下はZedの外部エージェント実装を調査して得た**設計上の知見**であり、
実装はnecoderの型と要件から独立に組み立てている。

- **完全一致ピンを避け、上限つき範囲にする**。`pkg@1.2.3`ではなく`pkg@0.0.0 - 1.2.3`。
  理由は、npmの`min-release-age`を設定している環境では公開直後の版が完全一致指定では入らないため。
  `<=1.2.3`ではなくハイフン記法を使うのは、Windowsで`npm.cmd`をPowerShell経由で起動すると
  `<`が入力リダイレクトとして解釈されるため。
  necoderの実装は`crates/acp_client/src/acp_client.rs`の`bounded_npm_spec`。
  ハイフン範囲がnpmに受理され上限を守ることは実測で確認した（`pkg@0.0.0 - 1.2.0`が`1.2.0`に解決し、
  より新しい`1.8.0`を選ばない）。
- **ユーザー設定に逃げ道を必ず置く**。レジストリが壊れても、新しい版を先に試したくても、
  ユーザーがリリースを待たずに回避できること。
- **コマンドを持つ設定と、環境変数だけの設定を分ける**。「レジストリ管理のエージェントのコマンドだけを
  半端に差し替える」形を作らせないため。necoderでは`AgentServerSetting::Custom`（command/args/env）と
  `AgentServerSetting::Registry`（envのみ）の2形（`crates/settings_core/src/settings_core.rs`）。
- **版の更新は通知に留め、走行中の作業を裏で止めない**。

これらは採用した設計判断の出所を記録するものであり、Zedのコードをnecoderへ移す指示または出自を
表すものではない。necoderの`registry.rs`はZedの対応するcrateとファイル構成・型・関数分割のいずれも
共有しない。

## 4. 未実装（次にやるなら）

- バイナリ配布の配備（DL → sha256検証 → 展開 → 版別ディレクトリ → 起動 → 古い版のGC）。
  これが入ると`kimi`のようにnpm外のエージェントが`curl | bash`の案内なしで使えるようになる。
  実物レジストリでは18件がバイナリ配布を持つ。
- レジストリ由来の新版を検知したときの「更新できます」導線（UI）。
- 設定画面がレジストリの一覧（実物39件）を出す。現在の組み込みカタログは7件。

## 5. 関連する決定

- エージェントをリモートで起動するか手元で起動するかは`docs/DECISIONS.md`の
  「リモートのエージェントはremote側で起動する（2026-09-02）」を参照。
  ACPの`fs`/`terminal`委譲が主要アダプタで未実装であることを実測で確認した結果、
  当面はremote側で起動する。
