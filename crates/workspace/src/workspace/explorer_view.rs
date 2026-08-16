use crate::workspace::*;

impl Workspace {
    pub(crate) fn render_explorer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let Some(slot) = self.active_slot() else {
            return div()
                .w(px(DOCK_WIDTH))
                .h_full()
                .flex_none()
                .bg(theme.bg0)
                .border_r_1()
                .border_color(theme.border);
        };
        // 表示モードで本体を切り替える。カラム表示は複数カラム分だけ横幅を広げる（Finder 風）。
        let body = match self.explorer_mode(cx) {
            ExplorerView::Tree => self.render_tree(slot, cx),
            ExplorerView::Columns => self.render_columns(slot, cx),
            ExplorerView::Icons => self.render_icons(slot, cx),
        };
        div()
            .w(px(self.chrome.explorer_width))
            .h_full()
            .flex_none()
            .relative() // リサイズハンドルの絶対配置基準
            .flex()
            .flex_col()
            .bg(theme.bg0)
            .border_r_1()
            .border_color(theme.border)
            // エクスプローラを触った → ⌘W の宛先はエディタタブ（Agent 判定を下げる）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, _cx| this.agent_active = false),
            )
            .child(self.render_explorer_header(slot, cx))
            .child(body)
            .child(self.render_explorer_footer(cx))
            .child(self.left_dock_resize_handle(cx))
    }

    /// 左ドックの右縁リサイズハンドル（explorer / todo / git 共通・幅は `explorer_width` を共有）。
    /// 置く先のコンテナは `.relative()`（絶対配置の基準）であること。3 ビューは排他表示なので
    /// 同 id が同時に 2 つ出ることはない。
    pub(crate) fn left_dock_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("left-dock-resize")
            .absolute()
            .top_0()
            .right(px(0.))
            .w(px(RESIZE_HANDLE_WIDTH))
            .h_full()
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|style| style.bg(theme.border))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.chrome.resizing_explorer = true;
                    this.chrome.resize_start_x = f32::from(event.position.x);
                    this.chrome.resize_start_width = this.chrome.explorer_width;
                    cx.notify();
                }),
            )
    }

    /// ツリー表示（縦。従来）。行 = chevron + アイコン + 名前。
    /// インライン命名の入力行（ツリー内に splice する・M10 ファイル操作）。
    pub(crate) fn render_naming_row(
        &self,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(naming) = self.explorer_naming(cx) else {
            return div().into_any_element();
        };
        let theme = self.theme.clone();
        let accent = self.accent();
        let icon = match naming.kind {
            NamingKind::NewDir => "▸",
            _ => " ",
        };
        let display: SharedString = SharedString::from(naming.value.clone());
        let focus = naming.focus.clone();
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(ROW_HEIGHT))
            .pl(px(8. + depth as f32 * INDENT))
            .pr(px(8.))
            .track_focus(&focus)
            .on_key_down(cx.listener(Self::on_naming_key_down))
            .child(
                div()
                    .flex_none()
                    .w(px(12.))
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(icon),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .h(px(19.))
                    .px(px(6.))
                    .rounded(px(4.))
                    .bg(theme.bg1)
                    .border_1()
                    .border_color(accent)
                    .text_size(px(12.))
                    .text_color(theme.fg0)
                    .overflow_hidden()
                    .child(div().whitespace_nowrap().overflow_hidden().child(display))
                    .child(div().flex_none().w(px(1.5)).h(px(13.)).bg(accent)),
            )
            .into_any_element()
    }

    pub(crate) fn render_tree(
        &self,
        slot: &ProjectSlot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let color = slot.color;
        let selected = slot.explorer.selected.clone();
        let git_status = &self.repository.status;
        let root = slot.worktree.root().to_path_buf(); // ドラッグ時の @メンション相対パス用
                                                       // インライン命名（M10）: rename は対象行を入力行に置き換え、New* は親フォルダ行の直後
                                                       // （親がルートなら先頭）に入力行を挿す。
        let naming = self.explorer_naming(cx);
        let naming_kind = naming.as_ref().map(|naming| naming.kind);
        let naming_parent = naming.as_ref().map(|naming| naming.parent.clone());
        let naming_target = naming.as_ref().and_then(|naming| naming.target.clone());
        let naming_at_root = naming_kind.is_some()
            && naming_kind != Some(NamingKind::Rename)
            && naming_parent.as_deref() == Some(root.as_path());
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        if naming_at_root {
            elements.push(self.render_naming_row(0, cx));
        }
        for (index, row) in slot.explorer.rows.iter().enumerate() {
            if naming_kind == Some(NamingKind::Rename) && naming_target.as_ref() == Some(&row.path)
            {
                elements.push(self.render_naming_row(row.depth, cx));
                continue;
            }
            elements.push(self.render_tree_row(
                slot, index, row, &theme, color, &selected, git_status, &root, cx,
            ));
            if naming_kind.is_some()
                && naming_kind != Some(NamingKind::Rename)
                && row.is_dir
                && naming_parent.as_ref() == Some(&row.path)
            {
                elements.push(self.render_naming_row(row.depth + 1, cx));
            }
        }
        div()
            .flex_1()
            .overflow_hidden()
            .children(elements)
            .into_any_element()
    }

    /// ツリーの 1 行（従来 render_tree のクロージャ本体を関数化・M10 ファイル操作で入力行と共存させるため）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_tree_row(
        &self,
        _slot: &ProjectSlot,
        index: usize,
        row: &TreeRow,
        theme: &Theme,
        color: Hsla,
        selected: &Option<PathBuf>,
        git_status: &HashMap<PathBuf, StatusKind>,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme.clone();
        let selected = selected.clone();
        let root = root.to_path_buf();
        {
            let path = row.path.clone();
            let is_dir = row.is_dir;
            let is_selected = selected.as_ref() == Some(&row.path);
            // git 色分け: ファイルは自身の状態、フォルダは配下に変更があれば ● を出す。
            let file_status = if is_dir {
                None
            } else {
                git_status.get(&row.path).copied()
            };
            let dir_dirty = is_dir
                && git_status
                    .keys()
                    .any(|changed| changed.starts_with(&row.path));
            let name_color = match file_status {
                Some(status) => Self::git_tint(&theme, status),
                None if is_selected => theme.fg0,
                None => theme.fg1,
            };
            let chevron = if row.is_dir {
                if row.is_expanded {
                    "▾"
                } else {
                    "▸"
                }
            } else {
                ""
            };
            div()
                .id(("tree-row", index))
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(ROW_HEIGHT))
                .pr_2()
                .pl(px(6.0 + row.depth as f32 * INDENT))
                .text_size(px(12.5))
                .text_color(if is_selected { theme.fg0 } else { theme.fg1 })
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                .when(is_selected, |element| {
                    element.bg(theme.bg3).border_l_2().border_color(color)
                })
                // gitignore 対象は淡く（git 管理外が一目で分かる）。
                .when(row.ignored, |element| element.opacity(0.45))
                // ファイルはチャット composer へドラッグ → @メンション参照にできる。
                .when(!is_dir, |element| {
                    let mention = row
                        .path
                        .strip_prefix(&root)
                        .unwrap_or(&row.path)
                        .to_string_lossy()
                        .to_string();
                    let theme = theme.clone();
                    element.on_drag(
                        DraggedFile {
                            path: mention.into(),
                            theme,
                        },
                        |dragged, _offset, _window, cx| cx.new(|_| dragged.clone()),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(9.))
                        .text_size(px(9.))
                        .text_color(theme.fg2)
                        .child(SharedString::from(chevron.to_string())),
                )
                .child(file_icon(&row.name, is_dir, row.is_expanded, &theme))
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(name_color)
                        .child(row.name.clone()),
                )
                // git バッジ（ファイル=状態文字・フォルダ=変更あり ●）
                .when_some(file_status, |element, status| {
                    element.child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(Self::git_tint(&theme, status))
                            .child(Self::git_letter(status)),
                    )
                })
                .when(dir_dirty, |element| {
                    element.child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .text_color(theme.warn)
                            .child("●"),
                    )
                })
                // エージェントが触ったファイル = スレッド色ドット（色リンク・M12-4）。
                .when_some(
                    self.agent_touched.get(&row.path).copied(),
                    |element, color| {
                        element.child(div().flex_none().size(px(7.)).rounded_full().bg(color))
                    },
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if is_dir {
                            this.toggle_dir(path.clone(), cx);
                        } else {
                            this.open_file(path.clone(), window, cx);
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let path = row.path.clone();
                        move |this, event: &MouseDownEvent, _window, cx| {
                            this.show_context_menu(path.clone(), is_dir, event.position, cx)
                        }
                    }),
                )
                .into_any_element()
        }
    }

    /// アイコングリッド表示（現在フォルダの直下。フォルダはクリックで中に入る・ファイルは開く）。
    pub(crate) fn render_icons(
        &self,
        slot: &ProjectSlot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let dir = slot
            .explorer
            .current_dir
            .clone()
            .unwrap_or_else(|| slot.worktree.root().to_path_buf());
        // controller refresh が事前構築した cache だけを読む。cache miss は空表示で、I/O はしない。
        let entries = slot.listed_dir(&dir);
        let selected = slot.explorer.selected.clone();
        div()
            .flex_1()
            .overflow_hidden()
            .flex()
            .flex_wrap()
            .content_start()
            .gap(px(2.))
            .p(px(6.))
            .children(entries.into_iter().enumerate().map(|(index, entry)| {
                let is_dir = entry.is_dir;
                let is_ignored = entry.ignored;
                let path = entry.path.clone();
                let is_selected = selected.as_ref() == Some(&entry.path);
                div()
                    .id(("icon-cell", index))
                    .w(px(84.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .py(px(8.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .when(is_selected, |element| element.bg(theme.bg3))
                    .when(is_ignored, |element| element.opacity(0.45))
                    // 大きめアイコン（グリッド用に 2 倍スケール）
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(30.))
                            .child(icon_large(&entry.name, is_dir, &theme)),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .text_size(px(11.))
                            .text_color(theme.fg1)
                            .text_center()
                            .overflow_hidden()
                            .child(SharedString::from(entry.name.clone())),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if is_dir {
                                this.enter_dir(path.clone(), cx);
                            } else {
                                this.open_file(path.clone(), window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let path = entry.path.clone();
                            move |this, event: &MouseDownEvent, _window, cx| {
                                this.show_context_menu(path.clone(), is_dir, event.position, cx)
                            }
                        }),
                    )
            }))
            .into_any_element()
    }

    /// Finder のカラム表示（Miller columns）。ルート→現在フォルダの各段をカラムで並べる。
    pub(crate) fn render_columns(
        &self,
        slot: &ProjectSlot,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let root = slot.worktree.root().to_path_buf();
        let current = slot
            .explorer
            .current_dir
            .clone()
            .unwrap_or_else(|| root.clone());
        // ルート → current の連鎖（各段がカラムになる）。
        let mut chain: Vec<PathBuf> = Vec::new();
        let mut walk = current.as_path();
        loop {
            chain.push(walk.to_path_buf());
            if walk == root {
                break;
            }
            match walk.parent() {
                Some(parent) if parent.starts_with(&root) || parent == root => walk = parent,
                _ => break,
            }
        }
        chain.reverse();
        // 460px に収まるよう末尾 3 段（＝現在フォルダ + 親 2 つ）だけ見せる。
        let visible_start = chain.len().saturating_sub(3);

        div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .children(
                chain
                    .iter()
                    .enumerate()
                    .skip(visible_start)
                    .map(|(column_index, dir)| {
                        // controller refresh が事前構築した cache だけを読む。cache miss は空表示で、I/O はしない。
                        let entries = slot.listed_dir(dir);
                        // このカラムで選択中（＝連鎖の次の段）のパス。
                        let selected_child = chain.get(column_index + 1).cloned();
                        div()
                            .w(px(150.))
                            .flex_none()
                            .h_full()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(theme.border)
                            .children(entries.into_iter().enumerate().map(|(row_index, entry)| {
                                let is_dir = entry.is_dir;
                                let is_ignored = entry.ignored;
                                let path = entry.path.clone();
                                let on_path = selected_child.as_ref() == Some(&entry.path);
                                div()
                                    .id(("col", column_index * 1000 + row_index))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .h(px(ROW_HEIGHT))
                                    .px(px(7.))
                                    .text_size(px(12.))
                                    .text_color(if on_path { theme.fg0 } else { theme.fg1 })
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                                    .when(on_path, |element| element.bg(theme.bg3))
                                    .when(is_ignored, |element| element.opacity(0.45))
                                    .child(file_icon(
                                        &entry.name,
                                        is_dir,
                                        is_dir && on_path,
                                        &theme,
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .child(SharedString::from(entry.name.clone())),
                                    )
                                    // フォルダは中に入る合図の ›
                                    .when(is_dir, |element| {
                                        element.child(
                                            div()
                                                .flex_none()
                                                .text_size(px(9.))
                                                .text_color(theme.fg2)
                                                .child("›"),
                                        )
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            if is_dir {
                                                this.enter_dir(path.clone(), cx);
                                            } else {
                                                this.open_file(path.clone(), window, cx);
                                            }
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener({
                                            let path = entry.path.clone();
                                            move |this, event: &MouseDownEvent, _window, cx| {
                                                this.show_context_menu(
                                                    path.clone(),
                                                    is_dir,
                                                    event.position,
                                                    cx,
                                                )
                                            }
                                        }),
                                    )
                            }))
                    }),
            )
            .into_any_element()
    }

    /// エクスプローラ上部のブレッドクラム。左の **⤴** で 1 段上へ（プロジェクトルートより上＝
    /// 隣のリポジトリへも辿れる。M5 受入）。プロジェクト配下では「プロジェクト名 → 各段」を、
    /// ルート外では「⌂プロジェクト（戻る） → 末尾数段」を出す。各段クリックでそのフォルダへ。
    pub(crate) fn render_explorer_header(
        &self,
        slot: &ProjectSlot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = slot.color;
        let root = slot.worktree.root().to_path_buf();
        let current = slot
            .explorer
            .current_dir
            .clone()
            .unwrap_or_else(|| root.clone());
        let in_project = current.starts_with(&root);

        let mut header = div()
            .flex()
            .items_center()
            .gap(px(1.))
            .h(px(28.))
            .px(px(6.))
            .flex_none()
            .text_size(px(11.))
            .text_color(theme.fg2)
            .overflow_hidden()
            .border_b_1()
            .border_color(theme.border);

        // 「上へ」= current の親へ（ルート直上へ出れば enter_dir が Finder カラムへ自動切替）。
        if let Some(parent) = current.parent().map(Path::to_path_buf) {
            header = header.child(
                div()
                    .id("crumb-up")
                    .flex_none()
                    .px(px(3.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child("⤴")
                    .tooltip(Tooltip::text(i18n::t!("explorer.up_folder"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.enter_dir(parent.clone(), cx)),
                    ),
            );
        }

        if in_project {
            // プロジェクト名（アクセント色・クリックでルートへ）→ 配下の各段。
            let root_for_click = root.clone();
            header = header.child(
                div()
                    .id(("crumb", 0usize))
                    .px(px(3.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|style| style.bg(theme.bg3))
                    .child(slot.name.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.enter_dir(root_for_click.clone(), cx)
                        }),
                    ),
            );
            if let Ok(relative) = current.strip_prefix(&root) {
                let segments: Vec<_> = relative.components().collect();
                let last = segments.len().saturating_sub(1);
                let mut accumulated = root.clone();
                for (index, segment) in segments.into_iter().enumerate() {
                    accumulated = accumulated.join(segment.as_os_str());
                    let is_current = index == last;
                    let label = segment.as_os_str().to_string_lossy().to_string();
                    let path = accumulated.clone();
                    header = header
                        .child(div().px(px(1.)).text_color(theme.fg2).child("›"))
                        .child(
                            div()
                                .id(("crumb", index + 1))
                                .px(px(3.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(is_current, |element| {
                                    element
                                        .text_color(theme.fg0)
                                        .font_weight(FontWeight::SEMIBOLD)
                                })
                                .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                                .child(SharedString::from(label))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.enter_dir(path.clone(), cx)
                                    }),
                                ),
                        );
                }
            }
        } else {
            // ルート外ブラウズ: ⌂プロジェクト（戻る）+ current までの末尾最大 3 段。
            let root_for_click = root.clone();
            header = header.child(
                div()
                    .id("crumb-home")
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .px(px(4.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|style| style.bg(theme.bg3))
                    .child("⌂")
                    .child(slot.name.clone())
                    .tooltip(Tooltip::text(
                        i18n::t!("explorer.back_to_project"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.enter_dir(root_for_click.clone(), cx)
                        }),
                    ),
            );
            let mut chain: Vec<PathBuf> = Vec::new();
            let mut walk = Some(current.as_path());
            while let Some(path) = walk {
                chain.push(path.to_path_buf());
                walk = path.parent();
                if chain.len() >= 3 {
                    break;
                }
            }
            chain.reverse();
            let last = chain.len().saturating_sub(1);
            for (index, path) in chain.into_iter().enumerate() {
                let is_current = index == last;
                let label = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string());
                header = header
                    .child(div().px(px(1.)).text_color(theme.fg2).child("›"))
                    .child(
                        div()
                            .id(("crumb-out", index))
                            .px(px(3.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .when(is_current, |element| {
                                element
                                    .text_color(theme.fg0)
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                            .child(SharedString::from(label))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.enter_dir(path.clone(), cx)
                                }),
                            ),
                    );
            }
        }
        header
    }

    /// エクスプローラ下部の表示モード切替（ツリー / カラム / アイコン）。
    pub(crate) fn render_explorer_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let current = self.explorer_mode(cx);
        // アイコンは Lucide SVG。svg は親の text_color を継承しない（自分の style.text.color のみ参照）
        // ため、色は svg へ直接指定。ホバー時の明るさ変化は group_hover で id 単位に効かせる。
        let button = |view: ExplorerView, id: &'static str, icon: &'static str, tip: String| {
            let active = current == view;
            let icon_color = if active { theme.fg0 } else { theme.fg2 };
            div()
                .id(id)
                .group(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded(px(5.))
                .cursor_pointer()
                .when(active, |element| element.bg(theme.bg3))
                .hover(|style| style.bg(theme.bg3))
                .child(
                    svg()
                        .path(icon)
                        .size(px(15.))
                        .text_color(icon_color)
                        .group_hover(id, |style| style.text_color(theme.fg0)),
                )
                .tooltip(Tooltip::text(tip, theme.clone()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| this.set_explorer_view(view, cx)),
                )
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(30.))
            .px(px(6.))
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .child(button(
                ExplorerView::Tree,
                "view-tree",
                "icons/list.svg",
                i18n::t!("explorer.view_tree"),
            ))
            .child(button(
                ExplorerView::Columns,
                "view-columns",
                "icons/columns-3.svg",
                i18n::t!("explorer.view_columns"),
            ))
            .child(button(
                ExplorerView::Icons,
                "view-icons",
                "icons/layout-grid.svg",
                i18n::t!("explorer.view_icons"),
            ))
    }

    /// エクスプローラの右クリックメニュー（開いていれば）。フォルダ=新規ウィンドウで開く 等。
    /// 背後に透明バックドロップを敷き、外側クリックで閉じる。
    pub(crate) fn render_explorer_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.explorer_context_menu(cx)?;
        let (bg2, bg3, border, fg1, fg0) = (
            self.theme.bg2,
            self.theme.bg3,
            self.theme.border,
            self.theme.fg1,
            self.theme.fg0,
        );
        let path = menu.path.clone();
        let is_dir = menu.is_dir;
        let position = menu.position;

        let item = move |id: &'static str, label: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(fg1)
                .cursor_pointer()
                .hover(move |style| style.bg(bg3).text_color(fg0))
                .child(label)
        };

        let mut menu_box = div()
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(210.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))]);

        let is_local = self
            .active_slot()
            .map(|slot| slot.remote_host.is_none())
            .unwrap_or(false);
        if is_dir {
            // 「ここをプロジェクトとして開く」= 現在のレールに再ルート（remote は同じ接続を再利用）。
            // browse（home 接続）で辿ったフォルダを「開く」の主導線。新窓は下の項目で明示。
            let here_path = path.clone();
            menu_box = menu_box.child(
                item("ctx-open-here", i18n::t!("explorer.ctx_open_here")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_dir_in_rail(here_path.clone(), cx)
                    }),
                ),
            );
            let open_path = path.clone();
            menu_box = menu_box.child(
                item("ctx-open-window", i18n::t!("explorer.ctx_open_window")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_folder_as_window(open_path.clone(), cx)
                    }),
                ),
            );
        } else {
            let open_path = path.clone();
            menu_box = menu_box.child(
                item("ctx-open", i18n::t!("explorer.ctx_open")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.open_file(open_path.clone(), window, cx);
                        this.hide_context_menu(cx);
                    }),
                ),
            );
        }
        // ── 既定アプリで開く / Finder で表示（ローカルのみ・シングルクリックで代替できない操作） ──
        if is_local {
            let open_path = path.clone();
            menu_box = menu_box.child(
                item("ctx-open-default", i18n::t!("explorer.ctx_open_default")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_with_default_app(&open_path, cx)
                    }),
                ),
            );
            let reveal_path = path.clone();
            menu_box = menu_box.child(
                item("ctx-reveal", i18n::t!("explorer.ctx_reveal")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.reveal_in_finder(&reveal_path, cx)
                    }),
                ),
            );
        }
        // ── ファイル操作（M10・local のみ） ──
        if is_local {
            let base = path.clone();
            menu_box = menu_box.child(
                item("ctx-new-file", i18n::t!("explorer.ctx_new_file")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.start_naming(NamingKind::NewFile, base.clone(), is_dir, window, cx)
                    }),
                ),
            );
            let base = path.clone();
            menu_box = menu_box.child(
                item("ctx-new-dir", i18n::t!("explorer.ctx_new_dir")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.start_naming(NamingKind::NewDir, base.clone(), is_dir, window, cx)
                    }),
                ),
            );
            let base = path.clone();
            menu_box = menu_box.child(
                item("ctx-rename", i18n::t!("explorer.ctx_rename")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.start_naming(NamingKind::Rename, base.clone(), is_dir, window, cx)
                    }),
                ),
            );
            let base = path.clone();
            menu_box = menu_box.child(
                item("ctx-duplicate", i18n::t!("explorer.ctx_duplicate")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| this.duplicate_entry(base.clone(), cx)),
                ),
            );
            let base = path.clone();
            menu_box = menu_box.child(
                item("ctx-trash", i18n::t!("explorer.ctx_trash")).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.trash_entry(base.clone(), window, cx)
                    }),
                ),
            );
        }
        let copy_path = path.clone();
        menu_box = menu_box.child(
            item("ctx-copy", i18n::t!("explorer.ctx_copy_path")).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.copy_path(&copy_path, cx)),
            ),
        );

        // 透明バックドロップ（外側クリックで閉じる）。メニューはその子（最前面）。
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.hide_context_menu(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.hide_context_menu(cx)),
                )
                .child(menu_box)
                .into_any_element(),
        )
    }

    // プロジェクト横断検索パネル（オーバーレイ・⌘⇧F）。ファイル別に結果をまとめ、クリック/Enter でジャンプ。
    pub(crate) fn render_search_panel(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.search_panel
            .as_ref()
            .map(|panel| panel.clone().into_any_element())
    }

    // hot exit の復元/破棄バー（起動時に前回の未保存スナップショットが見つかったとき・M10）。
}
