# Orca との差分 — 判定の証拠（2026-09-25）

[`orca-gap-2026-09.md`](./orca-gap-2026-09.md) の全 226 項目について、判定の根拠をここに残す。
各項目には、Orca 側で何を根拠にしたか、necoder 側のどのコードを見たか、「無」と判定した項目はどの検索で探したか、を書く。
後から誰でも同じ手順で確かめ直せることを目的にしている。

## 0. 前提と再現方法

### 比べた版

- **Orca**: `stablyai/orca@646e9a5b02514795af5139961ccca225dfa01b12`
  - 2026-09-25 12:38 UTC の main。コミットは "Update README downloads badge"。
  - `package.json` の version は 1.4.197 だが、同日の最新リリースは v1.4.211 だった。
- **necoder**: `97e866648281874e53f9001c9b612a6415f0ee8d`（main）に、未コミットの作業ツリー（61 エントリ）を載せた状態。
  - 行番号はすべてこの作業ツリーの 2026-09-25 時点のもの。
  - 調査を始めてからこの文書を書くまでの間に変わったファイルが無いことは、`find -newer` で確かめた。

### 書き方

- `od:<パス>` は Orca の公式ドキュメント `docs/site/content/docs/<パス>` のこと。
  - permalink は `https://github.com/stablyai/orca/blob/646e9a5b02514795af5139961ccca225dfa01b12/docs/site/content/docs/<パス>`。
- `os:<パス>` は Orca リポジトリのルートからのパス。permalink は `…/blob/646e9a5…/<パス>`。
- `PR #n` は `https://github.com/stablyai/orca/pull/n`。ドキュメントに無く、リリースノートにだけ載っている機能の根拠に使う。
- 「」内の英語は Orca のドキュメントからの引用（一部を抜粋）。
- necoder のパスはリポジトリのルートからの相対で、`path:行` または `path:開始-終了` と書く。
- **判定**: 有 / 一部 / 無 / 方式差 の 4 値。
  - 有: 同等以上がある。
  - 一部: 骨格はあるが一部が欠ける。
  - 無: コードに無い。ROADMAP に「予定」とあるだけのものも無とする。FEATURES の never で意図的に採らないものも無とし、備考に never と書く。
  - 方式差: 別のやり方で目的を果たしている。

### 再現手順

1. **Orca を固定版で取る**:
   ```sh
   git clone --filter=blob:none https://github.com/stablyai/orca.git orca
   git -C orca checkout 646e9a5b02514795af5139961ccca225dfa01b12
   ```
2. **リリースノート**: 安定版 74 本（v1.4.128 = 2026-07-08 〜 v1.4.211 = 2026-09-25）を取った。
   ```sh
   gh api 'repos/stablyai/orca/releases?per_page=100&page=1'   # page=2 も
   ```
   - タグが `v` で始まり `rc` を含まないものだけを残した。
   - `^\* feat[^ ]*: ` の行は 274 本（重複を除くと 269 件）。
3. **「無」の検索**: 項目ごとの正規表現を、下のコマンドで流す。正規表現と件数は §11 の表に全部載せてある。
   ```sh
   rg -i -n -e '<正規表現>' crates locales relay/src relay/public relay/host -g '!**/node_modules/**'
   ```
   - `orca/`・`zed/`・`codex/`・`herdr/` は参照用のクローンなので、範囲に入れない。
4. **判定の流れ**:
   1. 8 領域を、読み取り専用の調査エージェント 8 本に並列で判定させた。
   2. 主要な判定と脇道の発見（§12）は、書き手がコードを開いて確かめ直した。
   3. 「無」100 項目はすべて検索を流し直し、ヒットが出たものは中身を確かめた（§11）。
   4. necoder 側の引用は、ファイルの存在と行の範囲を機械検査した（§11 末尾）。

## 1. Orca 側の一次資料

- **公式ドキュメント 55 ページ（全部読んだ）**:
  - 概要: `index` / `first-session` / `ways-to-run` / `install`
  - モデル: `model/{agents-sessions, quick-open, session-restore, tabs-panes-splits, worktrees}`
  - エージェント: `agents/{claude-code, codex-hot-swap, codex, cursor-cli, glm-agent, hibernation, hooks-memory, native-chat, session-history, supported, usage-tracking}`
  - ブラウザ: `browser/{design-mode, overview, profiles}`
  - CLI: `cli/{automations, computer-use, orchestration, overview, reference, skills, worktree-checkpoints}`
  - 編集: `editing/{file-explorer, markdown, monaco, viewers}`
  - レビュー: `review/{annotate-ai-diff, attribution, commit-push, diff-viewer, github, jira, linear}`
  - レシピ: `recipes/{design-mode-fix, jump-worktrees, parallel-agents, remote-worktrees, review-ai-diff}`
  - その他: `activity` / `notifications` / `terminal` / `ssh` / `remote-servers` / `mobile` / `android-apk` / `settings` / `telemetry` / `troubleshooting` / `github-errors`
- **README**: 機能の見出しと、対応エージェントの一覧。README 自身が「the changelog is the real feature list」と書いているので、リリースノートも一次資料として扱った。
- **ドキュメントに無く、ソースで確かめた機能**:

  | 機能 | ソース |
  |---|---|
  | ペット | `os:src/renderer/src/components/pet/` |
  | Stats | `os:src/renderer/src/components/stats/` |
  | Ports | `os:src/main/ports/` |
  | トレイ | `os:src/main/tray/` |
  | Dock バッジ | `os:src/main/dock/unread-badge.ts` |
  | sparse checkout | `os:src/renderer/src/components/sparse/` |
  | オンボーディング | `os:src/renderer/src/components/{setup-guide, contextual-tours, feature-tips}/` |
  | ファイル操作の undo | `os:src/renderer/src/components/right-sidebar/fileExplorerUndoRedo.ts` |
  | PR コメントの AI 修正 | `os:src/renderer/src/components/right-sidebar/pr-comment*.ts` |
  | Design Mode の実装 | `os:src/main/browser/grab-guest-*.ts`、`os:src/shared/browser-grab-types.ts` |

---

## 2. 領域 A — ワークツリー・ワークスペース・サイドバー

### A01 リポジトリ追加と repo ごとの base ref — 一部
- Orca:
  - od:first-session.mdx「Click **Add Repo** … picks up your default branch as its **base ref**」「You can change the base ref later under the repo's settings.」
  - od:cli/reference.mdx `orca repo set-base-ref --repo id:<repoId> --ref origin/main`
- necoder（有る部分）: レールの ＋ から統一オープン（フォルダ / 最近 / SSH）`crates/workspace/src/workspace/rail_view.rs:288-312`
- necoder（無い部分）:
  - base は統合先 worktree の HEAD に固定（`git worktree add -b <branch> <path> HEAD`）`crates/project/src/project.rs:767-788`
  - 作成前に統合先を upstream へ早送りするだけ `crates/project/src/project.rs:692-748`
  - repo ごとの base ref 設定は `crates/settings_core/src/settings_core.rs` に無い

### A02 作成ダイアログ（空なら自動命名・start-from） — 一部
- Orca:
  - od:first-session.mdx「if you leave it blank, Orca names it after a marine creature」
  - od:model/worktrees.mdx §Start-from picker「Another local branch … A specific commit SHA. An existing remote branch — Orca will fetch and check it out.」
  - PR #17250（New Workspace で base ref を選ぶ）
- necoder:
  - 入力欄は 1 つで、1 行目から `⎇ task/<slug>` を表示する `crates/workspace/src/workspace/new_task_dialog.rs:64-72,101-112`
  - slug 化と衝突時の -2 / -3 `crates/project/src/project.rs:3094-3101,3139-3150`
  - ⎇ メニューの ⧉ で既存ローカルブランチを worktree にできる `crates/workspace/src/workspace/git_controller.rs:134-169`
  - 空入力は送信できない。起点ブランチ・SHA・リモートブランチは選べない

### A03 背景作成（進捗行・キャンセル・Retry） — 一部
- Orca: od:model/worktrees.mdx §Creation runs in the background「closes it immediately … appears in the sidebar with a progress row … cancel it from the in-tab panel … the panel surfaces the error with a Retry.」
- necoder:
  - 送信で即閉じる `crates/workspace/src/workspace/new_task_dialog.rs:69-70`
  - 早送り同期・worktree add・準備スクリプトは背景で実行 `crates/workspace/src/workspace/fleet_view.rs:506-518`
  - 準備が失敗したら phase=failed を台帳へ、作成に失敗したらトースト `crates/workspace/src/workspace/fleet_view.rs:577-584,592-595`
  - 進捗行・キャンセル・Retry は無い

### A04 ブランチ名の指定・課題から命名 — 無
- Orca: od:model/worktrees.mdx §Naming the branch「expand the **Advanced** drawer … **Branch name** field」「When you create from a **Linear issue**, Orca uses Linear's own branch name」
- necoder: ブランチは常に `task/<slug>` `crates/project/src/project.rs:3139-3150`
- 検索: `\blinear\.app|\bjira\b|atlassian|gitlab|glab\b|gh issue|issue_url|branch_name_override` → 3 件。すべて無関係（バグ報告の URL `crates/workspace/src/crash.rs:14,157` と、gitlab の URL を弾くテスト `crates/project/src/project.rs:3063`）

### A05 共有パス・`.worktreeinclude`・`sharedDirectories` — 一部
- Orca: od:model/worktrees.mdx §Shared directories & gitignored files（Worktree Shared Paths / `orca.yaml` の `worktree.sharedDirectories` / `.worktreeinclude`）。PR #10459
- necoder（有る部分）:
  - 準備スクリプトの雛形が `.env` / `.env.local` を main からコピーし、`CARGO_TARGET_DIR` を main の target へ向ける `crates/project/src/project.rs:3118-3136`
  - `.necoder/task.env` を ACP 起動時の環境へ入れる `crates/host/src/host.rs:5085-5101`、`crates/acp_client/src/acp_client.rs:1500`
- necoder（無い部分）: 宣言的な共有パスの一覧、APFS clone や symlink の自動化、`.worktreeinclude` / `orca.yaml` 相当

### A06 作成後の setup hook — 一部
- Orca: od:agents/hooks-memory.mdx「Configure commands to run automatically after a worktree is created」。od:cli/reference.mdx `--setup run|skip|inherit`
- necoder:
  - `.necoder/worktree-setup.sh` を新しい worktree で 1 回実行する（NECODER_MAIN_ROOT / TASK_ROOT / TASK_BRANCH を渡す）`crates/project/src/project.rs:3112-3114,3157-3170`
  - スクリプトの有無を表示し、「作る」で雛形を生成 `crates/workspace/src/workspace/new_task_dialog.rs:110-119`
  - CLI も同じ経路 `crates/necoder/src/fleet.rs:176-182`
  - 実行ごとの run / skip の切替と、設定 UI は無い

### A07 親子 worktree（ネスト・子孫ごと Sleep / Delete） — 無
- Orca: od:model/worktrees.mdx「**Parent workspace** … only nests the workspaces in Orca's sidebar」「**Sleep with Descendants (N)** and **Delete with Descendants…**」
- necoder: depends_on は待ち合わせ専用で、GUI には出ない `crates/storage/src/storage.rs:112-114`、`crates/necoder/src/fleet.rs:421-468`
- 検索: `parent_task|parent_worktree|parent_workspace|descendant|子 ?Task|親 ?Task` → 5 件。すべて無関係（Rust の可視性についてのコメント `crates/workspace/src/workspace.rs:48`、tree-sitter の `descendant_of_kind` `crates/lang/src/lang.rs:991-1057`）

### A08 絵文字の名前（:rocket:） — 無
- Orca: od:model/worktrees.mdx §Emoji workspace names
- necoder: slug 化で非 ASCII の文字は潰れる `crates/project/src/project.rs:3094-3101`
- 検索: `shortcode|:rocket:|emoji_short` → 0 件

### A09 ピン留め — 一部
- Orca: od:model/worktrees.mdx「You can pin a worktree to the top of its project」
- necoder:
  - 行の ◫ `crates/workspace/src/workspace/fleet_sidebar.rs:542-547`
  - 最大 3 本を舞台の列に固定 `crates/workspace/src/workspace/fleet_stage.rs:63-72`
  - `stage_pinned` はメモリにしか持たない `crates/workspace/src/workspace.rs:1151`
  - サイドバー上部への固定ではなく、再起動で消える

### A10 Sleep / Archive / Delete — 一部
- Orca: od:model/worktrees.mdx「right-clicking a worktree exposes archive / sleep / delete actions」。od:first-session.mdx「can be deleted with one click; their branches go with them」
- necoder:
  - Task を終了すると archived になる `crates/workspace/src/workspace/fleet_view.rs:703-730`
  - 確認つきで worktree を削除し、ブランチごと消す時は `-D` `crates/workspace/src/workspace/explorer_controller.rs:767-867`
  - 使っていないエージェントを既定 15 分で止める `crates/agent_panel/src/idle.rs:46-80`、`crates/settings_core/src/settings_core.rs:252-257`
  - worktree 単位で手動で休眠させる操作は無い

### A11 複数選択の一括操作 — 無
- Orca: od:model/worktrees.mdx「Hold `Cmd` … multi-selection, or hold `Shift` … right-clicking any selected worktree applies the action to every worktree in the selection.」
- necoder: 行のクリックは 1 件の切り替えだけ `crates/workspace/src/workspace/fleet_sidebar.rs:583-598`
- 検索: `multi_select|multiselect|selected_tasks|bulk_|一括|複数選択` → 39 件。すべて無関係。ACP の質問カード（elicitation）の「複数選択」`crates/acp_client/src/acp_client.rs:137-146,2247-2315`、`relay/public/app.mjs:250-251`、「一括」を含む別文脈

### A12 インライン改名・詳細ダイアログ — 一部
- Orca: od:model/worktrees.mdx「Double-click a worktree title in the sidebar to rename it inline … **Edit Worktree Details**」
- necoder:
  - 行をダブルクリックで改名 `crates/workspace/src/workspace/fleet_sidebar.rs:583-592`
  - 台帳へ保存 `crates/workspace/src/workspace/herd_view.rs:4-56`
  - 舞台の見出しでも改名できる `crates/workspace/src/workspace/fleet_stage.rs:493-499`
  - 詳細ダイアログ（課題リンクの編集など）は無い

### A13 サイドバーのフィルタ — 一部
- Orca: od:model/worktrees.mdx §Sidebar layout（Sleeping / Except default branch / Default branch / Automation-created / CLI-created / Other-client / Detached HEAD）。PR #10786
- necoder:
  - レールで選んだ 1 リポジトリに自動で絞り、アーカイブ済みは常に隠す `crates/workspace/src/workspace/fleet_sidebar.rs:199-230`
  - `N Tasks` の件数表示 `locales/ja.yml:326`
  - それ以外の切替・フィルタ件数・Clear は無い

### A14 グループ・並べ替え・サイドバー内検索 — 一部
- Orca: od:model/worktrees.mdx「Repo rows themselves can be reordered by drag.」「The header has its own filter input」。PR #13864（jump-to-top）
- necoder（有る部分）:
  - Editor モードは全プロジェクトのスレッドをプロジェクト別に出す `crates/workspace/src/workspace/herd_view.rs:84-130`
  - ⌘O で project / worktree の 2 階層検索 `crates/workspace/src/workspace/chrome.rs:502-521`、`crates/workspace/src/workspace/overlays.rs:95-235`
  - レールのドラッグは窓の外へ出して新窓にするだけ `crates/workspace/src/workspace/rail.rs:10-78`
- necoder（無い部分）: repo の並べ替え・サイドバー内のフィルタ入力・jump-to-top。未読は太字ではなく、状態ドットの「完了未確認」で示す

### A15 外部 worktree の検出と表示切替 — 一部
- Orca: od:model/worktrees.mdx §Using plain git「**hidden worktrees** card … **Non-Orca worktrees**」。PR #14189（出所ごとの表示切替）
- necoder:
  - ⌘O が各 repo の `git worktree list` を全部出す `crates/workspace/src/workspace/overlays.rs:113-165,169-235`
  - ⎇ メニューの worktree 欄 `crates/workspace/src/workspace/git_view.rs:835-895`
  - ブランチ付きで開いた worktree は Task として取り込む `crates/workspace/src/workspace.rs:958-988`
  - 出所別（Claude Code / GSD 等）の表示既定は無い

### A16 片付け画面（Resource Manager → Clean up workspaces） — 無
- Orca: od:model/worktrees.mdx §Resource Manager cleanup「status, recent activity, size, Git state, and linked review」
- necoder: 近いのは ⌘O の ↑↓ と dirty の表示だけ `crates/workspace/src/workspace/overlays.rs:200-230`
- 検索: `resource.?manager|cleanup_workspaces|disk_usage|dir_size|du -s|ディスク使用` → 0 件

