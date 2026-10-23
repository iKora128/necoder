# necoder 紹介ムービー

82 秒・1920×1080・60fps。映像は [Remotion](https://www.remotion.dev/)（React）で描き、音楽は `soundtrack/`（Rust・依存は serde だけ）で合成する。録音素材は使わない（猫の声も `scripts/gen-chime.py` が合成した通知音）。

| 出力 | 中身 |
|---|---|
| `out/necoder-promo-ja.mp4` | 日本語版 |
| `out/necoder-promo-en.mp4` | 英語版 |

## 作り直す

```sh
npm install
npm run soundtrack      # public/soundtrack-{ja,en}.wav（数秒）
npm run render:ja       # M3 Ultra で約 1 分半
npm run render:en
npm run studio          # ブラウザで 1 フレームずつ見ながら直す
```

## 構成

120 BPM なので 1 拍 = 0.5 秒 = 30 フレーム。場面は小節で切り、音楽と同じ表を使う（`src/timeline.ts` / `soundtrack/src/main.rs` 冒頭）。

| 小節 | 秒 | 場面 | ファイル |
|---|---|---|---|
| 0–3 | 0–8 | 迷子（同じ見た目の窓 20 枚・送り先を間違える） | `scenes/Lost.tsx` |
| 4–7 | 8–16 | だから作った → 5 色のレールがスネアロールで灯る | `scenes/Built.tsx` |
| 8–9 | 16–20 | ロゴ（ドロップの頭） | `scenes/Logo.tsx` |
| 10–15 | 20–32 | 実機 5 プロジェクトを拍で切替 → レール/タブ/キャレット/送り先 → ⌘O | `scenes/Color.tsx` |
| 16–23 | 32–48 | ACP エージェント → タブで何体でも → Fleet → Captain の任命と分解案 | `scenes/Agents.tsx` |
| 24–27 | 48–56 | かゆいところ: ⌘⇧V で Markdown / HTML プレビュー → Chat モード | `scenes/Touches.tsx` |
| 28–35 | 56–72 | スマホ連携（QR → 一覧 → 承認）→ Remote SSH → Rust × GPUI → 全部入り | `scenes/Power.tsx` |
| 36–40 | 72–82 | necoder の中で作っている → エンドカード | `scenes/Outro.tsx` |

打鍵音・風切り音・UI 音・猫の声の時刻は `src/cues.json` にあり、映像と `soundtrack` の両方が読む。文字数や場面の時刻を変えたら `npm run soundtrack` をやり直す。

## 事実の出所

画面に出る主張は README・LP・`docs/`・`locales/` から取り、作らない（`src/copy.ts` の先頭）。

- 識別色: `theme_core::IDENTITY_PALETTE_HEXES` の巡回 5 色。状態は色相でなく形と動き（UI-SPEC §1.3）
- Captain の任命と分解案: `locales/*.yml` の `captain.appoint_*` / `proposal_*`。Captain はコードを書かず、統合は人間（GLOSSARY）
- スマホ連携: `docs/REMOTE-CONTROL.md` と `relay/README.md`（QR が資格情報でアカウント不要・E2E・Mac に受信ポートを開けない）。Mac 側カードの文言は `settings.remote_*`
- Remote SSH: README（サーバ ~2.4MB の静的 musl・idle 6.5MB・両側 node 不要・OpenSSH の設定をそのまま）
- 性能は数値を出さず「Rust × GPUI・60fps・Electron を積まない」で見せる（LP の表現）

## 実機画面の撮り方

`public/shots/` の PNG は、稼働中の necoder に触れない隔離 offscreen 起動で撮った実画面（`docs/FLEET-V2.md` §10.5 の流儀）。

```sh
rm -rf /tmp/necoder-promo-src && mkdir -p /tmp/necoder-promo-src && git archive HEAD | tar -x -C /tmp/necoder-promo-src
# 撮影専用コピーにだけ NECODER_ACTIVE_PROJECT / NECODER_ACTIVE_OPEN のプローブを足してある（本体には入れていない）
(cd /tmp/necoder-promo-src && CARGO_TARGET_DIR=$PWD/target cargo build -p necoder --features screenshot)
ISO=/tmp/ncp promo/scripts/setup-capture.sh /tmp/necoder-promo-src   # 5 プロジェクト・色・LSP スタブ
ISO=/tmp/ncp promo/scripts/capture-all.sh
```

- 課金が起きる設定（`agent_prewarm` / `tier2_summaries` / `agent_auto_name` / `captain_agent`）は全部切り、`NECODER_ACP_PROBE` は使わない
- rust-analyzer は `NECODER_RUST_ANALYZER` で何もしないスタブに差し替える（本物だと隔離側で `cargo check --workspace` が孤児のまま走り続けた）
- 1 枚ごとに隔離 DB を消す（前の撮影のスレッドやタブを持ち越すと中身が変わる）
- ターミナルを開いた画面にはホスト名が写るので使わない
- HTML プレビューと Chat の成果物は、offscreen では WebView が描かれないので、同じ HTML を Chrome で描いて空欄へはめ込んだ（`public/shots/html-*.png` / `chat-*.png`）

## スマホ画面の撮り方

`relay/test/preview.mjs` と同じ隔離（wrangler dev :8791 のテスト設定 + 偽 GUI・実端末や本物の接続鍵には触れない）で、実物の PWA を iPhone 13 サイズで撮る。
中身は動画の流れに合わせた偽データ（`--data` に渡す snapshot / details）。承認の前後（チェック → 許可 → 「PCが受け付けました」）は、
preview.mjs に 3 手を足したものを relay の外（`/tmp`）で動かして撮った。

## 権利

- フォント: Archivo（可変・Google Fonts から取得）/ IBM Plex Sans JP / Guguru Sans Code はすべて OFL-1.1（`public/fonts/*-OFL.txt`）
- エージェントのロゴ: アプリ同梱の Simple Icons（CC0）。Codex と Grok はアプリと同じくモノグラム
- nyaco のスプライト・ロゴ: アプリと LP の同梱物
