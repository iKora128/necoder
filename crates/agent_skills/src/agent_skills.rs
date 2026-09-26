//! agent_skills — エージェント（Claude Code / Codex）が読む skill（`SKILL.md`）の配布と一覧。
//!
//! **入口だけを配る**: エージェントの skill 置き場に書くのは、いつ necoder を使うかと
//! 「使い方は `necoder skills get` を実行して読め」だけの薄い `SKILL.md`（stub）。コマンドと
//! フラグの説明は、実行中の necoder がその版の実装から出す（`crates/necoder/src/skills.rs`）。
//! 置いたファイルに使い方を書くと、necoder を更新した時に置いた側だけが古いまま残るため。
//!
//! 手法の出典: stablyai/orca@646e9a5 の `skill-stubs/`・`skills/orca-cli/SKILL.md`・
//! `docs/site/content/docs/cli/skills.mdx`（MIT）。「版に合った本文は CLI が出し、置くのは入口だけ」
//! という形だけを借り、文面・置き場・上書きの判定は necoder の CLI に合わせて独立に書いた。
//!
//! GUI（設定画面の Skills 節）と CLI（`necoder skills …`）の両方から使うので GPUI に依存しない。
//! **ホームディレクトリは引数で受ける**（テストが本物の `~/.claude` / `~/.codex` に触れないため）。
//! 実環境のホームは [`default_home`] で引く。

use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// necoder の skill の名前（置き場のフォルダ名 = front matter の `name`）。
pub const NECODER_SKILL_NAME: &str = "necoder";

/// stub の書式の版。**stub の文面を変えたら 1 つ上げる** — 入っている古い stub が
/// 「更新があります」になる（中身の比較だけでも気づけるが、版があれば古いと言い切れる）。
pub const STUB_REVISION: u32 = 1;

/// skill の本体ファイル名。
pub const SKILL_FILE_NAME: &str = "SKILL.md";

/// necoder が書いた stub の印（HTML コメント。エージェントが読む本文の邪魔をしない）。
/// これが無い `SKILL.md` は人や別のツールが書いたもの＝設定画面からは上書きしない。
const STUB_MARKER: &str = "necoder-skill-stub:";

/// front matter を読むために先頭から読む上限。本文（長い手順書もある）までは読まない。
const FRONT_MATTER_READ_LIMIT: u64 = 64 * 1024;

/// `SKILL.md` の無いフォルダへ潜る深さ。1 = 置き場の直下のもう 1 段まで
/// （Codex の同梱 skill は `~/.codex/skills/.system/<name>/SKILL.md` にある）。
const NESTED_SCAN_DEPTH: usize = 1;

/// 実環境のホーム（`paths::home_dir`）。テストはこれを使わず一時ディレクトリを渡す。
pub fn default_home() -> Option<PathBuf> {
    paths::home_dir()
}

// ---------------------------------------------------------------------------
// エージェントと置き場
// ---------------------------------------------------------------------------

/// necoder の skill を入れる先のエージェント（CLI の `--agent`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillAgent {
    ClaudeCode,
    Codex,
}

impl SkillAgent {
    pub const ALL: [SkillAgent; 2] = [SkillAgent::ClaudeCode, SkillAgent::Codex];

    /// `--agent` の値。
    pub fn id(self) -> &'static str {
        match self {
            SkillAgent::ClaudeCode => "claude",
            SkillAgent::Codex => "codex",
        }
    }

    /// 表示名（製品名なので訳さない）。
    pub fn label(self) -> &'static str {
        match self {
            SkillAgent::ClaudeCode => "Claude Code",
            SkillAgent::Codex => "Codex",
        }
    }

    /// エージェントの設定フォルダ（`~/.claude` / `~/.codex`）。在れば、このエージェントを使っているとみなす。
    fn config_dir(self, home: &Path) -> PathBuf {
        match self {
            SkillAgent::ClaudeCode => home.join(".claude"),
            SkillAgent::Codex => home.join(".codex"),
        }
    }

    /// ユーザー全体の skill 置き場（`~/.claude/skills` / `~/.codex/skills`）。
    pub fn user_skills_dir(self, home: &Path) -> PathBuf {
        self.config_dir(home).join("skills")
    }

    /// プロジェクトの skill 置き場。Claude Code は `<project>/.claude/skills`。Codex はリポジトリの
    /// `<project>/.agents/skills` を読む（openai/codex の `ext/skills` の root 解決で確認した）。
    pub fn project_skills_dir(self, project: &Path) -> PathBuf {
        match self {
            SkillAgent::ClaudeCode => project.join(".claude").join("skills"),
            SkillAgent::Codex => shared_skills_dir(project),
        }
    }
}

