## What & why / 何を・なぜ

<!-- 変更の目的。issue があればリンク -->

## Checklist

- [ ] `cargo test --workspace` green（ja/en parity テスト含む）
- [ ] UI change: offscreen screenshot attached（`./scripts/screenshot-app.sh`）/ UI 変更ならスクショ添付
- [ ] UI strings go through `i18n::t!` and are added to **both** `locales/ja.yml` and `locales/en.yml`
- [ ] No GPL-source code copied, translated, or adapted（Zed の GPL crate 等を取り込んでいない — CONTRIBUTING §2）
- [ ] Dependencies changed? `cargo deny check` passes / 依存を触ったら cargo-deny green
- [ ] CLA signed（初回は bot の案内に従う）