### A17 残ったブランチのレビュー（Preserved branches） — 方式差
- Orca: od:model/worktrees.mdx §Preserved branches「**Review N Branches**」
- necoder:
  - 削除前に、未コミット件数と「統合先にまだ入っていないコミット数」を git で数えて見せる `crates/workspace/src/workspace/worktree_delete.rs:28-42,104-133`
  - 「ブランチごと削除」は `branch -D` `crates/workspace/src/workspace/explorer_controller.rs:823-827`
  - 一括削除が無いので、残ったブランチを後から確かめる場面が生じない
  - 例外として、⎇ メニューのブランチ単体削除は `-d`（`crates/workspace/src/workspace/git_controller.rs:655`）なので未マージは拒否され、UI から強制削除する手段は無い

### A18 マルチリポの project group・folder workspace — 無
- Orca: od:model/worktrees.mdx §Multi-repo project groups & folder workspaces
- 検索: `project.?group|multi.?repo|folder.?workspace|複数リポ` → 0 件

### A19 repo アイコン・バッジ色 — 一部
- Orca: od:settings.mdx §Repository「Repo icons for the sidebar: choose an icon, full searchable emoji picker, uploaded image, website favicon, or GitHub avatar, then pick a preset or custom hex badge color.」
- necoder:
  - プリセット色と任意 hex `crates/workspace/src/workspace/rail.rs:216-327,412-452`
  - `.necoder/settings.json` と DB へ保存 `crates/workspace/src/workspace/rail.rs:92-106`
  - `icon` は絵文字か画像パスで、`.necoder/icon.{png,jpg,jpeg,webp}` を自動で見つける `crates/workspace/src/workspace.rs:765-797`
  - アイコンのピッカー・アップロード・favicon・GitHub アバターは無い

### A20 worktree カードに課題を表示 — 無（never: ホスティング連携）
- Orca: od:model/worktrees.mdx「Link a GitHub PR, Linear issue, GitLab MR, or **Jira** issue from the name field … Linked issues appear on the worktree card.」
- necoder: FEATURES.md の §6「never: … ホスティング連携」`FEATURES.md:80`
- 検索: `gh issue|issue_number|issue_url|\bjira\b|linear\.app` → 2 件。すべて無関係（バグ報告の URL `crates/workspace/src/crash.rs:14,157`）

### A21 進捗コメントとカード状態をエージェントが更新 — 有
- Orca: od:cli/worktree-checkpoints.mdx `orca worktree set --worktree active --comment "…" --workspace-status in-progress`
- necoder:
  - `necoder fleet status <id> <phase> [summary]` `crates/necoder/src/fleet.rs:536-542`
  - MCP の `fleet_update_task` `crates/necoder/src/mcp.rs:176-183`
  - summary はサイドバー行の 3 段目に出る `crates/workspace/src/workspace/fleet_sidebar.rs:246-250,504-514`
  - ターン開始・終了・承認待ち・失敗で状態が自動で遷移する `crates/workspace/src/workspace/notifications.rs:38-46,92-114`
- 注意: summary は次のターン終了時に要約で上書きされる。残り続けるメモ欄ではない

### A22 状態別ボード（Workspace Board） — 無
- Orca: od:settings.mdx §Shortcuts「**Toggle Workspace Board**」。od:cli/worktree-checkpoints.mdx の statuses `todo` / `in-progress` / `in-review` / `completed`
- necoder:
  - 旧管制の統合パイプライン（phase の列表示）は F3 で削除した記録がある `crates/workspace/src/workspace/control_view.rs:3-4`
  - 今あるのは要対応キューと編隊図 `crates/workspace/src/workspace/fleet_view.rs:1256-1260`
- 検索: `kanban|カンバン` → 0 件

### A23 自動命名のブランチを作業内容から改名 — 無
- Orca: od:settings.mdx §Git「Auto-Rename Branch From Work」
- necoder: Task の改名は表示名だけで、ブランチ `task/<slug>` は変わらない `crates/workspace/src/workspace/herd_view.rs:4-6`
- 検索: `rename_branch|RenameBranch|"branch", "-m"|branch -m` → 0 件

### A24 sparse checkout プリセット — 無
- Orca: `os:src/renderer/src/components/sparse/SparseCheckoutPresetSelect.tsx`、`SparseCheckoutPresetDraftForm.tsx`
- necoder: worktree は常に全体をチェックアウトする `crates/project/src/project.rs:767-788`
- 検索: `sparse.?checkout|sparse-checkout|--sparse` → 0 件

### A25 外で消された worktree の自動掃除 — 一部
- Orca: od:model/worktrees.mdx「If you `git worktree remove` from the CLI, Orca will notice and clean up its own state」
- necoder:
  - `.git` の index / HEAD / refs の変化で git 状態を取り直す `crates/workspace/src/workspace/project_watcher.rs:139-155`
  - linked worktree かどうかとブランチを毎回 git に問い直す `crates/workspace/src/workspace/project_session.rs:1466-1514`
  - 再起動時の復元は、存在を確かめずに開く `crates/workspace/src/workspace/project_session.rs:546-547`

### A26 削除ショートカット・名前のコピー — 無
- Orca: od:model/worktrees.mdx「press `Cmd-Shift-Backspace`」。PR #22338（copy workspace name）
- necoder:
  - ホバーで出る 🗑 から、確認つきで削除できる `crates/workspace/src/workspace/fleet_sidebar.rs:548-582`
  - キーの割当は無い `crates/keymap_core/src/keymap_core.rs:265-373`、`crates/workspace/src/workspace/commands.rs:14-235`
- 検索: `shift-backspace|copy_task_name|copy_branch_name|CopyBranch|copy_name` → 2 件。すべて無関係（エディタ用の `ctrl-shift-backspace` の対応付け `crates/keymap_core/src/keymap_core.rs:447,668`）

### A27 同じ依頼を N 体へ fan-out して比べる — 一部
- Orca: README「Fan one prompt across five agents, each in its own isolated git worktree — compare the results and merge the winner.」。od:recipes/parallel-agents.mdx
- necoder:
  - ＋Task を繰り返すと `task/<slug>-2` / `-3` が並ぶ `crates/project/src/project.rs:3143-3150`
  - 舞台 1〜3 列 `crates/workspace/src/workspace/fleet_stage.rs:45-61`、`crates/keymap_core/src/keymap_core.rs:345-347`
  - 承認した分解案を、行ごとの担当エージェントつきで一括作成する `crates/workspace/src/workspace/captain_proposals.rs:341-371`
  - GUI の ＋Task は担当エージェントを選べない（`create_task_in(…, None, None, …)`）`crates/workspace/src/workspace/fleet_view.rs:484`

### A28 Run on（local / SSH / サーバ / VM）の選択 — 一部
- Orca: od:model/worktrees.mdx §Create dialog「**Run on** lists ready hosts and recipes」。od:ways-to-run.mdx
- necoder:
  - Task の worktree は統合先と同じ host（ローカルか SSH 先）に作る `crates/workspace/src/workspace/fleet_view.rs:502-515`
  - 準備スクリプトは、リモートでも POSIX shell があれば動く `crates/project/src/project.rs:3157-3160`
  - 作成時に実行先は選べない。サーバ / VM は無い

---

## 3. 領域 B — エージェント・セッション・状態・使用量・チャット

### B01 対応エージェントの数 — 一部
- Orca: od:agents/supported.mdx「Orca works with **any CLI agent**」。表には Claude Code・Codex・Grok・Copilot・OpenCode・Pi・OMP・Prime Agent・Gemini・Antigravity・Ante・Aider・Goose・Amp・Kilocode・Kiro・Crush・Auggie・Autohand・Cline・Codebuff・Command Code・Muse・ZCode・Continue・Cursor・Devin・Droid・Kimi・Mistral Vibe・MiniMax・Qwen・Rovo Dev・Hermes・OpenClaw・Trae ほかが並ぶ。PR #22216（Muse Code）、#12935（Prime Agent）、#10763（Trae）
- necoder:
  - ACP の 7 種（`claude` / `codex` / `copilot` / `qwen` / `opencode` / `kimi` / `grok`）`crates/acp_client/src/acp_client.rs:386-477`
  - 公開 ACP レジストリ（39 件）は版を決めるのに使うだけ。`agent_servers` は既存 7 種の起動を上書きするだけで、8 本目は足せない `crates/settings_core/src/settings_core.rs:108-125`
  - Gemini は、後継の Antigravity CLI が ACP に対応していないため外している（意図した判断・コメントあり）`crates/acp_client/src/acp_client.rs:413-415`

### B02 検出・自動導入・有効 / 無効 — 一部
- Orca: od:agents/supported.mdx「one-click launch/setup」。od:settings.mdx §Agents「Installed agents — detected CLIs you can enable or disable.」
- necoder:
  - PATH 上の CLI を検出 `crates/acp_client/src/acp_client.rs:805-808`
  - 「導入」「ログイン」ボタン `crates/settings/src/settings.rs:470-500,1199-1225`
  - ACP アダプタの npm 自動導入 `crates/acp_client/src/install.rs:27-66`
  - エージェントの有効 / 無効の切替は無い。未ログインのものを候補から外すだけ `crates/acp_client/src/acp_client.rs:517-526`

### B03 状態の検出と表示 — 有
- Orca: od:model/agents-sessions.mdx §State indicators
- necoder:
  - Idle / Working / Blocked / Done を、色ではなく形と動きで示す `crates/agent_panel/src/agent_panel.rs:785-811,1031-1043`
  - 同じ表示をタブ・レール・herd・statusbar で共用 `crates/workspace/src/workspace/chrome.rs:1896-1902`、`crates/workspace/src/workspace/rail_view.rs:267`、`crates/workspace/src/workspace/herd_view.rs:370`
  - 状態は ACP の構造化イベントから作るので、画面を読み取る推定より確か

### B04 Agent Dashboard（カンバン・検索・フィルタ・pop-out） — 一部
- Orca: od:model/agents-sessions.mdx §Agent dashboard（Needs You / Working / Done / Idle・検索・Project / status / PR のフィルタ・pop-out）。PR #10243
- necoder:
  - 全プロジェクト横断の状態一覧 `crates/workspace/src/workspace/herd_view.rs:89-521`
  - 要対応キュー `crates/workspace/src/workspace/control_view.rs:221-340`
  - titlebar の要対応件数 `crates/workspace/src/workspace/chrome.rs:307`
  - カンバンの列・検索・フィルタ・pop-out は無い

### B05 Agent map — 有
- Orca: PR #12168 で実験的に追加したが、PR #15853（Remove agent map view from dashboard）で外した
- necoder: 編隊図のハブ / 扇形 / ツリー / カードの 4 表示 `crates/workspace/src/workspace/fleet_view.rs:1219-1263`（⌘⇧G `crates/keymap_core/src/keymap_core.rs:348`）

### B06 Restart chip — 有
- Orca: od:model/agents-sessions.mdx §Restart chip
- necoder: 終了や切断の後に「再開」チップを出し、session/load で会話を引き継ぐ `crates/agent_panel/src/agent_panel.rs:3682-3748,9028-9082`

### B07 権限の既定（Yolo / Manual 一括）・起動引数の上書き — 一部
- Orca: od:model/agents-sessions.mdx §Launch defaults。od:agents/supported.mdx §Permissions default（「**Yolo** and **Manual**」・引数と env の上書き・Reset）
- necoder:
  - 権限モードはスレッドのピルで選び、エージェントごとに次回へ持ち越す `crates/agent_panel/src/agent_panel.rs:4870-4897`
  - 起動引数と env の上書きは settings.json の `agent_servers` だけ `crates/settings_core/src/settings_core.rs:108-125`
  - 一括切替・GUI での編集・Reset は無い

### B08 hibernation — 有
- Orca: od:agents/hibernation.mdx（「off by default」・既定 30 分・worktree を開き直すと resume）
- necoder:
  - 静かで loadSession に対応したスレッドを N 分後に止める（既定 15 分・0 で無効）`crates/agent_panel/src/idle.rs:46-80`、`crates/settings_core/src/settings_core.rs:252-257`
  - 設定画面から変えられる `crates/settings/src/settings.rs:1745-1748`
  - 再開は次の送信時に session/load で行う

### B09 CLI 自身のセッション履歴を走査して再開 — 一部
- Orca: od:agents/session-history.mdx（`~/.codex/sessions`・`~/.claude` などを走査。Resume / Copy resume command / Copy session ID / Open log / Open cwd）
- necoder:
  - ⌘⇧H の履歴は necoder の DB に残したスレッドだけ `crates/workspace/src/workspace/overlays.rs:1311-1383`、`crates/agent_panel/src/agent_panel.rs:4080-4110`
- 検索: `\.claude/projects|codex/sessions|\.jsonl|--resume` → CLI の保存セッションを読むコードは無い

### B10 全エージェント横断の全文検索 — 一部
- Orca: PR #20029（query engine）、#20514（`orca search`）、#20885（全マシンの横断検索）
- necoder:
  - 会話内の ⌘F `crates/agent_panel/src/search.rs:1-4`
  - Chat モードの会話を横断する本文検索（LIKE）`crates/agent_panel/src/chat.rs:152-167`、`crates/storage/src/storage.rs:1283-1327`
  - Editor / Fleet のスレッド横断と、他 CLI の transcript は対象外

### B11 Continue in New Session（handoff） — 一部
- Orca: od:agents/codex.mdx §Continue in a new session
- necoder:
  - Captain 席だけが、文脈 40% か起床 20 回で新しい会話へ自動で交代する `crates/agent_panel/src/seat.rs:188-229`、`crates/workspace/src/workspace/captain.rs:153-156`
  - 一般のスレッドにはこの操作が無く、会話を始めた後はエージェントも変えられない `locales/ja.yml:42`

### B12 サブエージェントの子行 — 無
- Orca: od:agents/claude-code.mdx §Subagents and teams。od:agents/codex.mdx §Nested Task subagents。PR #14627
- 検索: `subagent|sub_agent|parent_tool|parentToolUse|サブエージェント|子エージェント` → 0 件

### B13 Claude Agent Teams — 無
- Orca: od:agents/supported.mdx「Claude Agent Teams … launch via `orca claude-teams` with native panes for each teammate」
- 検索: `teammate|agent.?team` → 0 件

### B14 アカウントの hot-swap — 無
- Orca: od:agents/codex-hot-swap.mdx（Claude も同じ）
- necoder: `agent_servers.<id>.env` で環境変数を足す口はある `crates/settings_core/src/settings_core.rs:120-124`
- 検索: `account_switch|switch_account|hot.?swap|CODEX_HOME|CLAUDE_CONFIG_DIR|アカウント切替` → 2 件。すべて無関係（MCP の発見のために `CODEX_HOME` を読むだけ `crates/acp_client/src/mcp.rs:368-372`）

### B15 使用量・レート制限（5 時間 / 週・reset・80% 警告） — 無
- Orca: od:agents/usage-tracking.mdx
- necoder:
  - トークンメーターは ACP の UsageUpdate の文脈窓 `crates/acp_client/src/acp_client.rs:174-175`
  - Σ チップは開いているスレッドの合計 `crates/agent_panel/src/agent_panel.rs:6621-6639`
  - どちらもプランの使用枠とは別物
- 検索: `rate.?limit|five_hour|seven_day|resets_at|utilization|レート制限|使用率` → 7 件。すべて無関係（relay の乱用対策のレート制限 `relay/src/worker.ts:3-7,63,133` と、認証エラーの判定テスト `crates/acp_client/src/acp_client.rs:2434`）

### B16 Stats（日別チャート・推定コスト・シェア画像） — 無
- Orca: od:agents/usage-tracking.mdx §Estimated cost (Stats)。`os:src/renderer/src/components/stats/ClaudeUsageDailyChart.tsx`、`ShareUsageCard.tsx`
- necoder: `Storage::token_ledger()` はテストからしか呼ばれていない `crates/storage/src/storage.rs:1637`
- 検索: `pricing|cost_usd|estimated_cost|推定コスト|日別` → 0 件

### B17 Keep computer awake — 一部
- Orca: od:settings.mdx §Agents「**Keep computer awake** — **On** … **Agent** … **Off** … **Caffeinate**」
- necoder: スマホ連携ホストが、ペアリング済みかつ AC 電源の間だけ `caffeinate -i -s` を掛ける `relay/host/bridge.mjs:307-316`。GUI の切替は無い

### B18 composer（添付・`/` のコマンドと skills・ピル） — 一部
- Orca: od:agents/native-chat.mdx §Composer「Type `/` for slash commands and discovered **skills**」。PR #20506（Fast mode）
- necoder:
  - ファイル（@ / D&D）と画像の添付、agent / mode / model / effort のピル `crates/agent_panel/src/agent_panel.rs:9545-9548,4785-4795`、`crates/acp_client/src/acp_client.rs:1554,2079`
  - `/` で始まる入力はそのまま送る `crates/agent_panel/src/agent_panel.rs:5325-5331`
  - 補完は無い。ACP の `AvailableCommandsUpdate` を `_ => {}` で捨てている `crates/acp_client/src/acp_client.rs:1972`（§12-8）

