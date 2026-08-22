# Contributing to necoder / コントリビュートガイド

Thanks for your interest! Issues (bugs, feature requests) are always welcome — Japanese or English, either is fine.
バグ報告・機能要望の issue は日英どちらでも歓迎です。PR を送る前に、以下の約束だけ読んでください。

## 1. CLA — required for every contribution / 全コントリビュートで必須

necoder is **AGPL-3.0-or-later**, and the maintainer keeps the freedom to relicense (including dual/commercial licensing) as an explicit project decision ([docs/DECISIONS.md](docs/DECISIONS.md) §5). To keep that possible, **every contribution requires signing the [CLA](CLA.md)** — it grants the maintainer a license to your contribution that includes relicensing rights. A bot will guide you through signing on your first PR (one comment, one time).

日本語要約: 本プロジェクトは AGPL ですが、再ライセンスの自由（デュアルライセンス等）をメンテナに残す方針です。そのため全ての PR に [CLA](CLA.md) への署名（初回 PR で bot がコメント案内・一度だけ）が必要です。**署名した貢献は将来 AGPL 以外のライセンスでも配布されうる**ことに同意いただきます。

## 2. The clean-room rule / クリーンルーム規律（最重要）

- The local `zed/` directory is a **reference clone** (gitignored). GPUI itself is Apache-2.0 and used as a dependency — that is fine.
- **Never port or adapt code from Zed's GPL crates** (`acp_thread`, `agent_servers`, `worktree`, `fs`, `editor`, …) or from any other GPL/proprietary codebase. Studying *techniques* and re-implementing from understanding is allowed; copying/translating code is not.
- When a module's approach was informed by another project, say so in the file-header comment (crate name + rough date), as existing modules do.
- CI runs [cargo-deny](deny.toml): new dependencies must be permissively licensed (MIT/Apache/BSD/ISC/MPL-2.0…). No GPL/AGPL dependencies.

## 3. Code & UI rules / コードと UI の約束

- Rust: no `unwrap()`/`expect()` on production paths, no `let _ =` to swallow errors, no `mod.rs`, full-word variable names. (We follow Zed's published Rust guidelines in spirit.)
- Crate direction: the editor core must not know about UI features, AI, or VCS — dependencies point inward only. Prefer registration-style seams (commands, status items, panels).
- **i18n**: no hardcoded UI strings. Everything goes through `i18n::t!("area.key")` and must be added to **both** `locales/ja.yml` and `locales/en.yml` (a parity test fails otherwise).
- **Color discipline**: identity colors (rail/tab/caret/thread) only — never decorative color. See [docs/UI-SPEC.md](docs/UI-SPEC.md).
- Comments and docs are primarily Japanese; English is fine in PRs from non-Japanese speakers.

## 4. Verify before you push / 検証ループ

```sh
cargo check -p necoder        # fast integrity check
cargo test --workspace         # includes the ja/en parity test and fuzz smoke
./scripts/screenshot-app.sh    # UI changes: attach the offscreen PNG to your PR
cargo deny check               # if you touched dependencies
```

Docs are the source of truth (`docs/ROADMAP.md` / `ARCHITECTURE.md` / `UI-SPEC.md`). If implementation and docs drift, fix the docs first.

## 5. Sending the PR

- Keep PRs small and focused; describe *why*, not just *what*.
- The PR template has a checklist (tests, i18n parity, clean-room, CLA) — please fill it honestly.
- CI must be green (macOS tests + perf budget guard + dependency audit).

Security issues: do **not** open a public issue — see [SECURITY.md](SECURITY.md).
