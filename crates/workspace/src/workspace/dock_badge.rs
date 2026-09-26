//! Dock のアイコンの要対応バッジ（O12）。
//!
//! 数え方は titlebar の `◐N`（[`Workspace::attention_badge_count`]）と同じ = 承認待ち・質問待ちの
//! スレッド + 失敗した Task を、**全窓で合計**する。0 件なら消す。隠している時・他のアプリを
//! 触っている時に「呼ばれている」ことが Dock だけで分かる。
//!
//! GPUI に Dock バッジの API は無いので、macOS だけ objc2-app-kit で
//! `NSApplication.dockTile.badgeLabel` を直接書く。他の OS では数えるだけで何もしない。

use crate::workspace::*;

/// 要対応の件数: 承認待ち・質問待ち（Blocked）のスレッド + 失敗した Task。
pub(crate) fn attention_count(
    activities: impl IntoIterator<Item = agent_panel::ThreadActivity>,
    failed_tasks: usize,
) -> usize {
    activities
        .into_iter()
        .filter(|activity| *activity == agent_panel::ThreadActivity::Blocked)
        .count()
        + failed_tasks
}

/// バッジの文字。0 件は出さない（`None` = 消す）。
pub(crate) fn dock_badge_label(total: usize) -> Option<String> {
    (total > 0).then(|| total.to_string())
}

/// いま Dock に出している文字（同じなら AppKit を呼ばない）。
#[derive(Default)]
struct DockBadge {
    shown: Option<String>,
}

impl gpui::Global for DockBadge {}

/// 全窓の要対応を数え直し、変わった時だけ Dock に反映する。スレッドの状態台帳
/// （`RunningRegistry`）が変わるたびと、Task の phase が変わった時に呼ぶ。
///
/// Workspace を読むので、Workspace の update の中からは `cx.defer` で抜けてから呼ぶこと。
pub(crate) fn refresh_dock_badge(cx: &mut App) {
    let total: usize = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Workspace>())
        .filter_map(|handle| {
            handle
                .read(cx)
                .ok()
                .map(|workspace| workspace.attention_badge_count(cx))
        })
        .sum();
    let label = dock_badge_label(total);
    let badge = cx.default_global::<DockBadge>();
    if badge.shown == label {
        return;
    }
    badge.shown = label.clone();
    show_on_dock(label.as_deref());
}

#[cfg(target_os = "macos")]
fn show_on_dock(label: Option<&str>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    // GPUI の前景＝メインスレッド。万一それ以外から来たら AppKit に触らない。
    let Some(main_thread) = MainThreadMarker::new() else {
        eprintln!("Dock バッジはメインスレッドからしか書けない（今回は見送り）");
        return;
    };
    let dock_tile = NSApplication::sharedApplication(main_thread).dockTile();
    let label = label.map(NSString::from_str);
    dock_tile.setBadgeLabel(label.as_deref());
    // 開発用: NECODER_DOCK_BADGE_PROBE=1 で Dock に実際に載った文字を読み戻して出す（撮影では Dock が
    // 写らないので、AppKit まで届いたことはこれで確かめる）。
    #[cfg(debug_assertions)]
    if std::env::var_os("NECODER_DOCK_BADGE_PROBE").is_some() {
        eprintln!(
            "dock badge: {:?}",
            dock_tile.badgeLabel().map(|shown| shown.to_string())
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn show_on_dock(_label: Option<&str>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_panel::ThreadActivity;

    #[test]
    fn counts_waiting_threads_and_failed_tasks() {
        let activities = [
            ThreadActivity::Blocked, // 承認待ち
            ThreadActivity::Blocked, // 質問待ち（同じ Blocked）
            ThreadActivity::Working,
            ThreadActivity::Done { interrupted: false },
            ThreadActivity::Idle,
        ];
        assert_eq!(attention_count(activities, 1), 3);
        assert_eq!(attention_count([ThreadActivity::Working], 0), 0);
    }

    #[test]
    fn zero_clears_the_badge() {
        assert_eq!(dock_badge_label(0), None);
        assert_eq!(dock_badge_label(3).as_deref(), Some("3"));
    }
}
