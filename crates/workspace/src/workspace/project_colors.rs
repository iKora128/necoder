//! プロジェクト色の永続化 — 識別色をレールの並び順に依存させない（2026-09-07）。
//!
//! 優先順（UI-SPEC §1.2）: `.necoder/settings.json` の `color` > ローカル DB `project_colors`
//! （手動選択と初回の自動割当を焼いたもの）> 未使用のパレット色。`.necoder` / DB で決まった色は
//! 同じ窓で衝突しても尊重する（「いつも同じ色」が識別の前提・衝突は手動で直せる）。自動割当
//! だけが衝突を避ける。リモートは `.necoder` がリモート側にあり読めないので DB > パレットの 2 段。
//!
//! 以前はリモートだけホスト単位（`host_colors`）で焼き、ローカルは並び順で巡回していた。
//! 並びが変わる操作（外す・別窓に単独で開く・窓セッションを失う）のたびに色がずれ、
//! 同じホストのプロジェクトは全部同色になっていた。

use crate::workspace::*;

/// storage 上の local を表す scope。リモートはホスト鍵（`host_scope_key`）。
pub(crate) const LOCAL_SCOPE: &str = "local";

/// storage のテーブルで local / リモートを区別する scope。リモートは `display_name` から
/// `connect_ssh_and_open` が記録するキー "user@host"/alias を復元する（「最近」と同じ鍵）。
pub(crate) fn host_scope_key(host: &dyn Host) -> String {
    if !host.is_remote() {
        return LOCAL_SCOPE.to_string();
    }
    let display = host.display_name();
    display
        .strip_prefix("SSH ")
        .unwrap_or(display)
        .replace(' ', "")
}

/// `project_colors` のキー（scope, path）。path は開いたときの root 文字列そのもの
/// （窓セッションに保存する root と同じ＝復元でも同じ鍵に当たる）。
pub(crate) fn project_color_key(worktree: &Worktree) -> (String, String) {
    (
        host_scope_key(worktree.host().as_ref()),
        worktree.root().to_string_lossy().to_string(),
    )
}

/// 決定済み（`.necoder` / DB）の色を尊重しつつ、未決の slot に未使用パレット色を配る。
/// 未決同士も互いに避ける。パレットを使い切ったら添字で巡回（従来どおり）。
pub(crate) fn assign_free_colors(pinned: &[Option<Hsla>]) -> Vec<Hsla> {
    let mut used: Vec<Hsla> = pinned.iter().flatten().copied().collect();
    pinned
        .iter()
        .enumerate()
        .map(|(index, pin)| {
            if let Some(color) = pin {
                return *color;
            }
            let color = theme_core::IDENTITY_PALETTE_HEXES
                .iter()
                .map(|&value| theme_core::color_from_hex(value))
                .find(|candidate| !used.iter().any(|taken| colors_close(*taken, *candidate)))
                .unwrap_or_else(|| project_color(index));
            used.push(color);
            color
        })
        .collect()
}

/// 窓を組み立てるときに全 slot の色を解決し、決まった色を DB へ焼く（session を作る前に 1 回。
/// TodoPanel 等の accent は slot.color から初期化されるので、その前に確定させる）。
/// DB は 1 往復の一括読みで、書くのは値が変わる行だけ。
pub(crate) fn resolve_project_colors(projects: &mut [ProjectSlot], storage: &storage::Storage) {
    let stored: HashMap<(String, String), u32> = match storage.load_project_colors() {
        Ok(rows) => rows
            .into_iter()
            .map(|(scope, path, color)| ((scope, path), color))
            .collect(),
        Err(error) => {
            eprintln!("プロジェクト色を読めない（今回は並び順で配る）: {error:#}");
            return;
        }
    };
    let keys: Vec<(String, String)> = projects
        .iter()
        .map(|slot| project_color_key(&slot.worktree))
        .collect();
    let pinned: Vec<Option<Hsla>> = projects
        .iter()
        .zip(&keys)
        .map(|(slot, key)| {
            slot.identity_color
                .or_else(|| stored.get(key).map(|&value| theme_core::color_from_hex(value)))
        })
        .collect();
    let colors = assign_free_colors(&pinned);
    for ((slot, key), color) in projects.iter_mut().zip(keys).zip(colors) {
        slot.color = color;
        let value = theme_core::hex_from_color(color);
        if stored.get(&key) != Some(&value) {
            if let Err(error) = storage.set_project_color(&key.0, &key.1, value) {
                eprintln!("プロジェクト色を保存できない: {error:#}");
            }
        }
    }
}

impl Workspace {
    /// DB に焼いてある色（レールへ足すときの `.necoder` の次の候補）。
    pub(crate) fn stored_project_color(&self, worktree: &Worktree) -> Option<Hsla> {
        let storage = self.persistence.storage.as_ref()?;
        let (scope, path) = project_color_key(worktree);
        match storage.project_color(&scope, &path) {
            Ok(value) => value.map(theme_core::color_from_hex),
            Err(error) => {
                eprintln!("プロジェクト色を読めない: {error:#}");
                None
            }
        }
    }

