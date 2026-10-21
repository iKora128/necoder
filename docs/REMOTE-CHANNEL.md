# REMOTE-CHANNEL.md — Remote SSH を「1 本の接続」にまとめる

Remote SSH の通り道の設計と受入条件の正。2026-10-21 起票（issue #79「Windows で接続できない」が発端）。
ARCHITECTURE §Remote・WINDOWS-PORT §D6 はここを指す。

## 0. なぜ作り直すか

これまでの Remote SSH は **OpenSSH の ControlMaster（1 本の TCP を複数の ssh で使い回す仕組み）に全部が乗っていた**。
ファイル操作の RPC・端末 1 枚ごとの `ssh -tt`・LSP / ACP の `ssh <cmd>`・ポート転送の `-O forward`・
`ne` の `-O forward -R`・サーバーの配備確認（4〜6 回の `ssh`）が、それぞれ別の `ssh` プロセスとして同じ master に乗る。

困ること:

| | 内容 |
|---|---|
| **Windows で動かない** | Windows の OpenSSH（9.5 で確認）は ControlMaster を持たない。`ControlPath` を付けただけで `getsockname failed: Not a socket` で落ちる（2026-10-21 実機）。#79 の直接の原因は手元の `/tmp` 決め打ちだが、直しても次でこれに当たる |
| **認証が何度も要る形に逃げられない** | 使い回しをやめて 1 本ずつ繋ぐと、パスワード・2 段階認証・ハードウェア鍵・パスフレーズ付き鍵（Windows の ssh-agent は既定で無効）の人は、端末や LSP を開くたびに訊かれる |
| **切れると端末が死ぬ** | 端末は `ssh -tt` の子なので、スリープや VPN 断で接続が落ちるとリモートのシェルごと消える |
| **WSL に流用できない** | 通り道が「ssh の多重化」に縛られていて、`wsl.exe` の標準入出力に差し替えられない（WINDOWS-PORT §D11） |

## 1. 決めたこと（形）

```
手元の necoder ──(ssh 1 本・標準入出力)──▶ bootstrap(sh) ─exec─▶ necoder-remote-server proxy
                                                                     │ Unix socket
                                                                     ▼
                                                    necoder-remote-server daemon（session ごと）
                                                    ├ ファイル操作・検索・RunCommand（従来の RPC）
                                                    ├ プロセス（LSP / ACP / 人のコマンド）の stdio
                                                    ├ PTY（端末）
                                                    ├ TCP（ポート転送）
                                                    └ `ne` の受け口（Unix socket）
```

- **ssh は 1 接続につき 1 プロセスだけ**。認証は接続のたびに 1 回（再接続も 1 回）。`-o ControlMaster=no -o ControlPath=none`
  を必ず付ける（利用者の `~/.ssh/config` が多重化を有効にしていても、Windows で落ちないように・自前の多重化と二重にしない）
- **端末・LSP・ACP・ポート転送は、その 1 本の RPC の上の「ストリーム」**になる。`ssh` を追加で起こさない
- **daemon がプロセスと PTY を持つ**。接続が切れても daemon 側のシェル・LSP は生き残り、再接続した client が
  **続きから受け取り直す**（送った入力・受けた出力をオフセットで突き合わせる）。端末はスリープ復帰で消えない
- 配備（サーバーのバイナリを置く）も**同じ 1 本の中で**済ませる（§3）。初回と版の更新でも認証は 1 回
- 手元のプラットフォームに依らない作り。**Windows・mac・Linux の client が同じコードを通る**（分岐は transport の
  起動とテスト用の pair だけ）。WSL は将来「ssh の代わりに `wsl.exe -d <distro> -- <bootstrap>`」で同じ daemon に繋ぐ
- Zed の GPL コード（`remote` / `remote_server` crate）は参照も複製もしない。OpenSSH の公開仕様・POSIX sh・
  alacritty_terminal（Apache-2.0）の公開 API の上に独立に組む

## 2. wire（protocol v4）

既存の frame（28 byte の header + JSON meta + raw body）に種類を 1 つ足す。

