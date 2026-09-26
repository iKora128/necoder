//! 端末内の検索（⌘F）。
//!
//! alacritty の `RegexSearch` / `RegexIter` で scrollback を含む全体を探し、件数と前 / 次の移動に使う。
//! 見えている範囲の一致は描画のたびに塗る（`sync` で表示範囲だけ探し直す＝出力が流れていても
//! 位置がずれない）。全体の数え直しは出力が続く間は間引く（[`REFRESH_INTERVAL`]）。
//!
//! 見た目はエディタの ⌘F バー（workspace の `render_buffer_search_bar`）に合わせる。置換は無い。
//! 入力欄は同じく手書き（IME の変換は通らない＝エディタの ⌘F バーと同じ制約）。

use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point as AlacPoint};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;
use gpui::{
    div, prelude::*, px, AnyElement, Context, FocusHandle, KeyDownEvent, MouseButton, SharedString,
    Window,
};
use ui::Tooltip;

use crate::TerminalView;

/// 全体の一致の上限。超えたら件数に `+` を付けて打ち切る。
const MAX_MATCHES: usize = 1000;

/// 出力が続く間の全体の数え直しの間隔。
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_millis(200);

/// 検索バーの状態。`None` = 閉じている。
pub(crate) struct TerminalSearch {
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) is_regex: bool,
    pub(crate) focus: FocusHandle,
    /// いまの語で組んだ正規表現（語が空・誤りなら None）。
    pub(crate) regex: Option<RegexSearch>,
    /// 全体の一致（上から順・最大 [`MAX_MATCHES`]）。
    pub(crate) matches: Vec<Match>,
    pub(crate) truncated: bool,
    /// いま見ている一致（`matches` の添字）。
    pub(crate) current: Option<usize>,
    /// 正規表現の誤り。
    pub(crate) invalid: bool,
    /// 開いた時の表示位置（Esc で戻る）。
    pub(crate) saved_offset: usize,
    /// 出力が増えた後の数え直しを予約済み。
    pub(crate) refresh_scheduled: bool,
}

impl TerminalSearch {
    pub(crate) fn new(focus: FocusHandle, saved_offset: usize) -> Self {
        Self {
            query: String::new(),
            case_sensitive: false,
            is_regex: false,
            focus,
            regex: None,
            matches: Vec::new(),
            truncated: false,
            current: None,
            invalid: false,
            saved_offset,
            refresh_scheduled: false,
        }
    }

    /// 語とトグルから正規表現を組み直す。
    pub(crate) fn rebuild_regex(&mut self) {
        self.invalid = false;
        self.regex = None;
        if self.query.is_empty() {
            return;
        }
        match RegexSearch::new(&search_pattern(
            &self.query,
            self.is_regex,
            self.case_sensitive,
        )) {
            Ok(regex) => self.regex = Some(regex),
            Err(_) => self.invalid = true,
        }
    }
}

