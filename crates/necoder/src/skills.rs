//! `necoder skills …` — エージェント向けの使い方（`get`）と、その入口の SKILL.md の配布（`install`）・
//! 一覧（`list`）。
//!
//! 置き場に置くのは入口だけ（`agent_skills::stub_skill_md`）で、使い方の本文はこの `get` が出す。
//! 本文のコマンド一覧は `fleet::FLEET_COMMANDS`・`terminal::TERMINAL_COMMANDS`・`mcp::tool_schemas`・
//! `TaskPhase::ALL`・`cli_shim::PASSTHROUGH_SUBCOMMANDS` から組み立てる＝コマンドを足せば本文にも出て、
//! 無いコマンドやフラグは本文に出ない（手で書いた説明が実装とずれない）。
//! 手法の出典: stablyai/orca@646e9a5 の `docs/site/content/docs/cli/skills.mdx`（MIT。
//! `orca skills get` が版に合った本文を出す形）。文面は necoder の実装から独立に書いた。

use crate::fleet::{ACTIVITIES, FLEET_COMMANDS};
use crate::terminal::TERMINAL_COMMANDS;
use agent_skills::{InstallOutcome, SkillAgent, SkillScan, SkillScope, StubState};
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use storage::TaskPhase;

const USAGE: &str = "使い方: necoder skills <get [--full] | install [--agent claude|codex|all] [--project <dir>] [--force] | list [--project <dir>]>";

/// `necoder skills …` を処理したら true（GUI は開かない）。失敗は終了コード 1。
pub(crate) fn run_cli() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("skills") {
        return false;
    }
    if let Err(error) = run(&args[1..]) {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
    true
}

/// `skills` に続く引数を処理する（古い `ne` シムからは `necoder cli skills …` 経由で来る）。
pub(crate) fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("get") => {
            let full = parse_get_options(&args[1..])?;
            let binary = cli_shim::current_binary()?;
            print!("{}", guide(full, &binary));
            Ok(())
        }
        Some("install") => run_install(&args[1..]),
        Some("list") => run_list(&args[1..]),
        _ => anyhow::bail!(USAGE),
    }
}

// ---------------------------------------------------------------------------
// 引数
// ---------------------------------------------------------------------------

fn parse_get_options(args: &[String]) -> Result<bool> {
    match args {
        [] => Ok(false),
        [flag] if flag == "--full" => Ok(true),
        _ => anyhow::bail!("知らない引数: {}\n{USAGE}", args.join(" ")),
    }
}

/// `skills install` の引数。
#[derive(Debug, Default, PartialEq, Eq)]
struct InstallOptions {
    /// `None` = `--agent` 省略（設定フォルダがあるエージェントへ）。
    agents: Option<Vec<SkillAgent>>,
    project: Option<PathBuf>,
    force: bool,
}

/// `--flag value` と `--flag=value` の両方を受ける小さな読み手。
struct Flags<'a> {
    args: &'a [String],
    index: usize,
}

impl<'a> Flags<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, index: 0 }
    }

    /// 次の `(flag, 値が = で付いていればその値)`。
    fn next_flag(&mut self) -> Option<(&'a str, Option<&'a str>)> {
        let argument = self.args.get(self.index)?.as_str();
        self.index += 1;
        Some(match argument.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (argument, None),
        })
    }

    /// フラグの値（`=` で付いていなければ次の引数）。
    fn value(&mut self, flag: &str, inline: Option<&'a str>) -> Result<&'a str> {
        if let Some(value) = inline {
            return Ok(value);
        }
        let value = self
            .args
            .get(self.index)
            .with_context(|| format!("{flag} の値がありません\n{USAGE}"))?;
        self.index += 1;
        Ok(value.as_str())
    }
}

fn parse_install_options(args: &[String]) -> Result<InstallOptions> {
    let mut options = InstallOptions::default();
    let mut flags = Flags::new(args);
    while let Some((flag, inline)) = flags.next_flag() {
        match flag {
            "--agent" => {
                let value = flags.value(flag, inline)?;
                let agents = agent_skills::parse_agents(value).with_context(|| {
                    format!("--agent は claude / codex / all のどれか（{value} は知らない）")
                })?;
                options.agents = Some(agents);
            }
            "--project" => options.project = Some(PathBuf::from(flags.value(flag, inline)?)),
            "--force" if inline.is_none() => options.force = true,
            _ => anyhow::bail!("知らない引数: {flag}\n{USAGE}"),
        }
    }
    Ok(options)
}