| kind | 向き | 用途 |
|---|---|---|
| 1 Request / 2 Response / 3 Error | 双方 | 従来どおり（id で対応） |
| 4 Event | server → client | `ServerEvent`（watch の変更・`ne` の open 依頼） |
| **5 Stream** | 双方 | `StreamMessage`。frame の id = **ストリーム id** |

`StreamMessage`（meta の JSON）:

| 種類 | 中身 |
|---|---|
| `Data { channel, offset }` | body が中身。channel 0 = 入力（client→server）・1 = 出力（stdout / PTY / TCP）・2 = stderr。offset はその channel の何バイト目からか |
| `Ack { channel, offset }` | 受け手が offset まで消費した（送り手は窓を進め、そこまでの再送用の控えを捨てる） |
| `Eof { channel, offset }` | その channel はもう来ない |
| `Exit { code, signal }` | プロセスが終わった（出力の Eof の後に必ず来る） |
| `Resize { columns, lines, pixel_width, pixel_height }` | PTY の大きさ |
| `Signal { name }` | `INT` / `TERM` / `KILL` / `HUP` をプロセスグループへ |
| `Close` | client がもう要らない（server は止めて片付ける） |
| `Lost { reason }` | そのストリームは続けられない（daemon が知らない・控えが溢れた） |

ストリームを開く request: `Spawn { project_id, spec }`（パイプの stdio）・`OpenTerminal { project_id, cwd, size, env }`
（ログインシェルを PTY で）・`OpenTcp { port }`（接続先の 127.0.0.1）→ `Response::Opened { stream_id }`。
繋ぎ直した client は `Attach { stream_id, output_offset, error_offset }` → `Response::Attached { input_offset }`。

**流量制御**: 各 channel は「送ったが Ack の来ていない」量を **1 MiB** までに抑える（超えたら送り手が止まる＝
プロセスの出力を読むのを待つ）。Data は 1 frame **32 KiB** まで（大きな出力が他の request の前に長く居座らない）。
受け手は 64 KiB 消費するごとに Ack を返す。**出力を読まない端末が daemon のメモリを食い尽くさない**。

**再接続**: daemon は Ack の来ていない出力を控えに持っている（＝最大 1 MiB）。client は受け取った位置から Attach し、
daemon はそこから送り直す。入力は逆向きに同じ（client が控えを持ち、daemon の `input_offset` から送り直す）。

- **プロセス（LSP / ACP）は欠けを許さない**: 切れている間に控えが満ちたら daemon は読むのを止めて待つ（プロセスは
  書き込みで止まる）。データは失われない
- **PTY は欠けを許す**: 切れている間も daemon は読み続け、控えが 1 MiB を超えたら古い方から捨てる（ビルドが止まらない）。
  繋ぎ直すと残っている分から見える
- **TCP は続けない**: 接続が切れたらそのストリームは閉じる（ブラウザが繋ぎ直す）。手元の待ち受けは残るので、
  再接続後の新しい接続は自動で通る（「張り直し」の処理が要らない）

**daemon の寿命**: client が 1 つも繋がっていない状態が 10 分続いたら、持っているプロセスと PTY を止めて終わる
（ControlPersist=600 と同じ長さ）。新しい接続が来たら古い接続は切る（1 session = 1 client・最新が勝つ）。

## 3. bootstrap（ssh 1 本の中で配備まで）

ssh のリモートコマンドは `/bin/sh -c '<スクリプト>'`。利用者のログインシェルが fish / csh でも通るよう、
スクリプトには `'` `\\` `!` と改行を入れない（§4 の罠）。スクリプトは標準出力に `NECODER-` で始まる行を出し、
client はそれ以外の行（`.bashrc` の echo 等）を読み飛ばす。

1. `NECODER-HELLO <os> <arch> <home> <tools>` — uname・`$HOME`・使える道具（`gzip` / GNU `head`）
2. 使えるサーバー（`NECODER_REMOTE_SERVER_COMMAND` / PATH の `necoder-remote-server` / 配備先）の `version` が
   一致すれば `NECODER-READY <path>` を出して `exec <path> proxy --session <id>`
