//! 確認待ち（UI-SPEC §8）: スレッドの状態を人に見せる面・線・動きと、件数の数え方。
//!
//! 状態ドットの形は覚えてもらえないので、タブと行の面・線・動きで見せる。強さの順は
//! あなた待ち > 未確認 > 生成中 > 確認済み（何も足さない）。色の例外は 2 つだけで、`err` は
//! あなた待ちにだけ、スレッド色の淡い面は未確認のタブにだけ使う（UI-SPEC §1.3）。
//!
//! タブ・スレッド一覧・Fleet の Task 行が同じものを使う。親は `.relative().overflow_hidden()`。
//!
//! **常に動かす物をここに置かない。** `with_animation` は親の View に再描画を要求し、親は AgentPanel
//! （スレッド本体）や Workspace なので、繰り返す動きは止まっている間ずっと CPU と GPU を使う。
//! 別の Entity に分けても祖先は dirty になる（`Window::mark_view_dirty`）ので避けられない。
//! 動かすのは完了の瞬間の 1 回だけで、終われば再描画を要求しない。

use std::ops::AddAssign;
use std::time::Duration;

use gpui::{
    div, px, relative, Animation, AnimationExt, AnyElement, ElementId, Hsla, IntoElement,
    SharedString, Styled,
};

use crate::{Theme, ThreadActivity};

/// 完了の瞬間に 1 回だけ流す光の長さ。
pub(crate) const FINISH_FLASH: Duration = Duration::from_millis(1100);
/// 完了の光のフレームレートの上限（親の再描画の回数を抑える）。
const FLASH_MAX_FPS: f32 = 15.0;

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

/// タブや行の下に敷く、状態の面と線。`animate` が真なら完了の瞬間に 1 回だけ光を流す（窓が前面でない
/// 時と reduce motion では流さない）。`id` はタブや行ごとに一意にする。
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
            layers.push(
                div()
                    .absolute()
                    .inset_0()
                    .bg(theme.err.alpha(0.17))
                    .into_any_element(),
            );
            layers.push(edge_line(edge, theme.err));
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
        // 生成中は状態ドットのスピナーが示す。面も線も足さない。
        ThreadActivity::Working | ThreadActivity::Idle => {}
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
            Animation::new(FINISH_FLASH).with_max_fps(FLASH_MAX_FPS),
            |element, delta| {
                element
                    .left(relative(delta * (1.0 + WIDTH) - WIDTH))
                    .opacity(1.0 - delta)
            },
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{prelude::*, Context, Window};
    use std::cell::Cell;
    use std::rc::Rc;

    /// 親の View（実物では AgentPanel や Workspace）に見立てて、描画の回数を数える。
    struct CountingParent {
        renders: Rc<Cell<usize>>,
        activity: ThreadActivity,
        just_finished: bool,
    }

    impl Render for CountingParent {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
                .relative()
                .size_full()
                .children(thread_state_layers(
                    "probe",
                    &Theme::dark(),
                    gpui::hsla(0.5, 0.5, 0.5, 1.0),
                    self.activity,
                    self.just_finished,
                    StateEdge::Top,
                    true,
                ))
        }
    }

    fn renders_while_idle(
        cx: &mut gpui::TestAppContext,
        activity: ThreadActivity,
        just_finished: bool,
    ) -> usize {
        let renders = Rc::new(Cell::new(0));
        let counter = renders.clone();
        let (_view, vcx) = cx.add_window_view(move |_window, _cx| CountingParent {
            renders: counter,
            activity,
            just_finished,
        });
        vcx.run_until_parked();
        let before = renders.get();
        for _ in 0..50 {
            vcx.executor().advance_clock(Duration::from_millis(100));
            vcx.run_until_parked();
        }
        renders.get() - before
    }

    /// 状態の表示で親を描き直し続けない。親はスレッド本体を含むので、ここで再描画を要求すると
    /// 承認待ちで止まっている間ずっと CPU と GPU を使う（UI-SPEC §8）。
    #[gpui::test]
    fn waiting_and_working_never_redraw_the_parent(cx: &mut gpui::TestAppContext) {
        assert_eq!(
            renders_while_idle(cx, ThreadActivity::Blocked, false),
            0,
            "あなた待ちが親を描き直している"
        );
    }

    #[gpui::test]
    fn working_never_redraws_the_parent(cx: &mut gpui::TestAppContext) {
        assert_eq!(
            renders_while_idle(cx, ThreadActivity::Working, false),
            0,
            "生成中の線が親を描き直している"
        );
    }

    #[gpui::test]
    fn unread_does_not_redraw_the_parent(cx: &mut gpui::TestAppContext) {
        assert_eq!(
            renders_while_idle(cx, ThreadActivity::Done { interrupted: false }, false),
            0
        );
    }

    /// 完了の光は 1 回で終わり、その後は親を描き直さない。光の長さは GPUI が実時間で測るので、
    /// ここだけ実際に待つ。
    #[gpui::test]
    fn the_finish_flash_stops_redrawing_when_it_ends(cx: &mut gpui::TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let counter = renders.clone();
        let (_view, vcx) = cx.add_window_view(move |_window, _cx| CountingParent {
            renders: counter,
            activity: ThreadActivity::Done { interrupted: false },
            just_finished: true,
        });
        let tick = |vcx: &mut gpui::VisualTestContext| {
            vcx.executor().advance_clock(Duration::from_millis(100));
            vcx.run_until_parked();
        };
        vcx.run_until_parked();
        std::thread::sleep(FINISH_FLASH + Duration::from_millis(100));
        tick(vcx);
        tick(vcx);
        let after_flash = renders.get();
        for _ in 0..30 {
            tick(vcx);
        }
        assert_eq!(renders.get(), after_flash, "光が終わったら描き直さない");
    }
}