fn parse_list_options(args: &[String]) -> Result<Option<PathBuf>> {
    let mut project = None;
    let mut flags = Flags::new(args);
    while let Some((flag, inline)) = flags.next_flag() {
        match flag {
            "--project" => project = Some(PathBuf::from(flags.value(flag, inline)?)),
            _ => anyhow::bail!("知らない引数: {flag}\n{USAGE}"),
        }
    }
    Ok(project)
}

/// 相対パスは今いるフォルダ基準で絶対にする（表示と書き込みの両方で同じ場所を指すように）。
fn absolute_directory(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("カレントディレクトリが分かりません")?
            .join(path)
    };
    anyhow::ensure!(
        absolute.is_dir(),
        "フォルダがありません: {}",
        absolute.display()
    );
    Ok(paths::canonicalize_or_keep(&absolute))
}

fn home() -> Result<PathBuf> {
    agent_skills::default_home().context("ホームディレクトリが分かりません")
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

/// 1 つの置き場への設置の結果。
#[derive(Debug, PartialEq, Eq)]
struct InstallReport {
    agent: SkillAgent,
    path: PathBuf,
    outcome: InstallOutcome,
}

/// 置き場ごとに入口の SKILL.md を置く。ホームは引数（テストは一時ディレクトリを渡す）。
fn install(
    home: &Path,
    options: &InstallOptions,
    project: Option<&Path>,
    desired: &str,
) -> Result<Vec<InstallReport>> {
    let agents = options
        .agents
        .clone()
        .unwrap_or_else(|| agent_skills::default_agents(home));
    agents
        .into_iter()
        .map(|agent| {
            let skills_dir = match project {
                Some(project) => agent.project_skills_dir(project),
                None => agent.user_skills_dir(home),
            };
            let path = agent_skills::stub_path(&skills_dir);
            let outcome = agent_skills::install_stub(&path, desired, options.force)?;
            Ok(InstallReport {
                agent,
                path,
                outcome,
            })
        })
        .collect()
}

fn run_install(args: &[String]) -> Result<()> {
    let options = parse_install_options(args)?;
    let home = home()?;
    let project = options
        .project
        .as_deref()
        .map(absolute_directory)
        .transpose()?;
    let desired = agent_skills::stub_skill_md(&cli_shim::current_binary()?);
    let reports = install(&home, &options, project.as_deref(), &desired)?;
    let mut refused = 0;
    for report in &reports {
        let label = report.agent.label();
        let path = agent_skills::display_path(&report.path, &home);
        match report.outcome {
            InstallOutcome::Created => println!("{label}: 置きました {path}"),
            InstallOutcome::Replaced(state) => {
                println!("{label}: 上書きしました {path}（{}）", state_text(state))
            }
            InstallOutcome::Unchanged => println!("{label}: 最新です {path}"),
            InstallOutcome::Refused(state) => {
                refused += 1;
                eprintln!(
                    "{label}: 中身の違う SKILL.md があるので書きませんでした {path}（{}）",
                    state_text(state)
                );
            }
        }
    }
    anyhow::ensure!(
        refused == 0,
        "上書きするなら --force を付けてもう一度実行してください"
    );
    Ok(())
}

/// 上書きの判断に使う状態の説明（`Missing` / `Current` は上書きの話にならない）。
fn state_text(state: StubState) -> &'static str {
    match state {
        StubState::Outdated => "necoder が書いた古い版",
        StubState::Foreign => "necoder が書いたものではない",
        StubState::Missing => "無い",
        StubState::Current => "最新",
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn run_list(args: &[String]) -> Result<()> {
    let project = match parse_list_options(args)? {
        Some(project) => absolute_directory(&project)?,
        None => std::env::current_dir().context("カレントディレクトリが分かりません")?,
    };
    let home = home()?;
    let desired = agent_skills::stub_skill_md(&cli_shim::current_binary()?);
    let roots = agent_skills::skill_roots(&home, Some(&project));
    let scan = agent_skills::scan(&roots);
    if scan.skills.is_empty() {
        println!("skill は見つかりませんでした。探した場所:");
        for root in &roots {
            println!("  {}", agent_skills::display_path(&root.path, &home));
        }
    } else {
        print!("{}", format_list(&scan, &home, &desired));
    }
    for skipped in &scan.skipped {
        eprintln!(
            "読めないので一覧から外しました: {}（{}）",
            agent_skills::display_path(&skipped.path, &home),
            skipped.reason
        );
    }
    Ok(())
}

/// CLI の一覧の「どのエージェントが読むか」。
fn scope_label(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::ClaudeUser => "Claude Code",
        SkillScope::CodexUser => "Codex",
        SkillScope::SharedUser => "共通（~/.agents）",
        SkillScope::ClaudeProject => "Claude Code・プロジェクト",
        SkillScope::SharedProject => "共通・プロジェクト",
    }
}

/// 1 skill = 3 行（名前（エージェント）/ 説明 / 場所）。necoder の skill には最新かどうかを添える。
fn format_list(scan: &SkillScan, home: &Path, desired: &str) -> String {
    let mut out = String::new();
    for skill in &scan.skills {
        let note = if skill.name == agent_skills::NECODER_SKILL_NAME {
            match agent_skills::read_stub_state(&skill.path, desired) {
                Ok(StubState::Current) => " · 最新".to_string(),
                Ok(StubState::Outdated) => {
                    " · 古い版（`necoder skills install --force` で更新）".to_string()
                }
                Ok(StubState::Foreign) => " · necoder が書いたものではない".to_string(),
                Ok(StubState::Missing) => String::new(),
                Err(error) => format!(" · 読めない（{error}）"),
            }
        } else {
            String::new()
        };
        out.push_str(&format!(
            "{}（{}）{note}\n  {}\n  {}\n",
            skill.name,
            scope_label(skill.scope),
            skill.description,
            agent_skills::display_path(&skill.path, home)
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// get（版に合った使い方の本文）
// ---------------------------------------------------------------------------

/// phase の意味。`match` を網羅にしてある＝phase を足すと、ここに書くまでビルドが通らない。
fn phase_meaning(phase: TaskPhase) -> &'static str {
    match phase {
        TaskPhase::Planned => "作っただけ（エージェントはまだ動いていない）",
        TaskPhase::Working => "エージェントが作業中",
        TaskPhase::Blocked => "承認・質問の待ち（人間の判断が要る）",
        TaskPhase::ReviewReady => "ターンが終わり、レビュー待ち",
        TaskPhase::ChangesRequested => "review で衝突が見つかった・直しを頼まれた",
        TaskPhase::MergeReady => "Conflict Radar が clean。人間の Integrate 待ち",
        TaskPhase::Integrating => "統合中（人間が Integrate を押した）",
        TaskPhase::Integrated => "統合済み",
        TaskPhase::Failed => "失敗（準備スクリプトの失敗を含む）",
        TaskPhase::Archived => "終了（worktree はディスクに残る）",
    }
}

/// コマンド一覧の 1 行（`ne fleet <name> <arguments>` + 要 GUI・人間の操作の印 + 要旨）。
fn fleet_command_line(command: &crate::fleet::FleetCommand) -> String {
    let mut marks = String::new();
    if command.needs_gui {
        marks.push_str("（要 GUI）");
    }
    if command.human_only {
        marks.push_str("（人間の操作・エージェントは実行しない）");
    }
    format!(
        "- `ne fleet {} {}`{marks} — {}\n",
        command.name, command.arguments, command.summary
    )
}

/// MCP の道具を `名前(必須, 任意?) — 説明` の 1 行ずつにする（`tools/list` と同じ定義から）。
fn mcp_tool_lines() -> Vec<String> {
    let schemas = crate::mcp::tool_schemas();
    let Some(tools) = schemas.as_array() else {
        return Vec::new();
    };
    tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?;
            let description = tool.get("description")?.as_str()?;
            let schema = tool.get("inputSchema");
            let required: Vec<&str> = schema
                .and_then(|schema| schema.get("required"))
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let parameters: Vec<String> = schema
                .and_then(|schema| schema.get("properties"))
                .and_then(Value::as_object)
                .map(|properties| {
                    properties
                        .keys()
                        .map(|key| {
                            if required.contains(&key.as_str()) {
                                key.clone()
                            } else {
                                format!("{key}?")
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let human_only = if name == "fleet_integrate_task" {
                "（人間の操作・エージェントは呼ばない）"
            } else {
                ""
            };
            Some(format!(
                "- `{name}({})`{human_only} — {description}",
                parameters.join(", ")
            ))
        })
        .collect()
}

/// `necoder skills get [--full]` の本文。`binary` = この本文を出している necoder の実体。
fn guide(full: bool, binary: &Path) -> String {
    let binary = binary.display();
    let version = env!("CARGO_PKG_VERSION");
    let mut out = String::new();
    out.push_str(&format!(
        "# necoder の使い方（エージェント向け）

necoder {version} · この本文は {binary} が、この版の実装から出している。
版が変わると中身も変わるので、覚えずに毎回ここを読む。ここに無いコマンドやフラグを、記憶や推測で作らない。
{more}

## 呼び方

例は `ne` で書く。`ne` コマンドが無い時は `\"{binary}\"` に置き換える（`ne fleet list .` と `\"{binary}\" fleet list .` は同じ）。

## necoder と Fleet

- necoder はエディタで、1 つのリポジトリで複数のエージェントを並走させる **Fleet** を持つ
- **Task** = 1 つの依頼 = `task/*` ブランチ 1 本 = linked worktree 1 つ。進み具合（phase）は台帳が持つ
- **統合先** = main の worktree。ここへ書き込むのは人間の Integrate だけ
- **Captain** = 目標を Task に分けて采配する、任命制のスレッド（コードは書かない）

## 守ること

- **Integrate は人間が押す。** `ne fleet integrate` と MCP の `fleet_integrate_task` を実行しない。できるのは `ne fleet review` で merge_ready にして、人間に Integrate を頼むところまで
- 承認待ち（ツールの許可）に、人間の代わりに答えない
- Task の worktree とブランチを消さない（`git worktree remove` / `git branch -D` をしない）。片付けは人間が Task カードの ⋯ から行う
- Task を作る時は `git worktree add` ではなく `ne fleet create` を使う（台帳に載らない worktree は Fleet に出ない）
- 頼まれない限り `ne config set` で設定を変えない
- Captain として動いている時は、自分でコードを書かない・ファイルを編集しない（指示と采配だけ）

## Task の指し方（`<task>`）

- id（`ne fleet create` の出力の `id`）か `id:<id>`
- `branch:<ブランチ>`（`task/` は省ける）・`name:<名前>`（Task の名前と完全一致）
- `active` = GUI で選択中の Task（要 GUI）
- 前置きなしは id → ブランチ → 名前の順に探す。ブランチと名前は、今いるフォルダのリポジトリの Task だけから探す。1 つに絞れなければ候補を出して失敗する
- 統合先（main）は id で指した時だけ選べる

## よく使う流れ

1. 現況を読む: `ne fleet list .` と `ne fleet digest <task>`
2. Task を切る: `ne fleet create . \"<名前>\"`（出力の JSON の `id` か、その名前で以後の `<task>` を指す）
3. エージェントを起こす: `ne fleet spawn-agent <task> [agent] [prompt...]`（要 GUI）
4. 待つ: `ne fleet wait <task> review_ready`（phase）・`ne fleet wait <task> idle`（activity）
5. レビュー: 統合先（main の worktree）で `ne fleet review <task>` → merge_ready なら人間に Integrate を頼む

- 自分の Task（今いる worktree）はブランチで指せる: `ne fleet status \"branch:$(git branch --show-current)\" review_ready \"<要約>\"`
- GUI で動いているエージェントの working / blocked / review_ready は necoder が自動で付ける。それ以外を伝える時は `ne fleet status <task> <phase> \"<要約>\"`

## コマンド

",
        more = if full {
            "（`--full`: 全コマンドの詳細つき）"
        } else {
            "全コマンドの詳細は `ne skills get --full`。"
        },
    ));
    for command in FLEET_COMMANDS {
        out.push_str(&fleet_command_line(command));
    }
    for command in TERMINAL_COMMANDS {
        out.push_str(&format!(
            "- `ne terminal {} {}`（要 GUI） — {}\n",
            command.name, command.arguments, command.summary
        ));
    }
    out.push_str(&format!(
        "- `ne {open}` — 起動中の necoder でファイル・フォルダを開く（`:<line>` でその行へ。引数なしは前面に出すだけ）
- `ne {diff}` — 2 つのファイルの diff を、起動中の necoder の diff タブで開く（要 GUI）
- `ne skills get [--full]` — この本文
- `ne mcp [root]` — MCP サーバ（stdio。道具は `--full`）

（要 GUI）= necoder が起動していないと失敗する。`[省略可]` `<必須>`。`...` の付いた引数だけは残りの引数を空白でつないで 1 つにする。
それ以外（`[title]` `[summary]` など）は 1 つの引数なので、空白を含むなら引用符で囲む。

## 出力と失敗

- fleet の結果は整形した JSON（標準出力）。Task の主な項目: `id` `root` `branch` `title` `phase` `result_summary` `depends_on`
- 失敗すると標準エラーに理由を出し、終了コード 1
",
        open = crate::cli::OPEN_ARGUMENTS,
        diff = crate::cli::DIFF_ARGUMENTS,
    ));
    if !full {
        return out;
    }

    out.push_str("\n## phase（台帳の進み具合）\n\n");
    for phase in TaskPhase::ALL {
        out.push_str(&format!(
            "- `{}` — {}\n",
            phase.as_str(),
            phase_meaning(phase)
        ));
    }
    out.push_str(
        "\n流れ: planned → working ⇄ blocked → review_ready → merge_ready → integrating → integrated。\
         review で衝突があれば changes_requested → working に戻る。\n",
    );
    out.push_str(&format!(
        "\n## activity（GUI の今の動き。`ne fleet wait` で待てる）\n\n{}\n\n\
         Task に複数のスレッドがあれば、いちばん注意の要るもの（blocked > working > done・interrupted > idle）で判定する。\
         activity の待ちは GUI が Task を開いている時だけ効く。\n",
        ACTIVITIES
            .iter()
            .map(|activity| format!("`{activity}`"))
            .collect::<Vec<_>>()
            .join(" / ")
    ));
    out.push_str(
        "
## 各コマンドの補足

- `create`: root を省くと今いるフォルダ（その HEAD から切る）。title を省くと `Task`。`.necoder/worktree-setup.sh` があれば作成直後に 1 回走り、失敗すると phase は `failed`
- `list`: root のリポジトリの Task だけ（統合先と Task の worktree のどちらから打っても同じ一覧）
- `review` / `integrate`: `[integration-root]` を省くと今いるフォルダを統合先とみなす。Task の worktree の中から打つ時は、統合先（main の worktree）のパスを渡す
- `wait` / `wait-deps`: 期限（既定 600 秒）を過ぎると失敗する。phase として読める値なら phase を、そうでなければ activity を待つ
- `digest`: GUI が Task を開いていなければ、台帳の分（phase・result_summary など）だけを返し、`gui` に理由が入る
- `send`: GUI で Task が開いていなければ失敗する（先に `spawn-agent`）
- `events`: 返った最後の `id` を覚えておき、次は `ne fleet events <id>` で差分だけ読む

## 端末（`ne terminal …`）

- 対象は、GUI の下ドックのタブの端末と、Fleet の Task カードに置いた端末。`<terminal>` は `ne terminal list` の `handle`（`t<番号>`）か `active`（選択中のプロジェクトの下ドックで前に出ている端末）
- ハンドルは GUI が動いている間だけ有効。GUI を再起動したり端末を閉じたりしたら `list` で取り直す
- `read` は今の画面を読む（人がスクロールで遡っていても関係ない）。送る前に読んで、端末が何を待っているかを確かめる
- `send` は GUI の設定「CLI から端末へ入力を送る」が on の時だけ効く（既定 off）。off の時は失敗するので、人に頼む。`--` の後ろは全部文字として送る
- `wait` は出力が止まるまで待つ。シェルが終わった端末はすぐ返る（`exited: true`）
",
    );
    out.push_str(
        "\n## MCP サーバ\n\n`ne mcp [root]` は stdio の MCP サーバ（改行区切りの JSON-RPC）。root を省くと今いるフォルダ。\
         エージェントへの登録は人間が設定で行う。道具（`?` = 省略可）:\n\n",
    );
    for line in mcp_tool_lines() {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!(
        "
## そのほか

- `ne <{passthrough}> …` は `\"{binary}\" <同じもの> …` と同じ
- `ne config list` / `ne config get <key>` / `ne config set <key> <value>` — 設定（settings.json）の読み書き
- `ne skills install [--agent claude|codex|all] [--project <dir>] [--force]` — 入口の SKILL.md を置く（中身の違うファイルは `--force` の時だけ上書き）
- `ne skills list [--project <dir>]` — 見つかった skill の一覧（`~/.claude/skills` `~/.codex/skills` `~/.agents/skills` とプロジェクトの `.claude/skills` `.agents/skills`）
- 人間向け（エージェントは使わない）: `\"{binary}\" install-cli` / `uninstall-cli`（`ne` の設置）、`ne remote …`（スマホ連携）
",
        passthrough = cli_shim::PASSTHROUGH_SUBCOMMANDS.join("|"),
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINARY: &str = "/Applications/necoder.app/Contents/MacOS/necoder";

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// テスト毎の一時ホーム。**本物の `~/.claude` / `~/.codex` には触れない**。
    fn scratch_home(tag: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("necoder_skills_cli_{}_{}", tag, std::process::id()));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).expect("前回の残りを消せる");
        }
        std::fs::create_dir_all(&directory).expect("一時ホームを作れる");
        directory
    }

    #[test]
    fn guide_lists_every_real_command_and_the_human_gate() {
        let short = guide(false, Path::new(BINARY));
        for command in FLEET_COMMANDS {
            assert!(
                short.contains(&format!("ne fleet {} {}", command.name, command.arguments)),
                "{} が本文に無い",
                command.name
            );
        }
        assert!(short.contains("**Integrate は人間が押す。**"));
        assert!(short.contains("`ne fleet integrate <task> [integration-root]`（人間の操作"));
        for command in TERMINAL_COMMANDS {
            assert!(
                short.contains(&format!(
                    "ne terminal {} {}",
                    command.name, command.arguments
                )),
                "terminal {} が本文に無い",
                command.name
            );
        }
        assert!(short.contains(&format!("ne {}", crate::cli::DIFF_ARGUMENTS)));
        assert!(short.contains("## Task の指し方"));
        assert!(short.contains(&format!("necoder {}", env!("CARGO_PKG_VERSION"))));
        assert!(short.contains(BINARY));
        // 短い版は詳細を持たない。
        assert!(!short.contains("## phase"));
    }

    #[test]
    fn full_guide_covers_phases_activities_and_mcp_tools() {
        let full = guide(true, Path::new(BINARY));
        assert!(full.starts_with("# necoder の使い方"));
        for phase in TaskPhase::ALL {
            assert!(full.contains(&format!("`{}`", phase.as_str())));
        }
        for activity in ACTIVITIES {
            assert!(full.contains(&format!("`{activity}`")));
        }
        let schemas = crate::mcp::tool_schemas();
        let tools = schemas.as_array().expect("道具の一覧は配列");
        assert_eq!(mcp_tool_lines().len(), tools.len());
        for tool in tools {
            let name = tool["name"].as_str().expect("name");
            assert!(full.contains(&format!("`{name}(")), "{name} が本文に無い");
        }
        // 引数の並びは serde_json の Map の順（features 次第）なので、中身だけを見る。
        let update = full
            .lines()
            .find(|line| line.starts_with("- `fleet_update_task("))
            .expect("fleet_update_task の行がある");
        for parameter in ["task_id", "phase", "summary?"] {
            assert!(update.contains(parameter), "{update}");
        }
        assert!(full.contains("`fleet_integrate_task(task_id)`（人間の操作"));
        for subcommand in cli_shim::PASSTHROUGH_SUBCOMMANDS {
            assert!(full.contains(subcommand));
        }
    }

    #[test]
    fn install_options_accept_both_flag_forms_and_reject_unknown_ones() {
        assert_eq!(
            parse_install_options(&strings(&[
                "--agent",
                "all",
                "--project",
                "/work",
                "--force"
            ]))
            .expect("読める"),
            InstallOptions {
                agents: Some(SkillAgent::ALL.to_vec()),
                project: Some(PathBuf::from("/work")),
                force: true,
            }
        );
        assert_eq!(
            parse_install_options(&strings(&["--agent=codex"]))
                .expect("読める")
                .agents,
            Some(vec![SkillAgent::Codex])
        );
        assert!(parse_install_options(&strings(&["--agent", "cursor"])).is_err());
        assert!(parse_install_options(&strings(&["--agent"])).is_err());
        assert!(parse_install_options(&strings(&["--global"])).is_err());
        assert!(parse_get_options(&strings(&["--full"])).expect("読める"));
        assert!(parse_get_options(&strings(&["--json"])).is_err());
        assert_eq!(
            parse_list_options(&strings(&["--project=/work"])).expect("読める"),
            Some(PathBuf::from("/work"))
        );
    }

    #[test]
    fn install_into_a_scratch_home_then_refuses_changed_files() {
        let home = scratch_home("install");
        let desired = agent_skills::stub_skill_md(Path::new(BINARY));
        let options = InstallOptions {
            agents: Some(SkillAgent::ALL.to_vec()),
            ..InstallOptions::default()
        };
        let reports = install(&home, &options, None, &desired).expect("置ける");
        let claude = home.join(".claude/skills/necoder/SKILL.md");
        let codex = home.join(".codex/skills/necoder/SKILL.md");
        assert_eq!(
            reports,
            vec![
                InstallReport {
                    agent: SkillAgent::ClaudeCode,
                    path: claude.clone(),
                    outcome: InstallOutcome::Created,
                },
                InstallReport {
                    agent: SkillAgent::Codex,
                    path: codex.clone(),
                    outcome: InstallOutcome::Created,
                },
            ]
        );
        assert_eq!(std::fs::read_to_string(&codex).expect("読める"), desired);

        // 手を入れたファイルは --force が無ければ残す。
        std::fs::write(&claude, "---\nname: necoder\ndescription: 手書き\n---\n").expect("書ける");
        let again = install(&home, &options, None, &desired).expect("読める");
        assert_eq!(
            again[0].outcome,
            InstallOutcome::Refused(StubState::Foreign)
        );
        assert_eq!(again[1].outcome, InstallOutcome::Unchanged);
        let forced = InstallOptions {
            force: true,
            ..options
        };
        let replaced = install(&home, &forced, None, &desired).expect("上書きできる");
        assert_eq!(
            replaced[0].outcome,
            InstallOutcome::Replaced(StubState::Foreign)
        );
        assert_eq!(std::fs::read_to_string(&claude).expect("読める"), desired);

        // --project は Claude Code が `.claude/skills`、Codex が `.agents/skills`。
        let project = home.join("repo");
        std::fs::create_dir_all(&project).expect("作れる");
        let in_project = install(&home, &forced, Some(&project), &desired).expect("置ける");
        assert_eq!(
            in_project[0].path,
            project.join(".claude/skills/necoder/SKILL.md")
        );
        assert_eq!(
            in_project[1].path,
            project.join(".agents/skills/necoder/SKILL.md")
        );

        // 一覧は necoder の skill に最新かどうかを添える。
        let scan = agent_skills::scan(&agent_skills::skill_roots(&home, Some(&project)));
        let listed = format_list(&scan, &home, &desired);
        assert!(listed.contains("necoder（Claude Code） · 最新"));
        assert!(listed.contains("necoder（共通・プロジェクト） · 最新"));
        std::fs::write(
            &codex,
            agent_skills::stub_skill_md(Path::new("/old/necoder")),
        )
        .expect("書ける");
        let scan = agent_skills::scan(&agent_skills::skill_roots(&home, None));
        assert!(format_list(&scan, &home, &desired).contains("necoder（Codex） · 古い版"));
        std::fs::remove_dir_all(&home).expect("片付けられる");
    }
}
