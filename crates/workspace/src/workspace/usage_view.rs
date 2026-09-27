//! usage_view — 使用量（O11）: statusbar のチップ、押すと出るポップオーバー、統計の画面（Stats）。
//!
//! 値の置き場は `agent_panel::usage::UsageLimits`（エージェントが知らせてきた最後の値・Global）と、
//! 台帳 `turn_usage`（storage）。ここは描くだけで、利用上限を問い合わせない。例外は Codex だけで、
//! ACP に出さない代わりに、ポップオーバーを開いた時に `refresh_codex_limits` が codex 本体へ 1 回訊く。
//!
//! 色（UI-SPEC §1.3）: 上限に近い（80% 以上）= `warn`、上限に達した = `err`。診断の ▲ / ⊗ と同じ意味色で、
//! 同じ Lucide アイコンを添える（色だけに頼らない）。それ以外は中立（fg / bg3）で、識別色は使わない。

use crate::workspace::*;
use agent_panel::usage::{self, AgentLimits, CodexRead, LimitStatus, UsageLimits, WindowReading};

/// ポップオーバーの幅。
const POPOVER_WIDTH: f32 = 440.0;
/// 統計の画面で見る日数（今日を含む）。
const STATS_DAYS: i64 = 30;
const DAY_MS: i64 = 86_400_000;

/// 使用量のポップオーバー（statusbar のチップから開く）。
pub(crate) struct UsagePopoverState {
    /// 左端の x（窓の座標）。開いた時に窓の幅に収まるよう決める。
    left: f32,
    focus: FocusHandle,
    /// 開く前にフォーカスがあった所（閉じたら返す。返さないとエディタへ戻るのにもう 1 回押す）。
    previous_focus: Option<FocusHandle>,
}

/// 統計の画面（パレット「使用量: 統計を開く」）。
pub(crate) struct UsageStatsState {
    focus: FocusHandle,
    rows: StatsRows,
    /// 開く前にフォーカスがあった所（閉じたら返す）。
    previous_focus: Option<FocusHandle>,
}

enum StatsRows {
    Loading,
    Loaded(Vec<storage::DailyUsage>),
    Failed(SharedString),
}

impl Workspace {
    /// statusbar の使用量チップ: いまのスレッドのエージェントの 5 時間枠・週枠（と上限に近い窓）。
    /// 値が無ければ出さない。
    pub(crate) fn render_usage_chip(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        // 値の置き場が無ければ鍵も作らない（描画の手間を増やさない）。
        cx.try_global::<UsageLimits>()?;
        let key = self.agent_panel.read(cx).active_usage_key(cx)?;
        let limits = cx.try_global::<UsageLimits>()?.get(&key)?.clone();
        let now_secs = agent_panel::now_unix_ms() / 1000;
        let headline = limits.headline(now_secs);
        let blocked = limits.blocked(now_secs);
        if headline.is_empty() && !blocked {
            return None;
        }
        let theme = self.theme.clone();
        let (color, icon) = if blocked {
            (theme.err, Some("icons/circle-x.svg"))
        } else if limits.near_limit(now_secs) {
            (theme.warn, Some("icons/triangle-alert.svg"))
        } else {
            (theme.fg1, None)
        };
        let label = if headline.is_empty() {
            SharedString::from(i18n::t!("usage.status_rejected"))
        } else {
            usage::headline_label(&headline)
        };
        Some(
            div()
                .id("statusbar-usage")
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(6.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(color)
                .hover(|style| style.bg(theme.bg2))
                .when_some(icon, |element, icon| {
                    element.child(svg().path(icon).size(px(12.)).flex_none().text_color(color))
                })
                .child(label)
                .tooltip(Tooltip::text(
                    i18n::t!("usage.chip_tip", "agent" => key.label()),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.open_usage_popover(event.position.x, true, window, cx);
                    }),
                )
                .into_any_element(),
        )
    }

    /// ポップオーバーを開く。`read_codex` = Codex を codex 本体に訊く（開いた時だけ・O11）。
    pub(crate) fn open_usage_popover(
        &mut self,
        anchor_x: gpui::Pixels,
        read_codex: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = f32::from(window.viewport_size().width);
        // チップの真上に、チップより少し右まで。窓からはみ出さない。
        let left = (f32::from(anchor_x) - POPOVER_WIDTH + 64.0)
            .min(viewport - POPOVER_WIDTH - 8.0)
            .max(8.0);
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.usage_popover = Some(UsagePopoverState {
            left,
            focus,
            previous_focus,
        });
        if read_codex {
            usage::refresh_codex_limits(false, cx);
        }
        cx.notify();
    }