/// 検索語 → alacritty の正規表現。正規表現でなければメタ文字を逃がす。大小の区別はトグルで決め、
/// alacritty の「大文字を含めば区別する」既定をインラインのフラグで上書きする。
pub(crate) fn search_pattern(query: &str, is_regex: bool, case_sensitive: bool) -> String {
    let flag = if case_sensitive { "(?-i)" } else { "(?i)" };
    if is_regex {
        return format!("{flag}{query}");
    }
    let mut pattern = String::from(flag);
    for character in query.chars() {
        if matches!(
            character,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

/// 全体（scrollback を含む）の一致を上から集める。
pub(crate) fn find_all<T>(term: &Term<T>, regex: &mut RegexSearch) -> (Vec<Match>, bool) {
    let start = AlacPoint::new(term.topmost_line(), Column(0));
    let end = AlacPoint::new(term.bottommost_line(), term.last_column());
    let mut matches = Vec::new();
    for found in RegexIter::new(start, end, Direction::Right, term, regex) {
        if matches.len() == MAX_MATCHES {
            return (matches, true);
        }
        matches.push(found);
    }
    (matches, false)
}

/// 表示範囲（`display_offset` から `screen_lines` 行）の一致。描画の強調に使う。
pub(crate) fn find_visible<T>(term: &Term<T>, regex: &mut RegexSearch) -> Vec<Match> {
    let display_offset = term.grid().display_offset() as i32;
    let top = Line(-display_offset);
    let bottom = Line(term.screen_lines() as i32 - 1 - display_offset);
    let start = AlacPoint::new(top, Column(0));
    let end = AlacPoint::new(bottom, term.last_column());
    RegexIter::new(start, end, Direction::Right, term, regex)
        .take(MAX_MATCHES)
        .collect()
}

/// 数え直した後に見る一致: 表示範囲の下端より上で一番下（＝いま見ている所に一番近い最新）。
/// 下端より上に無ければ先頭。
pub(crate) fn nearest_match(matches: &[Match], viewport_bottom: Line) -> Option<usize> {
    matches
        .iter()
        .rposition(|found| found.start().line <= viewport_bottom)
        .or_else(|| (!matches.is_empty()).then_some(0))
}

impl TerminalView {
    /// ⌘F バー（開いている時だけ）。エディタの ⌘F バーと同じ寸法・部品（置換行なし）。
    pub(crate) fn render_search_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let search = self.search.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent;
        let focused = search.focus.is_focused(window);

        let (counter, counter_color) = if search.invalid {
            (
                SharedString::from(i18n::t!("terminal.search_invalid_regex")),
                theme.err,
            )
        } else if search.matches.is_empty() {
            let text = if search.query.is_empty() {
                SharedString::default()
            } else {
                SharedString::from(i18n::t!("search.no_results"))
            };
            (text, theme.fg2)
        } else {
            let text = format!(
                "{}/{}{}",
                search.current.map_or(0, |current| current + 1),
                search.matches.len(),
                if search.truncated { "+" } else { "" }
            );
            (SharedString::from(text), theme.fg2)
        };

        let (display, text_color) = if search.query.is_empty() {
            (
                SharedString::from(i18n::t!("search.find_placeholder")),
                theme.fg2,
            )
        } else {
            (SharedString::from(search.query.clone()), theme.fg0)
        };
        let field = div()
            .id("tsearch-query")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .h(px(24.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(theme.bg1)
            .border_1()
            .border_color(if focused { accent } else { theme.border })
            .overflow_hidden()
            .cursor(gpui::CursorStyle::IBeam)
            .text_size(px(12.))
            .text_color(text_color)
            .child(div().overflow_hidden().whitespace_nowrap().child(display))
            .when(focused, |element| {
                element.child(div().flex_none().w(px(1.5)).h(px(14.)).bg(accent))
            });

        let chip = |id: &'static str, label: &'static str, active: bool, tip: SharedString| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .text_size(px(11.))
                .text_color(if active { theme.fg0 } else { theme.fg2 })
                .cursor_pointer()
                .when(active, |element| element.bg(accent.alpha(0.16)))
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(label)
                .tooltip(Tooltip::text(tip, theme.clone()))
        };

        let row = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(field)
            .child(
                div()
                    .flex_none()
                    .max_w(px(150.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .text_color(counter_color)
                    .child(counter),
            )
            .child(
                chip(
                    "tsearch-case",
                    "Aa",
                    search.case_sensitive,
                    SharedString::from(i18n::t!("search.case_sensitive")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        if let Some(search) = this.search.as_mut() {
                            search.case_sensitive = !search.case_sensitive;
                        }
                        this.refresh_search(true, cx);
                    }),
                ),
            )
            .child(
                chip(
                    "tsearch-regex",
                    ".*",
                    search.is_regex,
                    SharedString::from(i18n::t!("search.use_regex")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        if let Some(search) = this.search.as_mut() {
                            search.is_regex = !search.is_regex;
                        }
                        this.refresh_search(true, cx);
                    }),
                ),
            )
            .child(
                chip(
                    "tsearch-prev",
                    "‹",
                    false,
                    SharedString::from(i18n::t!("search.previous")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.step_search(-1, cx)),
                ),
            )
            .child(
                chip(
                    "tsearch-next",
                    "›",
                    false,
                    SharedString::from(i18n::t!("search.next")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.step_search(1, cx)),
                ),
            )
            .child(
                chip(
                    "tsearch-close",
                    "×",
                    false,
                    SharedString::from(i18n::t!("search.close")),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_search(false, window, cx)),
                ),
            );

        let focus = search.focus.clone();
        Some(
            div()
                .id("terminal-search-bar")
                .absolute()
                .top(px(8.))
                .right(px(14.))
                .w(px(380.))
                .max_w_full()
                .p(px(6.))
                .bg(theme.bg2)
                .rounded(px(8.))
                .border_1()
                .border_color(theme.border)
                .shadow(vec![gpui::BoxShadow::new(
                    px(0.),
                    px(6.),
                    gpui::hsla(0., 0., 0., 0.35),
                )
                .blur_radius(px(18.))])
                // 端末本体は等幅だが、バーはエディタの ⌘F バーと同じ UI フォント。
                .font_family("IBM Plex Sans JP")
                .cursor(gpui::CursorStyle::Arrow)
                .track_focus(&focus)
                .on_key_down(cx.listener(Self::on_search_key_down))
                // バー内のクリックは端末（選択・マウス報告）へ通さず、入力欄へフォーカスを戻す。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, window, cx| {
                        cx.stop_propagation();
                        window.focus(&focus, cx);
                    }),
                )
                .child(row)
                .into_any_element(),
        )
    }

    /// 検索欄のキー。どのキーも端末（PTY）へは流さない。
    fn on_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let modifiers = event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => self.close_search(true, window, cx),
            "enter" if modifiers.shift => self.step_search(-1, cx),
            "enter" => self.step_search(1, cx),
            "backspace" => {
                if let Some(search) = self.search.as_mut() {
                    search.query.pop();
                }
                self.refresh_search(true, cx);
            }
            _ => {
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = event.keystroke.key_char.as_deref() else {
                    return;
                };
                if text.is_empty() || text.chars().any(char::is_control) {
                    return;
                }
                if let Some(search) = self.search.as_mut() {
                    search.query.push_str(text);
                }
                self.refresh_search(true, cx);
            }
        }
    }

    /// 出力が増えた時の全体の数え直しを間引いて予約する（すぐには走らせない）。
    pub(crate) fn schedule_search_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        if search.refresh_scheduled || search.regex.is_none() {
            return;
        }
        search.refresh_scheduled = true;
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(REFRESH_INTERVAL).await;
            if view
                .update(cx, |view, cx| {
                    if let Some(search) = view.search.as_mut() {
                        search.refresh_scheduled = false;
                    }
                    view.refresh_search(false, cx);
                })
                .is_err()
            {
                // 端末が先に閉じた。数え直す相手が居ないだけ。
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    struct Size;

    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            5
        }
        fn screen_lines(&self) -> usize {
            5
        }
        fn columns(&self) -> usize {
            40
        }
    }

    fn term_with(text: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size, VoidListener);
        let mut parser = Processor::<StdSyncHandler>::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    fn count(term: &Term<VoidListener>, query: &str, is_regex: bool, case: bool) -> usize {
        let mut regex = RegexSearch::new(&search_pattern(query, is_regex, case)).expect("正規表現");
        find_all(term, &mut regex).0.len()
    }

    #[test]
    fn plain_queries_escape_regex_syntax_and_toggles_control_case() {
        let term = term_with("a.b axb\r\nError error ERROR\r\n");
        // 正規表現でない時の `.` は文字どおり。
        assert_eq!(count(&term, "a.b", false, false), 1);
        assert_eq!(count(&term, "a.b", true, false), 2);
        // 大小: 既定は区別しない（語に大文字があっても）・トグルで区別する。
        assert_eq!(count(&term, "Error", false, false), 3);
        assert_eq!(count(&term, "Error", false, true), 1);
        assert_eq!(count(&term, "error", false, true), 1);
        // 誤った正規表現は作れない（バーは件数の位置に誤りを出す）。
        assert!(RegexSearch::new(&search_pattern("(", true, false)).is_err());
        assert!(RegexSearch::new(&search_pattern("(", false, false)).is_ok());
    }

    #[test]
    fn matches_in_scrollback_are_found_and_the_nearest_is_the_newest() {
        // 5 行の画面に 12 行出す＝上の 7 行は scrollback。
        let text: String = (0..12)
            .map(|index| format!("line {index} hit\r\n"))
            .collect();
        let term = term_with(&text);
        let mut regex = RegexSearch::new(&search_pattern("hit", false, false)).expect("正規表現");
        let (matches, truncated) = find_all(&term, &mut regex);
        assert_eq!(matches.len(), 12);
        assert!(!truncated);
        assert!(matches[0].start().line < Line(0), "scrollback の一致も拾う");
        // 表示範囲の一致だけ（最下段は空行）。
        assert_eq!(find_visible(&term, &mut regex).len(), 4);
        // 開いた直後に見る一致は、表示の下端に一番近いもの（最新）。
        assert_eq!(nearest_match(&matches, Line(4)), Some(11));
        assert_eq!(nearest_match(&[], Line(4)), None);
    }
}
