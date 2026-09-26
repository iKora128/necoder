//! Project search panel. This crate is intentionally independent from the workspace shell.

use gpui::{
    div, prelude::*, px, Context, EventEmitter, FocusHandle, FontWeight, Hsla, IntoElement,
    KeyDownEvent, MouseButton, Render, SharedString, Window,
};
use host::Host;
use search::FileMatch;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use theme_core::Theme;
use ui::Tooltip;

const FILE_LIMIT: usize = 5_000;
const MAX_ROWS: usize = 300;

/// SearchPanel から shell への通知。
pub enum SearchPanelEvent {
    OpenMatch {
        path: PathBuf,
        line: usize,
        column: usize,
    },
    Dismissed,
}

/// Query、結果、非同期世代と描画を所有する project search Entity。
pub struct SearchPanel {
    host: Arc<dyn Host>,
    root: PathBuf,
    /// 検索をこのフォルダの中だけに絞る（エクスプローラの「フォルダ内を検索」・D18）。
    /// `None` = プロジェクト全体。結果の見出しは絞っても root からの相対で出す。
    scope: Option<PathBuf>,
    query: String,
    case_sensitive: bool,
    is_regex: bool,
    results: Vec<FileMatch>,
    results_query: Option<String>,
    selected: usize,
    error: Option<SharedString>,
    running: bool,
    active_search: Option<u64>,
    next_search_id: u64,
    focus: FocusHandle,
    return_focus: FocusHandle,
    theme: Theme,
    accent: Hsla,
}