### B19 構造化された質問カード — 有
- Orca: od:agents/native-chat.mdx §Questions from the agent
- necoder: ACP の elicitation（Claude の AskUserQuestion を含む）をカードで描き、単一選択と複数選択に答えられる `crates/acp_client/src/acp_client.rs:1222-1225,2241-2256`、`crates/agent_panel/src/agent_panel.rs:8885-8899`。自由入力の Other 欄には未対応

### B20 message rail — 一部
- Orca: PR #20719
- necoder: 「前の指示へ」・直近の問いの固定帯・「最新へ」`crates/agent_panel/src/agent_panel.rs:7543-7546,7129-7139,7473`。全プロンプトを目盛りのように並べる rail は無い

### B21 rewind・/clear・/compact — 一部
- Orca: PR #19235（structured session rewind）、#19164（/clear と /compact）
- necoder:
  - checkpoint はファイルだけを書き戻し、会話は巻き戻さない `crates/agent_panel/src/agent_panel.rs:6537-6580`
  - /compact は専用ボタンがある `crates/agent_panel/src/agent_panel.rs:5244-5247`

### B22 inline diff・plan・ツールのバッチ・BG タスクの停止 — 一部
- Orca: PR #18765（inline diff）、#21090（plan）、#19372（バッチ）、#18806 / #18773（サブエージェントの活動）、#18807（個別停止）
- necoder: ステップ内のインライン diff と diff タブ、plan の常設チェックリストはある `crates/agent_panel/src/agent_panel.rs:8156-8158,10663,7411`。バッチ・サブエージェント・個別停止は無い

### B23 履歴の行から構造化チャットで再開 — 一部
- Orca: PR #19176
- necoder: necoder のスレッドは session/load で再開できる `crates/acp_client/src/acp_client.rs:1526-1564`。necoder の外で始めた CLI のセッションは取り込めない

### B24 Chat ⇄ TUI の切替 — 方式差
- Orca: od:agents/native-chat.mdx「Toggle Chat UI ↔ terminal from the agent pane」
- necoder: PTY を持たず、ACP の構造化イベントだけで描く `crates/acp_client/src/acp_client.rs:1863-1973`。TUI 専用の操作は使えない

### B25 端末で直接起動したエージェントの状態検出・hooks 管理 — 無
- Orca: od:model/agents-sessions.mdx「State is detected from the terminal's OSC title sequence and agent hooks」。od:agents/hooks-memory.mdx §Agent status hooks（`orca agent hooks status|on|off`）
- necoder:
  - 端末は Wakeup / Exit / PtyWrite 以外のイベントを `_ => {}` で捨てる `crates/terminal_view/src/terminal_view.rs:352-363`
  - ROADMAP M14 の未チェック項目 `docs/ROADMAP.md:201`
- 検索: `Event::Title|TitleChanged|ResetTitle|statusline|UserPromptSubmit` → 0 件

### B26 `.claude` / `.codex` のフックと CLAUDE.md を尊重 — 有
- Orca: od:agents/hooks-memory.mdx §Per-repo hooks, §Memory files
- necoder:
  - 通常のスレッドはアダプタの既定どおり user / project / local の設定を読む `crates/acp_client/src/preset.rs:33-35,128-135`
  - エクスプローラが隠すのは `.git` だけ `crates/project/src/project.rs:227-233`

### B27 Codex goal — 無
- Orca: PR #22377
- necoder: 設定の `fleet_goal` はあるが、参照が 0 件の死にコード `crates/settings_core/src/settings_core.rs:211-213`（§12-9）
- 検索: `thread/goal|goal_mode|"/goal"` → 0 件

### B28 エージェント別の新規タブのキー — 一部
- Orca: od:terminal.mdx §Shortcuts「Each supported agent also has its own per-agent "New agent tab" action」
- necoder: ⌘⇧A は既定のエージェントのみ `crates/keymap_core/src/keymap_core.rs:340`

### B29 完了通知・未読 — 一部
- Orca: od:notifications.mdx。od:model/quick-open.mdx（bell・unread）
- necoder:
  - 画面に見えていない時だけの完了音と入力待ち音 `crates/agent_panel/src/agent_panel.rs:1893-1928`、`crates/agent_panel/src/sound.rs:47-60`
  - トースト `crates/workspace/src/workspace/notifications.rs:118-122`
  - エージェント別のミュート（`toggle_thread_mute`）`crates/agent_panel/src/agent_panel.rs:2698-2702`
  - 未確認の Done は、明示して確認するまで残る（`mark_done_seen`）`crates/agent_panel/src/agent_panel.rs:2846-2852`
  - スマホへの Web Push（承認待ちと質問だけ）`relay/host/bridge.mjs:225-240`
  - OS の通知・Dock・ベル・未読に戻す、は無い（§H01-H03）

### B30 Agents feed（横断フィード） — 一部
- Orca: od:activity.mdx
- necoder:
  - Fleet 下段のニュース（task_events の鏡・起動時に DB から読み戻す）`crates/workspace/src/workspace/fleet_view.rs:2535-2614`、`crates/workspace/src/workspace.rs:1284-1312`
  - クリックでの移動・未読バッジ・⌘F フィルタは無い。Fleet モード限定で 30 行まで

### B31 会話名の同期 — 一部
- Orca: od:terminal.mdx「tab titles can also show the **AI Vault conversation name** … manual renames still win」
- necoder:
  - 初回ターンの後に `claude -p` / `codex exec` で 1 回だけ命名し、手動の改名を優先する `crates/agent_panel/src/agent_panel.rs:2464-2520,4253`
  - エージェント側の会話名の更新は捨てる `crates/acp_client/src/acp_client.rs:1972`

### B32 モデル一覧を動的に取得 — 有
- Orca: od:agents/native-chat.mdx「**Claude** model choices come from the **installed Claude CLI on that host** (not a hardcoded list)」
- necoder:
  - ACP でエージェントが広告した値をそのまま使う `crates/agent_panel/src/agent_panel.rs:842-853,4770-4782`、`crates/acp_client/src/acp_client.rs:1236-1239,1945-1950`
  - 受け取った候補はエージェント単位で保持する

---

## 4. 領域 C — ターミナル・ペイン・セッション復元

### C01 描画の品質 — 一部
- Orca:
  - README「Ghostty-class terminals with WebGL rendering」
  - od:terminal.mdx「the same xterm.js-based terminal VS Code uses」
- necoder:
  - alacritty_terminal を使い、GPUI の自前 Element で描く `crates/terminal_view/src/terminal_view.rs:1117-1438`
  - 1 セルごとに shape している（バッチ処理なし）`crates/terminal_view/src/terminal_view.rs:1298`
  - 解釈する属性は BOLD / ITALIC / INVERSE / HIDDEN だけ `crates/terminal_view/src/terminal_view.rs:1300-1324`
  - 下線・取消線・DIM・カーソルの形と点滅・リガチャ・マウス報告は無い

### C02 端末の分割（無限・ネスト） — 無
- Orca:
  - od:model/tabs-panes-splits.mdx「**Split terminal right** or **Split terminal down**」
  - README「infinite splits」
- necoder: 端末はタブの列だけで、分割構造を持たない `crates/terminal_view/src/dock.rs:23-34`
  - エディタには ⌘\ の右分割が 1 枚だけある `crates/workspace/src/workspace/editor_area/tabs.rs:584-653`
- 検索: `split_down|SplitDown|split_terminal|SplitTerminal|下に分割` → 0 件

### C03 タブのドラッグで分割・種類の違うタブの混在 — 一部
- Orca: od:model/tabs-panes-splits.mdx「Drag a tab to the edge of a pane to create a split」「terminals, diffs, and browser tabs side by side」
- necoder:
  - タブのドラッグは並べ替えだけ `crates/workspace/src/workspace/chrome.rs:701-717`
  - タブの種類は Editor / Image / Pdf で、端末とエージェントは別のドックにある `crates/workspace/src/workspace.rs:281-296`

### C04 worktree ごとにレイアウトを保持 — 有
- Orca: od:model/tabs-panes-splits.mdx §Tab groups across worktrees
- necoder:
  - worktree ごとの ProjectSession が、タブ・分割・TerminalDock・AgentPanel を保持する `crates/workspace/src/workspace/project_session.rs:67-95,204-207`
  - 切替は active を差し替えるだけで、PTY は壊さない `crates/workspace/src/workspace/project_switch.rs:138-139,359-377`

### C05 アプリを終了しても PTY が生き続ける（daemon） — 無
- Orca: od:model/session-restore.mdx「A background daemon owns the PTYs, so closing the app window doesn't kill Claude Code, Codex, or any other agent CLI mid-task. On next launch, Orca warm-reattaches」
- necoder:
  - PTY はアプリ内の EventLoop スレッドが持ち、Drop の時に Shutdown する `crates/terminal_view/src/terminal_view.rs:252-257,871-877`
  - ACP の子プロセスは Drop で kill する `crates/host/src/host.rs:208-213`
  - コメントにも「PTY は再起動を越えないので保存しない」とある `crates/workspace/src/workspace.rs:1102-1103`
  - 代わりに、会話は session/load で冷たく再開できる `crates/acp_client/src/acp_client.rs:1526-1589`
- 検索: `setsid|reattach|warm.?attach|pty.?daemon|nohup|tmux` → 0 件

### C06 scrollback を永続化する — 無
- Orca: od:model/session-restore.mdx「**Terminal scrollback** — … including output produced while Orca was closed.」
- necoder:
  - 窓の保存内容はプロジェクト・開いたファイル・面・左ドックの幅だけ `crates/workspace/src/persistence.rs:18-47`
  - scrollback はメモリ上の 10,000 行 `crates/terminal_view/src/terminal_view.rs:219-222`
- 検索: `scrollback` → 1 件。無関係（テスト名 `crates/terminal_view/src/terminal_view.rs:1517`）

### C07 端末内の検索 — 無
- Orca: od:terminal.mdx §Search（ハイライト・大文字小文字・正規表現・前後の移動）。PR #9035（件数表示）
- necoder: ⌘F の BufferSearch はアクティブなエディタにしか効かない `crates/workspace/src/workspace/overlays.rs:857-865`
- 検索: `RegexSearch|search_next|TerminalSearch|term_search|terminal_search`
  - 全体では 3 件。すべて無関係（Agent パネルの会話内検索 `locales/ja.yml:23`、`crates/agent_panel/src/search.rs:283`）
  - `crates/terminal_view` に絞ると 0 件

### C08 OSC 52 — 無
- Orca: od:terminal.mdx §TUI clipboard (OSC 52)
- necoder: alacritty の ClipboardStore イベントは `_ => {}` で捨てている `crates/terminal_view/src/terminal_view.rs:352-363`
- 検索: `ClipboardStore|osc.?52` → 0 件

### C09 リンクのポップオーバー・URL・OSC 8 — 無
- Orca: od:terminal.mdx §Link actions。PR #13414、#13857、#21438
- necoder:
  - 行番号のつかないパスは捨てるので、URL はリンクにならない `crates/terminal_view/src/terminal_view.rs:62-73`
  - セルが OSC 8 のハイパーリンクを持たない `crates/terminal_view/src/terminal_view.rs:139-146`
- 検索: `hyperlink|osc.?8\b|link_popover|リンクをコピー` → 0 件

### C10 Copy Context — 無
- Orca: od:terminal.mdx §Copy terminal context
- necoder: 端末に右クリックメニューが無い。範囲を選んで ⌘C でコピーはできる `crates/terminal_view/src/terminal_view.rs:458-465,530-537`
- 検索: `copy_context|Copy Context|コンテキストをコピー` → 0 件

### C11 端末テーマのライブラリ・Ghostty / Warp の取り込み — 一部
- Orca: od:terminal.mdx §Themes, §Ghostty import, §Warp theme import。PR #18126（contrast floor）
- necoder:
  - アプリのテーマは組み込み 7 種とユーザー JSON `crates/theme_core/src/theme_core.rs:41-61,173-191`
  - 端末の ANSI 16 色は VSCode 系で固定。テーマに追従するのは前景・背景・カーソルだけ `crates/terminal_view/src/terminal_view.rs:989-1007,1037-1040`

### C12 kitty keyboard・Shift+Enter・IME の CSI-u・¥ → \ — 無
- Orca: od:terminal.mdx §Native key bindings「Orca advertises the kitty keyboard protocol, so terminal apps see real `Shift+Enter`」「JIS Yen (¥) to Backslash」。PR #13310
- necoder:
  - 修飾キーを見ない最小の対応表なので、Shift+Enter も `\r` になる `crates/terminal_view/src/terminal_view.rs:936-937`
  - IME の確定は生の UTF-8 で送る `crates/terminal_view/src/terminal_view.rs:811-826`
  - IME の変換中に押した Enter を PTY に漏らさない処理はある `crates/terminal_view/src/terminal_view.rs:452-456`
- 検索: `kitty|CSI.?u\b|DISAMBIGUATE|KeyboardModes|yen|¥|₩` → 0 件

### C13 Floating terminal / Floating Workspace — 一部
- Orca: od:terminal.mdx §Floating terminal。od:settings.mdx §Floating Workspace
- necoder:
  - repo に紐づかない Chat モード `crates/workspace/src/workspace/project_session.rs:252-271`、`crates/workspace/src/workspace/chat_view.rs:447-460`
  - どこからでも呼べる端末は無く、Chat モードにも端末は無い

### C14 Quick Commands — 無
- Orca: od:terminal.mdx §Quick Commands（コマンドやエージェント向けプロンプトを保存・Global / Project・モバイルと同期）
- necoder:
  - `open_command_terminal` はエージェント CLI の導入とログイン用 `crates/workspace/src/workspace/chrome.rs:1619-1643`
  - FEATURES では tasks.json が later `FEATURES.md:86`
- 検索: `quick.?command|saved.?command|tasks\.json|定型コマンド` → 0 件

### C15 インライン画像 — 無
- Orca: PR #19512（@xterm/addon-image）
- 検索: `sixel|iterm2|inline.?image|addon.?image|ImageProtocol` → 1 件。無関係（Markdown のテスト `crates/markdown/src/markdown.rs:739`）

### C16 Windows のシェル選択 — 一部
- Orca: od:terminal.mdx §Windows shell（PowerShell / CMD / WSL・＋ボタンのサブメニュー・WSL のパス変換）
- necoder: pwsh があれば使い、無ければ powershell を自動で選ぶ `crates/host/src/host.rs:493-521`

### C17 端末へのファイルドロップ — 無
- Orca: od:editing/file-explorer.mdx「Drop files onto an agent terminal to paste their paths at the prompt.」
- necoder: D&D を受けるのはエクスプローラ `crates/workspace/src/workspace/explorer_view.rs:136-142` と、エージェントの入力欄 `crates/agent_panel/src/agent_panel.rs:9545-9548` だけ
- 検索: `on_drop|ExternalPaths|DraggedFile`
  - 全体では 26 件。いずれもエクスプローラ・タブ・入力欄のもの
  - `crates/terminal_view` に絞ると 0 件

### C18 端末のショートカット（⌘T・⌘⇧\・同種タブの巡回・直前タブ） — 一部
- Orca: od:terminal.mdx §Shortcuts。od:model/tabs-panes-splits.mdx §Switching tabs
- necoder:
  - ⌘J で端末のドック `crates/keymap_core/src/keymap_core.rs:333`
  - ⌘\ の右分割はエディタだけ `crates/keymap_core/src/keymap_core.rs:335`
  - ⌘T は WorkspaceSymbols に使っている `crates/keymap_core/src/keymap_core.rs:273`
  - ⌘W と次 / 前のタブが効くのはエディタとスレッドで、端末のタブには効かない `crates/workspace/src/workspace/editor_area/tabs.rs:17-55`

### C19 端末タブの改名 — 一部
- Orca: od:cli/reference.mdx `orca terminal rename`
- necoder:
  - スレッドのタブは状態・アイコン・名前を出し、改名もできる `crates/agent_panel/src/agent_panel.rs:6684-6692,4211-4268`
  - 端末のタブは「ターミナル N」で固定 `crates/terminal_view/src/dock.rs:303`

### C20 シェル統合（OSC 133） — 無
- Orca: `os:src/main/powershell-osc133-bootstrap.ts`、`os:src/main/shell-ready-marker-scanner.ts`
- necoder: FEATURES で later `FEATURES.md:86`
- 検索: `osc.?133|shell.?integration|prompt_mark|FinalTerm|シェル統合` → 0 件

### C21 実行中の端末を閉じる前の確認 — 無
- Orca: PR #21569（confirm before stopping running terminals）
- necoder:
  - ⌘Q は確認なしで終了する `crates/necoder/src/main.rs:723-744`
  - 窓を閉じる時は常に true を返す `crates/workspace/src/persistence.rs:292-301`
  - タブの × で PTY を即座に破棄する `crates/terminal_view/src/dock.rs:252-268`
- 検索: `confirm_quit|quit_confirm|still_running|終了しますか` → 5 件。すべて無関係（Fleet のセルを閉じた時のトースト `locales/ja.yml:373`、`crates/workspace/src/workspace/fleet_view.rs:633-641`）

