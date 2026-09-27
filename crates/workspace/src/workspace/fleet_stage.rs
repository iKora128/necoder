//! Fleet の舞台。配置とタブの選択だけを持ち、会話・PTY の所有権は Session に残す。
use crate::workspace::*;
use super::fleet_view::pane_space;

impl Workspace {
    pub(super) fn stage_cards(&self) -> Vec<(usize, FleetPane)> {
        let active = self.active_slot().map(|slot| slot.task_space.id.clone());
        let mut spaces = Vec::new();
        if self.chrome.stage_columns > 1 {
            spaces.extend(self.chrome.stage_pinned.iter().cloned());
        }
        if let Some(active) = active {
            if !spaces.contains(&active) { spaces.push(active); }
        }
        let limit = self.chrome.stage_columns.min(((self.chrome.stage_width / 428.) as usize).max(1));
        spaces.into_iter().filter_map(|space| {
            let session = self.session_index_for_space(&space)?;
            let slot = &self.project_sessions.projects[session];
            if Some(slot.repository_key()) != self.active_repository_key() || slot.task_space.phase == TaskPhase::Archived { return None; }
            self.chrome.fleet_cells.iter().enumerate()
                .find(|(_, pane)| pane_space(pane) == &space)
                .map(|(index, _)| (index, FleetPane::Task { space }))
        }).take(limit).collect()
    }

    /// Fleet に居る間、Agent パネルは自前のスレッドタブ行を畳む（ペインバーのスレッドタブと二重にしない）。
    /// **毎 render で呼ぶ**（`Workspace::render`）。Fleet の出入りと panel の増減は入口が複数あり、
    /// 呼び出し側に配ると必ずどれかを取りこぼす。状態が変わらなければ即 return する。
    pub(crate) fn sync_embedded_panels(&mut self, cx: &mut Context<Self>) {
        let state = (
            self.chrome.fleet_mode,
            self.project_sessions.sessions.iter().map(|session| session.fleet_agents.len()).sum(),
        );
        if self.chrome.embedded_synced == Some(state) {
            return;
        }
        self.chrome.embedded_synced = Some(state);
        let panels: Vec<_> = self.project_sessions.sessions.iter().flat_map(|session| session.fleet_agents.iter().cloned()).collect();
        for panel in panels {
            panel.update(cx, |panel, cx| panel.set_embedded(state.0, cx));
        }
    }

