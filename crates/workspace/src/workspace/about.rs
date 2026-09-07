//! about — 「necoder について」モーダル + 手動アップデート確認。
//!
//! 右下トーストだった About を、普通の Mac アプリと同じ**モーダル**にする（2026-08-30 要望）。
//! 構成はネイティブ About パネル準拠: アイコン / 名前 / バージョン / コピーライト + リンク。
//! Sparkle 系アプリに倣い、更新の確認と適用もここに集約する — メニュー
//! 「アップデートを確認…」= About を開いて即確認。適用の実体は自動アップデートと同じ
//! `install_update`（chrome.rs・statusbar チップと状態を共有 = 二重管理しない）。
//!
//! offscreen QA: `NECODER_ABOUT=1` で起動時から開く（NECODER_SETTINGS と同型）。

use crate::workspace::*;
use gpui::img;

/// リンク行の飛び先（ラベルは URL/固有名なので i18n しない・statusbar の "UTF-8" と同じ扱い）。
const WEBSITE_URL: &str = "https://necoder.com";
const GITHUB_URL: &str = "https://github.com/iKora128/necoder";

impl Workspace {
    /// メニュー「necoder について」。モーダルを開くだけ（勝手にネットへ出ない）。
    pub(crate) fn about_action(&mut self, _: &About, window: &mut Window, cx: &mut Context<Self>) {
        self.open_about(window, cx);
    }

    /// メニュー「アップデートを確認…」。About を開いて即確認を始める（Sparkle の作法）。
    pub(crate) fn check_for_updates_action(
        &mut self,
        _: &CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_about(window, cx);
        self.start_manual_update_check(cx);
    }