### C22 シェルの引数・既定のシェル — 無
- Orca: PR #21904（interactive Unix shell arguments）、#21085（default terminal shell）
- necoder:
  - unix では alacritty の既定である `$SHELL` に任せる `crates/host/src/host.rs:498-500`
  - terminal_view は settings crate に依存しておらず、設定の入口そのものが無い
- 検索: `default_shell|shell_args|terminal_shell|login_shell` → 14 件。すべて無関係（起動時にログインシェルから PATH を引き継ぐ処理 `crates/workspace/src/shell_env.rs:63-206`、`crates/necoder/src/main.rs:483`）

### C23 file:line のクリック — 有
- Orca: od:terminal.mdx §Link actions（file path）
- necoder:
  - file:line を検出する（全角が混ざっても列がずれない）`crates/terminal_view/src/terminal_view.rs:48-91`
  - クリックで OpenPath を出す `crates/terminal_view/src/terminal_view.rs:1363-1392`
  - worktree の root を基準に解決する `crates/workspace/src/workspace/panels.rs:134-153`

### C24 端末のフォント・scrollback・カーソル — 無
- Orca: od:settings.mdx §Terminal「Font, theme, cursor style, padding.」
- necoder:
  - 12.5pt・行高 17 で固定 `crates/terminal_view/src/terminal_view.rs:94-95`
  - scrollback は 10,000 行で固定 `crates/terminal_view/src/terminal_view.rs:219-222`
  - フォント名も固定 `crates/terminal_view/src/terminal_view.rs:908`
  - 設定の `font_size` はエディタ用で、端末は読まない `crates/settings_core/src/settings_core.rs:174`
- 検索: `terminal_font|terminal_font_size|scrollback_lines|cursor_shape|cursor_style` → 0 件

---

## 5. 領域 D — エディタ・ビューア・エクスプローラ・ナビゲーション

### D01 自動保存 — 無
- Orca: od:editing/monaco.mdx §Autosave「Files save on blur and after short idle periods.」
- necoder:
  - 保存は ⌘S だけ
  - hot exit は、2 秒アイドルの後に未保存の内容を DB へ退避するだけで、ディスクには書かない `crates/workspace/src/workspace/editor_area/hot_exit.rs:12-28`
  - FEATURES には「v1: 自動保存」と書かれたまま `FEATURES.md:22`
- 検索: `auto_?save|autosave|save_on_blur|自動保存` → 0 件

### D02 プレビュータブ — 無
- Orca: PR #22398（プレビュータブをオフにする設定。つまり既定で有る）
- necoder:
  - エクスプローラのクリックは常に通常のタブを開く `crates/workspace/src/workspace/explorer_view.rs:349-354`、`crates/workspace/src/workspace/explorer_controller.rs:1229-1239`
  - `transient` は diff タブ用で、プレビュータブとは別物 `crates/workspace/src/workspace.rs:302-303`
- 検索: `preview_tab|is_preview|provisional_tab|仮タブ|プレビュータブ` → 6 件。すべて無関係（Chat の `is_previewable` `crates/chat_core/src/folder.rs:186` と、HTML プレビュータブについてのコメント `crates/workspace/src/workspace/notifications.rs:364`）

### D03 Markdown のリッチ編集 — 一部
- Orca: od:editing/markdown.mdx（スラッシュメニュー・`[[` リンク・表の操作・toggle・front matter・目次・レンダ結果での検索）
- necoder:
  - ⌘⇧V とパンくずの切替で、閲覧専用のプレビューが出る `crates/editor_view/src/markdown_preview.rs:1-9,69-147`、`crates/workspace/src/workspace/chrome.rs:1013-1060`
  - 見出し・コード・リスト・表・画像を描く
  - 引用の装飾は「後続」とされている `crates/editor_view/src/markdown_preview.rs:6`
  - WYSIWYG の編集は無い

### D04 Markdown へのレビュー注記 — 無
- Orca: od:editing/markdown.mdx §Review annotations（⌘⇧A Add Review Note）
- necoder: ⌘⇧A は新しいスレッドに使っている `crates/keymap_core/src/keymap_core.rs:340`
- 検索: `review_note|AddReviewNote|レビュー注記` → 0 件

### D05 Mermaid — 無
- Orca: od:editing/viewers.mdx §Mermaid
- necoder: Block の型に図が無い `crates/markdown/src/markdown.rs:66-100`
- 検索: `mermaid|\.mmd\b` → 0 件

### D06 PDF — 一部
- Orca: od:editing/viewers.mdx §PDF（スクロール・ズーム・選択・位置の復元）
- necoder:
  - OS のビューアでタブに出す。SSH 先の PDF はローカルに複製して見せる `crates/workspace/src/workspace/pdf_view.rs:1-19,31-116`
  - 隠れている間は WebView を壊さず隠すだけ `crates/webview_view/src/webview_view.rs:139-201`
  - 閉じて開き直した時の位置の復元と、Linux は無い

### D07 画像・画像 diff — 一部
- Orca: od:editing/viewers.mdx §Images。od:review/diff-viewer.mdx「Image diffs — side-by-side, swipe, and onion-skin modes」
- necoder:
  - 画像タブがある（寸法とサイズの表示、外部変更で読み直す）`crates/workspace/src/workspace/image_view.rs:17-31,80-89,103-153`
  - 画像の diff と、拡大・パンは無い

### D08 CSV / TSV の表 — 無
- Orca: od:editing/viewers.mdx §CSV / TSV
- 検索: `\bcsv\b|\btsv\b|CsvView` → 1 件。無関係（配信時の Content-Type `crates/webview_view/src/sandbox.rs:147`）

### D09 Jupyter notebook — 無（never）
- Orca: od:editing/viewers.mdx §Jupyter notebooks。`os:src/main/notebook/notebook-kernel.ts`
- necoder: FEATURES の「never: telemetry、notebooks、Web 版」`FEATURES.md:139`
- 検索: `ipynb|jupyter|nbformat` → 0 件

### D10 HTML のプレビュー（SSH・隔離・横に並べる） — 一部
- Orca: od:editing/viewers.mdx §HTML（ローカル / SSH / リモートの .html・フォルダの読み取り許可・外部リンクの確認）。PR #16679
- necoder:
  - ローカルの .html を ⌘⇧V で OS の WebView に出し、外部変更で自動で読み直す `crates/editor_view/src/editor_view.rs:315-325,607-626,710-725`、`crates/workspace/src/workspace/chrome.rs:1061-1104`
  - 隔離（専用スキーム・CSP・IPC 無し）は Chat の成果物だけ `crates/webview_view/src/sandbox.rs:1-38`
  - リモートは除外している `crates/editor_view/src/editor_view.rs:318`

### D11 ミニマップ — 無（FEATURES later）
- Orca: od:editing/monaco.mdx §Minimap
- necoder: FEATURES の later `FEATURES.md:41`
- 検索: `minimap|mini_map|ミニマップ` → 0 件

### D12 タブの中で Changes view に切り替える — 一部
- Orca: od:editing/monaco.mdx §Changes view mode
- necoder: 「名前 ⇄ HEAD」の読み取り専用 diff を、別のタブに開く `crates/workspace/src/workspace/editor_area/diff.rs:4-69`

### D13 Word wrap（diff 用は別の設定） — 一部
- Orca: od:editing/monaco.mdx §Word wrap
- necoder: ⌥Z でエディタごとに切り替える `crates/editor_view/src/editor_view.rs:551-555`。既定値は設定画面で変える `crates/settings/src/settings.rs:1648-1655`。diff 専用の設定は無い

### D14 フォント・UI ズーム・density — 一部
- Orca: od:editing/monaco.mdx §Custom editor font。od:settings.mdx §General（UI zoom）・§Appearance（density）
- necoder:
  - エディタの font_size だけ変えられる `crates/settings/src/settings.rs:1688-1697`
  - フォントファミリはコードに直書き `crates/editor_view/src/editor_view.rs:2161-2169`、`crates/workspace/src/workspace.rs:2081`
  - density は型だけで、描画から参照されていない（§12-7）

### D15 パス:行のコピー（⌘⌥C） — 一部
- Orca:
  - Zenn の紹介記事（「⌘ + option + C」でファイルパスと行番号をコピー）
  - od:editing/file-explorer.mdx「**Copy Relative Path** (`Cmd+Option+Shift+C`)」
- necoder:
  - タブの右クリックに「パスをコピー」「相対パスをコピー」`crates/workspace/src/workspace/chrome.rs:743-756,860-875`
  - エクスプローラに「パスをコピー」`crates/workspace/src/workspace/explorer_view.rs:1053-1058`
  - 行番号つきのコピーは無い

### D16 自然順・git の色・右クリックの discard / stage — 一部
- Orca: od:editing/file-explorer.mdx（「natural (numeric-aware) name order」・git の状態の色・「discard, stage」）
- necoder:
  - 外部の変更を即座に反映する `crates/workspace/src/workspace/project_watcher.rs:16-80`
  - git の色 `crates/workspace/src/workspace/project_session.rs:1522-1539`
  - 並び順はディレクトリ優先 → 小文字の辞書順なので、100 が 9 より前に来る `crates/project/src/project.rs:277-283`
  - staged と未 stage を畳んでいる `crates/project/src/project.rs:518-526`
  - エクスプローラのメニューに discard / stage は無い

### D17 ドロップ（Markdown へ画像・ファイルを OS のクリップボードへ） — 一部
- Orca: od:editing/file-explorer.mdx §External drag-drop。「choose **Copy** to place the file itself on the OS clipboard」
- necoder: Finder からエクスプローラへのドロップでコピーする（ローカルのみ）`crates/workspace/src/workspace/explorer_view.rs:126-146`、`crates/workspace/src/workspace/explorer_controller.rs:357-389`

### D18 フォルダの中を検索 — 無
- Orca: od:editing/file-explorer.mdx §Search a folder
- necoder: プロジェクト検索は常に worktree のルートから `crates/workspace/src/workspace/overlays.rs:813-833`
- 検索: `find_in_folder|search_in_folder|FindInFolder|フォルダ内` → 0 件

### D19 ⌘P（recency・gitignored を 2 パス目で） — 一部
- Orca: od:model/quick-open.mdx §Quick Open
- necoder:
  - 最大 5 万件を背景で集め、5 万件で 1 キー 16ms 以内を見るベンチがある `crates/workspace/src/workspace/overlays.rs:17-23`、`crates/ui/examples/bench_fuzzy.rs:18-65`
  - スコアは連続一致と先頭寄りだけ `crates/ui/src/ui.rs:471-499`
  - gitignored は除外する `crates/host/src/host.rs:672-680`

### D20 新規タブの omnibox — 無
- Orca: od:model/quick-open.mdx §New-tab omnibox
- necoder: ピッカーの種類に該当するものが無い `crates/workspace/src/workspace.rs:224-240`
- 検索: `omnibox|tab_switcher|TabSwitcher` → 0 件

### D21 Jump palette（タブの検索・最近のセッション・PR 番号・作成行） — 一部
- Orca: od:model/quick-open.mdx §Worktree Jump Palette
- necoder:
  - ⌘O でレールの全プロジェクトと worktree を一覧する `crates/workspace/src/workspace/overlays.rs:95-235`
  - ⌘1〜9 と ⌘⇧U `crates/keymap_core/src/keymap_core.rs:349,357-365`
  - 「worktree を作る」行の部品（`Picker::set_query_action`）はあるが、呼び出しが 0 件（§12-9）

### D22 設定の検索 — 無
- Orca: od:settings.mdx「Everything here is searchable with `Cmd-,` then typing a keyword.」
- necoder: 設定画面は 5 ページの左ナビだけ `crates/settings/src/settings.rs:255-275`
- 検索: `settings_search|search_settings|設定を検索` → 0 件

### D23 キー割り当ての GUI — 一部
- Orca: od:settings.mdx §Shortcuts「Full keymap — every binding remappable.」
- necoder:
  - ユーザーの keymap.json は保存するとすぐ反映される `crates/necoder/src/main.rs:206-278`
  - ⌘K ⌘S の一覧は参照専用で、既定の keymap しか見せない `crates/workspace/src/workspace/shortcut_sheet.rs:187-193`

### D24 言語機能 — 有（優位）
- Orca: od:editing/monaco.mdx §Language support「Orca is intentionally editor-first, not IDE-first — run type-checkers and linters in a terminal pane.」
- necoder:
  - 7 系統の LSP（SSH 先でも解決する）`crates/lang/src/lsp.rs:36-74`
  - F12 / ⇧F12 / F2 / ⌘. / ⌘T / F8 / ⌥⇧F `crates/keymap_core/src/keymap_core.rs:266-279`
  - tree-sitter の増分ハイライト `crates/editor_view/src/editor_view.rs:307-313`

### D25 大きなファイル・フォルダでの性能 — 一部
- Orca:
  - od:editing/markdown.mdx「For files larger than 300 KB, Orca opens the raw editor first」
  - `os:src/renderer/src/components/right-sidebar/FileExplorerVirtualRows.tsx`
- necoder:
  - rope `crates/editor_core/src/editor_core.rs:12,106-108`
  - 見えている行だけを描く `crates/editor_view/src/editor_view.rs:2698-2701`
  - 予算を超えると exit 1 するベンチ `crates/editor_core/examples/bench_editor.rs:72-86`
  - エクスプローラの行は仮想化しておらず、毎回全行を描く `crates/workspace/src/workspace/explorer_view.rs:167-202`

### D26 対応言語の幅 — 一部
- Orca: `os:src/renderer/src/lib/language-detect.ts`（拡張子 92 個 → 言語 ID 53 個）
- necoder: tree-sitter で 15 言語 `crates/lang/src/lang.rs:41-57,100-120`。Java・C#・Ruby・PHP・Swift・Kotlin・SQL・Dockerfile などは色が付かない

### D27 Find に選択を入れる・F7 — 有
- Orca: od:editing/monaco.mdx「File find seeds the search box from the current selection」。od:review/diff-viewer.mdx「`F7` / `Shift+F7`」
- necoder:
  - ⌘F は選択を検索語の初期値にする `crates/workspace/src/workspace/overlays.rs:855-887`
  - F7 / ⇧F7 `crates/keymap_core/src/keymap_core.rs:276-277`、`crates/workspace/src/workspace/editor_area/diff.rs:71-107`

### D28 全部閉じる・タブのピン留め — 一部
- Orca: od:model/tabs-panes-splits.mdx「`Cmd+Option+W`」。od:mobile.mdx（Close Other / Left / Right・pinned tabs）
- necoder: 「閉じる / 他のタブを閉じる / 右側のタブを閉じる」`crates/workspace/src/workspace/chrome.rs:877-907`、`crates/workspace/src/workspace/editor_area/tabs.rs:491-521`

### D29 Open in（VS Code / Cursor / Zed / ターミナル） — 一部
- Orca: od:settings.mdx §General「Open in menu」。od:ssh.mdx §Open a remote workspace in VS Code
- necoder: 「既定のアプリで開く」と「Finder で表示」だけ `crates/workspace/src/workspace/explorer_view.rs:986-1006`、`crates/project/src/project.rs:2118-2142`

---

## 6. 領域 E — レビュー・Git・連携

### E01 combined diff — 一部
- Orca: od:review/diff-viewer.mdx「**Combined diff** across all staged, unstaged, and untracked files.」
- necoder:
  - diff は 1 ファイルずつ HEAD と比べ、読み取り専用の一時タブに出す `crates/workspace/src/workspace/editor_area/diff.rs:24-66`
  - 無題のバッファなので +/− の色も付かない `crates/editor_view/src/editor_view.rs:307-309`
  - まとめて見られるのは Fleet の「変更」一覧だけ `crates/workspace/src/workspace/fleet_stage.rs:418-446`

### E02 比較の基準を切り替える — 一部
- Orca: od:review/diff-viewer.mdx §Scoping「switch to comparing against any commit, branch, or the base ref」
- necoder:
  - Fleet の一覧は Task の base_oid が起点 `crates/workspace/src/workspace/project_session.rs:1431-1452`
  - diff タブは HEAD と比べる `crates/workspace/src/workspace/editor_area/diff.rs:53-56`
  - 切り替える UI は無い。この 2 つの食い違いが §12-3 の不具合になっている

### E03 diff の横のファイルツリー — 一部
- Orca: od:review/diff-viewer.mdx §File tree
- necoder: Fleet カードのサイド欄に、平らな変更一覧がある（幅はドラッグで変えられるが保存されない）`crates/workspace/src/workspace/fleet_stage.rs:382-399,418-446`

### E04 変更の無い部分の折り畳み・空白の表示 — 一部
- Orca: PR #11955（collapsed unchanged regions）、#15120（Show Whitespace）
- necoder: 前後 3 行で固定（imara-diff の UnifiedDiffBuilder）。切り替えは無い

