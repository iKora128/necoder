//! 確認待ち（UI-SPEC §8）: スレッドの状態を人に見せる面・線・動きと、件数の数え方。
//!
//! 状態ドットの形は覚えてもらえないので、タブと行の面・線・動きで見せる。強さの順は
//! あなた待ち > 未確認 > 生成中 > 確認済み（何も足さない）。色の例外は 2 つだけで、`err` は
//! あなた待ちにだけ、スレッド色の淡い面は未確認のタブにだけ使う（UI-SPEC §1.3）。
//!
//! タブ・スレッド一覧・Fleet の Task 行が同じものを使う。親は `.relative().overflow_hidden()`。

use std::ops::AddAssign;
use std::time::Duration;

use gpui::{
    div, px, relative, Animation, AnimationExt, AnyElement, ElementId, Hsla, IntoElement,
    SharedString, Styled,
};

use crate::{Theme, ThreadActivity};

/// 完了の瞬間に 1 回だけ流す光の長さ。
pub(crate) const FINISH_FLASH: Duration = Duration::from_millis(1100);
/// あなた待ちの面の明滅の周期。
const WAIT_PULSE: Duration = Duration::from_millis(1800);
/// 生成中の線が流れる周期。
const WORK_RUN: Duration = Duration::from_millis(1600);
/// 動かし続ける物のフレームレート（生成中のスピナーと同じ上限で、窓の再描画を増やさない）。
const ANIMATION_MAX_FPS: f32 = 10.0;
/// 生成中の線の幅（親に対する割合）。
const WORK_LINE_WIDTH: f32 = 0.35;

/// 人が次に見るべきものの件数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttentionCounts {
    /// あなた待ち（承認・質問で止まった。見ても消えない）。
    pub waiting: usize,
    /// 未確認（ターンが終わった後、まだ見ていない）。
    pub unread: usize,
    /// 生成中。
    pub working: usize,
}

impl AttentionCounts {
    /// 1 スレッドの状態を足す。
    pub fn add_activity(&mut self, activity: ThreadActivity) {
        match activity {
            ThreadActivity::Blocked => self.waiting += 1,
            ThreadActivity::Done { .. } => self.unread += 1,
            ThreadActivity::Working => self.working += 1,
            ThreadActivity::Idle => {}
        }
    }

    /// 確認待ち（あなた待ち + 未確認）の件数。
    pub fn to_check(self) -> usize {
        self.waiting + self.unread
    }
}

impl AddAssign for AttentionCounts {
    fn add_assign(&mut self, other: Self) {
        self.waiting += other.waiting;
        self.unread += other.unread;
        self.working += other.working;
    }
}

/// 状態の線を置く辺。タブは上、一覧の行は左。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateEdge {
    Top,
    Left,
}

/// 名前を太字で立てる状態か（あなた待ちと未確認）。
pub fn wants_attention(activity: ThreadActivity) -> bool {
    matches!(
        activity,
        ThreadActivity::Blocked | ThreadActivity::Done { .. }
    )
}

/// タブや行の下に敷く、状態の面・線・動き。`animate` が偽なら動かさず同じ位置に止める
/// （窓が前面でない時と reduce motion）。`id` はタブや行ごとに一意にする。
pub fn thread_state_layers(
    id: impl Into<SharedString>,
    theme: &Theme,
    color: Hsla,
    activity: ThreadActivity,
    just_finished: bool,
    edge: StateEdge,
    animate: bool,
) -> Vec<AnyElement> {
    let id: SharedString = id.into();
    let mut layers = Vec::new();
    match activity {
        ThreadActivity::Blocked => {
            let err = theme.err;
            let surface = div().absolute().inset_0();
            layers.push(if animate {
                surface
                    .with_animation(
                        element_id(&id, "wait"),
                        Animation::new(WAIT_PULSE)
                            .repeat()
                            .with_max_fps(ANIMATION_MAX_FPS)
                            .with_easing(gpui::pulsating_between(0.4, 1.0)),
                        move |element, delta| element.bg(err.alpha(0.17 * delta)),
                    )
                    .into_any_element()
            } else {
                surface.bg(err.alpha(0.17)).into_any_element()
            });
            layers.push(edge_line(edge, err));
        }
        ThreadActivity::Done { .. } => {
            layers.push(
                div()
                    .absolute()
                    .inset_0()
                    .bg(match edge {
                        StateEdge::Top => color.alpha(0.15),
                        // 一覧の行は枠のプロジェクト色と競わないよう無彩色にする。
                        StateEdge::Left => theme.fg0.alpha(0.08),
                    })
                    .into_any_element(),
            );
            layers.push(edge_line(edge, color));
            if just_finished && animate {
                layers.push(finish_flash(&id, color));
            }
        }
        ThreadActivity::Working => {
            let line = div()
                .absolute()
                .bottom_0()
                .h(px(2.))
                .w(relative(WORK_LINE_WIDTH))
                .bg(color.alpha(0.75));
            layers.push(if animate {
                line.with_animation(
                    element_id(&id, "work"),
                    Animation::new(WORK_RUN)
                        .repeat()
                        .with_max_fps(ANIMATION_MAX_FPS),
                    |element, delta| {
                        let travel = 1.0 + WORK_LINE_WIDTH;
                        element.left(relative(delta * travel - WORK_LINE_WIDTH))
                    },
                )
                .into_any_element()
            } else {
                line.left_0().into_any_element()
            });
        }
        ThreadActivity::Idle => {}
    }
    layers
}

fn element_id(id: &SharedString, layer: &str) -> ElementId {
    ElementId::Name(SharedString::from(format!("{id}-{layer}")))
}

fn edge_line(edge: StateEdge, color: Hsla) -> AnyElement {
    let line = div().absolute().top_0().left_0().bg(color);
    match edge {
        StateEdge::Top => line.right_0().h(px(2.)).into_any_element(),
        StateEdge::Left => line.bottom_0().w(px(3.)).into_any_element(),
    }
}

/// 完了の瞬間に 1 回だけ、スレッド色の光を左から右へ流す。以後は再描画しない。
fn finish_flash(id: &SharedString, color: Hsla) -> AnyElement {
    const WIDTH: f32 = 0.5;
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(relative(WIDTH))
        .bg(color.alpha(0.32))
        .with_animation(
            element_id(id, "flash"),
            Animation::new(FINISH_FLASH),
            |element, delta| {
                element
                    .left(relative(delta * (1.0 + WIDTH) - WIDTH))
                    .opacity(1.0 - delta)
            },
        )
        .into_any_element()
}
