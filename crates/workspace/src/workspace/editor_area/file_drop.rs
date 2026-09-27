//! Finder からエディタへ落とす（O29・画像のドロップ）。
//!
//! - **Markdown に画像** → 画像を .md と同じフォルダへコピーし（project の中の画像ならコピーせずに
//!   そのまま指す）、キャレットの位置に `![名前](相対パス)` を入れる（1 枚 1 行・⌘Z で文が戻る）。
//!   同じ名前が既にあれば `name-1.png` のように避ける（上書きしない）。
//! - それ以外（画像でない・Markdown でない・画像とそれ以外が混ざる）→ 落としたファイルをタブで開く
//!   （フォルダは開かない＝プロジェクトとして開くのはレールの ＋）。
//!
//! 手元の project だけ（落ちてくるのは手元のパス。接続先へはエクスプローラに落とすとアップロード）。

use crate::workspace::image_view::is_image_path;
use crate::workspace::*;
use std::path::Component;

/// Markdown のファイルか（言語の判定と同じ）。
fn is_markdown_path(path: &Path) -> bool {
    lang::language_for_path(path) == Some(lang::LanguageId::Markdown)
}

/// `directory` の中で `name` を上書きしない置き場（あれば `name-1.ext`・`name-2.ext`…）。
fn unused_destination(directory: &Path, name: &std::ffi::OsStr) -> anyhow::Result<PathBuf> {
    let first = directory.join(name);
    if first.symlink_metadata().is_err() {
        return Ok(first);
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..1000 {
        let candidate = directory.join(format!("{stem}-{index}{extension}"));
        if candidate.symlink_metadata().is_err() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("置き場の名前を決められない: {}", first.display())
}

/// 画像を Markdown から指せる所に置く。project の中にあればそのまま（コピーしない）、外なら
/// `document_dir` へコピーする。返すのは置いた先と、コピーしたか。
fn place_image(
    source: &Path,
    document_dir: &Path,
    project_root: &Path,
) -> anyhow::Result<(PathBuf, bool)> {
    let source = paths::canonicalize_or_keep(source);
    if source.starts_with(paths::canonicalize_or_keep(project_root)) {
        return Ok((source, false));
    }
    let name = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("名前が取れない: {}", source.display()))?;
    let destination = unused_destination(document_dir, name)?;
    std::fs::copy(&source, &destination)
        .map_err(|error| anyhow::anyhow!("コピーできない: {}: {error}", source.display()))?;
    Ok((destination, true))
}

/// `from_dir` から `to` への相対パス（`/` 区切り）。根を共有しない（別のドライブ）なら `to` の
/// 絶対パスを `/` 区切りで。
fn relative_link(from_dir: &Path, to: &Path) -> String {
    let from: Vec<Component> = from_dir.components().collect();
    let target: Vec<Component> = to.components().collect();
    let common = from
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let name = |component: &Component| component.as_os_str().to_string_lossy().to_string();
    if common == 0 {
        return to.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - common];
    parts.extend(target[common..].iter().map(name));
    parts.join("/")
}

/// `![名前](相対パス)`。名前は拡張子を除いたファイル名（`[` `]` は逃がす）、パスの空白と括弧は
/// % で書く（Markdown のリンクの中で切れないように）。
pub(crate) fn markdown_image_link(image: &Path, document_dir: &Path) -> String {
    let alt = image
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default()
        .replace('[', "\\[")
        .replace(']', "\\]");
    let target = relative_link(document_dir, image)
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29");
    format!("![{alt}]({target})")
}

impl Workspace {
    /// Finder からエディタ（`target` = 落とされたペインのエディタ・`None` = 主ペインのいまのタブ）へ
    /// 落とされた `paths`。`position` は落とした所（窓の座標・画像のリンクをそこへ入れる）。
    pub(crate) fn drop_paths_on_editor(
        &mut self,
        paths: Vec<PathBuf>,
        target: Option<Entity<EditorView>>,
        position: Option<gpui::Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self.active_worktree() else {
            return;
        };
        if worktree.is_remote() {
            self.push_toast(
                SharedString::from(i18n::t!("editor.drop_remote")),
                self.accent(),
                cx,
            );
            return;
        }
        let files: Vec<PathBuf> = paths.into_iter().filter(|path| path.is_file()).collect();
        if files.is_empty() {
            return;
        }
        let editor = target.or_else(|| self.active_editor());
        let document = editor.as_ref().and_then(|editor| {
            editor
                .read(cx)
                .buffer()
                .path()
                .filter(|path| is_markdown_path(path))
                .map(Path::to_path_buf)
        });
        if let (Some(editor), Some(document)) = (editor, document) {
            if files.iter().all(|file| is_image_path(file)) {
                let root = worktree.root().to_path_buf();
                self.insert_dropped_images(&editor, &document, &files, &root, position, window, cx);
                return;
            }
        }
        for file in files {
            self.open_file(file, window, cx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_dropped_images(
        &mut self,
        editor: &Entity<EditorView>,
        document: &Path,
        images: &[PathBuf],
        project_root: &Path,
        position: Option<gpui::Point<gpui::Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document_dir) = document.parent() else {
            return;
        };
        let mut links = Vec::new();
        let mut copied = false;
        let mut first_error = None;
        for image in images {
            match place_image(image, document_dir, project_root) {
                Ok((placed, was_copied)) => {
                    copied |= was_copied;
                    links.push(markdown_image_link(&placed, document_dir));
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if !links.is_empty() {
            let text = links.join("\n");
            editor.update(cx, |view, cx| {
                view.insert_dropped_text(position, &text, window, cx)
            });
        }
        if copied {
            self.refresh_active_explorer(cx);
            self.refresh_git_status(cx);
        }
        if let Some(error) = first_error {
            self.push_toast(SharedString::from(format!("{error:#}")), self.accent(), cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_relative_and_safe_inside_markdown() {
        assert_eq!(
            markdown_image_link(Path::new("/p/docs/shot.png"), Path::new("/p/docs")),
            "![shot](shot.png)"
        );
        assert_eq!(
            markdown_image_link(Path::new("/p/assets/logo.svg"), Path::new("/p/docs/guide")),
            "![logo](../../assets/logo.svg)"
        );
        assert_eq!(
            markdown_image_link(
                Path::new("/p/docs/Screen Shot (2).png"),
                Path::new("/p/docs")
            ),
            "![Screen Shot (2)](Screen%20Shot%20%282%29.png)",
            "空白と括弧は % で"
        );
        assert_eq!(
            markdown_image_link(Path::new("/p/docs/a[1].png"), Path::new("/p/docs")),
            "![a\\[1\\]](a[1].png)"
        );
    }

    #[gpui::test]
    fn images_dropped_on_markdown_are_copied_and_linked(cx: &mut gpui::TestAppContext) {
        let base = std::env::temp_dir().join(format!("necoder_editor_drop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("project/docs")).unwrap();
        std::fs::create_dir_all(base.join("project/assets")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        let base = paths::canonicalize(&base).unwrap();
        let project = base.join("project");
        std::fs::write(project.join("docs/readme.md"), "# Title\n").unwrap();
        std::fs::write(project.join("assets/logo.png"), b"png").unwrap();
        std::fs::write(project.join("notes.txt"), "notes\n").unwrap();
        std::fs::write(base.join("outside/shot.png"), b"png").unwrap();
        let settings_path = base.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(vec![project.clone()], Theme::dark(), None, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            for session in &mut workspace.project_sessions.sessions {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace.open_file(project.join("docs/readme.md"), window, cx);
        });
        cx.run_until_parked();
        let text = |workspace: &Workspace, cx: &App| {
            workspace
                .active_editor()
                .map(|editor| editor.read(cx).buffer().text())
                .unwrap_or_default()
        };

        // 外の画像 → .md の隣へコピーしてリンク。2 回目は名前を避ける。project の中の画像は指すだけ。
        workspace.update_in(cx, |workspace, window, cx| {
            let outside = base.join("outside/shot.png");
            workspace.drop_paths_on_editor(vec![outside.clone()], None, None, window, cx);
            workspace.drop_paths_on_editor(vec![outside], None, None, window, cx);
            let logo = project.join("assets/logo.png");
            workspace.drop_paths_on_editor(vec![logo], None, None, window, cx);
        });
        assert!(project.join("docs/shot.png").is_file());
        assert!(project.join("docs/shot-1.png").is_file(), "上書きしない");
        assert!(
            !project.join("docs/logo.png").exists(),
            "project の中の画像はコピーしない"
        );
        workspace.update_in(cx, |workspace, _window, cx| {
            let text = text(workspace, cx);
            assert!(text.contains("![shot](shot.png)"), "{text}");
            assert!(text.contains("![shot-1](shot-1.png)"), "{text}");
            assert!(text.contains("![logo](../assets/logo.png)"), "{text}");
        });

        // 画像でないファイルは開く（Markdown に文は足さない）。
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.drop_paths_on_editor(vec![project.join("notes.txt")], None, None, window, cx);
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, _window, _cx| {
            assert!(
                workspace
                    .active_tab_path()
                    .is_some_and(|path| path.ends_with("notes.txt")),
                "落としたファイルをタブで開く"
            );
            assert_eq!(workspace.tabs.len(), 2);
        });
        let _ = std::fs::remove_dir_all(&base);
    }
}