    /// slot の今の色を DB へ焼く（手動選択・レールへの追加）。
    pub(crate) fn persist_project_color(&self, project_index: usize) {
        let (Some(storage), Some(slot)) = (
            self.persistence.storage.as_ref(),
            self.project_sessions.projects.get(project_index),
        ) else {
            return;
        };
        let (scope, path) = project_color_key(&slot.worktree);
        let value = theme_core::hex_from_color(slot.color);
        if let Err(error) = storage.set_project_color(&scope, &path, value) {
            eprintln!("プロジェクト色を保存できない: {error:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(index: usize) -> Hsla {
        theme_core::color_from_hex(theme_core::IDENTITY_PALETTE_HEXES[index])
    }

    #[test]
    fn pinned_colors_are_kept_even_when_they_collide() {
        // `.necoder` / DB で決まった色は衝突しても変えない（「いつも同じ色」が先）。
        let red = palette(0);
        let colors = assign_free_colors(&[Some(red), Some(red)]);
        assert!(colors_close(colors[0], red));
        assert!(colors_close(colors[1], red));
    }

    #[test]
    fn free_slots_avoid_pinned_and_each_other() {
        // pinned が red と blue → 未決 2 枚は green と orange（パレット順で最初の空き）。
        let pinned = [Some(palette(0)), None, Some(palette(2)), None];
        let colors = assign_free_colors(&pinned);
        assert!(colors_close(colors[0], palette(0)));
        assert!(colors_close(colors[1], palette(1)));
        assert!(colors_close(colors[2], palette(2)));
        assert!(colors_close(colors[3], palette(3)));
        for (left, first) in colors.iter().enumerate() {
            for second in colors.iter().skip(left + 1) {
                assert!(!colors_close(*first, *second), "同色 2 枚が出た");
            }
        }
    }

    #[test]
    fn exhausted_palette_falls_back_to_rotation() {
        // パレット 10 色を使い切った 11 枚目以降は添字巡回（落ちない・色は出る）。
        let pinned: Vec<Option<Hsla>> = vec![None; theme_core::IDENTITY_PALETTE_HEXES.len() + 2];
        let colors = assign_free_colors(&pinned);
        assert_eq!(colors.len(), pinned.len());
        let last = theme_core::IDENTITY_PALETTE_HEXES.len() + 1;
        assert!(colors_close(colors[last], project_color(last)));
    }

    #[test]
    fn scope_key_matches_recent_projects_convention() {
        // local は固定 scope。リモートは「最近」と同じホスト鍵（"SSH " 接頭辞と空白を落とす）。
        assert_eq!(host_scope_key(host::LocalHost::shared().as_ref()), LOCAL_SCOPE);
    }

    /// 窓を組み立てて各 slot の色を返す（テスト用・後片付け込み）。
    fn colors_of(
        roots: Vec<PathBuf>,
        storage: &storage::Storage,
        cx: &mut gpui::TestAppContext,
    ) -> Vec<Hsla> {
        let persistence = WindowPersistence {
            storage: Some(storage.clone()),
            window_id: None,
        };
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new(roots, Theme::dark(), Some(persistence), cx)
        });
        let colors = workspace.update_in(cx, |workspace, _window, _cx| {
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
            workspace
                .project_sessions
                .projects
                .iter()
                .map(|slot| slot.color)
                .collect::<Vec<_>>()
        });
        colors
    }

    #[gpui::test]
    fn project_colors_survive_reorder_and_restart(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "necoder_project_colors_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Worktree は root を canonicalize する（macOS の /var → /private/var）。DB の鍵もその
        // 文字列なので、テスト側も同じ形で持つ。
        let root = std::fs::canonicalize(&root).unwrap();
        let project_a = root.join("a");
        let project_b = root.join("b");
        let project_c = root.join("c");
        for dir in [&project_a, &project_b, &project_c] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).unwrap();
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let db_path = root.join("necoder.db");
        let storage = storage::Storage::open(&db_path).unwrap();

        // 初回: 並び順で配られ、その場で DB に焼かれる。
        let first = colors_of(vec![project_a.clone(), project_b.clone()], &storage, cx);
        assert!(!colors_close(first[0], first[1]), "同色 2 枚で始まっている");
        let stored_a = storage
            .project_color(LOCAL_SCOPE, &project_a.to_string_lossy())
            .unwrap()
            .expect("a の色が DB に焼かれていない");
        assert!(colors_close(theme_core::color_from_hex(stored_a), first[0]));

        // 再起動（別の窓を組み立て直す）で並びを入れ替え、c を足しても a/b の色は前回どおり。
        // c は a/b と衝突しない空き色を貰う。
        let second = colors_of(
            vec![project_c.clone(), project_b.clone(), project_a.clone()],
            &storage,
            cx,
        );
        assert!(colors_close(second[2], first[0]), "a の色が並び順で変わった");
        assert!(colors_close(second[1], first[1]), "b の色が並び順で変わった");
        assert!(!colors_close(second[0], first[0]) && !colors_close(second[0], first[1]));

        // `.necoder/settings.json` の色は DB より強い（チームで揃える色）。
        std::fs::create_dir_all(project_a.join(".necoder")).unwrap();
        std::fs::write(
            project_a.join(".necoder/settings.json"),
            r##"{"color":"#d946ef"}"##,
        )
        .unwrap();
        let third = colors_of(vec![project_a.clone()], &storage, cx);
        assert!(colors_close(third[0], theme_core::color_from_hex(0xd946ef)));
        // その色は DB にも写る（`.necoder` を消しても同じ色で開く）。
        assert_eq!(
            storage
                .project_color(LOCAL_SCOPE, &project_a.to_string_lossy())
                .unwrap(),
            Some(0xd946ef)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