    fn open_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.overlays.about = Some(focus);
        cx.notify();
    }

    fn close_about(&mut self, cx: &mut Context<Self>) {
        if self.overlays.about.take().is_some() {
            cx.notify();
        }
    }

    fn on_about_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            self.close_about(cx);
        }
    }

    /// 手動の更新確認（背景）。自動チェックが既に新版を見つけている / 確認中なら何もしない。
    /// 新版が見つかったら自動チェックと同じ `updater.status` に合流＝statusbar チップも同時に出る。
    pub(crate) fn start_manual_update_check(&mut self, cx: &mut Context<Self>) {
        if !updater::manual_check_supported()
            || self.updater.status.is_some()
            || self.updater.manual == ManualUpdateCheck::Checking
        {
            return;
        }
        self.updater.manual = ManualUpdateCheck::Checking;
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::check_for_update_manual(env!("CARGO_PKG_VERSION")) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                match result {
                    Ok(Some(info)) => {
                        workspace.updater.status = Some((info, UpdateState::Available));
                        workspace.updater.manual = ManualUpdateCheck::Idle;
                    }
                    Ok(None) => workspace.updater.manual = ManualUpdateCheck::UpToDate,
                    Err(error) => {
                        workspace.updater.manual =
                            ManualUpdateCheck::Failed(SharedString::from(format!("{error:#}")))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// About モーダルの描画（開いている時のみ）。shortcut_sheet と同じ器
    /// （薄暗い背景・背景クリック / Escape で閉じる・パネル内クリックは素通りしない）。
    pub(crate) fn render_about_modal(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let focus = self.overlays.about.as_ref()?;
        let theme = self.theme.clone();
        let accent = self.accent();

        // 更新まわりの 1 行 + ボタン。自動チェックの status が最優先（チップと同じ真実を映す）。
        let (update_line, update_button): (
            Option<(SharedString, gpui::Hsla)>,
            Option<(SharedString, AboutUpdateClick)>,
        ) = match self.updater.status.clone() {
            Some((info, UpdateState::Available)) => (
                Some((
                    SharedString::from(
                        i18n::t!("about.update_available", "version" => info.version),
                    ),
                    accent,
                )),
                Some((
                    SharedString::from(i18n::t!("about.update_now")),
                    AboutUpdateClick::Install,
                )),
            ),
            Some((_, UpdateState::Installing(progress))) => (
                Some((
                    SharedString::from(update_progress_label(progress)),
                    theme.fg2,
                )),
                None,
            ),
            Some((info, UpdateState::Ready)) => (
                Some((SharedString::from(i18n::t!("update.ready")), accent)),
                Some((
                    SharedString::from(
                        i18n::t!("update.restart", "version" => info.version.clone()),
                    ),
                    AboutUpdateClick::Restart,
                )),
            ),
            None => {
                let check_button = updater::manual_check_supported().then(|| {
                    (
                        SharedString::from(i18n::t!("about.check_button")),
                        AboutUpdateClick::Check,
                    )
                });
                match self.updater.manual.clone() {
                    ManualUpdateCheck::Checking => (
                        Some((SharedString::from(i18n::t!("about.checking")), theme.fg2)),
                        None,
                    ),
                    ManualUpdateCheck::UpToDate => (
                        Some((SharedString::from(i18n::t!("about.up_to_date")), theme.ok)),
                        check_button,
                    ),
                    ManualUpdateCheck::Failed(detail) => (Some((detail, theme.warn)), check_button),
                    ManualUpdateCheck::Idle => (None, check_button),
                }
            }
        };

        let link = |id: &'static str, label: &'static str, url: &'static str| {
            div()
                .id(id)
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(move |style| style.text_color(accent))
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        if let Err(error) = crate::crash::open_url(url) {
                            this.push_toast(
                                SharedString::from(format!("{error:#}")),
                                this.accent(),
                                cx,
                            );
                        }
                    }),
                )
        };

        let panel = div()
            .w(px(300.))
            .flex()
            .flex_col()
            .items_center()
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .rounded(px(14.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(10.),
                gpui::hsla(0., 0., 0., 0.45),
            )
            .blur_radius(px(28.))])
            .px(px(24.))
            .pt(px(26.))
            .pb(px(16.))
            .track_focus(focus)
            .on_key_down(cx.listener(Self::on_about_key_down))
            // パネル内クリックは背景の「閉じる」に伝播させない。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _window, cx| cx.stop_propagation()),
            )
            .child(img("icon/necoder.png").size(px(96.)))
            .child(
                div()
                    .pt(px(8.))
                    .text_size(px(19.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.fg0)
                    .child("necoder"),
            )
            .child(
                div()
                    .pt(px(1.))
                    .text_size(px(11.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(format!(
                        "v{}",
                        env!("CARGO_PKG_VERSION")
                    ))),
            )
            .child(
                div()
                    .pt(px(6.))
                    .text_size(px(11.5))
                    .text_color(theme.fg1)
                    .child(SharedString::from(i18n::t!("about.tagline"))),
            )
            .when_some(update_line, |element, (line, color)| {
                element.child(
                    div()
                        .pt(px(10.))
                        .max_w_full()
                        .text_size(px(11.))
                        .text_color(color)
                        .text_center()
                        .child(line),
                )
            })
            .when_some(update_button, |element, (label, click)| {
                element.child(
                    div()
                        .id("about-update-button")
                        .mt(px(10.))
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg2).text_color(theme.fg0))
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| match click {
                                AboutUpdateClick::Install => this.install_update(cx),
                                AboutUpdateClick::Check => this.start_manual_update_check(cx),
                                AboutUpdateClick::Restart => this.restart_after_update(window, cx),
                            }),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pt(px(14.))
                    .child(link("about-website", "necoder.com", WEBSITE_URL))
                    .child(div().text_size(px(11.)).text_color(theme.fg2).child("·"))
                    .child(link("about-github", "GitHub", GITHUB_URL)),
            )
            .child(
                div()
                    .pt(px(6.))
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("about.copyright"))),
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
                    cx.listener(|this, _, _window, cx| this.close_about(cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}

/// About モーダルの更新ボタンが何をするか（段階で変わる）。
#[derive(Clone, Copy)]
enum AboutUpdateClick {
    Install,
    Check,
    Restart,
}