3. 無ければ `NECODER-NEED`。client は uname に合う artifact を探し、`NECODER-UPLOAD <bytes> <sha256> <gzip>` の行と
   中身を送る（見つからなければ `NECODER-ABORT`）。スクリプトはちょうどその長さだけ読む（GNU `head -c` か
   `dd ibs=1`。どちらも余分に読まない＝続く RPC のバイトを食わない）→ 展開 → sha256 を確かめる（道具があれば）→
   `chmod 700` → `mv` → `version` を確かめる → 古い版を消す → `NECODER-READY` → `exec`
4. 以後は binary の RPC

配備先は従来どおり `~/.local/share/necoder/remote/servers/<version>-p<protocol>/necoder-remote-server`。
手元の artifact の場所: `NECODER_REMOTE_SERVER_BINARY` → `~/.local/share/necoder/remote/artifacts/<triple>/` →
mac の `.app/Contents/Resources/remote/<triple>/` → **Windows / Linux の `<exe のフォルダ>/remote/<triple>/`**（インストーラと zip が同梱する）→
同じプラットフォームなら隣の dev ビルド。

## 4. 罠

- **ログインシェルが POSIX とは限らない**: sshd はコマンドを利用者のシェルの `-c` に渡す。fish の単引用符は `\\` と `\'`、
  csh は `!` を解釈する。スクリプトは §3 の文字を使わない（`bootstrap_script_is_safe_for_any_login_shell` テストで固定）
- **`read` 以外で 1 行を読まない**: `head -n 1` などは標準入力を先読みして後ろの RPC を食う。sh の組み込み `read` は
  パイプから 1 byte ずつ読む
- **`.bashrc` の出力**: 非対話の ssh でも bash は `.bashrc` を読む。`NECODER-` 以外の行は捨て、失敗したときだけ最後の数行を見せる
- **`-T` を付ける**: tty を割り当てると改行の変換と echo で binary が壊れる
- **daemon は 1 session = 1 client**: 新しい接続が来たら古い接続を shutdown する。古い TCP の死を sshd が気付くのを待つと、
  再接続が古い接続の後ろで詰まる
- **Windows の ssh.exe は GUI から起こすとコンソール窓が光る**: `child_process::command` を使う（`CREATE_NO_WINDOW`）
- **1 frame は 1 回の書き込みで出す**: header と中身を分けて書くと、proxy / sshd がそれぞれを別の TCP の塊で送り、
  Nagle と相手の遅延 ACK が噛み合って 1 往復ごとに ~40ms 待つ（`-T` の ssh は TCP_NODELAY を立てない）。
  2026-10-21 実測で read_file の往復が平均 45ms → 2ms（`write_frame`）。以前の ControlMaster 経由でも同じだったはず
- **alacritty の EventLoop が見るキーは OS で違う**: unix は読み書き 0 / 子の終わり 1、Windows は 2 / 1。違うキーで知らせると
  EventLoop は黙って捨て、終わる時の「残りを読み切る」でしか読まない（画面が空のまま・打鍵も届かない）。`remote_pty.rs` の
  `READ_WRITE_KEY` と、それを捕まえる試験 `the_event_loop_relays_both_ways_while_the_terminal_lives`
- **remote の worktree の git を UI スレッドで聞かない**: 復元の `hydrate_restored_projects` は `TaskSpace::for_worktree`
  （git に 4 回聞く）を UI スレッドで呼んでいた。Windows で Remote SSH が初めて動いて debug ビルドの検査に当たった。
  git の事実は背景で `WorktreeGitFacts::read` してから `TaskSpace::from_git_facts` する

## 5. 受入条件

- [x] **R1 wire と daemon** — 2026-10-21。Stream frame・Spawn / OpenTerminal / OpenTcp / Attach・流量制御・最新の接続が勝つ daemon
      （`crates/host/src/{stream,server_stream}.rs`）。`stream_tests.rs`: パイプのプロセスの往復・stderr と終了コード・
      起こせない時の理由・窓（1 MiB）を超える出力の欠け無し（その間もファイル操作が詰まらない）・TCP 転送・最新の接続が勝つ。
      **Windows 47 本 / Linux 58 本 緑**（PTY・daemon・`ne` は接続先が unix なので Linux だけ）