    /// 舞台の列数トグル（▯ / ▯▯ / ▯▯▯）。専用の行は持たず、系譜ヘッダの右に同居する（縦の面積を舞台へ返す）。
    pub(super) fn render_stage_columns(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut group = div().flex_none().flex().items_center().gap(px(2.));
        for columns in 1..=3usize {
            let on = self.chrome.stage_columns == columns;
            group = group.child(div().id(("stage-layout", columns)).flex_none().h(px(20.)).px(px(7.)).flex().items_center().rounded(px(5.))
                .text_size(px(10.)).text_color(if on { self.theme.fg0 } else { self.theme.fg2 })
                .when(on, |element| element.bg(self.theme.bg3))
                .hover(|style| style.bg(self.theme.bg2))
                .cursor_pointer().child(SharedString::from("▯".repeat(columns)))
                .tooltip(Tooltip::text(i18n::t!("fleet.stage_columns", "count" => columns), self.theme.clone()))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.chrome.stage_columns = columns;
                    cx.notify();
                })));
        }
        group.into_any_element()
    }

    pub(super) fn toggle_stage_pin(&mut self, space: SpaceId, cx: &mut Context<Self>) {
        if let Some(index) = self.chrome.stage_pinned.iter().position(|current| current == &space) {
            self.chrome.stage_pinned.remove(index);
        } else {
            if self.chrome.stage_pinned.len() == 3 { self.chrome.stage_pinned.remove(0); }
            self.chrome.stage_pinned.push(space);
            self.chrome.stage_columns = self.chrome.stage_pinned.len().max(2);
        }
        cx.notify();
    }

    /// カード 1 枚の幅が「会話 | サイドペイン」の分割に足りるか。足りなければサイドペインは全面に出す。
    pub(super) fn stage_card_is_wide(&self) -> bool {
        self.stage_card_width() >= 900.
    }

    /// 開いているサイドペイン（変更 / ターミナル / ファイル）。`stage_tabs` が会話を指していれば無し。
    pub(super) fn stage_side(&self, space: &SpaceId) -> Option<FleetPane> {
        match self.chrome.stage_tabs.get(space) {
            Some(pane) => (!matches!(pane, FleetPane::Agent { .. } | FleetPane::Task { .. })).then(|| pane.clone()),
            // ブリッジ（統合先のカード）は、まだ何も選んでいなければ編隊図を開いておく — Fleet の家に入ったら編隊が見える。
            None => self.session_index_for_space(space)
                .filter(|index| self.project_sessions.projects[*index].task_space.is_integration())
                .map(|_| FleetPane::Formation { space: space.clone() }),
        }
    }

    /// ブリッジの編隊図を開く / 閉じる（系譜ヘッダの ⚑ 編隊・⌘⇧G）。ブリッジに居なければまずそこへ行く。
    pub(crate) fn toggle_formation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.fleet_repository_key() else { return; };
        let Some(index) = self.project_sessions.projects.iter().position(|slot| slot.repository_key() == key && slot.task_space.is_integration()) else { return; };
        let space = self.project_sessions.projects[index].task_space.id.clone();
        let on_bridge = self.project_sessions.active == index;
        let open = matches!(self.stage_side(&space), Some(FleetPane::Formation { .. }));
        if on_bridge && open {
            self.set_stage_side(index, None, cx);
            return;
        }
        self.seed_fleet_cells(cx);
        self.switch_project(index, window, cx);
        self.set_stage_side(index, Some(FleetPane::Formation { space }), cx);
    }

    /// カードの会話ペインに出す panel。サイドペインを開いても操作先（`agent_panel`）のまま動かさない。
    pub(super) fn conversation_panel(&self, session_index: usize) -> Entity<AgentPanel> {
        let session = &self.project_sessions.sessions[session_index];
        if let Some(FleetPane::Agent { panel, .. }) = self.chrome.stage_tabs.get(&self.project_sessions.projects[session_index].task_space.id) {
            return panel.clone();
        }
        if session.fleet_agents.contains(&session.agent_panel) { session.agent_panel.clone() } else { session.fleet_agents[0].clone() }
    }

    pub(super) fn set_stage_side(&mut self, session_index: usize, side: Option<FleetPane>, cx: &mut Context<Self>) {
        let space = self.project_sessions.projects[session_index].task_space.id.clone();
        let pane = side.unwrap_or_else(|| FleetPane::Agent { space: space.clone(), panel: self.conversation_panel(session_index) });
        self.chrome.stage_tabs.insert(space, pane);
        cx.notify();
    }

    /// `+N −M` の 2 チップ（増 = ok / 減 = err。git の語彙なので色は状態ではなく diff の符号）。差分なし・未取得は空。
    pub(super) fn shortstat_chips(&self, shortstat: Option<(usize, usize)>) -> Vec<gpui::AnyElement> {
        let Some((added, deleted)) = shortstat.filter(|stat| *stat != (0, 0)) else { return Vec::new(); };
        vec![
            div().flex_none().text_size(px(10.)).text_color(self.theme.ok).child(SharedString::from(format!("+{added}"))).into_any_element(),
            div().flex_none().text_size(px(10.)).text_color(self.theme.err).child(SharedString::from(format!("−{deleted}"))).into_any_element(),
        ]
    }

    /// スレッドタブの ×。閉じた結果 panel が空になったら、会話ペインを残っている会話へ移す
    /// （＋で足した panel は一覧からも外す。先頭の panel は Task の初期パネルとして残す）。
    pub(super) fn close_task_thread(&mut self, session_index: usize, panel: &Entity<AgentPanel>, thread: usize, cx: &mut Context<Self>) {
        panel.update(cx, |panel, cx| panel.close_thread(thread, cx));
        if panel.read(cx).statuses().is_empty() {
            let space = self.project_sessions.projects[session_index].task_space.id.clone();
            let session = &mut self.project_sessions.sessions[session_index];
            if session.fleet_agents.first() != Some(panel) {
                session.fleet_agents.retain(|candidate| candidate != panel);
                self.chrome.fleet_cells.retain(|pane| !matches!(pane, FleetPane::Agent { panel: candidate, .. } if candidate == panel));
            }
            let session = &mut self.project_sessions.sessions[session_index];
            if let Some(next) = session.fleet_agents.iter().find(|candidate| !candidate.read(cx).statuses().is_empty()).cloned() {
                session.agent_panel = next.clone();
                if matches!(self.chrome.stage_tabs.get(&space), Some(FleetPane::Agent { .. })) {
                    self.chrome.stage_tabs.insert(space.clone(), FleetPane::Agent { space, panel: next });
                }
            }
        }
        cx.notify();
    }

    fn task_shells(&self, space: &SpaceId) -> Vec<FleetPane> {
        self.chrome.fleet_cells.iter().filter(|pane| matches!(pane, FleetPane::Shell { space: owner, .. } if owner == space)).cloned().collect()
    }

    /// スレッドタブ 1 枚（色付きの ● + 名前・選択は上線 2px）。
    pub(super) fn thread_tab(&self, id: impl Into<gpui::ElementId>, label: SharedString, color: gpui::Hsla, selected: bool) -> gpui::Stateful<gpui::Div> {
        div().id(id).flex_none().flex().items_center().gap(px(6.)).px(px(10.)).h_full().border_t_2()
            .border_color(if selected { color } else { gpui::transparent_black() })
            .when(selected, |element| element.bg(self.theme.bg1))
            .text_size(px(11.)).text_color(if selected { self.theme.fg0 } else { self.theme.fg1 })
            .cursor_pointer()
            .child(div().flex_none().size(px(7.)).rounded_full().bg(color))
            .child(label)
    }

    /// サイドペインのトグル（タブではなくセグメント。押すと会話の横に開き、もう一度で閉じる）。
    pub(super) fn side_toggle(&self, id: impl Into<gpui::ElementId>, label: SharedString, count: Option<usize>, open: bool) -> gpui::Stateful<gpui::Div> {
        div().id(id).flex_none().flex().items_center().gap(px(5.)).h(px(20.)).px(px(9.)).rounded(px(5.))
            .text_size(px(11.)).text_color(if open { self.theme.fg0 } else { self.theme.fg1 })
            .when(open, |element| element.bg(self.theme.bg2).border_1().border_color(self.theme.border))
            .hover(|style| style.bg(self.theme.bg2))
            .cursor_pointer()
            .tooltip(Tooltip::text(i18n::t!("fleet.side_tip"), self.theme.clone()))
            .child(label)
            .when_some(count.filter(|count| *count > 0), |element, count| {
                element.child(div().text_size(px(10.)).text_color(self.theme.fg2).child(SharedString::from(count.to_string())))
            })
    }

    /// サイドペインの見出し（名前 + 補足 + 右端 ×）。`extras` は名前の直後に並べる（ターミナルの通番チップなど）。
    pub(super) fn side_header(&self, id: impl Into<gpui::ElementId>, title: SharedString, extras: Vec<gpui::AnyElement>, detail: Option<SharedString>,
        close: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> gpui::Div {
        div().flex_none().flex().items_center().gap(px(6.)).h(px(28.)).pl(px(12.)).pr(px(6.)).border_b_1().border_color(self.theme.border)
            .child(div().flex_none().text_size(px(11.)).text_color(self.theme.fg0).child(title))
            .children(extras)
            .when_some(detail, |element, detail| element.child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(10.)).text_color(self.theme.fg2).child(detail)))
            .child(div().id(id).flex_none().ml_auto().size(px(20.)).flex().items_center().justify_center().rounded(px(4.))
                .text_size(px(12.)).text_color(self.theme.fg2).hover(|style| style.bg(self.theme.bg2)).cursor_pointer().child("×")
                .tooltip(Tooltip::text(i18n::t!("fleet.side_close"), self.theme.clone()))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| close(this, window, cx))))
    }

    /// ペインバー: 左 = スレッドタブ + ＋（会話を足す）/ 右 = サイドペインのトグル（FLEET-V2 §3.5）。
    /// 「誰と話すか」と「何を横に置くか」は別の軸なので 1 本のタブに混ぜない。
    pub(super) fn render_pane_bar(&self, session_index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let session = &self.project_sessions.sessions[session_index];
        let space = self.project_sessions.projects[session_index].task_space.id.clone();
        let wide = self.stage_card_is_wide();
        let side = self.stage_side(&space);
        let conversation = self.conversation_panel(session_index);
        let mut bar = div().id(("task-tabs", session_index)).flex_none().flex().overflow_x_scroll().items_center().h(px(30.)).px(px(6.))
            .border_b_1().border_color(self.theme.border);
        let mut tab_index = 0usize;
        let thread_total: usize = session.fleet_agents.iter().map(|panel| panel.read(cx).statuses().len()).sum();
        // ブリッジ（統合先）: Captain は「main に住むスレッド」なので、スレッドタブの先頭に ⚑ で座らせる。
        // 未任命なら同じ位置が任命の入口になる（押すと会話ペインに任命の面が出る）。
        let bridge = self.project_sessions.projects[session_index].task_space.is_integration();
        let captain_agent = settings::get(cx).captain_agent.clone();
        let appointing = bridge && self.chrome.captain_appointing && captain_agent.is_none();
        if bridge {
            let main_panel = session.fleet_agents[0].clone();
            let captain_thread = main_panel.read(cx).statuses().iter().position(|status| captain::is_captain_thread_name(&status.name));
            let selected = (wide || side.is_none()) && match (&captain_agent, captain_thread) {
                (Some(_), Some(thread)) => main_panel == conversation && main_panel.read(cx).active_thread() == thread,
                _ => appointing,
            };
            let label: SharedString = if captain_agent.is_some() { i18n::t!("captain.title").into() } else { i18n::t!("captain.appoint_tab").into() };
            bar = bar.child(div().id(("task-tab-captain", session_index)).flex_none().flex().items_center().gap(px(6.)).px(px(10.)).h_full().border_t_2()
                .border_color(if selected { self.accent() } else { gpui::transparent_black() })
                .when(selected, |element| element.bg(self.theme.bg1))
                .text_size(px(11.)).text_color(if selected { self.theme.fg0 } else { self.theme.fg1 })
                .cursor_pointer()
                .child(div().flex_none().text_color(if captain_agent.is_some() { self.accent() } else { self.theme.fg2 }).child("⚑"))
                .child(label)
                .tooltip(Tooltip::text(i18n::t!("captain.tab_tip"), self.theme.clone()))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.focus_captain(&FocusCaptain, window, cx))));
        }
        for panel in &session.fleet_agents {
            for (thread, status) in panel.read(cx).statuses().into_iter().enumerate() {
                // 任命中の Captain スレッドは上の ⚑ タブが表す（二重に並べない）。
                if bridge && captain_agent.is_some() && captain::is_captain_thread_name(&status.name) { continue; }
                // 狭いカードでサイドペインが全面に出ている間は、どのスレッドも「選択中」に見せない（押すと会話へ戻る）。
                let selected = !appointing && panel == &conversation && panel.read(cx).active_thread() == thread && (wide || side.is_none());
                let panel = panel.clone();
                // × は選択中のタブだけ・最後の 1 本には出さない（Task から会話が無くならない）。閉じ方は既存の `close_thread` 1 本。
                let close = (selected && thread_total > 1).then(|| {
                    let panel = panel.clone();
                    div().id(("task-tab-close", session_index * 10000 + tab_index)).flex_none().size(px(16.)).flex().items_center().justify_center().rounded(px(4.))
                        .text_size(px(11.)).text_color(self.theme.fg2).hover(|style| style.bg(self.theme.bg2)).cursor_pointer().child("×")
                        .tooltip(Tooltip::text(i18n::t!("fleet.tab_close_thread"), self.theme.clone()))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_task_thread(session_index, &panel, thread, cx);
                        }))
                });
                bar = bar.child(self.thread_tab(("task-tab", session_index * 10000 + tab_index), status.name, status.color, selected)
                    .children(close)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        this.switch_project(session_index, window, cx);
                        this.chrome.captain_appointing = false;
                        panel.update(cx, |panel, cx| panel.focus_thread(thread, cx));
                        this.project_sessions.sessions[session_index].agent_panel = panel.clone();
                        let space = this.project_sessions.projects[session_index].task_space.id.clone();
                        if !this.stage_card_is_wide() || this.stage_side(&space).is_none() {
                            this.chrome.stage_tabs.insert(space.clone(), FleetPane::Agent { space, panel: panel.clone() });
                        }
                        cx.notify();
                    })));
                tab_index += 1;
            }
        }
        bar = bar.child(div().id(("task-tab-add", session_index)).flex_none().px(px(8.)).h_full().flex().items_center()
            .text_size(px(12.)).text_color(self.theme.fg2).hover(|style| style.text_color(self.theme.fg0)).cursor_pointer().child("＋")
            .tooltip(Tooltip::text(i18n::t!("fleet.tab_add_thread"), self.theme.clone()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                this.switch_project(session_index, window, cx);
                this.add_fleet_agent(cx);
            })));

        let shells = self.task_shells(&space).len();
        let mut toggles = div().flex_none().flex().items_center().gap(px(2.)).ml_auto().pl(px(8.))
            .child(div().flex_none().mr(px(4.)).text_size(px(11.)).text_color(self.theme.fg2).child("◨"));
        if bridge {
            toggles = toggles.child(self.side_toggle(("task-side-formation", session_index), i18n::t!("fleet.formation").into(), None, matches!(side, Some(FleetPane::Formation { .. })))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session_index, window, cx);
                    let space = this.project_sessions.projects[session_index].task_space.id.clone();
                    let open = matches!(this.stage_side(&space), Some(FleetPane::Formation { .. }));
                    this.set_stage_side(session_index, (!open).then_some(FleetPane::Formation { space }), cx);
                })));
            toggles = toggles.child(self.side_toggle(("task-side-captain-log", session_index), i18n::t!("captain.log").into(), Some(self.captain_log_count()), matches!(side, Some(FleetPane::CaptainLog { .. })))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session_index, window, cx);
                    let space = this.project_sessions.projects[session_index].task_space.id.clone();
                    let open = matches!(this.stage_side(&space), Some(FleetPane::CaptainLog { .. }));
                    this.set_stage_side(session_index, (!open).then_some(FleetPane::CaptainLog { space }), cx);
                })));
        }
        toggles = toggles.child(self.side_toggle(("task-side-diff", session_index), i18n::t!("fleet.tab_diff").into(), None, matches!(side, Some(FleetPane::Diff { .. })))
            .children(self.shortstat_chips(session.repository.shortstat()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                this.switch_project(session_index, window, cx);
                let space = this.project_sessions.projects[session_index].task_space.id.clone();
                if matches!(this.stage_side(&space), Some(FleetPane::Diff { .. })) { this.set_stage_side(session_index, None, cx); return; }
                this.refresh_git_status_for(session_index, cx);
                this.set_stage_side(session_index, Some(FleetPane::Diff { space }), cx);
            })));
        toggles = toggles.child(self.side_toggle(("task-side-terminal", session_index), i18n::t!("fleet.tab_terminal").into(), Some(shells), matches!(side, Some(FleetPane::Shell { .. })))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                this.switch_project(session_index, window, cx);
                let space = this.project_sessions.projects[session_index].task_space.id.clone();
                if matches!(this.stage_side(&space), Some(FleetPane::Shell { .. })) { this.set_stage_side(session_index, None, cx); return; }
                // 1 本も無ければここで作る（「端末を足す」を先に探させない）。
                match this.task_shells(&space).into_iter().next() {
                    Some(shell) => this.set_stage_side(session_index, Some(shell), cx),
                    None => this.add_terminal_to_selected_task(cx),
                }
            })));
        toggles = toggles.child(self.side_toggle(("task-side-files", session_index), i18n::t!("fleet.tab_files").into(), None, matches!(side, Some(FleetPane::Editor { .. })))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                this.switch_project(session_index, window, cx);
                let space = this.project_sessions.projects[session_index].task_space.id.clone();
                let open = matches!(this.stage_side(&space), Some(FleetPane::Editor { .. }));
                this.set_stage_side(session_index, (!open).then_some(FleetPane::Editor { space }), cx);
            })));
        bar.child(toggles).into_any_element()
    }

    /// サイドペイン本体（見出し + 中身）。ターミナルの追加は見出しの通番チップの ＋ から。
    pub(super) fn render_side_pane(&self, session_index: usize, side: FleetPane, cx: &mut Context<Self>) -> gpui::AnyElement {
        let session = &self.project_sessions.sessions[session_index];
        let slot = &self.project_sessions.projects[session_index];
        let space = slot.task_space.id.clone();
        let mut extras: Vec<gpui::AnyElement> = Vec::new();
        let (title, detail, body): (SharedString, Option<SharedString>, gpui::AnyElement) = match &side {
            FleetPane::Shell { id, .. } => {
                for (number, shell) in self.task_shells(&space).into_iter().enumerate() {
                    let current = shell == side;
                    extras.push(div().id(("task-shell", session_index * 100 + number)).flex_none().min_w(px(18.)).h(px(18.)).px(px(5.)).flex().items_center().justify_center().rounded(px(4.))
                        .text_size(px(10.)).text_color(if current { self.theme.fg0 } else { self.theme.fg2 }).when(current, |element| element.bg(self.theme.bg2))
                        .cursor_pointer().child(SharedString::from((number + 1).to_string()))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.set_stage_side(session_index, Some(shell.clone()), cx)))
                        .into_any_element());
                }
                extras.push(div().id(("task-shell-add", session_index)).flex_none().size(px(18.)).flex().items_center().justify_center().rounded(px(4.))
                    .text_size(px(11.)).text_color(self.theme.fg2).hover(|style| style.bg(self.theme.bg2)).cursor_pointer().child("＋")
                    .tooltip(Tooltip::text(i18n::t!("fleet.tab_add_terminal"), self.theme.clone()))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        this.switch_project(session_index, window, cx);
                        this.add_terminal_to_selected_task(cx);
                    })).into_any_element());
                let body = session.terminal_dock.read(cx).session(*id)
                    .map(|terminal| terminal.cached(StyleRefinement::default().size_full()).into_any_element())
                    .unwrap_or_else(|| div().into_any_element());
                (i18n::t!("fleet.tab_terminal").into(), Some(format!("⎇ {}", slot.branch.as_deref().unwrap_or("")).into()), body)
            }
            FleetPane::Terminal { .. } => (i18n::t!("fleet.tab_terminal").into(), None,
                session.terminal_dock.clone().cached(StyleRefinement::default().flex().flex_col().size_full()).into_any_element()),
            FleetPane::Tests { .. } => (i18n::t!("fleet.tab_terminal").into(), None,
                session.tests_dock.clone().cached(StyleRefinement::default().flex().flex_col().size_full()).into_any_element()),
            FleetPane::Editor { .. } => (i18n::t!("fleet.tab_files").into(), Some(i18n::t!("fleet.files_hint").into()), self.render_stage_files(session_index, cx)),
            FleetPane::Formation { .. } => {
                extras = self.render_graph_view_chips(cx);
                let tasks = self.project_sessions.projects.iter()
                    .filter(|candidate| candidate.repository_key() == slot.repository_key() && !candidate.task_space.is_integration() && candidate.task_space.phase != TaskPhase::Archived)
                    .count();
                (i18n::t!("fleet.formation").into(), Some(i18n::t!("fleet.tasks_count", "n" => tasks).into()),
                    div().id(("stage-formation", session_index)).size_full().overflow_y_scroll().child(self.render_formation(cx)).into_any_element())
            }
            FleetPane::CaptainLog { .. } => (i18n::t!("captain.log").into(), Some(i18n::t!("captain.human_gate").into()), self.render_captain_log()),
            _ => {
                extras.extend(self.shortstat_chips(session.repository.shortstat()));
                let files = session.repository.task_files.len();
                (i18n::t!("fleet.tab_diff").into(), Some(i18n::t!("fleet.diff_files", "count" => files).into()), self.render_stage_changes(session_index, cx))
            }
        };
        div().size_full().flex().flex_col()
            .child(self.side_header(("task-side-close", session_index), title, extras, detail, move |this, _, cx| this.set_stage_side(session_index, None, cx), cx))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .into_any_element()
    }

    /// カード 1 枚の幅（概算）。分割の可否とサイドペインのドラッグ量の換算に使う。
    pub(super) fn stage_card_width(&self) -> f32 {
        let cards = self.stage_cards().len().max(1) as f32;
        (self.chrome.stage_width - 20. - 8. * (cards - 1.)) / cards
    }

    /// カード本体 = 会話 | 境（ドラッグで幅を変える）| サイドペイン。幅が足りなければサイドペインだけを全面に出す。
    pub(super) fn render_card_body(&self, id: impl Into<gpui::ElementId>, conversation: gpui::AnyElement, side: Option<gpui::AnyElement>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match side {
            None => conversation,
            Some(side) if self.stage_card_is_wide() => div().size_full().flex()
                .child(div().flex_1().min_w_0().h_full().child(conversation))
                .child(div().id(id).flex_none().w(px(RESIZE_HANDLE_WIDTH)).h_full().border_l_1().border_color(self.theme.border)
                    .cursor(CursorStyle::ResizeLeftRight).hover(|style| style.bg(self.theme.border))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.chrome.resizing_side = Some((this.chrome.side_pane_ratio, this.stage_card_width()));
                        this.chrome.resize_start_x = f32::from(event.position.x);
                        cx.notify();
                    })))
                .child(div().flex_none().w(gpui::relative(self.chrome.side_pane_ratio)).h_full().child(side))
                .into_any_element(),
            Some(side) => side,
        }
    }

    pub(super) fn render_lineage_strip(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut strip = div().id("lineage-strip").h_full().flex().items_center().gap(px(12.)).overflow_x_scroll();
        for (index, slot) in self.project_sessions.projects.iter().enumerate() {
            if Some(slot.repository_key()) != self.active_repository_key() || slot.task_space.phase == TaskPhase::Archived { continue; }
            strip = strip.child(div().id(("lineage-task", index)).flex_none().text_size(px(10.)).text_color(slot.color).cursor_pointer()
                .child(SharedString::from(format!("{} {}", if slot.task_space.is_integration() { "━" } else if slot.task_space.phase == TaskPhase::Integrated { "╰━" } else { "┬━" }, slot.task_space.title)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| { this.switch_project(index, window, cx); })));
        }
        strip.into_any_element()
    }
}

