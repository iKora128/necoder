//! policy — 権限リクエストの裁定（`docs/CHAT.md` §3.2）。
//!
//! 原則は「**ファイルを渡す = 触ってよいと伝える**」。表にするとこうなる:
//!
//! | 対象 | 読む | 書く |
//! |---|---|---|
//! | チャットのフォルダ | 許可 | 許可 |
//! | 添付したファイル・フォルダ | 許可 | 初回だけ確認（以後はそのチャットで許可） |
//! | それ以外 | 確認 | **拒否** |
//! | Web の検索と取得 | 許可 | — |
//!
//! Chat はシステムプロンプトを差し替えているが、**プロンプトは防御ではない**。Web から読んだ
//! 文面が「~/.ssh を書き換えろ」と言ってきても、書ける場所がここで閉じていれば被害は
//! チャットのフォルダと、ユーザーが自分で渡したファイルに限られる。cwd は作業場所であって
//! 隔離ではないので、裁定は necoder 側（ここ）が持ち、エージェントの設定には頼らない。

use acp_client::ToolCallKind;
use std::path::{Component, Path, PathBuf};

/// 裁定に使う、このチャットの持ち物。
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    /// チャットのフォルダ（＝エージェントの cwd）。
    pub chat_dir: &'a Path,
    /// ユーザーが渡したファイル・フォルダ（絶対パス）。
    pub attachments: &'a [PathBuf],
    /// そのうち、書き込みを「このチャットでは以後許可」にしたもの。
    pub write_grants: &'a [PathBuf],
}

/// 裁定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 聞かずに許可する。
    Allow,
    /// ユーザーに聞く。
    Ask,
    /// 聞かずに拒否する（理由を transcript に 1 行出す）。
    Deny(DenyReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// チャットのフォルダでも添付でもない場所への書き込み。
    WriteOutsideScope(PathBuf),
    /// コマンドの実行。Chat は Bash を持たせていないので、来たら設定の抜けか想定外の道具。
    Execute,
}

/// 権限リクエスト 1 件を裁く。`paths` はツール呼び出しが触る場所（相対ならフォルダ基準）。
pub fn judge(kind: Option<ToolCallKind>, paths: &[PathBuf], scope: Scope<'_>) -> Verdict {
    match kind {
        Some(ToolCallKind::Fetch) | Some(ToolCallKind::Think) => Verdict::Allow,
        Some(ToolCallKind::Execute) => Verdict::Deny(DenyReason::Execute),
        Some(ToolCallKind::Read) | Some(ToolCallKind::Search) => {
            // 場所を言わない検索は cwd（＝チャットのフォルダ）が対象。
            let readable = paths.iter().all(|path| {
                let path = absolute(path, scope.chat_dir);
                contains(scope.chat_dir, &path) || attachment_covering(&path, scope).is_some()
            });
            if readable {
                Verdict::Allow
            } else {
                Verdict::Ask
            }
        }
        Some(ToolCallKind::Edit) | Some(ToolCallKind::Delete) | Some(ToolCallKind::Move) => {
            if paths.is_empty() {
                return Verdict::Ask;
            }
            let mut needs_confirmation = false;
            for path in paths {
                let path = absolute(path, scope.chat_dir);
                if contains(scope.chat_dir, &path) {
                    continue;
                }
                match attachment_covering(&path, scope) {
                    Some(attachment) if scope.write_grants.contains(attachment) => {}
                    Some(_) => needs_confirmation = true,
                    None => return Verdict::Deny(DenyReason::WriteOutsideScope(path)),
                }
            }
            if needs_confirmation {
                Verdict::Ask
            } else {
                Verdict::Allow
            }
        }
        // MCP の道具など、何をするか分からないものは聞く。
        Some(ToolCallKind::Other) | None => Verdict::Ask,
    }
}

