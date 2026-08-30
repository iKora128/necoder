# Remote SSH 2026 — necoder 実装方針

調査日: 2026-07-13

## 結論

necoder の Remote SSH は、リモートファイルを SFTP でローカルへ見せる機能ではない。
**ローカル UI + SSH transport + リモート常駐サーバー**の分散エディタとして作る。

```
local                                              remote
┌──────────────────────────┐      OpenSSH       ┌──────────────────────────┐
│ GPUI / input / clipboard │◀──────────────────▶│ necoder-remote-server   │
│ tree-sitter / theme / UI │  length-framed RPC │ worktree + watcher       │
│ dirty buffer backup      │                    │ file I/O + search + Git  │
│ SSH config / credentials │                    │ LSP + PTY + tasks + ACP  │
└──────────────────────────┘                    └──────────────────────────┘
```

これは Zed と VS Code が独立に到達している境界と一致する。UI はローカル、workspace に属する
処理はコードと同じマシンで動かす。`std::fs` や `std::process` を SSH コマンドへ一個ずつ置換する
設計は、往復遅延、引用、認証、再接続、PTY、watcher の全てで行き止まる。

## 2026-07-13 実装状況

この文書の後半は完成形の設計であり、現時点の実装済み範囲は次のとおり。

| 領域 | 実装済み | 未完了 |
|---|---|---|
| 境界 | `Host`、`LocalHost`、`RemoteHost`。project/editor/search/Git/LSP/terminal/ACP を接続 | watcher、task/debug adapter、formatter |
| wire | version/magic/request ID/型付き JSON header/raw body/frame 上限、8 worker multiplex | stream/event/cancel、compression、backpressure telemetry |
| file | root scope、symlink escape 拒否、batch list/search、revision 付き atomic save | watch event、rename/delete、巨大 file streaming |
| SSH | system OpenSSH、ControlMaster、alive option、same-target 自動配備、明示 cross artifact | GUI askpass、release download、署名/checksum、artifact cleanup |
| 復旧 | session daemon/proxy、5秒 heartbeat、lazy reconnect、master 再生成、project reopen | backoff/jitter UI、LSP/PTY handle 再同期、dirty buffer crash backup |
| UX | `ssh://` 起動/状態復元、status bar host 表示、検索の background 実行 | host picker、接続ログ、retry/cancel、port forwarding、trust UI |
| 検証 | full-duplex protocol、root escape、競合保存、process 並行性、real daemon/proxy 再接続 | 実 Linux、VPN/sleep/SSH kill、長時間運用、性能計測 |

したがって、これは実際に編集できる Remote SSH v1 の土台だが、まだ「Zed並みに完全」という受入状態ではない。

## 2026-07 時点で確認した事実

### Zed

このリポジトリの参照用 `zed/` は commit `5f8a7413a317`（2026-07-10）で、公開安定版 1.10.2
とほぼ同時点である。現行実装は次の構成になっている。

- `crates/remote`: SSH transport、ControlMaster、askpass、バイナリ配備、長さ付き Protobuf RPC、
  heartbeat、再接続、connection pool。
- `crates/remote_server`: headless project。worktree、buffer、Git、LSP、task、terminal、agent store を
  リモート側に組み立てる。
- 接続断時は remote server daemon を残し、proxy が再接続する。クライアントは 5 秒間隔で接続活動を
  見て、5 回の missed heartbeat 後に再接続へ移る。
- system `ssh` を使うため `~/.ssh/config`、ssh-agent、ProxyJump、PKCS#11 等を再実装しない。
- server binary は OS/arch とクライアント版に一致させる。Linux 配布物は musl/static で古い glibc や
  Nix 系でも動くようにする。
- ソース、LSP、task、terminal は remote。UI、tree-sitter、未保存変更、最近使った project は local。
- 同一の ControlMaster を protocol、terminal、task の接続で再利用する。

公式資料:

