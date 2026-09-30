//! 端末内の検索（⌘F）。
//!
//! alacritty の `RegexSearch` / `RegexIter` で scrollback を含む全体を探し、件数と前 / 次の移動に使う。
//! 見えている範囲の一致は描画のたびに塗る（`sync` で表示範囲だけ探し直す＝出力が流れていても
//! 位置がずれない）。全体の数え直しは出力が続く間は間引く（[`REFRESH_INTERVAL`]）。
//!
//! 見た目はエディタの ⌘F バー（workspace の `render_buffer_search_bar`）の寸法に合わせる。置換は無い。
//!
//! 入力欄は IME の正しい 1 行入力（`EditorView::plain`・設定の検索欄や Todo の追加欄と同じ作り・R12）＝
//! 日本語の変換・貼り付け・選択しての置き換え・カーソル移動が効く。キーの受け方（gpui は keymap の束が
//! 先で、誰も受けなかった時だけ `on_key_down` に落ちる）:
//!
//! - ⏎ / ⇧⏎ = 次 / 前。入力欄の改行（`editor::Newline` / `InsertNewline`）より先に受ける。変換中に
//!   届いたら何もしない＝確定の ⏎ は確定だけ
//! - esc = 開いた位置へ戻って閉じる（入力欄が親へ流す `editor::Cancel`）
//! - 変換中（未確定の文字がある間）は探し直さない。確定した時に探す
//! - 端末そのものの束（⌘F・⌘K・⌘T・⌘W・⌘\）は入力欄の中でも端末の物（[`intercept_terminal_keys`]）

use std::time::Duration;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point as AlacPoint};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;
use editor_view::EditorView;
use gpui::{
    div, prelude::*, px, Action, AnyElement, App, Context, Entity, Focusable, MouseButton,
    SharedString, Subscription, Window,
};
use ui::Tooltip;

use crate::{actions, TerminalView};

/// 全体の一致の上限。超えたら件数に `+` を付けて打ち切る。
const MAX_MATCHES: usize = 1000;

/// 出力が続く間の全体の数え直しの間隔。
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_millis(200);

/// 入力欄の字の大きさ（バーの件数・プレースホルダと同じ 12px）。
const FIELD_FONT_SIZE: f32 = 12.0;

/// 検索バーの状態。`None` = 閉じている。
pub(crate) struct TerminalSearch {
    /// 入力欄（IME の正しい `EditorView::plain`）。
    pub(crate) input: Entity<EditorView>,
    /// いま探している語（入力欄の写し。変換中は確定するまで前の語のまま）。
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) is_regex: bool,
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
    /// 入力欄の購読（打つたびに探す）と端末の束の受け口。閉じると一緒に落ちる。
    _subscriptions: Vec<Subscription>,
}

