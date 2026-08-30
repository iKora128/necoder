# r/macapps 用アナウンス文（2026-08-30）

r/rust は AI 生成コンテンツのゲートで断念（経緯は JOURNAL 参照）。r/macapps は「投稿するな」ではなく
「開示せよ + 信頼を積め」の文化なので、AI 製であることを正直に書いたまま出せる。

## ルールの要点（投稿前に必ずサブレで最新を確認）

- 自己宣伝は **30日に1回まで**（1開発者あたり）
- メインフィードの告知は trust/transparency 要件を満たした開発者のみ + **PCP テンプレート**必須（ピン留め or wiki にある。ここは外から取得できなかったので投稿前に必読）
- 満たさない場合は**月次 Megathread** のコメントに書く
- **サブレ内 karma 10** が必要（アプリに言及する前提条件）
- オープンソースアプリはタイトルに **[OS]**
- リンクは公式配布元のみ（GitHub リリース / 公式サイト）。短縮 URL・アフィリエイト禁止
- コメントでも開発者であることを開示する

### アカウント運用の注意（重要）

新アカ（Unique-Passenger-455）と古いアカの**両方から同じアプリを宣伝しない**こと。
同一アプリ×複数アカウントは、見つかると astroturfing 扱いで両方 ban になりうる。
r/macapps は**古いアカウントに一本化**するのが安全:

1. まず古いアカで**今月の Megathread** に下の短文を投稿
2. 並行して普通のコメント参加でサブレ内 karma 10 を貯める
3. 30日以上あけて、PCP テンプレートに沿った**メインフィード投稿**（下の長文を流用）

## (1) Megathread（The App Pile）用・PCP 形式

書式はスレ冒頭で指定されている: **App名 [スクショ推奨] / Problem / Comparison / Pricing+リンク**。
「短いほどよい」。**em ダッシュ等の AI 痕跡があると自動削除されやすい**と明記されているので、
ダッシュ・過剰な箇条書き・絵文字は使わない。以下は em ダッシュゼロで書いてある。

> **necoder: an open source, native code editor for running many projects and AI agents in one window**
>
> Problem: I used to develop with 10 to 20 Cursor windows open, one per project, and kept losing track of which window I was in and which AI agent I was prompting. necoder does Slack-style project switching in a single window, and every project and agent gets an identity color that runs through the rail, tabs and caret, so you always know where you are. Claude Code runs inside (via ACP) using the claude CLI, so your existing subscription just works, no API key.
>
> Comparison: Cursor and VSCode are Electron and one window per project. necoder is written from scratch in Rust on GPUI, the UI framework behind Zed, cold starts in about 215 ms and idles around 120 MB RAM on my machine. Zed itself is fast too, but its extensions can't draw UI, so per-project and per-agent colors are impossible there. That gap is the reason this app exists.
>
> Pricing: Free, open source (AGPL-3.0). No account, no telemetry. macOS Apple Silicon only for now.
>
> https://github.com/iKora128/necoder

スクショ: `lp/assets/img/hero.png` をコメントの画像ボタンで添付（色分けが一目で伝わる1枚）。

### 投稿前の罠（スレ冒頭の WARNING より）

- **メールアドレス未認証 + 初コメントにリンク**の組み合わせは 90% 自動削除される。投稿アカウントのメール認証を先に確認
- サブレ内 karma 10 を貯めてから、が安全（宣伝抜き・リンク抜きの普通のコメントで）
- AI 痕跡（em ダッシュ等）で削除確率が上がる。**この文面を AI に再度磨かせないこと**
- 削除されたら消さずに modmail

## (2) メインフィード用（PCP テンプレートに合わせて再構成すること）

タイトル案:

> **[OS] necoder — a native Rust code editor for juggling many projects and AI agents in one window (free, AGPL-3.0)**

本文案（テンプレートの項目に切り貼りする素材として）:

> I used to develop with 10–20 Cursor windows open at once. That stopped scaling, so I built my own editor: **necoder** — written from scratch in Rust on **GPUI**, the UI framework behind Zed. Not a VSCode fork, not Electron. I'm the developer.
>
> **What it does**
>
> - One window for everything: Slack-style switching between projects (and branches), parallel tasks as tabs
> - Color as a sense of direction: every project and AI agent carries an identity color through the rail, tab underlines and caret. You always know where you are and which agent you're about to prompt
> - AI agents live inside the editor: Claude Code and other ACP (Agent Client Protocol) agents run as colored threads with destination chips (thread → project ⎇ branch), diff review, checkpoints/rewind, an always-visible token meter, and a fleet view for parallel agents
> - It talks to the `claude` CLI, so the subscription you already pay for just works — no API key, no separate AI billing
> - Native performance: cold start ~215 ms, idle RAM ~120 MB on my machine — it's what a code editor feels like without Electron
>
> **Price & privacy**: free, open source (AGPL-3.0), no telemetry, no account, download from GitHub releases.
>
> **Honest notes**: macOS 14+ Apple Silicon only for now (Intel and Linux later). It's 2 months old, so expect rough edges. And in the spirit of transparency: necoder is largely built by the AI agents it hosts — under guardrails (fuzz testing against a reference implementation, CI perf budgets that fail the build, license audits), with design and review on me.
>
> GitHub: https://github.com/iKora128/necoder · Site: https://necoder.com/en/

画像: `lp/assets/img/hero.png`（全体像）+ `lp/assets/gif/stream-en.gif`（ACP ストリーミング）。
Megathread はコメントなので画像は貼れない場合が多い — その場合はリンクのみでよい。

## 投稿前チェック

- [ ] ピン留めの PCP テンプレート投稿を読み、本文をその項目立てに組み替えた
- [ ] 古いアカウントの r/macapps 内 karma が 10 以上ある（なければ先に Megathread + コメント参加）
- [ ] 直近30日以内に necoder の宣伝投稿をしていない
- [ ] リンクが GitHub / necoder.com のみ（短縮 URL なし）
