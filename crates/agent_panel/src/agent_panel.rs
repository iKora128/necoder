//! agent_panel — 右ドックのエージェントパネル（M4。差別化の本丸）。
//!
//! UI-SPEC §6: スレッド = 固有色のタブ。宛先チップ（スレッド名 + project ⎇ branch）と
//! トークン常時表示は必須要件（`docs/BACKGROUND.md` の痛点が原点）。アクティブスレッドの色が
//! タブ下線・トークンメーター・msg-user 左縁・thinking 左縁・composer 枠・送信ボタン・宛先チップの
//! ドットへ**一斉に貫通**する（= 混戦対策の本体）。
//!
//! composer は [`editor_view::EditorView`] の平坦モードを再利用する（IME・複数カーソル・undo を共通化）。
//! ACP との実接続（session/prompt/stream）は次段（B2）。現状はスレッド構造 + 送信でのユーザー発話追加まで。

use acp_client::{
    AgentEvent, ConfigCategory, ConfigOption, PermissionChoice, PermissionDiff, PermissionKind,
    SessionCommand, run_session,
};
use editor_view::{ComposerEvent, EditorView};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{
    Animation, AnimationExt, App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla,
    IntoElement, MouseButton, SharedString, Window, actions, div, img, prelude::*, pulsating_between,
    px,
};
use std::path::PathBuf;
use theme_core::{Theme, claude_bullet, thread_color};
use ui::Tooltip;

actions!(agent, [SubmitPrompt, CloseActiveThread]);

const THREAD_TABS_HEIGHT: f32 = 34.0;
const COMPOSER_INPUT_HEIGHT: f32 = 68.0;
/// composer のモデルセレクタに並べる候補（クリックでアクティブスレッドに設定）。
/// 表示ラベルの切替まで（ACP エージェントへの実反映は継続課題）。
const MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "claude-fable-5",
];
/// 権限モード（Claude Code 相当。実配線は ACP の SessionMode／set_mode 経由で継続課題）。
const PERMISSION_MODES: &[&str] = &["default", "accept edits", "bypass permissions", "plan"];
/// 推論の effort（Zed 下部コントロール相当）。
const EFFORTS: &[&str] = &["low", "medium", "high", "max"];

/// composer 下部の選択ピル種別（Zed のエージェント下部コントロールに倣う）。
#[derive(Clone, Copy, PartialEq)]
enum Selector {
    Agent,
    Mode,
    Model,
    Effort,
}

impl Selector {
    fn options(self) -> &'static [&'static str] {
        match self {
            Selector::Agent => acp_client::AGENT_LABELS,
            Selector::Mode => PERMISSION_MODES,
            Selector::Model => MODELS,
            Selector::Effort => EFFORTS,
        }
    }

}

/// transcript の 1 エントリ（VSCode Claude Code 拡張のトランスクリプトを踏襲）。
enum Entry {
    /// ユーザー発話（bg2 箱・左縁スレッド色）。
    User(SharedString),
    /// 思考（常時展開・斜体）。
    Thinking(SharedString),
    /// ステップ（⏺ ツール名 + 引数 → ⎿ 結果）。
    Step {
        tool: SharedString,
        args: SharedString,
        result: Option<SharedString>,
    },
    /// エージェントの本文（結論など）。
    Agent(SharedString),
}

/// 承認待ちの権限リクエスト（`session/request_permission` を UI で保持する間の状態）。
/// `respond` に選んだ選択肢の添字を送ると acp_client が応答する（このスレッドの当該ターンは
/// それまでブロックしている）。ユーザーが答えるまで composer 上部にカードを出す。
struct PendingPermission {
    title: SharedString,
    diffs: Vec<PermissionDiff>,
    options: Vec<PermissionChoice>,
    respond: mpsc::UnboundedSender<usize>,
}

/// 1 スレッド = 1 会話。固有色を持ち、UI 全体へ貫通する。
struct Thread {
    name: SharedString,
    color: Hsla,
    running: bool,
    /// このターンの開始時刻（経過秒の表示に使う。`None`＝未実行）。
    turn_started_at: Option<std::time::Instant>,
    model: SharedString,
    permission_mode: SharedString,
    effort: SharedString,
    /// このスレッドが話す ACP エージェント（ラベル。既定 "Claude"）。
    agent: SharedString,
    /// 添付コンテキスト（プロジェクト相対パス）。送信時に `@path` として prompt 先頭へ付ける。
    context: Vec<SharedString>,
    entries: Vec<Entry>,
    tokens_used: u32,
    tokens_max: u32,
    /// 表示用の補間値（`tokens_used` へ滑らかに追従＝カウントアップ演出）。追いついたら停止し idle 0% を保つ。
    tokens_shown: f32,
    /// 常駐 ACP セッションへ指示（prompt / mode 変更）を送るハンドル（初回送信で遅延起動）。`None` = 未起動。
    command_tx: Option<mpsc::UnboundedSender<SessionCommand>>,
    /// エージェントが広告する権限モード `(mode_id, 表示名)` と現在の mode_id（セッション開始後に埋まる）。
    available_modes: Vec<(SharedString, SharedString)>,
    current_mode_id: SharedString,
    /// エージェントが広告する設定オプション（モデル・思考レベル等）。あれば Model/Effort セレクタを
    /// 実選択肢に置き換え、選択で `session/set_config_option` を送る。
    configs: Vec<ConfigOption>,
    /// 承認待ちの権限リクエスト（あれば composer 上部にカードを出す）。
    pending_permission: Option<PendingPermission>,
}

impl Thread {
    fn empty(name: impl Into<SharedString>, index: usize) -> Thread {
        Thread {
            name: name.into(),
            color: thread_color(index),
            running: false,
            turn_started_at: None,
            model: "claude-fable-5".into(),
            permission_mode: "default".into(),
            effort: "high".into(),
            agent: "Claude Code".into(),
            context: Vec::new(),
            entries: Vec::new(),
            tokens_used: 0,
            tokens_max: 200_000,
            tokens_shown: 0.0,
            command_tx: None,
            available_modes: Vec::new(),
            current_mode_id: SharedString::default(),
            configs: Vec::new(),
            pending_permission: None,
        }
    }
}

/// エージェントパネル本体（右ドックまるごとを描く）。
pub struct AgentPanel {
    threads: Vec<Thread>,
    active: usize,
    theme: Theme,
    /// 宛先（アクティブプロジェクトのコンテキスト）。workspace が [`Self::set_destination`] で更新する。
    dest_project: SharedString,
    dest_branch: Option<SharedString>,
    /// ACP エージェントの起動 cwd（アクティブプロジェクトのルート）。無ければ送信できない。
    dest_cwd: Option<PathBuf>,
    composer: Entity<EditorView>,
    /// composer 下部の選択ピルのうち開いているメニュー（None = 閉）。
    open_menu: Option<Selector>,
    /// Add context の候補（プロジェクトのファイル相対パス。workspace が渡す）と開閉。
    context_files: Vec<SharedString>,
    context_menu_open: bool,
    /// Enter 送信の現在値（送信ヒント表示 + トグルの状態。composer にも反映する）。
    submit_on_enter: bool,
    /// トークン表示のカウントアップ補間タスクが稼働中か（多重起動防止。追いついたら false に戻す＝idle 0%）。
    token_ticker: bool,
    /// 直近の成功でマスコットがバンザイ中か（数秒で false に戻る）。世代番号で古いタイマーを無効化する。
    celebrating: bool,
    celebrate_gen: u32,
    /// ストリーミング平滑化: アクティブスレッドの末尾（Agent/Thinking）を何文字まで表示したか（タイプライタ）。
    /// チャンクは束で届くので、これを一定速度で目標長へ寄せてカクつきを消す。`usize::MAX`＝全部表示。
    reveal: usize,
    reveal_ticker: bool,
}

