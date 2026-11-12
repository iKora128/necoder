//! transcript のリンクの右クリックメニュー（2026-11-12 本人要望）。
//!
//! 左クリックは従来どおり「necoder で開く」。右クリックでは開き方を選べる:
//! ファイル = 開く / Finder で表示 / 既定のアプリで開く / パスをコピー / 相対パスをコピー、
//! URL = 開く / URL をコピー。Finder と既定のアプリはローカルの宛先だけ（SSH 先のパスを
//! こちらの Finder に渡しても意味が無い）。文言はエクスプローラ・タブのメニューと同じキーを使う。

use super::*;

/// transcript のリンクの右クリックメニュー。
pub(crate) struct LinkMenu {
    /// 押したリンクの行き先（描画時に解決済みの物）。
    pub(crate) destination: LinkDestination,
    /// 押した場所（窓の座標）。メニューの左上になる。
    pub(crate) position: gpui::Point<Pixels>,
}

/// メニューの 1 項目でやること。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LinkMenuAction {
    Open,
    Reveal,
    OpenWithDefaultApp,
    CopyPath,
    CopyRelativePath,
}

impl AgentPanel {
    /// 右クリックがリンクの上ならメニューを開いて `true`（リンク以外は何もしない）。
    pub(crate) fn open_link_menu_at(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((_, _, destination)) = self.link_at(position, cx) else {
            return false;
        };
        self.open_menu = None;
        self.thread_menu = None;
        self.link_menu = Some(LinkMenu {
            destination,
            position,
        });
        cx.notify();
        true
    }

    /// メニューを閉じる。開いていたら `true`。
    pub(crate) fn close_link_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if self.link_menu.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// 開発用（offscreen 検証）: 行き先を直に渡してメニューを開く。
    #[cfg(debug_assertions)]
    pub fn debug_open_link_menu(
        &mut self,
        path: PathBuf,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.link_menu = Some(LinkMenu {
            destination: LinkDestination::File {
                path,
                line: None,
                column: None,
            },
            position,
        });
        cx.notify();
    }

    /// 行き先に出す項目（上から順）。区切り線は「開き方」と「コピー」の間に 1 本。
    pub(crate) fn link_menu_actions(&self, destination: &LinkDestination) -> Vec<LinkMenuAction> {
        match destination {
            LinkDestination::Url(_) => vec![LinkMenuAction::Open, LinkMenuAction::CopyPath],
            LinkDestination::File { path, .. } => {
                let mut actions = vec![LinkMenuAction::Open];
                if !self.dest_host.is_remote() {
                    actions.push(LinkMenuAction::Reveal);
                    actions.push(LinkMenuAction::OpenWithDefaultApp);
                }
                actions.push(LinkMenuAction::CopyPath);
                if self.link_relative_path(path).is_some() {
                    actions.push(LinkMenuAction::CopyRelativePath);
                }
                actions
            }
        }
    }

    /// 宛先プロジェクトのルートからの相対パス（ルートの外・ルートそのものは `None`）。
    fn link_relative_path(&self, path: &Path) -> Option<String> {
        let root = self.dest_cwd.as_ref()?;
        let relative = path.strip_prefix(root).ok()?;
        let text = relative.to_string_lossy();
        (!text.is_empty()).then(|| text.into_owned())
    }

    /// 項目を実行してメニューを閉じる。
    pub(crate) fn run_link_menu_action(
        &mut self,
        destination: LinkDestination,
        action: LinkMenuAction,
        cx: &mut Context<Self>,
    ) {
        self.link_menu = None;
        match (action, &destination) {
            (LinkMenuAction::Open, _) => self.activate_link(destination, cx),
            (LinkMenuAction::Reveal, LinkDestination::File { path, .. }) => {
                if let Err(error) = project::reveal_in_finder_local(path) {
                    eprintln!("Finder 表示に失敗: {error:#}");
                }
            }
            (LinkMenuAction::OpenWithDefaultApp, LinkDestination::File { path, .. }) => {
                if let Err(error) = project::open_with_default_app_local(path) {
                    eprintln!("既定アプリで開けない: {error:#}");
                }
            }
            (LinkMenuAction::CopyPath, LinkDestination::File { path, .. }) => {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    path.to_string_lossy().into_owned(),
                ));
            }
            (LinkMenuAction::CopyPath, LinkDestination::Url(url)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()));
            }
            (LinkMenuAction::CopyRelativePath, LinkDestination::File { path, .. }) => {
                if let Some(relative) = self.link_relative_path(path) {
                    cx.write_to_clipboard(ClipboardItem::new_string(relative));
                }
            }
            // URL に Finder / 既定のアプリ / 相対パスは出さない（`link_menu_actions`）。
            (_, LinkDestination::Url(_)) => {}
        }
        cx.notify();
    }

    /// リンクの右クリックメニュー。面はスレッドのメニューと同じ（bg2・枠・角 8・影）で、色は中立。
    pub(crate) fn render_link_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let menu = self.link_menu.as_ref()?;
        let destination = menu.destination.clone();
        let theme = self.theme.clone();
        let is_url = matches!(destination, LinkDestination::Url(_));
        let mut menu_box = div()
            .w(px(210.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .p(px(4.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            // メニューの外を押したら閉じる（transcript・パネルの外を含む）。
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_link_menu(cx);
            }));
        let mut previous: Option<LinkMenuAction> = None;
        for action in self.link_menu_actions(&destination) {
            let copies = matches!(
                action,
                LinkMenuAction::CopyPath | LinkMenuAction::CopyRelativePath
            );
            let previous_copies = matches!(
                previous,
                Some(LinkMenuAction::CopyPath | LinkMenuAction::CopyRelativePath)
            );
            if previous.is_some() && copies && !previous_copies {
                menu_box = menu_box.child(div().my(px(3.)).h(px(1.)).bg(theme.border));
            }
            previous = Some(action);
            let (id, label) = match action {
                LinkMenuAction::Open => ("link-menu-open", i18n::t!("explorer.ctx_open")),
                LinkMenuAction::Reveal => ("link-menu-reveal", i18n::t!("explorer.ctx_reveal")),
                LinkMenuAction::OpenWithDefaultApp => (
                    "link-menu-open-default",
                    i18n::t!("explorer.ctx_open_default"),
                ),
                LinkMenuAction::CopyPath if is_url => {
                    ("link-menu-copy-url", i18n::t!("webtab.ctx_copy_url"))
                }
                LinkMenuAction::CopyPath => {
                    ("link-menu-copy-path", i18n::t!("explorer.ctx_copy_path"))
                }
                LinkMenuAction::CopyRelativePath => (
                    "link-menu-copy-relative",
                    i18n::t!("tabs.ctx_copy_relative"),
                ),
            };
            let target = destination.clone();
            menu_box = menu_box.child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(theme.fg1)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.run_link_menu_action(target.clone(), action, cx);
                        }),
                    ),
            );
        }
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(menu.position)
                    .snap_to_window()
                    .child(menu_box),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
