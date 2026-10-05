//! effort_meter — 設定のカードの思考量の**デジタル・メーター**（UI-SPEC §6・本人が mock で選んだ M-3）。
//!
//! 弧に並んだ細い LED が段に合わせて灯る回転計。先端に向かって明るくなり、先端の LED が光る。値を変えた時
//! だけ、灯る LED が走って少し行き過ぎてから戻る（止まっている間は描き直さない＝idle 0%）。
//!
//! 守ること（ゲージの案を比べた時の約束）:
//! - 段の数はエージェントのまま（広告の select の選択肢・3〜6 段）。目盛りの点は段の区切り
//! - `default`（Claude Code の「エージェント任せ」）は**目盛りに乗せない**＝ LED は全部消え、横の札が灯る
//! - 表示名は広告のまま（necoder は綴りを作らない）
//! - 色相を使わない: 灯った LED と光は前景の色（fg0）、消えた LED は fg2 の薄い色

use acp_client::{ConfigChoice, ConfigOption};
use gpui::{point, px, Bounds, Hsla, PathBuilder, Pixels, Point, Window};

/// 「エージェントの既定」を表す value_id。目盛りの外に置く。
const OFF_SCALE_VALUE_ID: &str = "default";
/// 弧に並べる LED の数（段の数に依らず同じ細かさ）。
pub(super) const SEGMENTS: usize = 34;
/// 弧の始まりの向き（度・右が 0・反時計回りが正）。左下から時計回りに右下まで。
const START_DEGREES: f32 = 215.0;
/// 弧の広さ（度）。下に切れ目を残す。
const SWEEP_DEGREES: f32 = 250.0;
/// メーターの大きさと、その中の弧の中心・半径（px）。
pub(super) const WIDTH: f32 = 112.0;
pub(super) const HEIGHT: f32 = 100.0;
const CENTER: (f32, f32) = (56.0, 54.0);
const RADIUS: f32 = 40.0;
/// LED の内側と外側の半径（中心からの距離）・太さ。
const LED_INNER: f32 = RADIUS - 6.0;
const LED_OUTER: f32 = RADIUS + 4.0;
const LED_WIDTH: f32 = 3.0;
/// 段の区切りの点の半径（中心からの距離）と大きさ。
const MARK_RADIUS: f32 = RADIUS + 10.0;
const MARK_SIZE: f32 = 1.3;
/// 値を変えた時の動きの長さ。
pub(super) const MOTION: std::time::Duration = std::time::Duration::from_millis(560);

/// 思考量の目盛り。`default` は目盛りの外。
pub(super) struct EffortScale {
    /// 目盛りの段（広告の順・`default` を除く）。
    pub(super) levels: Vec<ConfigChoice>,
    /// 目盛りの外の「エージェントの既定」（Claude Code の `default`）。
    pub(super) off_scale: Option<ConfigChoice>,
    /// 今の値が目盛りの何段目か（1 始まり）。`default` と、広告に無い値は `None`。
    pub(super) position: Option<usize>,
}

impl EffortScale {
    /// 広告の思考量の select と、今の value_id から作る。
    pub(super) fn of(config: &ConfigOption, value_id: &str) -> EffortScale {
        let (off, levels): (Vec<&ConfigChoice>, Vec<&ConfigChoice>) = config
            .choices()
            .iter()
            .partition(|choice| choice.value_id == OFF_SCALE_VALUE_ID);
        let position = levels
            .iter()
            .position(|choice| choice.value_id == value_id)
            .map(|index| index + 1);
        EffortScale {
            levels: levels.into_iter().cloned().collect(),
            off_scale: off.into_iter().next().cloned(),
            position,
        }
    }

    /// 灯す割合（0..=1）。目盛りの外（`default`）と広告に無い値は 0。
    pub(super) fn fraction(&self) -> f32 {
        match self.position {
            Some(position) if !self.levels.is_empty() => position as f32 / self.levels.len() as f32,
            _ => 0.0,
        }
    }
}

/// 灯す LED の数。行き過ぎ（1 を超える割合）も端で止める。
pub(super) fn lit_segments(fraction: f32) -> usize {
    (SEGMENTS as f32 * fraction.clamp(0.0, 1.0)).round() as usize
}

