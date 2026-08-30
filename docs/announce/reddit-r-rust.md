# アナウンス文 v3（Zenn 原稿 × 旧ドラフトのかけ合わせ・2026-08-30）

構成: 入りと骨格は Zenn 原稿（Cursor 20枚 / 怒り駆動開発 / 空間識失調 / マルチプレクサ）、
技術の弾は旧ドラフト（GPUI rev 固定・移植ゼロ・実測値・ファズ・正直な制約）。
英語版は r/rust 用、日本語版は X / 日本語圏の告知用。

## タイトル案

英語（r/rust・フレアは 🛠️ project）:

> **I got fed up with juggling 20 Cursor windows, so I wrote a code editor from scratch in Rust on GPUI (Zed's UI framework)**

日本語:

> **Cursorのウィンドウを20個開いていた自分が、Rust × GPUIでコードエディタを自作した**

## 英語版本文（約450語）

I used to develop with 10–20 Cursor windows open at once. That stopped scaling, so for the past two months I've been building my own editor: **necoder** — written from scratch in Rust on **GPUI**, the UI framework behind Zed, used here as a standalone git dependency. Not a VSCode fork. AGPL-3.0, macOS and Windows. More than half of my daily work now happens in it.

It began as anger-driven development: Electron's memory appetite, no sane way to run many projects and branches in one window, and never being sure which AI agent I was talking to. The one thing I refused to give up was the UX of the Claude Code extension. I tried moving to Zed first — but its extensions can't render UI at all, so per-thread and per-project colors are structurally impossible. If it doesn't exist, build it.

Three design rules:

- Parallel tasks go horizontal (tabs); concurrent projects go vertical (Slack-style workspace switching).
- Color is a sense of direction. Every project and agent carries an identity color that runs through the rail, tab underlines and caret. Modern editors have lost this spatial awareness — you get vertigo and fire prompts at the wrong agent.
- The editor is a multiplexer for the AI era. Claude Code and other ACP ([Agent Client Protocol](https://agentclientprotocol.com)) agents run inside: colored threads with destination chips (thread → project ⎇ branch), diff review, checkpoints/rewind, an always-visible token meter, and a fleet view for conducting parallel agents. It talks to the `claude` CLI, so the subscription you already pay for just works — no API key, no walled-garden AI billing.

Rust/GPUI notes, probably the interesting part for this sub:

- GPUI outside the Zed tree is very usable, but the API churns — I pin a rev and treat `gpui/examples` as the only reliable documentation.
- No code is ported from Zed or any GPL editor — techniques studied, implementations my own; `cargo-deny` audits the dependency tree in CI.
- The rest is battle-tested Rust: ropey, tree-sitter, alacritty_terminal, imara-diff, a hand-rolled LSP client, the `agent-client-protocol` crate, Turso.
- Dev-machine numbers: cold start ~215 ms, idle RSS ~120 MB, core editing ops ~1 µs — with a perf budget that fails CI when exceeded.
- The editor core is fuzzed against a reference implementation; the first run caught a real multi-cursor × multibyte bug.

Fittingly, necoder is largely built *by* the agents it hosts — under guardrails (fuzz tests, CI perf budgets, license audits), with design and review on me.

Honest limitations: no Linux build yet (GPUI supports it — it just hasn't been my itch), macOS is Apple Silicon only for now, and it's two months old, so expect rough edges. No telemetry, ever.

Repo: https://github.com/iKora128/necoder · Site: https://necoder.com/en/

If this UI philosophy clicks with you, feedback and contributions are very welcome.

## 日本語版本文

Cursorのウィンドウを20個並べて開発していたら限界が来たので、この2ヶ月、コードエディタを自作していました。necoder（ねこーだー）といいます。RustとGPUI（ZedのUIフレームワーク。単体のgit依存として使用）によるスクラッチ実装で、VSCodeのフォークではありません。AGPL-3.0、macOSとWindowsで動きます。今は開発の半分以上がこのエディタです。

原動力は怒り駆動開発です。Electronのメモリバカ食い、1ウィンドウで複数プロジェクトを捌けない構造、自分がどのAIエージェントに指示しているのか分からなくなる混乱。Claude Code拡張の使い勝手だけは手放したくなかったので、まずZedへの乗り換えを試しましたが、Zedの拡張はUIを描けず、スレッドやプロジェクトの色分けは構造的に不可能でした。無いなら作るしかない。

設計ルールは3つだけ:

- 並列作業は横へ（タブ）、並行作業は縦へ（Slack風のプロジェクト切り替え）
- 色は方向感覚。プロジェクトとエージェントごとの識別色がレール、タブの下線、キャレットまで貫通する。昨今のエディタはこの空間感覚を失っていて、ユーザーは空間識失調に陥り、違うAIに誤爆する
- エディタはAI時代のマルチプレクサ。Claude CodeなどのACP（[Agent Client Protocol](https://agentclientprotocol.com)）対応エージェントがエディタ内で動き、色付きスレッドと宛先チップ（スレッド → プロジェクト ⎇ ブランチ）、diffレビュー、チェックポイントと巻き戻し、常時表示のトークンメーター、並列エージェントの編隊ビューを備えます。`claude` CLIと話すので手持ちのサブスクがそのまま使え、APIキーも囲い込み課金も不要

RustとGPUIの話:

- GPUIはZedの外でも十分使えます。ただAPIの変化が速いので、revを固定して`gpui/examples`だけを信じて書いています。ネットの記事はだいたい古い
- ZedやGPLエディタからのコード移植はゼロ。手法は読んで学び、実装は自作。依存ツリーはCIでcargo-denyに監査させています
- 残りは枯れたRust部品の組み合わせ。ropey、tree-sitter、alacritty_terminal、imara-diff、手書きのLSPクライアント、agent-client-protocol crate、Turso
- 開発機の実測はコールドスタート約215ms、idle RSS約120MB、編集コアの操作約1µs。性能予算を超えるとCIが落ちます
- 編集コアは参照実装と突き合わせるファズテストを回していて、初回実行でマルチカーソル×マルチバイトの実バグを捕まえました

そしてnecoderの大部分は、necoderが載せているエージェント自身が書いています。ファズテスト、CIの性能予算、ライセンス監査というガードの内側で。設計とレビューは自分の仕事です。

正直な制約: Linuxビルドはまだありません（GPUIは対応していて、単に自分が使わないので後回し）。macOSは今のところApple Siliconのみ。生まれて2ヶ月なので粗はあります。テレメトリは一切なし。

リポジトリ: https://github.com/iKora128/necoder
サイト: https://necoder.com/

「このUI哲学、わかる！」という方、フィードバックとコントリビュートを待っています。

## 画像（必須）

本文中にインライン画像を 2〜3 枚まで:

1. Cursor ウィンドウ 20 個のスクショ（Zenn 記事用のあの画像）— 冒頭の一文の直後。ツカミとして最強
2. `lp/assets/img/hero.png`（色分けされた全体像）
3. `lp/assets/gif/stream-en.gif`（ACP ストリーミング）— 動くものが一番強い

`lp/assets/img/fleet.png`（編隊ビュー）は 4 枚目候補。入れるならどれか 1 枚と交換。

注意点: 外部リンクではなく **Reddit に直接アップロード**（インラインプレビューが出る方が読まれる）。UI は**英語ロケール**で撮る。GIF は英語版をそのまま使える。枚数を増やしすぎると逆に読み飛ばされる。

## 想定問答（コメント欄用）

- **「なぜ Zed に貢献しなかった?」** → 本文の Zed 段落が答え。深掘りされたら BACKGROUND.md へ誘導。Per-window テーマの PR が塩漬けである事実も補強材料
- **「Linux は?」** → 「GPUI は対応してるので技術的障壁はない。自分が使わないので後回しにしてる。要望が多ければ上げる」— 正直が一番
- **「AI が書いたコードの品質は?」** → 「だから機械のガードを厚くしてる: 参照実装と突合するファズ、CI の性能予算、cargo-deny。実際ファズが実バグを見つけた」— 具体例で返す
- **「AGPL + CLA って将来クローズドにする気?」** → 「公開済みコードは永久に AGPL。CLA は将来の選択肢（デュアルライセンス等）を残すためで、CONTRIBUTING に明記してある」— ここで誤魔化すと荒れる
- **「SSH/Remote は?」**（README・サイトには載っているため聞かれうる）→ 「works but experimental — that's why it's not in the post」と正直に返す
- **「GPUI 使ってみてどうだった?」** → ご褒美質問。rev 固定運用・examples が正・API churn の実体験を語れば伸びる

## 投稿前チェック

- [ ] r/rust の自己宣伝ルール（サイドバー）を確認
- [ ] README の SSH 節に experimental の一言を足すか検討（投稿から飛んだ人向け）
