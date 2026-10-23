# r/ClaudeCode・r/ClaudeAI 用アナウンス文（2026-10-20）

r/rust は AI 生成コンテンツとして叩かれて断念した。Claude Code で作ったこと自体が話題になる場所なので、
AI 製であることは隠さず最初に書く。入口は技術ではなく痛み（宛先の取り違え）。
中身は LP（necoder.com/en/）の英語文面に合わせた。ただし LP の機能を全部は並べない。並べると宣伝文に見える。

## 許される出し方（2026-10-20 時点・又聞き）

Reddit のルールページはこちらから読めなかった。別の開発者の宣伝記録（https://github.com/LeandroCannizzaro/LiveClaude/issues/8 ・2026-10-11）によると:

- r/ClaudeCode には毎週「Weekly showcase thread: What are you building with…」が立つ
- r/ClaudeAI には「Show us what you've created with Claude」のまとめスレッドがある
- 単純な作品紹介は、新規投稿ではなくこのまとめスレッドに書くのがルール上の推奨。その人も新規投稿は控えた

なので**まず両方のまとめスレッドにコメントで出す**。新規投稿は、サイドバーで許されていると確認できたときだけにする。

## 出す順番

1. **r/ClaudeCode の今週のショーケーススレッド**に「まとめスレッド用」を書く
2. **r/ClaudeAI のまとめスレッド**には数日あけて書く。同じ文面を同日に両方へ出すと連投に見える
3. 反応が良く、ルールが新規投稿を許しているなら長い版を新規投稿にする（r/ClaudeCode は痛みの版、r/ClaudeAI は「Built with Claude」フレアで作り方の版）
4. どれも**同じアカウント 1 つ**から。複数アカウントで同じアプリを出すと両方 ban になりうる（r/macapps 下書きの注意と同じ）

## 投稿前チェック

- [ ] サイドバーのルールと自己宣伝の条件を読む（外から取得できなかったので未確認）
- [ ] 冒頭の体験談を自分の実話に合わせる
- [ ] 画像は Reddit に直接アップロードする。動きなら `lp/assets/gif/stream-en.gif`、全体像なら `lp/assets/img/hero.png`、Fleet なら `lp/assets/img/fleet.png`
- [ ] LP の「lightest」「buttery」のような最上級や形容詞は Reddit の本文に持ち込まない。突っ込まれる
- [ ] 本文に em ダッシュ・太字ラベル・三点セットが紛れていないか見る。自分で手を入れて自分の声にする
- [ ] 投稿後の数時間はコメントに張り付いて返す

## 本番用（2026-10-20・本人が書いた版）

本人が最初から書き直した文面。これを正とする。下の版はすべて没（Claude が書いた版は整いすぎて「全体的に駄目」と本人判断）。
英訳は本人の勢いを残し、Electron の一文だけ足した。「VS Code より良い」はコメントで「どこが？」と聞かれるので、軽さとスマホの遠隔モードで返す。

タイトル（新規投稿のときだけ要る。ショーケーススレッドのコメントには不要）:

- 本人指定で「my best editor」を入れる（2026-10-20）。本命: `Cursor window hell was killing my Mac, so I built my best editor yet in Rust (runs Claude Code & Codex on your existing sub)`
- 「my best editor」案 2: `My best editor yet: Rust, no Electron, and Claude Code & Codex run on the sub you already pay for`
- 「my best editor」案 3: `Fed up with Cursor eating my RAM, I built my best editor yet in Rust`
- 旧本命: `Cursor window hell was killing my Mac, so I built my own editor in Rust (runs Claude Code & Codex on your existing sub)`
- 短く: `Cursor was eating all my RAM, so I built my own editor in Rust`
- 問いかけ: `What editor do you run Claude Code in? I got fed up with Cursor and built my own`
- 得を先に: `A lightweight Rust editor that runs Claude Code and Codex on the subscription you already pay for`
- 日本語（X など）: `Cursor のウィンドウ地獄で Mac が死にそうだったので、Rust でエディタを自作した（Claude Code と Codex が手持ちのサブスクのまま動く）`

画像（2026-10-20 に英語で撮り直し・リポジトリには入れていない）: `/tmp/necoder-shots/necoder-en-demo.gif`（約 12 秒・2 倍速）・`necoder-en-demo.mp4`（同じ中身。Reddit は動画の方が軽く再生される）・`necoder-en-still.png`（静止画）。レールに ripgrep / axum / fastapi / vite、Claude Code が英語で答える。lp/assets の既存素材は stream-en.gif 以外は日本語が写っている。

