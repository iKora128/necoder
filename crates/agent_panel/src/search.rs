//! search — **transcript 内の検索**（⌘F）。
//!
//! チャットの一覧の検索（`chat.rs`・どの会話かを探す）とは別で、こちらは**いま開いている会話の中**を
//! 探す。長い会話で「さっきの手順どこだっけ」を見つけるための道具。
//!
//! 作りは 3 つ:
//!
//! 1. **一致の強調**は [`AgentPanel::push_selectable_with_links`] の 1 か所に足す。transcript の文字は
//!    全部そこを通るので、本文・コード・ツールの出力・差分のどれに在っても同じように光る
//! 2. **移動はエントリ単位**（⏎ / ⇧⏎・↑↓ ボタン）。1 エントリの中の何個目かまでは追わず、
//!    **いま居るエントリの一致だけ濃く**して「どこを見ているか」を示す
//! 3. 数える一致は**エントリの素のテキスト**から数える（描画は可視範囲しか走らないので、
//!    描画から数えると画面外の一致が落ちる）
//!
//! 大文字小文字は **ASCII だけ**無視する。`to_lowercase()` は文字によってバイト長が変わり、
//! 強調に使うバイト範囲がずれる（日本語には大文字小文字が無いので実用上これで足りる）。

use super::*;

/// 検索バーの状態。`None` = 閉じている。
pub(crate) struct TranscriptSearch {
    /// 入力欄（IME の正しい `EditorView::plain`）。
    pub(crate) input: Entity<EditorView>,
    /// いまの検索語（入力欄の写し。描画のたびに読み直さない）。
    pub(crate) query: String,
    /// 一致を含むエントリの添字（昇順）。
    pub(crate) entries: Vec<usize>,
    /// 一致の総数（エントリ数ではなく出現回数）。
    pub(crate) total: usize,
    /// `entries` の中でいま見ている位置。
    pub(crate) current: usize,
}