impl Workspace {
    /// 変更ペイン: この Task が base から変えたファイルの一覧（状態の 1 文字 + パス + `+N −M`）。
    /// 押すとそのファイルの diff を Editor に開く（Fleet 内にエディタは持たない）。
    /// `GitPanel` の Entity は描画を持たない（本体は `render_git_panel` が active session だけを描く）ので、
    /// カードごとの一覧はここで組む — F2 以来「変更」の中身が空だった原因（2026-09-20）。
    pub(super) fn render_stage_changes(&self, session: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let root = self.project_sessions.projects[session].worktree.root().to_path_buf();
        let files = &self.project_sessions.sessions[session].repository.task_files;
        if files.is_empty() {
            return div().size_full().flex().items_center().justify_center().text_size(px(12.)).text_color(self.theme.fg2)
                .child(SharedString::from(i18n::t!("fleet.diff_empty"))).into_any_element();
        }
        let mut list = div().id(("stage-changes", session)).size_full().overflow_y_scroll().py(px(4.));
        for (index, file) in files.iter().enumerate() {
            let path = file.path.clone();
            let relative = path.strip_prefix(&root).unwrap_or(&path);
            let name = relative.file_name().unwrap_or_default().to_string_lossy().to_string();
            let folder = relative.parent().map(|parent| parent.to_string_lossy().to_string()).filter(|parent| !parent.is_empty());
            let tint = Self::git_tint(&self.theme, file.kind);
            list = list.child(div().id(("stage-change", session * 100000 + index)).flex().items_center().gap(px(7.)).px(px(12.)).h(px(24.))
                .text_size(px(12.)).hover(|style| style.bg(self.theme.bg2)).cursor_pointer()
                .child(div().flex_none().w(px(10.)).text_size(px(10.5)).text_color(tint).child(Self::git_letter(file.kind)))
                .child(div().flex_none().text_color(self.theme.fg0).child(SharedString::from(name)))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(10.5)).text_color(self.theme.fg2)
                    .when_some(folder, |element, folder| element.child(SharedString::from(folder))))
                .children(self.shortstat_chips(Some((file.added, file.deleted))))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session, window, cx);
                    this.chrome.fleet_mode = false;
                    this.open_diff_tab_for(path.clone(), None, window, cx);
                })));
        }
        list.into_any_element()
    }

    /// ファイルペイン: worktree のツリー。行 = ▸/▾ + 名前 + 右端に git の 1 文字（エクスプローラと同じ語彙）。
    /// ファイルを押すと Editor へ出る（Fleet 内にエディタは持たない）。
    pub(super) fn render_stage_files(&self, session: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let slot = &self.project_sessions.projects[session];
        let status = &self.project_sessions.sessions[session].repository.status;
        let mut tree = div().id(("stage-files", session)).size_full().overflow_y_scroll().py(px(4.));
        for (index, row) in slot.explorer.rows.iter().enumerate() {
            let path = row.path.clone();
            let directory = row.is_dir;
            let label = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let change = status.get(&path).copied();
            let glyph = if !directory { "" } else if row.is_expanded { "▾" } else { "▸" };
            tree = tree.child(div().id(("stage-file", session * 100000 + index)).flex().items_center().gap(px(5.))
                .pl(px(10. + row.depth as f32 * 12.)).pr(px(10.)).h(px(22.)).text_size(px(12.))
                .text_color(change.map_or(if directory { self.theme.fg1 } else { self.theme.fg0 }, |change| Self::git_tint(&self.theme, change)))
                .hover(|style| style.bg(self.theme.bg2)).cursor_pointer()
                .child(div().flex_none().w(px(10.)).text_size(px(9.)).text_color(self.theme.fg2).child(glyph))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(SharedString::from(label)))
                .when_some(change, |element, change| element.child(div().flex_none().text_size(px(10.)).child(Self::git_letter(change))))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.switch_project(session, window, cx);
                    if directory { this.toggle_dir(path.clone(), cx); } else {
                        this.chrome.fleet_mode = false;
                        this.open_file(path.clone(), window, cx);
                    }
                })));
        }
        tree.into_any_element()
    }
}

