//! Workspace state.json compatibility and public restore types.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedProject {
    pub(crate) root: PathBuf,
    /// Legacy one-file representation. Read for compatibility, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) open_file: Option<PathBuf>,
    #[serde(default)]
    pub(crate) open_files: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) active_file: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) remote_uri: Option<String>,
}

impl PersistedProject {
    fn files(&self) -> Vec<PathBuf> {
        if self.open_files.is_empty() {
            self.open_file.iter().cloned().collect()
        } else {
            self.open_files.clone()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PersistedState {
    pub(crate) projects: Vec<PersistedProject>,
    #[serde(default)]
    pub(crate) active: usize,
}

/// Standard state.json location on macOS.
pub fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/Shirushi/state.json"))
}

/// Read the legacy `(project roots, active index)` view of saved state.
pub fn load_state(path: &Path) -> Option<(Vec<PathBuf>, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: PersistedState = serde_json::from_str(&text).ok()?;
    if state.projects.is_empty() {
        return None;
    }
    let roots = state.projects.into_iter().map(|project| project.root).collect();
    Some((roots, state.active))
}

#[derive(Debug, Clone)]
pub struct SavedProject {
    pub root: PathBuf,
    pub open_files: Vec<PathBuf>,
    pub active_file: usize,
    pub remote_uri: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RestoredTabs {
    pub files: Vec<PathBuf>,
    pub active: usize,
}

impl RestoredTabs {
    pub fn single(file: PathBuf) -> Self {
        Self {
            files: vec![file],
            active: 0,
        }
    }
}

/// Read local/SSH saved projects while accepting the legacy root-only schema.
pub fn load_saved_state(path: &Path) -> Option<(Vec<SavedProject>, usize)> {
    let text = std::fs::read_to_string(path).ok()?;
    let state: PersistedState = serde_json::from_str(&text).ok()?;
    if state.projects.is_empty() {
        return None;
    }
    let projects = state
        .projects
        .into_iter()
        .map(|project| SavedProject {
            root: project.root.clone(),
            open_files: project.files(),
            active_file: project.active_file,
            remote_uri: project.remote_uri,
        })
        .collect();
    Some((projects, state.active))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_open_file_migrates_to_tabs() {
        let project: PersistedProject = serde_json::from_str(
            r#"{"root":"/tmp/project","open_file":"/tmp/project/a.rs"}"#,
        )
        .unwrap();
        assert_eq!(project.files(), vec![PathBuf::from("/tmp/project/a.rs")]);
    }

    #[test]
    fn open_files_take_precedence_over_legacy_file() {
        let project: PersistedProject = serde_json::from_str(
            r#"{"root":"/tmp/project","open_file":"old.rs","open_files":["a.rs","b.rs"],"active_file":1}"#,
        )
        .unwrap();
        assert_eq!(project.files(), vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
        assert_eq!(project.active_file, 1);
    }
}