impl TerminalSearch {
    fn new(
        input: Entity<EditorView>,
        query: String,
        saved_offset: usize,
        subscriptions: Vec<Subscription>,
    ) -> Self {
        Self {
            input,
            query,
            case_sensitive: false,
            is_regex: false,
            regex: None,
            matches: Vec::new(),
            truncated: false,
            current: None,
            invalid: false,
            saved_offset,
            refresh_scheduled: false,
            _subscriptions: subscriptions,
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

/// 端末そのものへの束（検索を開く・クリア・タブを開く / 閉じる・分割）。入力欄の操作
/// （コピー・貼り付け・すべて選択）は含めない＝検索欄の中では入力欄の物。
fn is_terminal_action(action: &dyn Action) -> bool {
    let action = action.as_any();
    action.is::<actions::Find>()
        || action.is::<actions::Clear>()
        || action.is::<actions::NewTab>()
        || action.is::<actions::CloseTab>()
        || action.is::<actions::Split>()
}

/// 検索欄にフォーカスがある間、端末そのものの束（[`is_terminal_action`]）を照合の前に受けて端末へ回す。
///
/// 検索欄（`Editor`）は `Terminal` より一段深い。gpui は文脈の無い束を一番深い文脈と同じ深さに置くので、
/// 入力欄の中では全域の束（⌘F = バッファ内検索・⌘W = エディタのタブを閉じる・⌘\ = エディタの分割）と
/// `Editor` の束（⌘T = シンボル）が端末の束より先に当たり、裏のエディタへ効いてしまう。押したキーを
/// 「端末にフォーカスがある時の文脈」（入力欄の `Editor` を外した文脈）で引き直し、端末そのものの束に
/// 当たれば端末で受ける（検索を開く前と同じ結果）。文字・矢印・⌘C / ⌘V / ⌘A はそのまま入力欄へ。
fn intercept_terminal_keys(cx: &mut Context<TerminalView>) -> Subscription {
    let terminal = cx.entity().downgrade();
    cx.intercept_keystrokes(move |event, window, cx| {
        let Some(terminal) = terminal.upgrade() else {
            return;
        };
        let focus = {
            let view = terminal.read(cx);
            view.search.as_ref().map(|search| {
                (
                    search.input.read(cx).focus_handle(cx),
                    view.focus_handle.clone(),
                )
            })
        };
        let Some((input_focus, terminal_focus)) = focus else {
            return;
        };
        // 束の途中（⌘K の後など）は gpui の照合に任せる。
        if !input_focus.is_focused(window) || window.has_pending_keystrokes() {
            return;
        }
        let mut context_stack = event.context_stack.clone();
        if context_stack
            .last()
            .is_some_and(|context| context.contains("Editor"))
        {
            context_stack.pop();
        }
        let action = {
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let (bindings, pending) =
                keymap.bindings_for_input(std::slice::from_ref(&event.keystroke), &context_stack);
            if pending {
                return;
            }
            bindings
                .first()
                .map(|binding| binding.action().boxed_clone())
        };
        let Some(action) = action.filter(|action| is_terminal_action(action.as_ref())) else {
            return;
        };
        // 受け手の居ない置き場（端末をタブにしない所）では、入力欄の束へ流す。
        if !window.is_action_available_in(action.as_ref(), &terminal_focus) {
            return;
        }
        terminal_focus.dispatch_action(action.as_ref(), window, cx);
        cx.stop_propagation();
    })
}

impl TerminalView {
    /// ⌘F バーを開く（`query` = 語の初期値）。入力欄を作り、打つたびに探すよう購読する。
    pub(crate) fn open_search(&mut self, query: String, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let accent = self.accent;
        let input = cx.new(|cx| {
            let mut input = EditorView::plain(theme, accent, true, cx);
            // tab 幅は既定（4）のまま。
            input.set_typography(FIELD_FONT_SIZE, 4, cx);
            // 端末は出力が無ければ描き直さない（idle 0%）。キャレットは点滅させず常に出す（前と同じ）。
            input.set_caret_blink_enabled(false, cx);
            input.set_plain_text(&query, cx);
            input
        });
        let subscriptions = vec![
            cx.observe(&input, |view, _input, cx| view.on_search_input_changed(cx)),
            intercept_terminal_keys(cx),
        ];
        self.search = Some(TerminalSearch::new(
            input,
            query,
            self.content.display_offset,
            subscriptions,
        ));
        self.refresh_search(true, cx);
    }

    /// 入力欄が変わった（打鍵・貼り付け・取り消し・変換の確定）。変換中は探し直さずに確定を待つ
    /// （ローマ字・かなの途中の語で scrollback を全部走査し、表示を一致へ跳ねさせない）。
    fn on_search_input_changed(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let input = search.input.read(cx);
        if input.has_marked_text() {
            return;
        }
        let query = input.plain_text();
        // キャレットの点滅などの notify（語は変わっていない）。
        if query == search.query {
            return;
        }
        search.query = query;
        self.refresh_search(true, cx);
    }

    /// 検索欄で変換中（未確定の文字がある）か。
    fn search_composing(&self, cx: &App) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).has_marked_text())
    }

    /// 検索欄にフォーカスがあるか。
    pub(crate) fn search_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// 入力欄の ⏎ / ⇧⏎。変換中は何もしない（確定の ⏎ は確定だけ・次へ進まない）。
    fn step_search_from_field(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.search_composing(cx) {
            return;
        }
        self.step_search(delta, cx);
    }

    /// 検索欄へ貼る。1 行目だけ（改行を含む語は端末の行を跨いで当たらない）・選択は置き換える。
    pub(crate) fn paste_into_search(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let line = text.lines().next().unwrap_or_default();
        search
            .input
            .update(cx, |input, cx| input.insert_text(line, cx));
    }