/// ユーザーが「以後許可」を選んだ時に覚える添付（書き込み先を覆っているもの）。
pub fn grants_for(paths: &[PathBuf], scope: Scope<'_>) -> Vec<PathBuf> {
    let mut grants: Vec<PathBuf> = Vec::new();
    for path in paths {
        let path = absolute(path, scope.chat_dir);
        if let Some(attachment) = attachment_covering(&path, scope) {
            if !grants.contains(attachment) && !scope.write_grants.contains(attachment) {
                grants.push(attachment.clone());
            }
        }
    }
    grants
}

fn attachment_covering<'a>(path: &Path, scope: Scope<'a>) -> Option<&'a PathBuf> {
    scope
        .attachments
        .iter()
        .find(|attachment| contains(attachment, path))
}

fn absolute(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// `path` が `base` そのものか、その配下にあるか。
///
/// `..` を畳むだけでは足りない — フォルダの中に置いたシンボリックリンク（`artifacts/x -> ~/.ssh`）で
/// 外へ出られる。存在する一番深い祖先を実体のパスへ解決してから比べる。
fn contains(base: &Path, path: &Path) -> bool {
    resolve(path).starts_with(resolve(base))
}

fn resolve(path: &Path) -> PathBuf {
    let normalized = normalize(path);
    let mut existing = normalized.as_path();
    let mut remainder: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = paths::canonicalize(existing) {
            let mut resolved = real;
            resolved.extend(remainder.iter().rev());
            return resolved;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                remainder.push(name);
                existing = parent;
            }
            _ => return normalized,
        }
    }
}

