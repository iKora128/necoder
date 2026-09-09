# necoder Remote

Mac / Windows に対応。Node.js 22 以上が必要です。Windows では既存 GUI と同じユーザー別名前付きパイプへ接続します。
ホストの保存先は `%LOCALAPPDATA%\necoder\remote`。Windows の ACL を作成ユーザーと SYSTEM に限定し、ホスト管理 API にも端末内の秘密による認証を要求します。
Windows 配布 zip の `control/host.mjs` とライセンスを exe の隣へ同梱します。Mac で Windows 用のパス生成はテストしていますが、Windows 実機での起動・ACL・パイプ接続の確認は別途必要です。
`ne remote ...` が未インストールなら `necoder.exe remote ...` を使用できます。
IPC の仕様は [Node.js](https://nodejs.org/api/net.html)、ACL は [Microsoft Set-Acl](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.security/set-acl) を参照。

Mac の necoder をスマホの PWA から操作する独立した Cloudflare Worker。
既存の necoder.com の LP は変更せず、control.necoder.com に配置します。

## 構成

スマホ PWA → HTTPS / WebSocket → Cloudflare Durable Object ← outbound WebSocket ← Mac のホスト → Unix socket → necoder / ACP agent。
Mac に受信用ポートや SSH を公開しません。コード実行・セッション・履歴の正本は Mac にあります。
既存の ACP セッションを操作するため、別の Claude / Codex プロセスをスマホ用に二重起動しません。

## セットアップ

Node.js 22 以上、更新版 necoder、Cloudflare Workers / Durable Objects を使用できるアカウントが必要です。

```sh
cd relay
npm ci
npm run build
npm run check
npm test
npx wrangler login
npm run deploy
node scripts/provision.mjs
```

`scripts/provision.mjs` は初回の秘密を生成し、Mac/Windows の保護ファイルと Worker secret に保存します。既存の鍵を保持して再実行できます。
別の PC を追加する際は初回 PC の provisioning token を安全に渡して `init` してください。既存 relay の秘密を新しい値で上書きすると他の PC がペアリングできなくなるため、スクリプトはそれを拒否します。
手動設定する場合は、暗号学的乱数の 32 byte 以上の provisioning token を `wrangler secret put PROVISION_TOKEN` で登録してください。
同じ値を Mac だけに `NECODER_PROVISION_TOKEN` 環境変数で渡して初期化します。公開 JS、URL、Git に入れないでください。

```sh
ne remote init https://control.necoder.com
ne remote pair iPhone
ne remote status
ne remote revoke <device-id>
ne remote stop
```

開発時は `ne remote` の代わりに `node relay/dist/host.mjs` を使えます。
`init` は既存の設定・鍵を上書きしません。設定は `~/.necoder/remote/config.json`、端末情報は `devices.json` に保存（ディレクトリ 0700、ファイル 0600）。
`pair` はホストを必要に応じてバックグラウンド起動します。Mac 再起動後は `ne remote start` を実行してください。
`NECODER_REMOTE_HOME` でホストの状態保存先、`NECODER_GUI_SOCK` で接続先 GUI socket / 名前付きパイプを変更できます。GUI の `NECODER_HOME` は IPC のパスを変えません。
`ne remote pair iPhone <task-id> ...` は指定プロジェクトだけを共有します。省略すると、その時点で開いているプロジェクトだけを共有します。
後から開いた別プロジェクトは自動共有されません。端末追加は最大 8 台、QR は 5 分で期限切れです。

iPhone はまず固定 HTTPS URL を Safari で開き「ホーム画面に追加」。追加した PWA 内に Mac の新しいペアリング URL を貼り付けて接続してください。
QR を Safari で読み取った場合、ホーム画面の PWA と保存領域が別なら再ペアリングが必要です。
通知はホーム画面に追加した対応 iOS とユーザーの明示許可が必要です。複数 Mac の通知を同じ PWA で使う場合は各 Mac の VAPID 鍵を揃えてください。

## 操作と切断

プロジェクト / スレッド一覧、直近の会話、指示送信、新規スレッド、実行中断、追跡ファイルの Git diff、今回だけの実行許可・拒否、選択式質問への回答に対応します。
常時許可、任意 shell、任意ファイルの読み書き、設定変更は公開 API に含めません。
実行中の追加指示は、いったん中断するか完了後に送信してください。ローカルの agent に設定済みの権限モードは継続し、bypass モードを PWA が強制的に承認必須に変えるわけではありません。
承認に必要な入力・差分が取得できない、または大きすぎる場合、スマホでは許可できません。Mac で確認してください。
本文は直近 60 項目、各 4 KB に制限。許可対象の差分を黙って切り詰めることはありません。
Git diff は HEAD と追跡ファイルの比較で、未追跡ファイルは含みません。

PWA を閉じても Mac の処理は PWA から独立しています。復帰時は新しい暗号セッションを作り、Mac から最新状態を再取得します。
Mac のスリープ・電源断・回線断の間は操作できません。Mac 自体の回線断は agent の API 通信も止める可能性があります。
スリープを避ける場合は電源に接続し、必要な時間だけ macOS の設定や `caffeinate -i` を利用してください。
回線復旧後にホストが再接続します。GUI 再起動は別世代として扱い、古い承認は拒否します。
命令の期限は 25 秒。オフライン中の命令はキューに入れず、勝手に再送しません。
受付記録を IPC より前に永続化し、同じ request ID の二重実行を防ぎます。クラッシュ等で結果不明の場合は会話を確認してから再操作してください。
Mac が 30 日間 room を更新できないと relay room は期限切れになり、再ペアリングが必要です。

## セキュリティと運用

ペアリングの共有秘密で相互認証し、接続ごとの P-256 ECDH + HKDF + 方向別 AES-GCM を使用します。
連番とセッション識別子で改ざん・再送・別接続からの流用を拒否します。ペアリング成功時に QR の秘密を別の長期鍵へ置換します。
Cloudflare は通信内容の復号鍵を持ちませんが、接続元・時刻・通信量は観測でき、通信を止めることはできます。
PWA の配信元や Cloudflare アカウントを乗っ取られると、改変 JS によって端末内の鍵を盗まれるリスクは残ります。
Cloudflare アカウントの MFA、最小権限のデプロイ資格情報、Mac のディスク暗号化とロックを推奨します。独立した暗号 / セキュリティ監査済みではありません。
PWA の保存鍵は IndexedDB の非抽出 CryptoKey で暗号化しますが、同じ origin の悪意ある JS に対する保護ではありません。
紛失時は Mac で revoke。これは回線断中でもローカルのアクセス権を直ちに失効します。スマホの「接続を削除」だけでは Mac 側の登録は失効しません。
Push に会話・コード・プロジェクト名を含めません。通知の配信は OS と push service 次第であり、保証されません。
リレーは hibernation、サイズ / レート制限、期限付き room を使用。アカウント全体の料金上限や DDoS 予算保証ではありません。
費用は契約プラン・通信回数・DO ストレージ利用量によります。Cloudflare の利用状況を確認し、必要なら課金アラートを設定してください。
既存の necoder.com のサブドメインと Cloudflare 管理証明書を使うため、この構成のための別ドメイン購入は不要です。

## 検証

```sh
npm run check
npm test
npm run build
npx playwright install chromium webkit
npm run test:browser
```

ブラウザ統合テストはローカル Worker と隔離された fake GUI を起動し、ペアリング・操作・切断復旧・承認・失効を検証します。
`npm run test:native` は `target/debug/necoder` を一時設定・一時 Git repo で起動し、実 GUI IPC を検証します（Mac 専用。ユーザーの GUI を終了しません）。
`npm run test:production` は設定済みの本番 relay に一時 room を作成し、ダミー状態の暗号化通信と不正命令拒否を検証した後、room を削除します。実プロジェクトや登録済み端末を操作しません。
実機 iPhone のホーム画面インストール、通知許可 / 配信、OS バックグラウンド復帰は別途実機で確認してください。
アプリ配布時は scripts/bundle-mac.sh がホストを Resources/control/host.mjs に同梱します（実行には Node.js が必要）。
公開する relay の対応ソースはビルド時に source.tar.gz として生成し、PWA からダウンロードできます。