    /// ⌘F バー（開いている時だけ）。エディタの ⌘F バーと同じ寸法（置換行なし）で、入力欄だけ EditorView。
    pub(crate) fn render_search_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let search = self.search.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent;
        let (input_focus, input_empty) = {
            let input = search.input.read(cx);
            (input.focus_handle(cx), input.buffer().len_bytes() == 0)
        };
        let focused = input_focus.is_focused(window);

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

        let field = div()
            .id("tsearch-query")
            .relative()
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
            // ⏎ / ⇧⏎ = 次 / 前。入力欄の改行より先に受ける（1 行の欄に改行は入れない）。
            .capture_action(cx.listener(|this, _: &editor_view::Newline, _window, cx| {
                cx.stop_propagation();
                this.step_search_from_field(1, cx);
            }))
            .capture_action(
                cx.listener(|this, _: &editor_view::InsertNewline, _window, cx| {
                    cx.stop_propagation();
                    this.step_search_from_field(-1, cx);
                }),
            )
            // ⌘V は 1 行目だけ（前と同じ）。
            .capture_action(cx.listener(|this, _: &editor_view::Paste, _window, cx| {
                cx.stop_propagation();
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    this.paste_into_search(&text, cx);
                }
            }))
            // esc = 開いた位置へ戻って閉じる（入力欄は複数カーソルを畳む時以外 `editor::Cancel` を
            // 親へ流す）。変換中の esc は変換の取り消し（IME の物）。
            .on_action(cx.listener(|this, _: &editor_view::Cancel, window, cx| {
                if !this.search_composing(cx) {
                    this.close_search(true, window, cx);
                }
            }))
            // 入力欄はこの欄（高さ 24）いっぱい。親が高さを決めないと 0 に潰れる。
            .child(search.input.clone())
            .when(input_empty, |field| {
                // 空の欄のキャレット（行頭・幅 2px）に重ならないよう、その右から。
                field.child(
                    div()
                        .absolute()
                        .inset_0()
                        .pl(px(10.))
                        .pr(px(8.))
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(theme.fg2)
                        .child(SharedString::from(i18n::t!("search.find_placeholder"))),
                )
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
                .font_family(ui::ui_font(cx))
                .cursor(gpui::CursorStyle::Arrow)
                // バー内のクリックは端末（選択・マウス報告）へ通さず、入力欄へフォーカスを戻す。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, window, cx| {
                        cx.stop_propagation();
                        window.focus(&input_focus, cx);
                    }),
                )
                .child(row)
                .into_any_element(),
        )
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

/// 検索欄（EditorView）のキーの受け方。既定の keymap と同じ並び（入力欄 → 全域 → 端末）で束を張る。
#[cfg(test)]
mod field_tests {
    use super::*;
    use alacritty_terminal::index::Side;
    use alacritty_terminal::selection::{Selection, SelectionType};
    use gpui::{ClipboardItem, EntityInputHandler, KeyBinding, TestAppContext, VisualTestContext};
    use theme_core::Theme;

    // workspace の全域の束（⌘F = バッファ内検索・⌘W = タブを閉じる）の代わり。
    gpui::actions!(search_field_test, [GlobalFind, GlobalClose]);

    /// 端末を置く面（ドック・workspace の代わり）: 端末の ⌘W と、全域の束が届いた数を数える。
    struct Host {
        terminal: Entity<TerminalView>,
        closed: usize,
        global_find: usize,
        global_close: usize,
    }

