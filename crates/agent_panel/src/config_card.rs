//! config_card — composer の下の**設定のカード**（UI-SPEC §6・2026-10-02）。
//!
//! 今まで Agent / Model / Effort の 3 つのピルだった所を、**チップ 1 つ → カード 1 枚**にする。カードは
//! エージェントが広告した設定（ACP `configOptions`）を select も boolean も全部並べる（Fast mode・
//! Codex の Collaboration mode 等）。形は本人が `mock/config-card.html` で選んだ C-1 で、**権限モードは
//! カードの外のピルに残す**（安全に関わる＝常に文字で見え、1 回で戻せる）。
//!
//! 守る決まりは今のピルと同じ:
//! - 保存・比較・送信は value_id（boolean は true / false）。表示名は描画にしか使わない
//! - 広告が届くまで候補を捏造しない。同じ agent が一度広告した物（在庫）は使う
//! - エージェントは会話を始める前だけ変えられる（Tab / 一覧）。Chat と席のスレッドでは変えない
//! - 色相を使わない（選択と印は bg3・つまみは fg0・Fast mode の稲妻も線の絵）
//!
//! エージェントの行の名前の右は、issue #38 H4 の「接続」の欄が入る場所（今は何も描かない）。

use super::*;
use gpui::Div;

/// Claude Code / codex-acp が Fast mode に付ける config_id。チップでは名前の代わりに稲妻の絵で出す。
const FAST_CONFIG_ID: &str = "fast";
/// 「エージェントの既定」を表す value_id（Claude Code の Model / Effort・Codex の Collaboration mode）。
/// これを選んでいる設定は、チップの要約に出さない。
const DEFAULT_VALUE_ID: &str = "default";
/// チップの要約の最大幅（長い要約は省略し、全文はツールチップ）。
const CHIP_SUMMARY_MAX_WIDTH: f32 = 250.0;
/// カードの幅。Agent ドックの最小幅（320）でもはみ出さない（チップは composer の左端から ~25px）。
const CARD_WIDTH: f32 = 290.0;
/// 一覧の最大の高さ（印に合わせてスクロールする）。
const CARD_LIST_MAX_HEIGHT: f32 = 236.0;

/// 開いているカード（閉じている間は `AgentPanel::config_card` が `None`）。
pub(crate) struct ConfigCard {
    page: CardPage,
    /// 一覧でキーが動かす印（行の添字）。
    cursor: usize,
    /// カードがキーを受けるフォーカス。開いた時に移し、閉じたら composer へ返す。
    focus: FocusHandle,
    /// 一覧のスクロール（印に合わせて動かす）。
    scroll: ScrollHandle,
}

#[cfg(test)]
impl ConfigCard {
    /// 試験用: いまの面の名前。
    pub(crate) fn page_name(&self) -> &'static str {
        match self.page {
            CardPage::Main => "main",
            CardPage::Models => "models",
            CardPage::Agents => "agents",
            CardPage::Choices(_) => "choices",
        }
    }

    /// 試験用: 一覧の印の位置。
    pub(crate) fn cursor_index(&self) -> usize {
        self.cursor
    }
}

/// カードの中で見せている面。
#[derive(Debug, Clone, PartialEq)]
enum CardPage {
    /// エージェント・モデル・思考量・その他の設定。
    Main,
    /// モデルの一覧。
    Models,
    /// エージェントの一覧（会話の前だけ）。
    Agents,
    /// その他の select の設定の選択肢（ACP の config_id）。
    Choices(String),
}

/// 一覧の 1 行。
struct ListRow {
    /// 決めた時に使う値（モデル・設定は value_id、エージェントはラベル）。
    value: SharedString,
    label: SharedString,
    /// エージェントが付けた説明（無ければ出さない）。
    detail: Option<SharedString>,
}

/// 思考量を動かす向き（←→ / Home・End）。
#[derive(Clone, Copy)]
enum EffortStep {
    Previous,
    Next,
    First,
    Last,
}

/// チップの要約の 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SummaryPart {
    Text(SharedString),
    /// Fast mode が on（稲妻の線の絵）。
    Fast,
}

/// チップの要約（**既定から外れた値だけ**・UI-SPEC §6）。モデル → 思考量 → その他の select →
/// on の boolean の順で、Fast mode は末尾の稲妻。空なら呼び手がエージェント名を出す。
///
/// - モデルと思考量は、value_id が `default` の時だけ出さない。`default` を広告しないエージェント
///   （Codex）ではいつも出る＝既定かどうか分からない値を隠さない
/// - その他の select は、`default` を広告していてそれ以外を選んでいる時だけ出す
/// - boolean は on の時だけ（Fast mode は稲妻、ほかは設定の名前）
/// - 広告がまだ無い時は、覚えている value_id をそのまま出す（表示名を作らない）
pub(crate) fn chip_summary(thread: &Thread, configs: &[ConfigOption]) -> Vec<SummaryPart> {
    let mut parts = Vec::new();
    for category in [ConfigCategory::Model, ConfigCategory::ThoughtLevel] {
        let config = configs.iter().find(|config| config.category == category);
        let value = card_value(thread, category, config);
        if value.is_empty() || value == DEFAULT_VALUE_ID {
            continue;
        }
        let label = config
            .and_then(|config| choice_name(config, &value))
            .unwrap_or_else(|| SharedString::from(value));
        parts.push(SummaryPart::Text(label));
    }
    let mut fast = false;
    for config in configs
        .iter()
        .filter(|config| config.category == ConfigCategory::Other)
    {
        match option_value(thread, config) {
            ConfigValue::Bool(true) if config.config_id == FAST_CONFIG_ID => fast = true,
            ConfigValue::Bool(true) => parts.push(SummaryPart::Text(config.name.clone().into())),
            ConfigValue::Bool(false) => {}
            ConfigValue::Id(value_id) => {
                let offers_default = config
                    .choices()
                    .iter()
                    .any(|choice| choice.value_id == DEFAULT_VALUE_ID);
                if offers_default && value_id != DEFAULT_VALUE_ID {
                    if let Some(name) = choice_name(config, &value_id) {
                        parts.push(SummaryPart::Text(name));
                    }
                }
            }
        }
    }
    if fast {
        parts.push(SummaryPart::Fast);
    }
    parts
}