impl SearchPanel {
    pub fn new(
        host: Arc<dyn Host>,
        root: PathBuf,
        theme: Theme,
        accent: Hsla,
        return_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            host,
            root,
            scope: None,
            query: String::new(),
            case_sensitive: false,
            is_regex: false,
            results: Vec::new(),
            results_query: None,
            selected: 0,
            error: None,
            running: false,
            active_search: None,
            next_search_id: 1,
            focus: cx.focus_handle(),
            return_focus,
            theme,
            accent,
        }
    }

    pub fn with_results(
        host: Arc<dyn Host>,
        root: PathBuf,
        query: String,
        results: Vec<FileMatch>,
        theme: Theme,
        accent: Hsla,
        return_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(host, root, theme, accent, return_focus, cx);
        panel.results_query = Some(query.clone());
        panel.query = query;
        panel.case_sensitive = true;
        panel.results = results;
        panel
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// クエリを直接セットして検索する（開発プローブ / プログラム操作用）。
    pub fn set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        self.query = query.into();
        self.run_search(cx);
    }

    pub fn scope(&self) -> Option<&Path> {
        self.scope.as_deref()
    }

    /// 検索の範囲を 1 フォルダに絞る / 外す（`None`）。クエリがあればその範囲で検索し直す。
    pub fn set_scope(&mut self, scope: Option<PathBuf>, cx: &mut Context<Self>) {
        // root そのもの（や root の外）はプロジェクト全体と同じ扱い＝チップを出さない。
        let scope = scope.filter(|folder| folder != &self.root && folder.starts_with(&self.root));
        if scope == self.scope {
            return;
        }
        self.scope = scope;
        if self.query.trim().is_empty() {
            self.results.clear();
            self.results_query = None;
            cx.notify();
        } else {
            self.run_search(cx);
        }
    }

    pub fn set_theme(&mut self, theme: Theme, accent: Hsla, cx: &mut Context<Self>) {
        self.theme = theme;
        self.accent = accent;
        cx.notify();
    }

    fn flat(&self) -> Vec<(usize, usize)> {
        self.results
            .iter()
            .enumerate()
            .flat_map(|(file_index, file)| {
                (0..file.matches.len()).map(move |match_index| (file_index, match_index))
            })
            .collect()
    }

    fn total_matches(&self) -> usize {
        self.results.iter().map(|file| file.matches.len()).sum()
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query_text = self.query.clone();
        let is_regex = self.is_regex;
        let case_sensitive = self.case_sensitive;
        if query_text.trim().is_empty() {
            self.results.clear();
            self.results_query = Some(query_text);
            self.error = None;
            self.selected = 0;
            self.running = false;
            self.active_search = None;
            cx.notify();
            return;
        }
        let query = match search::SearchQuery::new(&query_text, is_regex, case_sensitive) {
            Ok(query) => query,
            Err(error) => {
                self.results.clear();
                self.results_query = Some(query_text);
                self.error = Some(SharedString::from(format!("{error}")));
                self.selected = 0;
                self.running = false;
                self.active_search = None;
                cx.notify();
                return;
            }
        };
        let host = self.host.clone();
        let scope = self.scope.clone();
        let search_root = scope.clone().unwrap_or_else(|| self.root.clone());
        let search_id = self.next_search_id;
        self.next_search_id = self.next_search_id.wrapping_add(1).max(1);
        self.running = true;
        self.active_search = Some(search_id);
        self.error = None;
        cx.notify();
        cx.spawn(async move |panel, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    query.try_search_project_on(host.as_ref(), &search_root, FILE_LIMIT, MAX_ROWS)
                })
                .await;
            let _ = panel.update(cx, |panel, cx| {
                if panel.active_search != Some(search_id)
                    || panel.query != query_text
                    || panel.is_regex != is_regex
                    || panel.case_sensitive != case_sensitive
                    || panel.scope != scope
                {
                    return;
                }
                match outcome {
                    Ok(results) => {
                        panel.results = results;
                        panel.error = None;
                    }
                    Err(error) => {
                        panel.results.clear();
                        panel.error = Some(SharedString::from(format!("{error:#}")));
                    }
                }
                panel.results_query = Some(query_text);
                panel.selected = 0;
                panel.running = false;
                panel.active_search = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.flat().len() as isize;
        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.results_query.as_deref() != Some(self.query.as_str()) {
            return false;
        }
        let Some((file_index, match_index)) = self.flat().get(self.selected).copied() else {
            return false;
        };
        let Some(file) = self.results.get(file_index) else {
            return false;
        };
        let Some(found) = file.matches.get(match_index) else {
            return false;
        };
        cx.emit(SearchPanelEvent::OpenMatch {
            path: file.path.clone(),
            line: found.line,
            column: found.column,
        });
        window.focus(&self.return_focus, cx);
        true
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.return_focus, cx);
        cx.emit(SearchPanelEvent::Dismissed);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss(window, cx),
            "enter" => {
                if !self.open_selected(window, cx) {
                    self.run_search(cx);
                }
            }
            "up" => self.move_selection(-1, cx),
            "down" => self.move_selection(1, cx),
            "backspace" => {
                // 空の入力でもう一度 ⌫ = 範囲のチップを外す（入力欄のトークンと同じ所作）。
                if self.query.is_empty() && self.scope.is_some() {
                    self.set_scope(None, cx);
                    return;
                }
                self.query.pop();
                cx.notify();
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !text.chars().any(char::is_control) {
                        self.query.push_str(text);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn toggle_case(&mut self, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        self.run_search(cx);
    }

    fn toggle_regex(&mut self, cx: &mut Context<Self>) {
        self.is_regex = !self.is_regex;
        self.run_search(cx);
    }
}

impl EventEmitter<SearchPanelEvent> for SearchPanel {}

impl Render for SearchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.accent;
        let query_display = if self.query.is_empty() && self.scope.is_some() {
            SharedString::from(i18n::t!("searchpanel.placeholder_scoped"))
        } else if self.query.is_empty() {
            SharedString::from(i18n::t!("searchpanel.placeholder"))
        } else {
            SharedString::from(self.query.clone())
        };
        let query_color = if self.query.is_empty() {
            theme.fg2
        } else {
            theme.fg0
        };
        let toggle = |id, label, active, tip| {
            div()
                .id(id)
                .flex()
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
        let summary = match &self.error {
            _ if self.running => SharedString::from(i18n::t!("searchpanel.searching")),
            Some(error) => error.clone(),
            None if self.results_query.is_some() => SharedString::from(i18n::t!(
                "searchpanel.results",
                "n" => self.total_matches(),
                "files" => self.results.len()
            )),
            None => SharedString::from(i18n::t!("searchpanel.hint_enter")),
        };
        let summary_color = if self.error.is_some() {
            theme.err
        } else {
            theme.fg2
        };

        let mut rows = Vec::new();
        let mut flat_index = 0usize;
        let mut truncated = false;
        'files: for (file_index, file) in self.results.iter().enumerate() {
            let relative = file
                .path
                .strip_prefix(&self.root)
                .unwrap_or(file.path.as_path())
                .to_string_lossy()
                .to_string();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pt(px(7.))
                    .pb(px(2.))
                    .child(div().text_color(theme.fg2).child("⌕"))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.fg1)
                            .child(SharedString::from(relative)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(file.matches.len().to_string())),
                    )
                    .into_any_element(),
            );
            for (match_index, found) in file.matches.iter().enumerate() {
                if flat_index >= MAX_ROWS {
                    truncated = true;
                    break 'files;
                }
                let raw = found.line_text.trim_end();
                let lead = raw.len() - raw.trim_start().len();
                let line = &raw[lead..];
                let start = found.column.saturating_sub(lead).min(line.len());
                let end = (found.column + found.byte_range.len())
                    .saturating_sub(lead)
                    .clamp(start, line.len());
                let (prefix, rest) = line.split_at(start);
                let (matched, suffix) = rest.split_at(end - start);
                let selected = flat_index == self.selected;
                rows.push(
                    div()
                        .id(("search-hit", flat_index))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(ui::row_height(cx, 20.))
                        .pl(px(28.))
                        .pr(px(10.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(selected, |element| element.bg(accent.alpha(0.16)))
                        .hover(|style| style.bg(theme.bg3))
                        .child(
                            div()
                                .flex_none()
                                .w(px(34.))
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from((found.line + 1).to_string())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.))
                                .text_color(theme.fg1)
                                .child(SharedString::from(prefix.to_string()))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(accent)
                                        .child(SharedString::from(matched.to_string())),
                                )
                                .child(SharedString::from(suffix.to_string())),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                let Some(file) = this.results.get(file_index) else {
                                    return;
                                };
                                let Some(found) = file.matches.get(match_index) else {
                                    return;
                                };
                                cx.emit(SearchPanelEvent::OpenMatch {
                                    path: file.path.clone(),
                                    line: found.line,
                                    column: found.column,
                                });
                                window.focus(&this.return_focus, cx);
                            }),
                        )
                        .into_any_element(),
                );
                flat_index += 1;
            }
        }
        if truncated {
            rows.push(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(
                        i18n::t!("searchpanel.truncated", "n" => MAX_ROWS),
                    ))
                    .into_any_element(),
            );
        }

        let focus = self.focus.clone();
        div()
            .absolute()
            .inset_0()
            .track_focus(&focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.dismiss(window, cx)),
            )
            .flex()
            .flex_col()
            .items_center()
            .pt(px(96.))
            .child(
                div()
                    .w(px(720.))
                    .flex()
                    .flex_col()
                    .bg(theme.bg2)
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .shadow(vec![gpui::BoxShadow::new(
                        px(0.),
                        px(10.),
                        gpui::hsla(0., 0., 0., 0.45),
                    )
                    .blur_radius(px(28.))])
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(div().flex_none().text_color(theme.fg2).child("⌕"))
                            // 範囲のチップ（フォルダ内を検索・D18）。× か空の入力での ⌫ で外す。
                            .when_some(
                                scope_label(&self.root, self.scope.as_deref()),
                                |row, label| {
                                    row.child(
                                        div()
                                            .id("search-scope")
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .gap(px(2.))
                                            .pl(px(7.))
                                            .pr(px(2.))
                                            .rounded(px(5.))
                                            .bg(theme.bg3)
                                            .text_size(px(11.))
                                            .text_color(theme.fg1)
                                            .child(SharedString::from(i18n::t!(
                                                "searchpanel.scope",
                                                "path" => label
                                            )))
                                            .child(
                                                div()
                                                    .id("search-scope-clear")
                                                    .px(px(4.))
                                                    .rounded(px(4.))
                                                    .text_color(theme.fg2)
                                                    .cursor_pointer()
                                                    .hover(|style| style.text_color(theme.fg0))
                                                    .child("×")
                                                    .tooltip(Tooltip::text(
                                                        i18n::t!("searchpanel.scope_clear"),
                                                        theme.clone(),
                                                    ))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _window, cx| {
                                                            cx.stop_propagation();
                                                            this.set_scope(None, cx);
                                                        }),
                                                    ),
                                            ),
                                    )
                                },
                            )
                            .child(div().flex_1().text_color(query_color).child(query_display))
                            .child(
                                toggle(
                                    "search-case",
                                    "Aa",
                                    self.case_sensitive,
                                    i18n::t!("searchpanel.case_tip"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_case(cx);
                                    }),
                                ),
                            )
                            .child(
                                toggle(
                                    "search-regex",
                                    ".*",
                                    self.is_regex,
                                    i18n::t!("searchpanel.regex_tip"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_regex(cx);
                                    }),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py(px(4.))
                            .text_size(px(11.))
                            .text_color(summary_color)
                            .child(summary),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_h(px(440.))
                            .overflow_hidden()
                            .pb_1()
                            .children(rows),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .px_3()
                            .py(px(5.))
                            .border_t_1()
                            .border_color(theme.border)
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(SharedString::from(i18n::t!("searchpanel.hint_search")))
                            .child(SharedString::from(i18n::t!("searchpanel.hint_select")))
                            .child(SharedString::from(i18n::t!("searchpanel.hint_close"))),
                    ),
            )
    }
}

/// 範囲のチップに出す文字列（root からの相対・末尾に `/`・区切りは `/` に揃える）。
fn scope_label(root: &Path, scope: Option<&Path>) -> Option<String> {
    let scope = scope?;
    let relative = scope.strip_prefix(root).unwrap_or(scope);
    let mut label = relative.to_string_lossy().replace('\\', "/");
    label.push('/');
    Some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_label_is_relative_to_the_project() {
        let root = Path::new("/p");
        assert_eq!(
            scope_label(root, Some(Path::new("/p/src/ui"))),
            Some("src/ui/".to_string())
        );
        assert_eq!(scope_label(root, None), None);
    }

    #[test]
    fn total_and_flat_indices_follow_file_order() {
        let results = [2usize, 0, 3];
        let flat: Vec<_> = results
            .iter()
            .enumerate()
            .flat_map(|(file, count)| (0..*count).map(move |item| (file, item)))
            .collect();
        assert_eq!(flat, vec![(0, 0), (0, 1), (2, 0), (2, 1), (2, 2)]);
    }
}