    impl Render for Host {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_action(cx.listener(|host, _: &actions::CloseTab, _window, _cx| {
                    host.closed += 1;
                }))
                .on_action(cx.listener(|host, _: &GlobalFind, _window, _cx| {
                    host.global_find += 1;
                }))
                .on_action(cx.listener(|host, _: &GlobalClose, _window, _cx| {
                    host.global_close += 1;
                }))
                // 下ドックくらいの高さ（十数行）。出力の上の方は scrollback に入る。
                .child(div().w(px(800.)).h(px(240.)).child(self.terminal.clone()))
        }
    }

    /// `item0 hit` 〜 `item39 hit` を出した端末（上の方は scrollback）にフォーカスした窓。
    fn fixture(
        cx: &mut TestAppContext,
    ) -> (Entity<Host>, Entity<TerminalView>, &mut VisualTestContext) {
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("enter", editor_view::Newline, Some("Editor")),
                KeyBinding::new("shift-enter", editor_view::InsertNewline, Some("Editor")),
                KeyBinding::new("escape", editor_view::Cancel, Some("Editor")),
                KeyBinding::new("backspace", editor_view::Backspace, Some("Editor")),
                KeyBinding::new("left", editor_view::MoveLeft, Some("Editor")),
                KeyBinding::new("cmd-a", editor_view::SelectAll, Some("Editor")),
                KeyBinding::new("cmd-c", editor_view::Copy, Some("Editor")),
                KeyBinding::new("cmd-v", editor_view::Paste, Some("Editor")),
                KeyBinding::new("cmd-f", GlobalFind, None),
                KeyBinding::new("cmd-w", GlobalClose, None),
                KeyBinding::new("cmd-a", actions::SelectAll, Some("Terminal")),
                KeyBinding::new("cmd-c", actions::Copy, Some("Terminal")),
                KeyBinding::new("cmd-v", actions::Paste, Some("Terminal")),
                KeyBinding::new("cmd-f", actions::Find, Some("Terminal")),
                KeyBinding::new("cmd-k", actions::Clear, Some("Terminal")),
                KeyBinding::new("cmd-w", actions::CloseTab, Some("Terminal")),
            ]);
        });
        let (host, cx) = cx.add_window_view(|_window, cx| Host {
            terminal: cx.new(|cx| TerminalView::new_test(Theme::dark(), cx)),
            closed: 0,
            global_find: 0,
            global_close: 0,
        });
        let terminal = host.read_with(cx, |host, _| host.terminal.clone());
        terminal.update_in(cx, |terminal, window, cx| {
            let output: String = (0..40)
                .map(|index| format!("item{index} hit\r\n"))
                .collect();
            terminal.feed_output_for_test(output.as_bytes());
            terminal.sync(cx);
            window.focus(&terminal.focus_handle, cx);
        });
        (host, terminal, cx)
    }

    fn query(terminal: &Entity<TerminalView>, cx: &mut VisualTestContext) -> String {
        terminal.read_with(cx, |terminal, _| {
            terminal
                .search
                .as_ref()
                .map(|search| search.query.clone())
                .unwrap_or_default()
        })
    }

    fn field_text(terminal: &Entity<TerminalView>, cx: &mut VisualTestContext) -> String {
        terminal.read_with(cx, |terminal, cx| {
            terminal
                .search
                .as_ref()
                .map(|search| search.input.read(cx).plain_text())
                .unwrap_or_default()
        })
    }

    fn match_state(
        terminal: &Entity<TerminalView>,
        cx: &mut VisualTestContext,
    ) -> (usize, Option<usize>) {
        terminal.read_with(cx, |terminal, _| {
            terminal
                .search
                .as_ref()
                .map(|search| (search.matches.len(), search.current))
                .unwrap_or_default()
        })
    }

    fn display_offset(terminal: &Entity<TerminalView>, cx: &mut VisualTestContext) -> usize {
        terminal.read_with(cx, |terminal, _| {
            terminal.term.lock().grid().display_offset()
        })
    }

    fn field_focused(terminal: &Entity<TerminalView>, cx: &mut VisualTestContext) -> bool {
        terminal.update_in(cx, |terminal, window, cx| {
            terminal.search_input_focused(window, cx)
        })
    }

    /// 打つたびに一致が更新される。選択しての置き換え・カーソル移動・貼り付け（1 行目だけ）も
    /// 入力欄で効き、どの文字も PTY へは流れない。
    #[gpui::test]
    fn typing_in_the_field_updates_the_matches(cx: &mut TestAppContext) {
        let (_host, terminal, cx) = fixture(cx);
        cx.simulate_keystrokes("cmd-f");
        assert!(field_focused(&terminal, cx), "⌘F で開いて入力欄へ");
        cx.simulate_input("hit");
        assert_eq!(query(&terminal, cx), "hit");
        assert_eq!(
            match_state(&terminal, cx),
            (40, Some(39)),
            "開いた所に一番近い一致"
        );
        // ⌘A で選んで打つと置き換わる。
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("item3");
        assert_eq!(query(&terminal, cx), "item3");
        assert_eq!(match_state(&terminal, cx).0, 11, "item3 と item30〜39");
        // ← で戻った所へ入る。
        cx.simulate_keystrokes("left");
        cx.simulate_input("1");
        assert_eq!(query(&terminal, cx), "item13");
        assert_eq!(match_state(&terminal, cx).0, 1);
        cx.simulate_keystrokes("backspace");
        assert_eq!(query(&terminal, cx), "item3");
        // 貼り付けは 1 行目だけ（選択は置き換える）。
        cx.update(|_window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("item25 hit\nsecond line".into()))
        });
        cx.simulate_keystrokes("cmd-a cmd-v");
        assert_eq!(field_text(&terminal, cx), "item25 hit");
        assert_eq!(match_state(&terminal, cx).0, 1);
        assert!(
            terminal.read_with(cx, |terminal, _| terminal.debug_written_input().is_empty()),
            "検索欄に打った物は PTY へ流れない"
        );
    }

    /// ⏎ / ⇧⏎ で次 / 前（端で折り返し・欄に改行は入らない）。esc で閉じて開いた位置へ戻り、
    /// 打鍵は端末へ戻る。
    #[gpui::test]
    fn enter_steps_and_escape_returns_to_where_it_opened(cx: &mut TestAppContext) {
        let (_host, terminal, cx) = fixture(cx);
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("hit");
        assert_eq!(match_state(&terminal, cx), (40, Some(39)));
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(match_state(&terminal, cx).1, Some(38), "⇧⏎ = 前");
        cx.simulate_keystrokes("enter");
        assert_eq!(match_state(&terminal, cx).1, Some(39), "⏎ = 次");
        cx.simulate_keystrokes("enter");
        assert_eq!(match_state(&terminal, cx).1, Some(0), "端で先頭へ");
        assert!(
            display_offset(&terminal, cx) > 0,
            "scrollback の一致まで遡る"
        );
        assert_eq!(field_text(&terminal, cx), "hit", "欄に改行は入らない");
        assert!(field_focused(&terminal, cx));

        cx.simulate_keystrokes("escape");
        assert!(!terminal.read_with(cx, |terminal, _| terminal.search_open()));
        assert_eq!(display_offset(&terminal, cx), 0, "開いた位置へ戻る");
        let terminal_focused = terminal.update_in(cx, |terminal, window, _cx| {
            terminal.focus_handle.is_focused(window)
        });
        assert!(terminal_focused, "閉じたら端末へ打鍵が戻る");
        assert!(terminal.read_with(cx, |terminal, _| terminal.debug_written_input().is_empty()));
    }

    /// 変換中（未確定の文字がある間）は探し直さず、確定で探す。変換中に届いた ⏎ / ⇧⏎ は
    /// 何もしない（次へ進まず、欄に改行も入れない）。esc も変換中は閉じない（取り消しは IME の物）。
    #[gpui::test]
    fn composition_is_searched_only_when_confirmed(cx: &mut TestAppContext) {
        let (_host, terminal, cx) = fixture(cx);
        terminal.update_in(cx, |terminal, _window, cx| {
            terminal.feed_output_for_test("猫の hit\r\n犬\r\n猫\r\n".as_bytes());
            terminal.sync(cx);
        });
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("hit");
        assert_eq!(match_state(&terminal, cx), (41, Some(40)));
        let input = terminal.read_with(cx, |terminal, _| {
            terminal
                .search
                .as_ref()
                .map(|search| search.input.clone())
                .expect("開いている")
        });
        // 語を選んで「ねこ」を変換中にする（確定するまで前の語「hit」のまま）。
        cx.simulate_keystrokes("cmd-a");
        input.update_in(cx, |input, window, cx| {
            input.replace_and_mark_text_in_range(None, "ねこ", None, window, cx)
        });
        cx.run_until_parked();
        assert_eq!(field_text(&terminal, cx), "ねこ");
        assert_eq!(query(&terminal, cx), "hit", "変換中は探し直さない");
        assert_eq!(match_state(&terminal, cx), (41, Some(40)));
        for key in ["enter", "shift-enter", "escape"] {
            cx.simulate_keystrokes(key);
            assert_eq!(
                match_state(&terminal, cx),
                (41, Some(40)),
                "変換中の {key} で進まない"
            );
            assert_eq!(field_text(&terminal, cx), "ねこ", "欄に改行は入らない");
        }
        assert!(
            terminal.read_with(cx, |terminal, _| terminal.search_open()),
            "変換中の esc で閉じない"
        );

        // 確定（IME の確定の ⏎ はこの経路で届く）。確定したら探すが、次へは進まない。
        input.update_in(cx, |input, window, cx| {
            input.replace_text_in_range(None, "猫", window, cx)
        });
        cx.run_until_parked();
        assert_eq!(query(&terminal, cx), "猫");
        assert_eq!(
            match_state(&terminal, cx),
            (2, Some(1)),
            "一番新しい一致を見る（確定で次へ進まない）"
        );
        cx.simulate_keystrokes("shift-enter");
        assert_eq!(
            match_state(&terminal, cx).1,
            Some(0),
            "確定の後の ⇧⏎ は前へ"
        );
    }

    /// 端末そのものの束は入力欄の中でも端末の物: ⌘F は全域の束（バッファ内検索）に取られず入力欄に
    /// 残り、⌘K は画面を消し、⌘W は端末を閉じる（全域の ⌘W＝裏のタブを閉じる、に化けない）。
    #[gpui::test]
    fn terminal_keys_stay_with_the_terminal_in_the_field(cx: &mut TestAppContext) {
        let (host, terminal, cx) = fixture(cx);
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("hit");
        cx.simulate_keystrokes("cmd-f");
        assert!(field_focused(&terminal, cx), "⌘F は入力欄へ戻すだけ");
        assert_eq!(query(&terminal, cx), "hit");
        cx.simulate_keystrokes("cmd-k");
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.term.lock().grid().history_size()),
            0,
            "⌘K は端末を消す"
        );
        cx.simulate_keystrokes("cmd-w");
        host.read_with(cx, |host, _| {
            assert_eq!(host.closed, 1, "⌘W は端末を閉じる");
            assert_eq!(host.global_find, 0, "全域の ⌘F に取られない");
            assert_eq!(host.global_close, 0, "全域の ⌘W に化けない");
        });
        // 端末にフォーカスがある時も同じ（検索を開く前からの振る舞い）。
        terminal.update_in(cx, |terminal, window, cx| {
            window.focus(&terminal.focus_handle, cx);
        });
        cx.simulate_keystrokes("cmd-w");
        host.read_with(cx, |host, _| {
            assert_eq!(host.closed, 2);
            assert_eq!(host.global_close, 0);
        });
        // 入力欄の ⌘A は入力欄の物（端末をすべて選択しない）。
        cx.simulate_keystrokes("cmd-f cmd-a");
        assert!(terminal.read_with(cx, |terminal, _| terminal.term.lock().selection.is_none()));
    }

    /// 欄の中の ⌘C は欄の選択をコピーし、欄に選択が無ければ端末の選択をコピーする。
    #[gpui::test]
    fn copy_in_the_field_prefers_the_field_selection(cx: &mut TestAppContext) {
        let (_host, terminal, cx) = fixture(cx);
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("item7");
        cx.simulate_keystrokes("cmd-a cmd-c");
        let copied = cx.update(|_window, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(copied.as_deref(), Some("item7"), "欄の選択");

        // 欄の選択を外し、端末の 1 行を選んでおく。
        cx.simulate_keystrokes("left");
        terminal.update(cx, |terminal, _cx| {
            let start = AlacPoint::new(Line(0), Column(0));
            let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
            selection.update(AlacPoint::new(Line(0), Column(9)), Side::Right);
            terminal.term.lock().selection = Some(selection);
        });
        cx.simulate_keystrokes("cmd-c");
        let copied = cx.update(|_window, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        let expected =
            terminal.read_with(cx, |terminal, _| terminal.term.lock().selection_to_string());
        assert!(expected.as_deref().is_some_and(|text| !text.is_empty()));
        assert_eq!(copied, expected, "欄に選択が無ければ端末の選択");
        assert!(field_focused(&terminal, cx), "コピーしても入力欄のまま");
    }
}