/// モデル・思考量の今の値（value_id）。スレッドが選んでいればそれ、まだなら広告の今の値。
fn card_value(thread: &Thread, category: ConfigCategory, config: Option<&ConfigOption>) -> String {
    let wanted = match category {
        ConfigCategory::Model => &thread.model,
        ConfigCategory::ThoughtLevel => &thread.effort,
        ConfigCategory::Mode | ConfigCategory::Other => return String::new(),
    };
    match config {
        Some(config) if wanted.is_empty() => config.current().to_stored(),
        _ => wanted.to_string(),
    }
}

/// value_id を広告の表示名へ引く（描画専用）。広告に無ければ `None`。
fn choice_name(config: &ConfigOption, value_id: &str) -> Option<SharedString> {
    config
        .choices()
        .iter()
        .find(|choice| choice.value_id == value_id)
        .map(|choice| SharedString::from(choice.name.clone()))
}

/// select の選択肢を一覧の行にする。
fn select_rows(config: Option<&ConfigOption>) -> Vec<ListRow> {
    config
        .map(|config| {
            config
                .choices()
                .iter()
                .map(|choice| ListRow {
                    value: SharedString::from(choice.value_id.clone()),
                    label: SharedString::from(choice.name.clone()),
                    detail: choice.description.clone().map(SharedString::from),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 要約を 1 行の文にする（ツールチップ用・稲妻は設定の名前で書く）。
fn summary_text(parts: &[SummaryPart], configs: &[ConfigOption], agent: &str) -> String {
    if parts.is_empty() {
        return agent.to_string();
    }
    let fast_name = configs
        .iter()
        .find(|config| config.config_id == FAST_CONFIG_ID)
        .map(|config| config.name.as_str())
        .unwrap_or(FAST_CONFIG_ID);
    parts
        .iter()
        .map(|part| match part {
            SummaryPart::Text(text) => text.as_ref(),
            SummaryPart::Fast => fast_name,
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Fast mode の稲妻（色付きの絵文字 ⚡ は色相を持ち込むので使わない）。
fn fast_icon(size: f32, color: Hsla) -> impl IntoElement {
    svg()
        .path("icons/zap.svg")
        .size(px(size))
        .flex_none()
        .text_color(color)
}

impl AgentPanel {
    /// ⌘/（composer と開いたカードの中・keymap の `AgentPanel > (Editor || ConfigCard)`）。
    /// 開いていれば閉じる。IME の変換中は開かない（変換中のキーを奪わない）。
    pub(crate) fn on_toggle_config_card(
        &mut self,
        _: &ToggleConfigCard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.config_card.is_some() {
            self.close_config_card(window, cx);
        } else if !self.composer.read(cx).has_marked_text() {
            self.open_config_card(window, cx);
        }
    }

    /// カードに出すこのスレッドの広告（スレッド自身か在庫・捏造しない）。
    fn card_configs(&self) -> &[ConfigOption] {
        self.threads
            .get(self.active)
            .map(|thread| self.stocked_configs(thread))
            .unwrap_or_default()
    }

    /// このスレッドのエージェントをカードで変えられるか。会話の前だけ（同じ transcript に別の AI の文脈を
    /// 混ぜない）・Chat はエージェントを選ばない・席のスレッドは necoder が決める。
    fn card_agent_switchable(&self) -> bool {
        !self.chat_mode
            && !self.agent_selector_locked()
            && self
                .threads
                .get(self.active)
                .is_some_and(|thread| thread.seat.is_none())
    }

    /// カードを開けるか: 選べる物が 1 つでもある（広告か、会話の前のエージェント）。無ければチップは
    /// 押せない（押せるのに何も出ない面を作らない）。
    pub(crate) fn config_card_openable(&self) -> bool {
        self.card_agent_switchable() || !self.card_configs().is_empty()
    }

    /// カードを開いてキーをカードへ移す。ほかの浮かぶ面（ピルのメニュー・＋context）とは排他。
    pub(crate) fn open_config_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.config_card_openable() {
            return;
        }
        self.open_menu = None;
        self.context_menu_open = false;
        self.context_focus = None;
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.config_card = Some(ConfigCard {
            page: CardPage::Main,
            cursor: 0,
            focus,
            scroll: ScrollHandle::new(),
        });
        cx.notify();
    }

    /// カードを閉じて composer へフォーカスを返す（Esc・⌘/）。
    pub(crate) fn close_config_card(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_card.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    /// カードにフォーカスがあるか（プロジェクト切替でフォーカスを追う判定に足す）。
    pub(crate) fn config_card_focused(&self, window: &Window, cx: &App) -> bool {
        self.config_card
            .as_ref()
            .is_some_and(|card| card.focus.contains_focused(window, cx))
    }

    /// 一覧の行（モデル / エージェント / select の選択肢）。
    fn card_rows(&self, page: &CardPage, cx: &App) -> Vec<ListRow> {
        let configs = self.card_configs();
        match page {
            CardPage::Main => Vec::new(),
            CardPage::Models => select_rows(
                configs
                    .iter()
                    .find(|config| config.category == ConfigCategory::Model),
            ),
            CardPage::Choices(config_id) => select_rows(configs.iter().find(|config| {
                config.config_id == *config_id && config.category == ConfigCategory::Other
            })),
            CardPage::Agents => self
                .selector_choices(Selector::Agent, cx)
                .into_iter()
                .map(|choice| ListRow {
                    value: choice.value,
                    label: choice.label,
                    detail: None,
                })
                .collect(),
        }
    }

    /// 一覧の今の値（✓ を付ける行の値）。
    fn card_current(&self, page: &CardPage) -> Option<SharedString> {
        let thread = self.threads.get(self.active)?;
        let configs = self.stocked_configs(thread);
        match page {
            CardPage::Main => None,
            CardPage::Models => {
                let config = configs
                    .iter()
                    .find(|config| config.category == ConfigCategory::Model);
                Some(card_value(thread, ConfigCategory::Model, config).into())
            }
            CardPage::Agents => Some(thread.agent.clone()),
            CardPage::Choices(config_id) => {
                let config = configs
                    .iter()
                    .find(|config| config.config_id == *config_id)?;
                match option_value(thread, config) {
                    ConfigValue::Id(value_id) => Some(value_id.into()),
                    ConfigValue::Bool(_) => None,
                }
            }
        }
    }

    /// 一覧を開く。印は今の値から `offset` 行ずらした所（↑↓ で開いた時は隣のモデル）。
    fn open_card_list(&mut self, page: CardPage, offset: isize, cx: &mut Context<Self>) {
        let rows = self.card_rows(&page, cx);
        if rows.is_empty() {
            return;
        }
        let current = self.card_current(&page);
        let at = current
            .and_then(|current| rows.iter().position(|row| row.value == current))
            .unwrap_or(0);
        let cursor = (at as isize + offset).clamp(0, rows.len() as isize - 1) as usize;
        if let Some(card) = self.config_card.as_mut() {
            card.page = page;
            card.cursor = cursor;
            card.scroll.scroll_to_item(cursor);
        }
        cx.notify();
    }

    /// 一覧の行を決める。モデルと思考量とエージェントは今のピルと同じ道（sticky に value_id・
    /// セッションへ送る）、その他の select は `set_config_option`。決めたらカードへ戻る。
    fn choose_card_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(page) = self.config_card.as_ref().map(|card| card.page.clone()) else {
            return;
        };
        let Some(row) = self.card_rows(&page, cx).into_iter().nth(index) else {
            return;
        };
        match &page {
            CardPage::Main => {}
            CardPage::Models => self.select_option(Selector::Model, row.value, cx),
            CardPage::Agents => self.select_option(Selector::Agent, row.value, cx),
            CardPage::Choices(config_id) => {
                self.set_config_option(config_id, ConfigValue::Id(row.value.to_string()), cx)
            }
        }
        if let Some(card) = self.config_card.as_mut() {
            card.page = CardPage::Main;
            card.cursor = 0;
        }
        cx.notify();
    }

    /// 思考量を 1 段（←→）か端（Home / End）へ。その場で送る。
    fn step_card_effort(&mut self, step: EffortStep, cx: &mut Context<Self>) {
        let next = {
            let Some(thread) = self.threads.get(self.active) else {
                return;
            };
            let Some(config) = self
                .stocked_configs(thread)
                .iter()
                .find(|config| config.category == ConfigCategory::ThoughtLevel)
            else {
                return;
            };
            let choices = config.choices();
            let Some(last) = choices.len().checked_sub(1) else {
                return;
            };
            let current = card_value(thread, ConfigCategory::ThoughtLevel, Some(config));
            let index = choices.iter().position(|choice| choice.value_id == current);
            let next = match step {
                EffortStep::Previous => index.map_or(0, |index| index.saturating_sub(1)),
                EffortStep::Next => index.map_or(0, |index| (index + 1).min(last)),
                EffortStep::First => 0,
                EffortStep::Last => last,
            };
            if Some(next) == index {
                return;
            }
            SharedString::from(choices[next].value_id.clone())
        };
        self.select_option(Selector::Effort, next, cx);
    }

    /// 思考量の段を押した（トラックの点）。
    fn choose_card_effort(&mut self, value_id: SharedString, cx: &mut Context<Self>) {
        self.select_option(Selector::Effort, value_id, cx);
    }

    /// エージェントを次 / 前へ（Tab / ⇧Tab・会話の前だけ）。前の agent のセッションは畳んで張り直す。
    pub(crate) fn cycle_card_agent(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.card_agent_switchable() {
            return;
        }
        let choices = self.selector_choices(Selector::Agent, cx);
        if choices.len() < 2 {
            return;
        }
        let current = self.selector_value(Selector::Agent);
        let index = choices
            .iter()
            .position(|choice| choice.value == current)
            .unwrap_or(0);
        let next = (index as isize + delta).rem_euclid(choices.len() as isize) as usize;
        self.select_option(Selector::Agent, choices[next].value.clone(), cx);
    }

    /// カードの boolean の行（Fast mode 等）を押した: 反対の値をその場で送り、その agent の記憶に書く。
    pub(crate) fn toggle_card_option(&mut self, config_id: &str, cx: &mut Context<Self>) {
        let on = {
            let Some(thread) = self.threads.get(self.active) else {
                return;
            };
            let Some(config) = self
                .stocked_configs(thread)
                .iter()
                .find(|config| config.config_id == config_id)
            else {
                return;
            };
            match option_value(thread, config) {
                ConfigValue::Bool(on) => on,
                ConfigValue::Id(_) => return,
            }
        };
        self.set_config_option(config_id, ConfigValue::Bool(!on), cx);
    }

    /// カードが開いている間のキー（UI-SPEC §6）。修飾キー付き（⌘/ 等）はキー割り当てへ流す。
    fn on_config_card_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.config_card.as_ref() else {
            return;
        };
        let modifiers = &event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        }
        let page = card.page.clone();
        let cursor = card.cursor;
        let key = event.keystroke.key.as_str();
        match (&page, key) {
            // 一覧ならカードへ、カードなら閉じる。パネルの Esc（ターンの中断）へは渡さない。
            (CardPage::Main, "escape") => self.close_config_card(window, cx),
            (_, "escape") => {
                if let Some(card) = self.config_card.as_mut() {
                    card.page = CardPage::Main;
                    card.cursor = 0;
                }
            }
            (CardPage::Main, "up") => self.open_card_list(CardPage::Models, -1, cx),
            (CardPage::Main, "down") => self.open_card_list(CardPage::Models, 1, cx),
            (CardPage::Main, "enter") => self.open_card_list(CardPage::Models, 0, cx),
            (CardPage::Main, "left") => self.step_card_effort(EffortStep::Previous, cx),
            (CardPage::Main, "right") => self.step_card_effort(EffortStep::Next, cx),
            (CardPage::Main, "home") => self.step_card_effort(EffortStep::First, cx),
            (CardPage::Main, "end") => self.step_card_effort(EffortStep::Last, cx),
            (CardPage::Main, "tab") => {
                self.cycle_card_agent(if modifiers.shift { -1 } else { 1 }, cx)
            }
            (_, "up" | "down") => {
                let rows = self.card_rows(&page, cx).len();
                if let Some(card) = self.config_card.as_mut() {
                    card.cursor = if key == "up" {
                        cursor.saturating_sub(1)
                    } else {
                        (cursor + 1).min(rows.saturating_sub(1))
                    };
                    card.scroll.scroll_to_item(card.cursor);
                }
            }
            (_, "enter") => self.choose_card_row(cursor, cx),
            // 一覧の Tab は飲む（フォーカスをカードの外へ逃がさない）。
            (_, "tab") => {}
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// composer の下のチップ（エージェントのバッジ + 既定から外れた値の要約 + ▾）。開いている時は真上に
    /// カードを出す。
    pub(crate) fn render_config_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let Some(thread) = self.threads.get(self.active) else {
            return div().into_any_element();
        };
        let configs = self.stocked_configs(thread);
        let parts = chip_summary(thread, configs);
        let open = self.config_card.is_some();
        let openable = self.config_card_openable();
        let text_color = if open { theme.fg0 } else { theme.fg1 };
        let tip = if openable {
            i18n::t!("agent.card_chip_tip")
        } else {
            i18n::t!("agent.pill_awaiting_agent")
        };
        let tip = format!(
            "{}\n{tip}",
            summary_text(&parts, configs, thread.agent.as_ref())
        );
        let mut summary = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .min_w_0()
            .max_w(px(CHIP_SUMMARY_MAX_WIDTH))
            .overflow_hidden();
        if parts.is_empty() {
            summary = summary.child(div().min_w_0().truncate().child(thread.agent.clone()));
        }
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                summary = summary.child(div().flex_none().text_color(theme.fg2).child("·"));
            }
            summary = match part {
                SummaryPart::Text(text) => summary.child(div().min_w_0().truncate().child(text)),
                SummaryPart::Fast => summary.child(fast_icon(11., text_color)),
            };
        }
        let mut chip = div()
            .id("config-chip")
            .relative() // カードをこのチップ基準で絶対配置する
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(5.))
            .py(px(2.))
            .rounded(px(5.))
            .text_size(px(11.))
            .text_color(text_color)
            .when(open, |element| element.bg(theme.bg2))
            .child(agent_badge(&thread.agent, 12.))
            .child(summary)
            .tooltip(Tooltip::text(tip, theme.clone()));
        if openable {
            chip = chip
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                .child(div().text_size(px(8.)).text_color(theme.fg2).child("▾"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        // 開いていた時はカードの外側押しで閉じている。パネルの「押せば composer へ」に任せる。
                        if !open {
                            this.open_config_card(window, cx);
                            // パネル root がフォーカスを composer へ戻さないように止める。
                            cx.stop_propagation();
                        }
                    }),
                );
        }
        if let Some(card) = self.config_card.as_ref() {
            // 最前面に描く。GPUI は枠線を子の後に描くので、そのままだと composer の枠と区切り線が
            // カードの上に透けて乗る（隔離 offscreen の実画面で見つけた）。位置はチップ基準のまま。
            chip = chip.child(gpui::deferred(self.render_config_card(card, cx)).with_priority(1));
        }
        chip.into_any_element()
    }

    /// カード本体（チップの真上・左端揃え）。中身は面（カード / 一覧）で差し替える。
    fn render_config_card(&self, card: &ConfigCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let body = match &card.page {
            CardPage::Main => self.render_card_main(cx).into_any_element(),
            page => self.render_card_list(page, card, cx).into_any_element(),
        };
        let element = div()
            .id("config-card")
            .key_context("ConfigCard")
            .track_focus(&card.focus)
            .on_key_down(cx.listener(Self::on_config_card_key))
            .absolute()
            .bottom(px(24.))
            .left(px(0.))
            .w(px(CARD_WIDTH))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(10.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .overflow_hidden()
            .cursor_default()
            // カードの中を押してもパネルの「押せば composer へ」に流さない（カードがキーを持ち続ける）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            // タブ・transcript・別のピル・パネルの外のどこを押しても閉じる（押した先がフォーカスを取る）。
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.config_card = None;
                cx.notify();
            }))
            .child(body);
        if cx.reduce_motion() {
            return element.into_any_element();
        }
        element
            .with_animation(
                "config-card",
                Animation::new(std::time::Duration::from_millis(120))
                    .with_easing(gpui::ease_out_quint()),
                |element, delta| {
                    element
                        .opacity(delta)
                        .bottom(px(24.0 - 6.0 * (1.0 - delta)))
                },
            )
            .into_any_element()
    }

    /// カードの面: ①エージェント ②モデル ③思考量 ④その他の設定 ⑤キーの一言。
    fn render_card_main(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme.clone();
        let Some(thread) = self.threads.get(self.active) else {
            return div();
        };
        let configs = self.stocked_configs(thread);
        let switchable = self.card_agent_switchable();
        let model = configs
            .iter()
            .find(|config| config.category == ConfigCategory::Model);
        let effort = configs
            .iter()
            .find(|config| config.category == ConfigCategory::ThoughtLevel);
        let others: Vec<&ConfigOption> = configs
            .iter()
            .filter(|config| config.category == ConfigCategory::Other)
            .collect();

        // ① エージェント。名前の右は issue #38 H4 の「接続」の欄が入る場所（今は何も描かない）。
        let mut agent_row = div()
            .id("card-agent")
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(12.))
            .pt(px(9.))
            .text_size(px(11.))
            .text_color(theme.fg1)
            .child(agent_badge(&thread.agent, 14.))
            .child(div().min_w_0().truncate().child(thread.agent.clone()))
            .child(div().flex_1());
        if switchable {
            agent_row = agent_row
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(
                            div()
                                .px(px(4.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(theme.border)
                                .text_size(px(9.5))
                                .text_color(theme.fg1)
                                .child("Tab"),
                        )
                        .child(SharedString::from(i18n::t!("agent.card_tab_hint"))),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_card_list(CardPage::Agents, 0, cx);
                    }),
                );
        } else {
            agent_row = agent_row
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("agent.card_agent_fixed"))),
                )
                .tooltip(Tooltip::text(
                    i18n::t!("agent.pill_agent_locked"),
                    theme.clone(),
                ));
        }

        let mut main = div().flex().flex_col().child(agent_row);

        // ② モデル（表示名 + 選んでいる選択肢の説明）。押すと一覧。
        if let Some(config) = model {
            let value = card_value(thread, ConfigCategory::Model, Some(config));
            let label = choice_name(config, &value).unwrap_or_else(|| value.clone().into());
            let description = config
                .choices()
                .iter()
                .find(|choice| choice.value_id == value)
                .and_then(|choice| choice.description.clone());
            main = main.child(
                div()
                    .id("card-model")
                    .px(px(12.))
                    .pt(px(5.))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(div().min_w_0().truncate().child(label))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(theme.fg2)
                                    .child("›"),
                            ),
                    )
                    .when_some(description, |element, description| {
                        element.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .truncate()
                                .child(description),
                        )
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_card_list(CardPage::Models, 0, cx);
                        }),
                    ),
            );
        }

        // ③ 思考量（広告の順の段のトラック）。点を押して選ぶ・←→ で 1 段ずつ。
        if let Some(config) = effort {
            main = main.child(self.render_card_effort(thread, config, cx));
        }

        // 広告がまだ無い（会話の前でエージェントだけ選べる）。
        if configs.is_empty() {
            main = main.child(
                div()
                    .px(px(12.))
                    .pt(px(6.))
                    .pb(px(10.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("agent.card_no_options"))),
            );
        } else if effort.is_none() {
            main = main.child(div().h(px(10.)));
        }

        // ④ その他の設定（Fast mode・Collaboration mode 等）。広告の順に 1 行ずつ。
        if !others.is_empty() {
            main =
                main.child(
                    div()
                        .flex()
                        .flex_col()
                        .py(px(3.))
                        .border_t_1()
                        .border_color(theme.border)
                        .children(others.into_iter().enumerate().map(|(index, config)| {
                            self.render_card_option(index, thread, config, cx)
                        })),
                );
        }

        // ⑤ キーの一言（出来ることだけ並べる。Tab はエージェントの行に出しているので重ねない）。
        let mut keys: Vec<(&str, String)> = Vec::new();
        if model.is_some() {
            keys.push(("↑↓", i18n::t!("agent.card_key_models")));
        }
        if effort.is_some() {
            keys.push(("←→", i18n::t!("agent.card_key_effort")));
        }
        keys.push(("esc", i18n::t!("agent.card_key_close")));
        main.child(self.render_card_keys(keys, cx))
    }

    /// 思考量の段（トラック）。線 2px bg3・つまみまでの塗り fg2・段の点 fg2・つまみ fg0（色相なし）。
    fn render_card_effort(
        &self,
        thread: &Thread,
        config: &ConfigOption,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let choices = config.choices();
        let value = card_value(thread, ConfigCategory::ThoughtLevel, Some(config));
        let label = choice_name(config, &value).unwrap_or_else(|| value.clone().into());
        let at = choices.iter().position(|choice| choice.value_id == value);
        let last = choices.len().saturating_sub(1).max(1) as f32;
        let fraction = |index: usize| index as f32 / last;
        let mut track = div().relative().h(px(18.)).mt(px(4.)).mx(px(6.)).child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(8.))
                .h(px(2.))
                .rounded(px(1.))
                .bg(theme.bg3),
        );
        if let Some(at) = at {
            track = track.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(8.))
                    .h(px(2.))
                    .w(relative(fraction(at)))
                    .rounded(px(1.))
                    .bg(theme.fg2),
            );
        }
        for (index, choice) in choices.iter().enumerate() {
            let value_id = SharedString::from(choice.value_id.clone());
            track = track.child(
                div()
                    .id(("card-effort-stop", index))
                    .absolute()
                    .left(relative(fraction(index)))
                    .ml(px(-8.))
                    .top(px(1.))
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(div().size(px(4.)).rounded_full().bg(theme.fg2))
                    .tooltip(Tooltip::text(choice.name.clone(), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.choose_card_effort(value_id.clone(), cx);
                        }),
                    ),
            );
        }
        if let Some(at) = at {
            track = track.child(
                div()
                    .absolute()
                    .left(relative(fraction(at)))
                    .ml(px(-6.))
                    .top(px(3.))
                    .size(px(12.))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.bg2)
                    .bg(theme.fg0),
            );
        }
        div()
            .px(px(12.))
            .pt(px(10.))
            .pb(px(11.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("agent.card_effort")))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg0)
                            .child(label),
                    ),
            )
            .child(track)
    }

    /// その他の設定の 1 行。boolean はスイッチ（押すとすぐ送る）、select は今の値 + ›（押すと一覧）。
    fn render_card_option(
        &self,
        index: usize,
        thread: &Thread,
        config: &ConfigOption,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let config_id = config.config_id.clone();
        let value = option_value(thread, config);
        let is_fast = config.config_id == FAST_CONFIG_ID;
        let control = match &value {
            ConfigValue::Bool(on) => {
                let on = *on;
                div()
                    .relative()
                    .flex_none()
                    .w(px(26.))
                    .h(px(15.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(if on { theme.fg1 } else { theme.border })
                    .bg(if on { theme.fg1 } else { theme.bg3 })
                    .child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .left(px(if on { 13. } else { 2. }))
                            .size(px(9.))
                            .rounded_full()
                            .bg(if on { theme.bg2 } else { theme.fg2 }),
                    )
            }
            ConfigValue::Id(value_id) => div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(3.))
                .text_size(px(11.5))
                .text_color(theme.fg0)
                .child(
                    choice_name(config, value_id)
                        .unwrap_or_else(|| SharedString::from(value_id.clone())),
                )
                .child(div().text_color(theme.fg2).child("›")),
        };
        div()
            .id(("card-option", index))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(5.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg3))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(11.5))
                            .text_color(theme.fg1)
                            .when(is_fast, |element| element.child(fast_icon(11., theme.fg1)))
                            .child(config.name.clone()),
                    )
                    .when_some(config.description.clone(), |element, description| {
                        element.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(10.))
                                .text_color(theme.fg2)
                                .child(description),
                        )
                    }),
            )
            .child(control)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if matches!(value, ConfigValue::Bool(_)) {
                        this.toggle_card_option(&config_id, cx);
                    } else {
                        this.open_card_list(CardPage::Choices(config_id.clone()), 0, cx);
                    }
                }),
            )
    }

    /// 一覧の面（モデル / エージェント / select の選択肢）。頭に `‹ 名前` とキーの一言。
    fn render_card_list(
        &self,
        page: &CardPage,
        card: &ConfigCard,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let rows = self.card_rows(page, cx);
        let current = self.card_current(page);
        let title: SharedString = match page {
            CardPage::Main => SharedString::default(),
            CardPage::Models => i18n::t!("agent.card_list_models").into(),
            CardPage::Agents => i18n::t!("agent.card_list_agents").into(),
            CardPage::Choices(config_id) => self
                .card_configs()
                .iter()
                .find(|config| config.config_id == *config_id)
                .map(|config| SharedString::from(config.name.clone()))
                .unwrap_or_default(),
        };
        let agents = *page == CardPage::Agents;
        let keys = vec![
            ("↑↓", i18n::t!("agent.card_key_select")),
            ("⏎", i18n::t!("agent.card_key_choose")),
            ("esc", i18n::t!("agent.card_key_back")),
        ];
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("card-list-head")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(8.))
                    .pb(px(6.))
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(px(11.5))
                    .text_color(theme.fg1)
                    .cursor_pointer()
                    .child(div().text_size(px(13.)).text_color(theme.fg2).child("‹"))
                    .child(div().min_w_0().truncate().child(title))
                    .child(div().flex_1())
                    .child(self.render_card_key_hints(keys))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(card) = this.config_card.as_mut() {
                                card.page = CardPage::Main;
                                card.cursor = 0;
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .id("card-list")
                    .flex()
                    .flex_col()
                    .max_h(px(CARD_LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&card.scroll)
                    .p(px(4.))
                    .children(rows.into_iter().enumerate().map(|(index, row)| {
                        let now = current.as_ref() == Some(&row.value);
                        div()
                            .id(("card-row", index))
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .text_size(px(12.))
                            .text_color(if now { theme.fg0 } else { theme.fg1 })
                            .cursor_pointer()
                            .when(index == card.cursor, |element| element.bg(theme.bg3))
                            .hover(|style| style.bg(theme.bg3))
                            .when(agents, |element| {
                                element.child(agent_badge(&row.label, 14.))
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(180.))
                                    .truncate()
                                    .child(row.label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .children(row.detail),
                            )
                            .when(now, |element| {
                                element.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(theme.fg1)
                                        .child("✓"),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.choose_card_row(index, cx);
                                }),
                            )
                    })),
            )
    }

    /// カードの下端のキーの一言（`↑↓ モデル · ←→ 思考量 · …`）。
    fn render_card_keys(&self, keys: Vec<(&str, String)>, _cx: &mut Context<Self>) -> Div {
        let theme = &self.theme;
        div()
            .border_t_1()
            .border_color(theme.border)
            .px(px(12.))
            .pt(px(5.))
            .pb(px(6.))
            .child(self.render_card_key_hints(keys))
    }

    /// キーの一言の中身（キーは fg1・説明は fg2・10px）。
    fn render_card_key_hints(&self, keys: Vec<(&str, String)>) -> Div {
        let theme = &self.theme;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .text_size(px(10.))
            .text_color(theme.fg2)
            .children(keys.into_iter().map(|(key, label)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().text_color(theme.fg1).child(SharedString::from(key)))
                    .child(SharedString::from(label))
            }))
    }
}

