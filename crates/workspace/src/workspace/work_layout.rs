//! 作業場所と配置だけを保存する。会話・PTY・バッファの寿命は ProjectSession が所有する。
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WorkSurface {
    Agent,
    Terminal { id: u64 },
    File { path: PathBuf },
    Diff,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WorkPane {
    pub id: u64,
    pub tabs: Vec<WorkSurface>,
    pub active: usize,
    pub vertical_tabs: bool,
    pub weight: f32,
}

impl WorkPane {
    pub fn selected(&self) -> Option<&WorkSurface> {
        self.tabs.get(self.active)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WorkColumn {
    pub space: String,
    pub panes: Vec<WorkPane>,
    pub weight: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct RepositoryLayout {
    pub columns: Vec<WorkColumn>,
    /// 切り替え・閉じる操作でもその worktree のタブ配置を忘れない。
    pub hidden: BTreeMap<String, WorkColumn>,
    pub focused: Option<u64>,
    pub maximized: Option<u64>,
    pub grid: bool,
}

impl RepositoryLayout {
    pub fn pane(&self, id: u64) -> Option<&WorkPane> {
        self.columns
            .iter()
            .flat_map(|column| &column.panes)
            .find(|pane| pane.id == id)
    }

    pub fn pane_mut(&mut self, id: u64) -> Option<&mut WorkPane> {
        self.columns
            .iter_mut()
            .flat_map(|column| &mut column.panes)
            .find(|pane| pane.id == id)
    }

    pub fn column_for(&self, pane: u64) -> Option<usize> {
        self.columns
            .iter()
            .position(|column| column.panes.iter().any(|item| item.id == pane))
    }

    /// クリックは置換、隣に開くは挿入。既に見えている worktree は複製せずフォーカスする。
    pub fn open(&mut self, column: WorkColumn, beside: bool) -> u64 {
        if let Some(existing) = self.columns.iter().find(|item| item.space == column.space) {
            let focused = self
                .focused
                .filter(|id| existing.panes.iter().any(|pane| pane.id == *id))
                .unwrap_or(existing.panes[0].id);
            self.focused = Some(focused);
            if self.maximized.is_some() {
                self.maximized = Some(focused);
            }
            return focused;
        }
        let column = self.hidden.remove(&column.space).unwrap_or(column);
        let focused = column.panes[0].id;
        let index = self.focused.and_then(|id| self.column_for(id));
        if let Some(index) = index {
            if beside {
                self.columns.insert(index + 1, column);
            } else {
                let previous = std::mem::replace(&mut self.columns[index], column);
                self.hidden.insert(previous.space.clone(), previous);
            }
        } else {
            self.columns.push(column);
        }
        self.focused = Some(focused);
        if self.maximized.is_some() {
            self.maximized = Some(focused);
        }
        focused
    }

    pub fn close_column(&mut self, pane: u64) {
        let Some(index) = self.column_for(pane) else {
            return;
        };
        let column = self.columns.remove(index);
        let removed_focus = self
            .focused
            .is_some_and(|id| column.panes.iter().any(|p| p.id == id));
        self.hidden.insert(column.space.clone(), column);
        if removed_focus {
            self.focused = self
                .columns
                .get(index.min(self.columns.len().saturating_sub(1)))
                .and_then(|column| column.panes.first())
                .map(|pane| pane.id);
        }
        self.maximized = None;
    }

    pub fn reveal(&mut self, space: &str, surface: WorkSurface) -> Option<u64> {
        let column = self
            .columns
            .iter_mut()
            .find(|column| column.space == space)?;
        if let Some((pane, tab)) = column.panes.iter_mut().find_map(|pane| {
            pane.tabs
                .iter()
                .position(|tab| tab == &surface)
                .map(|index| (pane, index))
        }) {
            pane.active = tab;
            self.focused = Some(pane.id);
            if self.maximized.is_some() {
                self.maximized = Some(pane.id);
            }
            return Some(pane.id);
        }
        let index = column
            .panes
            .iter()
            .position(|pane| Some(pane.id) == self.focused)
            .unwrap_or(0);
        let pane = column.panes.get_mut(index)?;
        pane.tabs.push(surface);
        pane.active = pane.tabs.len() - 1;
        self.focused = Some(pane.id);
        Some(pane.id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct WorkLayoutState {
    pub version: u32,
    pub next_id: u64,
    pub repositories: BTreeMap<String, RepositoryLayout>,
    pub expanded: Vec<String>,
}

impl Default for WorkLayoutState {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            repositories: BTreeMap::new(),
            expanded: Vec::new(),
        }
    }
}

impl WorkLayoutState {
    pub fn allocate(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn column(&mut self, space: String, vertical_tabs: bool) -> WorkColumn {
        WorkColumn {
            space,
            weight: 1.0,
            panes: vec![WorkPane {
                id: self.allocate(),
                tabs: vec![WorkSurface::Agent],
                active: 0,
                vertical_tabs,
                weight: 1.0,
            }],
        }
    }

    /// 保存された比率・選択・ID を正規化。未知バージョンは表示配置だけ初期化する。
    pub fn sanitize(&mut self) {
        if self.version != 1 {
            *self = Self::default();
            return;
        }
        let mut highest = 0;
        let mut ids = std::collections::HashSet::new();
        for layout in self.repositories.values_mut() {
            let mut spaces = std::collections::HashSet::new();
            layout
                .columns
                .retain(|column| spaces.insert(column.space.clone()));
            layout
                .hidden
                .retain(|space, column| space == &column.space && !spaces.contains(space));
            for column in layout.columns.iter_mut().chain(layout.hidden.values_mut()) {
                column.weight = sane_weight(column.weight);
                column
                    .panes
                    .retain(|pane| pane.id > 0 && pane.id < u64::MAX / 2 && ids.insert(pane.id));
                let mut surfaces = Vec::new();
                for pane in &mut column.panes {
                    highest = highest.max(pane.id);
                    pane.weight = sane_weight(pane.weight);
                    pane.tabs.retain(|surface| {
                        if let WorkSurface::Terminal { id } = surface {
                            if *id == 0 || *id >= u64::MAX / 2 {
                                return false;
                            }
                            highest = highest.max(*id);
                        }
                        if surfaces.contains(surface) {
                            false
                        } else {
                            surfaces.push(surface.clone());
                            true
                        }
                    });
                    pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
                }
            }
            layout.columns.retain(|column| !column.panes.is_empty());
            layout.hidden.retain(|_, column| !column.panes.is_empty());
            if layout.focused.is_none_or(|id| layout.pane(id).is_none()) {
                layout.focused = layout
                    .columns
                    .first()
                    .and_then(|c| c.panes.first())
                    .map(|p| p.id);
            }
            if layout.maximized.is_some_and(|id| layout.pane(id).is_none()) {
                layout.maximized = None;
            }
        }
        self.next_id = highest + 1;
    }
}

fn sane_weight(weight: f32) -> f32 {
    if weight.is_finite() {
        weight.clamp(0.2, 8.0)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_restores_tabs_and_beside_does_not_duplicate_worktrees() {
        let mut state = WorkLayoutState::default();
        let a = state.column("main".into(), false);
        let b = state.column("feature".into(), true);
        let mut layout = RepositoryLayout::default();
        let first = layout.open(a.clone(), false);
        layout.reveal("main", WorkSurface::Terminal { id: 20 });
        layout.open(b.clone(), false);
        assert_eq!(layout.columns.len(), 1);
        layout.open(a, true);
        assert_eq!(layout.pane(first).unwrap().tabs.len(), 2);
        layout.open(b, true);
        assert_eq!(layout.columns.len(), 2);
        layout.close_column(first);
        assert_eq!(layout.columns.len(), 1);
        assert_eq!(layout.hidden["main"].panes[0].tabs.len(), 2);
    }

    #[test]
    fn restore_repairs_selection_and_keeps_terminal_identity_without_process_state() {
        let mut state = WorkLayoutState::default();
        let mut column = state.column("space".into(), true);
        column.panes[0].tabs.push(WorkSurface::Terminal { id: 50 });
        column.panes[0].active = 99;
        column.weight = -1.0;
        state
            .repositories
            .entry("repo".into())
            .or_default()
            .open(column, false);
        let json = serde_json::to_string(&state).unwrap();
        let mut restored: WorkLayoutState = serde_json::from_str(&json).unwrap();
        restored.sanitize();
        let pane = &restored.repositories["repo"].columns[0].panes[0];
        assert_eq!(pane.active, 1);
        assert!(pane.vertical_tabs);
        assert_eq!(restored.allocate(), 51);
    }
}