impl AgentPanel {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        // 設定は global（settings.json が真実）から取る。live-reload / CLI / MCP 変更に observe で追従。
        let submit_on_enter = settings::get(cx).submit_on_enter;
        let composer =
            cx.new(|cx| EditorView::plain(theme.clone(), thread_color(0), submit_on_enter, cx));
        // composer の Enter 送信要求（IME 変換中は来ない）を受けて submit する。
        cx.subscribe(&composer, |panel, _composer, event, cx| match event {
            ComposerEvent::Submit => panel.submit(cx),
        })
        .detach();
        // 設定変更（UI トグル / 手編集 / CLI / MCP のどれでも）に追従して composer へ反映。
        cx.observe_global::<settings::SettingsGlobal>(|panel, cx| {
            let submit_on_enter = settings::get(cx).submit_on_enter;
            if submit_on_enter != panel.submit_on_enter {
                panel.submit_on_enter = submit_on_enter;
                panel
                    .composer
                    .update(cx, |composer, cx| composer.set_submit_on_enter(submit_on_enter, cx));
                cx.notify();
            }
        })
        .detach();
        // 開発用: SHIRUSHI_ACP_PROBE があれば、少し待って空スレッドへ自動送信（実機ストリーミングの自己検証）。
        if let Ok(probe) = std::env::var("SHIRUSHI_ACP_PROBE") {
            if !probe.trim().is_empty() {
                cx.spawn(async move |panel, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    panel
                        .update(cx, |panel, cx| {
                            panel.switch_thread(1, cx); // 種の無い空スレッドへ（応答が先頭で見える）
                            panel.send_prompt_text(probe, cx);
                            // 開発用: SHIRUSHI_OPEN_MENU=model|effort|mode|agent でセレクタを開いて撮る
                            // （広告設定が届くと再描画され実選択肢が出る）。
                            if let Ok(which) = std::env::var("SHIRUSHI_OPEN_MENU") {
                                let selector = match which.trim() {
                                    "model" => Some(Selector::Model),
                                    "effort" => Some(Selector::Effort),
                                    "mode" => Some(Selector::Mode),
                                    "agent" => Some(Selector::Agent),
                                    _ => None,
                                };
                                if let Some(selector) = selector {
                                    panel.open_menu = Some(selector);
                                }
                            }
                        })
                        .ok();
                })
                .detach();
            }
        }
        AgentPanel {
            threads: seed_threads(),
            active: 0,
            theme,
            dest_project: "—".into(),
            dest_branch: None,
            dest_cwd: None,
            composer,
            open_menu: None,
            context_files: Vec::new(),
            context_menu_open: false,
            submit_on_enter,
            token_ticker: false,
            celebrating: false,
            celebrate_gen: 0,
            reveal: usize::MAX, // 初期は全表示（seed を打ち直さない）
            reveal_ticker: false,
        }
    }

    /// Enter 送信の有効/無効をトグルする。**設定 store（settings.json が真実）を更新するだけ**で、
    /// 実際の反映（composer 更新・再描画）は `observe_global` 経由で起きる（UI / CLI / MCP と同じ経路）。
    fn toggle_submit_on_enter(&mut self, cx: &mut Context<Self>) {
        let value = !self.submit_on_enter;
        settings::set_user_value(cx, "submit_on_enter", serde_json::Value::Bool(value));
    }

    /// 宛先チップに出すプロジェクト名・ブランチ・cwd を設定する（プロジェクト切替時に workspace が呼ぶ）。
    pub fn set_destination(
        &mut self,
        project: SharedString,
        branch: Option<SharedString>,
        cwd: Option<PathBuf>,
        files: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.dest_project = project;
        self.dest_branch = branch;
        self.dest_cwd = cwd;
        self.context_files = files;
        cx.notify();
    }

    fn active_color(&self) -> Hsla {
        self.threads.get(self.active).map(|thread| thread.color).unwrap_or_else(|| thread_color(0))
    }

    /// titlebar beacon / statusbar ドット用: (スレッド名, スレッド色, 実行中) の一覧。
    /// UI-SPEC §3: 実行中スレッドの状態を窓上部から常に見えるようにする（BACKGROUND の原点痛点）。
    pub fn beacons(&self) -> Vec<(SharedString, Hsla, bool)> {
        self.threads
            .iter()
            .map(|thread| (thread.name.clone(), thread.color, thread.running))
            .collect()
    }

    fn switch_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.threads.len() || index == self.active {
            return;
        }
        self.active = index;
        self.reveal = usize::MAX; // 切替先は全文表示（打ち直さない）
        let color = self.active_color();
        self.composer.update(cx, |composer, cx| composer.set_accent(color, cx));
        cx.notify();
    }

    /// スレッドタブを閉じる（× ボタン）。最後の 1 枚は残す（空 UI を避ける）。
    /// 閉じた瞬間そのスレッドの ACP セッション（`command_tx`）も drop され読みループが畳まれる。
    fn remove_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.threads.len() <= 1 || index >= self.threads.len() {
            return;
        }
        self.threads.remove(index);
        // active を有効な近傍へ寄せる（閉じたのが前なら1つ詰める／自分なら次のタブ、末尾なら前へ）。
        if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = self.active.min(self.threads.len() - 1);
        }
        self.reveal = usize::MAX; // 実効スレッドが変わるので全文表示
        let color = self.active_color();
        self.composer.update(cx, |composer, cx| composer.set_accent(color, cx));
        cx.notify();
    }

    fn add_thread(&mut self, cx: &mut Context<Self>) {
        let index = self.threads.len();
        self.threads.push(Thread::empty(format!("スレッド{}", index + 1), index));
        self.switch_thread(index, cx);
        cx.notify();
    }

    /// テーマを差し替える（テーマセレクタのライブプレビュー / 切替）。composer にも波及させる。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        self.composer.update(cx, |composer, cx| composer.set_theme(theme, cx));
        cx.notify();
    }

    /// 新規スレッドを作る公開口（workspace の ⌘⇧A から呼ぶ）。
    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.add_thread(cx);
    }

    fn toggle_menu(&mut self, selector: Selector, cx: &mut Context<Self>) {
        self.open_menu = if self.open_menu == Some(selector) { None } else { Some(selector) };
        cx.notify();
    }

    /// セレクタのドロップダウンに出す選択肢。エージェントが広告した実選択肢（モード/モデル/effort）が
    /// あればそれを、無ければ静的な既定ラベルを返す。
    fn selector_options(&self, selector: Selector) -> Vec<SharedString> {
        let advertised = self.threads.get(self.active).and_then(|thread| match selector {
            Selector::Mode if !thread.available_modes.is_empty() => {
                Some(thread.available_modes.iter().map(|(_, name)| name.clone()).collect())
            }
            Selector::Model => config_choice_names(thread, ConfigCategory::Model),
            Selector::Effort => config_choice_names(thread, ConfigCategory::ThoughtLevel),
            _ => None,
        });
        advertised
            .unwrap_or_else(|| selector.options().iter().map(|option| SharedString::from(*option)).collect())
    }

    fn selector_value(&self, selector: Selector) -> SharedString {
        self.threads
            .get(self.active)
            .map(|thread| match selector {
                Selector::Agent => thread.agent.clone(),
                Selector::Mode => thread.permission_mode.clone(),
                Selector::Model => thread.model.clone(),
                Selector::Effort => thread.effort.clone(),
            })
            .unwrap_or_default()
    }

    /// 選択ピルの値を設定してメニューを閉じる（ラベル切替。ACP の SessionMode/model/config への
    /// 実反映は継続課題）。
    fn select_option(&mut self, selector: Selector, value: SharedString, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            match selector {
                Selector::Agent => {
                    thread.agent = value;
                    thread.command_tx = None; // エージェント変更 → 次の送信で新セッションを張り直す
                }
                Selector::Mode => {
                    thread.permission_mode = value.clone();
                    // 表示名 → mode_id を引いて `session/set_mode` を送る（広告モードがある時）。
                    let mode_id = thread
                        .available_modes
                        .iter()
                        .find(|(_, name)| *name == value)
                        .map(|(id, _)| id.to_string());
                    if let (Some(mode_id), Some(command_tx)) = (mode_id, &thread.command_tx) {
                        command_tx.unbounded_send(SessionCommand::SetMode(mode_id)).ok();
                    }
                }
                Selector::Model => {
                    thread.model = value.clone();
                    send_set_config(thread, ConfigCategory::Model, &value);
                }
                Selector::Effort => {
                    thread.effort = value.clone();
                    send_set_config(thread, ConfigCategory::ThoughtLevel, &value);
                }
            }
        }
        self.open_menu = None;
        cx.notify();
    }

    fn toggle_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_open = !self.context_menu_open;
        self.open_menu = None; // セレクタメニューと排他
        cx.notify();
    }

    /// コンテキストにファイルを追加（重複は無視）してメニューを閉じる。
    fn add_context(&mut self, path: SharedString, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            if !thread.context.contains(&path) {
                thread.context.push(path);
            }
        }
        self.context_menu_open = false;
        cx.notify();
    }

    fn remove_context(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            if index < thread.context.len() {
                thread.context.remove(index);
            }
        }
        cx.notify();
    }

    /// composer 下部の選択ピル（Zed 風。クリックでドロップダウン）。
    /// composer 下部の選択コントロール。Zed 流の「テキスト + シェブロン」＝**枠も塗りも無し**、
    /// hover でだけ薄く面が出る。開いている時だけラベルをスレッド色（accent）にする（チップにしない）。
    fn render_selector_pill(&self, selector: Selector, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let value = self.selector_value(selector);
        let is_open = self.open_menu == Some(selector);
        let accent = self.active_color();
        let (id, tip): (&'static str, &'static str) = match selector {
            Selector::Agent => ("pill-agent", "エージェント（どの AI で話すか）"),
            Selector::Mode => ("pill-mode", "権限モード（編集・実行の許可レベル）"),
            Selector::Model => ("pill-model", "モデル"),
            Selector::Effort => ("pill-effort", "推論の深さ（effort）"),
        };
        let label_color = if is_open { accent } else { theme.fg1 };
        div()
            .id(id)
            .relative() // ドロップダウンをこのピル基準で絶対配置する（位置ズレを防ぐ）
            .flex()
            .items_center()
            .gap(px(3.))
            .px(px(5.))
            .py(px(2.))
            .rounded(px(4.))
            .text_size(px(11.))
            .text_color(label_color)
            .cursor_pointer()
            // 塗りは hover と open の時だけ薄く。テキストは hover で少し明るく。
            .hover(|style| style.bg(theme.bg2).text_color(if is_open { accent } else { theme.fg0 }))
            .when(is_open, |element| element.bg(theme.bg2))
            .child(value)
            .child(div().text_size(px(8.)).text_color(theme.fg2).child("▾"))
            .tooltip(Tooltip::text(tip, theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.toggle_menu(selector, cx)),
            )
            // 開いている時、このピルの真上にドロップダウンを出す（ピル基準なのでズレない）。
            .when(is_open, |element| element.child(self.render_selector_menu(selector, cx)))
    }

    /// 開いている選択ピルのドロップダウン（アクティブスレッドに値を設定）。ピルの子として絶対配置し、
    /// **ピルの真上**に開く。右寄りのピル（Model/Effort）は右端揃えにしてパネル外へはみ出さない。
    /// 出現時に一度だけ fade-in + 少し下からせり上がる（with_animation・oneshot＝idle 0% 維持）。
    fn render_selector_menu(&self, selector: Selector, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let current = self.selector_value(selector);
        // エージェントが広告する実選択肢（モード/モデル/effort）があればそれを優先。無ければ既定表示。
        let options = self.selector_options(selector);
        let align_right = matches!(selector, Selector::Model | Selector::Effort);
        div()
            .absolute()
            .bottom(px(24.)) // ピル（高さ ~20）の少し上に開く
            .when(align_right, |element| element.right(px(0.)))
            .when(!align_right, |element| element.left(px(0.)))
            .w(px(180.))
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .shadow(vec![gpui::BoxShadow::new(
                px(0.),
                px(6.),
                gpui::hsla(0., 0., 0., 0.4),
            )
            .blur_radius(px(16.))])
            .p(px(4.))
            .children(options.into_iter().enumerate().map(|(option_index, option)| {
                let selected = option == current;
                div()
                    .id(("selector-option", option_index))
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .text_size(px(12.))
                    .text_color(if selected { theme.fg0 } else { theme.fg1 })
                    .cursor_pointer()
                    .when(selected, |element| element.bg(theme.bg3))
                    .hover(|style| style.bg(theme.bg3))
                    .child(option.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation(); // ピルの toggle が再発火して開き直すのを防ぐ
                            this.select_option(selector, option.clone(), cx)
                        }),
                    )
            }))
            .with_animation(
                "selector-menu",
                Animation::new(std::time::Duration::from_millis(120)).with_easing(gpui::ease_out_quint()),
                |element, delta| element.opacity(delta).bottom(px(24.0 - 6.0 * (1.0 - delta))),
            )
            .into_any_element()
    }

    /// アクション（⌘Enter）ハンドラ。実処理は [`Self::submit`]。
    fn on_submit(&mut self, _: &SubmitPrompt, _window: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    /// ⌘W: アクティブスレッドを閉じる（× ボタンと同じ。最後の1枚は残す）。
    fn on_close_thread(&mut self, _: &CloseActiveThread, _window: &mut Window, cx: &mut Context<Self>) {
        self.remove_thread(self.active, cx);
    }

    /// composer の内容をアクティブスレッドへ積み、**常駐 ACP セッション**へ prompt を送る（空なら無視）。
    /// 応答は `run_session` からのイベントを [`Self::on_event`] で**逐次** transcript に反映する（ストリーミング）。
    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).plain_text();
        let prompt = text.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.composer.update(cx, |composer, cx| composer.clear(cx));
        self.send_prompt_text(prompt, cx);
    }

    /// prompt テキストをアクティブスレッドへ積み、常駐 ACP セッションへ送る（composer 非依存）。
    /// 開発時の自動プローブ（`SHIRUSHI_ACP_PROBE`）からも使う。
    fn send_prompt_text(&mut self, prompt: String, cx: &mut Context<Self>) {
        let thread_index = self.active;
        // 添付コンテキストを prompt 先頭へ `@path` として付ける（表示は素の prompt のまま）。
        let context_prefix: String = self
            .threads
            .get(thread_index)
            .map(|thread| thread.context.iter().map(|path| format!("@{path}\n")).collect())
            .unwrap_or_default();
        let full_prompt = if context_prefix.is_empty() {
            prompt.clone()
        } else {
            format!("{context_prefix}\n{prompt}")
        };
        if let Some(thread) = self.threads.get_mut(thread_index) {
            thread.entries.push(Entry::User(SharedString::from(prompt.clone())));
            thread.running = true;
            thread.turn_started_at = Some(std::time::Instant::now()); // 経過秒の起点
        }
        cx.notify();

        // 宛先プロジェクトの cwd が要る。
        let Some(cwd) = self.dest_cwd.clone() else {
            self.fail_turn(thread_index, "プロジェクトが未確定のため送れません", cx);
            return;
        };
        // セッション未起動なら遅延起動（初回のみプロセスを立てる。以後は常駐＝文脈が続く）。
        let has_session = self
            .threads
            .get(thread_index)
            .is_some_and(|thread| thread.command_tx.is_some());
        if !has_session {
            match self.start_session(thread_index, cwd, cx) {
                Some(command_tx) => {
                    if let Some(thread) = self.threads.get_mut(thread_index) {
                        thread.command_tx = Some(command_tx);
                    }
                }
                None => {
                    self.fail_turn(thread_index, "claude-agent-acp が見つかりません（node/npx を確認）", cx);
                    return;
                }
            }
        }
        // prompt を送信。送信路が死んでいたらセッションを畳んで報告。
        let alive = self
            .threads
            .get(thread_index)
            .and_then(|thread| thread.command_tx.as_ref())
            .map(|command_tx| command_tx.unbounded_send(SessionCommand::Prompt(full_prompt)).is_ok())
            .unwrap_or(false);
        if !alive {
            if let Some(thread) = self.threads.get_mut(thread_index) {
                thread.command_tx = None;
            }
            self.fail_turn(thread_index, "セッションが切れました。もう一度送信してください", cx);
        }
    }

    /// スレッド用の常駐 ACP セッションを起動する。バックグラウンドで `run_session` を回し、
    /// フォアグラウンドで受信イベントを [`Self::on_event`] に適用する。送信ハンドルを返す。
    /// claude-agent-acp が見つからなければ `None`。
    fn start_session(
        &self,
        thread_index: usize,
        cwd: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<mpsc::UnboundedSender<SessionCommand>> {
        // スレッドが選んでいるエージェント（Claude / Codex / …）の起動コマンドを解決。
        let agent_label = self
            .threads
            .get(thread_index)
            .map(|thread| thread.agent.clone())
            .unwrap_or_else(|| "Claude".into());
        let command = acp_client::AgentKind::by_label(&agent_label)?.command(cwd)?;
        let (command_tx, prompt_rx) = mpsc::unbounded::<SessionCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded::<AgentEvent>();
        let error_tx = event_tx.clone();

        cx.background_executor()
            .spawn(async move {
                if let Err(error) = run_session(command, prompt_rx, event_tx).await {
                    error_tx.unbounded_send(AgentEvent::Failed(error.to_string())).ok();
                }
            })
            .detach();

        cx.spawn(async move |panel, cx| {
            while let Some(event) = event_rx.next().await {
                if panel
                    .update(cx, |panel, cx| panel.on_event(thread_index, event, cx))
                    .is_err()
                {
                    break; // パネル破棄済み（ウィンドウを閉じた等）
                }
            }
        })
        .detach();

        Some(command_tx)
    }

    /// `run_session` の [`AgentEvent`] を transcript へ逐次反映する（ストリーミングの心臓部）。
    /// 増分テキストは直前の同種エントリへ連結（新ターンは先頭に User があるので自然に区切れる）。
    fn on_event(&mut self, thread_index: usize, event: AgentEvent, cx: &mut Context<Self>) {
        let active = self.active;
        let mut start_token_ticker = false;
        let mut celebrate_now = false;
        let mut ensure_reveal = false; // アクティブが Agent/Thinking をストリーム → タイプライタ稼働
        let mut reveal_reset = false; // 新しいストリームエントリ開始 → 先頭から打つ
        let Some(thread) = self.threads.get_mut(thread_index) else {
            return;
        };
        match event {
            AgentEvent::AgentChunk(text) => {
                match thread.entries.last_mut() {
                    Some(Entry::Agent(existing)) => {
                        let mut combined = existing.to_string();
                        combined.push_str(&text);
                        *existing = combined.into();
                    }
                    _ => {
                        thread.entries.push(Entry::Agent(text.into()));
                        reveal_reset = thread_index == active; // 新エントリは先頭から打つ
                    }
                }
                ensure_reveal = thread_index == active;
            }
            AgentEvent::ThoughtChunk(text) => {
                match thread.entries.last_mut() {
                    Some(Entry::Thinking(existing)) => {
                        let mut combined = existing.to_string();
                        combined.push_str(&text);
                        *existing = combined.into();
                    }
                    _ => {
                        thread.entries.push(Entry::Thinking(text.into()));
                        reveal_reset = thread_index == active;
                    }
                }
                ensure_reveal = thread_index == active;
            }
            AgentEvent::ToolStarted(title) => thread.entries.push(Entry::Step {
                tool: SharedString::from(title),
                args: SharedString::default(),
                result: None,
            }),
            AgentEvent::Usage { used, size } => {
                thread.tokens_used = used.min(u32::MAX as u64) as u32;
                if size > 0 {
                    thread.tokens_max = size.min(u32::MAX as u64) as u32;
                }
                // アクティブスレッドは表示値を目標へ滑らかに補間（カウントアップ）。
                // 非アクティブは即時同期（見えないので演出不要＝無駄な再描画を避ける）。
                if thread_index == active {
                    start_token_ticker = true;
                } else {
                    thread.tokens_shown = thread.tokens_used as f32;
                }
            }
            AgentEvent::Modes { modes, current } => {
                thread.available_modes = modes
                    .into_iter()
                    .map(|(id, name)| (SharedString::from(id), SharedString::from(name)))
                    .collect();
                let current = SharedString::from(current);
                if let Some((_, name)) = thread.available_modes.iter().find(|(id, _)| *id == current) {
                    thread.permission_mode = name.clone();
                }
                thread.current_mode_id = current;
            }
            AgentEvent::ModeChanged(id) => {
                let id = SharedString::from(id);
                if let Some((_, name)) = thread.available_modes.iter().find(|(mid, _)| *mid == id) {
                    thread.permission_mode = name.clone();
                }
                thread.current_mode_id = id;
            }
            AgentEvent::Configs(configs) => {
                // 現在値の表示名を Model / Effort ピルへ反映（mode は modes() 側で扱う）。
                for config in &configs {
                    let current_name = config
                        .choices
                        .iter()
                        .find(|(id, _)| *id == config.current)
                        .map(|(_, name)| SharedString::from(name.clone()));
                    match (config.category, current_name) {
                        (ConfigCategory::Model, Some(name)) => thread.model = name,
                        (ConfigCategory::ThoughtLevel, Some(name)) => thread.effort = name,
                        _ => {}
                    }
                }
                thread.configs = configs;
            }
            AgentEvent::PermissionRequest { title, diffs, options, respond } => {
                // 開発用: SHIRUSHI_AUTO_ALLOW があれば最初の許可肢で即応答（round-trip 自己検証）。
                if std::env::var_os("SHIRUSHI_AUTO_ALLOW").is_some() {
                    let allow = options
                        .iter()
                        .position(|option| {
                            matches!(option.kind, PermissionKind::Allow | PermissionKind::AllowAlways)
                        })
                        .unwrap_or(0);
                    respond.unbounded_send(allow).ok();
                    return;
                }
                thread.pending_permission = Some(PendingPermission {
                    title: SharedString::from(title),
                    diffs,
                    options,
                    respond,
                });
                // 承認待ちの間もターンは継続中（running のまま＝ pulse で「待ち」を示す）。
            }
            AgentEvent::TurnEnded => {
                thread.running = false;
                thread.pending_permission = None; // 念のため（通常は応答時に消える）
                // アクティブスレッドの成功でマスコットがバンザイ（数秒だけ）。失敗（Failed）では祝わない。
                celebrate_now = thread_index == active;
            }
            AgentEvent::Failed(error) => {
                thread.entries.push(Entry::Agent(SharedString::from(format!("エラー: {error}"))));
                thread.running = false;
            }
        }
        if start_token_ticker {
            self.ensure_token_ticker(cx);
        }
        if celebrate_now {
            self.start_celebrate(cx);
        }
        if reveal_reset {
            self.reveal = 0;
        }
        if ensure_reveal {
            self.ensure_reveal_ticker(cx);
        }
        cx.notify();
    }

    /// ストリーミング平滑化のタイプライタ。アクティブスレッド末尾（Agent/Thinking）の目標文字数へ
    /// `reveal` を ~60fps で一定速度に寄せる。ターンが終わり全文字出し切ったら停止＝idle 0%。
    fn ensure_reveal_ticker(&mut self, cx: &mut Context<Self>) {
        if self.reveal_ticker {
            return;
        }
        self.reveal_ticker = true;
        cx.spawn(async move |panel, cx| {
            loop {
                let done = panel
                    .update(cx, |panel, cx| {
                        let Some(thread) = panel.threads.get(panel.active) else {
                            return true;
                        };
                        let running = thread.running;
                        let target = match thread.entries.last() {
                            Some(Entry::Agent(text)) | Some(Entry::Thinking(text)) => {
                                text.chars().count()
                            }
                            // 末尾が非ストリーム（Step/User）なら出し切り扱い。
                            _ => panel.reveal.min(usize::MAX),
                        };
                        if panel.reveal < target {
                            // 残りに比例した歩幅＋最低速で、束で来ても滑らかに追従。
                            let remaining = target - panel.reveal;
                            let step = (remaining / 6).max(2);
                            panel.reveal = panel.reveal.saturating_add(step).min(target);
                            cx.notify();
                        }
                        // ターン継続中は回し続け、終了かつ出し切ったら停止。
                        !running && panel.reveal >= target
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
            }
            panel
                .update(cx, |panel, _cx| {
                    panel.reveal_ticker = false;
                })
                .ok();
        })
        .detach();
    }

    /// 成功直後にマスコットをバンザイさせる。約2.4秒で Idle に戻す（世代番号で古いタイマーを無効化）。
    /// バンザイ中はアニメが回るが、生成直後の一過性なので idle 0% は実質保たれる。
    fn start_celebrate(&mut self, cx: &mut Context<Self>) {
        self.celebrating = true;
        self.celebrate_gen = self.celebrate_gen.wrapping_add(1);
        let generation = self.celebrate_gen;
        cx.spawn(async move |panel, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2400))
                .await;
            panel
                .update(cx, |panel, cx| {
                    if panel.celebrate_gen == generation {
                        panel.celebrating = false;
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    /// トークン表示のカウントアップ補間を回す（多重起動しない）。
    /// アクティブスレッドの `tokens_shown` を `tokens_used` へ ~30fps で指数的に近づけ、
    /// 追いついたら停止する（＝タスク終了で再描画も止まり idle 0% を保つ）。
    fn ensure_token_ticker(&mut self, cx: &mut Context<Self>) {
        if self.token_ticker {
            return;
        }
        self.token_ticker = true;
        cx.spawn(async move |panel, cx| {
            loop {
                let done = panel
                    .update(cx, |panel, cx| {
                        let Some(thread) = panel.threads.get_mut(panel.active) else {
                            return true;
                        };
                        let target = thread.tokens_used as f32;
                        let diff = target - thread.tokens_shown;
                        if diff.abs() < 0.75 {
                            thread.tokens_shown = target;
                            cx.notify();
                            return true;
                        }
                        // 指数イージング（速く始まりゆっくり収束）＋最低歩幅で確実に到達。
                        let step = diff * 0.22;
                        let step = if step.abs() < 1.0 { diff.signum() } else { step };
                        thread.tokens_shown += step;
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
            }
            panel
                .update(cx, |panel, _cx| {
                    panel.token_ticker = false;
                })
                .ok();
        })
        .detach();
    }

    /// 承認待ちの権限リクエストに、選んだ選択肢の**添字**で応答する（許可/拒否ボタンのクリック）。
    /// respond へ送ると acp_client 側がブロックを解いてエージェントに `Selected(option_id)` を返す。
    fn answer_permission(&mut self, option_index: usize, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            if let Some(pending) = thread.pending_permission.take() {
                pending.respond.unbounded_send(option_index).ok();
            }
        }
        cx.notify();
    }

    /// ターンを失敗として畳む（エラー行を積み running を下ろす）。
    fn fail_turn(&mut self, thread_index: usize, message: &str, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(thread_index) {
            thread
                .entries
                .push(Entry::Agent(SharedString::from(format!("エラー: {message}"))));
            thread.running = false;
        }
        cx.notify();
    }

    // ── 描画 ──

    fn render_thread_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.active;
        let count = self.threads.len(); // 最後の1枚は × を出さない
        div()
            .flex()
            .items_stretch()
            .h(px(THREAD_TABS_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(theme.border)
            .children(self.threads.iter().enumerate().map(|(index, thread)| {
                let is_active = index == active;
                let color = thread.color;
                div()
                    .id(("thread-tab", index))
                    .flex()
                    .flex_col()
                    .h_full()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    // Zed 流の即時 hover（bg1）。アクティブは常時 bg1。id で hover 再描画を保証。
                    .hover(|style| style.bg(theme.bg1))
                    .when(is_active, |element| element.bg(theme.bg1))
                    // アクティブスレッドタブ上線 = スレッド色（UI-SPEC §6）
                    .child(div().h(px(2.)).w_full().bg(if is_active { color } else { theme.bg0 }))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(12.))
                            .text_size(px(12.))
                            .text_color(if is_active { theme.fg0 } else { theme.fg1 })
                            .child(pulsing_dot(("thtab-dot", index), 8.0, color, thread.running))
                            .child(thread.name.clone())
                            // × 閉じる（最後の1枚は出さない）。クリックはタブ切替へ伝播させない。
                            .when(count > 1, |element| {
                                element.child(
                                    div()
                                        .id(("thtab-close", index))
                                        .flex_none()
                                        .px(px(3.))
                                        .rounded(px(4.))
                                        .text_color(theme.fg2)
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(theme.fg0).bg(theme.bg2))
                                        .child("×")
                                        .tooltip(Tooltip::text("スレッドを閉じる  ⌘W", theme.clone()))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _window, cx| {
                                                cx.stop_propagation();
                                                this.remove_thread(index, cx);
                                            }),
                                        ),
                                )
                            }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.switch_thread(index, cx)),
                    )
            }))
            .child(
                div()
                    .id("add-thread")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(32.))
                    .h_full()
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                    .child("＋")
                    .tooltip(Tooltip::text("新規スレッド  ⌘⇧A", theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.add_thread(cx)),
                    ),
            )
    }

    fn render_meta(&self, active: bool) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let thread = self.threads.get(self.active);
        let (shown, max) = thread.map(|thread| (thread.tokens_shown, thread.tokens_max)).unwrap_or((0.0, 0));
        let used = shown.round().max(0.0) as u32; // 表示は補間値（カウントアップ演出）
        let ratio = if max == 0 { 0.0 } else { (shown / max as f32).clamp(0.0, 1.0) };
        let running = thread.map(|thread| thread.running).unwrap_or(false);
        // 末尾が Thinking ブロック中か（ACP の実状態）＝考える中。
        let thinking = thread
            .and_then(|thread| thread.entries.last())
            .map(|entry| matches!(entry, Entry::Thinking(_)))
            .unwrap_or(false);
        // マスコットの状態: 考え中→考える / 生成中→打鍵 / 直近成功→バンザイ / それ以外→寝落ち静止（0%）。
        let motion = if running {
            if thinking {
                MascotMotion::Think
            } else {
                MascotMotion::Typing
            }
        } else if self.celebrating {
            MascotMotion::Celebrate
        } else {
            MascotMotion::Idle
        };
        // 左のステータス行（スカスカ対策＋「何が起きてるか」）: 状態テキスト＋実行中は経過秒。
        let elapsed = running
            .then(|| thread.and_then(|thread| thread.turn_started_at))
            .flatten()
            .map(|start| start.elapsed().as_secs_f32());
        let (status_text, status_dim) = if running {
            (if thinking { "✳ 考え中" } else { "⏺ 生成中" }, false)
        } else if self.celebrating {
            ("🙌 完了", false)
        } else {
            ("待機中", true)
        };
        let status_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().text_color(if status_dim { theme.fg2 } else { color }).child(status_text))
            .when_some(elapsed, |element, secs| {
                element.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .font_family("Guguru Sans Code")
                        .child(format!("{secs:.1}s")),
                )
            });

        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(7.))
            .flex_none()
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .text_color(theme.fg1)
            .child(status_row)
            .child(div().flex_1())
            // ローディング・マスコット（考=考える/生成=打鍵/成功=バンザイ/待機=うとうと）。
            // 非アクティブ時は静止＝再描画ゼロ（見てない間は 0%）。トークン真横に。
            .child(render_mascot(motion, active))
            // トークンは常時可視（Zed+ACP で見えなかった痛点）
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        div()
                            .w(px(64.))
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(theme.bg3)
                            .overflow_hidden()
                            .child(div().h_full().w(px(64.0 * ratio)).bg(color)),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .child(format!("{}/{}", human_tokens(used), human_tokens(max))),
                    ),
            )
    }

    fn render_transcript(&self) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let entries = self.threads.get(self.active).map(|thread| &thread.entries);
        let mut list = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(13.))
            .px(px(12.))
            .py(px(12.))
            .bg(theme.bg1)
            .overflow_hidden();
        if let Some(entries) = entries {
            let count = entries.len();
            for (index, entry) in entries.iter().enumerate() {
                // 末尾エントリだけタイプライタ表示（reveal 文字まで）。他は全文。
                let reveal = if index + 1 == count { Some(self.reveal) } else { None };
                // 出現時に一度だけ fade in（id 固定なのでストリーミングの再描画では再発火しない）。
                list = list.child(
                    div().child(self.render_entry(entry, color, reveal)).with_animation(
                        ("transcript-entry", index),
                        Animation::new(std::time::Duration::from_millis(200)),
                        |element, delta| element.opacity(delta),
                    ),
                );
            }
        }
        list
    }

    /// タイプライタ用に文字列を `reveal` 文字で切る（`None` または末尾以外は全文）。
    fn revealed(text: &SharedString, reveal: Option<usize>) -> SharedString {
        match reveal {
            Some(limit) if limit < text.chars().count() => {
                text.chars().take(limit).collect::<String>().into()
            }
            _ => text.clone(),
        }
    }

    fn render_entry(&self, entry: &Entry, color: Hsla, reveal: Option<usize>) -> gpui::AnyElement {
        let theme = &self.theme;
        match entry {
            Entry::User(text) => div()
                .flex()
                .items_stretch()
                .rounded(px(7.))
                .overflow_hidden()
                .bg(theme.bg2)
                .border_1()
                .border_color(theme.border)
                .child(div().w(px(2.)).flex_none().bg(color.alpha(0.65)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0() // flex 子の折り返しを許可（はみ出し防止）
                        .px(px(11.))
                        .py(px(8.))
                        .text_size(px(12.5))
                        .text_color(theme.fg0)
                        .child(text.clone()),
                )
                .into_any_element(),
            Entry::Thinking(text) => div()
                .flex()
                .items_stretch()
                .child(div().w(px(2.)).flex_none().bg(color.alpha(0.45)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0() // 折り返し許可
                        .gap(px(3.))
                        .pl(px(11.))
                        .child(
                            div()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color.alpha(0.85))
                                .child("✳ Thinking…"),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .italic()
                                .text_color(theme.fg2)
                                .child(Self::revealed(text, reveal)),
                        ),
                )
                .into_any_element(),
            Entry::Step { tool, args, result } => {
                let mut body = div().flex().flex_col().min_w_0().child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.fg0)
                                .child(tool.clone()),
                        )
                        .when(!args.is_empty(), |element| {
                            element.child(
                                div()
                                    .font_family("Guguru Sans Code")
                                    .text_size(px(11.))
                                    .text_color(theme.fg2)
                                    .child(args.clone()),
                            )
                        }),
                );
                if let Some(result) = result {
                    body = body.child(render_result(result, theme.fg2));
                }
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .child(div().flex_none().text_color(claude_bullet()).child("⏺"))
                    .child(body)
                    .into_any_element()
            }
            Entry::Agent(text) => div()
                .text_size(px(12.5))
                .text_color(theme.fg0)
                .child(Self::revealed(text, reveal))
                .into_any_element(),
        }
    }

    /// Add context のドロップダウン（プロジェクトのファイル候補。クリックで添付）。
    fn render_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let attached: Vec<SharedString> = self
            .threads
            .get(self.active)
            .map(|thread| thread.context.clone())
            .unwrap_or_default();
        div()
            .absolute()
            .left(px(11.))
            .bottom(px(150.))
            .w(px(320.))
            .max_h(px(260.))
            .overflow_hidden()
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .p(px(4.))
            .child(
                div()
                    .px(px(9.))
                    .py(px(4.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child("ファイルを文脈に追加（先頭のみ・検索は継続課題）"),
            )
            .children(self.context_files.iter().take(18).cloned().enumerate().map(|(file_index, path)| {
                let already = attached.contains(&path);
                div()
                    .id(("context-file", file_index))
                    .flex()
                    .items_center()
                    .px(px(9.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .text_size(px(11.5))
                    .text_color(if already { theme.fg2 } else { theme.fg1 })
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.bg3))
                    .child(path.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| this.add_context(path.clone(), cx)),
                    )
            }))
    }

    /// 承認待ちの権限リクエストのカード（composer の直上）。ツール名・編集差分・許可/拒否ボタン。
    /// ターンをブロックしているので、transcript がスクロールしても常に見える位置に置く。
    fn render_permission_card(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let thread = self.threads.get(self.active)?;
        let pending = thread.pending_permission.as_ref()?;
        let theme = self.theme.clone();
        let color = thread.color;

        let mut card = div()
            .id("permission-card")
            .mx(px(12.))
            .mb(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .rounded(px(9.))
            .overflow_hidden()
            .bg(theme.bg2)
            .border_1()
            .border_color(color.alpha(0.6))
            .px(px(11.))
            .py(px(9.))
            // ヘッダ: 「承認が必要」+ ツールタイトル
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child("● 承認が必要"),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.5))
                            .text_color(theme.fg0)
                            .child(pending.title.clone()),
                    ),
            );

        // 編集差分（あれば）を diff レビューとして表示。
        for diff in &pending.diffs {
            card = card.child(render_diff(diff, &theme));
        }

        // 許可/拒否ボタン列（選択肢は ACP が広告したもの。添字で応答）。
        let buttons = div().flex().flex_wrap().items_center().gap(px(6.)).children(
            pending.options.iter().enumerate().map(|(index, option)| {
                permission_button(index, option, color, &theme, cx)
            }),
        );
        card = card.child(buttons);

        Some(card.into_any_element())
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let running = self.threads.get(self.active).map(|thread| thread.running).unwrap_or(false);
        let thread_name =
            self.threads.get(self.active).map(|thread| thread.name.clone()).unwrap_or_default();
        let destination = match &self.dest_branch {
            Some(branch) => format!("{} ⎇ {branch}", self.dest_project),
            None => self.dest_project.to_string(),
        };
        let context: Vec<SharedString> = self
            .threads
            .get(self.active)
            .map(|thread| thread.context.clone())
            .unwrap_or_default();

        div()
            .flex_none()
            .px(px(12.))
            .py(px(10.))
            .bg(theme.bg1)
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(9.))
                    .bg(theme.bg0)
                    .border_1()
                    .border_color(color.alpha(0.5))
                    .px(px(11.))
                    .py(px(8.))
                    // 宛先チップ: どのスレッド/PJ・ブランチに送るかを常に明示（混戦対策）
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pb(px(6.))
                            .text_size(px(10.5))
                            .text_color(theme.fg1)
                            .child(pulsing_dot("dest-dot", 8.0, color, running))
                            .child(thread_name)
                            .child(div().flex_1())
                            .child(div().text_color(theme.fg2).child(destination)),
                    )
                    // Add context: ＋ ボタン + 添付チップ（× で外す）
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(5.))
                            .pb(px(6.))
                            .child(
                                div()
                                    .id("add-context")
                                    .flex()
                                    .items_center()
                                    .px(px(7.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.border)
                                    .text_size(px(10.5))
                                    .text_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.text_color(theme.fg0).border_color(theme.fg1)
                                    })
                                    .child("＋ context")
                                    .tooltip(Tooltip::text("ファイルを文脈に添付", theme.clone()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.toggle_context_menu(cx)
                                        }),
                                    ),
                            )
                            .children(context.into_iter().enumerate().map(|(index, path)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .px(px(7.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .bg(theme.bg3)
                                    .text_size(px(10.5))
                                    .text_color(theme.fg1)
                                    .child(path)
                                    .child(
                                        div()
                                            .id(("context-chip-x", index))
                                            .text_color(theme.fg2)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(theme.fg0))
                                            .child("×")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _window, cx| {
                                                    this.remove_context(index, cx)
                                                }),
                                            ),
                                    )
                            })),
                    )
                    // composer 本体（平坦 EditorView。Enter=改行 / ⌘Enter=送信 / IME 確定 Enter は送信にしない）
                    .child(div().h(px(COMPOSER_INPUT_HEIGHT)).child(self.composer.clone()))
                    // Zed 風の下部コントロール列: エージェント / 権限モード / モデル / effort
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .pt(px(6.))
                            .child(self.render_selector_pill(Selector::Agent, cx))
                            .child(self.render_selector_pill(Selector::Mode, cx))
                            .child(self.render_selector_pill(Selector::Model, cx))
                            .child(self.render_selector_pill(Selector::Effort, cx)),
                    )
                    // 送信行: [Enter 挙動トグル] … [送信ボタン（現ヒント付き）]
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pt(px(6.))
                            // Enter の挙動トグル（日本語 IME の誤送信対策。設定に永続化）。
                            .child(
                                div()
                                    .id("enter-toggle")
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .px(px(7.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .text_size(px(10.))
                                    .text_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg3))
                                    .child(if self.submit_on_enter {
                                        "⏎ 送信 / ⇧⏎ 改行"
                                    } else {
                                        "⏎ 改行 / ⌘⏎ 送信"
                                    })
                                    .tooltip(Tooltip::text(
                                        "Enter の挙動を切替（送信 ⇄ 改行）。日本語 IME の変換確定では送信しません",
                                        theme.clone(),
                                    ))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.toggle_submit_on_enter(cx)
                                        }),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                // Zed 流の即時 hover（可逆トランジションはしない＝キビキビ・idle 0%）。
                                div()
                                    .id("send-button")
                                    .px(px(12.))
                                    .py(px(3.))
                                    .rounded(px(6.))
                                    .bg(color)
                                    .cursor_pointer()
                                    .hover(|style| style.opacity(0.85))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.bg0)
                                    .child(if self.submit_on_enter { "送信 ⏎" } else { "送信 ⌘⏎" })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| this.submit(cx)),
                                    ),
                            ),
                    ),
            )
    }
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.composer.read(cx).focus_handle(cx)
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        // ウィンドウがアクティブ（このアプリを見ている）時だけマスコットをアニメさせる。
        // 非アクティブへ切替時は GPUI が on_active_status_change→refresh で再描画するので自動で静止に切替わる。
        let active = window.is_window_active();
        div()
            .relative() // モデルメニューの絶対配置の基準
            .flex()
            .flex_col()
            .size_full() // 幅は workspace の可変ドックコンテナが決める
            .bg(theme.bg0)
            .text_color(theme.fg0)
            // agent フォーカス時のキー割当（keymap の "AgentPanel" context に一致）。⌘W=スレッド閉じ。
            .key_context("AgentPanel")
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_close_thread))
            .child(self.render_thread_tabs(cx))
            .child(self.render_meta(active))
            .child(self.render_transcript())
            // 承認待ちの権限リクエスト（あれば composer の直上に常時表示）。
            .children(self.render_permission_card(cx))
            .child(self.render_composer(cx))
            // セレクタのドロップダウンは各ピルの子として描く（render_selector_pill 内）。
            .when(self.context_menu_open, |element| {
                element.child(self.render_context_menu(cx))
            })
    }
}