/// 開発用（`NECODER_CONFIG_CARD_PROBE`）: 設定のカードを offscreen で撮るための見本の広告。
/// claude-agent-acp 0.81.2 / codex-acp 1.13.1 が実際に送る形をそのまま写した物（エージェントは起こさない）。
#[cfg(debug_assertions)]
mod probe {
    use acp_client::{ConfigCategory, ConfigChoice, ConfigKind, ConfigOption};

    fn select(
        config_id: &str,
        name: &str,
        category: ConfigCategory,
        current: &str,
        choices: &[(&str, &str, Option<&str>)],
    ) -> ConfigOption {
        ConfigOption {
            config_id: config_id.into(),
            name: name.into(),
            description: None,
            category,
            kind: ConfigKind::Select {
                current: current.into(),
                choices: choices
                    .iter()
                    .map(|(value_id, name, description)| ConfigChoice {
                        value_id: (*value_id).into(),
                        name: (*name).into(),
                        description: description.map(str::to_string),
                    })
                    .collect(),
            },
        }
    }

    fn fast(description: &str) -> ConfigOption {
        ConfigOption {
            config_id: "fast".into(),
            name: "Fast mode".into(),
            description: Some(description.into()),
            category: ConfigCategory::Other,
            kind: ConfigKind::Boolean { current: false },
        }
    }