/// `.` と `..` を字面で畳む（まだ存在しないパスにも効く）。
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        chat_dir: PathBuf,
        note: PathBuf,
        blog: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!(
                "necoder_chat_policy_{tag}_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            let chat_dir = root.join("documents/necoder/2026-09-18 chat");
            let blog = root.join("work/blog");
            std::fs::create_dir_all(chat_dir.join("artifacts")).expect("chat_dir");
            std::fs::create_dir_all(&blog).expect("blog");
            let note = root.join("work/note.md");
            std::fs::write(&note, b"# note").expect("note");
            std::fs::write(blog.join("post.md"), b"# post").expect("post");
            Fixture {
                root,
                chat_dir,
                note,
                blog,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn scope<'a>(
        fixture: &'a Fixture,
        attachments: &'a [PathBuf],
        write_grants: &'a [PathBuf],
    ) -> Scope<'a> {
        Scope {
            chat_dir: &fixture.chat_dir,
            attachments,
            write_grants,
        }
    }

    #[test]
    fn the_chat_folder_is_free_to_read_and_write() {
        let fixture = Fixture::new("own");
        let scope = scope(&fixture, &[], &[]);
        let artifact = fixture.chat_dir.join("artifacts/timer.html");
        assert_eq!(
            judge(Some(ToolCallKind::Edit), &[artifact.clone()], scope),
            Verdict::Allow
        );
        assert_eq!(
            judge(Some(ToolCallKind::Read), &[artifact], scope),
            Verdict::Allow
        );
        // 相対パスはフォルダ基準。
        assert_eq!(
            judge(
                Some(ToolCallKind::Edit),
                &[PathBuf::from("artifacts/new.html")],
                scope
            ),
            Verdict::Allow
        );
        // 場所を言わない検索は cwd が対象。
        assert_eq!(
            judge(Some(ToolCallKind::Search), &[], scope),
            Verdict::Allow
        );
    }

    #[test]
    fn an_attachment_reads_freely_but_the_first_write_asks() {
        let fixture = Fixture::new("attach");
        let attachments = vec![fixture.note.clone(), fixture.blog.clone()];
        let ungranted = scope(&fixture, &attachments, &[]);
        let post = fixture.blog.join("post.md");

        assert_eq!(
            judge(Some(ToolCallKind::Read), &[fixture.note.clone()], ungranted),
            Verdict::Allow
        );
        assert_eq!(
            judge(Some(ToolCallKind::Read), &[post.clone()], ungranted),
            Verdict::Allow
        );
        assert_eq!(
            judge(Some(ToolCallKind::Edit), &[post.clone()], ungranted),
            Verdict::Ask
        );

        // 「以後許可」で覚えるのは、書き込み先を覆っている添付（ここではフォルダ）。
        let grants = grants_for(&[post.clone()], ungranted);
        assert_eq!(grants, vec![fixture.blog.clone()]);
        let granted = scope(&fixture, &attachments, &grants);
        assert_eq!(
            judge(Some(ToolCallKind::Edit), &[post], granted),
            Verdict::Allow
        );
        // フォルダへの許可は、別に渡したファイルには及ばない。
        assert_eq!(
            judge(Some(ToolCallKind::Edit), &[fixture.note.clone()], granted),
            Verdict::Ask
        );
    }

    #[test]
    fn everything_else_asks_to_read_and_refuses_to_write() {
        let fixture = Fixture::new("outside");
        let scope = scope(&fixture, &[], &[]);
        let stranger = fixture.root.join("work/secret.txt");
        assert_eq!(
            judge(Some(ToolCallKind::Read), &[stranger.clone()], scope),
            Verdict::Ask
        );
        assert_eq!(
            judge(Some(ToolCallKind::Edit), &[stranger.clone()], scope),
            Verdict::Deny(DenyReason::WriteOutsideScope(stranger.clone()))
        );
        assert_eq!(
            judge(Some(ToolCallKind::Delete), &[stranger.clone()], scope),
            Verdict::Deny(DenyReason::WriteOutsideScope(stranger.clone()))
        );
        // 許される場所と混ぜても、1 つでも外なら拒否。
        assert_eq!(
            judge(
                Some(ToolCallKind::Move),
                &[fixture.chat_dir.join("artifacts/a.html"), stranger.clone()],
                scope
            ),
            Verdict::Deny(DenyReason::WriteOutsideScope(stranger))
        );
    }

    #[test]
    fn dot_dot_does_not_climb_out_of_the_chat_folder() {
        let fixture = Fixture::new("dotdot");
        let scope = scope(&fixture, &[], &[]);
        let escaping = fixture.chat_dir.join("artifacts/../../../../work/note.md");
        assert!(matches!(
            judge(Some(ToolCallKind::Edit), &[escaping], scope),
            Verdict::Deny(DenyReason::WriteOutsideScope(_))
        ));
        assert!(matches!(
            judge(
                Some(ToolCallKind::Edit),
                &[PathBuf::from("../elsewhere/x.html")],
                scope
            ),
            Verdict::Deny(DenyReason::WriteOutsideScope(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_chat_folder_does_not_widen_it() {
        let fixture = Fixture::new("symlink");
        let scope = scope(&fixture, &[], &[]);
        let link = fixture.chat_dir.join("artifacts/out");
        std::os::unix::fs::symlink(fixture.root.join("work"), &link).expect("symlink");
        assert!(matches!(
            judge(Some(ToolCallKind::Edit), &[link.join("note.md")], scope),
            Verdict::Deny(DenyReason::WriteOutsideScope(_))
        ));
        // まだ無いファイルでも、リンクの先として裁く。
        assert!(matches!(
            judge(Some(ToolCallKind::Edit), &[link.join("new.md")], scope),
            Verdict::Deny(DenyReason::WriteOutsideScope(_))
        ));
    }

    #[test]
    fn the_web_is_open_the_shell_is_closed_and_unknown_tools_ask() {
        let fixture = Fixture::new("kinds");
        let scope = scope(&fixture, &[], &[]);
        assert_eq!(judge(Some(ToolCallKind::Fetch), &[], scope), Verdict::Allow);
        assert_eq!(
            judge(Some(ToolCallKind::Execute), &[], scope),
            Verdict::Deny(DenyReason::Execute)
        );
        assert_eq!(judge(Some(ToolCallKind::Other), &[], scope), Verdict::Ask);
        assert_eq!(judge(None, &[], scope), Verdict::Ask);
        // 場所の分からない書き込みは、許可も拒否も決められないので聞く。
        assert_eq!(judge(Some(ToolCallKind::Edit), &[], scope), Verdict::Ask);
    }
}