/// 値を変えた時の動き: 少し行き過ぎてから戻る（`t` は 0..=1）。
pub(super) fn overshoot(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.70158_f32;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// 押した位置（メーターの左上からの px）に近い段（1 始まり）。弧の下の切れ目と、中心に近すぎる所は `None`。
pub(super) fn level_at(x: f32, y: f32, levels: usize) -> Option<usize> {
    if levels == 0 {
        return None;
    }
    let (dx, dy) = (x - CENTER.0, CENTER.1 - y);
    if dx.hypot(dy) < RADIUS * 0.45 {
        return None;
    }
    let angle = dy.atan2(dx).to_degrees();
    let travel = (START_DEGREES - angle).rem_euclid(360.0);
    // 弧の外（下の切れ目）は、両端から少し（半段ぶん）までは端の段として受ける。
    let slack = SWEEP_DEGREES / levels as f32 / 2.0;
    let travel = if travel > SWEEP_DEGREES {
        if travel <= SWEEP_DEGREES + slack {
            SWEEP_DEGREES
        } else if travel >= 360.0 - slack {
            0.0
        } else {
            return None;
        }
    } else {
        travel
    };
    let level = (travel / SWEEP_DEGREES * levels as f32).ceil() as usize;
    Some(level.clamp(1, levels))
}

/// メーターの色（前景だけ・色相を使わない）。
pub(super) struct MeterColors {
    /// 灯った LED と光（テーマの fg0）。
    pub(super) lit: Hsla,
    /// 消えた LED と区切りの点（テーマの fg2）。
    pub(super) unlit: Hsla,
}

/// メーターを描く。`fraction` は灯す割合（動きの途中は行き過ぎて 1 を超えうる）、`pulse` は先端の光の
/// 強まり（動きの途中だけ 0 より大きい）。
pub(super) fn paint(
    bounds: Bounds<Pixels>,
    window: &mut Window,
    fraction: f32,
    pulse: f32,
    levels: usize,
    colors: &MeterColors,
) {
    let origin = bounds.origin;
    let at = |radius: f32, degrees: f32| -> Point<Pixels> {
        let radians = degrees.to_radians();
        point(
            origin.x + px(CENTER.0 + radius * radians.cos()),
            origin.y + px(CENTER.1 - radius * radians.sin()),
        )
    };
    // 内側の薄い円（ガラスの文字盤）。
    let mut dial = PathBuilder::fill();
    dial.add_polygon(&circle_points(at(0.0, 0.0), RADIUS - 13.0, 40), true);
    if let Ok(path) = dial.build() {
        window.paint_path(path, colors.lit.alpha(0.022));
    }

    let lit = lit_segments(fraction);
    for segment in 0..SEGMENTS {
        let degrees = START_DEGREES - SWEEP_DEGREES * (segment as f32 + 0.5) / SEGMENTS as f32;
        let (inner, outer) = (at(LED_INNER, degrees), at(LED_OUTER, degrees));
        let on = segment < lit;
        if on && segment + 1 == lit {
            // 先端の光（重ね塗りの輪）。動いている間だけ強まる。
            paint_capsule(
                window,
                inner,
                outer,
                14.0,
                colors.lit.alpha(0.10 + pulse * 0.08),
            );
            paint_capsule(
                window,
                inner,
                outer,
                8.0,
                colors.lit.alpha(0.20 + pulse * 0.10),
            );
        }
        let color = if on {
            // 尾から先端へ明るくなる。
            let ramp = (segment as f32 + 1.0) / lit as f32;
            colors.lit.alpha(0.30 + 0.70 * ramp.powf(1.6))
        } else {
            colors.unlit.alpha(0.20)
        };
        paint_capsule(window, inner, outer, LED_WIDTH, color);
    }

    // 段の区切りの点（通り過ぎた区切りは灯る）。
    for level in 1..=levels {
        let boundary = level as f32 / levels as f32;
        let degrees = START_DEGREES - SWEEP_DEGREES * boundary;
        let color = if fraction + 1e-4 >= boundary {
            colors.lit.alpha(0.70)
        } else {
            colors.unlit.alpha(0.45)
        };
        let mut mark = PathBuilder::fill();
        mark.add_polygon(
            &circle_points(at(MARK_RADIUS, degrees), MARK_SIZE, 10),
            true,
        );
        if let Ok(path) = mark.build() {
            window.paint_path(path, color);
        }
    }
}

/// 両端が丸い太線（LED 1 本・光の輪）を多角形で塗る。`PathBuilder::stroke` は端が四角になるため。
fn paint_capsule(
    window: &mut Window,
    from: Point<Pixels>,
    to: Point<Pixels>,
    width: f32,
    color: Hsla,
) {
    let (fx, fy) = (f32::from(from.x), f32::from(from.y));
    let (tx, ty) = (f32::from(to.x), f32::from(to.y));
    let length = (tx - fx).hypot(ty - fy);
    if length <= f32::EPSILON {
        return;
    }
    let radius = width / 2.0;
    let direction = (ty - fy).atan2(tx - fx);
    let cap = |cx: f32, cy: f32, from_angle: f32, points: &mut Vec<Point<Pixels>>| {
        const STEPS: usize = 6;
        for step in 0..=STEPS {
            let angle = from_angle + std::f32::consts::PI * step as f32 / STEPS as f32;
            points.push(point(
                px(cx + radius * angle.cos()),
                px(cy + radius * angle.sin()),
            ));
        }
    };
    let mut points = Vec::with_capacity(16);
    // 先の端は進む向きの左から右へ半周、元の端は右から左へ半周。
    cap(tx, ty, direction - std::f32::consts::FRAC_PI_2, &mut points);
    cap(fx, fy, direction + std::f32::consts::FRAC_PI_2, &mut points);
    let mut builder = PathBuilder::fill();
    builder.add_polygon(&points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// 円を `steps` 角形で近似した頂点。
fn circle_points(center: Point<Pixels>, radius: f32, steps: usize) -> Vec<Point<Pixels>> {
    (0..steps)
        .map(|step| {
            let angle = std::f32::consts::TAU * step as f32 / steps as f32;
            point(
                center.x + px(radius * angle.cos()),
                center.y + px(radius * angle.sin()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client::{ConfigCategory, ConfigKind};

    fn effort(choices: &[&str]) -> ConfigOption {
        ConfigOption {
            config_id: "effort".into(),
            name: "Effort".into(),
            description: None,
            category: ConfigCategory::ThoughtLevel,
            kind: ConfigKind::Select {
                current: choices[0].into(),
                choices: choices
                    .iter()
                    .map(|value_id| ConfigChoice {
                        value_id: (*value_id).into(),
                        name: value_id.to_uppercase(),
                        description: None,
                    })
                    .collect(),
            },
        }
    }

    /// Claude Code の `default` は目盛りに乗せず（割合 0・段は 5）、Codex は 4 段のまま。
    #[test]
    fn default_stays_off_the_scale() {
        let claude = effort(&["default", "low", "medium", "high", "xhigh", "max"]);
        let scale = EffortScale::of(&claude, "default");
        assert_eq!(scale.levels.len(), 5);
        assert_eq!(
            scale
                .off_scale
                .as_ref()
                .map(|choice| choice.value_id.as_str()),
            Some("default")
        );
        assert_eq!(scale.position, None);
        assert_eq!(scale.fraction(), 0.0);
        let high = EffortScale::of(&claude, "high");
        assert_eq!(high.position, Some(3));
        assert!((high.fraction() - 0.6).abs() < 1e-6);

        let codex = effort(&["low", "medium", "high", "xhigh"]);
        let scale = EffortScale::of(&codex, "medium");
        assert!(scale.off_scale.is_none());
        assert_eq!(scale.levels.len(), 4);
        assert!((scale.fraction() - 0.5).abs() < 1e-6);
        assert_eq!(
            EffortScale::of(&codex, "turbo").fraction(),
            0.0,
            "広告に無い値は灯さない（捏造しない）"
        );
    }

    #[test]
    fn segments_follow_the_fraction() {
        assert_eq!(lit_segments(0.0), 0);
        assert_eq!(lit_segments(1.0), SEGMENTS);
        assert_eq!(lit_segments(1.2), SEGMENTS, "行き過ぎは端で止める");
        assert_eq!(lit_segments(-0.1), 0);
        assert_eq!(lit_segments(0.6), 20);
        assert!((overshoot(0.0) - 0.0).abs() < 1e-6);
        assert!((overshoot(1.0) - 1.0).abs() < 1e-6);
        assert!(overshoot(0.7) > 1.0, "途中で少し行き過ぎてから戻る");
    }

    /// 押した向きに近い段を選ぶ。弧の始まり（左下）は 1 段目、終わり（右下）は最後の段、真上は真ん中。
    /// 下の切れ目の真ん中と中心は選ばない。
    #[test]
    fn a_press_picks_the_nearest_level() {
        let toward = |degrees: f32| {
            let radians = degrees.to_radians();
            (
                CENTER.0 + RADIUS * radians.cos(),
                CENTER.1 - RADIUS * radians.sin(),
            )
        };
        let (x, y) = toward(START_DEGREES - 3.0);
        assert_eq!(level_at(x, y, 5), Some(1));
        let (x, y) = toward(START_DEGREES - SWEEP_DEGREES + 3.0);
        assert_eq!(level_at(x, y, 5), Some(5));
        let (x, y) = toward(90.0);
        assert_eq!(level_at(x, y, 5), Some(3));
        let (x, y) = toward(270.0);
        assert_eq!(level_at(x, y, 5), None, "下の切れ目の真ん中");
        assert_eq!(level_at(CENTER.0, CENTER.1, 5), None, "中心");
        assert_eq!(level_at(x, y, 0), None);
    }
}
