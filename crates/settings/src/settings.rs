//! settings — settings_core（純ロジック・GPUI 非依存）を GPUI アプリに載せる**反応的レイヤ**。
//!
//! 設計（ユーザー確定 2026-07-12）: **settings.json を唯一の真実**にし、UI トグル / CLI / MCP / 手編集は
//! すべてこの 1 つの store の「書き手」にする。3つを別々に作るとズレるので 1 本に集約する。
//! - 真実: [`settings_core::SettingsStore`]（default→user→project の3層マージ）
//! - 反応: [`SettingsGlobal`]（gpui `Global`）。ビューは `cx.observe_global::<SettingsGlobal>` で変化に反応
//! - 監視: user/project の `settings.json` の mtime を ~1.2s poll し、**実際に解決値が変わった時だけ**
//!   `update_global`＝無変化では observer を起こさない（**idle 0% を保つ**）。手編集・別プロセス CLI もこれで反映
//! - in-proc（UI トグル・in-proc MCP）は [`set_user_value`] で即時反映 + 永続化（poll を待たない）
//!
//! 監視は poll（mtime 差分）。真の event-driven（FSEvents/notify）へは後で差し替え可能だが、
//! 2 回の stat / 1.2s は事実上 0% で再描画も起こさない（変化時のみ）。

use gpui::{App, BorrowAppContext, Global};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub use settings_core::{
    Density, Settings, SettingsStore, persist_user_value, user_settings_path,
};

/// poll 間隔。手編集・CLI の反映がこの遅延内に起きる（in-proc は即時なので影響しない）。
const POLL_INTERVAL: Duration = Duration::from_millis(1200);

/// アプリ全体で共有する設定の真実（single source of truth）。
/// UI トグル・CLI・MCP・手編集がすべてここを更新し、`observe_global` で全ビューへ波及する。
pub struct SettingsGlobal {
    store: SettingsStore,
    user_path: Option<PathBuf>,
    project_dir: Option<PathBuf>,
}

impl Global for SettingsGlobal {}

impl SettingsGlobal {
    /// 現在の解決済み設定。
    pub fn settings(&self) -> &Settings {
        self.store.settings()
    }

    /// ファイル群から読み直して store を差し替える。
    fn reload(&mut self) {
        self.store = SettingsStore::load(self.user_path.as_deref(), self.project_dir.as_deref());
    }
}

/// 設定を読み込み、グローバルに載せ、ファイル監視（poll）を開始する。main が起動時に 1 回呼ぶ。
/// `project_dir` があれば `.shirushi/settings.json` も監視・マージ対象になる。
pub fn init(user_path: Option<PathBuf>, project_dir: Option<PathBuf>, cx: &mut App) {
    let store = SettingsStore::load(user_path.as_deref(), project_dir.as_deref());
    cx.set_global(SettingsGlobal {
        store,
        user_path: user_path.clone(),
        project_dir: project_dir.clone(),
    });
    spawn_watcher(user_path, project_dir, cx);
}

/// 現在の解決済み設定をクローンで取る。ビューは `cx.observe_global::<SettingsGlobal>` で変化に反応する。
/// グローバル未設定（init 前）でも安全に既定を返す。
pub fn get(cx: &App) -> Settings {
    cx.try_global::<SettingsGlobal>()
        .map(|global| global.settings().clone())
        .unwrap_or_default()
}

/// user 設定の 1 キーを更新して**即適用 + 永続化**する（UI トグル・in-proc MCP から）。
/// 書き込み→再読込で observer が発火し、全ビューへ波及する。poll は待たない。
pub fn set_user_value(cx: &mut App, key: &str, value: serde_json::Value) {
    let path = cx.try_global::<SettingsGlobal>().and_then(|global| global.user_path.clone());
    if let Some(path) = path {
        if let Err(error) = persist_user_value(&path, key, value) {
            eprintln!("設定の保存に失敗（実行時のみ反映）: {error:#}");
        }
    }
    if cx.has_global::<SettingsGlobal>() {
        cx.update_global::<SettingsGlobal, _>(|global, _| global.reload());
    }
}

/// user/project の settings.json を poll 監視し、**解決値が実際に変わった時だけ** global を更新する。
fn spawn_watcher(user_path: Option<PathBuf>, project_dir: Option<PathBuf>, cx: &mut App) {
    let project_file = project_settings_path(project_dir.as_deref());
    cx.spawn(async move |cx| {
        let mut seen_user = mtime(user_path.as_deref());
        let mut seen_project = mtime(project_file.as_deref());
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let now_user = mtime(user_path.as_deref());
            let now_project = mtime(project_file.as_deref());
            if now_user == seen_user && now_project == seen_project {
                continue; // 無変化 → observer を起こさない（idle 0% を保つ）
            }
            seen_user = now_user;
            seen_project = now_project;
            // mtime は変わった。ただし**解決値が本当に変わった時だけ** update_global で observer を発火。
            // （touch や無関係な再保存で無駄に再描画しないため。）app 終了後はこの task 自体が
            // 実行されない（timer await で止まったまま drop される）ので liveness チェックは不要。
            cx.update(|cx| {
                if !cx.has_global::<SettingsGlobal>() {
                    return;
                }
                let global = cx.global::<SettingsGlobal>();
                let fresh =
                    SettingsStore::load(global.user_path.as_deref(), global.project_dir.as_deref());
                let differs = fresh.settings() != global.settings();
                if differs {
                    cx.update_global::<SettingsGlobal, _>(|global, _| global.store = fresh);
                }
            });
        }
    })
    .detach();
}

/// プロジェクト設定ファイルのパス（`<project>/.shirushi/settings.json`）。
fn project_settings_path(project_dir: Option<&Path>) -> Option<PathBuf> {
    Some(project_dir?.join(".shirushi").join("settings.json"))
}

/// ファイルの最終更新時刻（無ければ `None`）。
fn mtime(path: Option<&Path>) -> Option<SystemTime> {
    std::fs::metadata(path?).ok()?.modified().ok()
}