/// エージェント共通の置き場（`<base>/.agents/skills`）。ホームの下とプロジェクトの下の両方にある。
fn shared_skills_dir(base: &Path) -> PathBuf {
    base.join(".agents").join("skills")
}

/// `--agent claude|codex|all` を解く。知らない値は `None`。
pub fn parse_agents(value: &str) -> Option<Vec<SkillAgent>> {
    if value == "all" {
        return Some(SkillAgent::ALL.to_vec());
    }
    SkillAgent::ALL
        .into_iter()
        .find(|agent| agent.id() == value)
        .map(|agent| vec![agent])
}

/// `--agent` を省いた時（と設定画面のボタン）の入れ先: 設定フォルダがあるエージェント。
/// どれも無ければ Claude Code（necoder の AI 機能の既定の相手）。
pub fn default_agents(home: &Path) -> Vec<SkillAgent> {
    let detected: Vec<SkillAgent> = SkillAgent::ALL
        .into_iter()
        .filter(|agent| agent.config_dir(home).is_dir())
        .collect();
    if detected.is_empty() {
        vec![SkillAgent::ClaudeCode]
    } else {
        detected
    }
}

/// skill 置き場の中の necoder の skill（`<skills>/necoder/SKILL.md`）。
pub fn stub_path(skills_dir: &Path) -> PathBuf {
    skills_dir.join(NECODER_SKILL_NAME).join(SKILL_FILE_NAME)
}

/// 一覧で走査する置き場の種類（= 一覧の「どのエージェントが読むか」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillScope {
    /// `~/.claude/skills`
    ClaudeUser,
    /// `~/.codex/skills`
    CodexUser,
    /// `~/.agents/skills`（エージェント共通の置き場。Codex などが読む）
    SharedUser,
    /// `<project>/.claude/skills`
    ClaudeProject,
    /// `<project>/.agents/skills`
    SharedProject,
}

impl SkillScope {
    /// 機械向けの名前（CLI の出力・テスト）。
    pub fn id(self) -> &'static str {
        match self {
            SkillScope::ClaudeUser => "claude",
            SkillScope::CodexUser => "codex",
            SkillScope::SharedUser => "agents",
            SkillScope::ClaudeProject => "project-claude",
            SkillScope::SharedProject => "project-agents",
        }
    }

    /// プロジェクトの下の置き場か。
    pub fn is_project(self) -> bool {
        matches!(self, SkillScope::ClaudeProject | SkillScope::SharedProject)
    }
}

/// 走査する置き場 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub scope: SkillScope,
    pub path: PathBuf,
}

/// 一覧の走査対象。並び = 表示順（ユーザー全体 → プロジェクト）。
pub fn skill_roots(home: &Path, project: Option<&Path>) -> Vec<SkillRoot> {
    let mut roots = vec![
        SkillRoot {
            scope: SkillScope::ClaudeUser,
            path: SkillAgent::ClaudeCode.user_skills_dir(home),
        },
        SkillRoot {
            scope: SkillScope::CodexUser,
            path: SkillAgent::Codex.user_skills_dir(home),
        },
        SkillRoot {
            scope: SkillScope::SharedUser,
            path: shared_skills_dir(home),
        },
    ];
    if let Some(project) = project {
        let project_roots = [
            (
                SkillScope::ClaudeProject,
                SkillAgent::ClaudeCode.project_skills_dir(project),
            ),
            (SkillScope::SharedProject, shared_skills_dir(project)),
        ];
        for (scope, path) in project_roots {
            // ホームそのものをプロジェクトとして開いていると、同じ置き場を 2 度数えてしまう。
            if roots.iter().all(|root| root.path != path) {
                roots.push(SkillRoot { scope, path });
            }
        }
    }
    roots
}

