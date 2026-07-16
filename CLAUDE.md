# Shirushi — エージェント作業ガイド

GPUI ベースの自作エディタ **Shirushi（しるし）**。ライセンス **AGPL-3.0**。ドキュメント・コメントは日本語で書く。
コンセプト: 「色による方向感覚」×「プロジェクト/ブランチ横断」×「AI エージェントネイティブ（ACP）」。

## 一次資料（ここを正とする）

| 何を知りたい | どこを見る |
|---|---|
| **何を次に作るか（受入条件）** | `docs/ROADMAP.md` — `/goal` コマンドはこれを上から消化する |
| **どう作るか（設計図）** | `docs/ARCHITECTURE.md` — crate 配置・依存方向・型契約・移植作法 |
| **どう見せるか（UI仕様）** | `docs/UI-SPEC.md` — トークン表・色の許可リスト・領域別仕様・キー表 |
| 直近の文脈・罠 | `docs/JOURNAL.md` — セッションごとの実装日誌 |
| 経緯・ビジョン | `docs/BACKGROUND.md` |
| 設計判断と根拠 | `docs/DECISIONS.md`（AGPL-3.0・ウィンドウモデル・i18n などの決定ログ含む） |
| 実装順序（原案） | `docs/MVP-PLAN.md` |
| 機能バックログ | `FEATURES.md` — **タグ（MVP/v1/later/never）はユーザー管理。勝手に変えない** |
| ビジュアルの正 | `mock/index.html` — 実装の見た目・色トークンはこれに合わせる |
| 3エディタの機能仕様 | `docs/research/`（vscode / zed / cursor / feature-matrix） |
| GPUI の書き方 | `zed/CLAUDE.md` の GPUI 節 + `zed/crates/gpui/examples/` — **現行 API の唯一の正。ネット記事や記憶の API は古い前提で疑う** |

## リポジトリの約束

- `zed/` は参照用クローン（.gitignore 済み）。**変更禁止・勝手に `git pull` しない**（GPUI の API churn で本体が壊れるため。更新はユーザーの判断で行い、追従修正とセットにする）
- GPUI は **git 依存（rev 固定）**（root Cargo.toml。CI ビルド可能化のため path 依存から移行済み・2026-07-15 現在）。ローカル `zed/` は API 調査・example 参照用。**rev 更新はユーザー判断**（`zed/` の pull と同じ扱い・追従修正とセット）
- Zed の GPL crate（`acp_thread` / `agent_servers` / `worktree` / `fs` 等）の**コード移植・改変は禁止**（DECISIONS §5。ライセンスは AGPL-3.0 確定＝2026-07-15。それでも全コードを自作/permissive に保つのは、デュアルライセンス・商用版・将来の緩和という**再ライセンスの自由を本人の手に残すため**。外部貢献を受け始める時は CLA 必須）。**手法の参考は可** — 読み下して自作 or permissive crate で代替し、参考元（crate 名と概ねの時点）をファイル冒頭コメントに残す（実例: git=CLI+imara-diff / terminal=alacritty_terminal / LSP=封筒自作。`docs/research/porting-git-terminal-lsp.md`）
- Apache 系の土台: `gpui`, crates.io の `agent-client-protocol`（ACP プロトコル）

## ビルド・検証ループ（変更のたびに回す）

1. `cargo check -p shirushi` — 速い整合性確認
2. `cargo test -p shirushi` — ロジック
3. UI 変更時: `./scripts/screenshot-app.sh` → 出力 PNG を Read して**目視で**検証（レイアウト崩れ・色）
4. `mock/` 変更時: ヘッドレス Chrome でスクショ → Read で検証:
   ```sh
   "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless \
     --screenshot=/tmp/mock.png --window-size=1680,1000 --hide-scrollbars \
     "file://$PWD/mock/index.html"
   ```
- ツールチェインは `rust-toolchain.toml`（1.95.0）で固定。初回は rustup が自動取得する
- 初回ビルドは GPUI の依存で時間がかかる（数分〜十数分）。壊れたと早合点しない

## 設計原則（UI）

- **色は識別に集約**: レール / タブ下線 / キャレット / スレッド色 / 選択の左バーのみ。グラデ・選択面の色塗りなど装飾には使わない
- スレッド = 色付きタブ。宛先チップ（スレッド名 + プロジェクト ⎇ ブランチ）とトークン常時表示は必須要件（`docs/BACKGROUND.md` の痛点が原点）
- チャットの見た目は VSCode の Claude Code 拡張風（⏺/⎿ トランスクリプト、✳ Thinking、テラコッタ #d97757 はバレットのみ）
- **性能予算**: 入力レイテンシ・起動時間は **Zed 比 ~80% を下限目標**（UX 優先の明示的判断）。ベンチは `zed/crates/editor_benchmarks` / `input_latency_ui` を参考に M2 までに導入

## コーディング規約

- `zed/CLAUDE.md` の「Rust coding guidelines」に従う（`unwrap()` 回避・`let _ =` でのエラー握り潰し禁止・`mod.rs` 禁止・完全な単語の変数名 等）
- crate の依存方向は Zed と同じ向きに保つ: エディタコアは UI 機能・AI・VCS を知らない（逆方向のみ）
- 新機能は「登録式」を優先（コアが機能を知らない構造。`docs/research/feature-matrix.md` §1 参照）
- **i18n 規律（M2 以降）**: UI 文字列のハードコード禁止。全て `t!("領域.キー")` 経由で `locales/ja.yml` + `en.yml` に**両方**書く（ARCHITECTURE §6）

## /goal コマンド

`.claude/commands/goal.md` — 「ROADMAP の次の受入条件を1歩実装する」自走コマンド。
ユーザーが `/goal`（または `/goal <対象>`）と打つと、現在地把握 → タスク選定 → 実装 → 検証 → ROADMAP/JOURNAL 更新まで行う。
このガイドと4枚の設計文書（ROADMAP/ARCHITECTURE/UI-SPEC/mock）が正であり続けることが /goal の前提 — 実装と文書が乖離したら**文書を直す方を先に**。

## マイルストーン（重い Phase 管理はしない）

M0 週末テスト → M1 骨組み → M2 編集+保存 → M3 レール+プロジェクト色 → M4 ACP+スレッド色タブ → M5 エクスプローラ → M6 検索 → M7 言語知能 → M8 Git+ターミナル → M9 Remote SSH（ここまで概ね完了・2026-07-15 時点）→ **M10 毎日使える → M11 言語×Git parity → M12 AI の唯一無二 → M13 公開準備**。
順序の正は `docs/ROADMAP.md`（機能の全量とタグは FEATURES.md）。完了の定義は「ユーザーが日常で触れること」。
