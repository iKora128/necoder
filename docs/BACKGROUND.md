# 自作エディタ検討メモ（GPUIベース）

作成: 2026-07-11 / 前提: macOS, Rust

Zedのエージェントまわりの不満（スレッド/プロジェクトの方向感覚が無い・タブが無い・
色分けできない）を詰めていった結果、「**自分の理想のエディタを建てる**」という話に発展した。
その検討の記録と、実際に手を動かすための計画。

---

## TL;DR（結論）

- **やる価値の判定**: 「スレッド色が欲しい」だけならエディタ自作はオーバーキル。
  だが **"自分の信じるエディタを建てたい"という独立した情熱があるなら正当な多年プロジェクト**。
- **作り方**: Zed の GPL アプリケーションコードは fork せず、公開仕様とライセンス適合を確認した
  描画・編集部品の上に necoder 固有のエディタを実装する（正確な依存境界は DECISIONS §5）。
- **フレームワーク**: **GPUI**（速さ・ネイティブが魂なら）か **Tauri**（Web技術・UI拡張の楽さなら）。
  → 今回は **GPUI** に傾倒。ただしGPUIは「Zedのために作られた」性質（API churn・薄いdocs）を背負う。
- **マルチプラットフォーム**: GPUIは **macOS / Linux / Windows** 対応（Web・モバイルは不可）。
  Windowsが一番荒い。
- **ライセンス**: necoder は AGPL-3.0-or-later。GPUI の固定依存グラフに含まれる GPL 推移依存との
  組み合わせを含め、正確な境界は DECISIONS §5 と THIRD_PARTY_NOTICES.md を正とする。
- **次の一手**: [`MVP-PLAN.md`](./MVP-PLAN.md) — まず比較用 `zed/` でGPUIのhello_worldを走らせて
  「動くか」を最小コストで確認する。

---