impl Workspace {
    pub(super) fn render_task_header(&self, cell: usize, space: &SpaceId, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(index) = self.session_index_for_space(space) else { return div().into_any_element(); };
        let slot = &self.project_sessions.projects[index];
        let phase = slot.task_space.phase;
        // ブリッジ（統合先）は Task の phase を持たない — 役割（統合先 · 保護）を出す。
        let (phase_key, action_key) = if slot.task_space.is_integration() { ("fleet.integration_row", "fleet.next_review") } else { match phase {
            TaskPhase::Blocked => ("fleet.phase_blocked", "fleet.next_allow"),
            TaskPhase::Failed | TaskPhase::ChangesRequested => ("fleet.phase_failed", "fleet.next_fix"),
            TaskPhase::ReviewReady => ("fleet.phase_review", "fleet.next_review"),
            TaskPhase::MergeReady => ("fleet.phase_review", "control.integrate"),
            TaskPhase::Integrating | TaskPhase::Integrated => ("fleet.phase_integrated", "fleet.cleanup_tip"),
            _ => ("fleet.phase_working", "fleet.next_review"),
        } };
        let renaming = self.chrome.task_renaming.as_ref().filter(|rename| rename.index == index && rename.site == RenameSite::Cell).map(|rename| rename.editor.clone());
        let title = if let Some(editor) = renaming { div().h(px(22.)).child(editor).into_any_element() } else {
            div().id(("task-title", index)).overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(self.theme.fg0).child(slot.task_space.title.clone())
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if event.click_count == 2 { this.start_task_rename(index, RenameSite::Cell, window, cx); }
                })).into_any_element()
        };
        div().flex_none().flex().items_center().gap(px(7.)).px(px(10.)).py(px(6.)).border_b_1().border_color(self.theme.border)
            .child(div().size(px(7.)).rounded_full().bg(slot.color))
            .child(div().flex_1().min_w_0().child(title))
            .child(div().text_size(px(10.)).text_color(self.theme.fg2).child(SharedString::from(format!("⎇ {}", slot.branch.as_deref().unwrap_or("")))))
            .child(div().text_size(px(9.)).text_color(self.theme.fg2).child(SharedString::from(i18n::t!(phase_key))))
            .when(index == self.project_sessions.active && !slot.task_space.is_integration(), |header| header.child(
                div().id(("task-next", index)).cursor_pointer().text_size(px(10.)).text_color(self.theme.fg1).child(SharedString::from(i18n::t!(action_key)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let space = this.project_sessions.projects[index].task_space.id.clone();
                        match phase {
                            TaskPhase::MergeReady => this.integrate_task(space, cx),
                            TaskPhase::Integrated | TaskPhase::Integrating => this.open_fleet_cell_menu(cell, event.position, cx),
                            TaskPhase::Failed | TaskPhase::ChangesRequested => {
                                let panel = this.project_sessions.sessions[index].agent_panel.clone();
                                panel.update(cx, |panel, cx| panel.focus_composer(window, cx));
                            }
                            TaskPhase::Blocked => {
                                let pending = this.project_sessions.sessions[index].agent_statuses(cx).into_iter().find_map(|(panel, thread, _)| {
                                    let card = panel.read(cx).permission_card(thread)?;
                                    let option = card.options.iter().find(|(_, kind, _)| *kind == agent_panel::AgentPermissionKind::Allow)?.0;
                                    Some((panel, thread, option))
                                });
                                if let Some((panel, thread, option)) = pending {
                                    panel.update(cx, |panel, cx| panel.respond_permission(thread, option, cx));
                                    this.transition_task_space(index, TaskPhase::Working, "permission_resolved", None, cx);
                                }
                            },
                            _ => this.review_task_for_merge(space, cx),
                        }
                    }))))
            .child(div().id(("task-expand", index)).cursor_pointer().text_color(self.theme.fg2).child("⤢")
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.chrome.stage_columns = 1; this.switch_project(index, window, cx); cx.notify();
                })))
            .child(div().id(("task-menu", index)).cursor_pointer().text_color(self.theme.fg2).child("⋯")
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| this.open_fleet_cell_menu(cell, event.position, cx))))
            .into_any_element()
    }
}
