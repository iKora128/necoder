# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/) · [Semantic Versioning](https://semver.org/). Each version is written in English first, followed by Japanese (### 日本語).
各版は英語のあとに日本語（### 日本語）を続けて書きます。リリースの手順は [`docs/RELEASE.md`](docs/RELEASE.md) にあります（タグを push すると、CI が署名済みの .dmg を Releases に添付します）。

## [Unreleased]

## [0.1.23] - 2026-10-01

> **This is a test release, like v0.1.21 and v0.1.22.** It contains everything in v0.1.22, and some of that has not been checked on a real machine yet (`docs/ORCA-PARITY-CHECKLIST.md`). It is not delivered through auto-update. If you would like to try it, install it manually from this Release page. Bug reports in an Issue are very welcome.

This release rebuilds the phone screen (remote control, served from control.necoder.com) so you can follow your agents from your phone. It opens on a list of every shared thread, and a conversation reads like a chat: replies are formatted, and the agent's tool calls are folded into a short summary. The Mac app now sends each thread's and project's color, so the phone uses the same colors as your desktop.

### Added

- **Phone: thread list**: The phone opens on every shared thread, grouped by project. Threads waiting for your approval or answer come first under "Needs you", and projects with running threads come next. Each row shows the thread's state by shape and motion, its agent, its tokens and the latest reply.
- **Phone: readable conversations**: Replies are formatted as Markdown (headings, lists, tables, code). Tool calls in a row are folded into one box that says what the agent did, such as "12 steps · Read 3 · Search 4 · Run 1". While a turn runs, the box shows the command running now. Open the box to see each step, and a step to see its output.
- **Phone: thread and project colors**: The top bar of a conversation takes the thread's color, and each project in the list gets a bar in its color, the same colors as on your Mac. This needs this version of necoder on the Mac. While the phone is connected to an older version, it stays neutral.
- **Phone: new thread**: ＋ next to a project starts a new thread with the agent you pick.

### Changed