    /// Claude Code の広告。`unavailable` = Fast mode が使えないプラン（説明に理由が付く）。
    pub(super) fn claude(unavailable: bool) -> Vec<ConfigOption> {
        vec![
            select(
                "mode",
                "Mode",
                ConfigCategory::Mode,
                "default",
                &[
                    ("default", "Default", None),
                    ("acceptEdits", "Accept Edits", None),
                    ("plan", "Plan Mode", None),
                    ("bypassPermissions", "Bypass Permissions", None),
                ],
            ),
            select(
                "model",
                "Model",
                ConfigCategory::Model,
                "default",
                &[
                    ("default", "Default (recommended)", Some("Fable 5.1")),
                    (
                        "fable",
                        "Fable 5.1",
                        Some("Most intelligent model for building agents"),
                    ),
                    ("opus", "Opus 4.8", None),
                    ("opus[1m]", "Opus 4.8 (1M context)", None),
                    ("sonnet", "Sonnet 5", None),
                    ("haiku", "Haiku 4.5", None),
                ],
            ),
            select(
                "effort",
                "Effort",
                ConfigCategory::ThoughtLevel,
                "default",
                &[
                    ("default", "Default", None),
                    ("low", "Low", None),
                    ("medium", "Medium", None),
                    ("high", "High", None),
                    ("xhigh", "Xhigh", None),
                    ("max", "Max", None),
                ],
            ),
            fast(if unavailable {
                "Faster responses on supported models — not available on the free plan"
            } else {
                "Faster responses on supported models"
            }),
        ]
    }