### E05 3-way の conflict UI・Abort — 無
- Orca: od:review/diff-viewer.mdx「**Merge-conflict UI** with three-way view and inline resolution.」。od:review/commit-push.mdx「**Abort merge** or **Abort rebase**」
- necoder（代わりにあるもの）:
  - 統合の前に `git merge-tree` で衝突を判定する Conflict Radar `crates/project/src/project.rs:798-815`
  - 統合に失敗したら自動で `merge --abort` する `crates/project/src/project.rs:852-856`
- 検索: `three.?way|conflict_marker|resolve_conflict|<<<<<<<|MERGE_HEAD` → 0 件

### E06 diff の行コメント — 無
- Orca:
  - od:review/annotate-ai-diff.mdx §Leave a comment「Hover any line in the diff. A **+** appears in the gutter.」
  - PR #20959（複数行の範囲）
  - `os:src/renderer/src/components/diff-comments/`（テストを除き 2,007 行）
- necoder:
  - gutter を押して出るのは hunk のメニューだけ `crates/workspace/src/workspace/editor_area/diff.rs:239-317`
  - エージェントへの添付はファイル単位 `crates/agent_panel/src/agent_panel.rs:4928-4974`
- 検索: `review_comment|ReviewComment|line_comment|diff_comment|DiffComment|行コメント` → 2 件。すべて無関係（`⌘/` のための「行コメント」接頭辞 `crates/lang/src/lang.rs:1269` と、テストのコメント）

### E07 まとめてエージェントへ送る — 無
- Orca: od:review/annotate-ai-diff.mdx §Send the batch「composes a single prompt with all your comments, line-anchored, then opens a **Send notes to** menu」。PR #10070
- necoder: 名前の似た `SendToAgent` は Todo の 1 項目をスレッドへ送る機能で、別物 `crates/workspace/src/workspace/todo_panel.rs:6,265,329`
- 検索: `review_notes|send_notes|SendNotes|まとめて送` → 1 件。無関係（effect cycle についてのコメント `crates/workspace/src/workspace.rs:1624`）

### E08 Resolve・再レビュー — 無
- Orca: od:review/annotate-ai-diff.mdx §Reply, resolve, re-review
- 検索: `resolve_comment|unresolved_comment|comment_thread` → 0 件

### E09 AI の帰属（行単位） — 一部
- Orca: od:review/attribution.mdx
- necoder:
  - エージェントが触ったファイルは、gutter の diff バーがスレッドの色になる `crates/workspace/src/workspace/notifications.rs:290-306`、`crates/editor_view/src/editor_view.rs:2764-2771`
  - エクスプローラにも色の点が付く `crates/workspace/src/workspace/explorer_view.rs:339-344`
  - 帰属はファイル単位。bypass モードでは付かない `docs/ROADMAP.md:142`

### E10 Source Control パネル — 一部
- Orca: od:review/commit-push.mdx §Source control panel（stage / discard・Commit / Push / Pull / Sync・状態で変わる主ボタン）
- necoder:
  - ファイル単位の stage / unstage、すべて stage、メッセージ、⌘⏎ でコミット、push、pull `crates/workspace/src/workspace/git_view.rs:228-283,506-535`、`crates/workspace/src/workspace/git_controller.rs:280-286`
  - discard・Sync・主ボタンは無い
  - 行を押しても diff が開かず、失敗は画面に出ず、⌘V が効かない（§12-2 / 4 / 5）

### E11 AI によるコミットメッセージ — 有
- Orca: od:review/commit-push.mdx「**Generate with AI**」
- necoder: `git diff HEAD | claude -p` の結果をメッセージ欄に入れる `crates/project/src/project.rs:1126-1152`、`crates/workspace/src/workspace/git_controller.rs:535-569`

### E12 フック失敗時の Fix with AI — 無
- Orca: od:review/commit-push.mdx「use **Fix with AI** from the failure details」
- necoder:
  - コミットの失敗は eprintln に出るだけ `crates/workspace/src/workspace/git_controller.rs:366`
  - そもそもフックが走らない（§12-1）
- 検索: `fix_with_ai|Fix with AI|FixWithAi` → 0 件

### E13 force-with-lease — 無
- Orca: od:review/commit-push.mdx「**Force push with lease**」
- necoder: push は素の `git push` と、失敗した時の `--set-upstream` での再試行だけ `crates/project/src/project.rs:1003-1022`
- 検索: `force-with-lease|force_with_lease|force_push|ForcePush` → 0 件

### E14 amend — 無
- Orca: od:review/commit-push.mdx §Amend
- 検索: `amend` → 0 件

### E15 conflict を AI に渡す — 無
- Orca: od:review/commit-push.mdx「**Resolve with AI** next to **Review conflicts**」
- necoder:
  - Radar で衝突が出ると changes_requested になる `crates/workspace/src/workspace/fleet_stage.rs:513-516`
  - 「修正を指示」は会話を開くだけで、衝突の中身は送らない `crates/workspace/src/workspace/control_view.rs:633-649`
- 検索: `resolve_with_ai|Resolve with AI|ResolveWithAi` → 0 件

### E16 AI アクションのレシピ（repo ごとの上書き） — 無
- Orca: od:review/commit-push.mdx §Per-repo AI action recipes
- necoder: プロンプトと claude をコードに直書きしている `crates/project/src/project.rs:1128-1131`
- 検索: `action_recipe|prompt_template|commit_prompt_template` → 0 件

### E17 PR の作成（アプリ内・AI で詳細を生成） — 一部
- Orca: od:review/commit-push.mdx §Open a hosted review「**Generate pull request details with AI**」
- necoder:
  - 「PR」ボタンは `gh pr create --web` でブラウザの作成画面を開く `crates/project/src/project.rs:1084-1093`、`crates/workspace/src/workspace/git_view.rs:104-124`
  - never（ホスティング連携）の中で、最小限だけ入っている

### E18 stacked PR — 無（never）
- Orca: od:review/github.mdx §Stacked pull requests。PR #13750
- 検索: `stacked|gh pr create --base` → 0 件

### E19 PR ビュー — 無（never）
- Orca: od:review/github.mdx §Reviews（checks・reviews・コメント・スレッドへの返信・リアクション）。PR #13470
- necoder: `gh pr view --web` でブラウザに出すだけ `crates/project/src/project.rs:1100-1116`
- 検索: `gh pr checks|pr_checks|pull_request_view|reaction` → 0 件

### E20 auto-merge・merge queue — 無（never）
- Orca: od:review/github.mdx §Auto-merge
- necoder: 統合はローカルの `merge --no-ff` で、方式が違う `crates/project/src/project.rs:830-858`
- 検索: `auto.?merge|merge_queue|gh pr merge` → 0 件

### E21 CI の失敗・Actions のログ — 無（never）
- Orca: od:review/github.mdx「**Fix broken checks**」§Actions
- 検索: `gh run|check_runs|statusCheckRollup|actions/runs` → 0 件

### E22 GitLab・Bitbucket・Azure DevOps・Gitea — 無（never）
- Orca: od:review/github.mdx §Connecting a provider。PR #5832
- necoder: GitHub 以外の remote では PR ボタンが出ない `crates/project/src/project.rs:1064-1076`
- 検索: `gitlab|glab\b|bitbucket|azure devops|dev\.azure|gitea` → 1 件。無関係（gitlab の URL を弾くテスト `crates/project/src/project.rs:3063`）

### E23 worktree ごとの PR の状態 — 一部
- Orca: od:review/github.mdx「Linked reviews show up in the sidebar with status」
- necoder: ↗ ボタンで、現在のブランチの PR（無ければ repo）をブラウザで開く `crates/workspace/src/workspace/git_view.rs:125-143`

### E24 Issues — 無（never）
- Orca: od:review/github.mdx §Issues
- 検索: `gh issue|IssueDrawer|issue_list` → 0 件

### E25 GitHub Projects — 無（never）
- Orca: od:review/github.mdx §Tasks。PR #17795（Roadmap をタイムラインで表示）
- 検索: `projectV2|gh project|github projects` → 0 件

### E26 Linear — 無
- Orca: od:review/linear.mdx
- 検索: `linear\.app|LINEAR_API|linear_api|LinearIssue` → 0 件

### E27 Jira — 無
- Orca: od:review/jira.mdx
- 検索: `\bjira\b|atlassian` → 0 件

### E28 課題から worktree を作る — 無
- Orca: od:review/github.mdx「Creating a worktree from a GitHub issue or PR opens the interactive workspace composer」
- necoder: Task は自由記述の題名から `task/<slug>` を作るだけ `crates/workspace/src/workspace/new_task_dialog.rs:101`
- 検索: `issue_url|github\.com/[^ ]*/issues` → 5 件。すべて無関係（リンク検出のテストの題材 `crates/ui/src/links.rs:361-366` と、バグ報告の URL `crates/workspace/src/crash.rs:14,157,322`）

### E29 GitHub API の残量 — 無（never）
- Orca: od:github-errors.mdx §Check GitHub API Budget
- necoder: GitHub をポーリングせず、ボタンを押した時だけ gh を呼ぶので、実害は小さい
- 検索: `X-RateLimit|rate_limit|api_budget` → 2 件。すべて無関係（relay の乱用対策 `relay/src/worker.ts:63,133`）

### E30 プロジェクトごとの gh アカウント — 無（never）
- Orca: PR #13664
- 検索: `gh auth|gh_account|gh_host|GH_HOST` → 0 件

### E31 ブランチの行と +/− の行数 — 一部
- Orca: od:review/commit-push.mdx（branch → base の表示・+/− のチップ・Source / Tests / Generated の内訳）
- necoder:
  - statusbar の ⎇ branch ●N `crates/workspace/src/workspace/chrome.rs:2123-2129`
  - Task の base を起点にした +N −M のチップ `crates/workspace/src/workspace/fleet_stage.rs:122-129`
  - ⌘O の ↑ahead ↓behind `crates/workspace/src/workspace/overlays.rs:209-217`

### E32 変更行のパスのコピー — 一部
- Orca: od:review/commit-push.mdx「Right-click a changed file for **Copy Path** / **Copy Relative Path**」
- necoder: エクスプローラ `crates/workspace/src/workspace/explorer_view.rs:1055` とタブ `crates/workspace/src/workspace/chrome.rs:862-869` にはある。ソース管理の行には右クリックメニューが無い

### E33 diff から HTML を横にプレビュー — 一部
- Orca: od:review/diff-viewer.mdx「**Open Preview to the Side**」
- necoder: エージェントが書いた HTML / MD は、ツールカードの「▣ プレビュー」から開ける `crates/agent_panel/src/agent_panel.rs:11036-11073`

### E34 コミットの署名・外部エディタ — 無
- Orca: od:settings.mdx §Git「Commit signing options. Editor for external git tools.」
- necoder: 設定に git 系の項目が無い `crates/settings_core/src/settings_core.rs:172-272`。git の CLI を使うので、利用者の gitconfig の gpgsign はおそらく効く（実機では未確認）
- 検索: `gpgsign|gpg\.sign|commit\.sign|GIT_EDITOR|core\.editor` → 0 件

### E35 Git 操作の総量（necoder 側の確認用） — 一部
- necoder にあるもの:
  - ブランチの切替・作成・削除 `crates/workspace/src/workspace/git_controller.rs:70-130,572-626,631-681`
  - worktree を開く・削除する（削除前に失うものを数える）`crates/workspace/src/workspace/worktree_delete.rs:107-114`
  - hunk 単位の stage・巻き戻し・コピー `crates/workspace/src/workspace/editor_area/diff.rs:109-383`
  - カーソル行の blame
  - コミットグラフ（30 件）`crates/workspace/src/workspace/git_view.rs:542-696`
  - push・pull（ff-only）
  - Task を作る前に base を自動で早送りする `crates/project/src/project.rs:696-748`
  - Radar と Integrate
- necoder に無いもの: stash（FEATURES の never「graph/stash UI」）

---

## 7. 領域 F — 内蔵ブラウザ・Design Mode・自動化

前提:
- necoder の WebView が読み込めるのは `file://` と、Chat の成果物用の `necoder-artifact://` だけ `crates/webview_view/src/webview_view.rs:68,105-113`。
- FEATURES の「拡張 / API 向けの汎用 webview は never」`FEATURES.md:100` と、「Chromium / ブラウザエンジンは同梱しない」`FEATURES.md:95` が効いている。

### F01 worktree ごとの内蔵ブラウザ — 無（never）
- Orca: od:browser/overview.mdx「Every Orca worktree has its own browser. It's a real Chromium window — address bar, history, devtools」
- necoder:
  - タブの種類は Editor / Image / Pdf だけ `crates/workspace/src/workspace.rs:281-295`
  - ブラウザらしい操作は、再読込ボタン `crates/workspace/src/workspace/chrome.rs:1082-1103` とスワイプの戻る / 進む `crates/webview_view/src/webview_view.rs:286` くらい
- 検索: `address.?bar|url_bar|UrlBar|load_url|go_back|アドレスバー` → 5 件。すべて無関係（更新の `browser_download_url` `crates/workspace/src/updater.rs:36,126`）

### F02 DevTools — 一部
- Orca: od:settings.mdx §Browser「Devtools opt-in.」
- necoder: プレビュー用の WebView は、右クリック →「要素を検証」で Web Inspector が常に使える `crates/webview_view/src/webview_view.rs:287-289`。任意の Web ページには使えない

### F03 プロファイル・cookie の取り込み・passkey — 無（never）
- Orca: od:browser/profiles.mdx。PR #14687（WebAuthn の account picker）
- 検索: `cookie|passkey|webauthn|incognito` → 0 件

### F04 ビューポート・デバイスのエミュレーション — 無（never）
- Orca: od:browser/overview.mdx §Viewport-size emulation。od:cli/overview.mdx `orca set device --name "iPhone 12"`
- 検索: `user_agent|with_user_agent|device_profile|emulate|エミュレーション` → 0 件

### F05 ダウンロード — 無（never）
- Orca: od:browser/overview.mdx §Downloads
- 検索: `download_started|with_download|DownloadShelf` → 0 件

