# MVP計画 —— 「無理かどうか」を最小コストで検証する

目的: エディタ全体をいきなり作らない。**GPUIが自分の手で動かせるか**を最短で確かめ、
少しずつ"エディタらしさ"を積む。**楽しいか苦行か**を体で判定する（＝週末テスト）。

前提: macOS / Rust（`rustup`, `cargo` 済み想定）。`~/Work/experience/necoder/zed` にZedのソースがclone済み。

---

## Step 0: GPUIが動くか即確認（5分）

自分で1行も書く前に、**GPUI公式exampleを走らせる**のが最速の「動く／動かない」判定。

```sh
cd ~/Work/experience/necoder/zed
cargo run -p gpui --example hello_world
```

- ウィンドウが出れば **GPUIはこのマシンで動く**＝土台クリア。
- 初回は依存ビルドで時間がかかる（Zedは巨大）。ここは我慢。
- 他に見て損しないexample（`~/Work/experience/necoder/zed/crates/gpui/examples/`）:
  - `input.rs` … テキスト入力（エディタの核心に一番近い）
  - `list_example.rs` … 仮想リスト（ファイルツリー/スレッド一覧の下地）
  - `data_table.rs` … 表
  - `painting.rs` / `gradient.rs` … 低レベル描画

> ⚠️ GPUIはAPIが動く（churn）。**必ずこの`~/Work/experience/necoder/zed`内の現行exampleを正**とすること。
> ネットの古いサンプルは高確率で今のAPIと合わない。

---

## Step 1: 自分のcrateからGPUIウィンドウを出す（半日）

`~/Work/experience/necoder/` 配下に実験用crateを作る。

```sh
cd ~/Work/experience/necoder
cargo new app --bin
cd app
```

`Cargo.toml` にGPUIをパス依存で足す（安定版が無いのでpath or git）:

```toml
[dependencies]
# ローカルのZedソースを指す（最も確実。API調査もしやすい）
gpui = { path = "../zed/crates/gpui" }
# もしくは git 固定:
# gpui = { git = "https://github.com/zed-industries/zed", rev = "<固定rev>" }
```

`src/main.rs` は **`~/Work/experience/necoder/zed/crates/gpui/examples/hello_world.rs` をコピーして出発点**にする
（現行APIに一致している唯一の信頼できる雛形）。まず空ウィンドウ→テキスト1行描画まで。

---

## Step 2以降: "エディタらしさ"を積む（各ステップ独立に検証）

1. **テキスト1行を描画**（hello_world改造）
2. **ファイルを読んで表示**（`std::fs::read_to_string` → 複数行描画。まずは読み取り専用ビューア）
3. **スクロール**（`list_example.rs` / `scrollable.rs` を参考に、行の仮想化）
4. **キー入力で編集**（`input.rs` を参考に、カーソル・挿入・削除）
5. **rope導入**（`ropey` で大きいファイルの編集を現実的に）
6. **シンタックスハイライト**（`tree-sitter` + 言語grammar。まず1言語）
7. **ファイルツリー**（`list_example.rs` の応用）
8. ここまで来たら「**自分が日常で使える最小エディタ**」＝MVP。

各ステップは**単体で"動いた/動かない"がはっきり**するように小さく刻む。
詰まったら**Zedソースで同じことをどうやっているか grep**（GPUIのdocsは薄いので、ソースが辞書）。

---

## 機能バックログ（層別・タグ運用）

`MVP / v1 / later / never` でタグ付けし、**仕様書ではなく生きたバックログ**として運用。
GitHub Issues+Milestones か、この repo の `FEATURES.md` でも可。

| 層 | MVP | v1 | later |
|---|---|---|---|
| コア編集 | 開く/編集/保存, カーソル, undo | multi-cursor, 矩形選択 | マクロ |
| 描画・UI | 単一pane, 行番号 | tab, split, panel | 分割レイアウト保存 |
| ナビゲーション | file tree | fuzzy find, go-to-line | symbol jump |
| 言語知能 | tree-sitter(1言語) | LSP(補完/診断) | 複数言語, inlay hints |
| 検索置換 | in-file | project-wide, regex | 構造検索 |
| VCS | — | git status/diff | blame, stage |
| Terminal | — | 埋め込み端末 | 複数tab |
| Debug | — | — | DAP |
| **拡張モデル** | — | **UI拡張API（差別化点）** | マーケット |
| 設定 | keymap/theme | settings.json | project-local |

**戦略の再確認**: 機能数でVSCodeに勝とうとしない。**MVP＋"1点だけ"世界一**
（UI拡張性 / スレッド色分け・方向感覚など、そもそもの出発点の不満）。

---

## 判定基準（週末テスト）

Step 0〜2 を実際にやってみて:

- **楽しくて手が止まらない / ソース読むのも苦じゃない** → ビジョン駆動。続ける価値あり。次はStep3〜。
- **依存ビルドの時点で萎える / API churnにイライラ / 苦行** → 答えが出た。
  VSCodeに戻る、または Zed に #53403 等で要望を積む方が費用対効果が高い。

**"無理かどうか"の答えは、議論じゃなく Step 0〜2 を触った時の自分の感情に出る。**

---

## 参考：example ↔ エディタ機能 対応表

| やりたいこと | 見るexample（`~/Work/experience/necoder/zed/crates/gpui/examples/`） |
|---|---|
| ウィンドウ/描画の基本 | `hello_world.rs` |
| テキスト入力・カーソル | `input.rs` |
| 行の仮想リスト（ツリー/一覧） | `list_example.rs` |
| スクロール | `scrollable.rs` |
| 表（データ表示） | `data_table.rs` |
| 低レベル描画・図形 | `painting.rs`, `gradient.rs`, `shadow.rs` |
| ウィンドウ間移動 | `move_entity_between_windows.rs` |

困ったら `~/Work/experience/necoder/zed` 本体を **"GPUIの生きた教科書"** として grep するのが最短。