- **Phone: conversation order**: Messages now read from old to new, with the latest just above the message box, which stays at the bottom of the screen. A conversation opens at the latest message, and new messages do not move the part you are reading. This replaces the newest-first order from v0.1.21 (#36).
- **Phone: questions and approvals**: They appear at the end of the conversation, right above the message box. Options are radio buttons or checkboxes.
- **Phone: settings**: The PC, reconnect, notifications, language and "remove this connection" moved into a settings sheet at the top right of the list.
- The phone reopens the conversation you were reading when you come back to the app.

### Fixed

- **Phone features from v0.1.21**: Typing your own answer to an agent's question and the "finished" notice were in v0.1.21, but control.necoder.com still served an older phone screen, so they did not work. They work now.

### 日本語

> **v0.1.21・v0.1.22 と同じくテストリリースです。** v0.1.22 の変更をすべて含み、その中にはまだ実機で確かめていない所があります（`docs/ORCA-PARITY-CHECKLIST.md`）。自動更新では配信しません。試してくださる方は、この Release ページから手動で入れてください。不具合を見つけたら Issue で教えてもらえると助かります。

スマホの画面（リモート管制・control.necoder.com）を、手元でエージェントを追えるように作り直した版です。開くと共有中のスレッドが一覧で並び、会話はチャットのように読めます。本文は整えて表示し、ツールの実行は短くまとめて畳みます。Mac の necoder がスレッドとプロジェクトの色を送るようになり、スマホでもデスクトップと同じ色で見分けられます。

#### 追加

- **スマホ：スレッド一覧**：開くと、共有中のスレッドがプロジェクトごとに並びます。承認や質問を待っているスレッドは「要対応」として先頭に、実行中のスレッドがあるプロジェクトはその次に並びます。各行には、状態（形と動きで表示）・エージェント・トークン・最後の返事を出します。
- **スマホ：読みやすい会話**：返事は Markdown（見出し・箇条書き・表・コード）として整えて出します。続けて走ったツール実行は 1 つの箱に畳み、「12 ステップ　読む 3・検索 4・実行 1」のように何をしたかを出します。実行中は、今動いているコマンドを出します。箱を開くと 1 件ずつ、さらに開くと出力が見られます。
- **スマホ：スレッドとプロジェクトの色**：会話画面の上端バーがスレッドの色になり、一覧ではプロジェクトごとにその色の縦線が付きます。Mac と同じ色です。この版の necoder が Mac に入っている必要があります。古い版に繋がっている間は、色の付かない中立色で出ます。
- **スマホ：新しいスレッド**：一覧のプロジェクトの ＋ から、エージェントを選んで新しいスレッドを始められます。

#### 変更

- **スマホ：会話の並び**：古い順に並び、最新は画面下に固定した入力欄のすぐ上に来ます。開くと最新の位置から始まり、遡って読んでいる間に新しい返事が届いても、読んでいる所は動きません。v0.1.21 の新しい順の並び（#36）はやめました。
- **スマホ：質問と承認**：会話の最後、入力欄のすぐ上に出します。質問の選択肢はラジオボタンかチェックボックスです。
- **スマホ：設定**：PC の切り替え・再接続・通知・言語・接続の削除を、一覧の右上から開く設定シートに移しました。
- アプリを開き直すと、読んでいた会話に戻ります。

#### 修正

- **v0.1.21 のスマホ側の機能**：質問に自分の言葉で答える欄と「終わった」通知は v0.1.21 に入っていましたが、control.necoder.com が古い画面のままだったため使えていませんでした。今回から使えます。

## [0.1.22] - 2026-10-01

> **This is a test release, like v0.1.21.** It contains everything in v0.1.21, and some of that has not been checked on a real machine yet (`docs/ORCA-PARITY-CHECKLIST.md`). It is not delivered through auto-update. If you would like to try it, install it manually from this Release page. Bug reports in an Issue are very welcome.

This release adds `@` mentions to the agent composer, fixes attachments whose path contains a space, and makes the explorer behave more like Finder: a double-click opens a file in its default app, and ⌥⌘C copies the selected item's path.

### Added

- **`@` mentions in the agent composer**: Type `@` and part of a file name, and the project's files are listed right above the composer. Enter or Tab puts `@path` into your message, for that message only. The list uses the same matching and order as ＋ context. It also works right after Japanese text (a space is inserted so the agent reads it) and with `＠` typed in Japanese input mode. Esc closes the list without stopping the turn. An email address like `a@b` does not open it.
- **Explorer: double-click opens the default app**: Double-click a file in the explorer to open it in its default app, like in Finder. A single click still opens a preview tab. For files on an SSH host, a double-click keeps the tab open as before.
- **Explorer: ⌥⌘C copies the selected item's path**: With the explorer focused, or right after you click a file in it, ⌥⌘C copies the full path of the selected item and shows a toast. While you work in the editor, ⌥⌘C still copies `path:line`.

### Changed

- A double-click in the explorer no longer keeps a preview tab open. Keep a preview tab by editing it, double-clicking its tab, reopening it with ⌘P, or pinning it.
- The tooltip of a ＋ context chip now says that the file is sent with every message until you remove it.

### Fixed

- **Attachments with a space in the path**: Claude Code did not read files attached with ＋ context when their path contained a space (or did not end in a letter or digit), because it cut the `@path` reference at the space. necoder now sends such paths as `@"path"`.
- **＋ context**: Enter attached the first match in file-list order instead of the first row shown. It now attaches the row you see at the top.

### 日本語

> **v0.1.21 と同じくテストリリースです。** v0.1.21 の変更をすべて含み、その中にはまだ実機で確かめていない所があります（`docs/ORCA-PARITY-CHECKLIST.md`）。自動更新では配信しません。試してくださる方は、この Release ページから手動で入れてください。不具合を見つけたら Issue で教えてもらえると助かります。

エージェントの入力欄に `@` メンションを入れ、空白を含むパスの添付が届いていなかった不具合を直した版です。エクスプローラは Finder に近づけました。ダブルクリックで既定のアプリが開き、⌥⌘C で選んでいる項目のパスをコピーできます。

#### 追加

- **エージェントの入力欄の `@` メンション**：`@` に続けてファイル名の一部を打つと、入力欄のすぐ上にプロジェクトのファイルが並びます。Enter か Tab で本文に `@パス` が入り、その 1 通だけに付きます。候補の絞り方と並びは ＋ context と同じです。日本語のすぐ後ろでも使えます（エージェントが読めるよう空白を挟みます）。日本語入力のまま打った `＠` でも出ます。Esc は候補だけを閉じ、実行中のターンは止めません。`a@b` のようなメールアドレスでは出ません。
- **エクスプローラ：ダブルクリックで既定のアプリ**：エクスプローラでファイルをダブルクリックすると、Finder と同じく既定のアプリで開きます。1 回クリックは今までどおりプレビュータブで開きます。SSH 先のファイルは、ダブルクリックで今までどおり普通のタブになります。
- **エクスプローラ：⌥⌘C で選んでいる項目のパス**：エクスプローラを押した状態と、エクスプローラでファイルを押した直後は、⌥⌘C で選んでいる項目のフルパスをコピーし、トーストで知らせます。エディタで作業している時の ⌥⌘C は、今までどおり `パス:行` をコピーします。

#### 変更

- エクスプローラのダブルクリックでは、プレビュータブを普通のタブにしなくなりました。普通のタブにするには、編集する・タブをダブルクリックする・⌘P で開き直す・ピン留めする、のどれかを使います。
- ＋ context のチップのツールチップで、外すまで送るたびに毎回付くことを伝えるようにしました。

#### 修正

- **空白を含むパスの添付**：＋ context で付けたファイルのパスに空白が入っている（または英数字で終わらない）と、Claude Code がファイルを読んでいませんでした。Claude Code が `@パス` を空白の手前で切るためです。こうしたパスは `@"パス"` と囲んで送るようにしました。
- **＋ context**：Enter で付くのが、表示の先頭ではなく一覧の並びで最初に当たったファイルでした。表示の先頭を付けるようにしました。

## [0.1.21] - 2026-09-30

> **This is a test release.** It is a large release with more than 300 changes since v0.1.20, and some of them have not been checked on a real machine yet (`docs/ORCA-PARITY-CHECKLIST.md`). It is not delivered through auto-update. If you would like to try it, install it manually from this Release page. Bug reports in an Issue are very welcome.

This release brings in the work to close the feature gap with Orca (#21). It adds change review, a Web tab for localhost, many terminal improvements, usage display, Fleet Task features, session history and full-text search, easier SSH, and adding and switching agents. In the agent panel, a leading `!` runs a shell command.

### Added

- **`!` in the agent panel**: Start a message with `!` to run it in the shell instead of sending it to the agent, like bash mode in the Claude Code CLI. The command runs in the thread's working directory (on the remote side for an SSH project, in the chat's folder for Chat). Output streams into the transcript and is passed to the agent along with the next message you send. While you type a `!` command, the input turns blue and shows where it will run. Stop it with the row's Stop button or Esc; this also stops any child processes the command started. A `!` sent from the phone or by an AI is not run; it arrives as plain text (#37).
- **Change review**: Read all of a worktree's changes on one screen. Open it from the editor, Source Control or Fleet. Add notes to lines and send them as one prompt to a Task or thread. Viewed marks and notes are saved and tied to the code they were made on.
- **Agent composer**: `/` completes slash commands and the repository's prompt recipes. The agent's goal shows above the composer, and you can pause, resume or clear it. A message rail next to the transcript lets you jump between prompts. You can answer an agent's question card in your own words, from the phone too. A subagent's steps fold under the Task tool call that started them.
- **Web tab for localhost**: Open localhost URLs in a Web tab with a preview toolbar. In Design mode, pick an element on the page and attach a cropped image of it to a thread. Web Inspector opens in a separate window. HTML files open through a built-in server. localhost links on an SSH host are forwarded before they open.
- **Terminal**: Adds xterm and kitty key encodings, mouse and focus reporting, text styles, cursor shapes, clickable URLs, titles, OSC 52, the bell, file drops, a context menu, and find in terminal. Also new: horizontal split (⌘\), opening a terminal as a tab in the editor area, a floating terminal you can call up from anywhere (⌃`), renaming tabs, running saved commands (▶), settings for font, scrollback, cursor and shell, importing color schemes from Ghostty, Windows Terminal, iTerm2 and Warp, and detecting a running command through OSC 133. necoder asks before you close a tab with a running command, and before you quit while agents or terminals are running.
- **Usage and statistics**: necoder records the cost, rate limits and per-turn tokens that ACP reports. See them in the status bar chip, its popover, and the statistics screen. Rate limits are tracked separately per agent, host and credentials.
- **Fleet and Task**: + Task lets you pick the branch name and the starting point. New Tasks can copy the files listed in `.worktreeinclude`, share dependency folders through links, and check out only the folders you need. The setup script can be skipped, or retried after it fails. You can adopt worktrees created outside necoder. The sidebar can filter, sort, multi-select, and clean up several Tasks at once (⌘⌫). Tasks that are still being created are shown and can be canceled. Tasks can nest, there is a "Details…" item, and when a conflict stops integration you can have the Task's agent resolve it. You can send one request to several agents as parallel Tasks and compare the results. The Captain adds a recommendation to waiting approvals, shows ledger notices as grey cards apart from human turns, and shows its token count.
- **Notifications**: When you are not looking at the necoder window, it notifies you when an agent asks a question, finishes a turn, or is waiting. The bell in the title bar keeps a history, and the sound volume is configurable. When a turn ends in a thread you are watching, the phone gets a notice too. The computer stays awake while agents are working.
- **Session history and full-text search**: Search the turns of every thread, and list and open an agent's past sessions. The thread history picker is rebuilt with a sectioned list, agent sessions and full-text search.
- **Agents**: Add any agent from the ACP registry in Settings, including downloading and verifying binaries and launching through uvx. Agents you add in settings.json show up in the list too. In Settings you can enable or disable agents, set the default permission mode for all of them at once, and override launch settings. You can switch the account Claude Code and Codex are logged in with. Start a new thread with a chosen agent from the palette or a key binding.
- **Skills**: Install the necoder skill from Settings, or use the CLI (`necoder skills get / install / list`).
- **CLI (`ne`)**: Control Tasks, terminals and diffs in the running app from the CLI. Typing into terminals is off by default.
- **Editor**: Adds auto save (on focus change or after a pause), pinned tabs, preview tabs that open on a single click, closing all tabs, copying `path:line`, opening in another editor, and dragging to place beside the editor. Markdown gets clickable links, front matter properties, `[toc]`, a side-by-side live preview (⌘K V), a `/` menu at the start of a line, and image drops from Finder. CSV and TSV files show as a table (⌘⇧V). You can quote selected lines into the thread composer.
- **Explorer**: Adds natural sort order, virtualized rows, undoing file operations with ⌘Z, staging and discarding from the context menu, and Find in folder. File search lists recent files first and can also search ignored files.
- **Highlighting**: Adds 12 languages, including Java, Ruby, PHP and SQL.
- **Git conflicts**: Resolve conflicts from a bar in the editor. Compare current, base and incoming side by side, and edit the result before you accept it. A rebase that stopped on a merge is now aborted as a rebase, and rebases in progress on SSH hosts are now visible.
- **SSH**: Move files between this machine and an SSH project. When a connection fails, necoder shows why and what to try next. You can test a connection without opening a project. Hosts can be added to `~/.ssh/config`, including jump hosts (every hop is checked). List the ports an SSH host is listening on and forward them to localhost. List the ports of dev servers running inside necoder, and open or stop them.
- **Settings**: Adds search in Settings, switching the display language without a restart, choosing fonts for the UI, code and terminal, row height by density, choosing status bar items, and changing key bindings from the ⌘K ⌘S sheet (with warnings for conflicts and for keys the OS takes first). You can see memory use per project and stop idle agents.
- **Other**: `:shortcode:` in Task, thread and terminal names turns into an emoji. The Help menu gains "Open Logs Folder" and "necoder Manual". The first launch after an update shows what changed.

### Changed

- necoder now reads agent session updates between turns too, and names a thread with the title the agent gives it.
- Commits and pushes made from the panel now run git hooks. The commit message input supports IME, and git failures show a toast.
- The Fleet tab row now lists only conversations. A Task now merges into the main checkout, not into the first slot that looks like an integration target.
- Remote control on the phone shows new messages below the composer, newest first (#36).

### Fixed

- **＋ context filter**: The filter field did not accept typing after it opened. Opening it now focuses the filter; Enter attaches the first match, and Esc closes it and returns to the composer (#39).
- **Todo board**: The input that opens from "＋" collapsed into a thin line, so you could not add a Todo (#29, #33).
- **Rail**: The main checkout disappeared from the rail when it was on a task/* branch (#31).
- **Captain**: The Captain's role instructions now apply on turns written by a person too (#12). Approved rows of a Captain's split now resume after a restart.
- Fleet column tooltips showed a raw `{count}`.
- Diff tabs showed some lines twice.
- necoder no longer overwrites settings.json when it cannot read it.
- Review notes now go to the thread you picked, not to whichever thread sits at its old position. Notes are saved in the order you wrote them, and failed saves can be retried. Change review now caps the number of lines and bytes it keeps in memory.
- localhost URLs from an SSH host no longer open on this machine.
- Text that arrived between turns (a turn the agent continued on its own, or a background task's report) was not saved and got lost.
- When you close a window, necoder now asks for confirmation if agents are running in it, even when other windows are still open.

### Thanks

Thanks to @torumitsutake and @Ryuhei-So for the PRs and detailed bug reports.

### 日本語

> **テストリリースです。** v0.1.20 から 300 件を超える変更を含む大きな版で、まだ実機で確かめていない所があります（`docs/ORCA-PARITY-CHECKLIST.md`）。自動更新では配信しません。試してくださる方は、この Release ページから手動で入れてください。不具合を見つけたら Issue で教えてもらえると助かります。

Orca との機能差を埋める作業（#21）をまとめて取り込んだ版です。変更レビュー、localhost を開く Web タブ、ターミナルの多くの改善、使用量の表示、Fleet の Task まわり、セッション履歴と全文検索、SSH の使い勝手、エージェントの追加と切り替えが入りました。エージェントパネルでは、先頭に `!` を付けるとシェルのコマンドを実行できます。

#### 追加

- **エージェントパネルの `!` 実行**：先頭に `!` を付けると、エージェントには送らずにシェルで実行します（Claude Code CLI の bash モードと同じ働きです）。コマンドはスレッドの作業ディレクトリで動きます。SSH プロジェクトならリモート側、Chat ならチャットのフォルダです。出力は transcript に流れながら表示され、次に送るメッセージに添えてエージェントへ渡します。`!` を打っている間は入力欄が青系の色になり、どこで実行するかを表示します。止めるときは行の「止める」を押すか Esc を押します。そのコマンドが起動した子プロセスも止まります。スマホや AI から届いた `!` は実行せず、ただの文として届きます（#37）。
- **変更レビュー**：worktree の変更を 1 画面で読めます。エディタ、ソース管理、Fleet のどこからでも開けます。行に注記を付け、1 つのプロンプトにまとめて Task やスレッドへ送れます。見た印と注記は、対象のコードに結び付けて保存します。
- **エージェントの入力欄**：`/` でスラッシュコマンドと、リポジトリに置いたプロンプトのレシピを補完します。エージェントの目標を入力欄の上に表示し、一時停止、再開、消去ができます。transcript の横にメッセージレールを置き、プロンプトの間を行き来できるようにしました。エージェントの質問カードには自分の言葉でも答えられます（スマホからも答えられます）。サブエージェントの手順は、それを起動した Task ツールの手順の下に畳んで表示します。
- **localhost の Web タブ**：localhost の URL を Web タブで開き、プレビュー用のツールバーを表示します。Design モードでページの要素を選ぶと、その切り抜きをスレッドに添付できます。Web Inspector は別の窓で開きます。HTML ファイルは組み込みのサーバーを通して開きます。SSH 先の localhost のリンクは、ポートを転送してから開きます。
- **ターミナル**：xterm と kitty のキー入力、マウスとフォーカスの報告、文字の装飾、カーソルの形、クリックできる URL に対応しました。タイトル、OSC 52、ベル、ファイルのドロップ、右クリックメニュー、ターミナル内検索も使えます。横分割（⌘\）、エディタ領域のタブとして開く機能、どこからでも呼び出せる浮動ターミナル（⌃`）、タブの名前の変更、保存したコマンドの実行（▶）も入りました。フォント、スクロールバック、カーソル、シェルを設定でき、Ghostty や Windows Terminal、iTerm2、Warp の配色を取り込めます。OSC 133 を読んで、コマンドが実行中かどうかを判定します。実行中のタブを閉じるときと、エージェントやターミナルが動いている状態で終了するときは確認を出します。
- **使用量と統計**：ACP が報告するコスト、レート制限、ターンごとのトークンを記録し、ステータスバーのチップ、ポップオーバー、統計画面で確認できます。レート制限はエージェント、ホスト、認証ごとに分けて扱います。
- **Fleet と Task**：＋ Task でブランチ名と起点を選べます。新しい Task では、`.worktreeinclude` に書いたファイルの持ち込み、依存フォルダのリンクでの共有、必要なフォルダだけのチェックアウトができます。準備スクリプトは飛ばすことも、失敗したあとにやり直すこともできます。necoder の外で作った worktree を取り込めます。サイドバーでは絞り込み、並べ替え、複数選択、まとめての片付け（⌘⌫）ができます。作成中の Task を表示して取り消せるようにし、Task の入れ子と「詳細…」も加えました。衝突で統合が止まったときは、その Task のエージェントに衝突を解かせることができます。1 つの依頼を複数のエージェントへ並行 Task として送り、結果を比べられます。Captain は承認待ちに推奨を添え、台帳の知らせを人の発言と分けて灰色のカードで表示し、トークン数を表示します。
- **通知**：necoder の窓を見ていないとき、エージェントの質問、ターンの終了、待ち状態を通知します。タイトルバーのベルに履歴が残り、通知音の音量も設定できます。見ているスレッドのターンが終わると、スマホにも知らせます。エージェントが動いている間は、パソコンがスリープしないようにします。
- **セッション履歴と全文検索**：全スレッドのターンを検索でき、エージェントの過去のセッションを一覧から開けます。スレッド履歴の画面を作り直し、区分けした一覧、エージェントのセッション、全文検索をまとめました。
- **エージェント**：ACP レジストリにある全エージェントを、設定から追加できます（バイナリの取得と検証、uvx での起動も含みます）。settings.json で足したエージェントも一覧に並びます。設定画面から、エージェントの有効と無効の切り替え、既定の権限モードの一括設定、起動設定の上書きができます。Claude Code と Codex のログインアカウントを切り替えられます。パレットやキー操作から、選んだエージェントで新しいスレッドを始められます。
- **skill**：necoder の skill を設定から入れられます。CLI（`necoder skills get / install / list`）も使えます。
- **CLI（`ne`）**：動いている necoder の Task、ターミナル、差分を CLI から操作できます（ターミナルへの入力は既定で無効です）。
- **エディタ**：自動保存（フォーカスが外れたとき、または手を止めてしばらく経ったとき）、タブのピン留め、1 回のクリックで開くプレビュータブ、全タブを閉じる操作、`path:line` のコピー、他のエディタで開く操作、ドラッグでエディタの横に並べる操作に対応しました。Markdown では、リンクのクリック、front matter のプロパティ表示、`[toc]`、横に並べるライブプレビュー（⌘K V）、行頭の `/` メニュー、Finder からの画像のドロップが使えます。CSV と TSV は表として表示します（⌘⇧V）。選んだ行をスレッドの入力欄に引用できます。
- **エクスプローラ**：自然順での並べ替え、行の仮想化、⌘Z でのファイル操作の取り消し、右クリックからのステージと破棄、フォルダ内の検索に対応しました。ファイル検索は最近開いたファイルを先に出し、無視されたファイルも探せます。
- **ハイライト**：Java、Ruby、PHP、SQL など 12 言語を追加しました。
- **Git の衝突**：エディタに出る衝突の帯から衝突を解けます。今の側、ベース、入ってくる側を並べて比べ、結果を編集してから採用できます。マージの途中で止まったリベースは、リベースとして中止するようにしました。SSH 先で進行中のリベースも見えるようになりました。
- **SSH**：手元のマシンと SSH プロジェクトの間でファイルを移せます。接続に失敗したときは、理由と次に試すことを表示します。プロジェクトを開かずに接続を試せます。ホストを `~/.ssh/config` に登録でき、踏み台も設定できます（踏み台はすべての段を確認します）。SSH 先で待ち受けているポートを一覧し、localhost へ転送できます。necoder の中で動いている開発サーバーのポートを一覧し、開いたり止めたりできます。
- **設定**：設定の検索、表示言語の即時切り替え、UI とコードとターミナルのフォントの選択、密度に応じた行の高さ、ステータスバーに出す項目の選択ができるようになりました。キーバインドは ⌘K ⌘S のシートから変えられ、衝突するキーと OS が先に取るキーには警告を出します。プロジェクトごとのメモリ使用量を見て、使っていないエージェントを止められます。
- **その他**：Task、スレッド、ターミナルの名前に書いた `:shortcode:` を絵文字に変えます。ヘルプメニューに「ログのフォルダを開く」と「necoder のマニュアル」を足しました。更新後に初めて起動したとき、変更点を表示します。

#### 変更

- エージェントのセッションの更新を、ターンとターンの間も読むようにしました。エージェントがスレッドに題名を付けると、スレッド名もそれに合わせます。
- パネルから行うコミットとプッシュで git フックが走るようにしました。コミットメッセージの入力は IME に対応し、git が失敗したときはトーストで知らせます。
- Fleet のタブの行には会話だけを並べるようにしました。Task は、統合先に見える最初の枠ではなく、メインのチェックアウトへマージします。
- スマホのリモート管制では、入力欄の下に新しいメッセージから順に表示します（#36）。

#### 修正

- **＋ context の絞り込み欄**：開いても文字を入力できない問題を直しました。開くと絞り込み欄にフォーカスが移り、Enter で先頭の候補を添付し、Esc で閉じて入力欄に戻ります（#39）。
- **Todo ボード**：「＋」で開く入力欄が細い線のように潰れ、Todo を追加できなかった問題を直しました（#29、#33）。
- **レール**：メインのチェックアウトが task/* ブランチにあると、レールから消えてしまう問題を直しました（#31）。
- **Captain**：人が書いたターンでも、Captain の役割の指示が効くようにしました（#12）。承認した分解案の行は、再起動したあとも続きから再開します。
- Fleet の列のツールチップに `{count}` がそのまま出ていた問題を直しました。
- 差分タブで同じ行が二重に表示される問題を直しました。
- settings.json を読めなかったときに、そのファイルを上書きしないようにしました。
- 変更レビューの注記が、選んだスレッドではなく、一覧で元の位置にある別のスレッドへ送られることがあった問題を直しました。注記は書いた順に保存し、保存に失敗したら再試行できます。変更レビューが抱えるメモリには、行数とバイト数の上限を設けました。
- SSH ホストの localhost URL を、手元のマシンで開いてしまわないようにしました。
- ターンとターンの間に届いた本文（エージェントが自分で続けたターンや、背景タスクの報告）が保存されずに失われる問題を直しました。
- 窓を閉じるとき、ほかの窓が開いていても、その窓で動いているエージェントがあれば確認を出すようにしました。

#### 謝辞

@torumitsutake さん、@Ryuhei-So さん、PR と詳しい不具合報告をありがとうございました。

## [0.1.20] - 2026-09-24

Improves launching AI agents on Windows, Japanese input in the built-in terminal, and Remote SSH connections. This release includes community PRs for Captain settings, Task input, SSH authentication, and a user manual.

### Added

- **SSH authentication prompts**: When you connect from the macOS GUI, you can now enter a password or a key passphrase. What you type is not saved to settings or history (#5).
- **Appointing and dismissing the Captain**: Do it from each row in Settings → AI Agents. This is separate from the default agent setting (#8).
- **User manual**: A guide that covers everything from installation to Fleet, Chat and Remote SSH, linked from the README (#7).

### Fixed

- **Windows agent launch**: necoder started Node.js's Unix `npx` by mistake. It now picks `npx.cmd` on Windows (#4).
- **Japanese input in the built-in terminal**: The terminal now tells the OS when IME composition is in progress, so the Enter that confirms a conversion no longer runs the command or sends the prompt (#3).
- **Remote SSH from Mac to Linux**: The macOS app now bundles the Linux server (x86_64 / aarch64) that runs on the remote host (#6). This is not a Linux desktop build.
- **+ Task input**: When you opened the dialog by clicking the button, the input lost focus. Esc now closes the dialog, and canceling puts focus back where it was (#9).
- Restoring a project's tabs took keyboard focus away from the rail.
- When several SSH authentication requests arrive at once, a new one no longer overwrites a prompt that is already waiting.
- Corrected the theme selector shortcut in the README to **⌘K ⌘T**.

### Thanks

Thanks to @torumitsutake, @Lehtien and @keitakoyama-cmd for the PRs and detailed bug reports.

### 日本語

Windows での AI エージェントの起動、内蔵ターミナルでの日本語入力、Remote SSH の接続を直しました。コミュニティからいただいた PR（Captain の設定、Task の入力、SSH の認証、利用者向けマニュアル）を取り込んでいます。

#### 追加

- **SSH の認証入力**：macOS の GUI から接続するとき、パスワードや鍵のパスフレーズを入力できるようになりました。入力した内容は設定にも履歴にも保存しません（#5）。
- **Captain の任命と解任**：設定 → AI エージェントの各行から操作できます。既定のエージェントとは別に設定します（#8）。
- **利用者向けマニュアル**：インストールから Fleet・Chat・Remote SSH までの使い方をまとめ、README からリンクしました（#7）。

#### 修正

- **Windows での AI エージェントの起動**：Node.js の Unix 用 `npx` を誤って起動していたのを直し、Windows 用の `npx.cmd` を選ぶようにしました（#4）。
- **内蔵ターミナルの日本語入力**：IME で変換中であることを OS に伝えるようにしました。これで、変換を確定する Enter でコマンドが実行されたり、プロンプトが送られたりしなくなりました（#3）。
- **Mac から Linux への Remote SSH**：接続先で動く Linux 用のサーバー（x86_64 / aarch64）を macOS アプリに同梱しました（#6）。Linux のデスクトップ版を出したわけではありません。
- **＋ Task の入力**：ボタンをクリックしてダイアログを開くと、入力欄からフォーカスが外れる問題を直しました。Esc で閉じられるようにし、取り消したあとは元の場所にフォーカスを戻します（#9）。
- プロジェクトのタブを復元するときに、レールのキーボードフォーカスが奪われる問題を直しました。
- SSH の認証要求が同時に届いたとき、先に待っている入力を上書きしないようにしました。
- README に書いていたテーマ選択のショートカットを **⌘K ⌘T** に訂正しました。

#### 謝辞

@torumitsutake さん、@Lehtien さん、@keitakoyama-cmd さん、PR と詳しい不具合報告をありがとうございました。

## [0.1.19] - 2026-09-19

This release adds **Chat mode**, a third surface for conversations that don't belong to any project. Switch to it with `Editor | Fleet | Chat` in the titlebar (⌘⇧J). The agent answers questions in plain text. Only when you ask for something to look at does it write a single standalone HTML or Markdown file to `necoder/<date first sentence>/artifacts/` in your Documents folder and show it to the right of the conversation. Artifacts are ordinary files: you can open them from Finder, and they stay even if you remove necoder. The rule is that handing over a file means the agent may touch it. It can read files you drop in, and it asks once before the first edit. Writes anywhere you didn't hand over are refused. Also new: pasting images (⌘V a screenshot) and searching inside a conversation (⌘F) now work in every thread, and agents you aren't using stop after a while.

### Added

- **Chat mode**: Open it from `Chat` in the titlebar, the rail icon, **⌘⇧J**, or "View: Chat mode" in the palette. Chat doesn't belong to a project, so the window frame has no color. A colored window is inside a project; an uncolored one is Chat.
  - **Chat list on the left**: + New chat (**⌘N**), search over both titles and message text, and groups for Pinned, Today, Yesterday and Previous 7 days. The ● at the start of a row shows whether the agent is running (outline only means it isn't). Right-click to pin, reveal the folder, export the conversation as Markdown, or delete it.
  - **Conversation in the middle, the existing editor area on the right**: With no artifacts, the right side stays closed and the conversation gets the full width. An artifact strip above the conversation lets you go back to them at any time.
  - **Where artifacts go**: `Documents/necoder/<YYYY-MM-DD first sentence>/artifacts/`. The folder is created on the first send, and folders of chats that left nothing behind are cleaned up. You can change the location in Settings.
  - **Files you hand over**: The agent can read attached files and folders. It asks before the first edit; choose "Always allow in this chat" and it stops asking. Writes anywhere else are refused with a one-line reason. The previous content is still recorded before each edit, so you can roll back from the checkpoint row in the transcript.
  - **Artifacts are sandboxed**: HTML written by the agent is shown in a dedicated view. It can't read outside that chat's `artifacts/`, can't reach the network, and doesn't navigate to other pages (links open in your default browser).
  - **Claude only** for now, because the way to pass chat-specific settings is specific to the Claude adapter.
- **`▣ Preview` on ⏺/⎿ tool cards**: Opens HTML or Markdown the agent wrote in a preview tab with one click. It appears in every thread, not only in Chat.
- **Paste and drop images** (all threads): Paste a screenshot with ⌘V or drop an image file on the composer, and the agent receives it as an image. Images that are too large are still attached as a path, as before.
- **Search inside a conversation** (**⌘F**, all threads): A search bar opens above the transcript. Every match is highlighted, and the entry you are on is shown darker. Move with ⏎ and ↑ ↓; close with esc.
- **Copy code blocks**: Hover a code block in the transcript and ⧉ appears at the top right. It copies the full text even when the block is collapsed.
- **Stop agents you aren't using**: Set it in Settings under "Behavior & editor > Stop unused agents after (minutes)". The default is 15 minutes; `0` turns it off. Each agent holds a few hundred MB, so this matters once many threads stay open. The conversation is kept, and the next message picks it up again.
- **Turn off claude.ai connectors**: A new switch at the top of Settings "MCP servers". By default they still load, as before. Connectors linked to your account are added to Claude threads automatically, so turning them off makes the first reply faster (measured 4.2 s → 1.8 s). **Chat mode never loads them**, whatever this setting says.
- **Tab context menu**: Reveal in Finder, Open in default app, Copy path, Close, Close others, and Close to the right. For Chat artifacts you can also choose "Copy to project" (if a file with the same name exists, it adds a number instead of overwriting).

### Changed

- **⌘N depends on the surface**: New chat in Chat, + Task in Fleet.
- **Agents start faster**: When the same version is already in the npx cache, necoder launches it directly instead of going through the wrapper. That removes a 48MB process per agent and the dependency resolution on every launch.
- **Completion sound**: It now plays only for panels you can't see. It no longer plays when a conversation finishes right in front of you.
- Removed the old Fleet grid UI. The Fleet v2 stage had replaced it, and nothing opened it anymore.

### Fixed

- **HTML preview stayed stale after the agent rewrote the file**: It now reloads on changes made outside necoder too. It reloads once after writes settle, so the page state isn't lost on every write.
- On Windows, clicking a path like `C:\…\ROADMAP.md:157` dropped the line number and landed at the top of the file.

### 日本語

**Chat モード**を追加しました。titlebar の `Editor | Fleet | Chat`（⌘⇧J）で切り替える 3 つ目の画面で、プロジェクトに属さない相談に使います。エージェントは相談には文章で答え、見せるもの（成果物）を頼んだ時だけ、書類フォルダの `necoder/<日付 最初の文>/artifacts/` に単体の HTML か Markdown を 1 枚書いて、会話の右に表示します。成果物は普通のファイルなので Finder から開けますし、necoder を消しても残ります。ファイルを渡すことは触ってよいと伝えること、という決まりにしました。ドロップしたファイルはエージェントが読めて、書き換えは最初の 1 回だけ確認します。渡していない場所への書き込みは断ります。あわせて、画像の貼り付け（スクリーンショットを ⌘V）と会話の中の検索（⌘F）を全スレッドで使えるようにし、使っていないエージェントは一定時間で止まるようにしました。

#### 追加

- **Chat モード**：titlebar の `Chat`、レールのアイコン、**⌘⇧J**、パレットの「表示: Chat モード」から開けます。Chat はプロジェクトに属さないので、窓の枠に色を付けません。色のある窓はプロジェクトの中、色のない窓は Chat です。
  - **左はチャットの一覧**：＋ 新しいチャット（**⌘N**）と、題名と本文の両方を探せる検索があります。チャットはピン留め、今日、昨日、過去 7 日に分けて並びます。行頭の ● はエージェントが起きているかどうかを示し、輪郭だけなら起きていません。右クリックで、ピン留め、フォルダを表示、会話を Markdown で書き出す、削除を選べます。
  - **中央は会話、右はこれまでのエディタ領域**：成果物がなければ右側は閉じていて、会話を広く使えます。会話の上に成果物の帯があり、いつでも成果物に戻れます。
  - **成果物の置き場**：`書類/necoder/<YYYY-MM-DD 最初の文>/artifacts/` です。フォルダは最初に送信した時に作り、何も残らなかったチャットのフォルダは片付けます。置き場は設定で変えられます。
  - **渡したファイルの扱い**：添付したファイルやフォルダはエージェントが読めます。書き換えは最初の 1 回だけ確認し、「このチャットでは以後許可」を選べば以後は聞きません。それ以外の場所への書き込みは断り、理由を 1 行残します。書き換える前の中身はこれまでどおり記録するので、transcript の checkpoint 行から戻せます。
  - **成果物の表示の閉じ込め**：エージェントが書いた HTML は専用の仕組みで表示します。そのチャットの `artifacts/` の外は読めず、外部と通信できず、ほかのページへも移動しません（リンクは既定のブラウザで開きます）。
  - 当面は **Claude 専用**です。会話用の設定を渡す口が Claude のアダプタにしかないためです。
- **⏺/⎿ のツールカードの `▣ プレビュー`**：エージェントが書いた HTML や Markdown を、1 クリックでプレビューのタブに開きます。Chat 以外のスレッドでも出ます。
- **画像の貼り付けとドロップ**（全スレッド）：スクリーンショットを ⌘V で貼るか、画像ファイルを入力欄にドロップすると、エージェントに画像として渡します。大きすぎる画像はこれまでどおりパスとして添付します。
- **会話の中の検索**（**⌘F**、全スレッド）：transcript の上に検索バーが出ます。一致した所はすべて光り、いま見ているエントリだけを濃く表示します。⏎ と ↑ ↓ で移動し、esc で閉じます。
- **コードブロックのコピー**：transcript のコードブロックにポインタを重ねると、右上に ⧉ が出ます。畳んでいても全文をコピーします。
- **使っていないエージェントを止める設定**：設定の「動作とエディタ > 使っていないエージェントを止めるまで（分）」で決めます。既定は 15 分で、`0` にすると止めません。エージェントは 1 本で数百 MB のメモリを使うので、開いたままのスレッドが増えると効いてきます。止めても会話は残り、次に送信した時に続きから再開します。
- **claude.ai のコネクタを切る設定**：設定の「MCP サーバ」の先頭に追加しました。既定ではこれまでどおり読み込みます。アカウントに繋いだコネクタは Claude のスレッドへ自動で入るため、切ると最初の応答が速くなります（実測 4.2 秒 → 1.8 秒）。**Chat モードはこの設定に関係なく読み込みません**。
- **タブの右クリックメニュー**：Finder で表示、既定アプリで開く、パスをコピー、閉じる、他を閉じる、右を閉じる、を選べます。Chat の成果物では「プロジェクトへコピー」も選べます（同じ名前のファイルがあれば上書きせず、連番を付けます）。

#### 変更

- **⌘N の意味**：画面ごとに決めました。Chat では新しいチャット、Fleet では ＋ Task です。
- **エージェントの起動**：同じ版が npx のキャッシュにあれば、ラッパーを通さず直接起動するようにしました。1 本あたり 48MB のプロセスと、起動のたびの依存の解決がなくなり、速く起動します。
- **完了音**：見えていないパネルで終わった時だけ鳴らすようにしました。目の前で終わった会話では鳴りません。
- 旧 Fleet のグリッド UI を削除しました。Fleet v2 の舞台に置き換わり、開く手段がなくなっていたものです。

#### 修正

- **HTML プレビューの更新漏れ**：エージェントがファイルを書き換えても、プレビューが古いままでした。外部からの変更でも読み直すようにしました。書き込みが落ち着いてから 1 回だけ読み直すので、ページの状態が毎回消えることはありません。
- Windows で `C:\…\ROADMAP.md:157` のようなパスをクリックすると、行番号が抜けてファイルの先頭に移動していた問題を直しました。

## [0.1.18] - 2026-09-16

Fleet is now a single screen built around Tasks. The center is a stage of Task cards (1 to 3 columns, ⌘⇧1/2/3), and each card holds its conversation, terminal, changes and files as tabs. Pending approvals and failures collect under Attention on the left, and you can allow, open Radar, Integrate or confirm right inside the card. **+ Task (⌘N)** takes a single instruction: it creates a worktree, runs the setup script, starts an agent and sends that instruction. **File paths and URLs in agent replies are now clickable.** The notification sounds were rebuilt so they actually sound like a cat.

### Added

- **Stage and Task cards** (center of Fleet): One Task is one card. The header shows `● name ⎇ branch`, a phase pill (Working / Awaiting approval / Waiting for review / Integrated / Failed), a single **"Next"** button whose action follows the phase (Go to review / Allow / Integrate / Request changes / Clean up), ⤢ (read it in one column) and ⋯ (clean-up menu).
  - **Tabs inside a Task**: `● thread name` (one or more) | Changes | Terminal N | Files | + Conversation | + Terminal. Adding conversations or terminals to the same worktree **doesn't add cards**. Clicking a file in the Files tab opens it in the editor.
  - **1 / 2 / 3 columns** (toggle at the top center, ⌘⇧1/2/3): Use **◫ (pin)** on a Task row to choose which Tasks go on the stage. If a card would get narrower than 420px, the column count drops automatically. Tasks taken off the stage can be brought back from the list on the left. Closing a card keeps its conversation and PTY alive.
- **+ Task dialog** (⌘N / + Task in the sidebar): One input field (multi-line, ⌘⏎ to start). From the first line it picks `task/<slug>` and the worktree location `<next to the repository>/<name>-worktrees/<slug>`, and shows both. Starting it creates the worktree, runs the setup script, starts the agent and sends what you wrote as is. The first line becomes the Task name.
  - **Setup script `.necoder/worktree-setup.sh`**: If it exists, it runs once with the new worktree as cwd (environment variables `NECODER_MAIN_ROOT` / `NECODER_TASK_ROOT` / `NECODER_TASK_BRANCH`). If it doesn't, "Create" in the dialog writes a template and opens it in the editor. The template does three things: copy in `.env`, generate `task.env`, and prepare dependencies. A Task whose setup fails becomes `failed` and shows up under Attention.
  - **`.necoder/task.env`**: One `KEY=VALUE` per line. It is injected into the environment of the ACP agents and terminals started for that Task (it is not evaluated as shell). For Rust, the single line `CARGO_TARGET_DIR=<main>/target` avoids recompiling dependencies for every worktree. Adding it to `.gitignore` is recommended.
- **Lineage strip**: One line in the Task's color below the collapsed lineage header (`┬━ name`, `╰━` once integrated, `━` for main). Click it to bring that Task to the stage. ⌘⇧G expands or collapses the previous four views.
- **Captain card** (⌘0): Tabs for the Captain thread, the Captain log and the Task list. Ledger events for Tasks (finished, failed, waiting for approval, integrated) reach the Captain **as one batched message** when it is free, together with a status table (Task / phase / digest). Anything you tell a Task directly is also recorded in the ledger and News as `human_send` and passed on to the Captain.
- **Paths and URLs in replies are links**: Both `crates/foo/src/bar.rs:42`-style text and Markdown links jump to the line when clicked. A bare file name (`agent_panel.rs`) opens only when it matches exactly one entry in the @mention index. **Only files that exist become links**, so numbers like `0.5` or `v0.1.17` never get underlined. HTML opens in the preview (or as source when a line number is given), images and PDFs open in their own tabs, and only folders and formats necoder can't display (zip, video, Office and so on) open in the OS default app. Drag selection still works: a link opens only when you press and release at the same spot. Terminal path links now use the same detector (still only paths with a line number, as before).

### Changed

- **Attention is now always in the Fleet sidebar**: It sits right below the Captain bar and covers only the selected repository. Allow / Always allow / Deny, View changes, Integrate and Confirm all finish inside the card. ⏎ / ⌘⇧U jumps to the first item.
- **Removed the "Control" and "Work" center tabs and the bottom + ACP / + Terminal / + Worktree bar**: Eight concepts, three center tabs and three + buttons made the flow hard to see. Attention moved to the sidebar, and the add actions moved to the Task card's tab row. The full-screen control view stays as the reference for Remote control (phone).
- Expanding a cell (⤢, ⌘⇧⏎) now means "show the stage in one column". The thumbnail strip is gone.
- Keys in Fleet: ⌘N (+ Task), ⌘⇧1/2/3 (columns), ⌘⇧G (lineage strip), ⌘⇧U (jump to the first Attention item). On non-mac systems ⌘⇧G is ctrl-alt-g (ctrl-shift-g is the Git panel).
- **Rebuilt the notification sounds**: The old sounds put a cat's melody (pitch rising and falling) on an electronic tone. **The formants were fixed**, so they didn't sound like a cat. Synthesis now uses a **vocal tract model** and plays "nya" as the mouth itself moving: a closed nasal [ɲ], an open [a], then closed again. We still don't use recordings, so the sounds are still our own (`scripts/gen-chime.py`).
- **The completion sound is now a 450ms "nyaa"** (default). **Waiting for input plays a 250ms "nya" twice.** One call or two lets you tell "finished" from "you're needed" by ear alone.
- **Three voices to choose from**: **Long meow** (default; completion 450ms / waiting 250ms×2), **Meow** (short; 250ms / 180ms×2) and **Chirp** (a kitten's chirp; 200ms / 150ms×2). You can pick different voices for completion and waiting.
- **Clicking a segment in Settings plays the sound right away** as a preview. You can't choose a notification sound without hearing it, so choosing and previewing are one action.
- Bundled wav files are now named `assets/sounds/<voice>-<scene>.wav` (`done.wav` / `waiting.wav` are gone). You can still set `sound_done` / `sound_waiting` to the path of your own file.

### Fixed

- **Phone PWA**: If you denied camera access, "Scan QR code" could turn into a button that silently did nothing. The cleanup (stopping the camera, closing the dialog) ran before the message was shown, so when cleanup failed in some environments the message never appeared. This fix needs a deploy on the relay side; it is not part of the `.app`.

### 日本語

Fleet を、**Task を中心にした 1 画面**に作り直しました。中央は Task カードを並べる舞台（1〜3 列、⌘⇧1/2/3）で、会話、ターミナル、変更、ファイルは **Task カードの中のタブ**にまとめました。承認待ちや失敗は左の「要対応」に集まり、許可、Radar、Integrate、確認までカードの中で済ませられます。**＋ Task（⌘N）** では指示を 1 つ書くだけで、worktree の作成、準備スクリプトの実行、エージェントの起動、指示の送信までを行います。エージェントの返答に出てくる**ファイルパスと URL は、クリックで開けるようになりました**。通知音は、猫の声に聞こえるように作り直しました。

#### 追加

- **舞台と Task カード**（Fleet の中央）：1 つの Task が 1 枚のカードです。ヘッダには `● 名前 ⎇ branch`、phase のピル（稼働中 / 承認待ち / レビュー待ち / 統合済み / 失敗）、phase に応じて中身が変わる **「次へ」ボタン 1 つ**（レビューへ / 許可 / Integrate / 修正を指示 / 片付け）、⤢（1 列にして読む）、⋯（片付けメニュー）が並びます。
  - **Task の中のタブ**：`● スレッド名`（複数可）| 変更 | ターミナル N | ファイル | ＋会話 | ＋端末。同じ worktree に会話や端末を何本足しても、**カードは増えません**。ファイルタブでクリックしたファイルはエディタ側で開きます。
  - **1 / 2 / 3 列**（中央上のトグル、⌘⇧1/2/3）：Task 行の **◫（ピン）** で、舞台に並べる Task を選びます。カード 1 枚の幅が 420px を下回る時は、列数を自動で減らします。舞台から外した Task は左の一覧から戻せます。カードを閉じても、会話と PTY はそのまま残ります。
- **＋ Task ダイアログ**（⌘N、またはサイドバーの ＋Task）：入力欄は 1 つです（複数行可、⌘⏎ で開始）。1 行目から `task/<slug>` と worktree の置き場所 `<リポジトリの隣>/<名前>-worktrees/<slug>` を自動で決めて表示します。開始すると、worktree の作成、準備スクリプト、エージェントの起動、書いた指示の送信までを続けて行い、1 行目を Task 名にします。
  - **準備スクリプト `.necoder/worktree-setup.sh`**：あれば、新しい worktree を cwd にして 1 回だけ実行します（環境変数は `NECODER_MAIN_ROOT` / `NECODER_TASK_ROOT` / `NECODER_TASK_BRANCH`）。なければ、ダイアログの「作る」でテンプレートを書き、エディタで開きます。テンプレートがするのは、`.env` の持ち込み、`task.env` の生成、依存の準備の 3 つです。準備に失敗した Task は `failed` になり、要対応に出ます。
  - **`.necoder/task.env`**：1 行に 1 つ `KEY=VALUE` を書きます。その Task で起動する ACP エージェントとターミナルの環境変数に入れます（シェルとしては評価しません）。Rust なら `CARGO_TARGET_DIR=<main>/target` の 1 行で、worktree ごとに依存を再コンパイルせずに済みます。`.gitignore` に入れておくことをおすすめします。
- **系譜の帯**：畳んだ系譜ヘッダの下に、Task の色で 1 行を出します（`┬━ 名前`、統合済みは `╰━`、main は `━`）。クリックするとその Task を舞台に出します。⌘⇧G で、従来の 4 つの表示を開いたり畳んだりできます。
- **Captain カード**（⌘0）：タブは Captain のスレッド、采配ログ、Task 一覧です。Task の完了、失敗、承認待ち、統合といった台帳のイベントは、Captain の手が空いた時に **1 通にまとめて**届き、現況表（Task / phase / digest）が付きます。Task に直接話しかけた内容も `human_send` として台帳とニュースに残り、Captain に伝わります。
- **返答の中のパスと URL のリンク**：`crates/foo/src/bar.rs:42` のような書き方も Markdown のリンクも、クリックするとその行に移動します。ファイル名だけ（`agent_panel.rs`）の場合は、@mention 用の索引で 1 つに決まる時だけ開きます。**リンクにするのは実在するファイルだけ**なので、`0.5` や `v0.1.17` のような数字に下線が付くことはありません。HTML はプレビューで（行番号付きならソースで）、画像や PDF は専用のタブで開き、フォルダと necoder で表示できない形式（zip、動画、Office など）だけを OS の既定アプリで開きます。ドラッグでの選択とは両立します（押した位置と離した位置が同じ時だけ開きます）。ターミナルのパスリンクも同じ検出の仕組みに揃えました（これまでどおり行番号付きのものだけ）。

#### 変更

- **要対応を Fleet サイドバーに常に表示**：Captain バーのすぐ下に、選んでいるリポジトリの分だけ出ます。許可、常に許可、拒否、変更を見る、Integrate、確認は、カードの中でその場で済みます。⏎ か ⌘⇧U で先頭の項目へ移動します。
- **中央の「管制」「作業」タブと、下部の ＋ACP / ＋Terminal / ＋Worktree バーの廃止**：概念が 8 つ、中央のタブが 3 つ、＋ボタンが 3 つあり、作業の流れが見えにくかったためです。要対応はサイドバーへ、追加の操作は Task カードのタブ行へ移しました。全画面の管制は、スマホから使うリモート管制の基準として残します。
- セルの拡大（⤢、⌘⇧⏎）は「舞台を 1 列にする」操作になりました。サムネイル列はなくなりました。
- Fleet で使うキーは、⌘N（＋ Task）、⌘⇧1・2・3（列数）、⌘⇧G（系譜の帯）、⌘⇧U（要対応の先頭へ）です。mac 以外では ⌘⇧G は ctrl-alt-g です（ctrl-shift-g は Git パネル）。
- **通知音の作り直し**：これまでの音は、猫の節回し（音程が上がって下がる）を電子音に載せたものでした。**フォルマントが固定**だったため、猫の声には聞こえませんでした。合成に**声道のモデル**を入れ、「にゃっ」を口の開け閉めそのものとして鳴らしています。閉じた鼻音 [ɲ] から開いた [あ] へ移り、また閉じます。録音を使わない方針は変えていないので、権利は引き続き自前で持っています（`scripts/gen-chime.py`）。
- **完了音は 450ms の「にゃー」**（既定）：**入力待ちは 250ms の「にゃっ」を 2 回**鳴らします。1 回か 2 回かで、耳だけで「終わった」と「呼ばれている」を聞き分けられます。
- **鳴き方を 3 つから選べる設定**：**にゃー**（既定、完了 450ms / 待ち 250ms×2）、**にゃっ**（短い、250ms / 180ms×2）、**みゃっ**（子猫のさえずるような声、200ms / 150ms×2）から選べます。完了と入力待ちで別の声にもできます。
- **設定でのその場の試聴**：設定でセグメントを押すと、その場で音が鳴ります。通知音は聴かないと選べないので、選ぶ操作と試聴を同じにしました。
- 同梱の wav の名前を `assets/sounds/<声>-<場面>.wav` に変えました（`done.wav` と `waiting.wav` は廃止）。`sound_done` / `sound_waiting` に自分のファイルのパスを書ける点は変わりません。

#### 修正

- **スマホ連携の PWA**：カメラを拒否した時に、「QRを読み取る」を押しても何も起きないことがありました。理由を表示する前に後始末（カメラの停止とダイアログを閉じる処理）をしていたため、環境によって後始末が失敗すると、文言の表示までたどり着けませんでした。反映には中継サーバ側のデプロイが必要です（`.app` には含まれません）。

## [0.1.17] - 2026-09-14

Phone pairing **no longer needs an account**. `control.necoder.com` is now open as a public relay run by the author, so all you need is Node.js 22+ and necoder. No Cloudflare account and no wrangler. You can **scan the QR code inside the home screen app**, which removes the iOS dead end where the Camera app opened Safari and the pairing didn't carry over. Your Mac no longer goes to sleep on its own while paired. Settings home now has two panes (nav on the left, a page on the right), and the HTML preview and PDF no longer cover UI in front of them.

### Added

- **Scan QR codes inside the PWA** ("Scan QR code" button): Until now you were expected to scan the QR code with the iPhone Camera app. That opens **Safari**, which has separate storage from the PWA added to your home screen, so the pairing didn't carry over (iOS has no universal links for PWAs). Like Signal and Tailscale, you now **scan inside the app**.
  - Only a **pairing URL for this origin** is accepted from the scanned text. On success, the camera stops and the dialog closes.
  - Devices that denied camera access or have no camera don't hit a dead end. "No camera? Paste the pairing URL" opens and shows why. Pasting the URL is now the fallback instead of the main path.
  - The decoder (jsQR, Apache-2.0) **loads only when you open the scanner**, not at startup.
- **Your Mac doesn't idle-sleep while paired** (macOS only): The point is to reach it from outside, so the condition is not "while connected" but **"while at least one device is paired"**. Once it sleeps, there is no way to wake it from outside.
  - **It doesn't keep the Mac awake on battery**, so a laptop in a bag won't drain. Plugging in or unplugging is picked up within 30 seconds. Display sleep is not blocked. Set `"keepAwake": false` in `config.json` to turn it off.
  - **Closing the lid still puts the Mac to sleep.** Power assertions don't cover it, and blocking it requires root. Dropping the connection silently is the worst outcome, so on Macs with a lid necoder says so after `ne remote pair` and through `sleeps_on_lid_close` in `ne remote status`. To use it with the lid closed, run in clamshell mode with AC power and an external display.
  - `ne remote status` now shows `keeps_awake` / `on_ac_power` / `sleeps_on_lid_close`.
- **Counts and filters on the MCP settings page**: "M of N enabled", source chips (All / necoder settings / Codex CLI / Claude Code / Cursor; **only sources that actually have at least one server** appear), and "Enabled only". The MCP row in the left nav shows an `enabled/total` badge, so you can see the state without opening it.

### Changed

- **Phone pairing no longer needs an account**: The 0.1.14 implementation required a single shared secret (`PROVISION_TOKEN`) to create rooms, which in practice meant everyone deployed their own Worker on a Cloudflare account. **That requirement is gone.** Setup is now just two commands: `ne remote init` → `ne remote pair`.
  - **Why it's safe without a token**: The room id, the host and phone tokens, and the long-term E2E keys are **all generated locally on the Mac**. The relay only keeps SHA-256 hashes of the tokens, and the E2E key travels only in the QR code's fragment (`#`). There is no identity for the relay to authenticate in the first place. The token was never authentication, only a gate against mass room creation.
  - Abuse is now limited in three ways: (1) a per-IP rate limit (5 per 60s), (2) unpaired rooms disappear after 300 seconds, and (3) proof-of-work (off by default; it can be turned on later without redistributing the host).
  - To run your own relay, switch to it with `ne remote init https://<your origin>` (`relay/README.md`).
- **Settings home now has two panes** (left: nav column; right: the selected page): Previously every section was stacked in one long scroll. The MCP server list has **no upper bound**, since it grows with every server you add to Codex CLI, Claude Code or Cursor, so Appearance and everything below it got pushed off screen.
  - Pages: **AI Agents / MCP servers / Appearance / Remote / Behavior & editor**. Nav labels are the same strings as the section headings, so the table of contents and the page never call things by different names.
  - **The page you were on is not written to `settings.json`**, because it isn't a setting. It is remembered until you close the window; after a restart you start at "AI Agents".
  - **The first run (onboarding) has no nav.** It is still one scroll, read from the top.
- **Separate message when the relay is busy**: "The relay is busy. Wait about a minute, then…". The cause is different from having no network, so it no longer shares the same message.

### Fixed

- **HTML preview and PDF stayed on top of ⌘P, ⌘F and menus**: OS child views (WKWebView on macOS, WebView2 on Windows) are **always drawn above** GPUI content in the same window, and there is no way to put a layer between them. necoder now **hides the native view while any interactive UI is open** (palette / ⌘F / project search / completion / hover / code actions / go to line / rename / inline edit / hunk menu / rail menu / color picker / branch menu / explorer context menu / SSH prompt / key list / About / worktree delete confirmation).
  - Toasts, the project name flash and confetti **don't count**. Having the preview vanish every time a notification appears would be more annoying.
  - While hidden, it shows "Returns when the panel in front closes" instead of a blank area. Otherwise it looks as if the preview disappeared.
  - Hiding also hands OS keyboard focus back to necoder, which **also fixes "I opened the palette but can't type Japanese"**.
- **Labels in Settings "Appearance" wrapped one character per line**: With seven themes, the chips no longer fit on one row, and the left column collapsed to zero width (Japanese has no spaces, so it wraps per character). The chips now sit in a two-row container that spans the full width.
- **`ne remote init` led to a dead end**: Without a provisioning token it just printed "Initialized", pairing always failed, and the GUI told you to run `ne remote init`, so people who had already run it went in circles. With the token itself removed (see "no longer needs an account" above), this is gone.

### 日本語

スマホ連携に**アカウントが要らなくなりました**。`control.necoder.com` を作者が運用する公開リレーとして開いたので、使う側に必要なのは Node.js 22+ と necoder だけです。Cloudflare のアカウントも wrangler も要りません。QR は**ホーム画面アプリの中で読み取れる**ようになりました。これで iOS の「カメラアプリで読むと Safari が開き、ペアリングが引き継がれない」という行き止まりがなくなります。ペアリング中は Mac が勝手にスリープしません。設定ホームは左のナビと右のページの 2 ペインになり、HTML プレビューと PDF が前面の UI を隠さなくなりました。

#### 追加

- **PWA の中での QR の読み取り**（「QRを読み取る」ボタン）：これまでは iPhone のカメラアプリで QR を読む前提でした。しかしそれだと **Safari** が開き、ホーム画面に追加した PWA とは保存領域が別なので、ペアリングが引き継がれませんでした（iOS には PWA 向けの universal link がありません）。Signal や Tailscale と同じく、**アプリの中で読む**形に変えました。
  - 読み取った文字列は、**この origin のペアリング URL しか受け付けません**。読み取れたらカメラを止めてダイアログを閉じます。
  - カメラを拒否した端末やカメラのない端末でも、行き止まりにはしません。「カメラが使えないとき（URLを貼り付ける）」が開き、理由を表示します。URL の貼り付けは、主な手順から代わりの手段に下げました。
  - デコーダ（jsQR、Apache-2.0）は、**スキャンを開いた時だけ読み込みます**。起動時には読み込みません。
- **ペアリング中の Mac のスリープ防止**（macOS のみ）：ペアリング中は、放っておいても Mac がスリープしなくなりました。外から繋ぐための機能なので、条件は「接続中」ではなく**「ペアリング済みの端末が 1 台でもある間」**です。スリープしてしまうと、外から起こす手段がないためです。
  - **電池で動いている間はスリープを止めません**。鞄の中のノートが起きたままで電池を使い切らないようにするためです。電源の抜き差しには 30 秒以内に追従します。ディスプレイのスリープは止めません。`config.json` に `"keepAwake": false` と書けば無効にできます。
  - **蓋を閉じた時のスリープは止まりません**。power assertion の対象外で、止めるには root 権限が要るためです。知らないうちに切れるのが一番困るので、蓋のある機種では `ne remote pair` の後と、`ne remote status` の `sleeps_on_lid_close` でそのことを知らせます。蓋を閉じたまま使う場合は、電源と外部ディスプレイを繋いだ clamshell モードにしてください。
  - `ne remote status` に `keeps_awake` / `on_ac_power` / `sleeps_on_lid_close` を表示します。
- **設定の MCP ページの件数と絞り込み**：「N 件中 M 件有効」の表示、出所のチップ（すべて / necoder の設定 / Codex CLI / Claude Code / Cursor。**1 件以上ある出所だけ**出ます）、「有効のみ」を追加しました。左のナビの MCP の行には `有効/全体` のバッジが出るので、開かなくても状態が分かります。

#### 変更

- **スマホ連携のアカウント不要化**：0.1.14 の実装では、部屋を作るのに共有の秘密 1 つ（`PROVISION_TOKEN`）が必要で、結果として各自が Cloudflare のアカウントで Worker をデプロイする形になっていました。**これをやめました**。使う側の手順は `ne remote init` → `ne remote pair` の 2 コマンドだけです。
  - **token なしでも安全な理由**：room id、host と phone の token、E2E の長期鍵は、**すべて Mac がローカルで作ります**。リレーが持つのは token の SHA-256 ハッシュだけで、E2E の鍵は QR のフラグメント（`#`）にしか載りません。リレーが認証すべき身元はもともと存在せず、token は認証ではなく、部屋を大量に作られるのを防ぐ関所でしかありませんでした。
  - 代わりの濫用対策は 3 つです。①IP ごとのレート制限（60 秒に 5 回）②ペアリングされていない部屋は 300 秒で消える ③proof-of-work（既定は無効。必要になれば、ホストを配り直さずに有効にできます）。
  - 自分でリレーを運用する場合は、`ne remote init https://<自分のorigin>` で切り替えられます（`relay/README.md`）。
- **設定ホームの 2 ペイン化**：左がナビの列、右が選んだ 1 ページです。これまでは全セクションを 1 枚の縦スクロールに積んでいました。MCP サーバは Codex CLI、Claude Code、Cursor に足すほど増える**上限のない一覧**なので、外観から下が画面の外へ押し出されていました。
  - ページは **AI エージェント / MCP サーバ / 外観 / リモート / 動作とエディタ** です。ナビのラベルはセクション見出しと同じ文字列にしたので、目次と本文で呼び名が食い違いません。
  - **どのページを見ていたかは `settings.json` に書きません**（設定値ではないため）。窓を閉じるまでは覚えていて、再起動すると「AI エージェント」から始まります。
  - **初回（オンボーディング）ではナビを出しません**。これまでどおり 1 枚のスクロールで、上から読んでもらいます。
- **リレーが混んでいる時の文言**：「中継サーバが混み合っています。1分ほど置いてから…」と出すようにしました。ネットに繋がらない場合とは原因が違うので、同じ文言にはしません。

#### 修正

- **HTML プレビューと PDF が ⌘P、⌘F、メニューより手前に居座る問題**：OS の子ビュー（macOS の WKWebView、Windows の WebView2）は、同じ窓の GPUI の描画より**常に手前**に表示され、間に層を挟む方法がありません。そこで、**操作を受け付ける UI が開いている間はネイティブのビューを隠す**ようにしました（パレット / ⌘F / プロジェクト検索 / 補完 / hover / コードアクション / 行移動 / リネーム / インライン編集 / hunk メニュー / レールメニュー / 色ピッカー / ブランチメニュー / エクスプローラの右クリック / SSH 入力 / キー一覧 / About / worktree 削除の確認）。
  - トースト、プロジェクト名のフラッシュ、紙吹雪は**対象にしません**。通知が出るたびにプレビューが消えるほうが邪魔だからです。
  - 隠している間は空白にせず、「前面の UI を閉じると表示に戻ります」と表示します。そうしないと、プレビューが消えたように見えます。
  - 隠す時に OS のキーボードフォーカスも necoder 側へ戻すので、**「パレットを開いたのに日本語が入力できない」問題も同時に直りました**。
- **設定の「外観」でラベルが 1 文字ずつ縦に折り返す問題**：テーマが 7 種類に増えてチップの合計幅が行に収まらなくなり、左の列だけが幅 0 まで潰れていました（空白のない日本語は 1 文字ずつ折り返します）。チップを行の幅いっぱいに置く、上下 2 段の入れ物に変えました。
- **`ne remote init` の行き止まり**：provisioning token がないまま「Initialized」とだけ表示して終わるので、ペアリングは必ず失敗していました。しかも GUI には「`ne remote init` を実行してください」と出るため、実行済みの人が同じところを回っていました。上の「アカウント不要化」で token そのものがなくなったので、この問題も解消しました。

## [0.1.16] - 2026-09-12

Started rebuilding Fleet. The entry point is now an `Editor | Fleet` segment at the left of the titlebar (⌘⇧M, with an Attention badge), and the left list now has one three-line row per Task, each showing **the original text of what you asked for**. The supervisor is renamed **Captain** (setting key `coordinator_agent` → `captain_agent`; the old key is not read). You can also click the rail and move between projects with ↑/↓.

### Added

- **Fleet entry in the titlebar**: An `Editor | Fleet` segment sits right of the project name and switches the whole surface (key: **⌘⇧M**). The small "Fleet" toggle at the top right is gone, because which surface you are on belongs next to which project you are in.
  - **Attention count badge on Fleet** (`◐ 2`): Pending approvals and failures catch your eye even while you are writing in Editor. Nothing is shown when the count is 0.
  - **Fleet icon on the rail**: Red when something needs attention, the project color while Fleet is showing. The setting is `rail.fleet`.
  - The moment you start a second Task, the hint "See them run side by side in Fleet (⌘⇧M)" appears once. That is the only hint; it never appears again.
- **Fleet's left list is now a list of Tasks**: It used to show thread rows under project headings, which made it hard to tell how many worktrees were running in parallel and what you had asked each one to do. Now **each Task is one row with three lines**.
  - Line 1: status + Task name + `⎇ branch` + tokens. Line 2: **`› what you asked for`** (the original text). Line 3: what it is doing now, or how it ended.
  - You read each row as "asked → did", so with N agents running in parallel you can **spot the one that drifted** at a glance. The instruction is your own words, so it doesn't get ✳ (the AI-generated mark). When a previous conversation is restored, the row is rebuilt from the last instruction you wrote.
  - At the top is the **Captain bar** (the assigned agent's name + its last decision in one line). **⌘0** takes you to that conversation. If no Captain is appointed, it says "Appoint a Captain" and takes you to Settings.
  - The integration target (`main`) isn't mixed in with Task rows. It gets its own line, `⌂ main · Integration · protected`, with "N branched · M integrated today" below it.
  - **Only Tasks in the repository selected on the rail** are shown. Activity in other repositories shows as dots on the rail.
- **Move between projects with ↑/↓ after clicking the rail**: Click anywhere on the rail (empty space works too) to move focus there, then use plain ↑/↓ with no modifier to go to the neighboring project. Escape or a click returns you to editing.

### Changed

- **Renamed the supervisor to Captain**: It is central to Fleet, so the name now fits the fleet metaphor. UI strings, the attribution name in News and the Captain thread's name all read `Captain` (the same word in Japanese and English).
  - **The setting key changes**: `coordinator_agent` → **`captain_agent`**. The old key is not read (edit your `settings.json`).
  - The rail icon setting also changes from `rail.herd` → **`rail.fleet`** (the old key is ignored).
- **⌘{ / ⌘} always switch tabs**: They used to turn into project switching right after you clicked the rail. That was a hidden mode where a key's meaning depended on what you had last clicked, so it is gone. Move between projects with ⌃⌘↑↓ or with the rail ↑/↓ above.
- **Fleet's default is back to the lineage graph plus the grid of cells**: The "Work" tab made default in 0.1.15 (a desk with worktrees in columns) turned out to be harder to use day to day than the old fleet grid. It is no longer the default and is now one of the center tabs, an optional alternate view. Choose "Work" in the center tabs to open it as before.
- **The + for adding cells is now three buttons** (+ ACP / + Terminal / + Worktree): They are always visible at the bottom of the grid, with where they will open shown next to them (project ⎇ branch; hover for the real path).
  - **+ ACP starts a separate conversation.** You can place any number of ACP cells on the same working directory, and none of them replaces an existing conversation. (Up to 0.1.15, + either always created a new worktree or added a tab to the same panel.)
  - **+ Terminal works the same way**, adding an independent shell cell in the chosen working directory.
  - **Use + Worktree only when you want to work in parallel on another branch.** Choose an existing branch, an existing worktree or a new branch name.
- **The original working directory (the `main` side) is now shown as a cell too**: The left list also shows everything by default instead of collapsing. Protecting the Git integration target is the job of Git-side gates, not of hiding it from the screen.
- **Removed the 8-cell limit on the grid**: Three cells sit in one row; from the fourth on they stack into a grid, and rows that don't fit scroll. Closing a cell keeps its contents (conversation, PTY) alive, and bringing it back from the list shows the same instance. Rearranging or expanding doesn't recreate the ACP session or PTY.

### Fixed

- **Unrelated projects showed up in Fleet**: A change meant to also show the current repository's `main` removed the filtering along with it, so every other repository on the rail became a cell. The fleet is now scoped to one repository. When you move to another repository, the cell layout (including the results of + and ×) is set aside and replaced with that repository's.
- **AI CLIs no longer run on every launch**: The check in Settings home for which agents are available ran every time, whether or not you opened Settings. It now runs **only when Settings home is actually shown**, so a normal launch doesn't spawn extra child processes.
- **Clicking the transcript could crash** (introduced in 0.1.15): When a conversation is too long to fit on screen, necoder draws only the visible range, and draws off-screen messages only to measure their height before discarding them. Click hit-testing included those discarded items, so clicking one that had no on-screen position crashed immediately. This also fixes selection mix-ups where any click selected the start of the last message and copied text came out in reverse order.
- **Esc didn't stop the turn while the cursor was in the composer**: The editor took Esc to collapse multiple cursors and swallowed it even when there was nothing to collapse, so the interrupt never reached the Agent panel and only the stop button worked. While renaming a thread or with a suggestion menu open, Esc still only closes that and doesn't stop a running turn.
- **Closed thread tabs came back on every restart**: Threads closed with the tab × or with ⌘W in the panel weren't archived in the ledger (DB), and restore on launch opens every thread that isn't archived, so all the closed tabs came back. The close paths are now merged into one. The opposite problem is fixed too: threads reopened with ⌘⇧T or from history disappeared on the next launch (they are now unarchived). Rows that already piled up will appear once on the next launch; close them there and they stay closed.

### 日本語

Fleet の作り直しを始めました。入口を titlebar 左の `Editor | Fleet` セグメント（⌘⇧M、要対応のバッジ付き）に置き、左の一覧を 1 行 1 Task の 3 段表示に替えて、各行に**自分が頼んだことの原文**を出します。「監督」は **Captain** に改名しました（設定キーは `coordinator_agent` から `captain_agent` へ。旧キーは読みません）。レールを押してから ↑/↓ でプロジェクトを移れるようにもなりました。

#### 追加

- **titlebar の Fleet の入口**：プロジェクト名の右隣に `Editor | Fleet` のセグメントを置き、押すと画面ごと切り替わります（キーは **⌘⇧M**）。右上にあった小さな「Fleet」トグルは廃止しました。いまどちらの画面にいるかは、どのプロジェクトにいるかの隣に出すべきだと考えたためです。
  - **Fleet 側の要対応の件数バッジ**（`◐ 2`）：Editor で書いている最中でも、承認待ちや失敗が出れば目に入ります。0 件の時は何も出しません。
  - **レールの Fleet アイコン**：要対応がある時は赤、Fleet を表示している時はプロジェクトの色になります。設定は `rail.fleet` です。
  - 2 本目の Task を作った時に、一度だけ「並走を Fleet で見る（⌘⇧M）」と出ます。案内はこの 1 回だけで、以後は出ません。
- **Fleet の左の一覧を Task の一覧に**：これまではプロジェクトの見出しの下にスレッドの行が並ぶ形で、並行して動いている worktree がいくつあるのか、それぞれに何を頼んだのかが読み取れませんでした。**1 行 1 Task の 3 段表示**に変えました。
  - 1 段目は状態、Task 名、`⎇ ブランチ`、トークンです。2 段目は **`› 自分が頼んだこと`**（原文）、3 段目はいま何をしているか、またはどう終わったかです。
  - 頼んだこと、やったことの順に読めるので、N 体を並行して動かしている時に**ずれているもの**がすぐ分かります。指示の原文は人が書いた言葉なので、✳（AI が生成した印）は付けません。前回の会話を復元した時も、最後に自分が書いた指示から表示し直します。
  - 一番上には **Captain バー**（担当エージェントの名前と、最後の采配 1 行）があります。**⌘0** でその会話に移れます。まだ任命していなければ「Captain を任命する」と出て、押すと設定に移ります。
  - 統合先（`main`）は Task の行に混ぜず、`⌂ main · 統合先 · 保護` の 1 行にまとめ、その下に「N 本が分岐中 · 今日 M 件統合」と出します。
  - 表示するのは、**レールで選んでいるリポジトリの Task だけ**です。ほかのリポジトリの動きはレールのドットで分かります。
- **レールを押して ↑/↓ でプロジェクトを移動**：レールのどこか（空いている所でも可）を押すとフォーカスがレールに移り、修飾キーなしの上下キーで隣のプロジェクトへ移れます。Escape かクリックで編集に戻ります。

#### 変更

- **「監督」を Captain に改名**：Fleet の中心になる機能なので、編隊のたとえに名前を合わせました。UI の文字列、ニュースの帰属名、Captain スレッドの名前が、すべて `Captain`（日英とも同じ語）になります。
  - **設定キーが変わります**：`coordinator_agent` → **`captain_agent`**。旧キーは読まないので、`settings.json` を書き換えてください。
  - レールのアイコン表示の設定も `rail.herd` → **`rail.fleet`** に変わりました（旧キーは無視します）。
- **⌘{ / ⌘} は常にタブの切り替え**：これまではレールを押した直後だけ、プロジェクトの切り替えに変わっていました。直前に何を押したかでキーの意味が変わる隠れたモードなので、やめました。プロジェクトの移動には ⌃⌘↑↓ か、上のレールの ↑/↓ を使ってください。
- **Fleet の既定を系譜グラフとセルのグリッドに戻す**：0.1.15 で既定にした「作業」タブ（worktree を列に並べる机）は、毎日使ってみると以前の編隊グリッドより扱いにくいと分かりました。既定から外し、中央のタブの 1 つ（任意で選ぶ別の表示）にしました。中央のタブで「作業」を選べば、これまでどおり開きます。
- **セルを足す ＋ を 3 つのボタンに**（＋ ACP / ＋ Terminal / ＋ Worktree）：グリッドの下に常に表示し、押した時にどこで開くかを隣に出します（プロジェクト ⎇ ブランチ。ホバーで実際のパス）。
  - **＋ ACP は独立した会話を作ります**。同じ作業ディレクトリに ACP のセルをいくつでも並べられ、既存の会話を置き換えません（0.1.15 までの ＋ は、必ず新しい worktree を作るか、同じパネルにタブを足すかのどちらかでした）。
  - **＋ Terminal も同じ**で、選んだ作業ディレクトリに独立したシェルのセルを足します。
  - **別のブランチで並行して作業したい時だけ ＋ Worktree** を使います。既存のブランチ、既存の worktree、新しいブランチ名から選べます。
- **元の作業ディレクトリ（`main` 側）もセルとして表示**：左の一覧も、既定で畳まずにすべて表示します。Git の統合先を守るのは Git 側の gate の役目で、画面から消すことではないからです。
- **グリッドの 8 枚の上限を撤廃**：3 枚までは横 1 列、4 枚目からはグリッドに積み、入りきらない行はスクロールします。セルを閉じても中身（会話と PTY）は動いたままで、一覧から戻せば同じものが出ます。配置を変えても拡大しても、ACP と PTY は作り直しません。

#### 修正

- **Fleet に関係のないプロジェクトが並ぶ問題**：いまのリポジトリの `main` も並べるための変更で、絞り込みまで一緒に外してしまい、レール上のほかのリポジトリが 1 つずつセルになっていました。編隊をリポジトリ単位に閉じ、別のリポジトリへ移った時はセルの並び（＋ や × の結果も含む）を畳んで、移った先のものに差し替えます。
- **起動のたびに AI CLI を実行していた問題**：設定ホームに出す「どのエージェントが使えるか」の確認が、設定を開くかどうかに関係なく毎回動いていました。**設定ホームを実際に表示した時だけ**確認するようにしたので、普段の起動で子プロセスが増えません。
- **transcript をクリックすると落ちることがある問題**（0.1.15 で混入）：会話が画面に収まらない長さになると、necoder は見えている範囲だけを描き、画面外の発言は高さを測るためだけに描いて捨てます。クリック位置の判定がこの捨てた分まで対象にしていたため、画面上の位置を持たないものに当たると落ちていました。あわせて、どこを押しても一番下の発言の先頭が選ばれる問題と、コピーした文が上下逆につながる問題も直しました。
- **入力欄にカーソルがある時に Esc でターンを止められなかった問題**：エディタが Esc を「複数カーソルを畳む」操作として受け取り、畳むものがない時もそこで Esc を握りつぶしていました。そのため中断が Agent パネルまで届かず、停止ボタンでしか止められませんでした。スレッド名を変えている間と候補メニューを開いている間は、これまでどおり Esc はそれを閉じるだけで、動いているターンは止めません。
- **閉じたスレッドのタブが再起動のたびに戻る問題**：タブの × やパネル内の ⌘W で閉じたスレッドが、台帳（DB）でアーカイブされていませんでした。起動時の復元は「アーカイブされていないスレッドを全部開く」ので、閉じたはずのタブがそろって戻っていました。閉じる経路を 1 つにまとめて直しました。逆に、⌘⇧T や履歴から開き直したスレッドが次の起動で消える問題も直しました（アーカイブを解除するようにしました）。すでに溜まっている分は次の起動で一度だけ出てくるので、そこで閉じ直せば以後は戻りません。

## [0.1.15] - 2026-09-11

Threads can now use MCP servers, and you can pick a model right after launch. Also new: a "Work" tab that shows worktrees side by side, a PDF tab, and different notification sounds for different moments. The phone pairing QR code always failed because the control IPC never started in release builds; that is now fixed at the root.

### Added

- **Pass MCP servers to threads**: In ACP, choosing which MCP servers to connect to is the editor's job, and necoder had been passing none. That is why servers registered in Codex CLI or Claude Code showed up as `not installed or connected` in necoder threads.
  - Servers under `mcp_servers.<name>` in `settings.json` are passed (`command` = stdio / `url` = http, sse; `${VAR}` is expanded in headers and environment variables).
  - Servers already registered in `~/.codex/config.toml` / `~/.claude.json` / `~/.cursor/mcp.json` **also show up in the list**, but **they are off by default**. necoder shouldn't spawn child processes or connect to billed remote services just because another tool's config mentions them (turn each one on once in Settings; `docs/DECISIONS.md` §8).
  - New "MCP servers" section in Settings (source label + transport + toggle). Changes apply from the next thread you open.
  - Servers that can't be passed aren't silently dropped; the transcript shows the reason in one line (a transport the agent doesn't advertise, a local stdio server in a remote session, or an unset `${VAR}`).
- **Pick a model right after launch**: For the active thread only, necoder opens the ACP session ahead of time (after 500ms) instead of waiting for the first send. The model, reasoning effort and permission mode lists are there before your first message, ready to pick from. Turn it off with the "Connect agents up front" setting (`agent_prewarm`, on by default).
  - At most **two** sessions are kept open without ever being used. Flipping through tabs doesn't add a process for every tab you looked at (an unused session measured about 16MB; the oldest are closed first, and coming back resumes with `session/load`).
- **"Work" tab in Fleet**: Open one repository as a desk and see its worktrees side by side in columns (`main` can sit in a column as a worktree too).
  - The **work tree** on the left has **three levels** (repository → worktree → surface). Clicking a thread row switches to that column's Agent, so the tree doubles as a thread switcher.
  - Each pane has one tab row. **Tab position is set per pane**: top or left (with vertical tabs, the identifying color moves from the underline to a left bar). The default comes from the `work_tabs_position` setting.
  - Moving a bottom-dock terminal into the work surface keeps **the same PTY running** (and you can move it back). Closing a column keeps its sessions alive, and reopening restores the tab layout. The layout is saved in the window session.
- **Open PDFs in a tab**: necoder has no renderer of its own and lets the OS viewer draw it (PDFKit on macOS, WebView2 on Windows). PDFs over Remote SSH are copied to a local temporary file for display, and **the copy is deleted when you close the tab** (remote content isn't left on your machine). Linux gets a fallback view (right-click → "Open in default app").
- **Answer the agent's multiple-choice questions** (AskUserQuestion): These forms used to be rejected outright, so to the agent it looked as if you hadn't answered. **Multi-select questions work too** (each click toggles an option; "Submit" confirms). The Remote PWA accepts multi-select as well.
- **Two notification sounds**: One plays when a turn ends, another when the agent **stops to wait for your input** on an approval or a question. You can tell "finished" from "you're needed" by ear alone. The sounds are cat meows bundled with necoder (synthesized with `scripts/gen-chime.py`).
  - The waiting sound plays when **that thread isn't visible**: not only while you are in another app, but also while you are looking at another tab in the same window. It doesn't play when the thread stops right in front of you, for requests passed through by auto-approve, or while muted.

### Changed

- The "Completion sound" setting is now a **choice of sound** instead of on/off (Meow / System / Off). "Waiting-for-you sound" works the same way. Put a path to an audio file in `settings.json` to play your own sound (keys `sound_done` / `sound_waiting`; the old `completion_sound` is removed).
- Heredoc folding in the transcript now also covers **commands fed through stdin**, like `python3 - <<'PY'`. The command that ran stays in the header, and only the body goes into the expandable area. Commands after the heredoc also stay in the header.
- The Agent panel embedded in the Work tab collapses its chrome (it draws no thread tab row of its own, and puts state and tokens on one meta line). **The token display is never collapsed.**

### Fixed

- **The control IPC (`~/.necoder/gui.sock`) never started in release builds**: The call sat inside a development-only block (`#[cfg(debug_assertions)]`), so the phone pairing QR code always failed. The `ne` command also couldn't find the running window and opened the app through LaunchServices every time.
- When generating the QR code failed, the same message (about the Node.js version and so on) appeared whatever the cause. It now shows guidance for each cause (not initialized, device limit reached, host not bundled, relay, timeout and so on).
- The GUI now recovers if it loses the socket (it checks the owner every 30 seconds and binds again).
- **The model name stayed "—" right after launch** (regression in 0.1.14): If you had never picked a model yourself, the pill was empty until the first send. It now starts from the last recorded model and switches to the display name once the prewarmed session advertises its models.
- **Closed the last two ways closed tabs came back after a restart**: (1) If one project failed to open (for example, a deleted worktree), every project after it received **the neighboring project's tab list**. (2) Closing the window discarded the last save, so "close a tab, then immediately ×" was undone (the × in the custom titlebar, that is, Windows / Linux).
- **Japanese input stopped working in the agent panel after opening an HTML preview**: The native WebView kept OS keyboard focus, and the IME composition session was attached to it (keys that don't go through the IME still worked, so only the keymap seemed to be alive). Focus is now taken back as soon as you click outside the preview. PDF tabs go through the same path.
- **Clicking a PDF did nothing**: necoder tried to read it as text and failed silently.
- **Settings and other tabs couldn't be opened in AI full screen** (they did open, but the center stayed on the Agent, so it looked like nothing happened). Actions that bring the editor forward, such as opening a file, ⌘, or the ⚙ on the rail, now exit full screen.
- **A long "current question" pinned at the top pushed the agent's answer out of view**: A multi-line question now collapses to its first line, and ▸ expands it. Even expanded, the height is capped and any overflow scrolls inside the strip.
- In Settings, rows with a long subtitle (such as an MCP stdio command) squeezed the toggle down to zero width.
- On Remote SSH, the first agent discovery right after reconnecting failed and fell back to the `npx` path. Read-only commands are now retried to prevent this.

### Notes

- MCP changes apply **from the next thread you open** (running sessions aren't reconnected). Servers found in other tools are off by default, so turn on the ones you want once in Settings.
- Notification sounds currently play on macOS only (Windows comes in the W phase). If you had set `completion_sound`, rewrite it as `sound_done` (the old key is ignored).
- PDF tabs work on macOS and Windows only (they use the OS viewer).
- Because agents are started up front, just opening and looking at tabs can start up to two sessions (about 16MB each while unused). To turn this off, disable "Connect agents up front" in Settings.

### 日本語

スレッドから MCP サーバを使えるようにし、起動直後からモデルを選べるようにしました。worktree を横に並べて見る「作業」タブ、PDF タブ、場面ごとに違う通知音も追加しました。スマホ連携の QR が必ず失敗していた不具合は、配布ビルドで管制 IPC が一度も起動していなかったことが原因で、これを根本から直しました。

#### 追加

- **スレッドへの MCP サーバの受け渡し**：ACP では、どの MCP サーバに繋ぐかを決めるのはエディタ側の役目ですが、necoder はこれまで 1 つも渡していませんでした。Codex CLI や Claude Code に登録したサーバが、necoder のスレッドからは `not installed or connected` になっていたのはこのためです。
  - `settings.json` の `mcp_servers.<名前>` に書いたサーバを渡します（`command` なら stdio、`url` なら http か sse。ヘッダと環境変数の `${VAR}` を展開します）。
  - `~/.codex/config.toml` / `~/.claude.json` / `~/.cursor/mcp.json` に登録済みのサーバも**一覧に出ます**。ただし**既定はオフ**です。ほかのツールの設定を理由に、勝手に子プロセスを起動したり、課金されるリモートに繋いだりしないためです（設定画面で 1 回オンにすれば使えます。`docs/DECISIONS.md` §8）。
  - 設定画面に「MCP サーバ」セクションを追加しました（出所のラベル、伝送方式、トグル）。変更は次に開くスレッドから効きます。
  - 渡せなかったサーバは黙って外さず、transcript に理由を 1 行出します（エージェントが対応を広告していない伝送方式、リモートセッションでのローカルの stdio、未設定の `${VAR}`）。
- **起動直後のモデル選択**：アクティブなスレッド 1 本についてだけ、送信を待たずに ACP のセッションを先に立てます（500ms 後）。モデル、思考量、権限モードの一覧が最初の送信の前から出て、押して選べます。設定の「エージェントを先に起動」（`agent_prewarm`、既定はオン）で切れます。
  - 立てたまま一度も使っていないセッションは **2 本まで**です。タブを次々に見ても、見た数だけプロセスが増えることはありません（未使用のセッションは実測で約 16MB。古いものから閉じ、戻った時は `session/load` で続きから再開します）。
- **Fleet の「作業」タブ**：リポジトリ 1 つを机として開き、worktree を列に並べて同時に見られます（`main` も 1 つの worktree として同じように列に置けます）。
  - 左の**作業ツリーは 3 段**です（リポジトリ → worktree → 面）。スレッドの行をクリックすると、その列の Agent に移って切り替わります。ツリーがそのままスレッドの切り替えに使えます。
  - ペインの中のタブ行は 1 本です。**タブの位置はペインごと**に上か左を選べます（縦タブの時、識別色は下線ではなく左のバーに出ます）。既定値は設定の `work_tabs_position` で決まります。
  - 下のドックのターミナルを作業面へ移しても、**同じ PTY が動き続けます**（行き来できます）。列を閉じてもセッションは残り、次に開くとタブの配置ごと戻ります。配置は窓のセッションに保存します。
- **PDF をタブで開く**：自前のレンダラは持たず、OS のビューア（macOS は PDFKit、Windows は WebView2）で表示します。Remote SSH 越しの PDF は手元に一時的に複製して表示し、**タブを閉じると複製を消します**（リモートの中身を手元に残しません）。Linux では代わりの表示になります（右クリック →「既定アプリで開く」）。
- **エージェントの選択肢付きの質問への回答**（AskUserQuestion）：これまではフォームごと拒否していたため、エージェントからは「ユーザーが答えなかった」ように見えていました。**複数選択の質問にも対応しました**（押すたびに選択が切り替わり、「これで回答」で確定します）。Remote PWA でも複数選択で答えられます。
- **2 種類の通知音**：ターンが終わった時と、承認や質問で**あなたの入力を待って止まった**時とで、違う音を鳴らします。耳だけで「終わった」と「呼ばれている」を聞き分けられます。音は necoder に同梱した猫の鳴き声です（合成音で、`scripts/gen-chime.py` で作っています）。
  - 入力待ちの音は、**そのスレッドが見えていない時**に鳴ります。ほかのアプリを使っている時だけでなく、同じ窓で別のタブを見ている時も鳴ります。目の前で止まった時、自動承認で通った要求、ミュート中は鳴りません。

#### 変更

- 設定の「完了音」を、オン/オフから**音の選択**に変えました（にゃー / システム音 / オフ）。「入力待ちの音」も同じように選べます。`settings.json` に音声ファイルのパスを書けば、自分で用意した音も鳴らせます（キーは `sound_done` / `sound_waiting`。旧 `completion_sound` は廃止）。
- transcript の heredoc の畳み込みを、`python3 - <<'PY'` のように**標準入力へ流し込んで実行する**場合にも広げました。実行したコマンドは見出しに残し、本文だけを展開できる領域に入れます。heredoc の後に続くコマンドも見出しに残します。
- 作業タブに埋め込んだ Agent パネルは、周りの表示を畳みます（自前のスレッドタブ行を描かず、状態とトークンを 1 行のメタ情報にまとめます）。**トークン表示は畳みません**。

#### 修正

- **配布ビルドで管制 IPC（`~/.necoder/gui.sock`）が一度も起動していなかった問題**：呼び出しが開発用のブロック（`#[cfg(debug_assertions)]`）の中にあったため、スマホ連携の接続用 QR は必ず失敗していました。`ne` コマンドも起動中の窓を見つけられず、毎回 LaunchServices 経由でアプリを開いていました。
- QR の発行に失敗した時、原因に関係なく同じ文言（Node.js のバージョンなど）が出ていました。原因ごとに違う案内を出すようにしました（未初期化、端末数の上限、ホストが同梱されていない、中継サーバ、タイムアウトなど）。
- GUI が socket を見失っても復帰できるようにしました（30 秒ごとに持ち主を確かめて張り直します）。
- **起動直後にモデル名が「—」のままだった問題**（0.1.14 の退行）：自分でモデルを選んだことがないと、最初の送信までピルが空でした。記録してある最後のモデルを出し、先に立てたセッションがモデルの一覧を知らせてきたら、表示名に置き換えます。
- **閉じたタブが再起動で戻る問題の残り 2 経路を塞ぎました**：①開けなかったプロジェクト（消えた worktree など）が 1 つ混ざると、それ以降のすべてのプロジェクトが**隣のプロジェクトのタブ列**を受け取っていました。②窓を閉じると直前の保存が捨てられ、「タブを閉じてすぐ ×」が無かったことになっていました（自前のタイトルバーの ×、つまり Windows と Linux）。
- **HTML プレビューを開くと、エージェントの入力欄で日本語を打てなくなる問題**：ネイティブの WebView が OS のキーボードフォーカスを持ったままで、IME の変換セッションがそちらに付いていました（変換を通さないキーは効くので、キー操作だけは動いているように見えていました）。プレビューの外をクリックした時点でフォーカスを取り戻します。PDF タブも同じ仕組みで直ります。
- **PDF をクリックしても何も起きなかった問題**：テキストとして読もうとして、何も言わずに失敗していました。
- **AI 全画面の間、設定タブやほかのタブを開けなかった問題**：開いても中央が Agent のままで、何も起きないように見えていました。ファイルを開く、⌘,、レールの ⚙ など、エディタを前に出す操作で全画面を閉じるようにしました。
- **上に固定した「いまの問い」が長いと、エージェントの回答を押し出していた問題**：複数行の問いは先頭の 1 行に畳み、▸ で開けるようにしました。開いても高さには上限があり、はみ出した分は帯の中でスクロールします。
- 設定画面で副題が長い行（MCP の stdio コマンドなど）が、トグルを幅 0 まで潰していた問題を直しました。
- Remote SSH で再接続した直後、最初のエージェントの探索が失敗して `npx` の経路に切り替わってしまう問題を、読み取り専用のコマンドを送り直すことで防ぎました。

#### 利用上の注意

- MCP の変更は**次に開くスレッドから**効きます（動いているセッションは繋ぎ直しません）。ほかのツールから見つけたサーバは既定でオフなので、使いたいものは設定画面で一度オンにしてください。
- 通知音はいまのところ macOS でだけ鳴ります（Windows は W フェーズで対応します）。`completion_sound` を設定していた場合は、`sound_done` に書き直してください（旧キーは無視します）。
- PDF タブは macOS と Windows でだけ使えます（OS のビューアを使うため）。
- エージェントを先に起動するため、タブを開いて見るだけでもセッションが最大 2 本立ちます（未使用なら 1 本あたり約 16MB）。止めたい場合は、設定の「エージェントを先に起動」をオフにしてください。

## [0.1.14] - 2026-09-10

This release adds the Remote PWA, which lets you control your existing work from a phone. It also fixes Remote SSH reconnects, recovers from expired agent sign-ins, keeps your model choices, and makes long inputs easier to read.

### Added

- **Phone pairing QR in Settings**: Open the gear menu → Phone connection to show a QR code, copy the URL, or issue a new one. The QR hides itself after 5 minutes.
- **Remote PWA for phones**: At `control.necoder.com` you can view and control the existing ACP threads on your Mac or Windows machine. It supports QR pairing, end-to-end encryption, allow once / deny for permission requests, multiple-choice questions, Git diffs, notifications, and resyncing after a disconnect. Manage connections and revoke devices with `ne remote` (requires Node.js 22+). Setup and limits are in `relay/README.md`.
- **Long user input folds in the transcript**: Input longer than 12 lines (or 1000 bytes) shows only its first 8 lines by default. Switch to the full text with the `▸ Show all N lines` chip below it or the fold button at the top right of the entry. Pasted logs or specs no longer fill the thread with your own input and push the agent's reply off screen.
- **A dedicated card for an expired agent sign-in**: Instead of a generic Internal error, necoder shows the command to sign in again, plus "Reconnect and resend" and "Close". While the sign-in is expired, queued messages are not sent automatically.

### Changed

- **Model, thinking effort, and permission mode are remembered by ACP value ID**: They are saved per agent in `agent_config_defaults`, so a difference in how a display name is written no longer drops you back to the default model. Restored threads also get your saved choices.
- **The Remote host ships with the Mac and Windows builds**: On Windows it uses a named pipe with an ACL limited to your user. Issuing a QR runs in the background, so the Settings screen stays responsive.
- **Bigger action buttons on transcript entries**: The copy button `⧉` grows from 22px to **28px** square (the glyph from 12.5px to 15px, and the background now changes on hover). The fold button sits to its left.

### Fixed

- **Remote SSH reconnect failed forever, from both Resume and sending**: After a disconnect, re-creating the master failed to bind `-R` because the previous `ne` gateway socket (`/tmp/necoder-cli-….sock`) was still on the remote, and `ExitOnForwardFailure=yes` then took the whole master down with exit 255 (the client-side `StreamLocalBindUnlink` does not apply to `-R`). That is why it only recovered when you reopened a terminal: the standalone session created a master without `-R`. The master now starts without `-R`, and the gateway forward is added afterward with `ssh -O forward`, after removing the stale socket through the master. If the forward fails, only `ne` stops working; the connection stays up.
- **SSH failures were misreported as "claude-agent-acp not found"**: Agent lookup on the remote now tells "searched and not found" apart from "could not search" (for example, the reconnect failed). For the latter, the error line shows the actual cause.
- **The first lookup right after a reconnect failed and fell back to npx**: `command -v` is read-only, so it is now sent as safe to retry (`Host::run_command_retry_safe`).
- **An expired sign-in could lose the conversation context**: An auth error alone no longer discards the session; necoder lets the agent resume it. The send path is rebuilt only when the session can no longer be resumed ("session has ended").

### Usage notes

- Remote needs **Node.js 22 or later and a one-time host setup**. Installing the app does not by itself connect every user to the shared relay. [Setup guide](https://github.com/iKora128/necoder/blob/v0.1.14/relay/README.md)
- Issuing a QR shares every project open at that moment. On the phone you switch between projects and threads and control each one separately. Nothing can be controlled while the PC is asleep, powered off, or offline. Commands whose result is unconfirmed and old approvals are never resent automatically.
- The QR is valid for 5 minutes. A link scanned with the camera opens in the regular browser. If the Home Screen PWA does not pick up the connection, paste a new pairing URL into the PWA. Notifications on iPhone require adding the PWA to the Home Screen and allowing notifications.
- Settings in the old `agent_defaults` / `default_model` / `default_effort` format do not carry over. After updating, check your model and other choices, and pick them again if needed.
- Tested on Chromium / WebKit, the production relay, and the Mac GUI IPC. Notifications on a real iPhone and GUI integration on a real Windows machine still need checking. Remote has not had an independent security audit.

### 日本語

スマホから既存の作業を操作できる Remote PWA を追加しました。SSH の再接続を直したほか、エージェントの認証が切れても復帰できるようにし、モデルなどの選択が保たれるようにし、長い入力を読みやすくしました。

#### 追加

- **設定画面からのスマホ連携の QR 発行**：歯車 → スマホ連携で、QR の表示、URL のコピー、再発行ができます。QR は 5 分たつと自動で隠れます。
- **スマホ向けの Remote PWA**：`control.necoder.com` から、Mac や Windows で動いている既存の ACP スレッドを見たり操作したりできます。QR でのペアリング、端末間の暗号化、その回だけの許可と拒否、選択式の質問への回答、Git diff、通知、切断後の再同期に対応しています。接続と端末の失効は `ne remote` で管理します（Node.js 22 以降が必要です）。セットアップ方法と制約は `relay/README.md` にあります。
- **トランスクリプトでの長いユーザー入力の折り畳み**：12 行（または 1000 バイト）を超える入力は、最初は先頭 8 行だけを表示します。本文の下の `▸ 全 N 行を表示` チップか、エントリ右上の折り畳みボタンで全文に切り替えられます。貼り付けたログや仕様書で自分の入力がスレッドを埋め、エージェントの応答が画面の外へ押し出されることがなくなりました。
- **エージェントの認証切れを知らせる専用カード**：ふつうの Internal error とは分けて、再ログインに使うコマンドと「再接続して再送」「閉じる」を表示します。認証が切れている間は、キューに積んだメッセージを自動で送りません。

#### 変更

- **モデル、思考量、権限モードの記憶**：ACP の値 ID で覚えるようにしました。エージェントごとに `agent_config_defaults` へ保存するので、表示名の書き方が違うだけで既定のモデルに戻ってしまうことがなくなります。復元したスレッドにも保存済みの選択を適用します。
- **Remote ホストの同梱**：Mac と Windows の配布物に Remote ホストを入れました。Windows では名前付きパイプと、自分のユーザーだけに絞った ACL を使います。QR の発行は裏で行うので、設定画面の操作は止まりません。
- **トランスクリプトのエントリの操作ボタン**：大きくしました。コピーボタン `⧉` は 22px 角から **28px 角** になりました（グリフは 12.5px から 15px に。ポインタを載せると背景も変わります）。折り畳みボタンはその左に並びます。

#### 修正

- **Remote SSH の再接続**：「再開」からも送信からも失敗し続けていました。切断後に master を張り直すとき、remote に残っていた前回の `ne` gateway socket（`/tmp/necoder-cli-….sock`）のせいで `-R` の bind が失敗し、`ExitOnForwardFailure=yes` によって master ごと exit 255 で終了していました（client 側の `StreamLocalBindUnlink` は `-R` には効きません）。ターミナルを開き直したときだけ直っていたのは、standalone session が `-R` なしで master を作っていたからです。master は `-R` なしで張り、gateway の転送は master 経由で古い socket を消してから `ssh -O forward` で後から足すようにしました。転送に失敗しても `ne` が使えなくなるだけで、接続は切れません。
- **SSH 側の失敗の誤報**：「claude-agent-acp が見つかりません」と誤って表示していました。remote でのエージェント探索で「探したが無かった」と「探せなかった（再接続の失敗など）」を区別し、後者は原因の文をそのままエラー行に出します。
- **再接続直後のエージェント探索**：最初の探索が失敗して npx 経由に切り替わっていました。`command -v` は読み取りだけの命令なので、送り直してよい要求として送ります（`Host::run_command_retry_safe`）。
- **認証切れで失われる会話の文脈**：認証エラーだけではセッションを破棄せず、エージェント側の再開に任せるようにしました。「session has ended」で再開できなくなったときだけ送信路を作り直します。

#### 利用上の注意

- Remote を使うには **Node.js 22 以降と、ホストの初回の接続設定** が必要です。アプリを入れただけで誰でも共有 relay につながるわけではありません。[セットアップ手順](https://github.com/iKora128/necoder/blob/v0.1.14/relay/README.md)
- QR を発行した時点で開いているプロジェクトをすべて共有します。スマホでプロジェクトやスレッドを切り替えて、それぞれを操作します。PC がスリープ中のときや、電源や回線が切れているときは操作できません。結果を確認できていない命令や古い承認を、自動で送り直すことはありません。
- QR の有効期限は 5 分です。カメラで読み取ったリンクは通常のブラウザで開きます。ホーム画面の PWA に接続情報が引き継がれないときは、新しいペアリング URL を PWA に貼り付けてください。iPhone で通知を受けるには、ホーム画面への追加と通知の許可が必要です。
- 古い `agent_defaults` / `default_model` / `default_effort` 形式の設定は、新しい設定に引き継がれません。更新後にモデルなどの選択を確認し、必要なら選び直してください。
- Chromium と WebKit、本番の relay、Mac の GUI IPC で動作を確かめました。実機の iPhone での通知と、Windows 実機での GUI 連携はまだ確認が必要です。Remote について、独立したセキュリティ監査はまだ受けていません。

## [0.1.13] - 2026-09-08

### Added

- **The SSH chip in the status bar shows the connection state**: The color of "SSH" tells you whether you are connected, connecting, disconnected, or not connected. When disconnected or not connected, a "Reconnect" or "Connect" text chip lets you reconnect right away, instead of waiting out the heartbeat's backoff of up to 60 seconds while the host is unreachable. State changes come from a subscription to the host and show up immediately.
- **Resume a dropped agent session**: When the connection to the agent closes (for example, because SSH dropped), the thread shows "Session lost" and a "Resume" chip above the composer, and the next send (or Resume) restarts the agent. If the agent advertises `loadSession`, necoder calls `session/load` with the previous session id, so you can continue the same conversation (a short note tells you whether it was carried over). The session id is stored per thread in the DB, so this also works on the first send after restarting the app.

### Changed

- **Remote I/O on the UI thread now panics in debug builds** (for developers): Release builds still warn and continue. The known violations are fixed, so new ones are caught right away in tests and during development.
- **Remote project colors are per project, not per host**: Projects on the same host no longer all share one color. As with local projects, each project gets its own color (the `host_colors` table in the DB is no longer used; leaving it there is harmless).

### Fixed

- **necoder used a full CPU core while connected over Remote SSH, even when idle**: After the file watch subscription was replaced and its channel closed, waiting returned an error immediately, which was treated as "no changes" and retried forever. The watch pump now exits on disconnect, and generation tracking plus serialization keep the cleanup of an old watch from unsubscribing the new one.
- **After SSH dropped, every send returned "Incoming transport closed"**: When the transport to the agent closed, necoder treated it as a failure of that turn only and kept the session, so every later send went to the dead connection and failed at once. A closed transport now tears down the whole session, and a disconnect while idle is detected without waiting for the next send (you no longer find out only when you send).
- **The first turn of a new thread ran with a different model or thinking effort than the pill showed** (for example, the pill said Opus but Fable replied): With lazy startup, the model change was queued behind the first prompt and not sent until the turn finished. As with the permission mode, the thread's model and thinking effort are now applied to the agent right after the session is created, before the first prompt (values the agent does not advertise still fall back to what the agent advertises).
- **`ne .` in the Remote SSH integrated terminal opened necoder on the remote Mac**: It now goes back to your machine over an SSH reverse Unix socket and passes the remote absolute path to the current window as a URI on the same SSH host. The local end of the reverse socket is an `open`-only gateway, so no other control IPC operations are exposed to the remote.
- **Remote SSH queries left on the UI thread**: All of them now run in the background. Adding a project to the rail (resolving the worktree and querying git), opening from the explorer in a new window or the current rail, split panes, hunk revert, hot exit restore, and launching with `necoder ssh://host/path` used to freeze for as long as a reconnect took (for example, right after waking from sleep). Launching with an `ssh://` argument now shows the window first, as restore does, and connects in the background.
- **Project colors changed after a restart or after reordering the rail**: Auto-assigned colors were not saved, and the palette cycled by position on the rail. The chosen color is now stored in necoder.db (`project_colors`, keyed by local/host plus the root path) and reused the next time the project opens. The order of precedence is `.necoder/settings.json` > DB > an unused palette color. Colors you pick by hand go into the same table.

### 日本語

#### 追加

- **ステータスバーの SSH チップ**：接続状態がわかるようにしました。「SSH」の色で、接続済み、接続中、切断、未接続を表します。切断中や未接続のときは「再接続」「接続」の文字チップからすぐに張り直せます（ホストに届かない間、heartbeat は最大 60 秒まで間隔を空けて再試行しますが、それを待つ必要はありません）。状態の変化は host からの購読で、すぐに反映されます。
- **切れたエージェントのセッションの再開**：SSH の切断などでエージェントとの接続が閉じたスレッドでは、composer のすぐ上に「セッションが切れました」と「再開」チップを出し、次の送信（または再開）でエージェントを立ち上げ直します。エージェントが `loadSession` を広告していれば、前回のセッション id で `session/load` を呼ぶので、同じ会話の続きから話せます（引き継げたかどうかは一言で表示します）。セッション id はスレッドごとに DB に残るので、アプリを再起動した後の最初の送信でも引き継げます。

#### 変更

- **UI スレッドからの remote I/O の検出**（開発者向け）：debug ビルドでは panic にしました。release ビルドはこれまでどおり警告を出して続行します。既知の違反はすべて直したので、新しい違反はテストや開発中にすぐ見つかります。
- **リモートプロジェクトの色**：ホストごとからプロジェクトごとにしました。同じホストのプロジェクトがすべて同じ色になっていたのをやめ、ローカルと同じく 1 プロジェクトに 1 色を割り当てます（DB の `host_colors` は使わなくなります。残っていても害はありません）。

#### 修正

- **Remote SSH 接続中の CPU 使用率**：何もしていなくても necoder が CPU を 1 コア使い切っていました。ファイル監視の購読が入れ替わってチャネルが閉じた後、待機がすぐにエラーを返し、それを「変更なし」とみなして再試行し続けていたためです。切断したら監視の pump を終えるようにし、古い監視の後始末が新しい購読を解除しないよう、世代の管理と直列化も加えました。
- **SSH 切断後の「Incoming transport closed」**：送信するたびにこのエラーが返っていました。エージェントとの transport が閉じても、そのターンだけの失敗として扱ってセッションを残していたため、以後の送信が死んだ接続に流れてすぐエラーになっていたのが原因です。transport が切れたらセッションごと閉じ、待機中の切断も次の送信を待たずに検知します。送信して初めて気づく、ということはなくなりました。
- **新しいスレッドの最初のターンのモデル**：ピルの表示と違うモデルや思考量で動いていました（Opus と表示しているのに Fable が応答する、など）。遅延起動では、モデル変更の指示が最初の prompt の後ろに並び、ターンが終わるまで送られていませんでした。権限モードと同じく、セッションを作った直後、最初の prompt より前に、スレッドのモデルと思考量をエージェントに合わせるようにしました（エージェントが広告していない値のときは、これまでどおりエージェントが広告している値を使います）。
- **Remote SSH の統合ターミナルでの `ne .`**：接続先の Mac の necoder を開いていました。SSH の reverse Unix socket で接続元に戻し、リモートの絶対パスを同じ SSH host の URI として今のウィンドウに渡すようにしました。reverse socket の接続元側には `open` 専用の gateway を置いたので、管制 IPC のほかの操作は remote に公開しません。
- **UI スレッドに残っていた Remote SSH の問い合わせ**：すべて裏へ移しました。プロジェクトをレールに追加する（worktree の解決と git への問い合わせ）、エクスプローラから別ウィンドウや今のレールに開く、ペインの分割、hunk の巻き戻し、hot exit の復元、`necoder ssh://host/path` での起動は、再接続が必要な場面（スリープから復帰した直後など）で、その待ち時間だけ固まっていました。`ssh://` 引数での起動は、復元と同じく先にウィンドウを出し、接続は裏で追いつきます。
- **プロジェクトの色の変化**：再起動やレールの並べ替えで、プロジェクトの色が変わっていました。自動で割り当てた色を保存せず、レール上の位置でパレットを順に回していたのが原因です。決めた色を necoder.db（`project_colors`。local かホストの鍵と、ルートのパスで引きます）に保存し、次に開くときはそれを使います。優先順は `.necoder/settings.json` > DB > まだ使っていないパレットの色です。手動で選んだ色も同じ表に書き込みます。

## [0.1.12] - 2026-09-07

This release fixes the root cause of Remote SSH freezing after waking from sleep. Everything that waited on an SSH round trip on the UI thread (switching projects, startup, the explorer, checking for external changes, and starting terminals, LSP, and agents) now runs in the background, and dropped connections are detected and re-established faster. It also fixes the editor placing the caret away from where you clicked, and adds the standard Mac editing keys that were missing.

### Added

- **Standard Mac text editing keys** (in both the editor and the AI input box, and listed in the shortcut list):
  - Select: ⇧⌘← / ⇧⌘→ and ⇧Home / ⇧End to the start / end of the line, ⇧⌘↑ / ⇧⌘↓ to the start / end of the document
  - Delete: ⌘⌫ to the start of the line, ⌃K to the end of the line (at the end of a line it joins the next line), ⌥Delete for the next word
  - Emacs-style: ⌃N / ⌃P / ⌃F / ⌃B for up / down / right / left, ⌃D for forward delete, ⌃H for Backspace
  - On Windows they follow VS Code's bindings (Ctrl+Shift+Home / End, Ctrl+Delete, and so on), with ⌘⌫ / ⌃K as Ctrl+Shift+Backspace / Ctrl+Shift+Delete. Emacs-style keys that would conflict are not bound.
- **A real-machine regression test for Remote SSH** (for developers): `scripts/test-remote-ssh-docker.sh` builds the Linux remote-server, starts an SSH server in Docker, and injects a disconnect where TCP stays up but the other side goes silent. Using the same shape as waking from sleep (`docker compose pause`), it locks in that necoder reconnects on its own and can read again without the user doing anything.

### Changed

- **Update checks every 6 hours**: Before, necoder checked only once, 10 seconds after launch, so if you kept it open you did not see later versions until you reopened it. Elapsed time is measured by the wall clock, so time spent asleep counts. A new notice does not replace a download in progress or a pending restart.
- **Tool output in the AI transcript keeps the first 100 and last 100 lines** (the middle becomes "… (N lines omitted)"): Before, only the last 24 lines were kept, so you always lost either the top of a `Read` or the end of a `cargo build`.
- **Remote I/O on the UI thread is detected at runtime** (for developers): By default it prints a warning to stderr and continues. `NECODER_STRICT_MAIN_THREAD_IO=1` makes it panic, and `NECODER_ALLOW_MAIN_THREAD_IO=1` turns the check off. It exists to catch code that breaks the rule "call blocking APIs in the background".

### Fixed

- **The editor placed the caret away from where you clicked**: Hit testing measured widths with a different font from rendering (the OS default proportional font). On markdown lines mixing Japanese and ASCII, the caret landed about three full-width characters away from the click, and drag selection, hover, and the IME candidate position were off by the same amount. Hit testing now measures with the same run layout used for rendering, including the typefaces for headings, bold, and emphasis.
- **Switching projects over Remote SSH froze the app for tens of seconds**: The switch handler called the remote host's file listing, tab restore, and change watching synchronously from the UI thread. When a reconnect was needed (such as right after waking from sleep), that wait turned directly into time you could not do anything. This work now runs in the background: the destination chip updates immediately, and the ＋context file candidates and tabs fill in as they load.
- **With a Remote SSH project open, startup waited for the connection before showing a window**: If you quit with a remote project open, the next launch showed no window until connecting finished (including the wait for any unreachable host). necoder now builds the rail and tabs from the last saved state and shows the window first, while connecting and loading content catch up in the background. Unreachable hosts are reported in a toast, and necoder tries to reconnect when you interact with them.
- **Reconnecting over Remote SSH after sleep took too long**: `ssh -O check` only looks at whether the multiplexing master is alive, and it reports a master whose TCP connection has died as alive, so connections on top of it hung silently. The handshake now gives up after 10 seconds and the master is re-created, and the retry interval for unreachable hosts widens from 5 seconds up to 60 seconds.
- **Remote SSH took up to 30 seconds to notice a dropped connection**: The liveness check (Ping) timeout is now 5 seconds. Once a reconnect is decided, requests on that connection fail immediately and are retried on the new one (before, they waited for each request's own timeout). If data is still arriving when the timer runs out, the connection is treated as busy, not dead, and is kept, so a large file transfer on a slow link is not cut off and resent over and over.
- **Remote SSH got slower the more you did while disconnected**: A reconnect to an unreachable host takes over ten seconds each time, yet every waiting operation started its own reconnect from scratch. Operations that were waiting now receive the failure that happened meanwhile as is, and only operations that come later (that is, the user retrying) try to connect.
- **A Remote SSH handshake that closed immediately took terminals down with it**: On an immediate EOF (not a timeout), necoder also re-created the ControlMaster and cut off the terminals on the same connection. A startup failure on the remote-server side no longer touches the master.
- **Remote SSH froze on every explorer action**: Expanding, collapsing, and renaming folders, file watch events, manual refresh (↻), and switching branches each stalled for an SSH round trip. Tree reads now run in the background, and collapsing a folder or expanding an already loaded one shows immediately.
- **Remote SSH external-change checks and terminal / LSP / agent startup waited on the connection on the UI thread**: Checking and reloading an open file that changed outside necoder, and fetching HEAD when an agent turn starts or ends, now run in the background too. SSH sessions for terminals and LSP connect on their own when there is no master (`ControlMaster=auto`), so building the launch spec no longer touches the network.
- **Folded summaries in the AI transcript showed too few lines**: The N in "▸ N lines" was counted after truncation, so 1000 lines of output showed as "24 lines" and expanding it did not show more. It now shows the line count before truncation.

### 日本語

Remote SSH で、スリープから復帰した後に固まる問題を根本から直しました。プロジェクトの切り替え、起動、エクスプローラ、外部変更の照合、ターミナルや LSP やエージェントの起動のうち、UI スレッドで SSH の往復を待っていた処理をすべて裏へ移し、切れた接続の検知と張り直しも速くしました。あわせて、エディタでクリックした位置とキャレットがずれる問題を直し、足りなかった Mac 標準の編集キーを追加しました。

#### 追加

- **Mac 標準のテキスト編集キー一式**：エディタと AI の入力欄の両方で使えます。ショートカット一覧にも載せました。
  - 選択：⇧⌘← / ⇧⌘→ と ⇧Home / ⇧End で行頭 / 行末まで、⇧⌘↑ / ⇧⌘↓ で文書の先頭 / 末尾まで
  - 削除：⌘⌫ で行頭まで、⌃K で行末まで（行末で押すと次の行とつなげます）、⌥Delete で次の単語
  - emacs 風：⌃N / ⌃P / ⌃F / ⌃B で上 / 下 / 右 / 左、⌃D で前方削除、⌃H で Backspace
  - Windows では VS Code のキー配置（Ctrl+Shift+Home / End、Ctrl+Delete など）に合わせ、⌘⌫ / ⌃K は Ctrl+Shift+Backspace / Ctrl+Shift+Delete にしました。ぶつかる emacs 風のキーは割り当てません。
- **Remote SSH の実機回帰テスト**（開発者向け）：`scripts/test-remote-ssh-docker.sh` が、Linux 用 remote-server のビルド、Docker 上での SSH サーバの起動、「TCP はつながったまま相手が応答しなくなる」切断の注入までを行います。スリープ復帰と同じ形（`docker compose pause`）で、ユーザーが何もしなくても自動で張り直して読めるところまでをテストで固めました。

#### 変更

- **更新の確認の間隔**：6 時間ごとに確認するようにしました。これまでは起動の 10 秒後に 1 回確認するだけで、necoder を開いたままだと、その後に出た版は開き直すまで見えませんでした。経過時間は壁時計で測るので、スリープしていた時間もきちんと数に入ります。ダウンロード中や再起動待ちの表示を、新しいお知らせで上書きすることはありません。
- **AI トランスクリプトのツール出力**：先頭 100 行と末尾 100 行を残すようにしました（間は「… (N 行省略)」）。以前は末尾 24 行だけだったので、`Read` の頭か `cargo build` の最後のどちらかが必ず欠けていました。
- **UI スレッドからの remote I/O の検出**（開発者向け）：実行時に検出するようにしました。既定では stderr に警告を出して続行します。`NECODER_STRICT_MAIN_THREAD_IO=1` で panic し、`NECODER_ALLOW_MAIN_THREAD_IO=1` で検査を止めます。「blocking API は裏で呼ぶ」という決まりが破られたときに捕まえるための仕組みです。

#### 修正

- **エディタのクリック位置とキャレットのずれ**：ヒットテストが、描画とは別のフォント（OS 既定のプロポーショナルフォント）で幅を測っていました。そのため日本語と ASCII が混ざる markdown の行では、クリックした所から全角 3 文字ほど離れた位置にキャレットが置かれ、ドラッグ選択やホバー、IME の候補の位置も同じだけずれていました。描画に使ったフォントと同じ run の構成（見出しや太字、強調の書体を含む）で測るようにしました。
- **Remote SSH でのプロジェクト切り替え時の無反応**：プロジェクトを切り替えると、アプリが数十秒反応しなくなっていました。切り替えの処理が UI スレッドから remote host のファイル一覧、タブの復元、変更の監視を同期で呼んでいたため、スリープ復帰の直後のように再接続が必要な場面では、その待ち時間がそのまま操作できない時間になっていました。これらを裏の処理に移しました。宛先チップはすぐに切り替わり、＋context の候補ファイルとタブは読めたものから順に表示されます。
- **Remote SSH のプロジェクトを開いたままの起動**：接続を待つ間、ウィンドウが出ませんでした。remote のプロジェクトを開いたまま終了すると、次の起動では接続が済むまで（届かないホストがあれば、その待ち時間も含めて）ウィンドウが出なかったのです。前回保存した情報だけでレールとタブを組み立てて先にウィンドウを出し、接続と中身の読み込みは裏で追いつくようにしました。届かないホストはトーストで知らせ、操作したときに再接続を試みます。
- **Remote SSH のスリープ復帰後の再接続**：時間がかかっていました。`ssh -O check` は多重化 master が生きているかしか見ず、TCP だけ死んだ master も「生きている」と答えるため、その上の接続が黙って固まっていたのが原因です。ハンドシェイクを 10 秒で打ち切って master ごと張り直し、届かないホストへの再試行の間隔は 5 秒から最大 60 秒まで広げます。
- **Remote SSH の切断の検知**：切れた接続に気づくまで最大 30 秒かかっていました。生存確認（Ping）の待ち時間を 5 秒にし、張り直しが決まった時点で、その接続に乗っていた要求はすぐに失敗させて新しい接続でやり直します（以前は要求ごとのタイムアウトまで待っていました）。時間切れでも受信が進んでいれば、混んでいるだけとみなして切りません。遅い回線で大きなファイルを転送しているときに、切っては送り直すのを繰り返さないためです。
- **Remote SSH の切断中の操作**：切断中に操作するほど遅くなっていました。届かないホストへの再接続は 1 回で十数秒かかるのに、待たされていた操作がそれぞれ最初から接続をやり直していたためです。待っている間に起きた失敗はそのまま受け取り、後から来た操作（つまりユーザーの再試行）だけが接続しに行くようにしました。
- **Remote SSH のハンドシェイク失敗とターミナル**：ハンドシェイクがすぐ閉じたとき、ターミナルまで切っていました。時間切れではなくすぐ EOF になった場合にも ControlMaster を作り直し、同じ接続に乗っていたターミナルを切っていたのです。remote-server 側の起動失敗では master に触らないようにしました。
- **Remote SSH のエクスプローラ操作**：操作のたびに固まっていました。フォルダの展開や折り畳み、リネーム、変更監視のイベント、手動更新（↻）、ブランチの切り替えのたびに、SSH の往復のぶんだけ止まっていました。ツリーの読み取りを裏へ移したので、折り畳みと読み込み済みフォルダの展開はすぐに反映されます。
- **Remote SSH の外部変更の照合と起動処理**：外部変更の照合や、ターミナル、LSP、エージェントの起動が UI スレッドで接続を待っていました。開いているファイルが外で変わったときの照合と再読み込み、エージェントのターンの開始時と終了時の HEAD 取得も裏へ移しました。ターミナルと LSP の SSH セッションは、master が無ければ自分で接続する（`ControlMaster=auto`）ので、起動の設定を組み立てる段階ではネットワークに触りません。
- **AI トランスクリプトの畳んだ要約の行数**：実際より少なく表示していました。「▸ N 行」の N を切り詰めた後の行数から数えていたため、1000 行の出力が「24 行」と表示され、開いても増えませんでした。切り詰める前の行数を出すようにしました。

## [0.1.11] - 2026-09-04

### Added

- **A progress bar on the update chip**: After you click "⬆ Update", the label moves through "Downloading NN% → Verifying signature → Replacing the app" and a thin bar underneath fills up (the line in the About modal shows the same text).
- **The installed-update chip becomes a "⟳ Restart to vX" button**: Clicking it quits necoder and reopens it on the new version automatically (before, it only told you to restart by hand). Quitting takes the same path as a normal ⌘Q (discarding the unsaved hot exit snapshots and tidying up window sessions).

### Changed

- **Rail background in Workspace switching mode**: The left rail background is now brighter, `bg2` instead of `bg1`, so it is easier to see that `⌘{` / `⌘}` now act on the rail.
- **Claude Code heredoc file writes fold by default**: When Claude Code puts a whole file in the Bash tool title as `cat > file <<EOF`, it now collapses to `Write file` plus a line count (`書き込み file` in the Japanese UI). Expand it to read the full file with syntax colors based on its extension.

### Fixed

- **Workspaces open at quit were sometimes not restored after a restart**: The window session is now saved to Turso synchronously right before quitting, and a process that failed to open the DB no longer retries opening it for every Workspace.
- **Diagnostics and terminal SVGs in the status bar were invisible**: GPUI SVGs do not inherit the parent's text color, so each SVG now sets its color directly (the preview and explorer refresh icons had the same problem and are fixed too).

### 日本語

#### 追加

- **更新チップの進捗バー**：「⬆ 更新」を押すと、表示が「ダウンロード中 NN% → 署名を検証中 → 差し替え中」と進み、下の細いバーが伸びます（About モーダルの 1 行にも同じ文言が出ます）。
- **差し替え済みチップの「⟳ 再起動して vX へ」ボタン**：押すと necoder が終了し、新しいバージョンで自動的に開き直します（これまでは手動で再起動するよう案内するだけでした）。終了は通常の ⌘Q と同じ手順で行います（未保存の内容のスナップショットの破棄と、ウィンドウのセッションの整理）。

#### 変更

- **ワークスペース切り替えモード中のレールの背景**：左のレールの背景を `bg1` から `bg2` へ明るくしました。`⌘{` / `⌘}` の操作先がレールに移っていることがわかりやすくなります。
- **Claude Code のヒアドキュメントによるファイル書き込みの表示**：Claude Code が Bash ツールのタイトルに載せる `cat > file <<EOF` のファイル全文を、既定で `書き込み file` と行数に畳むようにしました（英語 UI では `Write file`）。展開すると、拡張子に合わせたシンタックスカラーで全文を確認できます。

#### 修正

- **再起動時のワークスペースの復元漏れ**：再起動すると、開いていたワークスペースが戻らないことがありました。終了の直前にウィンドウのセッションを Turso へ同期で保存し、DB を開けなかったプロセスがワークスペースごとに open を繰り返さないようにしました。
- **ステータスバーの診断とターミナルの SVG アイコン**：透明になって見えていませんでした。GPUI の SVG は親の文字色を受け継がないため、各 SVG に色を直接指定しました（同じ問題があったプレビューとエクスプローラの更新アイコンも直しました）。

## [0.1.10] - 2026-09-03

necoder now brings back every window you had open, and window state lives in necoder.db instead of `state.json`. The explorer gets a refresh button and the status bar gets a terminal button.

### Added

- **Restoring multiple windows**: On launch, necoder reopens every window from the last session, with the one you used last in front. Windows you closed yourself stay closed. If you closed all windows before quitting, only the last one comes back.
- **Explorer refresh button**: The ↻ button at the right end of the explorer header reloads the tree, the git colors, and the gutter diff. Use it when the file watcher misses something, such as files an AI agent created.
- **Terminal button in the status bar**: It sits right of the diagnostics count. It works like the terminal button on the rail and ⌃`, and stays lit while the terminal is open.
- **Rail as the last-touched area**: After you click anywhere on the rail, ⌘{ ⌘} switch to the previous or next project until you click outside the rail. The rail turns one step brighter so you can see where the keys go. On a trackpad, you can tap the rail and press ⌘}.
- **Destination flash on project switch**: When you switch projects from the keyboard (⌃⌘↑↓ / ⌘1..9), the destination shows in the center for about a second: project name and ⎇ branch, with a bar in the project color on the left.
- **Web Inspector in release builds**: Right-click → "Inspect Element" (Web Inspector) in the HTML preview now works in release builds too.

### Changed

- **Where window state is saved**: Each window's open projects and open tabs are now saved in necoder.db (`window_sessions`) instead of `state.json`. Each window has one row and updates only its own row. The old `state.json` is no longer read. If it is still there, it is ignored and you can delete it.
- **Diagnostics icons in the status bar**: The diagnostics counts now use Lucide icons (circle-x / triangle-alert) instead of the text glyphs ✗ ▲, so they no longer break with different fonts.
- **Release page text**: CI now generates the Release page text from CHANGELOG. The text for 0.1.3 to 0.1.6 was filled in too.

### Fixed

- **Tabs closed in another window came back**: A tab you closed in one window could reappear on the next launch. Every window overwrote the whole `state.json`, so the last window to write won. The move to necoder.db above fixes this.
- **Old windows reopened after `necoder <path>`**: If you launched with `necoder <path>` from a terminal and quit several times, the next plain launch opened all those past windows. On quit, necoder now keeps only the windows open at that moment for restoring.
- **Blank icons in the status bar**: The diagnostics icons and the eye icon of the preview toggle were not drawn because they were missing from the asset table. A test now checks that every icon is registered.
- **Shortcuts in AI full screen**: While an AI tab was full screen, ⌘W / ⌘⇧T / ⌃Tab went to the editor tabs and did not work.
- **Keyboard focus and the HTML preview**: necoder now hands keyboard focus back to GPUI before it hides or destroys the HTML preview. This prevents a hidden WebView from holding on to key input and blocking shortcuts.

### 日本語

前回開いていたウィンドウをすべて復元するようにし、ウィンドウの状態の保存先を `state.json` から necoder.db に移しました。エクスプローラに更新ボタンを、ステータスバーにターミナルボタンを加えました。

#### 追加

- **複数ウィンドウの復元**：起動すると、前回開いていたウィンドウをすべて開き直します。最後に使ったウィンドウが前面に来ます。自分で閉じたウィンドウは次回は戻りません。すべて閉じてから終了した場合は、最後のウィンドウだけが戻ります。
- **エクスプローラの更新ボタン**：ヘッダ右端の ↻ を押すと、ツリーと git の色、ガターの差分表示を読み直します。AI エージェントが作ったファイルなど、ファイルの監視が取りこぼした時に使えます。
- **ステータスバーのターミナルボタン**：診断件数の右にあります。レールのターミナルボタンや ⌃` と同じ動きで、ターミナルを開いている間は点灯します。
- **レールを最後に触った場所として扱う**：レールのどこかを押すと、レールの外を押すまでは ⌘{ ⌘} で前後のプロジェクトへ切り替えられます。その間はレールが一段明るくなり、キーがどこに効くか分かります。トラックパッドでレールを軽く叩いてから ⌘} を押す、という使い方ができます。
- **切り替え先のプロジェクトの表示**：キーボードでプロジェクトを切り替えると（⌃⌘↑↓ / ⌘1..9）、行き先を画面の中央に 1 秒ほど表示します。プロジェクト名と ⎇ ブランチを出し、左にプロジェクト色のバーを付けます。
- **release ビルドの Web Inspector**：release ビルドでも、HTML プレビューの右クリックから「要素を検証」（Web Inspector）を開けるようになりました。

#### 変更

- **ウィンドウの状態の保存先**：ウィンドウごとに開いているプロジェクトとタブを、`state.json` ではなく necoder.db（`window_sessions`）に保存するようにしました。1 つのウィンドウが 1 行を持ち、自分の行だけを書き換えます。古い `state.json` は読みません。残っていても無視するので、消してかまいません。
- **ステータスバーの診断アイコン**：診断件数の記号を、文字の ✗ ▲ から Lucide のアイコン（circle-x / triangle-alert）に替えました。フォントの違いで崩れなくなります。
- **Release ページの本文**：CI が CHANGELOG から生成するようにしました。0.1.3 から 0.1.6 の本文も補いました。

#### 修正

- **別のウィンドウで閉じたタブが次の起動で戻る**：すべてのウィンドウが `state.json` を丸ごと上書きしていたため、最後に書いたウィンドウの内容が残っていました。上の保存先の変更で直りました。
- **`necoder <path>` の後に古いウィンドウが開く**：ターミナルから `necoder <path>` で起動して終了するのを繰り返すと、次に普通に起動した時に過去のウィンドウがすべて開いていました。終了時に、その時点で開いていたウィンドウだけを復元の対象に残すようにしました。
- **ステータスバーのアイコンが空白になる**：診断のアイコンと、プレビュー切り替えの目のアイコンが描かれていませんでした。アセットの表に登録していなかったのが原因です。今後はすべてのアイコンが登録されているかをテストで確かめます。
- **AI の全画面でのショートカット**：AI のタブを全画面にしている間、⌘W / ⌘⇧T / ⌃Tab がエディタのタブ側に渡ってしまい、効きませんでした。
- **HTML プレビューとキーボードフォーカス**：HTML プレビューを隠す前と破棄する前に、キーボードフォーカスを GPUI 側へ戻すようにしました。隠れた WebView がキー入力を握ったままになり、ショートカットが効かなくなるのを防ぎます。

## [0.1.9] - 2026-09-02

necoder now looks up ACP agent versions in the public registry, so new adapter releases reach you without a new necoder build. You can also override how an agent is launched in `settings.json`.

### Added

- **Agent versions from the public registry**: When an adapter such as claude-agent-acp ships a new version, necoder uses it from the next session start, without waiting for a necoder release (rebuild). The lookup runs in the background 12 seconds after launch. It is cached for an hour and comes from the ACP project's official CDN. When offline, necoder falls back to the built-in catalog. Threads that are already running are not affected.
- **`agent_servers.<id>` in `settings.json`**: Overrides how an agent is launched. `{"type":"custom","command":…}` replaces the launch command entirely and skips version lookup, so you are fully in control. `{"type":"registry","env":{…}}` only adds environment variables.

### Changed

- **Default versions in the built-in catalog**: claude-agent-acp 0.66.0 → 0.73.0 / codex-acp 1.1.14 → 1.8.0 / copilot 1.0.70 → 1.0.82 / qwen-code 0.19.9 → 0.22.3. From now on the registry will be ahead, so the catalog is the fallback for offline use.
- **Copy button on transcript entries**: The copy button on AI transcript entries is now a 22px square with a 1px border. The old 18×14px button was hard to hit.

### 日本語

ACP エージェントの版を公開レジストリから調べるようにしました。アダプタの新版が出ると、necoder を作り直さなくても使えます。エージェントの起動方法は `settings.json` で上書きできます。

#### 追加

- **公開レジストリからのエージェントの版の解決**：claude-agent-acp などのアダプタに新版が出ると、necoder のリリース（再ビルド）を待たずに、次にセッションを始めた時から使います。確認は起動の 12 秒後に裏で行います。結果は 1 時間キャッシュし、取得先は ACP プロジェクトの公式 CDN です。オフラインの時は組み込みのカタログを使います。動いているスレッドには影響しません。
- **`settings.json` の `agent_servers.<id>`**：エージェントの起動方法を上書きします。`{"type":"custom","command":…}` は起動コマンドを丸ごと差し替え、版の解決もしません。すべて自分で決められます。`{"type":"registry","env":{…}}` は環境変数だけを足します。

#### 変更

- **組み込みカタログの既定の版**：claude-agent-acp 0.66.0 → 0.73.0 / codex-acp 1.1.14 → 1.8.0 / copilot 1.0.70 → 1.0.82 / qwen-code 0.19.9 → 0.22.3 に上げました。今後はレジストリの方が先に進むので、カタログはオフラインの時の土台になります。
- **transcript のコピーボタン**：AI transcript の各エントリのコピーボタンを、1px の枠が付いた 22px 角に大きくしました。これまでの 18×14px は狙って押しにくいものでした。

## [0.1.8] - 2026-08-31

Local HTML files now have a preview that uses the OS WebView, and Markdown renders GFM tables. `ne .` and `ne <file>` now reliably reach the running necoder.

### Added

- **Local HTML preview**: In an `.html` tab, switch between source and preview with "Preview" in the breadcrumb or ⌘⇧V. necoder uses WKWebView on macOS and WebView2 on Windows, and creates it only when the preview is first shown. No Chromium or other browser engine is bundled. Linux shows that the preview is not supported. ↻ reloads, and saving the file reloads it automatically.
- **Freeing memory from hidden HTML previews**: The WebView of an HTML preview that stays hidden is destroyed automatically to free memory. The setting is `html_preview_evict_minutes` (default 15 minutes, `0` turns it off). The WebView is created again when you show the preview.
- **GFM tables in Markdown**: Tables are rendered both in the `.md` preview and in the AI transcript. Column widths follow the content, with a minimum width for short columns. Left, center, and right alignment (`|:---:|`) and inline styles in cells such as `code` and **bold** are supported.
- **Image project icons on the rail**: Put `.necoder/icon.png` (jpg, jpeg, and webp also work) in the project and the rail shows it instead of the emoji or initial. You can also set a path in `icon` in settings. The ring in the project color stays.
- **Fast-forward before `+ Task`**: Before Fleet creates a Task with `+ Task`, it fast-forwards the default branch to its upstream. This only happens when the branch is clean and behind (ff-only). When offline, with no upstream, or when the branch is dirty or diverged, necoder skips this silently and creates the Task from the current HEAD.

### Fixed

- **`ne .` / `ne <file>` started a new instance**: Sometimes the command did not reach the running necoder and started a second one. The IPC socket now repairs itself, files are handed over as documents with `open -a`, and a single-instance guard stops any second copy. This also fixes several necoder icons appearing in the Dock.
- **Composer in AI full screen**: When you made an AI agent tab full screen, the composer's line wrapping broke and the text flickered. The composer now tells its parent when the wrapped height changes, and the scroll position is clamped again.

### 日本語

ローカルの HTML ファイルを OS の WebView でプレビューできるようになり、Markdown の GFM 表も描けるようになりました。`ne .` と `ne <file>` は、起動中の necoder に確実に届くようになりました。

#### 追加

- **ローカル HTML のプレビュー**：`.html` のタブで、パンくずの「プレビュー」か ⌘⇧V でソースとプレビューを切り替えられます。macOS では WKWebView、Windows では WebView2 を使い、プレビューを初めて表示する時まで作りません。Chromium などのブラウザエンジンは同梱しません。Linux では対応していない旨を表示します。↻ で読み直せるほか、保存に成功すると自動で読み直します。
- **隠れた HTML プレビューのメモリ回収**：HTML プレビューを隠したまま置いておくと、その WebView を自動で破棄してメモリを返します。設定は `html_preview_evict_minutes` で、既定は 15 分、`0` で無効です。もう一度表示した時に作り直します。
- **Markdown の GFM 表**：`.md` の整形プレビューと AI transcript の両方で表を描きます。列の幅は内容に合わせて配分し、短い列にも最小の幅を確保します。`|:---:|` による左、中央、右の揃えと、セルの中の `コード` や **強調** などのインラインの装飾に対応しました。
- **レールのプロジェクトアイコンに画像**：プロジェクトに `.necoder/icon.png`（jpg、jpeg、webp も可）を置くだけで、絵文字や頭文字の代わりにその画像を表示します。settings の `icon` にパスを書いても指定できます。プロジェクト色のリングはそのまま残ります。
- **`+ Task` の前の早送り**：Fleet で `+ Task` から Task を作る前に、デフォルトブランチを upstream まで自動で早送りします。早送りするのは、作業ツリーがきれいで upstream より遅れている時だけです（ff-only）。オフラインの時、upstream が無い時、未コミットの変更がある時、履歴が分岐している時は、何も言わずに飛ばし、今の HEAD から作ります。

#### 修正

- **`ne .` / `ne <file>` で新しいインスタンスが立つ**：コマンドが起動中の necoder に届かず、もう 1 つ起動してしまうことがありました。IPC のソケットが自分で直るようにし、ファイルは `open -a` で書類として渡す方式に変え、2 つ目が立たないようにする歯止めも入れました。Dock にアイコンが複数並ぶ症状もこれで直りました。
- **AI の全画面での composer**：AI エージェントのタブを全画面にすると、composer の折り返しが崩れて文字がちらついていました。折り返した高さが変わったことを親に伝え、スクロール位置も範囲に収め直すようにしました。

## [0.1.7] - 2026-08-30

This release adds the `ne` command for the terminal, an About dialog with a manual update check, and license notices in the downloads. The composer now grows with its content.

### Added

- **`ne` command for the terminal**: Works like VS Code's `code`. `ne .` / `ne <file>` opens in the window of the running necoder (forwarded over IPC and brought to the front), or launches the app if it is not running. `ssh://` and the existing `config` / `fleet` / `mcp` subcommands are passed through as before. Install it from Settings > Command line (macOS asks for your password) or with `necoder install-cli`. You can remove it from the same places.
- **About dialog and update check**: The menu has "About necoder", and the menu and the command palette have "Check for Updates…". A manual check tells you whether an update is available, you are up to date, or the check failed.
- **License notices in the downloads**: `THIRD_PARTY_NOTICES.md` and `licenses/` are now included in both the macOS and Windows downloads.

### Changed

- **Composer grows with its content**: The composer input grows from its default height up to 420px, and scrolls inside beyond that. This fixes losing sight of the cursor after pasting long text.
- **Auto-scroll while selecting**: In the editor, the ACP transcript, and the terminal, dragging a selection past the edge of the view now scrolls and extends the selection.

### Fixed

- **Dropped API streams**: When an AI agent's API stream was cut off ("Connection closed mid-response"), the whole thread session died. Now only that turn fails, and you can send again in the same thread.

### 日本語

ターミナルから使う `ne` コマンドと、手動でアップデートを確認できる About の画面を加え、配布物にライセンスの通知を同梱しました。composer の入力欄は内容に合わせて伸びるようになりました。

#### 追加

- **ターミナル用の `ne` コマンド**：VS Code の `code` にあたるコマンドです。`ne .` や `ne <file>` は、起動中の necoder があればそのウィンドウで開き（IPC で渡して前面に出す）、無ければアプリを起動します。`ssh://` と、これまでの `config` / `fleet` / `mcp` サブコマンドはそのまま渡します。入れるには 設定 > コマンドライン（macOS の認証ダイアログが出ます）か `necoder install-cli` を使います。削除も同じ場所からできます。
- **About の画面とアップデートの確認**：メニューに「necoder について」を、メニューとコマンドパレットに「アップデートを確認…」を加えました。手動で確認すると、新しい版がある、最新です、確認できなかった、のどれなのかを分けて伝えます。
- **配布物のライセンス通知**：macOS 版と Windows 版の両方に `THIRD_PARTY_NOTICES.md` と `licenses/` を同梱しました。

#### 変更

- **composer の入力欄が内容に合わせて伸びる**：既定の高さから 420px まで伸び、それを超えると中でスクロールします。長い文を貼ると入力位置が見えなくなる問題を直しました。
- **選択中の自動スクロール**：エディタ、ACP の transcript、ターミナルで、選択しながらビューの外までドラッグすると、自動でスクロールして選択が伸びます。

#### 修正

- **API のストリーミングの切断**：AI エージェントの API ストリーミングが途中で切れると（「Connection closed mid-response」）、スレッドのセッションごと落ちていました。そのターンだけが失敗するようにしたので、同じスレッドで送り直せます。

## [0.1.6] - 2026-08-28

The terminal can now scroll back through past output, and the explorer supports Finder-style drag and drop. This release also fixes a checkpoint bug that could overwrite a file with a fragment.

### Added

- **Terminal scrollback**: Scroll back through past output with the mouse wheel or trackpad. Before, the terminal kept 10k lines of history but always stayed pinned to the bottom. In the alternate screen (less / vim), scrolling sends arrow keys. Typing, pasting, or committing IME input returns to the bottom.
- **Drag and drop in the explorer**: Move files by dragging, as in Finder, in all three views (tree, columns, icons). Dropping files from Finder copies them in, recursively. Existing files are never overwritten.
- **New file from ⌘P**: In an empty local project, ⌘P offers "New file…" and "New folder…".
- **Agent brand badges in the composer**: The agent picker pill and its menu show each agent's brand badge.
- **Agent status in Settings**: Settings notices within a few seconds when you install an agent or finish logging in, and marks it "Available". You no longer need to reopen Settings.

### Changed

- **Thread history**: Thread history shows only the threads of the active project.

### Fixed

- **Checkpoint rewind could break files**: Rewinding a checkpoint could overwrite a file with a fragment. The snapshot saved the Edit tool's `oldText`, which is only the replaced hunk, as if it were the whole file. Rewinding then left only that hunk in the file. In one real case, an 8,700-line file became 5 lines. Snapshots now always read the full file from disk.
- **Bypass permissions**: Bypass permissions did not take effect on the first turn of a new tab or when you switched to it in the middle of a turn. The mode is now applied right after the session is created. If you switch to bypass while an approval card is waiting, necoder approves it on the spot and continues.
- **Landing page**: Added an English hero capture, fixed agent icons blending into the background in dark mode, and added a dedicated OGP card.

### 日本語

ターミナルで過去の出力をさかのぼれるようになり、エクスプローラでは Finder のようにドラッグ＆ドロップで動かせるようになりました。checkpoint を巻き戻すとファイルが断片で上書きされることがある不具合も直しました。

#### 追加

- **ターミナルのスクロールバック**：ホイールやトラックパッドで過去の出力をさかのぼれます。これまでは 10k 行の履歴を持っていても、常に最下段に固定されていました。代替画面（less / vim）では矢印キーを送ります。入力やペースト、IME の確定で最下段に戻ります。
- **エクスプローラのドラッグ＆ドロップ**：Finder と同じようにドラッグでファイルを移動できます。ツリー、カラム、アイコンの 3 つの表示すべてで使えます。Finder からドロップするとコピーして加えます。フォルダは中身ごとコピーし、既にあるファイルは上書きしません。
- **⌘P から新規作成**：空のローカルプロジェクトでは、⌘P に「新規ファイル…」と「新規フォルダ…」を出します。
- **composer のエージェントのバッジ**：エージェントを選ぶピルとそのメニューに、各エージェントのブランドのバッジを付けました。
- **設定画面のエージェントの状態**：エージェントを入れたりログインを済ませたりすると、数秒で気づいて「利用可能」に切り替えます。設定画面を開き直す必要はありません。

#### 変更

- **スレッドの履歴**：アクティブなプロジェクトのスレッドだけを表示するようにしました。

#### 修正

- **checkpoint の巻き戻しでファイルが壊れる**：巻き戻すと、ファイルが断片で上書きされることがありました。スナップショットが、Edit ツールの `oldText`（置き換えた hunk だけ）をファイル全体として保存していたためです。巻き戻すとファイルの中身がその hunk だけになり、実際に 8,700 行のファイルが 5 行になりました。常にディスクからファイル全体を読んで保存するように直しました。
- **bypass permissions が効かない**：新しいタブの最初のターンと、ターンの途中で切り替えた時に効いていませんでした。セッションを作った直後にモードを反映するようにしました。承認待ちのカードが出ている時に bypass へ切り替えると、その場で許可して先へ進みます。
- **LP**：英語版のヒーローのキャプチャを加え、ダークモードでエージェントのアイコンが背景に溶け込む問題を直し、OGP のカード画像を専用のものにしました。

## [0.1.5] - 2026-08-27

necoder can now open images in a tab and show them in the Markdown preview. On Windows, a chip tells you when a new version is out.

### Added

- **Image viewer**: Open png, jpg, gif, webp, and other images in a tab. Editing, saving, ⌘F, and LSP are off in image tabs. The image reloads automatically when it changes on disk.
- **Images in the Markdown preview**: The rendered preview (⌘⇧V) now shows images. Relative paths are resolved from the `.md` file's folder. If an image cannot be read, it falls back to `🖼 alt` text. A toggle button for the preview (👁) sits at the right end of the breadcrumb.
- **Windows update notice**: When a new version is out, a "⬆ Get vX.Y.Z" chip appears. Clicking it opens the Release page in your default browser. This only covers releases that include a zip. The update check after launch now starts after 10 seconds instead of 90.
- **Landing page**: The download link now reads "Mac / Windows", and Windows 10+ (x64) was added to the supported systems.

### Changed

- **Tab switching follows ⌘W**: ⌃Tab / ⌘⌥← → now pick their target the same way ⌘W does. If you last used the editor, they switch file tabs. If you last used the Agent area, they switch AI threads.
- **Composer height**: The composer's default height went from 104px (about 4 lines) to 86px (about 3 lines).

### Fixed

- **Running tool display**: A multi-line Bash command made the running tool display grow tall. It now shows only the first line, cut at 100 characters.
- **git blame label**: The "Uncommitted" label in git blame is now translated. The English locale used to show Japanese.

### 日本語

画像をタブで開けるようになり、Markdown の整形プレビューにも画像を表示するようになりました。Windows では、新しい版が出るとチップで知らせます。

#### 追加

- **画像ビューア**：png、jpg、gif、webp などの画像をタブで開けます。画像のタブでは、編集と保存、⌘F、LSP は使えません。ディスク上で画像が変わると自動で読み直します。
- **Markdown の整形プレビューの画像**：整形プレビュー（⌘⇧V）で画像を表示します。相対パスは `.md` のあるフォルダを起点に解決します。読めない画像は `🖼 alt` の文字で代わりに表示します。パンくずの右端に整形プレビューの切り替えボタン（👁）を置きました。
- **Windows のアップデートの知らせ**：新しい版が出ると「⬆ vX.Y.Z を入手」のチップを出し、押すと Release ページを既定のブラウザで開きます。対象は zip が付いたリリースだけです。あわせて、起動後にアップデートを確認するまでの待ち時間を 90 秒から 10 秒に縮めました。
- **LP**：ダウンロードの案内を「Mac / Windows 版」に変え、対応環境に Windows 10+（x64）を書き足しました。

#### 変更

- **タブの切り替えを ⌘W に合わせる**：⌃Tab と ⌘⌥← → は、⌘W と同じ基準で切り替える対象を選びます。最後に使ったのがエディタならファイルのタブを、Agent の領域なら AI のスレッドを切り替えます。
- **composer の高さ**：composer の既定の高さを 104px（約 4 行）から 86px（約 3 行）にしました。

#### 修正

- **実行中のツールの表示**：複数行の Bash コマンドだと、実行中のツールの表示が縦に伸びていました。1 行目だけを 100 文字までで表示するようにしました。
- **git blame の表示**：「未コミット」の表示を翻訳の対象にしました。英語のロケールで日本語が出ていました。

## [0.1.4] - 2026-08-26

This release adds a keyboard shortcut sheet and fixes selecting text in an agent's reply while it is still streaming.

### Added

- **Keyboard shortcut sheet (⌘K ⌘S)**: A read-only overlay built automatically from the default keymap. It lists action names and keys in sections: Editor, AI chat, Control, and Global. Keys are shown as mac symbols or with Windows spelling. Escape or a click on the background closes it. You can also open it from the command palette.

### Fixed

- **Could not select a streaming reply**: You could not drag-select or ⌘C the agent's reply while it was being generated. The tail of the reply was drawn in a separate view during streaming. It now uses the same selectable rendering as a finished reply. The typewriter effect stays.

### 日本語

キーボードショートカットの一覧を加え、生成中のエージェントの返答を選択できなかった不具合を直しました。

#### 追加

- **キーボードショートカットの一覧（⌘K ⌘S）**：既定の keymap から自動で作る、見るだけのオーバーレイです。エディタ、AI チャット、管制、グローバルの区分ごとに、動作の名前とキーを並べます。キーは mac では記号で、Windows では Windows の綴りで表示します。Escape か背景のクリックで閉じます。コマンドパレットからも開けます。

#### 修正

- **生成中の返答を選択できない**：エージェントが返答を生成している間、その本文をドラッグで選んだり ⌘C でコピーしたりできませんでした。生成中の末尾の本文を別のビューで描いていたのをやめ、生成が終わった後と同じ、選択できる描き方にまとめました。タイプライター風の表示はそのままです。

## [0.1.3] - 2026-08-26

This release adds five bundled themes and an Appearance section in Settings. ACP gains elicitation (questions with choices) and a send queue.

### Added

- **Five bundled themes**: Solarized Dark / Solarized Light / Gruvbox Dark / Catppuccin Mocha / High Contrast Dark. In `settings.json`, `theme` accepts either the id (`solarized-dark`) or the display name.
- **Appearance in Settings**: Themes are listed as chips and apply as soon as you click one. An "Open settings.json" button was added, along with the palette command "Preferences: Open settings.json". Editing `theme` in `settings.json` by hand also applies right away.
- **ACP elicitation**: necoder supports elicitation (questions with choices). You pick one of the agent's choices on a card and send back Accept or Decline. Forms that include text input are declined, to stay on the safe side.
- **ACP send queue**: Pressing Enter while a reply is being generated no longer sends right away. The message is queued and sent automatically when the turn ends, so you can no longer start a second turn by accident. Next to the destination chip are "Interrupt & send" (steer) and cancel.
- **ACP client name**: necoder now identifies itself as necoder in `clientInfo` of `initialize`.

### Changed

- **Command lines in the transcript**: The command line in the transcript (⏺ plus arguments) is cut to one line and expands on click, the same way ⎿ results do.

### Fixed

- **Long file names in the explorer**: Deep in the tree, the indent pushed file names out of view. The explorer now scrolls horizontally.
- **Idle shown while generating**: The thread could show as idle while a reply was being generated. A second message queued during generation did not set the running state.

### 日本語

テーマを 5 つ同梱し、設定画面に「外観」を加えました。ACP では、選択肢付きの質問（Elicitation）と送信キューに対応しました。

#### 追加

- **同梱テーマ 5 種**：Solarized Dark / Solarized Light / Gruvbox Dark / Catppuccin Mocha / High Contrast Dark です。`settings.json` の `theme` には、id（`solarized-dark`）と表示名のどちらでも書けます。
- **設定画面の「外観」**：テーマをチップで並べ、押すとすぐに切り替わります。「settings.json を開く」ボタンと、パレットの「設定: settings.json を開く」も加えました。`settings.json` の `theme` を手で書き換えても、すぐに反映します。
- **ACP の Elicitation**：選択肢付きの質問に対応しました。エージェントが出した選択肢をカードで選び、Accept か Decline を返します。文字の入力を含むフォームには、安全のため Decline を返します。
- **ACP の送信キュー**：生成中に Enter を押してもすぐには送らず、キューに積んで、ターンが終わったら自動で送ります。うっかり 2 つ目のターンを始めてしまうことがなくなります。宛先チップの横に「中断して今すぐ」（steer）と取り消しがあります。
- **ACP のクライアント名**：`initialize` の clientInfo で necoder と名乗るようにしました。

#### 変更

- **transcript のコマンド行**：transcript のコマンド行（⏺ と引数）を 1 行に縮め、押すと展開するようにしました。結果の ⎿ と同じ扱いです。

#### 修正

- **エクスプローラの長いファイル名**：深い階層では、インデントに押されてファイル名が見えなくなっていました。横にスクロールできるようにしました。
- **生成中なのにアイドルと表示される**：生成中にキューへ積んだ 2 つ目の送信で、実行中の状態が立っていませんでした。

## [0.1.2] - 2026-08-25

The v0.1.0 and v0.1.1 downloads crashed right after launch on any machine other than the one that built them. This release fixes that and adds checks so it cannot happen again.

### Fixed

- **Downloads did not start (critical)**: This affected the `.dmg` and `.zip` of v0.1.0 and v0.1.1. The mascot images were loaded by opening `env!("CARGO_MANIFEST_DIR")` as a **path at run time**, so the app **panicked right after launch on any machine other than the build machine**:

  ```text
  mascot asset D:\a\necoder\necoder\crates\agent_panel/assets/mascot\idle.png:
  指定されたパスが見つかりません。 (os error 3)
  ```

  (`D:\a\necoder\necoder` is the path on the GitHub Actions runner.)
  The images are now embedded in the binary with `include_bytes!`, like the fonts and icons.
  **v0.1.0 and v0.1.1 do not work. Please update to this version.**
- **Crash logs lost the real cause**: When there was a double panic, the crash log **erased the first panic, which was the real cause**. A panic inside the startup closure cannot unwind across GPUI's extern "C" boundary, so it turns into a second panic (cannot unwind). That second panic overwrote `crash-<unix seconds>-<pid>.log`, which has the same second and PID. This got in the way while tracking down the bug above. Crash logs are now appended to.

### Added

- **Regression test for build-time paths**: `crates/necoder/tests/no_runtime_manifest_paths.rs` finds any place in any crate that opens a build-time path at run time. **This bug can never show up on a development machine**, because the path exists there, so reviewers cannot catch it by eye.
- **Checks on the final release artifacts**: Release CI now also checks the shipped binaries. If `strings` finds the build machine's path (`$GITHUB_WORKSPACE/crates`) in a binary, the job fails. The development fallback for finding the remote server (`target/debug/…`) is now limited to debug builds, so release binaries contain no absolute paths from the build machine at all, and the check can expect zero matches.
- **VCRUNTIME check on Windows**: The VCRUNTIME dependency check in the Windows job of `release.yml` now actually runs. It did not look for Visual Studio under `Program Files`, so it had never checked anything before.

### 日本語

v0.1.0 と v0.1.1 の配布物は、ビルドしたマシン以外では起動直後に落ちていました。この版で直し、同じことが二度と起きないように確認の仕組みも入れました。

#### 修正

- **配布物が起動しない致命的な不具合**：v0.1.0 と v0.1.1 の `.dmg` と `.zip` が該当します。マスコットの画像を読み込む時に、`env!("CARGO_MANIFEST_DIR")` を**実行時のパス**として開いていたため、**ビルドしたマシン以外では起動した直後に panic** していました。

  ```text
  mascot asset D:\a\necoder\necoder\crates\agent_panel/assets/mascot\idle.png:
  指定されたパスが見つかりません。 (os error 3)
  ```

  （`D:\a\necoder\necoder` は GitHub Actions のランナー上のパスです）
  フォントやアイコンと同じく、`include_bytes!` でバイナリに埋め込むように直しました。
  **v0.1.0 と v0.1.1 は使えません。この版に更新してください。**
- **クラッシュログから本当の原因が消える**：panic が 2 回続いた時に、**1 回目、つまり本当の原因を消していました**。起動のクロージャの中で起きた panic は GPUI の extern "C" の境界を越えて unwind できず、2 回目の panic（cannot unwind）になります。これが同じ秒、同じ PID の `crash-<unix秒>-<pid>.log` を上書きしていました。上の不具合を調べる時に実際に困りました。ログは追記するように変えました。

#### 追加

- **ビルド時のパスを検出する回帰テスト**：`crates/necoder/tests/no_runtime_manifest_paths.rs` を加え、ビルド時のパスを実行時に開いている所をすべての crate から探します。そのパスは開発機には実在するので、**この不具合は開発機ではそもそも再現しません**。人の目では防げないため、テストで止めます。
- **最終的な配布物の確認**：release CI で、配布するバイナリそのものも確かめるようにしました。`strings` でビルド機のパス（`$GITHUB_WORKSPACE/crates`）が見つかったら失敗させます。あわせて、リモートサーバを探す時の開発用の fallback（`target/debug/…`）を debug ビルドだけに限りました。これで release のバイナリにはビルド機の絶対パスが 1 つも残らず、確認の期待値を 0 件にできます。
- **Windows の VCRUNTIME の確認**：`release.yml` の Windows ジョブで、VCRUNTIME への依存の確認が実際に動くようになりました。`Program Files` にある Visual Studio を見ていなかったため、これまで一度も確認できていませんでした。

## [0.1.1] - 2026-08-24

necoder now runs natively on Windows, and `necoder-windows-x64.zip` is part of the downloads.

### Added

- **Native Windows support** (W0 to W6 in `docs/WINDOWS-PORT.md`). `necoder-windows-x64.zip` was added to the downloads:
  - Launching, editing, saving, search, Git, and the integrated terminal (ConPTY + PowerShell) work on Windows.
  - A new `paths` crate keeps the locations of settings, the DB, and logs in one place. On Windows, settings go to Roaming and the DB and logs go to Local. **This also fixes an existing bug where Linux created `~/Library/Application Support/`.**
  - The control IPC is abstracted over Unix sockets and named pipes.
  - A default Windows keymap (VS Code style `Ctrl-` bindings), and key labels shown per platform (`⌘S` ↔ `Ctrl+S`).
  - Caption buttons in the title bar (minimize, maximize, close).
  - CI always runs `check-windows` (`-D warnings` + `cargo test --workspace`).

### Fixed

- **Terminal dock drew nothing**: The root of a `.cached()` view ignores `flex_1()` and collapses to zero height. This may have affected macOS as well.
- **File watching on first launch**: File watching failed when the settings folder did not exist yet on first launch. This affected all three platforms.
- **Git gutter with CRLF**: Added a regression test for the problem where the git gutter could mark every line as Modified in a CRLF working tree.

### Notes

- **Unsigned Windows build**: The Windows build is not signed, so SmartScreen shows a warning on first launch.
- **In-app updates on macOS only**: On Windows, update by downloading the zip again.

### 日本語

Windows でネイティブに動くようになり、配布物に `necoder-windows-x64.zip` が加わりました。

#### 追加

- **Windows へのネイティブ対応**（`docs/WINDOWS-PORT.md` の W0 から W6）。配布物に `necoder-windows-x64.zip` を加えました。
  - 起動、編集、保存、検索、Git、統合ターミナル（ConPTY + PowerShell）が Windows で動きます。
  - `paths` crate を新しく作り、設定と DB、ログの置き場所を 1 か所で決めるようにしました。Windows では、設定を Roaming に、DB とログを Local に分けて置きます。**Linux で `~/Library/Application Support/` を作っていた不具合もこれで直りました。**
  - 制御用の IPC を、Unix ソケットと名前付きパイプのどちらでも使えるように抽象化しました。
  - Windows の既定の keymap（VS Code に合わせた `Ctrl-` 系）を用意し、キーの表記を OS ごとに出し分けます（`⌘S` ↔ `Ctrl+S`）。
  - タイトルバーに最小化、最大化、閉じるのボタンを付けました。
  - CI で `check-windows` を常に回します（`-D warnings` + `cargo test --workspace`）。

#### 修正

- **ターミナルのドックが何も描かない**：`.cached()` にした view の root では `flex_1()` が効かず、高さが 0 に潰れていました。mac でも起きていた可能性があります。
- **初回起動のファイル監視**：初めて起動した時、設定のフォルダがまだ無いとファイルの監視に失敗していました。3 つのプラットフォームすべてで起きていました。
- **CRLF と git gutter**：CRLF の作業ツリーで、git gutter が全行を Modified と表示することがある問題に回帰テストを加えました。

#### 注意

- **Windows 版は未署名**：初めて起動する時に SmartScreen の警告が出ます。
- **アプリ内の更新は macOS だけ**：Windows では zip をもう一度ダウンロードして更新します。

## [0.1.0] - 2026-08-22

The first public release of necoder.

### Added

- **Preparation for the first public release (v0.1.0)**:
  - Opening files and folders from Finder and the Dock (`CFBundleDocumentTypes` wired to `on_open_urls`).
  - App logs when launched from the GUI (Finder or the Dock) in `~/Library/Application Support/necoder/logs/` (the last 20 are kept).
  - A dependency license audit (cargo-deny) runs in CI on every change.
  - A CLA, CONTRIBUTING, SECURITY, and a Code of Conduct.
- **The editor itself**: Built through milestones M0 to M14: the color rail, ACP threads, LSP, tree-sitter for many languages, Git hunks and blame, the integrated terminal, Remote SSH, and multi-agent work (now Fleet). See `docs/ROADMAP.md` for details.

### Fixed

- **One source for the release version**: `Cargo.toml` is now the only place the version is set, and CI rejects a tag that does not match. Before, the version was hard-coded in Info.plist, and forgetting to bump it made the update chip show up forever.
- **Error messages for updates and the control IPC**: These are now translated. The English locale used to show Japanese errors.

### 日本語

necoder の最初の公開リリースです。

#### 追加

- **最初の公開リリース（v0.1.0）に向けた準備**：
  - Finder や Dock からファイルとフォルダを開けるようにしました（`CFBundleDocumentTypes` と `on_open_urls` をつなぎました）。
  - GUI（Finder や Dock）から起動した時のアプリのログを `~/Library/Application Support/necoder/logs/` に残します（直近の 20 本を保持）。
  - 依存ライブラリのライセンス監査（cargo-deny）を CI で常に回します。
  - CLA、CONTRIBUTING、SECURITY、Code of Conduct を用意しました。
- **エディタ本体**：M0 から M14 までで作りました。色のレール、ACP のスレッド、LSP、tree-sitter による多言語対応、Git の hunk と blame、統合ターミナル、Remote SSH、複数のエージェントを並べて動かす機能（今の Fleet）が入っています。詳しくは `docs/ROADMAP.md` を見てください。

#### 修正

- **リリースの版の出どころを 1 つに**：版を書くのは `Cargo.toml` だけにし、タグと合わない時は CI が止めます。これまでは Info.plist に直接書いていたため、上げ忘れると更新のチップがいつまでも出ていました。
- **更新と制御用 IPC のエラーメッセージ**：翻訳の対象にしました。英語のロケールで日本語のエラーが出ていました。

<!-- リリース時: Unreleased を [x.y.z] - YYYY-MM-DD へ繰り上げ、新しい Unreleased 節を上に作る -->