// ---------------------------------------------------------------------------
// 走査（`necoder skills list` と設定画面の一覧）
// ---------------------------------------------------------------------------

/// 見つかった skill 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    /// 空白と改行は 1 つの空白に畳んである。
    pub description: String,
    /// `SKILL.md` のパス。
    pub path: PathBuf,
    pub scope: SkillScope,
}

/// 一覧から外したもの（読めない・front matter が壊れている）とその理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSkill {
    pub path: PathBuf,
    pub reason: String,
}

/// 走査の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillScan {
    pub skills: Vec<SkillEntry>,
    pub skipped: Vec<SkippedSkill>,
}

/// 置き場を順に走査する。`<root>/<name>/SKILL.md` を 1 つの skill として数え、`SKILL.md` の無い
/// フォルダは [`NESTED_SCAN_DEPTH`] 段まで潜る。壊れた front matter は落とさずに `skipped` へ
/// （他人の置き場なので、1 つの壊れたファイルで一覧全体を止めない）。
pub fn scan(roots: &[SkillRoot]) -> SkillScan {
    let mut result = SkillScan::default();
    for root in roots {
        scan_directory(&root.path, root.scope, 0, &mut result);
    }
    result
}

fn scan_directory(directory: &Path, scope: SkillScope, depth: usize, result: &mut SkillScan) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        // 置き場が無いのは普通（そのエージェントを使っていない）。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            result.skipped.push(SkippedSkill {
                path: directory.to_path_buf(),
                reason: error.to_string(),
            });
            return;
        }
    };
    // `is_dir` はシンボリックリンクの先を見る＝リンクで置いた skill も数える。
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    children.sort();
    for child in children {
        let skill_file = child.join(SKILL_FILE_NAME);
        if skill_file.is_file() {
            match read_skill(&skill_file, &child) {
                Ok((name, description)) => result.skills.push(SkillEntry {
                    name,
                    description,
                    path: skill_file,
                    scope,
                }),
                Err(reason) => result.skipped.push(SkippedSkill {
                    path: skill_file,
                    reason,
                }),
            }
        } else if depth < NESTED_SCAN_DEPTH {
            scan_directory(&child, scope, depth + 1, result);
        }
    }
}

/// 1 つの `SKILL.md` から名前と説明を読む。`name` が無ければフォルダ名で代える（Codex と同じ扱い）。
fn read_skill(skill_file: &Path, directory: &Path) -> Result<(String, String), String> {
    let text = read_head(skill_file).map_err(|error| error.to_string())?;
    let front_matter = parse_front_matter(&text).map_err(|error| error.to_string())?;
    let name = front_matter.name.unwrap_or_else(|| {
        directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    Ok((name, front_matter.description))
}

/// 先頭の [`FRONT_MATTER_READ_LIMIT`] バイトだけ読む（途中で切れた UTF-8 は置換文字にする）。
fn read_head(path: &Path) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(FRONT_MATTER_READ_LIMIT)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------------------
// front matter
// ---------------------------------------------------------------------------

/// `SKILL.md` の front matter のうち、一覧に要るもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    /// 無ければ `None`（一覧ではフォルダ名で代える）。
    pub name: Option<String>,
    /// 空白と改行は 1 つの空白に畳んである。
    pub description: String,
}

/// front matter が読めない理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontMatterError {
    /// 1 行目が `---` でない。
    Missing,
    /// 閉じの `---` が無い。
    Unclosed,
    /// YAML として読めない（型の違いを含む）。
    Invalid(String),
    /// `description` が無い・空。
    NoDescription,
}

impl std::fmt::Display for FrontMatterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrontMatterError::Missing => {
                write!(formatter, "front matter（先頭の ---）がありません")
            }
            FrontMatterError::Unclosed => {
                write!(formatter, "front matter の閉じ（---）がありません")
            }
            FrontMatterError::Invalid(detail) => {
                write!(formatter, "front matter を YAML として読めません: {detail}")
            }
            FrontMatterError::NoDescription => write!(formatter, "description がありません"),
        }
    }
}

impl std::error::Error for FrontMatterError {}