- [Zed Remote Development](https://zed.dev/docs/remote-development)
- [SSH Remoting is Here](https://zed.dev/blog/remote-development)

Zed の実装は高水準の設計比較に限って参照し、GPL アプリケーションコードは necoder へ取り込まない。

2026-06-24 の利用者記事にある「VS Code server 8.3 GB 対 Zed 約 570 MB」「接続が速い」は、
その利用者の3ホストにおける実測であり一般化できるベンチではない。単一の小さな Rust server、
ControlMaster、daemon 再利用という説明は Zed の公式資料との比較情報として扱う。

- [VSCodeからZedに乗り換えたら、軽いうえに大学鯖にも優しくなれた話](https://zenn.dev/toramutton/articles/zed-debut-ssh)

### VS Code

VS Code も UI extension は local、workspace extension は remote server で実行する。server と client は
機能互換のため版を一致させ、自動導入する。Remote SSH の障害例からは、server/extension host の重量、
sleep/VPN 復帰、再接続 token、複数接続モードが実運用の主要な難所だと分かる。

- [Remote Development using SSH](https://code.visualstudio.com/docs/remote/ssh)
- [Supporting Remote Development](https://code.visualstudio.com/api/advanced-topics/remote-extensions)
- [Remote SSH Troubleshooting](https://github.com/microsoft/vscode-remote-release/wiki/Remote-SSH-troubleshooting)

### OpenSSH

transport は独自 SSH library ではなく system OpenSSH を使う。`ControlMaster` は1本の暗号化接続上で
複数 session を共有できる。`ControlPath` は `%C` 相当で接続を一意化し、他ユーザーが書けない
ディレクトリへ置く必要がある。`ServerAliveInterval` は protocol-level keepalive で、既定は無効。
necoder 自身の application heartbeat と役割を分ける。

- [OpenSSH ssh_config(5)](https://man.openbsd.org/ssh_config)

## necoder の境界

### ローカルに残す

- GPUI window、描画、入力、IME、clipboard、theme、keymap、project rail。
- tree-sitter の構文表示。入力ごとに SSH 往復を発生させない。
- dirty buffer と crash recovery backup。接続断でも編集内容を失わない。
- SSH config、known_hosts、agent、鍵。password は設定へ永続化しない。
- AI provider の API key と OS keychain。remote へ暗黙転送しない。

### リモートで動かす

- ファイル走査、gitignore、watcher、read/write/rename/delete。
- project search。ファイルを全件 local へ転送して検索しない。
- Git CLI と worktree 操作。
- language server と formatter。ソースと同じ filesystem/environment を使う。
- PTY、task、debug adapter。
- ACP coding agent。remote filesystem を tool から直接扱う必要があるため。ただし認証情報は自動転送せず、
  remote での明示ログインか、将来の capability 制限付き credential broker を使う。

### 共通 Host API

最初に `Host` 境界を作り、local もその実装の1つにする。remote のためだけの分岐を UI 全体へ散らさない。

- `FileSystem`: metadata/read/read_dir/write atomic/watch。
- `Process`: spawn/stdin/stdout/stderr/exit/cancel/environment。
- `Pty`: spawn/input/output/resize/kill。
- `Git`: 実装は generic command でもよいが、UI へは型付き結果を返す。
- `Search`: server-side stream + cancel。

path は local `PathBuf` と混同せず、`HostId + RemotePath` で識別する。同じ `/home/me/code` でも host が
違えば別 project である。永続化キーも `ssh://user@host:port/path` の正規形を使う。

## Protocol

v1 から次を満たす。

- magic、protocol version、request id、message kind、header length、body length を持つ length-prefixed frame。
- metadata は型付き・後方互換を考慮して version/capabilities を handshake する。
- file/PTY/process output は JSON の byte array にせず raw body で送る。
- header/body の上限を検証し、壊れた peer による無制限 allocation を防ぐ。
- request/response、server event、stream、cancel を区別する。
- write は read 時の revision を条件にした optimistic concurrency + 同一ディレクトリ内 temp file から
  atomic rename。外部変更を黙って上書きしない。
- protocol の stdout と server log の stderr を分離する。
- path request は open 済み worktree に scope する。`..` と不正な absolute path を拒否する。

Protobuf は Zed 規模では有効だが v1 の必須条件ではない。necoder は versioned JSON header + raw body で
開始し、wire codec を1 crateへ閉じ込める。型と frame 境界を守れば後から Protobuf/postcard へ交換できる。

## 接続と再接続

1. system `ssh` で ControlMaster を作る。host key 確認や鍵 passphrase は UI askpass へ中継する。
2. remote OS/arch/shell/server version を検出する。
3. 一致する static server を `~/.local/share/necoder/remote/servers/<version>/` へ配備する。
   remote download と local download + SFTP/SCP upload の両方を用意する。
4. `proxy --session <random-256-bit-id>` を起動する。daemon が無ければ開始し、あれば再接続する。
5. 5 秒の application heartbeat、jitter 付き exponential backoff、手動 retry/cancel を実装する。
6. reconnect 後に watcher subscription、open buffer revision、PTY/LSP handle を capability ごとに再同期する。

ControlMaster の socket は private temp/cache directory に置き、connection identity は host の文字列だけでなく
user、port、ssh config、ProxyJump と主要 option を含める。port forward は既定で localhost bind とする。

## セキュリティ

- `StrictHostKeyChecking=no` を設定しない。OpenSSH 既定の ask/known_hosts を尊重する。
- user が入力した SSH option は allowlist parser を通す。necoder が所有する `-M/-S/-T/-N/-O` を上書きさせない。
- shell command は文字列連結しない。固定 bootstrap 以外は RPC 後に server の process API で argv として渡す。
- control socket directory は owner only。session id は十分な entropy を持たせ、ログへ出さない。
- agent forwarding (`-A`) と remote port forwarding (`-R`) は明示 opt-in。
- workspace trust 前は task/LSP/agent/project settings の自動実行を止める。
- log は destination と状態を残すが、password、token、環境変数値、完全な private key path は redact する。

## 性能予算

- 接続済み host の project open: p50 300 ms 以下、p95 1 s 以下（server/LSP warm）。
- cold bootstrap は工程別時間を記録し、接続、platform detect、download/upload、server start、scan を分離する。
- editor input と local syntax highlight は network latency 0。
- file open は1往復、save は1往復。directory/watch/search は batch/stream。N files = N SSH process は禁止。
- protocol compression は計測後。まず SSH compression (`-C`) をユーザー設定に委ね、小ファイルでのCPU悪化を避ける。
- remote server idle CPU は watcher event 待ちで 0%、project ごとの常駐メモリを計測する。

## 段階的な完成条件

1. **境界**: local が `Host` 経由でも全機能・性能を維持する。
2. **Files v1**: `ssh://host/path` で tree/open/edit/conflict-safe save/watch/search が動く。
3. **Workspace v1**: Git、LSP、PTY、task、ACP が remote で動く。local process を誤起動しない。
4. **Reliability**: sleep、VPN断、SSH kill、server restart を注入し、dirty buffer を失わず再接続する。
5. **Distribution**: Linux x86_64/aarch64 musl + macOS x86_64/aarch64、version pin、署名/checksum、古い版 cleanup。
6. **UX**: Remote Projects、SSH config host picker、接続ログ、retry/cancel、port forward、host badge。

「Zed並みに完全」は機能数ではなく、この境界を最後まで守り、接続断と外部変更をテストすることで達成する。
