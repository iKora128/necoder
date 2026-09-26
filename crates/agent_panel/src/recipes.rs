//! レシピ（O16・repo ごとの定型プロンプト）: `.necoder/recipes/<名前>.md` を composer の `/` 補完に
//! `necoder:<名前>` として出し、選ぶと**本文を composer へ入れる**（送らない・人が確かめて ⌘⏎）。
//! どのエージェントでも使える（Claude の `.claude/commands` は Claude だけ・Codex の skill は Codex だけ）。
//!
//! - 1 行目が `# ` で始まればそれを説明にし、本文からは外す（無ければ説明は本文の 1 行目）。
//! - 読むのは宛先（プロジェクト）が変わった時と、`/` を打ち始めた時（前に読んでから
//!   [`RECIPE_STALE_AFTER`] 経っていれば）。背景で Host 経由（SSH の先でも同じ）。描画中は読まない。
//! - 上限: [`MAX_RECIPES`] 件・1 本 [`MAX_RECIPE_BYTES`]（大きい物は読み飛ばす）。

use super::*;

/// `/` 補完でレシピの名前に付ける印（エージェントのコマンドと取り違えない）。
pub(crate) const RECIPE_PREFIX: &str = "necoder:";
/// 読み直すまでの間（`/` を打つたびに fs を読まない）。
pub(crate) const RECIPE_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_RECIPES: usize = 100;
const MAX_RECIPE_BYTES: u64 = 64 * 1024;

/// レシピ 1 本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recipe {
    /// ファイル名（`.md` を除く）。
    pub(crate) name: String,
    pub(crate) description: String,
    /// composer へ入れる本文。
    pub(crate) body: String,
}

/// レシピのファイルを読む（純関数）。`# ` の 1 行目は説明にして本文から外す。
pub(crate) fn parse_recipe(name: &str, text: &str) -> Recipe {
    let text = text.trim_start_matches('\u{feff}');
    let (description, body) = match text.split_once('\n') {
        Some((first, rest)) if first.starts_with("# ") => {
            (first[2..].trim().to_string(), rest.trim().to_string())
        }
        _ if text.starts_with("# ") => (text[2..].trim().to_string(), String::new()),
        _ => (
            text.lines().next().unwrap_or("").trim().to_string(),
            text.trim().to_string(),
        ),
    };
    Recipe {
        name: name.to_string(),
        description,
        body,
    }
}

/// `<root>/.necoder/recipes/*.md` を読む（背景で呼ぶ）。無ければ空。名前の昇順。
pub(crate) fn load_recipes(host: &dyn Host, root: &Path) -> Vec<Recipe> {
    let folder = root.join(".necoder").join("recipes");
    let Ok(entries) = host.read_dir(&folder) else {
        return Vec::new();
    };
    let mut recipes: Vec<Recipe> = entries
        .into_iter()
        .filter(|entry| !entry.is_dir && entry.name.ends_with(".md"))
        .take(MAX_RECIPES)
        .filter_map(|entry| {
            let size = host.metadata(&entry.path).ok()?.len;
            if size > MAX_RECIPE_BYTES {
                return None;
            }
            let content = host.read_file(&entry.path).ok()?;
            let text = String::from_utf8_lossy(&content.bytes);
            let name = entry.name.trim_end_matches(".md");
            Some(parse_recipe(name, &text))
        })
        .collect();
    recipes.sort_by(|left, right| left.name.cmp(&right.name));
    recipes
}

impl AgentPanel {
    /// 宛先のレシピを背景で読み直す。
    pub(crate) fn refresh_recipes(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.dest_cwd.clone() else {
            self.recipes.clear();
            return;
        };
        let host = self.dest_host.clone();
        self.recipes_read_at = Some(std::time::Instant::now());
        cx.spawn(async move |panel, cx| {
            let recipes = cx
                .background_executor()
                .spawn(async move { load_recipes(host.as_ref(), &root) })
                .await;
            // Err = 読んでいる間にパネルが閉じた。
            panel
                .update(cx, |panel, cx| {
                    if panel.recipes != recipes {
                        panel.recipes = recipes;
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    /// `/` を打ち始めた: 前に読んでから時間が経っていれば読み直す（ファイルを足した直後にも出る）。
    pub(crate) fn refresh_recipes_if_stale(&mut self, cx: &mut Context<Self>) {
        let stale = self
            .recipes_read_at
            .is_none_or(|read_at| read_at.elapsed() >= RECIPE_STALE_AFTER);
        if stale {
            self.refresh_recipes(cx);
        }
    }

    /// `/` 補完に足すレシピの候補。
    pub(crate) fn recipe_commands(&self) -> impl Iterator<Item = acp_client::SlashCommand> + '_ {
        self.recipes.iter().map(|recipe| acp_client::SlashCommand {
            name: format!("{RECIPE_PREFIX}{}", recipe.name),
            description: recipe.description.clone(),
            hint: None,
        })
    }

    /// `necoder:<名前>` のレシピの本文。
    pub(crate) fn recipe_body(&self, command_name: &str) -> Option<String> {
        let name = command_name.strip_prefix(RECIPE_PREFIX)?;
        self.recipes
            .iter()
            .find(|recipe| recipe.name == name)
            .map(|recipe| recipe.body.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heading_line_becomes_the_description() {
        let recipe = parse_recipe(
            "review",
            "# 変更を読んで指摘する\n\n差分を読み、\n問題を挙げて。\n",
        );
        assert_eq!(recipe.description, "変更を読んで指摘する");
        assert_eq!(recipe.body, "差分を読み、\n問題を挙げて。");
        let plain = parse_recipe("fix", "テストを直して\n全部通るまで");
        assert_eq!(plain.description, "テストを直して");
        assert_eq!(
            plain.body, "テストを直して\n全部通るまで",
            "見出しが無ければ本文はそのまま"
        );
    }

    #[test]
    fn recipes_are_read_from_the_project_folder() {
        let root = std::env::temp_dir().join(format!("necoder_recipes_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let folder = root.join(".necoder/recipes");
        std::fs::create_dir_all(folder.join("nested")).expect("作れる");
        std::fs::write(folder.join("review.md"), "# 読んで指摘\n差分を読んで").expect("書ける");
        std::fs::write(folder.join("audit.md"), "依存を調べて").expect("書ける");
        std::fs::write(folder.join("notes.txt"), "md でない").expect("書ける");
        let recipes = load_recipes(&LocalHost, &root);
        assert_eq!(
            recipes
                .iter()
                .map(|recipe| recipe.name.as_str())
                .collect::<Vec<_>>(),
            vec!["audit", "review"],
            "名前の順・md だけ・フォルダは読まない"
        );
        assert!(load_recipes(&LocalHost, &root.join("nowhere")).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
