# Shirushi（印）

自作エディタ。**「色による方向感覚」×「プロジェクト/ブランチ横断」×「AI エージェントネイティブ」** の交点を狙う。
目標は **Agent 時代の最高のエディタ**。

- 名前: **Shirushi**（しるし・印。暫定確定 — ドメイン `shirushi.ai` 取得済み）
- 土台: **GPUI**（Apache-2.0, `./zed/crates/gpui` を path 参照）/ Rust 1.95 / まず macOS
- ライセンス: **AGPL-3.0**（2026-07-11 決定 — Zed の GPL 資産 `acp_thread`/`agent_servers`/`worktree`/`fs` を移植して使う戦略とセット）
- AI: 自前エージェントは作らず **ACP クライアント**（`agent-client-protocol` crate + `claude-agent-acp` で Claude Code サブスクがそのまま動く）
- 性能予算: 入力レイテンシ・起動 **Zed 比 ~80% を下限目標**（UX 優先の明示的判断）

## リポジトリ構成

```
editor/
├── README.md            ← このファイル（ハブ）
├── CLAUDE.md            ← エージェント作業ガイド（規約・検証ループ・一次資料の場所）
├── FEATURES.md          ← 生きた機能バックログ（MVP / v1 / later / never）
├── LICENSE              ← AGPL-3.0
├── Cargo.toml           ← workspace（crates/* / zed は exclude）
├── rust-toolchain.toml  ← 1.95.0（zed と同一チャンネル）
├── crates/
│   └── shirushi/        ← 本体 bin crate（M1 の骨組みウィンドウ）
├── scripts/
│   └── screenshot-app.sh ← UI 検証用スクショ
├── .claude/commands/
│   └── goal.md          ← /goal コマンド（ROADMAP を1歩ずつ自走実装）
├── docs/
│   ├── ROADMAP.md       ← 受入条件つきマイルストーン（/goal が消化する）
│   ├── ARCHITECTURE.md  ← 実装設計図（crate 配置・型契約・移植作法・i18n）
│   ├── UI-SPEC.md       ← UI 実装仕様（トークン表・色の許可リスト・領域別）
│   ├── JOURNAL.md       ← 実装日誌（セッションごとの学び）
│   ├── BACKGROUND.md    ← 検討の経緯・Zed探索の記録
│   ├── DECISIONS.md     ← 設計判断と根拠 + 決定ログ
│   ├── MVP-PLAN.md      ← 週末テスト〜MVPの実行計画（原案）
│   └── research/        ← 3エディタ機能全列挙 + 横断マトリクス
├── mock/
│   ├── index.html       ← ビジュアルモック v0.3（ブラウザで開くだけ）
│   └── README.md        ← モックで何を決めるか
└── zed/                 ← Zed ソース一式（参照用クローン、git 管理外・変更禁止）
```

## いま何をするか（順番）

1. **M0 週末テスト**: `cd zed && cargo run -p gpui --example hello_world`
   （Metal Toolchain は導入済み 2026-07-11）
2. **本体の起動確認**: `cargo run -p shirushi`（骨組みウィンドウ）
3. **ビジュアル微調整**: `open mock/index.html` — 決め残しは mock/README.md 参照
4. 以降は **`/goal`** — [`docs/ROADMAP.md`](docs/ROADMAP.md) の受入条件を上から1歩ずつ自走実装

## 参照リポジトリ（ローカル）

| リポジトリ | 場所 | 用途 |
|---|---|---|
| Zed | `./zed`（リポジトリ内・git管理外） | GPUI の生きた教科書・GPL 資産の移植元・機能の仕様書 |
| VSCode | `~/Work/vscode` | 機能一覧の正典（contrib構造）・UI拡張APIの参考 |
| Cursor | （ソース非公開） | AI層の仕様は `docs/research/cursor-features.md` 参照 |