#[derive(Deserialize)]
struct RawFrontMatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// 先頭の `---` と `---` に挟まれた YAML から `name` / `description` を読む。
/// 知らないキー（`license` / `metadata` など）は無視する。
pub fn parse_front_matter(text: &str) -> Result<FrontMatter, FrontMatterError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err(FrontMatterError::Missing);
    }
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return Err(FrontMatterError::Unclosed);
    }
    if yaml.trim().is_empty() {
        return Err(FrontMatterError::NoDescription);
    }
    let raw: RawFrontMatter = serde_yaml::from_str(&yaml)
        .map_err(|error| FrontMatterError::Invalid(error.to_string()))?;
    let description = raw
        .description
        .map(|description| collapse_whitespace(&description))
        .filter(|description| !description.is_empty())
        .ok_or(FrontMatterError::NoDescription)?;
    let name = raw
        .name
        .map(|name| collapse_whitespace(&name))
        .filter(|name| !name.is_empty());
    Ok(FrontMatter { name, description })
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// necoder の skill（stub）の生成と設置
// ---------------------------------------------------------------------------

/// necoder の skill の `SKILL.md` 全文。`binary` = `skills get` に答える necoder の実体
/// （`necoder` は PATH に無いことが多いので、`ne` が無い時のために絶対パスも書く。Captain の
/// 役割プロンプトが実体のパスを渡しているのと同じ理由）。
pub fn stub_skill_md(binary: &Path) -> String {
    format!(
        r#"---
name: {name}
description: necoder（エディタ）を CLI から操作する。Fleet（Task ごとの worktree で複数のエージェントを並走させる仕組み）・Captain・`ne` コマンドの話が出たら、necoder のコマンドを打つ前に使う。
---

# necoder

これは入口だけで、使い方の本文ではない。本文は入っている necoder の版に合わせて CLI が出す。
necoder のコマンドを打つ前に次を実行し、出力を読むこと（`ne` コマンドが無ければ 2 行目）:

```sh
ne skills get
"{binary}" skills get
```

全コマンドの詳細は `skills get --full`。出力に無いコマンドやフラグを、記憶や推測で作らない。

<!-- {marker} {revision} -->
"#,
        name = NECODER_SKILL_NAME,
        binary = binary.display(),
        marker = STUB_MARKER,
        revision = STUB_REVISION,
    )
}

/// 置き場にある necoder の skill の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StubState {
    /// まだ置いていない。
    Missing,
    /// この necoder が書くものと同じ。
    Current,
    /// necoder が書いた stub だが中身が違う（stub の版が古い・別の necoder の実体を指している）。
    Outdated,
    /// necoder の印が無い `SKILL.md`（人や別のツールが書いた）。
    Foreign,
}

/// 既存の中身（無ければ `None`）と、この necoder が書く中身から状態を決める。
pub fn stub_state(existing: Option<&str>, desired: &str) -> StubState {
    match existing {
        None => StubState::Missing,
        Some(text) if text == desired => StubState::Current,
        Some(text) if text.contains(STUB_MARKER) => StubState::Outdated,
        Some(_) => StubState::Foreign,
    }
}

/// 置き場の `SKILL.md` を読んで状態を返す（無ければ [`StubState::Missing`]）。
pub fn read_stub_state(path: &Path, desired: &str) -> std::io::Result<StubState> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(stub_state(Some(&text), desired)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(StubState::Missing),
        Err(error) => Err(error),
    }
}

/// [`install_stub`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// 新しく置いた。
    Created,
    /// 中身の違う `SKILL.md` を上書きした（上書き前の状態つき）。
    Replaced(StubState),
    /// 同じ中身が既にあった（書いていない）。
    Unchanged,
    /// 中身の違う `SKILL.md` があるので書かなかった（`force` が無い）。
    Refused(StubState),
}

