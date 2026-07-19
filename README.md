# Shirushi（印）

自作エディタ。**「色による方向感覚」×「プロジェクト/ブランチ横断」×「AI エージェントネイティブ」** の交点を狙う。
目標は **Agent 時代の最高のエディタ**。

![Shirushi — Todo board consumed by the AI agent, with color-coded threads](docs/images/hero-todo-board.png)

## Quick start (English)

Shirushi is a GPUI-based code editor built for the agent era: **orient by color** (per-project colors that run through rails, tabs, carets and AI threads), **work across projects/branches** (worktree-first windows, ⌘O dashboard shows what's running where), and **AI-agent-native** (Claude Code over ACP — colored threads, always-visible token meter, checkpoints, a todo board the agent checks off by itself).

1. Install Rust (the pinned toolchain in `rust-toolchain.toml` is picked up automatically) and run `cargo run -p shirushi`.
2. First keys: `⌘O` open a project / worktree · `⌘P` open a file · `⇧⌘A` start an AI thread (needs the `claude` CLI) · `⇧⌘P` all commands.
3. The UI follows your locale (Japanese / English). Everything AI runs through your existing Claude Code subscription via ACP — no extra API key.

- 名前: **Shirushi**（しるし・印。暫定確定 — ドメイン `shirushi.ai` 取得済み）
- 土台: **GPUI**（Apache-2.0・**git 依存 rev 固定**。ローカル `./zed` は API 調査用の参照クローン）/ Rust 1.95 / まず macOS
- ライセンス: **AGPL-3.0**（確定 2026-07-15。ただし**全コードは自作/permissive 依存**で構成 — Zed の GPL crate はコード移植せず手法のみ参考。再ライセンスの自由を保持・DECISIONS §5）
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

## Remote SSH v1

system OpenSSH の設定、known_hosts、ssh-agent、ProxyJump をそのまま使う。開発ビルドでは先に
remote server バイナリも作り、絶対パスを含む SSH URI を渡す。

```sh
cargo build -p host --bin shirushi-remote-server
cargo run -p shirushi -- 'ssh://user@example.com:22/home/user/project'
```

同じ OS/CPU の接続先には `target/debug/shirushi-remote-server` を自動配備する。異なる target へは、
その接続先向けにビルドした artifact を明示する。

```sh
SHIRUSHI_REMOTE_SERVER_BINARY=/path/to/linux-aarch64/shirushi-remote-server \
  cargo run -p shirushi -- 'ssh://example.com/home/user/project'
```

接続先は status bar の `SSH user@host` として表示され、前回の SSH URI も password 無しで復元する。
現在の v1 は OpenSSH の terminal prompt を使うため、GUI askpass、watch 再同期、配布 artifact の
署名/checksum、実 Linux ホストでの長時間障害試験は未完了。詳細は
[`docs/research/remote-ssh-2026.md`](docs/research/remote-ssh-2026.md) を参照。

## 参照リポジトリ（ローカル）

| リポジトリ | 場所 | 用途 |
|---|---|---|
| Zed | `./zed`（リポジトリ内・git管理外） | GPUI の生きた教科書・GPL 資産の移植元・機能の仕様書 |
| VSCode | `~/Work/vscode` | 機能一覧の正典（contrib構造）・UI拡張APIの参考 |
| Cursor | （ソース非公開） | AI層の仕様は `docs/research/cursor-features.md` 参照 |