```
Hey all, what editor are you using these days? Cursor? VS Code? Or just Claude Code straight from the terminal?

Cursor eats so much memory that it got too heavy for me to get real work done. I work on a lot of projects in parallel, so I was stuck in window hell. And because it's Electron, every window is basically running another browser underneath. It adds up fast.

So I built my own editor in Rust. Super lightweight but powerful, and no Electron at all. It feels a lot like VS Code, just better. SSH is light too, and there's a built-in remote mode so you can use it from your phone.

And of course, it works with the Claude Code or Codex subscription you already have!

Give it a try, I think you're gonna love it!

https://necoder.com/en/
```

日本語:

```
みんな！エディタ何使ってる？Cursor？VS Code？それとも Claude Code をターミナルから直接使ってる？

Cursor がメモリをたくさん食って、重すぎて仕事がまともに進まなくなった。自分はたくさんのプロジェクトを並行してやってるから、ウィンドウ地獄なんだよね。しかも Electron だから、ウィンドウ 1 枚ごとに裏でブラウザが 1 個動いてるようなもの。そりゃ重くなる。

ということで、Rust でめっちゃ軽量でパワフルなエディタを作ったよ。Electron は一切なし。使い心地は VS Code に近いけど、それよりもっといいやつ。あと SSH が軽かったり、標準でスマホから使える遠隔モードにも対応してる。

もちろん、自分が使ってる Claude Code や Codex のサブスクがそのまま使えるよ！

ぜひ試してみてね！きっと最高の開発体験になるはずだから！

https://necoder.com/
```

## 没: まとめスレッド用（短い版・2026-10-20 改: 軽さとサブスクを主役に）

本人の判断で、主役は「Rust で軽くて速い」「自分の AI サブスクがすぐ使える」。色は最後に一言だけ残す（「なぜ Zed じゃない？」への答えになるので消さない）。
速さは形容詞ではなく実測値で言う。「blazingly fast」は Rust 界隈でネタにされる言い回しなので使わない。

```
Made a code editor in Rust that runs Claude Code and Codex on the subscription you already have.

I was living in 10 to 20 Cursor windows with Claude Code in each, and my Mac was not happy about it. So I wrote my own. No Electron, drawn straight on the GPU. On my machine it opens in about 0.2 seconds and sits around 120MB of memory when idle.

Claude Code and Codex run right inside it on the plan you already pay for. No API key, no extra AI subscription. Turning one on is a button and a browser sign-in.

Each project also gets its own color, which started as my personal obsession and turned out to be really handy with a lot of sessions open.

Free, open source (AGPL), Mac and Windows. And yes, Claude Code wrote most of it.

https://necoder.com/en/

What editor are you all running Claude Code in these days?
```

日本語版（X・日本語圏向け）:

```
Rust でコードエディタを作りました。Claude Code と Codex が、いま契約しているサブスクのまま中で動きます。

Cursor のウィンドウを 10〜20 個開いて、それぞれで Claude Code を回していたら Mac が悲鳴を上げていたので、自分で作ることにしました。Electron なしで GPU に直接描いているので、手元の Mac だと起動は 0.2 秒くらい、何もしていないときのメモリは 120MB くらいです。

Claude Code と Codex は、いつものプランのまま中で動きます。API キーも、追加の AI 課金もいりません。ボタンを押してブラウザでサインインするだけです。

あと、プロジェクトごとに色が付きます。これは完全に自分の趣味で入れたんですが、セッションをたくさん開いていると地味に助かります。

無料のオープンソースで、Mac と Windows で動きます。ちなみにコードはほとんど Claude Code が書きました。

https://necoder.com/

みなさん、Claude Code はいまどのエディタで使ってます？
```

## r/ClaudeCode 新規投稿用（痛みの版）

注意: 2026-10-20 に主役を「軽さとサブスク」へ変えた。新規投稿にするときは、上の短い版を膨らませる方を優先する。この版は色を主役にしていた頃のもの。

タイトル:

> I kept sending prompts to the wrong Claude Code session, so I gave every project a color

本文:

```
I used to have 10 to 20 editor windows open, each with Claude Code running in a different repo or branch. They all look the same. More than once I sent a long prompt to the wrong one and spent the afternoon cleaning up.

So I built necoder. Each project gets a color, and it follows the project everywhere: the sidebar, the tab, the cursor and the Claude thread itself. Every thread shows which repo and branch it's talking to, and how many tokens it has used, so I find out about compaction before it happens instead of after.

[GIF]

Claude Code and Codex run inside it over ACP, on the plan you already pay for. No API key. Turning one on is a button and a browser sign-in, no terminal needed. You review each diff before it lands and can rewind if you don't like it.

Two things I added later because I needed them. Fleet gives each task its own worktree and runs several agents side by side, with a "Captain" agent that splits the work and brings it back for review. And when I'm away from my desk, I scan a QR code and answer approvals from my phone. The connection is end-to-end encrypted.

Full disclosure: I'm the dev, and most of the code was written with Claude Code. It's a native Rust app, no Electron. Free, open source (AGPL), no telemetry, macOS (Apple Silicon) and Windows.

https://necoder.com/en/
https://github.com/iKora128/necoder

How do you keep track of which session is which? Tmux names, separate windows, just being careful?
```

## r/ClaudeAI 新規投稿用（Built with Claude・作り方の版）

切り口は「Claude Code でエディタを丸ごと作ったとき、どう壊れないようにしたか」。機能の話は短くする。

タイトル:

> I built a whole code editor mostly with Claude Code. Here's what kept it from falling apart

本文:

```
For the last few months I've been building necoder, a native code editor for people who run a lot of Claude Code sessions at once. Each project gets a color that follows it into the Claude threads, so you can tell at a glance which repo and branch a session is working on. Claude Code and Codex run inside it on your existing plan.

[GIF]

Most of the code was written by Claude Code, and for a while now it's been written inside necoder itself. What made that work was putting checks around it that don't depend on me reading every line. The editing core is fuzz tested against a simple reference implementation, and the first run found a real bug with multiple cursors and multibyte text. CI fails if startup time or memory goes over budget. A license check runs on every dependency.

My job ended up being design and review. Tests catch logic bugs well, but UI problems mostly showed up only when I actually used it every day.

Free, open source (AGPL), no telemetry, macOS (Apple Silicon) and Windows.

https://necoder.com/en/

Happy to answer questions about the workflow.
```

注意: 最後から 2 段落目は実体験に合わせて書き換えるか消す。盛らない。

## 想定問答

- **「サブスクで使っていいの？規約は？」** → claude CLI を ACP で呼んでいるだけ。Anthropic のヘルプでは Agent SDK・`claude -p`・サードパーティアプリは今もプランの利用枠から引かれる（6/15 の分離は停止中・2026-10-20 確認）。https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan 。将来変わりうるので断言しすぎない
- **「スマホ連携はコードがあなたのサーバーを通るの？」** → 中継は Cloudflare 上の relay を通るが、中身は E2E 暗号化。鍵は Mac が作り、QR の `#` 以降にしか載らないので relay からは読めない。独立した監査は受けていない、と正直に言う（relay/README.md）
- **「No WebView って書いてあるけど HTML プレビューは？」** → LP は「no WebView」だが、HTML プレビューだけは OS の WebView を開いたときに作る。突っ込まれたらそのまま認める（README の記述が正確）
- **「Claude Desktop や `claude agents` ではダメ？」** → エディタも Git もターミナルも同じ画面にあって、全部が同じ色で繋がっているのが違い
- **「Orca / herdr / cmux と何が違う？」** → あちらはエージェントの置き場を管理する道具。necoder はエディタ本体に色と宛先を組み込んでいる。比べるより「両方使ってみて」でよい
- **「なぜ Zed じゃない？」**（軽さとサブスクを主役にすると一番来る。Zed も Rust 製で速く、ACP で Claude Code をサブスクのまま動かせる）→ Zed の拡張は UI を描けないので、スレッドやプロジェクトの色分けが作れなかった。描画は Zed の GPUI を使っている。Fleet・スマホ承認・ボタン一つの導入も違いとして挙げられる
- **「0.2 秒って何と比べて？」** → 自分の Mac での実測（`scripts/startup-time.sh`）。Cursor や VS Code と同じ条件で測った数字は持っていないので、比較の数字は言わない
- **「Linux は？」** → まだ無い。GPUI は対応しているので要望が多ければ出す
- **「vibe coded でしょ？」** → 否定しない。コードの大半は Claude Code が書いた。その代わりファズ・CI の性能予算・ライセンス監査で守っている、と具体で返す
- **「Nyaco って何？」** → AI の状態（考え中・書いている・終わった・待機）を猫の様子で見せるもの。本文には入れず、聞かれたら画像で返すと受ける