// ── 自由関数 ──

/// マスコットの状態（スレッド状態から算出し [`render_mascot`] へ渡す）。
#[derive(Clone, Copy, PartialEq)]
enum MascotMotion {
    /// アイドル（待機）＝寝落ちの**静止**1枚。アニメしない＝idle CPU 0% を保つ。
    Idle,
    /// 生成中＝打鍵ループ。
    Typing,
    /// 思考中（末尾が Thinking ブロック）＝考えるループ。
    Think,
    /// 直近の成功直後＝バンザイ（数秒だけ再生して Idle へ戻る）。
    Celebrate,
}

/// ローディング・マスコット（猫耳コーダー娘）。生成中/祝福中はフィルムストリップを `steps()` 再生
/// （overflow-hidden 枠を横スクロール）、アイドルは静止。アニメは実行/祝福中のみ＝idle CPU 0% を保つ
/// （[`pulsing_dot`] と同じ設計）。
///
/// フレームは **image→video（Kling v3.0・start=end でループ）でキャラ固定のまま生成 → 共通窓で切り出し
/// → 量子化** した「本物の中割り」なので、text→image を並べた時のような軸ブレ（シェイク）が無い。
fn render_mascot(motion: MascotMotion, active: bool) -> gpui::AnyElement {
    const N: usize = 15; // 各ストリップのコマ数
    const H: f32 = 64.0; // 表示高さ
    const W: f32 = H * 60.0 / 72.0; // 1 コマのアスペクト（共通 60x72）
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mascot");
    let window = |child: gpui::AnyElement| {
        div().w(px(W)).h(px(H)).flex_none().overflow_hidden().child(child).into_any_element()
    };
    // 単一フレーム画像（idle.png）を等倍表示。
    let single = |file: &str| window(img(PathBuf::from(format!("{dir}/{file}"))).h(px(H)).w(px(W)).into_any_element());
    // ストリップの先頭コマだけ静止表示（overflow 窓＝ml 0）。非アクティブ時の凍結用＝再描画ゼロ。
    let frame0 =
        |file: &str| window(img(PathBuf::from(format!("{dir}/{file}"))).h(px(H)).w(px(W * N as f32)).into_any_element());
    // ストリップを steps 再生（横スクロール）。
    let anim = |file: &str, id: &'static str| {
        window(
            img(PathBuf::from(format!("{dir}/{file}")))
                .h(px(H))
                .w(px(W * N as f32))
                .with_animation(
                    id,
                    Animation::new(std::time::Duration::from_millis(100 * N as u64)).repeat(),
                    |element, delta| {
                        let index = ((delta * N as f32).floor() as usize).min(N - 1);
                        element.ml(px(-(W * index as f32)))
                    },
                )
                .into_any_element(),
        )
    };
    // 非アクティブ（このウィンドウを見ていない）時は先頭コマで静止＝アニメ再描画ゼロ＝idle 0%。
    match (motion, active) {
        (MascotMotion::Idle, true) => anim("doze-strip.png", "mascot-doze"),
        (MascotMotion::Idle, false) => single("idle.png"),
        (MascotMotion::Typing, true) => anim("typing-strip.png", "mascot-typing"),
        (MascotMotion::Typing, false) => frame0("typing-strip.png"),
        (MascotMotion::Think, true) => anim("think-strip.png", "mascot-think"),
        (MascotMotion::Think, false) => frame0("think-strip.png"),
        (MascotMotion::Celebrate, true) => anim("celebrate-strip.png", "mascot-celebrate"),
        (MascotMotion::Celebrate, false) => frame0("celebrate-strip.png"),
    }
}

