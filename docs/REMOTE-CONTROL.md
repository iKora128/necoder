# Remote PWA — 実装・運用状況

公開 URL: https://control.necoder.com （既存の necoder.com とは別 Worker）。
セットアップ・セキュリティ境界・切断時の動作は [relay/README.md](../relay/README.md)。

## このチェックアウトから使う

Node.js 22 以上が必要です。この Mac の relay 接続設定と Cloudflare secret は設定済みです。
起動中の旧 necoder を保存して終了し、更新ビルドを起動します（既存のユーザーアプリは実装作業で終了・上書きしていません）。

```sh
./target/debug/necoder
```

スマホに共有したいプロジェクトを開き、**歯車 → スマホ連携 → 接続用QRを発行** を押します。
設定画面内にQRと「ペアリングURLをコピー」が表示されます。QRは5分後に自動で非表示になり、再発行できます。
「QRを隠す」は表示だけを閉じます（発行済みQRの即時失効ではありません）。発行時に開いている全プロジェクトが共有対象です。

ターミナルから発行する方法も引き続き使えます。

```sh
./target/debug/necoder remote pair iPhone
```

iPhone で固定 URL をホーム画面に追加し、PWA にペアリング URL を貼り付けます。QR / URL は 5 分有効です。
配布版の更新後は `ne remote pair`、Windows では `necoder.exe remote pair` が使えます。
Windows 用バイナリの生成と実機検証は、この Mac では行っていません。Windows CI にホスト起動・名前付きパイプ認証・配布同梱の検査を追加済みです。

## QR が発行できないとき（2026-09-11）

設定画面の失敗表示は**原因ごとに違う一文**を出す。総称の一文（「原因不明」）が出た時だけ、
ターミナルで `ne remote pair` を実行すると生の理由が見られる。

- 画面に出す文字列は `crates/settings/src/remote.rs` の allowlist（`KNOWN_FAILURES` →
  `failure_message`）を通したものだけ。**host の生 stderr は画面にもログにも出さない**
  （ペアリング URL やローカルのパスが混ざり得るため）。新しい失敗コードを host 側に足す時は、
  この allowlist と `locales/ja.yml` / `en.yml` の `settings.remote_err_*` も一緒に足す。
- v0.1.14 までは **release ビルドが管制 IPC を一度も起動していなかった**（`main.rs` の呼び出しが
  開発用 probe の `#[cfg(debug_assertions)]` ブロックの中にあった）。この版の .app では QR は
  必ず失敗する。0.1.15 以降で修正済み（JOURNAL 2026-09-11）。
- 最頻の原因は `necoder_not_running_or_update_required` = **ホストが GUI と話せない**。
  Remote host は `~/.necoder/gui.sock`（管制 IPC）越しに `remote_snapshot` を取るので、
  GUI が socket を張っていないと「necoder が起動していない」と判断する。GUI 側は 30 秒ごとに
  `control_ping` で持ち主を確かめ、見失っていれば張り直す（`control_ipc.rs`）。
  それでも出るなら `lsof -U -p <GUI の pid> | grep gui.sock` で持ち主を確かめる。

## 2026-09-10 の確認

- Cloudflare custom domain / HTTPS / Worker / SQLite Durable Object / provisioning secret の配置に成功。
- 本番 relay に一時 room を作り、暗号化ペアリング、QR 鍵の置換、認証済み snapshot、不許可 API 拒否を確認して room を削除。
- ホスト / 暗号 / アクセス制御 / OS パス / ローカル認証の Node テスト 11 本が成功。
- Chromium と WebKit のモバイル表示で統合テスト 4 本が成功。ペアリング、共有限定、送信、差分、承認、再読込、GUI 停止復旧、ホスト回線断復旧、失効を検証。
- 実 GUI を一時状態・一時 Git repo で起動し、snapshot、diff、新規スレッド、詳細取得、期限切れ / 古い世代 / 古い turn の拒否を確認。agent の外部 API は呼んでいない。
- `cargo test -p necoder -p agent_panel -p cli_shim --features necoder/runtime-shaders` とビルドが成功。
- 実機 iPhone のインストール・通知許可・Push 配信、Windows 実機の ACL・GUI 連携は未検証。独立したセキュリティ監査は未実施。