### F06 Design Mode — 無（never の範囲とぶつかる・本体の §2 に実装案）
- Orca:
  - od:browser/design-mode.mdx「click any UI element on the rendered page, and the element drops into the agent chat as rich context — with its DOM, computed styles, and a screenshot」「The source file/line if a dev-mode source map is available.」
  - od:recipes/design-mode-fix.mdx
  - 実装（テストを除く）:
    - 注入スクリプト `os:src/main/browser/grab-guest-*.ts` 971 行
    - スクショの切り抜き `os:src/main/browser/browser-grab-screenshot.ts` 106 行
    - 画面側の注記 UI `os:src/renderer/src/components/browser-pane/annotate/` 4,390 行
    - 集める項目の型 `os:src/shared/browser-grab-types.ts`
  - ソースの位置は React の `_debugSource` だけを見ている `os:src/main/browser/grab-guest-react-script.ts:73`。React 19 はこれを廃止したので（[facebook/react #32574](https://github.com/facebook/react/issues/32574)）、React 19 のアプリではソース位置が取れない
- necoder: Chat の成果物には、あえてページから necoder を呼ぶ口（IPC）を付けていない `crates/webview_view/src/sandbox.rs:11-13`
- 検索: `design.?mode|getComputedStyle|computed.?style|element.?picker|outerHTML|with_ipc_handler|with_initialization_script` → 0 件

### F07 ページへの注釈 — 無
- Orca: PR #17511（注釈のトレイ）。`os:src/renderer/src/components/browser-pane/annotate/`
- 検索: `page_annotation|annotate_page|ページ注釈` → 0 件

### F08 リンクの開き先を選ぶ — 無
- Orca: od:browser/overview.mdx §Link routing
- necoder:
  - エージェントの返答にある URL は、OS の既定のブラウザで開く `crates/agent_panel/src/agent_panel.rs:753-756`、`crates/workspace/src/crash.rs:220`
  - Markdown プレビューのリンクは、色と下線が付くだけでクリックできない `crates/editor_view/src/markdown_preview.rs:285-293`
- 検索: `link_routing|system_browser|SystemBrowser` → 0 件

### F09 ブラウザ自動化の CLI — 無（never）
- Orca: od:cli/reference.mdx §Built-in browser（`orca goto / snapshot / click / fill / wait / screenshot / console / network / pdf`）
- necoder:
  - MCP の道具は、ファイル・検索・git・fleet_* だけ `crates/necoder/src/mcp.rs:130-260`
  - 制御 IPC は open / spawn_agent / send / tasks 系だけ `crates/workspace/src/workspace/control_ipc.rs:286-732`
- 検索: `browser_goto|browser_click|browser_fill|"goto"|"fill"` → 0 件

### F10 リモート経由のブラウザ通信 — 無
- Orca: od:browser/overview.mdx §Remote workspaces
- necoder: SSH の転送は、リモートの `ne` が GUI へ戻るための reverse forward だけ `crates/host/src/host.rs:2477-2505`
- 検索: `LocalForward|DynamicForward|socks` → 0 件

### F11 成果物の公開リンク（Share as artifact） — 無
- Orca: od:browser/overview.mdx §Share as artifact。od:cli/reference.mdx §Artifacts
- necoder:
  - relay が持つ API は `/api/health` と `/api/rooms/:id(/ws)` だけ `relay/src/worker.ts:46-65`
  - `publish_artifact` は M16 で「後回し」と明記されている `docs/ROADMAP.md:265`
- 検索: `share_artifact|publish_artifact|public.?link|公開リンク` → 0 件

### F12 Computer Use — 無
- Orca: od:cli/computer-use.mdx
- 検索: `AXUIElement|CGEvent|computer_use|computer-use|list_apps|UIAutomation` → 5 件。すべて無関係（Captain 席で Codex の computer-use MCP を止める設定とテスト `crates/acp_client/src/preset.rs:211-216`、`crates/agent_panel/src/seat.rs:360`）

### F13 iOS / Android エミュレータ — 無
- Orca: od:cli/overview.mdx §Mobile emulator。od:cli/skills.mdx（`orca-emulator-android`）
- 検索: `simctl|simulator|emulator|\badb\b|xcrun` → 0 件

### F14 HTML を横に並べてプレビュー — 一部
- Orca: od:editing/viewers.mdx §HTML「**Open in Orca Browser** or **Open Preview to the Side**」
- necoder（有る部分）:
  - ローカルの .html をタブの中で ⌘⇧V で表示し、保存時と外部変更時に読み直す `crates/editor_view/src/editor_view.rs:315-325,1065,1712`
  - ツールカードの「▣ プレビュー」`crates/agent_panel/src/agent_panel.rs:11040-11071`
  - Chat では右側に自動で出す `crates/workspace/src/workspace/chat_view.rs:276-300`
- necoder（無い部分）:
  - SSH / リモートの HTML（`crates/editor_view/src/editor_view.rs:318` で除外）
  - Linux `crates/webview_view/src/webview_view.rs:33-35`
  - 「横にプレビュー」の専用コマンド

### F15 Web 検索 — 無（never）
- Orca: od:model/quick-open.mdx「Type a web search … with your [Default Search Engine]」
- 検索: `search_engine|google\.com/search|duckduckgo|検索エンジン` → 0 件

### F16 ページのズーム — 無（never）
- Orca: od:settings.mdx §Browser「Default Zoom」
- 検索: `zoom_factor|set_zoom|page_zoom|with_zoom` → 0 件

---

## 8. 領域 G — リモート・モバイル・CLI・オーケストレーション

### G01 SSH ターゲットの登録 UI・接続テスト — 一部
- Orca: od:ssh.mdx §Add an SSH target（フォーム・OpenSSH config のピッカー・一括取り込み・Test）
- necoder:
  - `~/.ssh/config` の Host 一覧、最近のリモートプロジェクト、`ssh://…` の手入力 `crates/workspace/src/workspace/remote_ssh.rs:46-112`、`crates/host/src/host.rs:2120-2140`
  - 登録フォーム・Include の展開・接続テストは無い

### G02 ホスト鍵の検証と案内 — 一部
- Orca: od:ssh.mdx §Host key verification。PR #14844
- necoder:
  - 検証は system OpenSSH の known_hosts に任せる `crates/host/src/host.rs:2770-2784`
  - 初回の指紋は SSH_ASKPASS 経由で GUI にそのまま出る `crates/workspace/src/workspace/remote_ssh.rs:548-553`
  - 失敗した時は「exit status」しか出ないので、`ssh-keygen -R` の案内も出ない `crates/host/src/host.rs:2460-2466`

### G03 ProxyJump・多重化・GSSAPI・FIDO2 — 一部
- Orca: od:ssh.mdx §Advanced connection options, §Kerberos / GSSAPI, §FIDO2
- necoder:
  - ControlMaster は常に有効 `crates/host/src/host.rs:2232-2242,2448-2475`
  - ProxyJump と鍵は `~/.ssh/config` に従う `crates/workspace/src/workspace/remote_ssh.rs:43-45`
  - GSSAPI と FIDO2 は system ssh 任せで、動くかは確かめていない

### G04 パスフレーズの保持 — 一部
- Orca: od:ssh.mdx §Passphrases
- necoder: 秘密はどこにも保存しない `crates/workspace/src/workspace/remote_ssh.rs:306-312`。ControlMaster が生きている間は入力し直さなくてよい `crates/host/src/host.rs:2238`

### G05 SSH 上の worktree とエージェント — 有
- Orca: od:recipes/remote-worktrees.mdx
- necoder:
  - Task の作成は slot の Host の上で走る `crates/workspace/src/workspace/fleet_view.rs:491-515`
  - ACP は `host.spawn_process` でリモートに起動する `crates/acp_client/src/acp_client.rs:1506-1508`
  - 保存・commit・push も Host 経由 `crates/project/src/project.rs:951,1003`
  - CLI の `necoder fleet create` だけは LocalHost に固定 `crates/necoder/src/fleet.rs:176-182`

### G06 切断を越えて PTY が生き続ける — 一部
- Orca: od:ssh.mdx §Sessions across app close「Remote terminal sessions are leased through the relay running on the remote host, so they survive Orca closing on your laptop.」
- necoder:
  - 端末は素の `ssh -tt … exec $SHELL -l` なので、切断すると終わる `crates/host/src/host.rs:4183-4232`
  - 会話は session/load で続けられる `crates/acp_client/src/acp_client.rs:1526-1560`

### G07 接続状態の表示と自動再接続 — 一部
- Orca: od:ssh.mdx §Status。PR #12396（カード上で再接続）
- necoder:
  - statusbar の SSH チップ（ok / warn / err）と再接続のボタン `crates/workspace/src/workspace/chrome.rs:2047-2119`
  - heartbeat で自動再接続する `crates/host/src/host.rs:3234-3245`
  - レールのカードには server バッジしか無い `crates/workspace/src/workspace/rail_view.rs:237-258`

### G08 ポート転送 — 無
- Orca: od:ssh.mdx §Port forwarding（listen しているポートを自動検出・1 クリック・再接続後も維持・特権ポートは読み替え）
- necoder: 転送はリモートの `ne` 用の `-O forward -R` だけ `crates/host/src/host.rs:2499-2506`
- 検索: `LocalForward|port.?forward|/proc/net/tcp|ポート転送|ポートフォワード` → 0 件

### G09 リモートのファイルのダウンロード・アップロード — 無
- Orca: od:ssh.mdx §Downloading remote files and folders。od:editing/file-explorer.mdx「For SSH worktrees, drag-drop also works — Orca uploads the file」
- necoder: エクスプローラの D&D コピーはローカル専用と、コードに書いてある `crates/workspace/src/workspace/explorer_view.rs:122-125`、`crates/workspace/src/workspace/explorer_controller.rs:360-372`
- 検索: `sftp|\bscp\b|upload|アップロード` → 7 件。すべて無関係（remote-server のバイナリ配備 `crates/host/src/host.rs:2667-2709`）

### G10 VS Code Remote-SSH で開く — 方式差
- Orca: od:ssh.mdx §Open a remote workspace in VS Code。PR #10005
- necoder: necoder 自体が Remote SSH のエディタで、LSP・検索・Git・端末・ACP をリモートの Host で動かす `crates/lang/src/lsp.rs:184-186`、`crates/acp_client/src/acp_client.rs:1506-1508`

### G11 ツールチェインの無い Linux でも動く — 有（優位）
- Orca: od:ssh.mdx「**remote terminals will not work** until build tools are installed」
- necoder:
  - static-musl の Rust バイナリを配るので、ツールチェインは要らない `crates/host/src/host.rs:2964-2978`、`scripts/bundle-mac.sh:64-86`
  - 端末は素の ssh なので、そのまま動く `crates/host/src/host.rs:4192-4197`

### G12 Remote Orca Server・`orca serve` — 一部
- Orca: od:remote-servers.mdx
- necoder:
  - 別の端末からの操作はスマホの PWA 向けだけ。5 分で切れるペアリング URL と、端末ごとに取り消せる token がある `relay/host/bridge.mjs:245-262,279-291`
  - Tailscale のアドレスは採らないと決めている `docs/DECISIONS.md:192,210`
  - Web のクライアントは FEATURES の never（「Web 版」`FEATURES.md:139`）

### G13 Cloud VM のレシピ — 無（never に相当）
- Orca: od:ways-to-run.mdx §4 Cloud VMs（`orca.yaml` のレシピ・Vercel Sandbox / Fly / Modal / Docker）
- necoder:
  - 近いのは worktree ごとの `.necoder/worktree-setup.sh` `crates/project/src/project.rs:3113,3160`
  - FEATURES の「never: … Cloud Agents …」`FEATURES.md:127`
- 検索: `devcontainer|orca\.yaml|vm_recipe|vercel|fly\.io|docker` → 11 件。すべて無関係（テスト用の Docker ホストと、Dockerfile のアイコン）

### G14 ネイティブのモバイルアプリ — 方式差
- Orca: od:mobile.mdx（iOS App Store / Android APK）
- necoder: ネイティブのアプリは作らず、ホーム画面に置く PWA（Service Worker と Web Push）で代える判断 `relay/public/manifest.webmanifest:1-8`、`relay/public/sw.js:19-25`、`docs/DECISIONS.md:192-196`

### G15 モバイル: 全ホストの一覧・ファイル・端末 — 一部
- Orca: od:mobile.mdx §What you can do from mobile
- necoder:
  - 接続先 PC の切替（1 台ずつ）、プロジェクトとスレッドの一覧と状態、会話の直近 60 項目 `relay/public/app.mjs:39-45,164-214`、`crates/agent_panel/src/remote.rs:49-80`
  - ファイルツリー・生の端末・補助キー・Live モードは無い

### G16 モバイル: 返信・添付・音声 — 一部
- Orca: od:mobile.mdx
- necoder: 返信・中断・新しいスレッド・差分を見てからの承認 / 拒否・選択式の質問への回答 `relay/public/app.mjs:381-393`、`crates/agent_panel/src/remote.rs:139-247`。添付と音声は無い

### G17 モバイル: ソース管理 — 一部
- Orca: od:mobile.mdx §Source control on mobile
- necoder: 読み取り専用の `git diff HEAD`（追跡ファイルのみ・180KB まで）`relay/public/app.mjs:394-397`、`crates/workspace/src/workspace/remote_control.rs:43-66`

### G18 モバイル: アカウント・workspace の作成 — 無
- Orca: od:mobile.mdx §Account switcher, §Create a workspace
- necoder: スレッドのトークン数の表示 `relay/public/app.mjs:194` と、既存プロジェクトの中での新しいスレッドだけ
- 検索: `rate_limit|usage_limit|create_workspace|edit_host` → 2 件。すべて無関係（relay の乱用対策）

### G19 モバイル: 完了の push 通知 — 一部
- Orca: od:mobile.mdx「Get push notifications when an agent finishes」。PR #18554
- necoder: VAPID による本物の Web Push はある `relay/host/bridge.mjs:225-244`、`relay/public/sw.js:19-25`。ただし通知するのは承認待ちと質問待ちだけで、完了は通知しない

### G20 CLI: worktree / repo と selector — 一部
- Orca: od:cli/reference.mdx §Selectors, §Repos, §Worktrees
- necoder: `necoder fleet create/list/status/digest` `crates/necoder/src/fleet.rs:517-624` と、`ne <path>` `crates/necoder/src/cli.rs:75-121`

### G21 CLI: 任意の端末の read / send / wait — 一部
- Orca: od:cli/reference.mdx §Terminals（`terminal read --screen` / `send` / `wait --for tui-idle` / `split` など）
- necoder: エージェントのスレッドなら `fleet spawn-agent / send / digest / wait <id> idle|blocked` で動かせる `crates/necoder/src/fleet.rs:381-419,590-616`。任意の端末は対象外

### G22 CLI: file open / diff — 一部
- Orca: od:cli/reference.mdx §Files
- necoder: `ne <path>` で、起動中の GUI にファイルを開く `crates/necoder/src/cli.rs:75-121`、`crates/workspace/src/workspace/control_ipc.rs:312-371`。diff と open-changed は無い

### G23 Orchestration（Run・DAG・メッセージ・gate・別ホスト） — 一部
- Orca: od:cli/orchestration.mdx
- necoder:
  - Task の DAG（depend / wait-deps）・phase・spawn / send / digest・イベントログ `crates/necoder/src/fleet.rs:421-468`、`crates/necoder/src/mcp.rs:208-265`
  - Captain（分解案を人間が承認した分だけ worktree を切る）`crates/workspace/src/workspace/captain_proposals.rs:1-33`
  - グループ宛て・ask・汎用の gate・別ホストのワーカーは無い

### G24 スケジュール実行（cron / RRULE） — 無（never）
- Orca: od:cli/automations.mdx
- necoder: FEATURES の「never: … Automations 相当」`FEATURES.md:127`
- 検索: `\bcron\b|rrule|automation|定期実行|予約実行` → 1 件。無関係（entitlements の `automation.apple-events` `crates/necoder/resources/necoder.entitlements:9`）

### G25 Skills の配布と更新 — 無
- Orca: od:cli/skills.mdx
- 検索: `SKILL\.md|npx skills|\.claude/skills|skills install` → 0 件

### G26 MCP サーバの登録 — 有
- Orca: od:cli/skills.mdx §MCP servers
- necoder:
  - 設定の MCP ページで、一覧・有効化・絞り込みができる `crates/settings/src/settings.rs:1403-1497`
  - Codex CLI・Claude Code・Cursor の設定から自動で見つける（既定は off）`crates/acp_client/src/mcp.rs:1-33`

### G27 ヘッドレスでのアカウント登録 — 無
- Orca: od:cli/reference.mdx §Account（`orca account add --agent codex`）
- necoder: ログインは各 CLI の login_cmd に任せる `crates/acp_client/src/acp_client.rs:383-394`
- 検索: `account add|add_account|login_account` → 0 件

### G28 ヘッドレスでの運用・QR ペアリング — 一部
- Orca: od:remote-servers.mdx §Alternative: `orca serve`, §Mobile from a headless server
- necoder: `ne remote pair` で端末に QR を出せる `relay/host/cli.mjs:103-121`。GUI の起動が前提 `relay/host/bridge.mjs:246`

### G29 リモートの状態をリアルタイムに反映 — 有
- Orca: od:ssh.mdx「Agent status … propagates over SSH the same way it does locally」
- necoder: ACP のクライアントはローカルで動き、リモートのエージェントとは SSH の stdio で話すので、状態は同じ経路で集まる `crates/acp_client/src/acp_client.rs:1506-1508`、`crates/agent_panel/src/agent_panel.rs:2595-2609`

### G30 リモートで履歴を見る — 一部
- Orca: od:agents/session-history.mdx §Resume needs a local workspace
- necoder:
  - necoder のスレッドの履歴はリモートのプロジェクトでも見られる `crates/workspace/src/workspace/overlays.rs:1305-1384`
  - session/load でリモートのエージェントとして再開できる
  - CLI 自身が保存したセッションは読まない

---

## 9. 領域 H — アプリ全体・通知・設定・配布

### H01 完了の通知（system・音・チップ） — 一部
- Orca: od:notifications.mdx §Agent-finished pings, §Tuning
- necoder:
  - 完了音と入力待ち音 `crates/agent_panel/src/agent_panel.rs:1884-1928`
  - 右下のトースト `crates/workspace/src/workspace/notifications.rs:119-125,193-207,421-450`
  - スマホへの Web Push `relay/host/bridge.mjs:224-243`
  - OS のデスクトップ通知は無い
- 検索: `UNUserNotification|NSUserNotification|display notification|notify_rust` → 0 件

### H02 ベル（通知の履歴）・未読に戻す — 一部
- Orca: od:notifications.mdx §Persistent bell, §Mark unread
- necoder:
  - titlebar の要対応バッジ `crates/workspace/src/workspace/chrome.rs:364-386`
  - 要対応キュー。「確認」で既読にする `crates/workspace/src/workspace/control_view.rs:221-340,775-806`
  - 未読に戻す操作は無い

### H03 Dock のバッジ — 無
- Orca: od:notifications.mdx「On macOS, the same unread count is mirrored as a badge on the Dock icon」。`os:src/main/dock/unread-badge.ts`
- necoder: Dock まわりは右クリックメニュー（新しいウィンドウ）だけ `crates/necoder/src/menus.rs:138-144`
- 検索: `dock_badge|badge_label|NSDockTile|dockTile|set_badge|request_user_attention` → 0 件

### H04 カスタム音・音量 — 一部
- Orca: od:notifications.mdx §Custom sounds（任意のファイル・音量）
- necoder:
  - 完了と入力待ちで別々に、同梱の猫の声 3 種・システム音・off・任意のファイルを選べ、押すと試聴できる `crates/agent_panel/src/sound.rs:63-72`、`crates/settings/src/settings.rs:922-927,1708-1723`
  - 任意のファイルは settings.json に手で書く
  - 音量は無い

### H05 PR の check 失敗・更新の通知 — 一部
- Orca: od:settings.mdx §Notifications「PR check failures. Update available.」
- necoder: 更新だけ（statusbar のチップと About）`crates/workspace/src/workspace/chrome.rs:2274-2348`、`crates/workspace/src/workspace/about.rs:61-90`

### H06 システムトレイ — 無
- Orca: `os:src/main/tray/system-tray.ts`、`tray-attention-icon.ts`
- 検索: `NSStatusItem|NSStatusBar|status_item|system_tray|\btray\b` → 0 件

### H07 音声入力 — 無
- Orca: od:settings.mdx §Voice（オンデバイスの STT。日本語の Parakeet TDT-CTC JA を含む）。PR #8207
- 検索: `dictation|speech|whisper|parakeet|microphone|音声入力|マイク` → 4 件。すべて無関係（relay の Permissions-Policy で `microphone=()` を禁止する設定 `relay/public/_headers:6` と「マイクロ」秒）

### H08 設定の検索 — 無
- Orca: od:settings.mdx「Everything here is searchable with `Cmd-,`」
- 検索: `settings_search|search_settings|設定を検索` → 0 件（D22 と同じ）

### H09 キー割り当ての UI — 一部
- Orca: od:settings.mdx §Shortcuts。PR #14734（Mission Control との衝突の警告）
- necoder: keymap.json は保存すると即反映され、メニューのキー表記も追従する `crates/necoder/src/main.rs:206-273`。GUI・keymap.json を開くコマンド・衝突の警告は無い

### H10 UI のズーム・フォント・density — 一部
- Orca: od:settings.mdx §General, §Appearance
- necoder:
  - テーマ `crates/theme_core/src/theme_core.rs:326-331`
  - アクセント色の代わりのプロジェクト色 `crates/workspace/src/workspace/chrome.rs:2026-2045`
  - エディタの font_size `crates/settings/src/settings.rs:1690`
  - Zoom の項目は窓の最大化のこと `crates/workspace/src/workspace.rs:1995`

### H11 アプリアイコンの切替 — 無
- Orca: od:settings.mdx §Appearance「App Icon — cycle between Classic, Watercolor, and Blue」
- 検索: `set_app_icon|applicationIconImage|alternate_icon|app_icon` → 0 件

### H12 UI の言語 — 一部
- Orca: od:settings.mdx §Appearance **Language**。`os:src/renderer/src/i18n/locales/`（en / es / fr / ja / ko / zh）。PR #16455
- necoder: ja と en。既定は OS の言語に合わせ、settings.json の `locale` で上書きする `crates/i18n/src/i18n.rs:18-29,80`、`crates/necoder/src/main.rs:588-590`。設定画面に切替は無い

### H13 オンボーディング — 一部
- Orca: `os:src/renderer/src/components/setup-guide/SetupGuideModal.tsx`、`contextual-tours/ContextualTourOverlay.tsx`、`feature-tips/FeatureTipsModal.tsx`
- necoder:
  - 初回だけの「ようこそ」画面 `crates/settings/src/settings.rs:2018-2070`
  - 何も開いていない時の、キー付きの案内 4 行 `crates/workspace/src/workspace/chrome.rs:1712-1733`
  - 2 本目の Task で 1 回だけ出る Fleet の案内 `crates/workspace/src/workspace/fleet_view.rs:601-622`

### H14 初回起動時の設定の取り込み — 一部
- Orca: od:install.mdx §First launch「Offer to import `~/.claude`, `~/.codex`, and Ghostty terminal settings」
- necoder: 他のツールの設定から MCP サーバを見つける（既定は off）`crates/acp_client/src/mcp.rs:26-30,356-359`

### H15 テレメトリ — 無（never・思想の違い）
- Orca: od:telemetry.mdx（PostHog・opt-out）
- necoder: 「telemetry は採らない」`crates/workspace/src/crash.rs:3-5`、`FEATURES.md:139`
- 検索: `telemetry|posthog` → 1 件（その方針のコメント）

### H16 クラッシュ・ログ・フィードバック — 一部
- Orca: od:troubleshooting.mdx §Logs, §Reporting issues。PR #14823（Crashpad の minidump）、#10465（フィードバックに画像を添付）
- necoder:
  - panic をログに残す `crates/workspace/src/crash.rs:30-75`
  - 次回起動時にチップを出し、ログの抜粋を入れた Issue の下書きを開く `crates/workspace/src/crash.rs:129-161`、`crates/workspace/src/workspace/chrome.rs:2255-2273`
  - Finder から起動した時の stderr もログにする `crates/workspace/src/logging.rs:35-72`
  - minidump・ログを開くメニュー・スクショの添付は無い

### H17 更新のチャネル・changelog — 一部
- Orca: od:install.mdx §Updates（stable / RC / perf / ローカルビルド）。PR #11250（hourly）
- necoder:
  - stable だけを確認し、dmg を spctl で検証してから差し替える `crates/workspace/src/updater.rs:105-115,239-321`
  - Windows はリリースページを開くだけ、Linux は確認しない `crates/workspace/src/updater.rs:52-60`

### H18 配布の形 — 一部
- Orca: od:install.mdx（macOS arm64 / x64・Homebrew・Windows のインストーラ・Linux の AppImage / deb / rpm・AUR）
- necoder:
  - Apple Silicon 用の署名・公証済みの dmg と、Windows x64 の zip だけ `.github/workflows/release.yml:43,159-218,338-347`
  - 未対応の形は `docs/RELEASE.md:42-45` に「未」と書いてある

### H19 プラグイン — 無（Marketplace は never・WASM は later）
- Orca: od:settings.mdx §Plugins (Experimental)。PR #8549
- necoder: `FEATURES.md:98,100`
- 検索: `plugin|marketplace|extension_host|プラグイン` → 2 件。すべて無関係（Claude のプラグイン由来の MCP を無視する、というコメント `crates/acp_client/src/preset.rs:37,92`）

### H20 ペット — 一部
- Orca: `os:src/renderer/src/components/pet/PetOverlay.tsx`、`pet-models.ts`（同梱 3 体とカスタム画像・画面に浮かべる）
- necoder:
  - 猫のマスコットは、エージェントパネルごとに 1 匹いる（`MascotView` を持ち、パネル内に固定して描く）`crates/agent_panel/src/agent_panel.rs:1741,1942,7265-7336`
  - 状態は Idle・Typing・Think・Celebrate・Plead・Worry の 6 つ `crates/agent_panel/src/agent_panel.rs:9964-9979`
  - 状態はスレッドの状態から決める。承認待ちが長引くと Plead から Worry へ変わる `crates/agent_panel/src/agent_panel.rs:7076-7117`
  - 画面に浮かべること・キャラクターの切替・画像の差し替えは無い

### H21 Resource Manager — 一部
- Orca: od:settings.mdx §Appearance「**Resource Manager** (CPU/memory/sessions, daemon controls, workspace disk scans)」
- necoder: statusbar 中央の「N 実行・M 承認待ち・K 完了」と、アイドルの自動停止だけ `crates/workspace/src/workspace/chrome.rs:1816-1976`、`crates/settings_core/src/settings_core.rs:252-257`

### H22 外部アプリで開く（Open in） — 一部
- Orca: od:settings.mdx §General「Open in menu」
- necoder: 「既定のアプリで開く」「Finder で表示」`locales/ja.yml:558-559`、`crates/project/src/project.rs:2117-2142`

### H23 star・Discord への導線 — 無
- Orca: `os:src/renderer/src/components/StarNagCard.tsx`。README（Discord）
- necoder: About にサイトと GitHub へのリンクがあるだけ `crates/workspace/src/workspace/about.rs:15-16,270-272`
- 検索: `discord|star_nag|github_star` → 0 件

### H24 About に GPU の状態を出す — 無
- Orca: PR #11722
- necoder: GPUI は常に GPU で描くので、実害は小さい
- 検索: `gpu_specs|GpuSpecs|gpu_info` → 0 件

### H25 statusbar の項目の切替 — 無
- Orca: od:settings.mdx §Appearance「Status bar toggles」
- necoder: statusbar の項目は固定 `crates/workspace/src/workspace/chrome.rs:1979-2372`
- 検索: `statusbar_items|status_bar_items|show_statusbar` → 0 件

### H26 OS のショートカットとの衝突の警告 — 無
- Orca: PR #14734
- 検索: `mission.?control|shortcut_conflict` → 0 件

### H27 TCC の権限の案内 — 無
- Orca: `os:src/main/macos-tcc-prompt-notice.ts`、`os:src/main/macos-full-disk-access-status.ts`
- 検索: `FullDiskAccess|full.?disk|x-apple\.systempreferences|\bTCC\b` → 0 件

### H28 Windows / Linux の正式な対応 — 一部
- Orca: od:install.mdx
- necoder:
  - Windows は CI の必須ジョブで、zip も配っている `.github/workflows/ci.yml:26-35`、`.github/workflows/release.yml:243-363`
  - Linux の GUI はビルドできるかの確認だけ（continue-on-error）`.github/workflows/ci.yml:89-92`

### H29 アプリ内のヘルプ — 一部
- Orca: onorca.dev/docs（55 ページ）。od:troubleshooting.mdx「**Help → Open Logs**」
- necoder: ヘルプメニューに、ショートカット一覧（⌘K ⌘S）とバグ報告がある `crates/necoder/src/menus.rs:129-134`、`crates/workspace/src/workspace/shortcut_sheet.rs:1-9`

### H30 ファイル操作の undo — 一部
- Orca: `os:src/renderer/src/components/right-sidebar/fileExplorerUndoRedo.ts`（削除・作成・名前変更を 50 手まで undo / redo）
- necoder: ゴミ箱へ移すだけ `crates/project/src/project.rs:2089-2114`。MANUAL の記述と合っていない（§12-6）

---

## 10. 追加で確認したもの

### X1 Ports パネル（worktree ごとに起動中のポート・開く / 止める） — 無
- Orca:
  - `os:src/main/ports/local-workspace-port-scanner.ts`
  - `os:src/main/ports/advertised-url-watcher.ts`（端末の出力に出た URL から、どの worktree のものかを特定する）
  - `os:src/renderer/src/components/right-sidebar/local-workspace-ports-panel.tsx`
- 検索: `lsof|/proc/net/tcp|port_scan|listening_port|ポート` → 12 件。すべて無関係（「ビューポート」の中の「ポート」と、URL 解析のコメント `crates/ui/src/links.rs:368`）

### X2 PR のレビューコメントを AI に直させ、返信・resolve する — 無
- Orca:
  - `os:src/renderer/src/components/right-sidebar/pr-comment-fixing-reply-body.ts`。返信の定型文「Fixing. Will be in the next commit」を持つ
  - `os:src/renderer/src/components/right-sidebar/pr-comments-ai-launch-ack.ts`。CodeRabbit など bot のコメントにも返信する
- necoder: PR のビューそのものが無い（E19）
- 検索: `review_thread|pr_comment|coderabbit` → 0 件

---

## 11. 「無」の検索の再現記録

「無」と判定した 100 項目は、2026-09-25 に下のコマンドで検索を流し直した。
正規表現は `orca-gap-2026-09-searches.tsv`（項目 ID と正規表現をタブ区切りにしたもの）に置いてある。

```sh
while IFS=$'\t' read -r id re; do
  printf '%s\t%s\n' "$id" "$(rg -i -e "$re" crates locales relay/src relay/public relay/host -g '!**/node_modules/**' | wc -l | tr -d ' ')"
done < docs/research/orca-gap-2026-09-searches.tsv
```

結果:

- 70 項目は 0 件だった。
- 30 項目はヒットしたが、すべて開いて中身を確かめ、別の機能のものだった。C07 と C17 は範囲を `crates/terminal_view` に絞ると 0 件になる。

どの項目の判定も変わらなかった。件数は、この後にコードが変わると増減する。

- **A04** — 3 件（無関係: バグ報告の Issue URL（crates/workspace/src/crash.rs:14,157）と、gitlab の URL を弾くテスト（crates/project/src/project.rs:3063））: `\blinear\.app|\bjira\b|atlassian|gitlab|glab\b|gh issue|issue_url|branch_name_override`
- **A07** — 5 件（無関係: Rust の可視性についてのコメント（crates/workspace/src/workspace.rs:48）と、tree-sitter の descendant_of_kind（crates/lang/src/lang.rs:991-1057））: `parent_task|parent_worktree|parent_workspace|descendant|子 ?Task|親 ?Task`
- **A08** — 0 件: `shortcode|:rocket:|emoji_short`
- **A11** — 39 件（無関係: ACP の質問カードの「複数選択」（crates/acp_client/src/acp_client.rs:137-146,2247-2315、relay/public/app.mjs:250-251）ほか）: `multi_select|multiselect|selected_tasks|bulk_|一括|複数選択`
- **A16** — 0 件: `resource.?manager|cleanup_workspaces|disk_usage|dir_size|du -s|ディスク使用`
- **A18** — 0 件: `project.?group|multi.?repo|folder.?workspace|複数リポ`
- **A20** — 2 件（無関係: バグ報告の Issue URL（crates/workspace/src/crash.rs:14,157））: `gh issue|issue_number|issue_url|\bjira\b|linear\.app`
- **A22** — 0 件: `kanban|カンバン`
- **A23** — 0 件: `rename_branch|RenameBranch|"branch", "-m"|branch -m`
- **A24** — 0 件: `sparse.?checkout|sparse-checkout|--sparse`
- **A26** — 2 件（無関係: エディタ用の ctrl-shift-backspace の対応付け（crates/keymap_core/src/keymap_core.rs:447,668））: `shift-backspace|copy_task_name|copy_branch_name|CopyBranch|copy_name`
- **B12** — 0 件: `subagent|sub_agent|parent_tool|parentToolUse|サブエージェント|子エージェント`
- **B13** — 0 件: `teammate|agent.?team`
- **B14** — 2 件（無関係: MCP の発見のために CODEX_HOME を読むだけ（crates/acp_client/src/mcp.rs:368-372））: `account_switch|switch_account|hot.?swap|CODEX_HOME|CLAUDE_CONFIG_DIR|アカウント切替`
- **B15** — 7 件（無関係: relay の乱用対策のレート制限（relay/src/worker.ts:3-7,63,133）と、認証エラーの判定テスト（crates/acp_client/src/acp_client.rs:2434））: `rate.?limit|five_hour|seven_day|resets_at|utilization|レート制限|使用率`
- **B16** — 0 件: `pricing|cost_usd|estimated_cost|推定コスト|日別`
- **B25** — 0 件: `Event::Title|TitleChanged|ResetTitle|statusline|UserPromptSubmit`
- **B27** — 0 件: `thread/goal|goal_mode|"/goal"`
- **C02** — 0 件: `split_down|SplitDown|split_terminal|SplitTerminal|下に分割`
- **C05** — 0 件: `setsid|reattach|warm.?attach|pty.?daemon|nohup|tmux`
- **C06** — 1 件（無関係: テスト名 scroll_display_moves_into_scrollback（crates/terminal_view/src/terminal_view.rs:1517））: `scrollback`
- **C07** — 3 件（無関係: Agent パネルの会話内検索（locales/ja.yml:23、crates/agent_panel/src/search.rs:283）。crates/terminal_view に絞ると 0 件）: `RegexSearch|search_next|TerminalSearch|term_search|terminal_search`
- **C08** — 0 件: `ClipboardStore|osc.?52`
- **C09** — 0 件: `hyperlink|osc.?8\b|link_popover|リンクをコピー`
- **C10** — 0 件: `copy_context|Copy Context|コンテキストをコピー`
- **C12** — 0 件: `kitty|CSI.?u\b|DISAMBIGUATE|KeyboardModes|yen|¥|₩`
- **C14** — 0 件: `quick.?command|saved.?command|tasks\.json|定型コマンド`
- **C15** — 1 件（無関係: Markdown のテスト（crates/markdown/src/markdown.rs:739））: `sixel|iterm2|inline.?image|addon.?image|ImageProtocol`
- **C17** — 26 件（無関係: エクスプローラ・タブ・入力欄の D&D（crates/workspace/src/workspace/explorer_view.rs:122-137 ほか）。crates/terminal_view に絞ると 0 件）: `on_drop|ExternalPaths|DraggedFile`
- **C20** — 0 件: `osc.?133|shell.?integration|prompt_mark|FinalTerm|シェル統合`
- **C21** — 5 件（無関係: Fleet のセルを閉じた時のトースト（locales/ja.yml:373、crates/workspace/src/workspace/fleet_view.rs:633-641））: `confirm_quit|quit_confirm|still_running|終了しますか`
- **C22** — 14 件（無関係: 起動時にログインシェルから PATH を引き継ぐ処理（crates/workspace/src/shell_env.rs:63-206、crates/necoder/src/main.rs:483））: `default_shell|shell_args|terminal_shell|login_shell`
- **C24** — 0 件: `terminal_font|terminal_font_size|scrollback_lines|cursor_shape|cursor_style`
- **D01** — 0 件: `auto_?save|autosave|save_on_blur|自動保存`
- **D02** — 6 件（無関係: Chat の is_previewable（crates/chat_core/src/folder.rs:186 ほか）と、HTML プレビュータブについてのコメント（crates/workspace/src/workspace/notifications.rs:364））: `preview_tab|is_preview|provisional_tab|仮タブ|プレビュータブ`
- **D04** — 0 件: `review_note|AddReviewNote|レビュー注記`
- **D05** — 0 件: `mermaid|\.mmd\b`
- **D08** — 1 件（無関係: 配信時の Content-Type（crates/webview_view/src/sandbox.rs:147））: `\bcsv\b|\btsv\b|CsvView`
- **D09** — 0 件: `ipynb|jupyter|nbformat`
- **D11** — 0 件: `minimap|mini_map|ミニマップ`
- **D18** — 0 件: `find_in_folder|search_in_folder|FindInFolder|フォルダ内`
- **D20** — 0 件: `omnibox|tab_switcher|TabSwitcher`
- **D22** — 0 件: `settings_search|search_settings|設定を検索`
- **E05** — 0 件: `three.?way|conflict_marker|resolve_conflict|<<<<<<<|MERGE_HEAD`
- **E06** — 2 件（無関係: ⌘/ のための「行コメント」接頭辞（crates/lang/src/lang.rs:1269）と、テストのコメント（crates/necoder/tests/no_runtime_manifest_paths.rs:103））: `review_comment|ReviewComment|line_comment|diff_comment|DiffComment|行コメント`
- **E07** — 1 件（無関係: effect cycle についてのコメント（crates/workspace/src/workspace.rs:1624））: `review_notes|send_notes|SendNotes|まとめて送`
- **E08** — 0 件: `resolve_comment|unresolved_comment|comment_thread`
- **E12** — 0 件: `fix_with_ai|Fix with AI|FixWithAi`
- **E13** — 0 件: `force-with-lease|force_with_lease|force_push|ForcePush`
- **E14** — 0 件: `amend`
- **E15** — 0 件: `resolve_with_ai|Resolve with AI|ResolveWithAi`
- **E16** — 0 件: `action_recipe|prompt_template|commit_prompt_template`
- **E18** — 0 件: `stacked|gh pr create --base`
- **E19** — 0 件: `gh pr checks|pr_checks|pull_request_view|reaction`
- **E20** — 0 件: `auto.?merge|merge_queue|gh pr merge`
- **E21** — 0 件: `gh run|check_runs|statusCheckRollup|actions/runs`
- **E22** — 1 件（無関係: gitlab の URL を弾くテスト（crates/project/src/project.rs:3063））: `gitlab|glab\b|bitbucket|azure devops|dev\.azure|gitea`
- **E24** — 0 件: `gh issue|IssueDrawer|issue_list`
- **E25** — 0 件: `projectV2|gh project|github projects`
- **E26** — 0 件: `linear\.app|LINEAR_API|linear_api|LinearIssue`
- **E27** — 0 件: `\bjira\b|atlassian`
- **E28** — 5 件（無関係: リンク検出のテストの題材（crates/ui/src/links.rs:361-366）と、バグ報告の URL（crates/workspace/src/crash.rs:14,157,322））: `issue_url|github\.com/[^ ]*/issues`
- **E29** — 2 件（無関係: relay の乱用対策（relay/src/worker.ts:63,133））: `X-RateLimit|rate_limit|api_budget`
- **E30** — 0 件: `gh auth|gh_account|gh_host|GH_HOST`
- **E34** — 0 件: `gpgsign|gpg\.sign|commit\.sign|GIT_EDITOR|core\.editor`
- **F01** — 5 件（無関係: 更新の browser_download_url（crates/workspace/src/updater.rs:36,126,429-431））: `address.?bar|url_bar|UrlBar|load_url|go_back|アドレスバー`
- **F03** — 0 件: `cookie|passkey|webauthn|incognito`
- **F04** — 0 件: `user_agent|with_user_agent|device_profile|emulate|エミュレーション`
- **F05** — 0 件: `download_started|with_download|DownloadShelf`
- **F06** — 0 件: `design.?mode|getComputedStyle|computed.?style|element.?picker|outerHTML|with_ipc_handler|with_initialization_script`
- **F07** — 0 件: `page_annotation|annotate_page|ページ注釈`
- **F08** — 0 件: `link_routing|system_browser|SystemBrowser`
- **F09** — 0 件: `browser_goto|browser_click|browser_fill|"goto"|"fill"`
- **F10** — 0 件: `LocalForward|DynamicForward|socks`
- **F11** — 0 件: `share_artifact|publish_artifact|public.?link|公開リンク`
- **F12** — 5 件（無関係: Captain 席で Codex の computer-use MCP を止める設定とテスト（crates/acp_client/src/preset.rs:211-216、crates/agent_panel/src/seat.rs:360、crates/acp_client/src/mcp.rs:547、crates/agent_panel/src/agent_panel.rs:12092））: `AXUIElement|CGEvent|computer_use|computer-use|list_apps|UIAutomation`
- **F13** — 0 件: `simctl|simulator|emulator|\badb\b|xcrun`
- **F15** — 0 件: `search_engine|google\.com/search|duckduckgo|検索エンジン`
- **F16** — 0 件: `zoom_factor|set_zoom|page_zoom|with_zoom`
- **G08** — 0 件: `LocalForward|port.?forward|/proc/net/tcp|ポート転送|ポートフォワード`
- **G09** — 7 件（無関係: remote-server のバイナリ配備（crates/host/src/host.rs:2667-2709、crates/host/tests/remote_ssh_live.rs:13））: `sftp|\bscp\b|upload|アップロード`
- **G13** — 11 件（無関係: テスト用の Docker ホスト（crates/host/src/host.rs:2771、crates/host/tests/remote_ssh_live.rs:6-424）と Dockerfile のアイコン（crates/workspace/src/workspace.rs:1502-1513））: `devcontainer|orca\.yaml|vm_recipe|vercel|fly\.io|docker`
- **G18** — 2 件（無関係: relay の乱用対策（relay/src/worker.ts:63,133））: `rate_limit|usage_limit|create_workspace|edit_host`
- **G24** — 1 件（無関係: entitlements の automation.apple-events（crates/necoder/resources/necoder.entitlements:9））: `\bcron\b|rrule|automation|定期実行|予約実行`
- **G25** — 0 件: `SKILL\.md|npx skills|\.claude/skills|skills install`
- **G27** — 0 件: `account add|add_account|login_account`
- **H03** — 0 件: `dock_badge|badge_label|NSDockTile|dockTile|set_badge|request_user_attention`
- **H06** — 0 件: `NSStatusItem|NSStatusBar|status_item|system_tray|\btray\b`
- **H07** — 4 件（無関係: Permissions-Policy で microphone=() を禁止する設定（relay/public/_headers:6）と「マイクロ」秒・ベンチ（crates/editor_core/examples/bench_editor.rs:1 ほか））: `dictation|speech|whisper|parakeet|microphone|音声入力|マイク`
- **H08** — 0 件: `settings_search|search_settings|設定を検索`
- **H11** — 0 件: `set_app_icon|applicationIconImage|alternate_icon|app_icon`
- **H15** — 1 件（無関係: 「telemetry は採らない」というコメント（crates/workspace/src/crash.rs:3））: `telemetry|posthog`
- **H19** — 2 件（無関係: Claude のプラグイン由来の MCP を無視する、というコメント（crates/acp_client/src/preset.rs:37,92））: `plugin|marketplace|extension_host|プラグイン`
- **H23** — 0 件: `discord|star_nag|github_star`
- **H24** — 0 件: `gpu_specs|GpuSpecs|gpu_info`
- **H25** — 0 件: `statusbar_items|status_bar_items|show_statusbar`
- **H26** — 0 件: `mission.?control|shortcut_conflict`
- **H27** — 0 件: `FullDiskAccess|full.?disk|x-apple\.systempreferences|\bTCC\b`
- **X1** — 12 件（無関係: 「ビューポート」の中の「ポート」と、URL 解析のコメント（crates/ui/src/links.rs:368））: `lsof|/proc/net/tcp|port_scan|listening_port|ポート`
- **X2** — 0 件: `review_thread|pr_comment|coderabbit`

---

## 12. 不具合と文書のずれ（抜粋つき）

[`orca-gap-2026-09.md`](./orca-gap-2026-09.md) の §3 に挙げたものの証拠。
抜粋はすべて 2026-09-25 の作業ツリーから、行番号ごと写した。

### 12-1. パネルからのコミットで pre-commit / commit-msg フックが走らない

すべての git 呼び出しは `run_git` を通る。`run_git` は、信頼できない repo を開いた時の対策として、毎回フックを無効にする。
コミットも同じ関数を通るので、利用者が自分で押したコミットでもフックが走らない。
push も同じなので、pre-push も走らない。
Orca はフックを普通に走らせる（od:review/commit-push.mdx「Pre-commit hooks from the repo run as usual」）。

```text
crates/project/src/project.rs
423:     let mut hardened: Vec<String> = vec![
424:         "-c".into(),
425:         "core.fsmonitor=false".into(),
426:         "-c".into(),
427:         "core.hooksPath=/dev/null".into(),
...
951: pub fn commit_on(host: &dyn Host, dir: &Path, message: &str) -> Result<()> {
...
954:         run_git(host, dir, ["commit", "-m", message]).context("git commit の実行に失敗")?;
```

### 12-2. ソース管理パネルの変更行を押しても diff が開かない

diff を開くイベント（`OpenDiff`）は、型と受け側はあるが、送る側がどこにも無い。
変更行の要素にはクリックのハンドラが無い。行の中で `on_mouse_down` を持つのは ± ボタンだけ（`crates/workspace/src/workspace/git_view.rs:481-537`）。
`rg "GitPanelEvent::OpenDiff|emit\(git_ui::GitPanelEvent" crates` でヒットするのは、受け側の 1 か所だけ。

```text
crates/git_ui/src/lib.rs
25: pub enum GitPanelEvent {
26:     RepositoryChanged,
27:     OpenDiff(PathBuf),
crates/workspace/src/workspace/panels.rs
91:             git_ui::GitPanelEvent::OpenDiff(path) => {
92:                 self.project_sessions.sessions[session_index].pending_open_git_diff =
93:                     Some(path.clone());
```

### 12-3. Fleet の「変更」から開く diff が、エージェントがコミット済みだと空になる

- 一覧は Task の base を起点に作る `crates/workspace/src/workspace/project_session.rs:1431-1452`。
- 行を押した時に開く diff は、HEAD と比べる `crates/workspace/src/workspace/editor_area/diff.rs:24-58`。
- エージェントが変更をコミットしていると、一覧には +N −M が出るのに、押すと差分なしとして stderr に書くだけで、何も開かない。

```text
crates/workspace/src/workspace/fleet_stage.rs
439:                 .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
440:                     this.switch_project(session, window, cx);
441:                     this.chrome.fleet_mode = false;
crates/workspace/src/workspace/editor_area/diff.rs
53:             let Some(diff_text) = diff_text else {
54:                 eprintln!("diff: 差分なし（HEAD と同一） {}", path.display());
55:                 return;
```

### 12-4. コミット・push・pull の失敗が画面に出ない

```text
crates/workspace/src/workspace/git_controller.rs
366:                     Err(error) => eprintln!("コミットに失敗: {error:#}"),
...
476:                     Err(error) => {
477:                         eprintln!(
478:                             "{} に失敗: {error:#}",
479:                             if is_push { "push" } else { "pull" }
```

### 12-5. コミットメッセージ欄で ⌘V が効かない

- メッセージ欄は EditorView ではなく、キー入力を 1 つずつ拾う自前の入力。⌘ / ⌃ / fn 付きのキーを捨てる。
- この 2 ファイルには Paste のアクションも無い（`rg "on_action|Paste|paste" crates/workspace/src/workspace/git_controller.rs crates/workspace/src/workspace/git_view.rs` → 0 件）。
- input handler も無いので、日本語 IME で入力できるかは怪しい（実機では確かめていない）。

```text
crates/workspace/src/workspace/git_controller.rs
306:             _ => {
307:                 let modifiers = event.keystroke.modifiers;
308:                 if modifiers.platform || modifiers.control || modifiers.function {
309:                     return;
```

### 12-6. MANUAL の「ファイル操作は取り消せる」が実装と合わない

- エクスプローラのファイル操作に undo は無い。ゴミ箱へ移すだけ `crates/project/src/project.rs:2089-2114`。
- keymap にはエクスプローラ用の割当が無い（`rg -i explorer crates/keymap_core/src/keymap_core.rs` → 0 件）。
- この行はコミット済みの MANUAL（`git show HEAD:docs/MANUAL.md`）にもある。

```text
docs/MANUAL.md
131: - ファイル操作は取り消せる
```

### 12-7. MANUAL の `density`（compact / cozy）が効かない

`density` を参照しているのは `crates/settings_core/src/settings_core.rs` の中（定義・既定値・テスト）だけで、描画からは参照されていない（`rg -l density crates` → この 1 ファイル）。

```text
docs/MANUAL.md
448: | `density` | compact | 行の密度（compact / cozy） |
crates/settings_core/src/settings_core.rs
173:     pub density: Density,
```

### 12-8. ACP のスラッシュコマンド一覧と、会話名の更新を捨てている

- SessionUpdate の match は、Plan の次が `_ => {}` になっている。そのため `AvailableCommandsUpdate`（エージェントが提供する `/` コマンドと skills の一覧）と、会話名の更新が落ちる。
- `rg -i "AvailableCommand|available_commands" crates` → 0 件。

```text
crates/acp_client/src/acp_client.rs
1970:                                             event_tx.unbounded_send(AgentEvent::Plan(items)).ok();
1971:                                         }
1972:                                         _ => {}
```

### 12-9. 使われていないコード

| 対象 | 確かめ方と結果 |
|---|---|
| 設定 `fleet_goal`（`crates/settings_core/src/settings_core.rs:213`） | `rg fleet_goal crates` → 定義のファイル以外で 0 件 |
| `Storage::token_ledger()`（`crates/storage/src/storage.rs:1637`） | 呼んでいるのはテスト（同じファイルの 3011 行）だけ |
| `Picker::set_query_action`（`crates/ui/src/ui.rs:203`） | `rg set_query_action crates` → 定義の 1 件だけ |

---

## 13. 引用の機械検査

この文書と本体の necoder 側の引用（`path:行` / `path:開始-終了` / `path:行,行`）を全部取り出し、次の 2 つを確かめた。

- ファイルが存在すること。
- 行番号がファイルの行数の範囲に入っていること。


**結果（2026-09-25）**

| 文書 | 引用 | ファイル | ファイルが無い / 範囲外 |
|---|---:|---:|---:|
| `orca-gap-2026-09.md` | 68 件 | 34 本 | 0 件 |
| `orca-gap-2026-09-evidence.md` | 504 件 | 98 本 | 0 件 |

**中身の確認**:
- 行番号の範囲だけでは、その行が主張の根拠になっているかまでは分からない。そこで、この文書の引用を全件、「主張の文」と「引用先の行」を並べて出力し、目で照らし合わせた。
- 見つかった誤りは 1 件だった。B29 で「エージェント別のミュート」の根拠を、`mark_done_seen`（未確認の Done を確認済みにする処理）と取り違えていたので、`toggle_thread_mute` に直した。
- H20 は誤りではなかったが、根拠を「置き場所」と「状態の種類」に分けて書き直した。
- ほかに、引用の範囲の続きまで開いて確かめたものがある。例: E05 の `merge --abort` は `crates/project/src/project.rs:854` にある。

**限界**:
- 行番号は、2026-09-25 の作業ツリー基準。コードが変わればずれる。ずれた時は、書いてある関数名や型名（`toggle_thread_mute`・`run_git` など）で追い直し、§11 の検索を流し直すこと。
- 「無」の判定は「この検索では見つからなかった」という意味で、存在しないことの証明ではない。ただし 100 項目すべてで、ヒットの中身まで確かめている。

**検査スクリプト**（リポジトリのルートで実行する）:

```sh
python3 - <<'PY'
import re, pathlib
pat = re.compile(r"(?<![\w/:.-])((?:crates|relay|locales|docs|scripts|\.github)/[A-Za-z0-9_./-]+|FEATURES\.md):(\d+(?:-\d+)?(?:,\d+(?:-\d+)?)*)")
for doc in ["docs/research/orca-gap-2026-09.md", "docs/research/orca-gap-2026-09-evidence.md"]:
    total, bad = 0, []
    for m in pat.finditer(pathlib.Path(doc).read_text()):
        total += 1
        path, spec = m.group(1), m.group(2)
        p = pathlib.Path(path)
        if not p.is_file():
            bad.append((path, spec, "missing"))
            continue
        n = sum(1 for _ in p.open(encoding="utf-8", errors="replace"))
        for part in spec.split(","):
            a, _, b = part.partition("-")
            a = int(a)
            b = int(b) if b else a
            if not (1 <= a <= b <= n):
                bad.append((path, spec, f"file has {n} lines"))
                break
    print(doc, total, "citations,", len(bad), "problems", bad)
PY
```