impl AgentPanel {
    /// ⌘F。開いて入力欄へフォーカスする。開いている時は入力欄を選び直す（押し直しで探し直せる）。
    pub(crate) fn open_transcript_search(
        &mut self,
        _: &FindInTranscript,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transcript_search.is_none() {
            let theme = self.theme.clone();
            let color = self.active_color();
            let input = cx.new(|cx| EditorView::plain(theme, color, true, cx));
            // 入力のたびに数え直す（打ちながら結果が変わる）。
            cx.observe(&input, |panel, _, cx| panel.refresh_transcript_search(cx))
                .detach();
            // 入力欄の Enter は「次の一致へ」（送信ではない）。
            cx.subscribe(&input, |panel, _, event: &ComposerEvent, cx| {
                if matches!(event, ComposerEvent::Submit) {
                    panel.step_transcript_match(1, cx);
                }
            })
            .detach();
            self.transcript_search = Some(TranscriptSearch {
                input,
                query: String::new(),
                entries: Vec::new(),
                total: 0,
                current: 0,
            });
        }
        if let Some(search) = &self.transcript_search {
            let handle = search.input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    /// 開発用（offscreen 検証）: 検索バーを開いて語を入れる。
    #[cfg(debug_assertions)]
    pub fn debug_find_in_transcript(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_transcript_search(&FindInTranscript, window, cx);
        if let Some(search) = &self.transcript_search {
            search
                .input
                .update(cx, |input, cx| input.set_plain_text(query, cx));
        }
        self.refresh_transcript_search(cx);
    }

    /// 履歴ビューの全文検索の一致へ飛ぶ（O15）: 検索バーを `query` で開き、一致のうち**最後**の
    /// エントリを見せる（DB の検索が返すのは、そのスレッドで最新の一致）。一致が無ければ開くだけ。
    pub fn reveal_transcript_match(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_transcript_search(&FindInTranscript, window, cx);
        if let Some(search) = &self.transcript_search {
            search
                .input
                .update(cx, |input, cx| input.set_plain_text(query, cx));
        }
        self.refresh_transcript_search(cx);
        let Some(search) = &mut self.transcript_search else {
            return;
        };
        let Some(last) = search.entries.len().checked_sub(1) else {
            return;
        };
        search.current = last;
        let entry = search.entries[last];
        self.transcript_list.scroll_to_reveal_item(entry);
        cx.notify();
    }

    /// 閉じる（Esc・✕）。閉じたら強調も消える。
    pub(crate) fn close_transcript_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transcript_search.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn transcript_search_open(&self) -> bool {
        self.transcript_search.is_some()
    }

    /// 入力が変わった / スレッドが変わった / 会話が伸びた → 数え直す。
    pub(crate) fn refresh_transcript_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &self.transcript_search else {
            return;
        };
        let query = search.input.read(cx).plain_text();
        let previous = search.entries.get(search.current).copied();
        let (entries, total) = self.count_transcript_matches(&query);
        // 数え直しても「いま見ているエントリ」を保つ（打ち足すたびに先頭へ戻らない）。
        let current = previous
            .and_then(|entry| entries.iter().position(|candidate| *candidate >= entry))
            .unwrap_or(0);
        let Some(search) = &mut self.transcript_search else {
            return;
        };
        search.query = query;
        search.entries = entries;
        search.total = total;
        search.current = current.min(search.entries.len().saturating_sub(1));
        cx.notify();
    }

    fn count_transcript_matches(&self, query: &str) -> (Vec<usize>, usize) {
        let query = query.trim();
        if query.is_empty() {
            return (Vec::new(), 0);
        }
        let Some(thread) = self.threads.get(self.active) else {
            return (Vec::new(), 0);
        };
        let mut entries = Vec::new();
        let mut total = 0;
        for (index, entry) in thread.entries.iter().enumerate() {
            let count = match_ranges(&entry_plain_text(entry), query).len();
            if count > 0 {
                entries.push(index);
                total += count;
            }
        }
        (entries, total)
    }

    /// 次 / 前の一致へ（`delta` = +1 / -1）。端で折り返す。
    pub(crate) fn step_transcript_match(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(search) = &mut self.transcript_search else {
            return;
        };
        let count = search.entries.len();
        if count == 0 {
            return;
        }
        let next = (search.current as isize + delta).rem_euclid(count as isize) as usize;
        search.current = next;
        let entry = search.entries[next];
        self.transcript_list.scroll_to_reveal_item(entry);
        cx.notify();
    }

    /// いま強調すべきエントリ（濃い方）。
    fn current_search_entry(&self) -> Option<usize> {
        let search = self.transcript_search.as_ref()?;
        search.entries.get(search.current).copied()
    }

    /// transcript の 1 区画ぶんの検索強調。`item` は描画中のエントリの添字。
    pub(crate) fn transcript_search_highlights(
        &self,
        item: usize,
        text: &str,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(search) = &self.transcript_search else {
            return Vec::new();
        };
        let query = search.query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        // いま見ているエントリだけ濃く（スレッド色）。ほかは淡い面（どこに在るかが分かる程度）。
        let color = if self.current_search_entry() == Some(item) {
            self.active_color().alpha(0.45)
        } else {
            self.theme.fg2.alpha(0.28)
        };
        match_ranges(text, query)
            .into_iter()
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        background_color: Some(color),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    pub(crate) fn render_transcript_search(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let search = self.transcript_search.as_ref()?;
        let theme = self.theme.clone();
        let has_query = !search.query.trim().is_empty();
        let count: SharedString = if !has_query {
            SharedString::default()
        } else if search.total == 0 {
            i18n::t!("agent.search_none").into()
        } else {
            i18n::t!(
                "agent.search_count",
                "position" => search.current + 1,
                "entries" => search.entries.len(),
                "total" => search.total
            )
            .into()
        };
        let step_button = |id: &'static str, glyph: &'static str, tip: String, delta: isize| {
            div()
                .id(id)
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .child(glyph)
                .tooltip(Tooltip::text(tip, theme.clone()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |panel, _, _window, cx| {
                        cx.stop_propagation();
                        panel.step_transcript_match(delta, cx);
                    }),
                )
        };
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .py(px(5.))
                .bg(theme.bg1)
                .border_b_1()
                .border_color(theme.border)
                .text_size(px(11.5))
                .child(div().flex_none().text_color(theme.fg2).child("⌕"))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        // 高さを与えないと 1 行ぶんに潰れて文字が出ない（チャットの一覧の検索欄と同じ）。
                        .h(px(18.))
                        .child(search.input.clone())
                        .when(!has_query, |field| {
                            field.child(
                                div()
                                    .absolute()
                                    .top(px(0.))
                                    .left(px(1.))
                                    .text_color(theme.fg2)
                                    .child(SharedString::from(i18n::t!(
                                        "agent.search_placeholder"
                                    ))),
                            )
                        }),
                )
                .child(div().flex_none().text_color(theme.fg2).child(count))
                .child(step_button(
                    "transcript-search-prev",
                    "↑",
                    i18n::t!("agent.search_prev"),
                    -1,
                ))
                .child(step_button(
                    "transcript-search-next",
                    "↓",
                    i18n::t!("agent.search_next"),
                    1,
                ))
                .child(
                    div()
                        .id("transcript-search-close")
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child("✕")
                        .tooltip(Tooltip::text(i18n::t!("agent.search_close"), theme.clone()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |panel, _, window, cx| {
                                cx.stop_propagation();
                                panel.close_transcript_search(window, cx);
                            }),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// `haystack` の中の `needle` の出現（バイト範囲・昇順・重なりなし）。
///
/// 大文字小文字は **ASCII だけ**無視する（バイト長が変わらない＝範囲がずれない）。
/// 見つけた位置が文字の境界でなければ捨てる（多バイト文字の途中に当たる偽の一致を弾く）。
fn match_ranges(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    let hay = haystack.as_bytes().to_ascii_lowercase();
    let pin = needle.as_bytes().to_ascii_lowercase();
    let mut ranges = Vec::new();
    let mut start = 0;
    while start + pin.len() <= hay.len() {
        if hay[start..start + pin.len()] == pin[..]
            && haystack.is_char_boundary(start)
            && haystack.is_char_boundary(start + pin.len())
        {
            ranges.push(start..start + pin.len());
            start += pin.len();
        } else {
            start += 1;
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ignore_ascii_case_and_keep_byte_ranges_exact() {
        assert_eq!(match_ranges("Hello hello", "hello"), vec![0..5, 6..11]);
        // 日本語（大文字小文字が無い）。バイト範囲がそのまま使える。
        let text = "タイマーとタイマー";
        assert_eq!(match_ranges(text, "タイマー"), vec![0..12, 15..27]);
        for range in match_ranges(text, "タイマー") {
            assert_eq!(&text[range], "タイマー");
        }
        // 重なりは作らない（`aa` の中の `aa` は 1 つ）。
        assert_eq!(match_ranges("aaa", "aa"), vec![0..2]);
        assert!(match_ranges("abc", "").is_empty());
        assert!(match_ranges("abc", "abcd").is_empty());
    }

    /// 多バイト文字の途中に当たるバイト列は一致にしない（強調が文字を割らない）。
    #[test]
    fn a_match_inside_a_multibyte_character_is_rejected() {
        // 「ー」= E3 83 BC。「ソ」= E3 82 BD。並びの中の継続バイトに引っかからないこと。
        let text = "ソーダ";
        for range in match_ranges(text, "ー") {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            assert_eq!(&text[range], "ー");
        }
    }

    #[gpui::test]
    fn the_search_counts_entries_and_occurrences_and_walks_them(cx: &mut gpui::TestAppContext) {
        let path = std::env::temp_dir().join(format!(
            "necoder_agent_search_{}_{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::write(&path, r#"{"onboarded":true,"agent_prewarm":false}"#).unwrap();
        cx.update(|cx| settings::init(Some(path.clone()), None, cx));
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update_in(cx, |panel, window, cx| {
            let active = panel.active;
            panel.threads[active].entries = vec![
                Entry::User("タイマーを作って".into()),
                Entry::Agent("関係のない返事".into()),
                Entry::Agent("タイマーを作りました。タイマーは 25 分です".into()),
            ];
            panel.open_transcript_search(&FindInTranscript, window, cx);
            let input = panel.transcript_search.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| input.set_plain_text("タイマー", cx));
            panel.refresh_transcript_search(cx);

            let search = panel.transcript_search.as_ref().unwrap();
            assert_eq!(search.entries, vec![0, 2], "一致したエントリ");
            assert_eq!(search.total, 3, "出現の総数");
            assert_eq!(search.current, 0);

            // 強調: いま見ているエントリは濃く、ほかは淡く。一致の無いエントリには出さない。
            let strong = panel.transcript_search_highlights(0, "タイマーを作って");
            let faint = panel.transcript_search_highlights(2, "タイマーを作りました");
            assert_eq!(strong.len(), 1);
            assert_eq!(faint.len(), 1);
            assert_ne!(strong[0].1.background_color, faint[0].1.background_color);
            assert!(panel
                .transcript_search_highlights(1, "関係のない返事")
                .is_empty());

            // 移動は端で折り返す。
            panel.step_transcript_match(1, cx);
            assert_eq!(panel.transcript_search.as_ref().unwrap().current, 1);
            panel.step_transcript_match(1, cx);
            assert_eq!(panel.transcript_search.as_ref().unwrap().current, 0);
            panel.step_transcript_match(-1, cx);
            assert_eq!(panel.transcript_search.as_ref().unwrap().current, 1);

            // 閉じたら強調も消える。
            panel.close_transcript_search(window, cx);
            assert!(!panel.transcript_search_open());
            assert!(panel
                .transcript_search_highlights(0, "タイマーを作って")
                .is_empty());
        });
        let _ = std::fs::remove_file(path);
    }
}
