# RELEASE — 公開とリリースの手順（人の手番チェックリスト）

M13「英語話者が DL して 10 分」の実運用手順。ソフト側の導線（署名 dmg CI・自動更新・EN UI・
welcome・クラッシュ報告）は実装済み — ここに残っているのは **GitHub 設定とタグ操作という人の手番**。

## 0. 一度だけ: リポジトリ設定（public 化の前後）

- [ ] **Apple 署名 secrets を入れる**（karui リポジトリと同じ 6 つ・release.yml 冒頭コメント参照）:
      `APPLE_SIGNING_IDENTITY` / `APPLE_CERTIFICATE_BASE64` / `APPLE_CERTIFICATE_PASSWORD` /
      `APPLE_API_KEY` / `APPLE_API_ISSUER` / `APPLE_API_KEY_BASE64`
      （空のままでも動くが**未署名 dmg** になり、自動更新の spctl 検証が通らない）
- [ ] Settings → Security → **Private vulnerability reporting を有効化**（SECURITY.md の導線先）
- [ ] Settings → Branches → main の保護（force-push 禁止・必須 CI: test-macos / audit-deps）
- [ ] （public 化）Settings → General → Visibility を Public へ
- [ ] public 化後に cla.yml が動くことを確認（テスト PR に bot コメントが付く）

## 1. リリースを切る（毎回）

1. [ ] `CHANGELOG.md` の Unreleased を版へ繰り上げ（**この節がそのまま Release ページの本文になる**。
       `scripts/release-notes.sh <version>` で CI と同じ出力を手元で確認できる。節が無いと
       release.yml が落ちる）
2. [ ] `Cargo.toml` の `[workspace.package] version` を上げる（唯一の出所。Info.plist へは
       bundle-mac.sh が注入・タグとの一致は release.yml が検証して不一致なら fail）
3. [ ] `cargo test --workspace && cargo deny check` green を確認
4. [ ] `git tag v<version> && git push origin v<version>` → CI が署名+公証済み dmg を Release に添付
       し、本文を CHANGELOG から書き込む。公開後に本文を直すときは
       `scripts/release-notes.sh <version> | gh release edit v<version> --notes-file -`
5. [ ] Release ページの自動生成 dmg を**実際に DL して**新規マシン相当（隔離属性つき）で開く:
       Gatekeeper 警告なしで起動・「10 分コース」（開く→編集→保存→検索→AI 1 タスク）を通す
6. [ ] ゴミ箱（Finder AppleScript fallback）と `claude` 子プロセス起動が**公証ビルドで**動くことを確認
       （hardened runtime + entitlements の実地検証・初回リリースで特に重要）

## 2. 自動更新の E2E（v0.1.1 で一度だけ）

- [ ] v0.1.0 を入れた状態で v0.1.1 をリリース → 10 秒後に statusbar へ「⬆ v0.1.1 に更新」チップ
      → クリック → 署名検証 → 差し替え →「⟳ 再起動で反映」まで通しで確認
- 注: リリース確認は**未認証の GitHub API** なので、リポジトリが private のうちは 404 になり
      チップは出ない（= 本当の E2E は public 化後）

## 3. 公開後の伸びしろ（順不同）

- [ ] Homebrew cask（リリース済み dmg の sha256 が要るのでリリース後）
- [ ] Intel (x86_64) mac ビルドの提供判断（現在は macos-14 = Apple Silicon のみ）
- [ ] DMG の背景画像（現在は Applications リンクのみ）
- [ ] Linux GUI ビルドの実機検証（CI の check-linux は continue-on-error のまま）
