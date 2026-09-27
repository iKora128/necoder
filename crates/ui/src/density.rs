//! リストの行の詰め具合（O27・`density`・UI-SPEC §1.4）。compact = 既定の高さ・cozy = 4px 高く
//! （23 → 27）。使う側は高さを直書きせず [`row_height`] / [`row_padding`] で引く。workspace が
//! settings.json の `density` から [`RowDensity`] を global に置き、変わったら置き直す。
//! global が無ければ compact。

use gpui::{px, App, Global, Pixels};

/// 行の詰め具合。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowDensity {
    #[default]
    Compact,
    Cozy,
}

impl Global for RowDensity {}

fn density(cx: &App) -> RowDensity {
    cx.try_global::<RowDensity>().copied().unwrap_or_default()
}

/// 行の高さ（`compact` は詰めた時の高さ・cozy は 4px 足す）。
pub fn row_height(cx: &App, compact: f32) -> Pixels {
    px(match density(cx) {
        RowDensity::Compact => compact,
        RowDensity::Cozy => compact + 4.0,
    })
}

/// 行の上下の余白（`compact` は詰めた時の余白・cozy は 2px 足す＝上下で 4px）。
pub fn row_padding(cx: &App, compact: f32) -> Pixels {
    px(match density(cx) {
        RowDensity::Compact => compact,
        RowDensity::Cozy => compact + 2.0,
    })
}