- [x] **R2 再接続しても続く** — 2026-10-21。接続を切って繋ぎ直すと、プロセスの stdout が欠けず重複せずに続き、切れている間に
      書いた入力も届く（`a_process_survives_a_dropped_connection`・どの OS でも走る `input_written_while_disconnected_…`）。
      端末のシェルも生きていて、切れている間に打った分が実行される（`a_remote_terminal_survives_a_dropped_connection`）
- [x] **R3 ssh 1 本の transport** — 2026-10-21。ControlMaster を使わない（`-o ControlMaster=no -o ControlPath=none`）。
      bootstrap が同じ接続の中で配備まで済ませる（`ssh_link.rs`）。手元で `/tmp` と `/dev/urandom` を使わない（`getrandom`）。
      **Windows の ssh.exe（OpenSSH_for_Windows_9.5p2）から WSL の sshd へ、実 SSH の試験 10 本がすべて緑（`ne` の試験だけは手元が unix の時に走る）**
      （`scripts/test-remote-ssh-wsl.ps1`: ファイル・検索・コマンド・巨大な木・watch × 2・ssh を落としての再接続・無言で死んだ回線（sshd の SIGSTOP）からの復帰・
      daemon を殺しての再接続・**端末とプロセスが ssh を落としても生き残る**・ベンチ）。初回は配備込みで 1.7 秒、
      2 回目から 0.3 秒で繋がる。read_file の往復は平均 0.9ms（フレームを 1 回で書くように直す前は 45ms・§4）。無言で死んだ回線は凍結を解いて 0.16 秒で戻る
- [x] **R4 端末** — 2026-10-21。`TerminalView::new_remote` が RPC の PTY を alacritty の EventLoop で描く（`remote_pty.rs`・
      `EventedPty` の実装）。**Windows の GUI から WSL の端末を開いて打鍵・色・題名・`uname` が出るのを目視で確認**。
      EventLoop を通した往復と終わりは `the_event_loop_relays_both_ways_while_the_terminal_lives` で固定（Windows / Linux）
- [x] **R5 ポート転送と `ne`** — 2026-10-21。手元の待ち受け + OpenTcp（`forwarded_ports_relay_tcp_both_ways`・Windows でも緑）。
      `ne` は daemon の Unix socket → Event → 手元の client → GUI の制御口（`ne_in_a_remote_terminal_reaches_the_local_gui`）。
      Ports パネルは Windows でも接続先の一覧と転送を出す（手元の一覧だけ未対応のまま）
- [x] **R6 askpass を Windows でも** — 2026-10-21。`control_ipc` の `askpass` と token の発行を unix 限定から外した。
      Windows は GUI のプロセスだけが askpass を付ける（名前付きパイプは在るかを見るだけで接続を 1 つ使うため）。
      パスフレーズの一言は Windows では ssh-agent サービスを起こす手順から案内する。**実機でパスワード / パスフレーズの
      入力欄が出るのは未確認**（試験の sshd は鍵だけ・ユーザー権限の sshd はパスワード認証ができない）
- [x] **R7 配布** — 2026-10-21。`bundle-windows.ps1` が `<exe のフォルダ>/remote/<triple>/` に Linux 用 remote-server を入れ、
      `release.yml` の windows ジョブは remote-servers ジョブの成果物を落としてから呼ぶ（無ければ落とす）
- [x] **R8 文書と UI の言葉** — 2026-10-21。ARCHITECTURE・WINDOWS-PORT §D6・UI-SPEC・ORCA-PARITY・CHANGELOG・UI 文言
- [ ] **R9 mac の確認（実機・ユーザー）**: mac から Linux へ繋いで、ファイル・端末・LSP・ACP・ポート転送・`ne`・スリープ復帰。
      **mac の client のコードは Windows / Linux と同じ道を通る**（分岐は手元の socket の対と askpass の見張りだけ）が、
      mac の実機では試していない
- [ ] **R10 askpass の実機確認（ユーザー）**: Windows / mac でパスワード認証・パスフレーズ付きの鍵・初めてのホスト鍵の確認が
      GUI の入力欄に出て、通る