    /// パレット「使用量: レート制限を表示」: チップと同じポップオーバーを statusbar の右寄りに開く。
    /// チップは値が無いと出ないので、ACP で知らせてこない Codex の値を読む入口にもなる。
    pub(crate) fn show_usage_limits(
        &mut self,
        _: &ShowUsageLimits,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = window.viewport_size().width - px(300.);
        self.open_usage_popover(anchor, true, window, cx);
    }

    fn close_usage_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popover) = self.overlays.usage_popover.take() else {
            return;
        };
        if let Some(previous) = popover.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn on_usage_popover_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_usage_popover(window, cx);
        }
    }

    /// 使用量のポップオーバー: エージェントごとの窓・使用率・リセットまでの時間・受け取った時刻。
    pub(crate) fn render_usage_popover(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.usage_popover.as_ref()?;
        let theme = self.theme.clone();
        let now_ms = agent_panel::now_unix_ms();
        let active_key = self.agent_panel.read(cx).active_usage_key(cx);
        let (mut agents, codex_read, codex_installed) = match cx.try_global::<UsageLimits>() {
            Some(limits) => (
                limits
                    .agents()
                    .map(|(key, limits)| (key.clone(), limits.clone()))
                    .collect::<Vec<_>>(),
                limits.codex.clone(),
                limits.codex_installed == Some(true),
            ),
            None => (Vec::new(), CodexRead::Idle, false),
        };
        // いまのスレッドの鍵を先頭に（残りは鍵の順のまま）。同じエージェントでも場所・置き場が違えば
        // 別の行（R08）。
        agents.sort_by_key(|(key, _)| Some(key) != active_key.as_ref());
        let codex_label = usage::codex_label();

        let mut body = div().flex().flex_col().gap(px(12.));
        for (key, limits) in &agents {
            let codex = key.is_local_codex().then_some(&codex_read);
            body =
                body.child(self.render_usage_agent(&key.label(), Some(limits), codex, now_ms, cx));
        }
        if codex_installed && !agents.iter().any(|(key, _)| key.is_local_codex()) {
            body = body.child(self.render_usage_agent(
                &SharedString::from(codex_label),
                None,
                Some(&codex_read),
                now_ms,
                cx,
            ));
        }
        if agents.is_empty() && !codex_installed {
            body = body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("usage.no_values"))),
            );
        }

        let footer = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .pt(px(10.))
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("usage.note"))),
            )
            .when(codex_installed, |element| {
                element.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("usage.codex_note"))),
                )
            })
            .child(div().flex().child(
                text_chip("usage-open-stats", i18n::t!("usage.open_stats"), &theme).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_usage_stats(&UsageStats, window, cx);
                    }),
                ),
            ));

        let card = div()
            .absolute()
            .left(px(state.left))
            .bottom(px(STATUSBAR_HEIGHT + 6.))
            .w(px(POPOVER_WIDTH))
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(12.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .track_focus(&state.focus)
            .on_key_down(cx.listener(Self::on_usage_popover_key_down))
            // カード内クリックは背景の「閉じる」に伝播させない。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(SharedString::from(i18n::t!("usage.popover_title"))),
            )
            .child(body)
            .child(footer);

        Some(
            div()
                .absolute()
                .inset_0()
                // 外側（statusbar のチップを含む）を押すと閉じる。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_usage_popover(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }

    /// ポップオーバーの 1 エージェント分。`codex` = Codex の読み取りの状態（Codex の時だけ）。
    fn render_usage_agent(
        &self,
        label: &SharedString,
        limits: Option<&AgentLimits>,
        codex: Option<&CodexRead>,
        now_ms: i64,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme.clone();
        let now_secs = now_ms / 1000;
        let status = limits.and_then(|limits| match limits.status {
            Some(LimitStatus::Rejected) if limits.blocked(now_secs) => Some((
                SharedString::from(i18n::t!("usage.status_rejected")),
                theme.err,
            )),
            Some(LimitStatus::Warning) if limits.near_limit(now_secs) => Some((
                SharedString::from(i18n::t!("usage.status_warning")),
                theme.warn,
            )),
            _ => None,
        });
        let received = limits.map(|limits| {
            SharedString::from(i18n::t!(
                "usage.received",
                "when" => agent_panel::relative_time_label(limits.received_at_ms)
            ))
        });
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child(label.clone()),
            )
            .when_some(status, |element, (text, color)| {
                element.child(div().text_size(px(10.5)).text_color(color).child(text))
            })
            .child(div().flex_1())
            .when_some(received, |element, received| {
                element.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(received),
                )
            })
            .when(codex.is_some(), |element| {
                element.child(
                    text_chip("usage-codex-reload", i18n::t!("usage.reload"), &theme)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _window, cx| {
                                cx.stop_propagation();
                                usage::refresh_codex_limits(true, cx);
                            }),
                        ),
                )
            });

        let mut section = div().flex().flex_col().gap(px(5.)).child(header);
        if let Some(limits) = limits {
            for reading in &limits.windows {
                section = section.child(self.render_usage_window(limits, reading, now_ms));
            }
        }
        let codex_line = match codex {
            Some(CodexRead::Loading) => Some(i18n::t!("usage.codex_loading")),
            Some(CodexRead::Failed(error)) => {
                Some(i18n::t!("usage.codex_failed", "error" => error))
            }
            _ => None,
        };
        if let Some(line) = codex_line {
            section = section.child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(line)),
            );
        }
        section
    }

    /// 窓 1 行: 名前・使用率のバー・使用率・リセットまでの時間（古い窓は受け取った時刻も）。
    fn render_usage_window(
        &self,
        limits: &AgentLimits,
        reading: &WindowReading,
        now_ms: i64,
    ) -> gpui::Div {
        let theme = &self.theme;
        let now_secs = now_ms / 1000;
        let reset = reading.is_reset(now_secs);
        let percent = reading.used_percent.filter(|_| !reset);
        let color = match percent {
            Some(percent) if percent >= 100.0 && limits.status == Some(LimitStatus::Rejected) => {
                theme.err
            }
            Some(percent) if usage::is_near_limit(percent) => theme.warn,
            _ => theme.fg1,
        };
        let mut detail = match reading.resets_at {
            Some(_) if reset => i18n::t!("usage.reset_done"),
            Some(resets_at) => usage::resets_in_label(resets_at, now_secs).to_string(),
            None => String::new(),
        };
        // 同じエージェントの他の窓より古い値（代表の窓だけの知らせで更新されなかった窓）は時刻を添える。
        if reading.received_at_ms + 60_000 < limits.received_at_ms {
            let received = i18n::t!(
                "usage.received",
                "when" => agent_panel::relative_time_label(reading.received_at_ms)
            );
            detail = if detail.is_empty() {
                received
            } else {
                format!("{detail} · {received}")
            };
        }
        let track = 64.0_f32;
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(11.5))
            .child(
                div()
                    .w(px(104.))
                    .flex_none()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.fg1)
                    .child(usage::window_label(&reading.window)),
            )
            // 使用率のバー（溝 bg3・伸びる側は中立。上限に近ければ warn / 止められたら err）。
            .child(
                div()
                    .w(px(track))
                    .h(px(4.))
                    .flex_none()
                    .rounded(px(2.))
                    .bg(theme.bg3)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(px(
                                track * (percent.unwrap_or(0.0) as f32 / 100.0).clamp(0.0, 1.0)
                            ))
                            .rounded(px(2.))
                            .bg(color),
                    ),
            )
            .child(
                div()
                    .w(px(36.))
                    .flex_none()
                    .text_right()
                    .text_color(if percent.is_some() { color } else { theme.fg2 })
                    .child(SharedString::from(
                        percent
                            .map(usage::percent_label)
                            .unwrap_or_else(|| "—".to_string()),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(detail)),
            )
    }

    /// パレット「使用量: 統計を開く」。台帳を背景で読んでから表を出す。
    pub(crate) fn open_usage_stats(
        &mut self,
        _: &UsageStats,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // ポップオーバーから来た時は、ポップオーバーを開く前の場所へ返す。
        let previous_focus = match self.overlays.usage_popover.take() {
            Some(popover) => popover.previous_focus,
            None => window.focused(cx),
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let Some(storage) = self.persistence.storage.clone() else {
            self.overlays.usage_stats = Some(UsageStatsState {
                focus,
                rows: StatsRows::Loaded(Vec::new()),
                previous_focus,
            });
            cx.notify();
            return;
        };
        self.overlays.usage_stats = Some(UsageStatsState {
            focus,
            rows: StatsRows::Loading,
            previous_focus,
        });
        cx.notify();
        // 日付の境目はローカルの 0 時。今日を含む直近 30 日。
        let offset_ms = chat_core::date::local_offset_seconds() * 1000;
        let today = (agent_panel::now_unix_ms() + offset_ms).div_euclid(DAY_MS);
        let since_ms = (today - (STATS_DAYS - 1)) * DAY_MS - offset_ms;
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { storage.daily_usage(since_ms, offset_ms) })
                .await;
            let updated = workspace.update(cx, |workspace, cx| {
                if let Some(state) = workspace.overlays.usage_stats.as_mut() {
                    state.rows = match result {
                        Ok(rows) => StatsRows::Loaded(rows),
                        Err(error) => StatsRows::Failed(SharedString::from(format!("{error:#}"))),
                    };
                    cx.notify();
                }
            });
            if let Err(error) = updated {
                eprintln!("使用量の統計を出せない（窓が閉じた）: {error:#}");
            }
        })
        .detach();
    }

    fn close_usage_stats(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stats) = self.overlays.usage_stats.take() else {
            return;
        };
        if let Some(previous) = stats.previous_focus {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn on_usage_stats_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_usage_stats(window, cx);
        }
    }

    /// 統計の画面: 日付 × エージェントのターン数・トークン・コスト（直近 30 日・necoder の台帳の分だけ）。
    pub(crate) fn render_usage_stats(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.usage_stats.as_ref()?;
        let theme = self.theme.clone();
        let message = |text: String| {
            div()
                .py(px(18.))
                .text_size(px(12.))
                .text_color(theme.fg2)
                .child(SharedString::from(text))
        };
        let content = match &state.rows {
            StatsRows::Loading => message(i18n::t!("usage.stats_loading")),
            StatsRows::Failed(error) => message(i18n::t!("usage.stats_failed", "error" => error)),
            StatsRows::Loaded(rows) if rows.is_empty() => message(i18n::t!("usage.stats_empty")),
            StatsRows::Loaded(rows) => self.render_usage_table(rows),
        };

        let panel = div()
            .w(px(660.))
            .max_h(px(620.))
            .flex()
            .flex_col()
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .rounded(px(12.))
            .overflow_hidden()
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(10.),
                gpui::hsla(0., 0., 0., 0.45),
            )
            .blur_radius(px(28.))])
            .track_focus(&state.focus)
            .on_key_down(cx.listener(Self::on_usage_stats_key_down))
            // パネル内クリックは背景の「閉じる」に伝播させない。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _window, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .px(px(18.))
                    .pt(px(16.))
                    .pb(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(SharedString::from(i18n::t!("usage.stats_title"))),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("usage.stats_subtitle"))),
                    ),
            )
            .child(
                div()
                    .id("usage-stats-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(18.))
                    .child(content),
            )
            .child(
                div()
                    .px(px(18.))
                    .pt(px(10.))
                    .pb(px(14.))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("usage.stats_note"))),
            );

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.35))
                // 背景クリックで閉じる。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_usage_stats(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    /// 表: 30 日の合計（エージェントごと）→ 日ごとの行（新しい日から・同じ日はエージェント名順）。
    fn render_usage_table(&self, rows: &[storage::DailyUsage]) -> gpui::Div {
        let theme = &self.theme;
        let totals = usage_totals(rows);
        let max_tokens = rows
            .iter()
            .map(|row| row.total_tokens)
            .max()
            .unwrap_or(0)
            .max(1);
        let header = |text: String, width: Option<f32>, right: bool| {
            let cell = div()
                .text_size(px(10.5))
                .text_color(theme.fg2)
                .when(right, |cell| cell.text_right())
                .child(SharedString::from(text));
            match width {
                Some(width) => cell.w(px(width)).flex_none(),
                None => cell.flex_1().min_w_0(),
            }
        };
        let mut table = div().flex().flex_col().gap(px(2.)).pb(px(12.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(4.))
                .border_b_1()
                .border_color(theme.border)
                .child(header(i18n::t!("usage.stats_date"), Some(96.), false))
                .child(header(i18n::t!("usage.stats_agent"), None, false))
                .child(header(i18n::t!("usage.stats_turns"), Some(52.), true))
                .child(header(i18n::t!("usage.stats_tokens"), Some(150.), true))
                .child(header(i18n::t!("usage.stats_cost"), Some(76.), true)),
        );
        for (index, total) in totals.iter().enumerate() {
            let label = (index == 0).then(|| i18n::t!("usage.stats_total"));
            table = table.child(self.render_usage_row(label, total, None, true));
        }
        let mut previous_day = None;
        for (index, row) in rows.iter().enumerate() {
            let date = (previous_day != Some(row.day))
                .then(|| chat_core::date::Date::from_unix_utc(row.day * 86_400).to_string());
            let first_of_day = date.is_some();
            previous_day = Some(row.day);
            let bar = row.total_tokens as f32 / max_tokens as f32;
            table = table.child(
                self.render_usage_row(date, row, Some(bar), false)
                    .when(first_of_day && index == 0, |element| element.mt(px(6.)))
                    .when(first_of_day && index > 0, |element| {
                        element
                            .mt(px(4.))
                            .pt(px(4.))
                            .border_t_1()
                            .border_color(theme.border)
                    }),
            );
        }
        table
    }

    /// 表の 1 行。`bar` = トークンの棒の長さ（その期間の最大に対する割合・合計の行は無し）。
    fn render_usage_row(
        &self,
        date: Option<String>,
        row: &storage::DailyUsage,
        bar: Option<f32>,
        emphasized: bool,
    ) -> gpui::Div {
        let theme = &self.theme;
        let track = 56.0_f32;
        let text = if emphasized { theme.fg0 } else { theme.fg1 };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(2.))
            .text_size(px(11.5))
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .text_color(theme.fg2)
                    .child(SharedString::from(date.unwrap_or_default())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(text)
                    .child(SharedString::from(row.agent.clone())),
            )
            .child(
                div()
                    .w(px(52.))
                    .flex_none()
                    .text_right()
                    .text_color(text)
                    .child(SharedString::from(row.turns.to_string())),
            )
            .child(
                div()
                    .w(px(150.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .when_some(bar, |cell, bar| {
                        // トークンの棒（溝 bg3・伸びる側 fg2。色相は使わない）。
                        cell.child(
                            div()
                                .w(px(track))
                                .h(px(4.))
                                .rounded(px(2.))
                                .bg(theme.bg3)
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(px(track * bar.clamp(0.0, 1.0)))
                                        .rounded(px(2.))
                                        .bg(theme.fg2),
                                ),
                        )
                    })
                    // 数字の幅を揃えて、棒の位置を列で揃える。
                    .child(
                        div()
                            .w(px(58.))
                            .flex_none()
                            .text_right()
                            .text_color(text)
                            .child(SharedString::from(usage::tokens_label(row.total_tokens))),
                    ),
            )
            .child(
                div()
                    .w(px(76.))
                    .flex_none()
                    .text_right()
                    .text_color(if row.cost_usd.is_some() {
                        text
                    } else {
                        theme.fg2
                    })
                    .child(SharedString::from(
                        row.cost_usd
                            .map(usage::usd_label)
                            .unwrap_or_else(|| "—".to_string()),
                    )),
            )
    }
}