    /// Codex の広告（Collaboration mode が増え、Model / Reasoning effort に `default` が無い）。
    pub(super) fn codex() -> Vec<ConfigOption> {
        vec![
            select(
                "collaboration_mode",
                "Collaboration mode",
                ConfigCategory::Other,
                "default",
                &[
                    ("default", "Default", None),
                    ("plan", "Plan", Some("Plan before making changes")),
                ],
            ),
            select(
                "model",
                "Model",
                ConfigCategory::Model,
                "gpt-5.6-sol",
                &[
                    (
                        "gpt-5.6-sol",
                        "GPT-5.6-Sol",
                        Some("Latest frontier agentic coding model."),
                    ),
                    ("gpt-5.6", "GPT-5.6", None),
                    ("gpt-5.5-codex", "GPT-5.5-Codex", None),
                ],
            ),
            select(
                "reasoning_effort",
                "Reasoning effort",
                ConfigCategory::ThoughtLevel,
                "medium",
                &[
                    ("low", "Low", None),
                    ("medium", "Medium", None),
                    ("high", "High", None),
                    ("xhigh", "Xhigh", None),
                ],
            ),
            fast("1.5x speed, increased usage"),
        ]
    }
}

#[cfg(debug_assertions)]
impl AgentPanel {
    /// 開発用（`NECODER_CONFIG_CARD_PROBE`・`;` 区切りで順に実行）: 設定のカードを offscreen で撮る。
    /// エージェントは起こさず、見本の広告を本番と同じ `on_event` で流す。
    ///
    /// `claude` / `claude-free`（Fast が使えないプラン）/ `codex` = 広告を流す / `opus-high-fast` =
    /// Opus 4.8・High・Fast on を選ぶ / `plan` = Collaboration mode を Plan に / `fixed` = 会話を始めた
    /// 後（エージェントは固定）/ `open` = カードを開く / `models` = カードを開いてモデルの一覧へ。
    pub fn debug_config_card_probe(
        &mut self,
        commands: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.active;
        for command in commands.split(';').map(str::trim) {
            match command {
                "claude" | "claude-free" => {
                    self.on_event(
                        active,
                        AgentEvent::Modes {
                            modes: vec![
                                ("default".into(), "Default".into()),
                                ("acceptEdits".into(), "Accept Edits".into()),
                                ("plan".into(), "Plan Mode".into()),
                                ("bypassPermissions".into(), "Bypass Permissions".into()),
                            ],
                            current: "default".into(),
                        },
                        cx,
                    );
                    self.on_event(
                        active,
                        AgentEvent::Configs(probe::claude(command == "claude-free")),
                        cx,
                    );
                }
                "codex" => {
                    if let Some(thread) = self.threads.get_mut(active) {
                        thread.agent = "Codex".into();
                        thread.model = SharedString::default();
                        thread.effort = SharedString::default();
                        thread.options.clear();
                    }
                    self.on_event(
                        active,
                        AgentEvent::Modes {
                            modes: vec![
                                ("read-only".into(), "Read Only".into()),
                                ("auto".into(), "Agent".into()),
                                ("full-access".into(), "Agent (full access)".into()),
                            ],
                            current: "auto".into(),
                        },
                        cx,
                    );
                    self.on_event(active, AgentEvent::Configs(probe::codex()), cx);
                }
                "opus-high-fast" => {
                    self.select_option(Selector::Model, "opus".into(), cx);
                    self.select_option(Selector::Effort, "high".into(), cx);
                    self.set_config_option("fast", ConfigValue::Bool(true), cx);
                }
                "plan" => {
                    self.set_config_option(
                        "collaboration_mode",
                        ConfigValue::Id("plan".into()),
                        cx,
                    );
                }
                "fixed" => {
                    if let Some(thread) = self.threads.get_mut(active) {
                        thread
                            .entries
                            .push(Entry::User("設定のカードを撮る".into()));
                    }
                }
                "open" => self.open_config_card(window, cx),
                "models" => {
                    self.open_config_card(window, cx);
                    self.open_card_list(CardPage::Models, 1, cx);
                }
                // カードの外に残した権限モードのピルのメニュー。
                "mode-menu" => self.toggle_menu(Selector::Mode, cx),
                "" => {}
                other => eprintln!("NECODER_CONFIG_CARD_PROBE: 知らない命令 {other}"),
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_client::{ConfigChoice, ConfigKind};

    fn select(
        config_id: &str,
        category: ConfigCategory,
        current: &str,
        choices: &[(&str, &str)],
    ) -> ConfigOption {
        ConfigOption {
            config_id: config_id.into(),
            name: config_id.into(),
            description: None,
            category,
            kind: ConfigKind::Select {
                current: current.into(),
                choices: choices
                    .iter()
                    .map(|(value_id, name)| ConfigChoice {
                        value_id: (*value_id).into(),
                        name: (*name).into(),
                        description: None,
                    })
                    .collect(),
            },
        }
    }

    fn boolean(config_id: &str, name: &str, current: bool) -> ConfigOption {
        ConfigOption {
            config_id: config_id.into(),
            name: name.into(),
            description: None,
            category: ConfigCategory::Other,
            kind: ConfigKind::Boolean { current },
        }
    }

    /// claude-agent-acp 0.81.2 が広告する形（necoder は boolean の受け取りを広告している）。
    fn claude_configs(fast: bool) -> Vec<ConfigOption> {
        vec![
            select(
                "mode",
                ConfigCategory::Mode,
                "default",
                &[
                    ("default", "Default"),
                    ("bypassPermissions", "Bypass Permissions"),
                ],
            ),
            select(
                "model",
                ConfigCategory::Model,
                "default",
                &[
                    ("default", "Default (recommended)"),
                    ("opus", "Opus 4.8"),
                    ("sonnet", "Sonnet 5"),
                ],
            ),
            select(
                "effort",
                ConfigCategory::ThoughtLevel,
                "default",
                &[("default", "Default"), ("high", "High"), ("max", "Max")],
            ),
            boolean("fast", "Fast mode", fast),
        ]
    }

    /// codex-acp 1.13.1 の形: Model / Reasoning effort に `default` が無く、Collaboration mode にある。
    fn codex_configs(collaboration: &str) -> Vec<ConfigOption> {
        vec![
            select(
                "collaboration_mode",
                ConfigCategory::Other,
                collaboration,
                &[("default", "Default"), ("plan", "Plan")],
            ),
            select(
                "model",
                ConfigCategory::Model,
                "gpt-5.6-sol",
                &[("gpt-5.6-sol", "GPT-5.6-Sol"), ("gpt-5.6", "GPT-5.6")],
            ),
            select(
                "reasoning_effort",
                ConfigCategory::ThoughtLevel,
                "medium",
                &[("low", "Low"), ("medium", "Medium"), ("high", "High")],
            ),
            boolean("fast", "Fast mode", false),
        ]
    }

    fn texts(parts: &[SummaryPart]) -> Vec<String> {
        parts
            .iter()
            .map(|part| match part {
                SummaryPart::Text(text) => text.to_string(),
                SummaryPart::Fast => "⚡".to_string(),
            })
            .collect()
    }

    /// 全部既定なら要約は空（チップはエージェント名だけ）。権限モードの二重の広告は数えない。
    #[test]
    fn defaults_leave_the_chip_to_the_agent_name() {
        let thread = Thread::empty("t", 0);
        let configs = claude_configs(false);
        let parts = chip_summary(&thread, &configs);
        assert!(parts.is_empty(), "{parts:?}");
        assert_eq!(summary_text(&parts, &configs, "Claude Code"), "Claude Code");
    }

    /// 既定から外れた値だけを、モデル → 思考量 → Fast の順に。表示名は広告の物。
    #[test]
    fn chip_shows_only_what_departs_from_the_defaults() {
        let mut thread = Thread::empty("t", 0);
        thread.model = "opus".into();
        thread.effort = "high".into();
        let configs = claude_configs(true);
        let parts = chip_summary(&thread, &configs);
        assert_eq!(texts(&parts), vec!["Opus 4.8", "High", "⚡"]);
        assert_eq!(
            summary_text(&parts, &configs, "Claude Code"),
            "Opus 4.8 · High · Fast mode",
            "ツールチップでは稲妻を設定の名前で書く"
        );

        // 思考量だけ外れている。
        thread.model = "default".into();
        thread.effort = "max".into();
        let parts = chip_summary(&thread, &claude_configs(false));
        assert_eq!(texts(&parts), vec!["Max"]);
    }

    /// `default` を広告しない select（Codex の Model / Effort）はいつも出す。Collaboration mode は
    /// `default` を広告しているので、Plan の時だけ出す。
    #[test]
    fn codex_shows_model_and_effort_and_plan_only_when_chosen() {
        let thread = Thread::empty("t", 0);
        let parts = chip_summary(&thread, &codex_configs("default"));
        assert_eq!(texts(&parts), vec!["GPT-5.6-Sol", "Medium"]);
        let parts = chip_summary(&thread, &codex_configs("plan"));
        assert_eq!(texts(&parts), vec!["GPT-5.6-Sol", "Medium", "Plan"]);
    }

    /// スレッドが望む値（sticky・返事待ち）が広告の値より先。boolean の名前は Fast 以外なら名前で出す。
    #[test]
    fn wished_values_and_other_booleans_are_summarized() {
        let mut thread = Thread::empty("t", 0);
        thread.options.insert("fast".into(), "true".into());
        thread.options.insert("thinking".into(), "true".into());
        let mut configs = claude_configs(false);
        configs.push(boolean("thinking", "Extended thinking", false));
        let parts = chip_summary(&thread, &configs);
        assert_eq!(texts(&parts), vec!["Extended thinking", "⚡"]);
    }

    /// 広告がまだ無い時は、覚えている value_id をそのまま出す（表示名を作らない）。`default` は出さない。
    #[test]
    fn before_any_advertisement_the_remembered_value_id_is_shown() {
        let mut thread = Thread::empty("t", 0);
        thread.model = "opus[1m]".into();
        thread.effort = "default".into();
        assert_eq!(texts(&chip_summary(&thread, &[])), vec!["opus[1m]"]);
    }
}