## 目次
1. [経緯（なぜこの話になったか）](#経緯)
2. [Part 1: Zed探索で分かったこと（背景・リファレンス）](#part-1-zed探索で分かったこと)
3. [Part 2: 設計判断](#part-2-設計判断) → 詳細は [`DECISIONS.md`](./DECISIONS.md)
4. [Part 3: 機能スコープ](#part-3-機能スコープ設計)
5. [参考リンク](#参考リンク)

---

## 経緯

出発点は「ZedでClaude Codeを使う際のUX不満」だった。追っていくと、欲しかったものは
**"今どのスレッド/どのプロジェクトにいるか、色で一目で分かること"**（＝タブUIの本質）だと判明。
しかしこれはZedの設計思想と構造的に噛み合わず、設定でも拡張でもforkでも現実的に埋まらない、
と分かった（→ Part 1）。そこから「じゃあ自分で作ったら？」に発展した。

**重要な気づき**: 欲しいのは「**VSCode並みのUI拡張性 × Zed並みの速さ**」。
だがこの2つは**構造的な綱引き**（VSCodeの拡張性=webview=重さの元、Zedの速さ=それを捨てた結果）。
「いいとこ取り」は業界の**未解決フロンティア**であって、フリーランチではない。ここが全ての難所。

---

## Part 1: Zed探索で分かったこと

### 実際にZed設定へ入れたもの（`~/.config/zed/settings.json`）
| 設定 | 効果 |
|---|---|
| `project_panel.dock: "left"` | ファイルツリー左 |
| `agent.dock: "right"` | Agent Panel右 |
| `agent.sidebar_side: "right"` | Threads Sidebar（履歴）を右へ |
| `agent.use_modifier_to_send: true` | Enter=改行 / Cmd+Enter=送信（IME誤送信対策） |
| `agent.thinking_display: "always_expanded"` | thinkingブロック常時展開 |
| `agent_servers.claude-acp.env.MAX_THINKING_TOKENS: "16000"` | **thinkingを有効化**（下記の罠） |

### 見つかった罠・限界（＝自作を考えた理由）
- **thinkingは`effort`では出ない**。ACPアダプタ(`claude-agent-acp`)は
  `MAX_THINKING_TOKENS`環境変数（or `_meta`明示）でしかSDKの`thinking`を有効化しない。
  effort/modelだけでは可視thinkingゼロ。
- **Zed拡張はUIを描けない**。実装できるのは 言語/文法・デバッガ・テーマ・アイコンテーマ・
  スニペット・MCPサーバ の6つ**だけ**（WASMサンドボックス、GPUI非公開）。
  → 色分けもタブも「拡張で追加」が**原理的に不可能**。
- **スレッド/プロジェクトごとの色機能は存在しない**（ソース検索でゼロ）。
- **テーマはプロジェクト単位で上書き不可**（`theme`はグローバル設定のみ）。
- **トークン表示**はACP外部エージェントでは "integration依存" で薄い（VSCode拡張に劣る）。
- **UI拡張の根っこ**: RFC [#53403](https://github.com/zed-industries/zed/discussions/53403)
  "Allow Extensions to Render GUI"（40👍）。メンテナ談：**extensions roadmapには居るが、
  チーム主導で慎重にやる大仕事**。＝「拒否ではないが、まだ／自分たちの手で」。
- **per-window theme**（Peacock相当）のPR [#58755](https://github.com/zed-industries/zed/pull/58755)
  は存在するが **CHANGES_REQUESTED＋コンフリクト＋停滞**。当てにできない。

→ 結論：**Zedの現行アーキテクチャでは「色で即判別」は構造的に無理**。だから自作の話へ。

---

## Part 2: 設計判断

要点のみ。詳細な根拠は [`DECISIONS.md`](./DECISIONS.md) に分離。

- **中核の緊張**: 速い×ネイティブ（Zed/GPUI） vs UI拡張が楽（VSCode/webview）。両立が難所。
- **フレームワーク**: **GPUI**（速さ重視）or **Tauri**（webview=VSCode側、拡張が楽だが編集性能は妥協）。
  → GPUIに傾倒。
- **GPUIの実態**: Apache-2.0で単体利用可だが**「Zedのために作られた」**
  （in-tree開発・安定版なし・**API churn**・docs薄い・非Zed事例少）。
  得るもの＝Zed級の速度。払うもの＝早期採用コスト。
- **代替フレームワーク**: **Floem**（Lapceが採用、より汎用・standalone向け）。
  他に iced / egui / Dioxus。
- **プラットフォーム**: GPUI = mac/linux/windows デスクトップ（**Web・モバイル無し**）。
  成熟度 mac > linux > windows。
- **ライセンス**: GPL=有効（Zedと同型）。だが義務でなく選択。
  copyleft保護(GPL) vs 採用/商用余地(Apache/MIT)。先行例：Zed=GPL / Lapce=Apache / Helix=MPL /
  VSCode=MIT / Neovim=Apache。
- **規模感**: エディタ自作は person-decades 級。Zed=一流チーム＋資金＋数年。
  先行の個人/小規模例（**Lapce**, **Helix**）は「可能」の証明だが、数年かけてなお"ニッチ"が相場。

---

## Part 3: 機能スコープ設計

**全部を仕様化しない**（着手前に死ぬ）。**MVP（歩く骨格）＋優先度付きバックログ**で進める。
既存エディタ（VSCodeのコマンド一覧 / Zedのdocs・keymap）を**実行可能な仕様書**として盗む。

### 機能の層（＝spec骨格 / バックログのカテゴリ）
1. コア編集 — buffer / cursors / selections / undo
2. 描画・UI — pane / tab / panel / theme
3. ナビゲーション — file tree / fuzzy find / go-to-symbol
4. 言語知能 — Tree-sitter / LSP
5. 検索置換 — in-file / project-wide / regex
6. VCS — git
7. Terminal
8. Debug — DAP
9. **拡張モデル ← あなたの差別化点（UI拡張を一級市民に）**
10. 設定 / keymap / config
11. （任意）Collaboration

**戦略**: VSCodeに機能数で勝とうとしない。**MVP＋"1点だけ"世界一**
（＝UI拡張性 / 色分け・方向感覚）。残りは"そこそこparity・後回し"。

---

## 参考リンク

### Zed / エージェント関連
- RFC UI拡張: https://github.com/zed-industries/zed/discussions/53403 （根っこ・40👍）
- per-window theme: https://github.com/zed-industries/zed/discussions/32293 （73👍）/ PR https://github.com/zed-industries/zed/pull/58755
- ACPトークン表示: https://github.com/zed-industries/zed/discussions/49472 （118👍）
- サイドバー整理: https://github.com/zed-industries/zed/discussions/54865
- Agent Panel配色: https://github.com/zed-industries/zed/discussions/53162
- Claude ACPアダプタ: https://github.com/agentclientprotocol/claude-agent-acp

### 自作の土台
- GPUI: Zed repository の `crates/gpui` と公開 examples。crate 自体は Apache-2.0 表示だが、
  現在の依存グラフに含まれる GPL 推移依存は [`DECISIONS.md`](./DECISIONS.md) §5 を正とする
- Floem（Lapce）: https://github.com/lapce/floem
- Tree-sitter（MIT）/ tower-lsp・lsp-types / ropey / alacritty_terminal

### 先行エディタ（比較用）
- Lapce（Rust, Floem, Apache-2.0）: https://github.com/lapce/lapce
- Helix（Rust, MPL-2.0）: https://github.com/helix-editor/helix
- Zed（Rust, GPUI, GPL）: https://github.com/zed-industries/zed