/// エージェントごとの期間の合計（ターン・トークン・コスト）。コストはどこかの日にあれば合計、無ければ `None`。
fn usage_totals(rows: &[storage::DailyUsage]) -> Vec<storage::DailyUsage> {
    let mut totals: Vec<storage::DailyUsage> = Vec::new();
    for row in rows {
        match totals.iter_mut().find(|total| total.agent == row.agent) {
            Some(total) => {
                total.turns += row.turns;
                total.total_tokens += row.total_tokens;
                total.cost_usd = match (total.cost_usd, row.cost_usd) {
                    (Some(left), Some(right)) => Some(left + right),
                    (left, right) => left.or(right),
                };
            }
            None => totals.push(storage::DailyUsage {
                day: 0,
                ..row.clone()
            }),
        }
    }
    totals.sort_by(|left, right| left.agent.cmp(&right.agent));
    totals
}

/// 枠付きの小さな文字チップ（SSH の「再接続」と同じ様式・アイコン無し）。
fn text_chip(id: &'static str, label: String, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(7.))
        .py(px(1.))
        .rounded(px(5.))
        .border_1()
        .border_color(theme.border)
        .text_size(px(10.5))
        .text_color(theme.fg1)
        .cursor_pointer()
        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
        .child(SharedString::from(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daily(
        day: i64,
        agent: &str,
        turns: i64,
        tokens: i64,
        cost: Option<f64>,
    ) -> storage::DailyUsage {
        storage::DailyUsage {
            day,
            agent: agent.into(),
            turns,
            total_tokens: tokens,
            cost_usd: cost,
        }
    }

    /// 統計の画面は台帳を背景で読み（開いた直後は「読み込み中」）、閉じたら開く前のフォーカスへ返す。
    /// ポップオーバーから開いた時も、ポップオーバーを開く前の場所へ返す。
    #[gpui::test]
    fn stats_load_in_the_background_and_return_focus_on_close(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "necoder_workspace_usage_stats_{}_{}",
            std::process::id(),
            agent_panel::now_unix_ms()
        ));
        std::fs::create_dir_all(&root).expect("作業ディレクトリを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let storage = storage::Storage::open(&root.join("necoder.db")).expect("DB を開ける");
        storage
            .record_turn_usage(&storage::TurnUsageRecord {
                thread_id: "thread-1".into(),
                agent: "Claude Code".into(),
                ended_at: agent_panel::now_unix_ms(),
                total_tokens: 1_000,
                cost_usd: Some(0.5),
                ..storage::TurnUsageRecord::default()
            })
            .expect("書ける");
        let sources = vec![ProjectSource::new(host::LocalHost::shared(), root.clone())];
        let (workspace, cx) = cx.add_window_view(|_window, cx| {
            Workspace::new_sources(sources, Theme::dark(), None, cx)
        });
        cx.run_until_parked();

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.persistence.storage = Some(storage.clone());
            let rail = workspace.chrome.rail_focus.clone();
            window.focus(&rail, cx);
            // ポップオーバー → 「統計を開く」の順で開く（Codex は訊かない）。
            workspace.open_usage_popover(px(800.), false, window, cx);
            workspace.open_usage_stats(&UsageStats, window, cx);
            assert!(
                workspace.overlays.usage_popover.is_none(),
                "統計を開けば閉じる"
            );
            assert!(matches!(
                workspace
                    .overlays
                    .usage_stats
                    .as_ref()
                    .map(|stats| &stats.rows),
                Some(StatsRows::Loading)
            ));
        });
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            match workspace
                .overlays
                .usage_stats
                .as_ref()
                .map(|stats| &stats.rows)
            {
                Some(StatsRows::Loaded(rows)) => {
                    assert_eq!(rows.len(), 1, "{rows:?}");
                    assert_eq!(rows[0].total_tokens, 1_000);
                }
                _ => panic!("台帳を読み終えている"),
            }
            workspace.close_usage_stats(window, cx);
            assert!(workspace.overlays.usage_stats.is_none());
            assert!(
                workspace.chrome.rail_focus.is_focused(window),
                "ポップオーバーを開く前の場所へフォーカスを返す"
            );
            for session in workspace.project_sessions.sessions.iter_mut() {
                session._watch = None;
                session._watch_pump = None;
            }
        });
        std::fs::remove_dir_all(&root).expect("片付けられる");
    }

    /// 期間の合計はエージェントごと。コストはある日だけ足し、どの日にも無ければ無し（0 と区別する）。
    #[test]
    fn totals_sum_per_agent_and_keep_unknown_cost_unknown() {
        let totals = usage_totals(&[
            daily(10, "Claude Code", 2, 4_000, Some(0.75)),
            daily(10, "Codex", 1, 7_000, None),
            daily(9, "Claude Code", 1, 500, None),
            daily(8, "Codex", 3, 1_000, None),
        ]);
        assert_eq!(
            totals,
            vec![
                daily(0, "Claude Code", 3, 4_500, Some(0.75)),
                daily(0, "Codex", 4, 8_000, None),
            ]
        );
    }
}
