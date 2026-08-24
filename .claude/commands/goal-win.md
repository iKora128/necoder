---
description: Windows 対応（W フェーズ）の次の受入条件を1歩実装する
---

あなたは necoder の **Windows 移植**担当エージェント。`docs/WINDOWS-PORT.md` を頼りに次の1歩を自走せよ。

引数: `$ARGUMENTS`（空なら WINDOWS-PORT の W フェーズを上から。指定があればそれを優先）

## 手順

1. **現在地の把握**（必ず全部読む）:
   - `docs/WINDOWS-PORT.md` — **一次資料**。未チェックの受入条件（上から）・設計決定（§2）・罠（§4）
   - `docs/JOURNAL.md` — 直近エントリの「学び/罠」「次」
   - `git status` / 直近コミット
2. **タスク選定**: 未完フェーズの未チェック項目のうち依存順で最初の1つ。**W0 → W6 の順序は依存関係**（paths を入れる前にコンパイルを通そうとしない）。着手前に選定理由を1行報告
3. **実装**: 以下が正。乖離したら実装でなく文書を疑い、文書側の修正を先に提案:
   - プラットフォーム差分: `docs/WINDOWS-PORT.md` §2 の設計決定（D1〜D8）
   - それ以外（設計・UI・用語・規約）: 従来どおり `ARCHITECTURE.md` / `UI-SPEC.md` / `GLOSSARY.md` / `CLAUDE.md`
4. **検証**:
   - **mac 非退行が先**: `cargo check -p necoder` → `cargo test --workspace`（この環境で回せる分）
   - Windows 実機の確認が要る項目は、**やったこと・確認手順・期待結果を報告に明記**してユーザーへ手番を渡す（実機が無い環境で「動いた」と書かない）
   - UI 変更は `--features screenshot` + `NECODER_SCREENSHOT` の PNG を Read して目視
5. **記録**:
   - `docs/WINDOWS-PORT.md` の該当チェックボックスを更新（**実際に検証できた分だけ**）
   - `docs/JOURNAL.md` 末尾に1エントリ追記（やったこと/学び・罠/次）
6. **報告**: 変更ファイル一覧・受入条件の充足状況・**ユーザーが Windows 実機でやるべきこと**・次の1歩

## この作業に固有の禁止事項

- **mac の挙動を変えること**（パス文字列・キーバインド・既定値）。mac 互換は unit test で固定してから進む
- `#[cfg(target_os = ...)]` を view 層・業務ロジックに撒くこと。分岐は foundation（paths / transport / command ラッパ）に閉じる
- Windows 実機で未確認の項目にチェックを入れること
- UI 文字列のハードコード（`t!` + ja/en 両方・i18n parity テスト）
- `zed/` 配下の変更・`git pull`・gpui rev の更新
- `FEATURES.md` のタグ変更（ユーザー管理）
- WINDOWS-PORT の「やらないこと」に手を出すこと（Remote SSH の Windows 版・MSIX・Mica 等）