/// スレッド色のドット。実行中は breathing で pulse（mock: 1.6s）。停止中は静止。
fn pulsing_dot(
    id: impl Into<gpui::ElementId>,
    diameter: f32,
    color: Hsla,
    running: bool,
) -> gpui::AnyElement {
    let dot = div()
        .size(px(diameter))
        .rounded(px(diameter / 2.0))
        .flex_none()
        .bg(color);
    if running {
        dot.with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(1600))
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            |element, delta| element.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

/// 指定カテゴリの広告設定の選択肢名一覧（無ければ `None`＝静的既定にフォールバック）。
fn config_choice_names(thread: &Thread, category: ConfigCategory) -> Option<Vec<SharedString>> {
    let config = thread.configs.iter().find(|config| config.category == category)?;
    if config.choices.is_empty() {
        return None;
    }
    Some(config.choices.iter().map(|(_, name)| SharedString::from(name.clone())).collect())
}

/// 選んだ表示名から value_id を引いて `session/set_config_option` を送る（広告設定 + セッションがある時のみ）。
/// 広告が無ければ何もしない（＝ラベル切替のみで実反映はできない）。
fn send_set_config(thread: &Thread, category: ConfigCategory, value_name: &SharedString) {
    let Some(config) = thread.configs.iter().find(|config| config.category == category) else {
        return;
    };
    let Some((value_id, _)) =
        config.choices.iter().find(|(_, name)| name.as_str() == value_name.as_ref())
    else {
        return;
    };
    if let Some(command_tx) = &thread.command_tx {
        command_tx
            .unbounded_send(SessionCommand::SetConfig {
                config_id: config.config_id.clone(),
                value_id: value_id.clone(),
            })
            .ok();
    }
}

/// 権限リクエストの 1 選択肢ボタン。種類でスタイルを変える（許可=スレッド色 / 拒否=中立）。
/// クリックで [`AgentPanel::answer_permission`] に添字を渡す。
fn permission_button(
    index: usize,
    option: &PermissionChoice,
    color: Hsla,
    theme: &Theme,
    cx: &mut Context<AgentPanel>,
) -> gpui::AnyElement {
    let base = div()
        .id(("permission-option", index))
        .flex()
        .items_center()
        .px(px(11.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(SharedString::from(option.label.clone()))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| this.answer_permission(index, cx)),
        );
    // 許可系はスレッド色で前に出す。拒否/未知は中立（罫線）でホバー時に意味色。
    match option.kind {
        PermissionKind::Allow => base
            .bg(color)
            .text_color(theme.bg0)
            .hover(|style| style.opacity(0.85))
            .into_any_element(),
        PermissionKind::AllowAlways => base
            .border_1()
            .border_color(color.alpha(0.7))
            .text_color(color)
            .hover(|style| style.bg(theme.bg3))
            .into_any_element(),
        PermissionKind::Reject | PermissionKind::RejectAlways => base
            .border_1()
            .border_color(theme.border)
            .text_color(theme.fg1)
            .hover(|style| style.text_color(theme.err).border_color(theme.err))
            .into_any_element(),
        PermissionKind::Other => base
            .border_1()
            .border_color(theme.border)
            .text_color(theme.fg1)
            .hover(|style| style.text_color(theme.fg0))
            .into_any_element(),
    }
}

/// 差分の色調（追加=ok / 削除=err / 文脈=fg2。テキスト色のみ・面塗りはしない＝UI-SPEC の色規律）。
#[derive(Clone, Copy)]
enum DiffTone {
    Context,
    Removed,
    Added,
}

/// 編集差分 1 件を表示する（ファイルパス + コンパクトな行差分）。mono。
fn render_diff(diff: &PermissionDiff, theme: &Theme) -> impl IntoElement {
    let lines = compact_line_diff(diff.old_text.as_deref(), &diff.new_text);
    let body = div()
        .flex()
        .flex_col()
        .rounded(px(6.))
        .bg(theme.bg1)
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(
            div()
                .px(px(9.))
                .py(px(4.))
                .border_b_1()
                .border_color(theme.border)
                .font_family("Guguru Sans Code")
                .text_size(px(10.5))
                .text_color(theme.fg1)
                .child(SharedString::from(diff.path.clone())),
        );
    let mut rows = div()
        .flex()
        .flex_col()
        .px(px(9.))
        .py(px(5.))
        .font_family("Guguru Sans Code")
        .text_size(px(11.));
    for (tone, text) in lines {
        let (prefix, tone_color) = match tone {
            DiffTone::Context => (" ", theme.fg2),
            DiffTone::Removed => ("−", theme.err),
            DiffTone::Added => ("+", theme.ok),
        };
        rows = rows.child(div().text_color(tone_color).child(format!("{prefix} {text}")));
    }
    body.child(rows)
}

/// 変更前後の全文から共通の前後を刈り取ってコンパクトな差分行を作る（プレビュー用）。
/// 単一領域の編集（Claude の典型）に十分。複数領域は中間を 1 ブロックにまとめる近似。
fn compact_line_diff(old: Option<&str>, new: &str) -> Vec<(DiffTone, String)> {
    const CONTEXT: usize = 2;
    const MAX_SIDE: usize = 14; // removed/added 各最大行
    let new_lines: Vec<&str> = new.lines().collect();
    let Some(old) = old else {
        // 新規ファイル: 全部 Added（cap）。
        return cap_lines(&new_lines, DiffTone::Added, MAX_SIDE);
    };
    let old_lines: Vec<&str> = old.lines().collect();

    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len().saturating_sub(prefix)
        && suffix < new_lines.len().saturating_sub(prefix)
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = &old_lines[prefix..old_lines.len() - suffix];
    let added = &new_lines[prefix..new_lines.len() - suffix];

    let mut out = Vec::new();
    // 先行コンテキスト（prefix 末尾の CONTEXT 行）。
    for line in &old_lines[prefix.saturating_sub(CONTEXT)..prefix] {
        out.push((DiffTone::Context, line.to_string()));
    }
    out.extend(cap_lines(removed, DiffTone::Removed, MAX_SIDE));
    out.extend(cap_lines(added, DiffTone::Added, MAX_SIDE));
    // 後続コンテキスト（suffix 先頭の CONTEXT 行）。
    for line in old_lines[old_lines.len() - suffix..].iter().take(CONTEXT) {
        out.push((DiffTone::Context, line.to_string()));
    }
    out
}

/// 行群を tone 付きで最大 `max` 行まで。超過分は「… （他 N 行）」でまとめる。
fn cap_lines(lines: &[&str], tone: DiffTone, max: usize) -> Vec<(DiffTone, String)> {
    let mut out: Vec<(DiffTone, String)> = lines
        .iter()
        .take(max)
        .map(|line| (tone, line.to_string()))
        .collect();
    if lines.len() > max {
        out.push((tone, format!("… （他 {} 行）", lines.len() - max)));
    }
    out
}

/// ステップの結果行（`⎿ …`。複数行は続く行をインデント）。mono・fg2。
fn render_result(text: &str, color: Hsla) -> impl IntoElement {
    let mut lines = div()
        .flex()
        .flex_col()
        .pt(px(3.))
        .font_family("Guguru Sans Code")
        .text_size(px(11.))
        .text_color(color);
    for (index, line) in text.lines().enumerate() {
        let prefix = if index == 0 { "⎿ " } else { "   " };
        lines = lines.child(div().child(format!("{prefix}{line}")));
    }
    lines
}

/// トークン数を人間可読に（`23400` → `23.4k` / `200000` → `200k`）。
fn human_tokens(count: u32) -> String {
    if count < 1000 {
        return count.to_string();
    }
    let thousands = count as f32 / 1000.0;
    if (thousands.fract()).abs() < 0.05 {
        format!("{}k", thousands.round() as u32)
    } else {
        format!("{thousands:.1}k")
    }
}

/// 初期スレッド群。先頭は mock v0.3 の会話例を種として持つ（ACP 配線までのプレースホルダ）。
fn seed_threads() -> Vec<Thread> {
    let rope = Thread {
        name: "rope設計".into(),
        color: thread_color(0),
        running: false,
        turn_started_at: None,
        model: "claude-fable-5".into(),
        permission_mode: "default".into(),
        effort: "high".into(),
        agent: "Claude Code".into(),
        context: Vec::new(),
        tokens_used: 23_400,
        tokens_max: 200_000,
        tokens_shown: 23_400.0,
        command_tx: None,
        available_modes: Vec::new(),
        current_mode_id: SharedString::default(),
        configs: Vec::new(),
        pending_permission: None,
        entries: vec![
            Entry::User("MVPのバッファ、ropey と Zed の sum-tree どっちに寄せるべき？".into()),
            Entry::Thinking(
                "Zed の text crate は sum-tree ベースで CRDT 前提の設計。協調編集を MVP で切るなら過剰。\
                 ropey は API が安定していて docs も厚い。undo 履歴は rope と独立に持てるので移行コストも低い…"
                    .into(),
            ),
            Entry::Step {
                tool: "Read".into(),
                args: "(zed/crates/text/src/text.rs)".into(),
                result: Some("1,842 行 — SumTree<Chunk> / anchor / clock::Global を確認".into()),
            },
            Entry::Step {
                tool: "Update Todos".into(),
                args: "".into(),
                result: Some("☒ text crate の設計を調査\n☐ Buffer trait の切り方を決める".into()),
            },
            Entry::Agent(
                "結論: MVP は ropey。Zed の text は CRDT（協調編集）前提の sum-tree で、単独編集の \
                 MVP には複雑さが釣り合いません。Buffer を trait で切っておけば、後から sum-tree 系へ \
                 差し替える道も残せます。"
                    .into(),
            ),
        ],
    };
    vec![rope, Thread::empty("tab色分け", 1), Thread::empty("gpui起動", 2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(diff: &[(DiffTone, String)]) -> String {
        diff.iter()
            .map(|(tone, text)| {
                let mark = match tone {
                    DiffTone::Context => " ",
                    DiffTone::Removed => "-",
                    DiffTone::Added => "+",
                };
                format!("{mark}{text}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn diff_trims_common_prefix_and_suffix() {
        // 中央 1 行だけの変更 → 周辺は context、中央だけ -/+。
        let old = "a\nb\nc\nd\ne";
        let new = "a\nb\nX\nd\ne";
        let result = tones(&compact_line_diff(Some(old), new));
        assert_eq!(result, " a\n b\n-c\n+X\n d\n e");
    }

    #[test]
    fn diff_new_file_is_all_added() {
        let result = compact_line_diff(None, "l1\nl2");
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|(tone, _)| matches!(tone, DiffTone::Added)));
    }

    #[test]
    fn diff_caps_large_blocks_with_summary() {
        // 30 行の追加 → 14 行 + 「他 N 行」の要約 1 行。
        let new: String = (0..30).map(|i| format!("line{i}\n")).collect();
        let result = compact_line_diff(Some(""), &new);
        assert_eq!(result.len(), 15); // 14 + 要約
        assert!(result.last().unwrap().1.contains("他 16 行"));
    }

    #[test]
    fn human_tokens_formats_thousands() {
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(23_400), "23.4k");
        assert_eq!(human_tokens(200_000), "200k");
    }
}