/// `path` に `desired` を置く。中身の違うファイルがある時は `force` の時だけ上書きする
/// （**上書きの前に知らせる**のは呼び手の仕事: CLI は `Refused` を見て `--force` を案内する）。
pub fn install_stub(path: &Path, desired: &str, force: bool) -> Result<InstallOutcome> {
    let state = read_stub_state(path, desired)
        .with_context(|| format!("{} を読めません", path.display()))?;
    match state {
        StubState::Current => return Ok(InstallOutcome::Unchanged),
        StubState::Outdated | StubState::Foreign if !force => {
            return Ok(InstallOutcome::Refused(state))
        }
        StubState::Missing | StubState::Outdated | StubState::Foreign => {}
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("{} を作れません", parent.display()))?;
    }
    std::fs::write(path, desired).with_context(|| format!("{} に書けません", path.display()))?;
    Ok(if state == StubState::Missing {
        InstallOutcome::Created
    } else {
        InstallOutcome::Replaced(state)
    })
}

/// ユーザー全体の置き場 1 つ分の、necoder の skill の状態（設定画面の「necoder の skill」行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubPlacement {
    pub agent: SkillAgent,
    pub path: PathBuf,
    /// 読めなかった時は理由の文。
    pub state: Result<StubState, String>,
}

/// `agents` それぞれのユーザー全体の置き場で、necoder の skill がどうなっているか。
pub fn user_stub_placements(
    home: &Path,
    agents: &[SkillAgent],
    desired: &str,
) -> Vec<StubPlacement> {
    agents
        .iter()
        .map(|agent| {
            let path = stub_path(&agent.user_skills_dir(home));
            let state = read_stub_state(&path, desired).map_err(|error| error.to_string());
            StubPlacement {
                agent: *agent,
                path,
                state,
            }
        })
        .collect()
}

