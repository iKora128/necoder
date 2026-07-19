impl Workspace {
    fn apply_project_color(
        &mut self,
        project_index: usize,
        color: Hsla,
        hex: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_color_picker(window, cx);
        let Some(slot) = self.project_sessions.projects.get_mut(project_index) else {
            return;
        };
        slot.color = color;
        let remote_key = slot
            .worktree
            .host()
            .is_remote()
            .then(|| slot.worktree.host().display_name().to_string());
        match &remote_key {
            // ローカルは従来どおり `.shirushi/settings.json` へ（リポジトリ共有・チーム寄り）。
            None => {
                let settings_path = slot.worktree.root().join(".shirushi/settings.json");
                if let Err(error) = settings_core::persist_user_value(
                    &settings_path,
                    "color",
                    serde_json::Value::String(hex.to_string()),
                ) {
                    eprintln!(".shirushi への色の保存に失敗: {error:#}");
                }
            }
            // リモートは `.shirushi` がリモート側にあり使えないので、手動色をローカル DB に焼く（M13 #3b・再接続で復元）。
            Some(key) => {
                if let (Some(storage), Ok(value)) = (
                    self.persistence.storage.clone(),
                    u32::from_str_radix(hex.trim_start_matches('#'), 16),
                ) {
                    let _ = storage.set_host_color(key, value);
                }
            }
        }
        // アクティブなら全ペイン（タブ + 分割）のキャレット等アクセントへ波及。
        // レール/タブは render 時に slot.color を読むので notify で追従する（明示波及が要るのはキャレットだけ）。
        if project_index == self.project_sessions.active {
            let editors: Vec<Entity<EditorView>> = self
                .tabs
                .iter()
                .map(|tab| tab.editor.clone())
                .chain(self.split_editor.clone())
                .collect();
            for editor in editors {
                editor.update(cx, |view, cx| view.set_accent(color, cx));
            }
        }
        cx.notify();
    }

    /// 色ピッカーを開く（右クリック位置 or キーボード起動時のアンカー位置）。hex 入力へフォーカス。
    fn open_color_picker(
        &mut self,
        project_index: usize,
        position: Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.overlays.color_picker = Some(ColorPickerState { project_index, position, hex: String::new(), focus });
        cx.notify();
    }

    /// 色ピッカーを閉じ、フォーカスをアクティブエディタへ戻す（rename と同型）。
    fn close_color_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlays.color_picker.take().is_some() {
            if let Some(editor) = self.active_editor() {
                let handle = editor.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    /// ⌘K⌘C / コマンドパレットからアクティブプロジェクトの色ピッカーを開く。
    /// マウス位置が無いので、アクティブなレール項目の位置（pt_2 8px + index*38）にアンカーする。
    fn open_project_color(&mut self, _: &ProjectColor, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = gpui::point(px(RAIL_WIDTH), px(8. + self.project_sessions.active as f32 * 38. + 4.));
        self.open_color_picker(self.project_sessions.active, anchor, window, cx);
    }

    /// 色ピッカーの hex 入力キー処理（rename と同型 + 16 進フィルタ）。
    fn on_color_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_color_picker(window, cx),
            "enter" => {
                let Some(state) = self.overlays.color_picker.as_ref() else {
                    return;
                };
                let project_index = state.project_index;
                let candidate = format!("#{}", state.hex);
                // 6 桁揃ったときだけ適用（parse_hex_color が len==6 を要求）。未完・不正は無視。
                if let Some(color) = parse_hex_color(&candidate) {
                    self.apply_project_color(project_index, color, &candidate, window, cx);
                }
            }
            "backspace" => {
                if let Some(state) = self.overlays.color_picker.as_mut() {
                    state.hex.pop();
                    cx.notify();
                }
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                let Some(text) = &event.keystroke.key_char else {
                    return;
                };
                if let Some(state) = self.overlays.color_picker.as_mut() {
                    // 16 進 6 桁まで（`#` はラベルで出すので取り込まない）。
                    for ch in text.chars().filter(|ch| ch.is_ascii_hexdigit()) {
                        if state.hex.len() >= 6 {
                            break;
                        }
                        state.hex.push(ch);
                    }
                    cx.notify();
                }
            }
        }
    }

    /// 色ピッカー（識別用の厳選スウォッチ + 任意 hex 入力・M12-11 / Peacock 拡張）。
    fn render_color_picker(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let state = self.overlays.color_picker.as_ref()?;
        let project_index = state.project_index;
        let position = state.position;
        let theme = self.theme.clone();
        // 対象プロジェクトの現在色を hex 入力のキャレット色に使う（文脈のヒント）。
        let anchor_color = self
            .project_sessions
            .projects
            .get(project_index)
            .map(|slot| slot.color)
            .unwrap_or_else(|| self.accent());
        let hex_display: SharedString = SharedString::from(state.hex.clone());
        let hex_empty = state.hex.is_empty();
        Some(
            div()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_color_picker(window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .left(position.x + px(8.))
                        .top(position.y)
                        .w(px(168.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(8.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(8.))
                        .shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
                        ])
                        .track_focus(&state.focus)
                        .on_key_down(cx.listener(Self::on_color_key_down))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div().flex().flex_wrap().gap(px(6.)).children(
                                theme_core::IDENTITY_PALETTE_HEXES.iter().enumerate().map(
                                    |(swatch_index, &value)| {
                                        let hex = format!("#{value:06x}");
                                        let color = parse_hex_color(&hex)
                                            .unwrap_or_else(|| project_color(swatch_index));
                                        div()
                                            .id(("color-swatch", swatch_index))
                                            .size(px(22.))
                                            .rounded(px(6.))
                                            .bg(color)
                                            .cursor_pointer()
                                            .hover(|style| style.border_2().border_color(gpui::white()))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.apply_project_color(project_index, color, &hex, window, cx)
                                                }),
                                            )
                                    },
                                ),
                            ),
                        )
                        .child(
                            // 任意 hex 入力行（許可リスト外の色も許すエスケープハッチ・UI-SPEC §1.2）。
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .h(px(24.))
                                .px(px(6.))
                                .bg(theme.bg1)
                                .border_1()
                                .border_color(theme.border)
                                .rounded(px(6.))
                                .text_size(px(12.))
                                .text_color(theme.fg0)
                                .child(div().flex_none().text_color(theme.fg2).child(SharedString::from("#")))
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .when(hex_empty, |element| element.text_color(theme.fg2))
                                        .child(if hex_empty {
                                            SharedString::from(i18n::t!("color.hex_placeholder"))
                                        } else {
                                            hex_display
                                        }),
                                )
                                .child(div().flex_none().w(px(1.5)).h(px(12.)).bg(anchor_color)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// レール項目の右クリックメニュー（M10-2）。色スウォッチ + 新規窓 / レールから外す /
    /// （worktree タブなら）worktree・ブランチ削除。破壊的操作は二段確認。
    fn render_rail_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.overlays.rail_menu.as_ref()?;
        let index = menu.project_index;
        let position = menu.position;
        let confirm = menu.confirm;
        let theme = self.theme.clone();
        let slot = self.project_sessions.projects.get(index)?;
        let is_worktree = slot.worktree_branch.is_some();
        let slot_root = slot.worktree.root().to_path_buf();
        let (bg2, bg3, border, fg0, fg1, fg2, err) =
            (theme.bg2, theme.bg3, theme.border, theme.fg0, theme.fg1, theme.fg2, theme.err);

        // 通常のメニュー行（アイコン + ラベル）。danger=true は削除系（hover で赤・armed で確認文言）。
        let make_row = move |row_id: &'static str,
                             icon: &'static str,
                             label: SharedString,
                             danger: bool,
                             armed: bool| {
            let base = if armed { err } else { fg1 };
            div()
                .id(row_id)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(12.))
                .text_color(base)
                .cursor_pointer()
                .hover(move |style| {
                    if danger {
                        style.bg(bg3).text_color(err)
                    } else {
                        style.bg(bg3).text_color(fg0)
                    }
                })
                .child(div().w(px(14.)).flex_none().text_color(if armed { err } else { fg2 }).child(icon))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(label))
        };

        let mut menu_box = div()
            .absolute()
            .left(position.x + px(8.))
            .top(position.y)
            .w(px(228.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(4.))
            .bg(bg2)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .shadow(vec![
                gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.4)).blur_radius(px(16.)),
            ])
            // メニュー内クリックは背後の backdrop へ伝えない（閉じ・誤爆防止）。
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // 色スウォッチ列（速い色変更。許可リスト外の hex は「その他の色…」へ）。
            .child(
                div().flex().flex_wrap().gap(px(5.)).p(px(4.)).children(
                    theme_core::IDENTITY_PALETTE_HEXES.iter().enumerate().map(|(swatch_index, &value)| {
                        let hex = format!("#{value:06x}");
                        let color = parse_hex_color(&hex).unwrap_or_else(|| project_color(swatch_index));
                        div()
                            .id(("rail-swatch", swatch_index))
                            .size(px(20.))
                            .rounded(px(6.))
                            .bg(color)
                            .cursor_pointer()
                            .hover(|style| style.border_2().border_color(gpui::white()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.apply_project_color(index, color, &hex, window, cx);
                                    this.close_rail_menu(cx);
                                }),
                            )
                    }),
                ),
            )
            // 「その他の色…」= hex 入力つきフル色ピッカーへ（エスケープハッチ）。
            .child(
                make_row("rail-more-colors", "🎨", SharedString::from(i18n::t!("rail.menu_more_colors")), false, false)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.close_rail_menu(cx);
                            this.open_color_picker(index, position, window, cx);
                        }),
                    ),
            )
            .child(div().h(px(1.)).bg(border).my(px(2.)))
            // 新しいウィンドウで開く（旧・既定挙動を明示操作に格下げ）。
            .child({
                let path = slot_root.clone();
                make_row("rail-new-window", "⧉", SharedString::from(i18n::t!("rail.menu_new_window")), false, false)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.close_rail_menu(cx);
                            this.open_folder_as_window(path.clone(), cx);
                        }),
                    )
            })
            .child(div().h(px(1.)).bg(border).my(px(2.)))
            // レールから外す（安全＝ディスク無傷。表示だけ消す）。
            .child(
                make_row("rail-remove", "✕", SharedString::from(i18n::t!("rail.menu_remove")), false, false)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| this.remove_project_slot(index, window, cx)),
                    ),
            );

        // worktree タブだけ: worktree 削除 / worktree ごとブランチ削除（二段確認）。
        if is_worktree {
            let wt_armed = confirm == Some(RailMenuAction::RemoveWorktree);
            let wt_label = if wt_armed {
                i18n::t!("rail.menu_confirm_delete")
            } else {
                i18n::t!("rail.menu_remove_worktree")
            };
            menu_box = menu_box.child(
                make_row("rail-remove-worktree", "🗂", SharedString::from(wt_label), true, wt_armed)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if wt_armed {
                                this.remove_slot_worktree(index, window, cx);
                            } else {
                                this.arm_rail_confirm(RailMenuAction::RemoveWorktree, cx);
                            }
                        }),
                    ),
            );
            let br_armed = confirm == Some(RailMenuAction::DeleteBranch);
            let br_label = if br_armed {
                i18n::t!("rail.menu_confirm_delete")
            } else {
                i18n::t!("rail.menu_delete_branch")
            };
            menu_box = menu_box.child(
                make_row("rail-delete-branch", "🗑", SharedString::from(br_label), true, br_armed)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            if br_armed {
                                this.delete_slot_branch(index, window, cx);
                            } else {
                                this.arm_rail_confirm(RailMenuAction::DeleteBranch, cx);
                            }
                        }),
                    ),
            );
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| this.close_rail_menu(cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| this.close_rail_menu(cx)))
                .child(menu_box)
                .into_any_element(),
        )
    }

    // レールメニューの破壊的操作を二段確認の「1 段目」にする（もう一度クリックで実行）。
    fn arm_rail_confirm(&mut self, action: RailMenuAction, cx: &mut Context<Self>) {
        if let Some(menu) = self.overlays.rail_menu.as_mut() {
            menu.confirm = Some(action);
            cx.notify();
        }
    }

    // ── Agent パネル連携（トースト・色リンク・生中継・M12-3/4/5） ──
}