/// 表示用に、ホームの下のパスを `~/…` に縮める。
pub fn display_path(path: &Path, home: &Path) -> String {
    if home.as_os_str().is_empty() {
        return path.display().to_string();
    }
    match path.strip_prefix(home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Ok(relative) => Path::new("~").join(relative).display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト毎の一時ホーム。**本物の `~/.claude` / `~/.codex` には触れない**。
    /// tag でテスト毎に分ける（cargo test は並列なので、共有すると互いに消し合う）。
    fn scratch_home(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "necoder_agent_skills_{}_{}",
            tag,
            std::process::id()
        ));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).expect("前回の残りを消せる");
        }
        std::fs::create_dir_all(&directory).expect("一時ホームを作れる");
        directory
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("親がある")).expect("フォルダを作れる");
        std::fs::write(path, text).expect("書ける");
    }

    const BINARY: &str = "/Applications/necoder.app/Contents/MacOS/necoder";

    #[test]
    fn stub_front_matter_is_valid_and_points_to_skills_get() {
        let stub = stub_skill_md(Path::new(BINARY));
        let front_matter = parse_front_matter(&stub).expect("stub 自身の front matter は読める");
        assert_eq!(front_matter.name.as_deref(), Some(NECODER_SKILL_NAME));
        assert!(front_matter.description.contains("necoder"));
        // Codex の上限（name 64・description 1024 文字）に収まること。
        assert!(front_matter.description.chars().count() <= 1024);
        assert!(stub.contains("ne skills get"));
        assert!(stub.contains(&format!("\"{BINARY}\" skills get")));
        assert!(stub.contains(STUB_MARKER));
    }

    #[test]
    fn stub_state_tells_current_outdated_and_foreign_apart() {
        let desired = stub_skill_md(Path::new(BINARY));
        assert_eq!(stub_state(None, &desired), StubState::Missing);
        assert_eq!(stub_state(Some(&desired), &desired), StubState::Current);
        // 別の実体（dev ビルド）を指している stub = necoder 製だが古い。
        let other_binary = stub_skill_md(Path::new("/work/target/debug/necoder"));
        assert_eq!(
            stub_state(Some(&other_binary), &desired),
            StubState::Outdated
        );
        let older_revision = desired.replace(
            &format!("{STUB_MARKER} {STUB_REVISION}"),
            &format!("{STUB_MARKER} 0"),
        );
        assert_eq!(
            stub_state(Some(&older_revision), &desired),
            StubState::Outdated
        );
        let foreign = "---\nname: necoder\ndescription: 手で書いた\n---\n";
        assert_eq!(stub_state(Some(foreign), &desired), StubState::Foreign);
    }

    #[test]
    fn install_creates_keeps_refuses_and_forces() {
        let home = scratch_home("install");
        let path = stub_path(&SkillAgent::ClaudeCode.user_skills_dir(&home));
        let desired = stub_skill_md(Path::new(BINARY));

        assert_eq!(
            install_stub(&path, &desired, false).expect("置ける"),
            InstallOutcome::Created
        );
        assert_eq!(std::fs::read_to_string(&path).expect("読める"), desired);
        assert_eq!(
            install_stub(&path, &desired, false).expect("読める"),
            InstallOutcome::Unchanged
        );

        // necoder 製の古い stub: force が無ければ書かない（上書きの前に知らせる）。
        let outdated = stub_skill_md(Path::new("/old/necoder"));
        std::fs::write(&path, &outdated).expect("書ける");
        assert_eq!(
            install_stub(&path, &desired, false).expect("読める"),
            InstallOutcome::Refused(StubState::Outdated)
        );
        assert_eq!(std::fs::read_to_string(&path).expect("読める"), outdated);
        assert_eq!(
            install_stub(&path, &desired, true).expect("上書きできる"),
            InstallOutcome::Replaced(StubState::Outdated)
        );
        assert_eq!(std::fs::read_to_string(&path).expect("読める"), desired);

        // 人が書いた SKILL.md も、force が無ければ残す。
        let foreign = "---\nname: necoder\ndescription: 手で書いた\n---\n本文\n";
        std::fs::write(&path, foreign).expect("書ける");
        assert_eq!(
            install_stub(&path, &desired, false).expect("読める"),
            InstallOutcome::Refused(StubState::Foreign)
        );
        assert_eq!(std::fs::read_to_string(&path).expect("読める"), foreign);

        std::fs::remove_dir_all(&home).expect("片付けられる");
    }

    #[test]
    fn placements_report_each_agent_state() {
        let home = scratch_home("placements");
        let desired = stub_skill_md(Path::new(BINARY));
        let claude = stub_path(&SkillAgent::ClaudeCode.user_skills_dir(&home));
        write(&claude, &desired);
        let placements = user_stub_placements(&home, &SkillAgent::ALL, &desired);
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].agent, SkillAgent::ClaudeCode);
        assert_eq!(placements[0].path, claude);
        assert_eq!(placements[0].state, Ok(StubState::Current));
        assert_eq!(placements[1].agent, SkillAgent::Codex);
        assert_eq!(placements[1].state, Ok(StubState::Missing));
        std::fs::remove_dir_all(&home).expect("片付けられる");
    }

    #[test]
    fn scan_reads_front_matter_and_skips_broken_ones() {
        let home = scratch_home("scan");
        let project = home.join("work").join("repo");
        write(
            &home.join(".claude/skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: >-\n  折り返した\n  説明\nlicense: MIT\n---\n# 本文\n",
        );
        // YAML として壊れている。
        write(
            &home.join(".claude/skills/broken/SKILL.md"),
            "---\nname: [unclosed\ndescription: x\n---\n",
        );
        // 閉じが無い。
        write(
            &home.join(".claude/skills/unclosed/SKILL.md"),
            "---\nname: unclosed\ndescription: x\n",
        );
        // SKILL.md の無いフォルダは数えない。
        std::fs::create_dir_all(home.join(".claude/skills/empty-folder")).expect("作れる");
        // Codex の同梱 skill（1 段下）。
        write(
            &home.join(".codex/skills/.system/beta/SKILL.md"),
            "---\nname: beta\ndescription: 同梱\n---\n",
        );
        // front matter が無い。
        write(
            &home.join(".agents/skills/plain/SKILL.md"),
            "# 見出しだけ\n",
        );
        // name が無ければフォルダ名。
        write(
            &project.join(".claude/skills/gamma/SKILL.md"),
            "---\ndescription: プロジェクトの skill\n---\n",
        );
        write(
            &project.join(".agents/skills/delta/SKILL.md"),
            "\u{feff}---\r\nname: delta\r\ndescription: CRLF\r\n---\r\n",
        );

        let result = scan(&skill_roots(&home, Some(&project)));
        let found: Vec<(&str, &str, SkillScope)> = result
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.description.as_str(), skill.scope))
            .collect();
        assert_eq!(
            found,
            vec![
                ("alpha", "折り返した 説明", SkillScope::ClaudeUser),
                ("beta", "同梱", SkillScope::CodexUser),
                ("gamma", "プロジェクトの skill", SkillScope::ClaudeProject),
                ("delta", "CRLF", SkillScope::SharedProject),
            ]
        );
        assert_eq!(
            result.skills[0].path,
            home.join(".claude/skills/alpha/SKILL.md")
        );
        let mut skipped: Vec<PathBuf> = result
            .skipped
            .iter()
            .map(|skipped| skipped.path.clone())
            .collect();
        skipped.sort();
        assert_eq!(
            skipped,
            vec![
                home.join(".agents/skills/plain/SKILL.md"),
                home.join(".claude/skills/broken/SKILL.md"),
                home.join(".claude/skills/unclosed/SKILL.md"),
            ]
        );
        std::fs::remove_dir_all(&home).expect("片付けられる");
    }

    #[test]
    fn front_matter_errors_are_specific() {
        assert_eq!(
            parse_front_matter("# no front matter"),
            Err(FrontMatterError::Missing)
        );
        assert_eq!(
            parse_front_matter("---\nname: a\n"),
            Err(FrontMatterError::Unclosed)
        );
        assert_eq!(
            parse_front_matter("---\n---\n"),
            Err(FrontMatterError::NoDescription)
        );
        assert_eq!(
            parse_front_matter("---\nname: a\ndescription: \"\"\n---\n"),
            Err(FrontMatterError::NoDescription)
        );
        assert!(matches!(
            parse_front_matter("---\nname: a\ndescription: [1, 2]\n---\n"),
            Err(FrontMatterError::Invalid(_))
        ));
    }

    #[test]
    fn roots_skip_the_project_when_it_is_the_home() {
        let home = Path::new("/home/test");
        let roots = skill_roots(home, Some(home));
        // `~/.agents/skills` は共有の置き場として 1 度だけ。`.claude/skills` も同じ。
        assert_eq!(roots.len(), 3);
        let with_project = skill_roots(home, Some(Path::new("/work/repo")));
        assert_eq!(with_project.len(), 5);
        assert_eq!(with_project[3].scope, SkillScope::ClaudeProject);
        assert_eq!(with_project[3].path, Path::new("/work/repo/.claude/skills"));
        assert_eq!(with_project[4].path, Path::new("/work/repo/.agents/skills"));
    }

    #[test]
    fn default_agents_follow_existing_config_folders() {
        let home = scratch_home("detect");
        assert_eq!(default_agents(&home), vec![SkillAgent::ClaudeCode]);
        std::fs::create_dir_all(home.join(".codex")).expect("作れる");
        assert_eq!(default_agents(&home), vec![SkillAgent::Codex]);
        std::fs::create_dir_all(home.join(".claude")).expect("作れる");
        assert_eq!(default_agents(&home), SkillAgent::ALL.to_vec());
        std::fs::remove_dir_all(&home).expect("片付けられる");
    }

    #[test]
    fn agent_values_and_paths() {
        assert_eq!(parse_agents("claude"), Some(vec![SkillAgent::ClaudeCode]));
        assert_eq!(parse_agents("codex"), Some(vec![SkillAgent::Codex]));
        assert_eq!(parse_agents("all"), Some(SkillAgent::ALL.to_vec()));
        assert_eq!(parse_agents("cursor"), None);
        let home = Path::new("/home/test");
        assert_eq!(
            stub_path(&SkillAgent::Codex.user_skills_dir(home)),
            Path::new("/home/test/.codex/skills/necoder/SKILL.md")
        );
        assert_eq!(
            stub_path(&SkillAgent::ClaudeCode.project_skills_dir(Path::new("/work/repo"))),
            Path::new("/work/repo/.claude/skills/necoder/SKILL.md")
        );
        assert_eq!(
            display_path(Path::new("/home/test/.claude/skills"), home),
            Path::new("~").join(".claude/skills").display().to_string()
        );
        assert_eq!(
            display_path(Path::new("/work/repo"), home),
            Path::new("/work/repo").display().to_string()
        );
    }
}
