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
    AgentEvent, AgentKind, ConfigCategory, ConfigOption, PermissionChoice, PermissionDiff,
    PermissionKind, PlanItem, PlanStatus, SessionCommand, TurnEnd,
};
// 管制（P3）が許可ボタンの種類を見分けるための再輸出（workspace は acp_client を直接知らない）。
pub use acp_client::PermissionKind as AgentPermissionKind;

/// 既定 Agent の oneshot テンプレート（workspace の編隊総括・P4 が使う橋渡し。
/// workspace は acp_client を直接知らない）。
pub fn oneshot_template(label: &str) -> Option<&'static str> {
    acp_client::AgentKind::by_label(label).and_then(|kind| kind.oneshot())
}
use editor_view::{ComposerEvent, EditorView};
use futures::channel::mpsc;
use futures::StreamExt;
use gpui::{
    actions, div, img, prelude::*, pulsating_between, px, svg, Animation, AnimationExt, App,
    ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, Focusable, FontWeight,
    HighlightStyle, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollHandle, SharedString, StyledText, TextLayout, Window,
};
use host::{Host, LocalHost};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use theme_core::{claude_bullet, thread_color, Theme};
use ui::{DraggedFile, Tooltip};

mod markdown;

actions!(agent, [SubmitPrompt, CloseActiveThread]);

/// ドラッグ中のスレッドタブのゴースト（Chrome 風の並べ替え用。ポインタに追従する小チップ）。
#[derive(Clone)]
struct DraggedThreadTab {
    index: usize,
    name: SharedString,
    color: Hsla,
    theme: Theme,
}

impl Render for DraggedThreadTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(self.theme.bg2)
            .border_1()
            .border_color(self.color)
            .text_size(px(12.))
            .text_color(self.theme.fg0)
            .child(self.name.clone())
    }
}
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

/// スレッドの見せ方（explorer の Tree/Columns/Icons と同じ「登録式ビュー」。Bar と List は排他）。
/// Bar = 色付き横タブ（既定）。List = 縦リスト（「チャットのスペース履歴」風・多数でも一望）。
/// プロジェクト⎇ブランチで束ねる Grouped は後続。
#[derive(Clone, Copy, PartialEq)]
enum AgentTabsView {
    Bar,
    List,
}

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
    /// checkpoint（この時点へ戻せる・M12-2）。承認直前の変更前内容が blob に入っている。
    Checkpoint { id: i64, label: SharedString },
}

/// transcript の選択可能リージョン（毎フレーム再構築・M13）。`layout` は同フレームの
/// paint 後に有効になる（transcript は全リージョン描画 = 登録したものは必ず paint される）。
/// リージョンの同一性は**登録順のインデックス**（= このベクタ内の位置）。1 エントリが markdown で
/// 複数ブロック（＝複数リージョン）に割れても選択が破綻しないよう、エントリ index ではなく
/// リージョン index を選択の基準にする。
struct SelectableRegion {
    /// 表示テキストそのもの（コピーはここから切る。offset はこのテキスト基準）。
    text: SharedString,
    layout: TextLayout,
}

/// transcript ドラッグ選択の 1 点（リージョン index, リージョン内 byte）。タプル順で比較。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct TranscriptPoint {
    region: usize,
    offset: usize,
}

/// transcript のドラッグ選択（エントリ跨ぎ・M13）。GPUI に選択プリミティブが無いため自前:
/// StyledText の `TextLayout::index_for_position` でヒットテストし、選択は highlight 背景で描く。
struct TranscriptSelection {
    start: TranscriptPoint,
    end: TranscriptPoint,
    selecting: bool,
}

impl TranscriptSelection {
    fn normalized(&self) -> (TranscriptPoint, TranscriptPoint) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

/// 承認待ちの権限リクエスト（`session/request_permission` を UI で保持する間の状態）。
/// `respond` に選んだ選択肢の添字を送ると acp_client が応答する（このスレッドの当該ターンは
/// それまでブロックしている）。ユーザーが答えるまで composer 上部にカードを出す。
struct PendingPermission {
    title: SharedString,
    diffs: Vec<PermissionDiff>,
    options: Vec<PermissionChoice>,
    respond: mpsc::UnboundedSender<usize>,
    /// 承認待ちに入った時刻。マスコットの段階演出に使う（まず祈る→長引くと頬に手であわあわ）。
    since: std::time::Instant,
}

/// workspace への通知イベント（トースト・色リンク・statusbar ドット・M12-4/5）。
#[derive(Debug, Clone)]
pub enum PanelEvent {
    /// prompt を ACP session へ送る直前。Fleet Task lifecycle の `working` 起点。
    TurnStarted { thread: SharedString, color: Hsla },
    /// ターン完了（summary = 触ったファイル数・経過秒 / digest = 最後の発言の末尾・P1）。
    TurnEnded {
        thread: SharedString,
        color: Hsla,
        summary: SharedString,
        digest: Option<SharedString>,
        /// エージェント別ミュート中（P2）: workspace はトーストを出さない（ニュースには載せる）。
        muted: bool,
    },
    TurnFailed { thread: SharedString, color: Hsla, message: SharedString, muted: bool },
    /// 権限待ちで停止中（裏の窓でも気づけるように）。`title` = 何の許可か（digest 素材・P1）。
    PermissionWaiting { thread: SharedString, color: Hsla, title: SharedString, muted: bool },
    /// Tier 2 の ✳ 1 行要約が生成できた（P4）。workspace が task_events へキャッシュし
    /// 管制の総括デバウンスを蹴る。**状態は運ばない**（文だけ・状態は事実層が別で流れている）。
    SummaryReady { thread: SharedString, tier2: SharedString },
    /// 初回ターン後に AI がスレッド名を付けた（#6）。Task 名がプレースホルダのままなら
    /// workspace が Task 名にも引き継ぐ（2026-07-24・タスク命名）。
    ThreadAutoNamed { name: SharedString },
    /// エージェントの編集を承認した（色リンク用・M12-4）。
    FilesTouched {
        files: Vec<std::path::PathBuf>,
        color: Hsla,
    },
    /// 承認カードの「エディタで開く」（diff レビュー本体化・M12-6）。
    OpenDiffRequest {
        title: String,
        old_text: String,
        new_text: String,
    },
    /// スレッド履歴を開く（ヘッダの履歴ボタン・#5。window が要るので workspace が pending で消化）。
    OpenHistoryRequest,
}

/// エージェントスレッドの状態（herdr の 5 状態を Shirushi 流にマップ・#）。**色相は状態に使わない**
/// （UI-SPEC §1.3「色＝識別」）。ドット/beacon は常にスレッド識別色で、状態は形と動き（脈動/リング/グリフ）で見せる。
/// ロールアップ（フッター・レール・⌘O）はこれを最重要（Blocked>Working>Done>Idle）で畳む。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadActivity {
    /// 待機（session あり・ターン無し）。
    Idle,
    /// 実行中（ストリーミング中）。
    Working,
    /// 承認/入力待ちで停止中（ACP がターンを実際にブロック）。
    Blocked,
    /// 直近ターン完了・未確認（見るまで残るラッチ）。`interrupted`＝拒否/キャンセルで終わった。
    Done { interrupted: bool },
}

impl ThreadActivity {
    /// ロールアップの優先度（大きいほど注意を要する）。Blocked > Working > Done > Idle。
    pub fn urgency(self) -> u8 {
        match self {
            ThreadActivity::Idle => 0,
            ThreadActivity::Done { .. } => 1,
            ThreadActivity::Working => 2,
            ThreadActivity::Blocked => 3,
        }
    }

    /// ambient に出す価値がある状態か（Idle は各所に出さない＝静かに保つ）。
    pub fn is_signal(self) -> bool {
        self.urgency() > 0
    }
}

/// 全ウィンドウ横断の「スレッド状態」台帳: worktree root → (スレッド名, 色, 状態)。
/// ⌘O ダッシュボード・フッターのロールアップ・レールのドットが「どこで何が起きているか」を読む（M12-12・#）。
/// 各窓の AgentPanel が自分の root のエントリを上書きする（書き手は窓ごとに排他）。
#[derive(Default)]
pub struct RunningRegistry(
    pub HashMap<std::path::PathBuf, Vec<(SharedString, Hsla, ThreadActivity)>>,
);

impl gpui::Global for RunningRegistry {}

/// herd サイドバー（状態一覧・M14）の 1 スレッド行。`beacons()` のリッチ版で、エージェント種別と
/// トークンも返す。**色相は識別（`color`）、状態は `activity`（形と動き）**で見せる（UI-SPEC §1.3/§11）。
pub struct AgentStatus {
    pub name: SharedString,
    pub color: Hsla,
    pub activity: ThreadActivity,
    /// 話す ACP エージェントのラベル（`agent_badge` でアイコンを引く。既定 "Claude Code"）。
    pub agent: SharedString,
    pub tokens_used: u32,
    pub tokens_max: u32,
    /// スレッドの開始時刻（unix ms）。「いつスタートしたか」を herd 行/編隊セルに出す（M14）。
    pub created_at_ms: i64,
    /// 最後にユーザーが入力した時刻（unix ms・まだ入力が無ければ None）。
    pub last_input_at_ms: Option<i64>,
    /// 遷移スナップショット（Tier 1・決定論・FLEET-CONTROL-PLAN P1）。
    /// Blocked=許可待ちの内容 / Done=最後の発言の末尾 / Failed=エラー文 /
    /// Working=ライブ素材（最新ツール + plan の進行中項目・保存せずその場で合成）。
    pub digest: Option<SharedString>,
    /// plan の完了数 / 総数（plan が無ければ 0/0）。
    pub plan_done: u32,
    pub plan_total: u32,
    /// このスレッドが（承認済み編集で）触ったファイル数（diff stat の代替・軽量）。
    pub files_touched: u32,
    /// 現ターンの経過秒（実行中でなければ None）。
    pub turn_elapsed_secs: Option<u64>,
    /// エージェント別ミュート中か（P2・トースト/完了音を抑止。表示は 🔕）。
    pub muted: bool,
    /// Tier 2 の ✳ 1 行要約（P4・LLM 生成の印＝表示側は ✳ テラコッタを付ける）。
    pub tier2: Option<SharedString>,
}

/// 管制の要対応キュー（P3）が承認カードを組むための素材（`permission_card()` が返す）。
pub struct PermissionCard {
    pub title: SharedString,
    /// `(respond へ返す添字, 種別, 表示ラベル)`。
    pub options: Vec<(usize, PermissionKind, SharedString)>,
    pub waited_secs: u64,
    /// 添付 diff のファイル数（「N files」表示用）。
    pub diff_files: usize,
}

/// 1 スレッド = 1 会話。固有色を持ち、UI 全体へ貫通する。
struct Thread {
    /// 永続化キー（unix ms + 連番。再起動を跨いで同一・M12-1）。
    id: String,
    name: SharedString,
    color: Hsla,
    running: bool,
    /// 直近ターンが完了して**まだ確認していない**時の終わり方（`Some`＝herdr の Done ラッチ）。
    /// `TurnEnded` で立て、そのスレッドを見た（`switch_thread`）か次のターンを始めた時にクリアする。
    done: Option<TurnEnd>,
    /// このターンの開始時刻（経過秒の表示に使う。`None`＝未実行）。
    turn_started_at: Option<std::time::Instant>,
    /// スレッドの開始時刻（unix ms・再起動を跨いで DB の created_at と往復する・M14）。
    created_at_ms: i64,
    /// 最後にユーザーが入力した時刻（unix ms・送信のたびに更新。復元時は最後の user turn から）。
    last_input_at_ms: Option<i64>,
    model: SharedString,
    permission_mode: SharedString,
    effort: SharedString,
    /// このスレッドが話す ACP エージェント（ラベル。既定 "Claude Code"＝`AgentKind` の label と一致）。
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
    /// 永続化済みの entries 数（TurnEnded 時にここから先を DB へ追記・M12-1）。
    persisted_entries: usize,
    /// このスレッドが（承認済み編集で）触ったファイル（色リンク・M12-4）。
    touched_files: Vec<std::path::PathBuf>,
    /// エージェントの実行プラン（`SessionUpdate::Plan` の最新全量・M12-9）。
    /// transcript 上部の常設チェックリストに出す。空 = 非表示。
    plan: Vec<PlanItem>,
    /// ユーザーが手動でタブ名を変更したか（true なら AI 自動命名で上書きしない・#4/#6）。
    name_is_custom: bool,
    /// 遷移スナップショット（Tier 1・FLEET-CONTROL-PLAN P1）。**状態遷移時のみ**更新する:
    /// PermissionRequest→許可待ちの内容 / TurnEnded→最終発言の末尾 / Failed→エラー文。
    /// Working 中は保存せず `live_digest()` を流す（生成不要・無料）。
    digest: Option<SharedString>,
    /// エージェント別ミュート（P2）: true の間このスレッドのトースト/完了音を出さない。
    /// ニュースフィードには**載る**（見えるが鳴らない・herd 行の 🔕 で切替）。
    muted: bool,
    /// Tier 2 遷移スナップショット（✳ 1 行要約・P4）: Done/Failed 遷移の数秒後に oneshot が埋める。
    /// **状態を上書きしない**（状態と数字は事実層・これは添える文）。新しいターン開始でクリア。
    tier2: Option<SharedString>,
}

impl Thread {
    /// 現在の状態（herdr の 5 状態マップ・#）。Blocked（承認待ち）は Working（実行中）より優先で、
    /// これまで `running: bool` に埋もれていた「待ち」を分離する。Done は未確認ラッチ。
    fn activity(&self) -> ThreadActivity {
        if self.pending_permission.is_some() {
            ThreadActivity::Blocked
        } else if self.running {
            ThreadActivity::Working
        } else if let Some(reason) = self.done {
            ThreadActivity::Done {
                interrupted: reason == TurnEnd::Interrupted,
            }
        } else {
            ThreadActivity::Idle
        }
    }

    fn empty(name: impl Into<SharedString>, index: usize) -> Thread {
        Thread {
            id: new_thread_id(),
            name: name.into(),
            color: thread_color(index),
            running: false,
            done: None,
            turn_started_at: None,
            created_at_ms: now_unix_ms(),
            last_input_at_ms: None,
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
            persisted_entries: 0,
            touched_files: Vec::new(),
            plan: Vec::new(),
            name_is_custom: false,
            digest: None,
            muted: false,
            tier2: None,
        }
    }

    /// Working 中のライブ素材（Tier 1 の無料版・P1）: 実行中ツールの説明 + plan の進行中項目。
    /// 保存しない（描画のたびに最新を合成＝「今なにをしているか」が常に生）。
    fn live_digest(&self) -> Option<SharedString> {
        let tool = self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Step { tool, .. } => Some(tool.clone()),
            _ => None,
        });
        let step = self
            .plan
            .iter()
            .find(|item| item.status == PlanStatus::InProgress)
            .map(|item| item.content.clone());
        match (tool, step) {
            (Some(tool), Some(step)) => Some(SharedString::from(format!("{tool} · {step}"))),
            (Some(tool), None) => Some(tool),
            (None, Some(step)) => Some(SharedString::from(step)),
            (None, None) => None,
        }
    }

    /// plan の (完了数, 総数)。plan なし = (0, 0)。
    fn plan_progress(&self) -> (u32, u32) {
        let total = self.plan.len() as u32;
        let done = self
            .plan
            .iter()
            .filter(|item| item.status == PlanStatus::Completed)
            .count() as u32;
        (done, total)
    }
}

/// 遷移スナップショットの末尾抽出（Tier 1・P1）: テキスト末尾の 1〜2 文を 1 行に畳む。
/// LLM を使わない決定論抽出 — 「最後に何と言って終わったか」をそのまま見せる。
pub fn digest_tail(text: &str) -> Option<SharedString> {
    const MAX_CHARS: usize = 140;
    let flat = text.trim();
    if flat.is_empty() {
        return None;
    }
    // 段落の最後を対象に（コードブロック等の途中で切らない・行単位が最小単位）。
    let last_block: &str = flat.rsplit("\n\n").next().unwrap_or(flat);
    // 文区切り（。．.!?！?）で末尾 2 文を拾う。区切りが無ければブロック全体。
    let mut sentences: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for (index, character) in last_block.char_indices() {
        if matches!(character, '。' | '．' | '!' | '？' | '！' | '?')
            || (character == '.'
                && last_block[index + character.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace))
        {
            let end = index + character.len_utf8();
            let sentence = last_block[start..end].trim();
            if !sentence.is_empty() {
                sentences.push(sentence);
            }
            start = end;
        }
    }
    let rest = last_block[start..].trim();
    if !rest.is_empty() {
        sentences.push(rest);
    }
    let tail_count = sentences.len().saturating_sub(2);
    let mut joined = sentences[tail_count..].join(" ");
    // 改行は 1 行表示に潰す（herd 行 / セルヘッダは単行）。
    joined = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() > MAX_CHARS {
        let tail: String = joined
            .chars()
            .skip(joined.chars().count() - MAX_CHARS)
            .collect();
        joined = format!("…{tail}");
    }
    Some(SharedString::from(joined))
}

/// エントリのコピー用プレーンテキスト（GPUI の素のテキストは選択ドラッグ不可のため、
/// hover の ⧉ でエントリ単位コピーを提供する・M13 UX。本文のドラッグ選択は残件）。
fn entry_plain_text(entry: &Entry) -> String {
    match entry {
        Entry::User(text) | Entry::Thinking(text) | Entry::Agent(text) => text.to_string(),
        Entry::Step { tool, args, result } => match result {
            Some(result) => format!("{tool} {args}\n{result}"),
            None => format!("{tool} {args}"),
        },
        Entry::Checkpoint { label, .. } => format!("checkpoint — {label}"),
    }
}

/// サブシーケンス一致（fuzzy・M12-7）。クエリの文字が順番に現れれば true。
fn fuzzy_matches(haystack: &str, query: &str) -> bool {
    let mut chars = haystack.chars();
    query.chars().all(|needle| chars.any(|c| c == needle))
}

/// 既定のプレースホルダ名か（空 or "スレッドN"）。AI 自動命名の対象判定（#6）。
/// seed 名（"rope設計" 等）や既に付いた名前は対象外＝上書きしない。
fn is_placeholder_name(name: &str) -> bool {
    let name = name.trim();
    name.is_empty()
        || name
            .strip_prefix("スレッド")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

/// 現在時刻（unix ms）。スレッドの開始/最終入力時刻の記録に使う。
pub fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// unix ms を「たった今 / N分前 / N時間前 / N日前」へ整形する（herd 行・編隊セル・履歴 Picker・M14）。
/// 相対表記なのはタイムゾーン変換の依存を増やさず「サクッと見る」に十分なため。0 以下は「—」。
pub fn relative_time_label(unix_ms: i64) -> SharedString {
    let elapsed_ms = now_unix_ms().saturating_sub(unix_ms);
    if unix_ms <= 0 {
        return SharedString::from("—");
    }
    let minutes = elapsed_ms / 60_000;
    let label = if minutes < 1 {
        i18n::t!("time.just_now")
    } else if minutes < 60 {
        i18n::t!("time.minutes_ago", "count" => minutes)
    } else if minutes < 60 * 24 {
        i18n::t!("time.hours_ago", "count" => minutes / 60)
    } else {
        i18n::t!("time.days_ago", "count" => minutes / (60 * 24))
    };
    SharedString::from(label)
}

/// スレッド id（unix ms + プロセス内連番）。
fn new_thread_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!("{}-{}", now_unix_ms(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// エージェントパネル本体（右ドックまるごとを描く）。
pub struct AgentPanel {
    threads: Vec<Thread>,
    active: usize,
    // ⌘W で閉じたスレッド（⌘⇧T で復元。会話履歴ごと戻す。新しいものが末尾）。
    closed_threads: Vec<Thread>,
    theme: Theme,
    /// 宛先（アクティブプロジェクトのコンテキスト）。workspace が [`Self::set_destination`] で更新する。
    dest_project: SharedString,
    dest_branch: Option<SharedString>,
    /// ACP エージェントの起動 cwd（アクティブプロジェクトのルート）。無ければ送信できない。
    dest_cwd: Option<PathBuf>,
    dest_host: Arc<dyn Host>,
    composer: Entity<EditorView>,
    /// タブ改名中の (対象 index, 入力欄)。ダブルクリックで開く・IME 正しい EditorView::plain（#4）。
    renaming: Option<(usize, Entity<EditorView>)>,
    /// transcript のスクロール（M13 UX: ホイールで遡れる。ストリーミング中は底に居る時だけ追従）。
    transcript_scroll: ScrollHandle,
    /// スレッドタブ列の横スクロール（タブが増えても潰さず左右へ送れる。縦ホイールも横送りにマップ）。
    tabs_scroll: ScrollHandle,
    /// スレッドの見せ方（Bar 横タブ / List 縦リスト。登録式・スイッチャで切替）。
    tabs_view: AgentTabsView,
    /// List ビューの縦スクロール（スレッドが多いときリスト内で送る）。
    thread_list_scroll: ScrollHandle,
    /// transcript の選択可能リージョン（render で clear→push・mouse イベントが読む）。
    transcript_regions: Rc<RefCell<Vec<SelectableRegion>>>,
    /// transcript のドラッグ選択（None = 選択なし）。⌘C でコピー・Esc/外クリックで解除。
    transcript_selection: Option<TranscriptSelection>,
    /// composer 下部の選択ピルのうち開いているメニュー（None = 閉）。
    open_menu: Option<Selector>,
    /// Add context の候補（プロジェクトのファイル相対パス。workspace が渡す）と開閉。
    context_files: Vec<SharedString>,
    context_menu_open: bool,
    /// ＋context の fuzzy 絞り込みクエリ（M12-7・Picker 化）。
    context_query: String,
    context_focus: Option<FocusHandle>,
    /// Enter 送信の現在値（送信ヒント表示 + トグルの状態。composer にも反映する）。
    submit_on_enter: bool,
    /// トークン表示のカウントアップ補間タスクが稼働中か（多重起動防止。追いついたら false に戻す＝idle 0%）。
    /// ローカル永続化 DB（M12-1。workspace が起動後に渡す。None = 永続化なしで動く）。
    storage: Option<storage::Storage>,
    /// Fleet では thread/AgentRun を TaskSpace に帰属させる stable ID。表示名や rail 順と分離する。
    storage_scope: Option<String>,
    token_ticker: bool,
    /// 直近の成功でマスコットがバンザイ中か（数秒で false に戻る）。世代番号で古いタイマーを無効化する。
    celebrating: bool,
    celebrate_gen: u32,
    /// ストリーミング平滑化: アクティブスレッドの末尾（Agent/Thinking）を何文字まで表示したか（タイプライタ）。
    /// チャンクは束で届くので、これを一定速度で目標長へ寄せてカクつきを消す。`usize::MAX`＝全部表示。
    reveal: usize,
    reveal_ticker: bool,
    /// 折り畳んだ Thinking のうちユーザーが開いたもの（thread.id, entry index）。実行中の最終ブロックは常に展開。
    expanded_thoughts: std::collections::HashSet<(String, usize)>,
    /// この panel の window がアクティブか（render で更新・GPUI が activation 変化で再描画するので追従する）。
    /// 完了音は**非アクティブ時のみ**鳴らす（見ている画面に音は要らない・P2）。
    window_active: bool,
}

impl gpui::EventEmitter<PanelEvent> for AgentPanel {}

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
                panel.composer.update(cx, |composer, cx| {
                    composer.set_submit_on_enter(submit_on_enter, cx)
                });
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
        // 開発用: SHIRUSHI_TRANSCRIPT_SEL_PROBE=1 で transcript 選択を注入（M13 の描画+コピー検証）。
        if std::env::var("SHIRUSHI_TRANSCRIPT_SEL_PROBE").is_ok_and(|value| value == "1") {
            cx.spawn(async move |panel, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1200))
                    .await;
                panel
                    .update(cx, |panel, cx| {
                        panel.transcript_selection = Some(TranscriptSelection {
                            start: TranscriptPoint {
                                region: 0,
                                offset: 3,
                            },
                            end: TranscriptPoint {
                                region: 4,
                                offset: 9,
                            },
                            selecting: false,
                        });
                        cx.notify();
                    })
                    .ok();
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(800))
                    .await;
                panel
                    .update(cx, |panel, _cx| match panel.transcript_selected_text() {
                        Some(text) => eprintln!("TRANSCRIPT_SEL({} bytes):\n{text}", text.len()),
                        None => eprintln!("TRANSCRIPT_SEL: none"),
                    })
                    .ok();
            })
            .detach();
        }
        // 開発用: SHIRUSHI_COMPOSER_PROBE="<text>" で composer に下書きを流し込む（折り返し等の描画検証）。
        if let Ok(text) = std::env::var("SHIRUSHI_COMPOSER_PROBE") {
            if !text.is_empty() {
                composer.update(cx, |composer, cx| composer.set_plain_text(&text, cx));
            }
        }
        // 開発用: SHIRUSHI_PLAN_PROBE=1 で実行プランを直接注入（M12-9 常設チェックリストの描画検証。
        // 実 ACP を起動せずに ●/☒/☐ の 3 状態が出ることをオフスクリーンで確かめる）。
        if std::env::var("SHIRUSHI_PLAN_PROBE").is_ok_and(|value| value == "1") {
            cx.spawn(async move |panel, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(400))
                    .await;
                panel
                    .update(cx, |panel, cx| {
                        let active = panel.active;
                        let items = vec![
                            PlanItem {
                                content: "text crate の設計を調査".into(),
                                status: PlanStatus::Completed,
                            },
                            PlanItem {
                                content: "Buffer trait の切り方を決める".into(),
                                status: PlanStatus::InProgress,
                            },
                            PlanItem {
                                content: "rope 移行の見積もりを出す".into(),
                                status: PlanStatus::Pending,
                            },
                        ];
                        panel.on_event(active, AgentEvent::Plan(items), cx);
                    })
                    .ok();
            })
            .detach();
        }
        // 既定エージェント（設定 or 前回選択）を種スレッドにも適用して「そのまま開く」を実現。
        let default_agent = default_agent_name(cx);
        let mut threads = seed_threads();
        for thread in &mut threads {
            thread.agent = default_agent.clone();
        }
        // 開発用: SHIRUSHI_TABS_PROBE=<n> でタブを n 枚まで水増しし、横スクロール（溢れ挙動）を検証する。
        if let Ok(count) = std::env::var("SHIRUSHI_TABS_PROBE") {
            if let Ok(count) = count.trim().parse::<usize>() {
                while threads.len() < count {
                    let index = threads.len();
                    let mut thread = Thread::empty(format!("スレッド{}", index + 1), index);
                    thread.agent = default_agent.clone();
                    threads.push(thread);
                }
            }
        }
        // 初期表示は末尾（最新）を見せる（スクロール化 M13 での回帰防止）。
        // 開発用: SHIRUSHI_SCROLL_TOP で先頭のまま（transcript 上部＝Thinking 等の目視撮影用）。
        let transcript_scroll = ScrollHandle::new();
        if std::env::var_os("SHIRUSHI_SCROLL_TOP").is_none() {
            transcript_scroll.scroll_to_bottom();
        }
        AgentPanel {
            threads,
            storage: None,
            storage_scope: None,
            closed_threads: Vec::new(),
            active: 0,
            theme,
            dest_project: "—".into(),
            dest_branch: None,
            dest_cwd: None,
            dest_host: LocalHost::shared(),
            composer,
            renaming: None,
            transcript_scroll,
            tabs_scroll: ScrollHandle::new(),
            tabs_view: initial_tabs_view(&settings::get(cx).agent_tabs_view),
            thread_list_scroll: ScrollHandle::new(),
            transcript_regions: Rc::new(RefCell::new(Vec::new())),
            transcript_selection: None,
            open_menu: None,
            context_files: Vec::new(),
            context_menu_open: false,
            context_query: String::new(),
            context_focus: None,
            submit_on_enter,
            token_ticker: false,
            celebrating: false,
            celebrate_gen: 0,
            reveal: usize::MAX, // 初期は全表示（seed を打ち直さない）
            reveal_ticker: false,
            expanded_thoughts: std::collections::HashSet::new(),
            window_active: true,
        }
    }

    /// 新しい TaskSpace 用。通常 View の初回デモ transcript を持ち込まず、実 AgentRun 1 本で始める。
    pub fn new_task(theme: Theme, cx: &mut Context<Self>) -> Self {
        // スレッド色は**パネル横断で巡回**（2026-07-24 色モデル: Task 色はリポジトリ継承になったため、
        // 各 Task の 1 本目が全部同じ色にならないようグローバルに回す）。
        static TASK_THREAD_COLOR_SEED: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let seed = TASK_THREAD_COLOR_SEED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut panel = Self::new(theme, cx);
        let mut thread = Thread::empty("Task", seed);
        thread.agent = default_agent_name(cx);
        panel.threads = vec![thread];
        panel.active = 0;
        panel
    }

    /// Enter 送信の有効/無効をトグルする。**設定 store（settings.json が真実）を更新するだけ**で、
    /// 実際の反映（composer 更新・再描画）は `observe_global` 経由で起きる（UI / CLI / MCP と同じ経路）。
    fn toggle_submit_on_enter(&mut self, cx: &mut Context<Self>) {
        let value = !self.submit_on_enter;
        settings::set_user_value(cx, "submit_on_enter", serde_json::Value::Bool(value));
    }

    /// 宛先チップに出すプロジェクト名・ブランチ・cwd を設定する（プロジェクト切替時に workspace が呼ぶ）。
    /// 永続化 DB を受け取る（workspace が起動後に呼ぶ・M12-1）。
    /// 過去のスレッドがあれば mock の種を置き換えて復元する（transcript は直近 200 turn）。
    pub fn set_storage(&mut self, storage: storage::Storage, cx: &mut Context<Self>) {
        self.set_storage_inner(storage, None, None, cx);
    }

    /// TaskSpace ごとに AgentRun を復元する。`legacy_project` は旧版が表示名を保存していた DB の
    /// 一方向移行用で、次回 persist から stable `scope` へ更新される。
    pub fn set_storage_for_scope(
        &mut self,
        storage: storage::Storage,
        scope: String,
        legacy_project: &str,
        cx: &mut Context<Self>,
    ) {
        self.set_storage_inner(storage, Some(scope), Some(legacy_project), cx);
    }

    fn set_storage_inner(
        &mut self,
        storage: storage::Storage,
        scope: Option<String>,
        legacy_project: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let restored = storage
            .load_threads()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| {
                scope.as_ref().is_none_or(|scope| {
                    row.3 == *scope || legacy_project.is_some_and(|legacy| row.3 == legacy)
                })
            })
            .collect::<Vec<_>>();
        if !restored.is_empty() {
            let mut threads = Vec::new();
            for (
                id,
                name,
                color_index,
                _project,
                _branch,
                model,
                tokens_used,
                tokens_limit,
                created_at_ms,
                last_input_at_ms,
            ) in restored
            {
                let mut thread = Thread::empty(name, color_index as usize);
                thread.id = id.clone();
                thread.created_at_ms = created_at_ms;
                thread.last_input_at_ms = last_input_at_ms;
                if let Some(model) = model {
                    thread.model = model.into();
                }
                thread.tokens_used = tokens_used.max(0) as u32;
                thread.tokens_shown = thread.tokens_used as f32;
                if tokens_limit > 0 {
                    thread.tokens_max = tokens_limit as u32;
                }
                let turns = storage.load_recent_turns(&id, 200).unwrap_or_default();
                thread.entries = turns
                    .into_iter()
                    .map(|(role, content)| match role.as_str() {
                        "user" => Entry::User(content.into()),
                        "thinking" => Entry::Thinking(content.into()),
                        "step" => Entry::Step {
                            tool: content.into(),
                            args: SharedString::default(),
                            result: None,
                        },
                        "checkpoint" => {
                            let (id, label) =
                                content.split_once('\t').unwrap_or(("0", "checkpoint"));
                            Entry::Checkpoint {
                                id: id.parse().unwrap_or(0),
                                label: label.to_string().into(),
                            }
                        }
                        _ => Entry::Agent(content.into()),
                    })
                    .collect();
                thread.persisted_entries = thread.entries.len();
                threads.push(thread);
            }
            self.threads = threads;
            self.active = 0;
        } else {
            // 初回起動: 現在の（種）スレッドをメタだけ登録（mock 会話の本文は書かない）。
            for (index, thread) in self.threads.iter_mut().enumerate() {
                thread.persisted_entries = thread.entries.len();
                let _ = storage.upsert_thread(
                    &thread.id,
                    thread.name.as_ref(),
                    index as i64,
                    scope.as_deref().unwrap_or(""),
                    None,
                    Some(thread.model.as_ref()),
                    thread.tokens_used as i64,
                    thread.tokens_max as i64,
                );
            }
        }
        self.storage = Some(storage);
        self.storage_scope = scope;
        if std::env::var_os("SHIRUSHI_SCROLL_TOP").is_none() {
            self.transcript_scroll.scroll_to_bottom(); // 復元直後も末尾（最新）を見せる
        }
        cx.notify();
    }

    /// 未永続の entries を DB へ追記し、スレッドのメタを upsert する（TurnEnded / 送信時・M12-1）。
    fn persist_thread(&mut self, thread_index: usize) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        let project = self
            .storage_scope
            .clone()
            .unwrap_or_else(|| self.dest_project.to_string());
        let branch = self.dest_branch.clone();
        let Some(thread) = self.threads.get_mut(thread_index) else {
            return;
        };
        for entry in thread.entries.iter().skip(thread.persisted_entries) {
            let (role, content) = match entry {
                Entry::User(text) => ("user", text.to_string()),
                Entry::Thinking(text) => ("thinking", text.to_string()),
                Entry::Step { tool, .. } => ("step", tool.to_string()),
                Entry::Agent(text) => ("agent", text.to_string()),
                Entry::Checkpoint { id, label } => ("checkpoint", format!("{id}\t{label}")),
            };
            if let Err(error) = storage.insert_turn(&thread.id, role, &content) {
                eprintln!("turn の永続化に失敗: {error:#}");
                return; // persisted_entries を進めない = 次回リトライ
            }
        }
        thread.persisted_entries = thread.entries.len();
        // 色 index は巡回パレットの逆引き（見つからなければ 0）。
        let color = thread.color;
        let color_index = (0..12).find(|i| thread_color(*i) == color).unwrap_or(0) as i64;
        if let Err(error) = storage.upsert_thread(
            &thread.id,
            thread.name.as_ref(),
            color_index,
            &project,
            branch.as_ref().map(|branch| branch.as_ref()),
            Some(thread.model.as_ref()),
            thread.tokens_used as i64,
            thread.tokens_max as i64,
        ) {
            eprintln!("スレッドの永続化に失敗: {error:#}");
        }
    }

    /// 最初のターン後、まだ既定名なら会話の冒頭から AI にタイトルを付けてもらう（#6・非同期）。
    /// 手動改名済み or 既に命名済み（＝プレースホルダでない）スレッドは対象外。失敗は静かに既定名のまま。
    fn maybe_auto_name(&mut self, thread_index: usize, cx: &mut Context<Self>) {
        if !settings::get(cx).agent_auto_name {
            return;
        }
        let Some(thread) = self.threads.get(thread_index) else {
            return;
        };
        if thread.name_is_custom || !is_placeholder_name(&thread.name) {
            return;
        }
        // 会話の冒頭（最初の user 発言 + 最初の agent 応答の先頭）を excerpt に。
        let Some(first_user) = thread.entries.iter().find_map(|entry| match entry {
            Entry::User(text) => Some(text.to_string()),
            _ => None,
        }) else {
            return; // まだ発話がない
        };
        let first_agent = thread.entries.iter().find_map(|entry| match entry {
            Entry::Agent(text) => Some(text.chars().take(400).collect::<String>()),
            _ => None,
        });
        let excerpt = match first_agent {
            Some(agent) => format!("User: {first_user}\n\nAgent: {agent}"),
            None => format!("User: {first_user}"),
        };
        let Some(cwd) = self.dest_cwd.clone() else {
            return;
        };
        let host = self.dest_host.clone();
        // タイトル生成はユーザーの既定 Agent の vendor CLI で（Claude 決め打ちをやめる・#6）。
        // 既定 agent に一発生成 CLI が無ければタイトルは既定名のまま（壊さない）。
        let agent_label = settings::get(cx).default_agent.clone();
        let Some(oneshot) =
            acp_client::AgentKind::by_label(&agent_label).and_then(|kind| kind.oneshot())
        else {
            return;
        };
        let thread_id = thread.id.clone();
        cx.spawn(async move |panel, cx| {
            let generated = cx
                .background_executor()
                .spawn(
                    async move { project::name_thread_on(host.as_ref(), &cwd, &excerpt, oneshot) },
                )
                .await;
            let Ok(name) = generated else {
                return; // claude 未導入等 → 既定名のまま（静かに諦める）
            };
            panel
                .update(cx, |panel, cx| {
                    if let Some(index) = panel
                        .threads
                        .iter()
                        .position(|thread| thread.id == thread_id)
                    {
                        let thread = &mut panel.threads[index];
                        // 生成待ちの間にユーザーが手動改名していたら尊重する。
                        if !thread.name_is_custom && is_placeholder_name(&thread.name) {
                            let name = SharedString::from(name);
                            thread.name = name.clone();
                            cx.emit(PanelEvent::ThreadAutoNamed { name });
                            panel.persist_thread(index);
                            cx.notify();
                        }
                    }
                })
                .ok();
        })
        .detach();
    }

    /// このパスを触ったスレッドの色（色リンク・M12-4）。複数スレッドなら最後に触った方。
    pub fn touched_color_for(&self, path: &std::path::Path) -> Option<Hsla> {
        self.threads
            .iter()
            .rev()
            .find(|thread| thread.touched_files.iter().any(|touched| touched == path))
            .map(|thread| thread.color)
    }

    /// アクティブスレッドが触ったファイル一覧（「触ったファイル n」チップ用・M12-4）。
    pub fn active_touched_files(&self) -> Vec<std::path::PathBuf> {
        self.threads
            .get(self.active)
            .map(|thread| thread.touched_files.clone())
            .unwrap_or_default()
    }

    pub fn set_destination(
        &mut self,
        project: SharedString,
        branch: Option<SharedString>,
        host: Arc<dyn Host>,
        cwd: Option<PathBuf>,
        files: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let destination_changed = self.dest_host.id() != host.id() || self.dest_cwd != cwd;
        self.dest_project = project;
        self.dest_branch = branch;
        self.dest_host = host;
        self.dest_cwd = cwd;
        self.context_files = files;
        if destination_changed {
            for thread in &mut self.threads {
                thread.command_tx = None;
            }
        }
        self.sync_running_registry(cx);
        cx.notify();
    }

    /// 実行中スレッド台帳（Global・全窓横断）へ自窓の状態を書く（⌘O ダッシュボード用・M12-12）。
    /// キーは worktree root（= dest_cwd）。書き手は窓ごとに排他なので上書きで正しい。
    fn sync_running_registry(&self, cx: &mut Context<Self>) {
        let Some(root) = self.dest_cwd.clone() else {
            return;
        };
        let rows: Vec<(SharedString, Hsla, ThreadActivity)> = self
            .threads
            .iter()
            .map(|thread| (thread.name.clone(), thread.color, thread.activity()))
            .collect();
        cx.default_global::<RunningRegistry>().0.insert(root, rows);
    }

    /// 現在アクティブなスレッド名（herd 下段の「focus 中 worktree」見出し・M14）。空パネルは None。
    pub fn active_thread_name(&self) -> Option<SharedString> {
        self.threads.get(self.active).map(|thread| thread.name.clone())
    }

    pub fn active_color(&self) -> Hsla {
        self.threads
            .get(self.active)
            .map(|thread| thread.color)
            .unwrap_or_else(|| thread_color(0))
    }

    /// titlebar beacon / statusbar ドット用: (スレッド名, スレッド色, 状態) の一覧。
    /// UI-SPEC §3: 実行中スレッドの状態を窓上部から常に見えるようにする（BACKGROUND の原点痛点）。
    pub fn beacons(&self) -> Vec<(SharedString, Hsla, ThreadActivity)> {
        self.threads
            .iter()
            .map(|thread| (thread.name.clone(), thread.color, thread.activity()))
            .collect()
    }

    /// herd サイドバー（状態一覧・M14）向けの全スレッド要約。`beacons()` にエージェント種別と
    /// トークンを足したもの。プロジェクト（宛先）別グループは workspace 側が session を跨いで束ねる。
    pub fn statuses(&self) -> Vec<AgentStatus> {
        self.threads
            .iter()
            .map(|thread| {
                let activity = thread.activity();
                // Working 中はライブ素材を合成（保存しない）。それ以外は遷移時に確定した digest。
                let digest = match activity {
                    ThreadActivity::Working => thread.live_digest().or_else(|| thread.digest.clone()),
                    _ => thread.digest.clone(),
                };
                let (plan_done, plan_total) = thread.plan_progress();
                AgentStatus {
                    name: thread.name.clone(),
                    color: thread.color,
                    activity,
                    agent: thread.agent.clone(),
                    tokens_used: thread.tokens_used,
                    tokens_max: thread.tokens_max,
                    created_at_ms: thread.created_at_ms,
                    last_input_at_ms: thread.last_input_at_ms,
                    digest,
                    plan_done,
                    plan_total,
                    files_touched: thread.touched_files.len() as u32,
                    turn_elapsed_secs: matches!(
                        activity,
                        ThreadActivity::Working | ThreadActivity::Blocked
                    )
                    .then(|| thread.turn_started_at.map(|at| at.elapsed().as_secs()))
                    .flatten(),
                    muted: thread.muted,
                    tier2: thread.tier2.clone(),
                }
            })
            .collect()
    }

    /// エージェント別ミュートの切替（P2・herd 行の 🔕）。muted 中はこのスレッドの
    /// トースト/完了音を出さない（ニュースフィードには載る＝見えるが鳴らない）。
    pub fn toggle_thread_mute(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(index) {
            thread.muted = !thread.muted;
            cx.notify();
        }
    }

    /// 管制の要対応キュー（P3）向け: 承認待ちカードの素材。
    /// options は `(respond へ返す添字, 種別, 表示ラベル)` — 添字は ACP が広告した順そのまま。
    pub fn permission_card(&self, thread_index: usize) -> Option<PermissionCard> {
        let thread = self.threads.get(thread_index)?;
        let pending = thread.pending_permission.as_ref()?;
        Some(PermissionCard {
            title: pending.title.clone(),
            options: pending
                .options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    (index, option.kind, SharedString::from(option.label.clone()))
                })
                .collect(),
            waited_secs: pending.since.elapsed().as_secs(),
            diff_files: pending.diffs.len(),
        })
    }

    /// 承認待ちへ**任意スレッド**で応答する（管制のインライン許可/拒否・P3）。
    /// アクティブスレッドの承認カードと同じ一本道（checkpoint は受信時に記録済み）。
    pub fn respond_permission(
        &mut self,
        thread_index: usize,
        option_index: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(thread) = self.threads.get_mut(thread_index) {
            if let Some(pending) = thread.pending_permission.take() {
                pending.respond.unbounded_send(option_index).ok();
            }
        }
        self.sync_running_registry(cx);
        cx.notify();
    }

    /// Done ラッチを**明示確認**で落とす（Done→Idle の確認済み遷移・P3。herdr の done/idle 区別）。
    /// スレッドを開かなくても管制のキューから「確認」で消せる。
    pub fn mark_done_seen(&mut self, thread_index: usize, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(thread_index) {
            thread.done = None;
        }
        self.sync_running_registry(cx);
        cx.notify();
    }

    /// herd 行 / ⌘O から特定スレッドをアクティブにする（M14 focus-follows の入口）。index は
    /// `statuses()`/`beacons()` の並び（＝タブ順）に一致する。範囲外・現在アクティブは no-op。
    pub fn focus_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        self.switch_thread(index, cx);
    }

    /// スレッドの transcript 末尾 `max` 件を (bullet, text) で返す（編隊グリッドの Agent セル・M14 #3）。
    /// bullet = ⏺ ステップ/本文 / ✳ 思考 / ▸ ユーザー / ⟲ checkpoint。空スレッドは空 Vec。
    pub fn transcript_lines(&self, thread_index: usize, max: usize) -> Vec<(SharedString, SharedString)> {
        let Some(thread) = self.threads.get(thread_index) else {
            return Vec::new();
        };
        let skip = thread.entries.len().saturating_sub(max);
        thread
            .entries
            .iter()
            .skip(skip)
            .map(|entry| {
                let (bullet, text): (&str, SharedString) = match entry {
                    Entry::User(text) => ("▸", text.clone()),
                    Entry::Thinking(text) => ("✳", text.clone()),
                    Entry::Step { tool, result, .. } => {
                        ("⏺", result.clone().unwrap_or_else(|| tool.clone()))
                    }
                    Entry::Agent(text) => ("⏺", text.clone()),
                    Entry::Checkpoint { label, .. } => ("⟲", label.clone()),
                };
                (SharedString::from(bullet), text)
            })
            .collect()
    }

    /// 本文テキストを**選択可能**にして描く（M13・transcript ドラッグ選択の部品）。
    /// `base_highlights`（markdown の装飾など）に加え、選択範囲だけ背景（スレッド色）を
    /// `combine_highlights` で重ねる（重なりは端点スイープで非重複ランへ畳まれる＝入れ子強調も安全）。
    /// layout を registry へ登録し、マウスイベントのヒットテストに使う。リージョン index = 登録順（描画順）。
    fn push_selectable(
        &self,
        text: SharedString,
        base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> gpui::AnyElement {
        let region_index = self.transcript_regions.borrow().len();
        let selection = self
            .region_selection(region_index, text.len())
            .map(|range| {
                (
                    range,
                    HighlightStyle {
                        background_color: Some(self.active_color().alpha(0.30)),
                        ..Default::default()
                    },
                )
            });
        let highlights: Vec<_> = gpui::combine_highlights(base_highlights, selection).collect();
        let mut styled = StyledText::new(text.clone());
        if !highlights.is_empty() {
            styled = styled.with_highlights(highlights);
        }
        let layout = styled.layout().clone();
        self.transcript_regions
            .borrow_mut()
            .push(SelectableRegion { text, layout });
        styled.into_any_element()
    }

    /// 装飾なしの選択可能テキスト（プレーンなエントリ用）。
    fn selectable_text(&self, text: SharedString) -> gpui::AnyElement {
        self.push_selectable(text, Vec::new())
    }

    /// リージョンに掛かる選択 byte range（無ければ None。offset はヒットテスト由来 = char 境界保証）。
    fn region_selection(&self, region: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.transcript_selection.as_ref()?.normalized();
        if region < start.region || region > end.region {
            return None;
        }
        let from = if region == start.region {
            start.offset.min(len)
        } else {
            0
        };
        let to = if region == end.region {
            end.offset.min(len)
        } else {
            len
        };
        (from < to).then(|| from..to)
    }

    /// マウス位置 → transcript 上の選択点。リージョンの隙間は直前リージョンの末尾へ丸める。
    fn transcript_point_at(&self, position: gpui::Point<gpui::Pixels>) -> Option<TranscriptPoint> {
        let regions = self.transcript_regions.borrow();
        let mut previous_end: Option<TranscriptPoint> = None;
        for (region_index, region) in regions.iter().enumerate() {
            let bounds = region.layout.bounds();
            if position.y < bounds.top() {
                // このリージョンより上 = 直前の末尾（最上部より上なら先頭）。
                return Some(previous_end.unwrap_or(TranscriptPoint {
                    region: region_index,
                    offset: 0,
                }));
            }
            if position.y <= bounds.bottom() {
                let offset = match region.layout.index_for_position(position) {
                    Ok(index) | Err(index) => index.min(region.text.len()),
                };
                return Some(TranscriptPoint {
                    region: region_index,
                    offset,
                });
            }
            previous_end = Some(TranscriptPoint {
                region: region_index,
                offset: region.text.len(),
            });
        }
        previous_end
    }

    /// 選択テキストを結合して返す（エントリ間は空行区切り）。空選択は None。
    fn transcript_selected_text(&self) -> Option<String> {
        let (start, end) = self.transcript_selection.as_ref()?.normalized();
        if start == end {
            return None;
        }
        let regions = self.transcript_regions.borrow();
        let mut parts = Vec::new();
        for (region_index, region) in regions.iter().enumerate() {
            if region_index < start.region || region_index > end.region {
                continue;
            }
            let text = region.text.as_ref();
            let from = if region_index == start.region {
                start.offset.min(text.len())
            } else {
                0
            };
            let to = if region_index == end.region {
                end.offset.min(text.len())
            } else {
                text.len()
            };
            if from < to {
                parts.push(text[from..to].to_string());
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    fn on_transcript_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let point = self.transcript_point_at(event.position);
        self.transcript_selection = point.map(|point| TranscriptSelection {
            start: point,
            end: point,
            selecting: true,
        });
        cx.notify();
    }

    fn on_transcript_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .transcript_selection
            .as_ref()
            .is_some_and(|selection| selection.selecting)
        {
            return;
        }
        let point = self.transcript_point_at(event.position);
        if let (Some(selection), Some(point)) = (self.transcript_selection.as_mut(), point) {
            if selection.end != point {
                selection.end = point;
                cx.notify();
            }
        }
    }

    fn on_transcript_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(selection) = self.transcript_selection.as_mut() {
            selection.selecting = false;
            if selection.start == selection.end {
                self.transcript_selection = None; // ただのクリック = 選択なし
            }
            cx.notify();
        }
    }

    /// パネル全域のキー: transcript 選択中の ⌘C はそれをコピー（composer より優先）。Esc で解除。
    fn on_panel_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform
            && !keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && keystroke.key == "c"
        {
            if let Some(text) = self.transcript_selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.stop_propagation();
                return;
            }
        }
        if keystroke.key == "escape" && self.transcript_selection.take().is_some() {
            cx.notify();
        }
    }

    /// ストリーミング追従: **底に居る時だけ**最下部へ張り付く（遡って読んでいる間は動かさない）。
    /// offset.y は下へスクロールするほど負（GPUI）。底からの残りが 1 行分程度なら「底に居る」。
    fn follow_transcript_if_at_bottom(&self) {
        let offset = self.transcript_scroll.offset();
        let max = self.transcript_scroll.max_offset();
        if max.y <= px(0.) || offset.y <= -(max.y - px(60.)) {
            self.transcript_scroll.scroll_to_bottom();
        }
    }

    fn switch_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.threads.len() || index == self.active {
            return;
        }
        self.renaming = None; // 別タブへ切替えたら編集中の改名は破棄する
        self.active = index;
        if let Some(thread) = self.threads.get_mut(index) {
            thread.done = None; // 見た＝herdr の Done ラッチ（完了・未確認）を解除
        }
        self.reveal = usize::MAX; // 切替先は全文表示（打ち直さない）
        self.transcript_scroll.scroll_to_bottom(); // 切替先は最新（末尾）を見せる
        let color = self.active_color();
        self.composer
            .update(cx, |composer, cx| composer.set_accent(color, cx));
        self.sync_running_registry(cx); // Done 解除をロールアップ（フッター/レール/⌘O）へ反映
        cx.notify();
    }

    /// スレッドタブを閉じる（× ボタン / ⌘W）。**最後の 1 枚も閉じられる**（空状態＝＋で再開）。
    /// ACP セッション（`command_tx`）は畳むが、会話履歴ごと closed_threads に退避し ⌘⇧T で復元可能。
    fn remove_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.threads.len() {
            return;
        }
        let mut closed = self.threads.remove(index);
        closed.command_tx = None; // セッションは畳む（復元時に次の送信で張り直す）
        closed.running = false;
        closed.turn_started_at = None;
        self.closed_threads.push(closed);
        // active を有効域へ寄せる。空になったら 0（描画は get(active)=None で空状態になる）。
        if self.threads.is_empty() {
            self.active = 0;
        } else if index < self.active {
            self.active -= 1;
        } else if index == self.active {
            self.active = self.active.min(self.threads.len() - 1);
        }
        self.reveal = usize::MAX; // 実効スレッドが変わるので全文表示
        let color = self.active_color();
        self.composer
            .update(cx, |composer, cx| composer.set_accent(color, cx));
        cx.notify();
    }

    /// 指定インデックスのスレッドを閉じる（アーカイブして一覧から除く）。編隊セルの × / herd 行の削除・
    /// ⌘W から共用。台帳の行は残り、`closed_threads` に積むので ⌘⇧T で復元できる（M12-1）。
    pub fn close_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        // 永続化側はアーカイブ（一覧から消えるが台帳の行は残る・M12-1）。
        if let (Some(storage), Some(thread)) = (self.storage.clone(), self.threads.get(index)) {
            let _ = storage.archive_thread(&thread.id);
        }
        self.remove_thread(index, cx);
    }

    /// アクティブな AI スレッドタブを閉じる（⌘W）。workspace が Agent フォーカス時に呼ぶ。
    pub fn close_active_thread(&mut self, cx: &mut Context<Self>) {
        self.close_thread(self.active, cx);
    }

    /// 直近に閉じたスレッドを復元する（⌘⇧T。会話履歴ごと戻し、末尾へ追加してアクティブに）。
    pub fn restore_closed_thread(&mut self, cx: &mut Context<Self>) {
        if let Some(thread) = self.closed_threads.pop() {
            self.threads.push(thread);
            self.active = self.threads.len() - 1;
            self.reveal = usize::MAX; // 復元スレッドは全文表示
            let color = self.active_color();
            self.composer
                .update(cx, |composer, cx| composer.set_accent(color, cx));
            cx.notify();
        }
    }

    /// 履歴から選んだスレッドを開く（#5）。既に開いていれば切替、無ければ復元して末尾へ・アクティブに。
    /// 開いたスレッドの index を返す（編隊モードが復元セルを出すのに使う・M14。storage 無しは None）。
    pub fn open_thread_from_history(
        &mut self,
        id: &str,
        name: &str,
        color_index: usize,
        created_at_ms: i64,
        last_input_at_ms: Option<i64>,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if let Some(index) = self.threads.iter().position(|thread| thread.id == id) {
            self.switch_thread(index, cx);
            return Some(index);
        }
        let storage = self.storage.clone()?;
        let mut thread = thread_from_storage(&storage, id, name, color_index);
        thread.created_at_ms = created_at_ms;
        thread.last_input_at_ms = last_input_at_ms;
        self.threads.push(thread);
        self.active = self.threads.len() - 1;
        self.renaming = None;
        self.reveal = usize::MAX; // 復元スレッドは全文表示
        let color = self.active_color();
        self.composer
            .update(cx, |composer, cx| composer.set_accent(color, cx));
        self.transcript_scroll.scroll_to_bottom();
        cx.notify();
        Some(self.active)
    }

    /// composer（入力欄）にキーボードフォーカスを移す。タブ操作時に呼び「Agent にいる」状態にする。
    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// 次のタブへ（Chrome ⌘⌥→ / ⌃Tab。末尾で先頭へ回る）。
    pub fn select_next_thread(&mut self, cx: &mut Context<Self>) {
        if self.threads.len() > 1 {
            self.switch_thread((self.active + 1) % self.threads.len(), cx);
        }
    }

    /// 前のタブへ（Chrome ⌘⌥← / ⌃⇧Tab。先頭で末尾へ回る）。
    pub fn select_prev_thread(&mut self, cx: &mut Context<Self>) {
        let count = self.threads.len();
        if count > 1 {
            self.switch_thread((self.active + count - 1) % count, cx);
        }
    }

    /// タブを `from` から `to` へ移動する（ドラッグ並べ替え）。active は同じスレッドを指し続ける。
    fn move_thread(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let count = self.threads.len();
        if from >= count || to >= count || from == to {
            return;
        }
        let thread = self.threads.remove(from);
        self.threads.insert(to, thread);
        // active が指すスレッドを追従させる（remove→insert のインデックスずれを補正）。
        self.active = if self.active == from {
            to
        } else {
            let mut active = self.active;
            if from < active {
                active -= 1;
            }
            if to <= active {
                active += 1;
            }
            active
        };
        cx.notify();
    }

    fn add_thread(&mut self, cx: &mut Context<Self>) {
        let index = self.threads.len();
        let mut thread = Thread::empty(format!("スレッド{}", index + 1), index);
        thread.agent = default_agent_name(cx); // 新規は既定エージェントで開く
        self.threads.push(thread);
        self.switch_thread(index, cx);
        cx.notify();
    }

    /// タブ名のインライン編集を開始する（ダブルクリック・#4）。IME 正しい `EditorView::plain` を使う。
    fn start_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.threads.len() {
            return;
        }
        if self
            .renaming
            .as_ref()
            .is_some_and(|(editing, _)| *editing == index)
        {
            return; // 既に同じタブを編集中
        }
        let color = self.threads[index].color;
        let name = self.threads[index].name.clone();
        // Enter で確定（submit_on_enter=true・IME 変換確定の Enter では Submit を出さない＝誤確定しない）。
        let editor = cx.new(|cx| {
            let mut view = EditorView::plain(self.theme.clone(), color, true, cx);
            view.set_plain_text(name.as_ref(), cx);
            view
        });
        cx.subscribe(&editor, |panel, _editor, event, cx| match event {
            ComposerEvent::Submit => panel.confirm_rename(cx),
        })
        .detach();
        let handle = editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.renaming = Some((index, editor));
        cx.notify();
    }

    /// 改名を確定する（Enter）。空白のみは無視。手動改名フラグを立て永続化する（#4）。
    fn confirm_rename(&mut self, cx: &mut Context<Self>) {
        let Some((index, editor)) = self.renaming.take() else {
            return;
        };
        let text = editor.read(cx).plain_text();
        let name = text.trim();
        if !name.is_empty() {
            if let Some(thread) = self.threads.get_mut(index) {
                thread.name = SharedString::from(name.to_string());
                thread.name_is_custom = true; // 以後 AI 自動命名で上書きしない
            }
            self.persist_thread(index);
        }
        cx.notify();
    }

    /// 改名を取り消す（Esc / 別タブへ切替）。入力内容は破棄する。
    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            cx.notify();
        }
    }

    /// 開発用: フォーカス無しで改名入力を開く（offscreen スクショ検証・#4）。
    #[cfg(debug_assertions)]
    pub fn debug_start_rename(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        if index >= self.threads.len() {
            return;
        }
        let color = self.threads[index].color;
        let name = self.threads[index].name.clone();
        let editor = cx.new(|cx| {
            let mut view = EditorView::plain(self.theme.clone(), color, true, cx);
            view.set_plain_text(name.as_ref(), cx);
            view
        });
        self.renaming = Some((index, editor));
        cx.notify();
    }

    /// 開発用: 複数スレッドに各状態（Working/Blocked/Done/中断 Done）を仕込む（offscreen で
    /// タブ・List・beacon・フッター・レール・⌘O の状態表示を1枚で検証する・#）。
    #[cfg(debug_assertions)]
    /// 開発用: 管制タブの受入シナリオ（P3・`SHIRUSHI_CONTROL_PROBE`）。Task パネル（1 thread）へ
    /// Working/Blocked/Done/Failed/Idle の代表状態を digest 素材つきで仕込む。
    #[cfg(debug_assertions)]
    pub fn debug_set_state(&mut self, style: u8, cx: &mut Context<Self>) {
        use std::time::{Duration, Instant};
        let Some(thread) = self.threads.get_mut(0) else {
            return;
        };
        match style {
            0 => {
                // Working: ライブ digest（実行中ツール + plan 進行中）・経過 12 分。
                thread.running = true;
                thread.turn_started_at = Instant::now().checked_sub(Duration::from_secs(12 * 60));
                thread.tokens_used = 42_100;
                thread.tokens_shown = 42_100.0;
                thread.entries.push(Entry::Step {
                    tool: "Bash(cargo test -p shirushi)".into(),
                    args: SharedString::default(),
                    result: None,
                });
                thread.plan = vec![
                    PlanItem { content: "設計を確認".into(), status: PlanStatus::Completed },
                    PlanItem { content: "実装".into(), status: PlanStatus::Completed },
                    PlanItem { content: "テスト修正".into(), status: PlanStatus::InProgress },
                    PlanItem { content: "docs 更新".into(), status: PlanStatus::Pending },
                    PlanItem { content: "スクショ検証".into(), status: PlanStatus::Pending },
                ];
            }
            1 => {
                // Blocked: 45 秒待ちの承認カード（respond は宙ぶらりんで良い＝描画検証専用）。
                let (respond, _rx) = mpsc::unbounded();
                let title = SharedString::from("shell: cargo publish を実行してよいですか");
                thread.digest = Some(title.clone());
                thread.running = true;
                thread.turn_started_at = Instant::now().checked_sub(Duration::from_secs(70));
                thread.tokens_used = 8_400;
                thread.tokens_shown = 8_400.0;
                thread.pending_permission = Some(PendingPermission {
                    title,
                    diffs: Vec::new(),
                    options: vec![
                        PermissionChoice { label: "許可".into(), kind: PermissionKind::Allow },
                        PermissionChoice {
                            label: "常に許可".into(),
                            kind: PermissionKind::AllowAlways,
                        },
                        PermissionChoice { label: "拒否".into(), kind: PermissionKind::Reject },
                    ],
                    respond,
                    since: Instant::now()
                        .checked_sub(Duration::from_secs(45))
                        .unwrap_or_else(Instant::now),
                });
            }
            2 => {
                thread.done = Some(TurnEnd::Completed);
                thread.digest =
                    Some("ヒーロー節を ja/en 両方書き換え、スクショ検証まで完了。".into());
                thread.tier2 = Some("LP のヒーロー節を ja/en 両方更新し radar clean".into());
                thread.tokens_used = 12_000;
                thread.tokens_shown = 12_000.0;
            }
            3 => {
                thread.done = Some(TurnEnd::Interrupted);
                thread.digest = Some("cargo check 失敗 (2) — notify 8.2 の API 変更".into());
                thread.tier2 = Some("notify 8.2 の破壊的変更で cargo check が 2 件失敗".into());
                thread.tokens_used = 5_100;
                thread.tokens_shown = 5_100.0;
            }
            _ => {}
        }
        self.sync_running_registry(cx);
        cx.notify();
    }

    pub fn debug_set_activities(&mut self, cx: &mut Context<Self>) {
        for (index, thread) in self.threads.iter_mut().enumerate() {
            thread.running = false;
            thread.done = None;
            thread.pending_permission = None;
            match index % 4 {
                0 => {
                    // Working: digest はライブ素材（最新ツール + plan の進行中）から合成される（P1）。
                    thread.running = true;
                    thread.turn_started_at = Some(std::time::Instant::now());
                    thread.entries.push(Entry::Step {
                        tool: "Bash cargo test -p shirushi".into(),
                        args: SharedString::default(),
                        result: None,
                    });
                    thread.plan = vec![
                        PlanItem { content: "設計を確認".into(), status: PlanStatus::Completed },
                        PlanItem { content: "テストを直す".into(), status: PlanStatus::InProgress },
                        PlanItem { content: "docs 更新".into(), status: PlanStatus::Pending },
                    ];
                }
                1 => {
                    let (respond, _rx) = mpsc::unbounded(); // Blocked（承認待ち）
                    let title = SharedString::from("workspace.rs への書き込みを許可しますか");
                    thread.digest = Some(title.clone()); // 実経路（on_event）と同じ素材①
                    thread.pending_permission = Some(PendingPermission {
                        title,
                        diffs: Vec::new(),
                        options: Vec::new(),
                        respond,
                        since: std::time::Instant::now(),
                    });
                }
                2 => {
                    thread.done = Some(TurnEnd::Completed); // Done（未確認）
                    thread.digest =
                        Some("全 21 test green。次は P2 のニュース常設に進めます。".into());
                }
                _ => {
                    thread.done = Some(TurnEnd::Interrupted); // Done（中断）
                    thread.digest = Some("エラー: 接続が切断されました（exit 1）".into());
                }
            }
        }
        self.sync_running_registry(cx); // レール/フッター/⌘O のロールアップへ反映
        cx.notify();
    }

    /// テーマを差し替える（テーマセレクタのライブプレビュー / 切替）。composer にも波及させる。
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme.clone();
        self.composer
            .update(cx, |composer, cx| composer.set_theme(theme, cx));
        cx.notify();
    }

    /// 新規スレッドを作る公開口（workspace の ⌘⇧A から呼ぶ）。
    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.add_thread(cx);
    }

    /// 新規スレッドを作り、その index を返す（編隊グリッドの ＋Agent＝新エージェント起動・M14）。
    /// `add_thread` は末尾に append してそこへ switch するので、新 index = 末尾。
    pub fn new_thread_index(&mut self, cx: &mut Context<Self>) -> usize {
        self.add_thread(cx);
        self.threads.len().saturating_sub(1)
    }

    /// 監督スレッドの確保（P6）: 名前一致のスレッドがあればアクティブ化・無ければ作って改名。
    /// pinned = 名前で再利用（`name_is_custom` を立てて自動命名の上書きも防ぐ）。
    pub fn ensure_named_thread(&mut self, name: &str, agent: &str, cx: &mut Context<Self>) -> usize {
        if let Some(index) = self
            .threads
            .iter()
            .position(|thread| thread.name.as_ref() == name)
        {
            self.switch_thread(index, cx);
            return index;
        }
        let index = self.acquire_thread(Some(agent.to_string()), cx);
        if let Some(thread) = self.threads.get_mut(index) {
            thread.name = SharedString::from(name.to_string());
            thread.name_is_custom = true;
        }
        index
    }

    /// スレッドが手一杯か（実行中 or 承認待ち）。監督 wake の「重ねない」判定（P6）。
    pub fn thread_busy(&self, index: usize) -> bool {
        self.threads
            .get(index)
            .is_some_and(|thread| thread.running || thread.pending_permission.is_some())
    }

    /// IPC の spawn 用（P5）: 未使用の空スレッド（entries 無し・未実行）があれば使い回し、
    /// 無ければ新規。`agent` 指定があれば宛先エージェントを差し替える。返り値 = アクティブ化済み index。
    pub fn acquire_thread(&mut self, agent: Option<String>, cx: &mut Context<Self>) -> usize {
        let reusable = self.threads.iter().position(|thread| {
            thread.entries.is_empty() && !thread.running && thread.done.is_none()
        });
        let index = match reusable {
            Some(index) => {
                self.switch_thread(index, cx);
                index
            }
            None => self.new_thread_index(cx),
        };
        if let Some(agent) = agent {
            if let Some(thread) = self.threads.get_mut(index) {
                thread.agent = SharedString::from(agent);
            }
        }
        cx.notify();
        index
    }

    fn toggle_menu(&mut self, selector: Selector, cx: &mut Context<Self>) {
        self.open_menu = if self.open_menu == Some(selector) {
            None
        } else {
            Some(selector)
        };
        cx.notify();
    }

    /// セレクタのドロップダウンに出す選択肢。エージェントが広告した実選択肢（モード/モデル/effort）が
    /// あればそれを、無ければ静的な既定ラベルを返す。
    fn selector_options(&self, selector: Selector) -> Vec<SharedString> {
        let advertised = self
            .threads
            .get(self.active)
            .and_then(|thread| match selector {
                Selector::Mode if !thread.available_modes.is_empty() => Some(
                    thread
                        .available_modes
                        .iter()
                        .map(|(_, name)| name.clone())
                        .collect(),
                ),
                Selector::Model => config_choice_names(thread, ConfigCategory::Model),
                Selector::Effort => config_choice_names(thread, ConfigCategory::ThoughtLevel),
                _ => None,
            });
        advertised.unwrap_or_else(|| {
            selector
                .options()
                .iter()
                .map(|option| SharedString::from(*option))
                .collect()
        })
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

    /// 選択ピルの値を**このスレッドだけ**に反映（Model/Effort は `session/set_config_option`、
    /// Mode は `session/set_mode` を実際に送る）。**グローバル既定（`default_agent` 等）は触らない**
    /// — 既定は Settings 画面でだけ変える。哲学「自分で決めた既定は意図しない限り変わらない」
    /// （DECISIONS §・ドリフト禁止）。
    fn select_option(&mut self, selector: Selector, value: SharedString, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            match selector {
                Selector::Agent => {
                    // このスレッドのエージェントだけ差し替え（次の送信で新セッションを張り直す）。
                    // 以前はここで default_agent を巻き添え保存していたが、ドリフトの原因なので廃止。
                    thread.agent = value.clone();
                    thread.command_tx = None;
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
                        command_tx
                            .unbounded_send(SessionCommand::SetMode(mode_id))
                            .ok();
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
        self.context_query.clear();
        self.context_focus = self.context_menu_open.then(|| cx.focus_handle());
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

    /// ドロップされたファイルパスを @メンションに加える（プロジェクト root 配下なら相対、外なら絶対）。
    /// D&D（Finder / エクスプローラ）→ context 参照の受け口。
    fn add_context_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let display = self
            .dest_cwd
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .map(|relative| relative.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        self.add_context(SharedString::from(display), cx);
    }

    /// composer 下部の選択ピル（Zed 風。クリックでドロップダウン）。
    /// composer 下部の選択コントロール。Zed 流の「テキスト + シェブロン」＝**枠も塗りも無し**、
    /// hover でだけ薄く面が出る。開いている時だけラベルをスレッド色（accent）にする（チップにしない）。
    fn render_selector_pill(&self, selector: Selector, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let value = self.selector_value(selector);
        let is_open = self.open_menu == Some(selector);
        let accent = self.active_color();
        let (id, tip): (&'static str, String) = match selector {
            Selector::Agent => ("pill-agent", i18n::t!("agent.pill_agent")),
            Selector::Mode => ("pill-mode", i18n::t!("agent.pill_mode")),
            Selector::Model => ("pill-model", i18n::t!("agent.pill_model")),
            Selector::Effort => ("pill-effort", i18n::t!("agent.pill_effort")),
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
            .hover(|style| {
                style
                    .bg(theme.bg2)
                    .text_color(if is_open { accent } else { theme.fg0 })
            })
            .when(is_open, |element| element.bg(theme.bg2))
            .child(value)
            .child(div().text_size(px(8.)).text_color(theme.fg2).child("▾"))
            .tooltip(Tooltip::text(tip, theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.toggle_menu(selector, cx)),
            )
            // 開いている時、このピルの真上にドロップダウンを出す（ピル基準なのでズレない）。
            .when(is_open, |element| {
                element.child(self.render_selector_menu(selector, cx))
            })
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
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(option_index, option)| {
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
                    }),
            )
            .with_animation(
                "selector-menu",
                Animation::new(std::time::Duration::from_millis(120))
                    .with_easing(gpui::ease_out_quint()),
                |element, delta| {
                    element
                        .opacity(delta)
                        .bottom(px(24.0 - 6.0 * (1.0 - delta)))
                },
            )
            .into_any_element()
    }

    /// アクション（⌘Enter）ハンドラ。実処理は [`Self::submit`]。
    fn on_submit(&mut self, _: &SubmitPrompt, _window: &mut Window, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    /// ⌘W: アクティブスレッドを閉じる（× ボタンと同じ。最後の1枚は残す）。
    fn on_close_thread(
        &mut self,
        _: &CloseActiveThread,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    pub fn send_prompt_text(&mut self, prompt: String, cx: &mut Context<Self>) {
        let thread_index = self.active;
        // 添付コンテキストを prompt 先頭へ `@path` として付ける（表示は素の prompt のまま）。
        let context_prefix: String = self
            .threads
            .get(thread_index)
            .map(|thread| {
                thread
                    .context
                    .iter()
                    .map(|path| format!("@{path}\n"))
                    .collect()
            })
            .unwrap_or_default();
        let full_prompt = if context_prefix.is_empty() {
            prompt.clone()
        } else {
            format!("{context_prefix}\n{prompt}")
        };
        if let Some(thread) = self.threads.get_mut(thread_index) {
            thread
                .entries
                .push(Entry::User(SharedString::from(prompt.clone())));
            thread.running = true;
            thread.done = None; // 新しいターン開始＝直前の「完了・未確認」ラッチは消す
            thread.tier2 = None; // ✳ 要約は前ターンの文＝新ターンでは古い（P4）
            thread.turn_started_at = Some(std::time::Instant::now()); // 経過秒の起点
            thread.last_input_at_ms = Some(now_unix_ms()); // 「最終いつ入力したか」（M14）
        }
        self.sync_running_registry(cx); // ⌘O ダッシュボードの「実行中」を即時反映（M12-12）
        cx.notify();

        // 宛先プロジェクトの cwd が要る。
        let Some(cwd) = self.dest_cwd.clone() else {
            self.fail_turn(thread_index, &i18n::t!("agent.err_no_project"), cx);
            return;
        };
        if let Some(thread) = self.threads.get(thread_index) {
            cx.emit(PanelEvent::TurnStarted {
                thread: thread.name.clone(),
                color: thread.color,
            });
        }
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
                    self.fail_turn(thread_index, &i18n::t!("agent.err_no_acp"), cx);
                    return;
                }
            }
        }
        // prompt を送信。送信路が死んでいたらセッションを畳んで報告。
        let alive = self
            .threads
            .get(thread_index)
            .and_then(|thread| thread.command_tx.as_ref())
            .map(|command_tx| {
                command_tx
                    .unbounded_send(SessionCommand::Prompt(full_prompt))
                    .is_ok()
            })
            .unwrap_or(false);
        if !alive {
            if let Some(thread) = self.threads.get_mut(thread_index) {
                thread.command_tx = None;
            }
            self.fail_turn(thread_index, &i18n::t!("agent.err_session_lost"), cx);
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
            // 既定は AgentKind の label と完全一致させる（"Claude" だと by_label が None＝解決不能）。
            .unwrap_or_else(|| "Claude Code".into());
        let host = self.dest_host.clone();
        let command =
            acp_client::AgentKind::by_label(&agent_label)?.command_on(host.as_ref(), cwd)?;
        let (command_tx, prompt_rx) = mpsc::unbounded::<SessionCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded::<AgentEvent>();
        let error_tx = event_tx.clone();

        cx.background_executor()
            .spawn(async move {
                if let Err(error) =
                    acp_client::run_session_on(host, command, prompt_rx, event_tx).await
                {
                    // `{:#}` で anyhow の原因鎖まで出す（例: 「ACP セッションが異常終了: ACP
                    // initialize が 30 秒応答しません（無言ハング）」）。`to_string()` だと上位 context
                    // だけになりハンドシェイクの無言ハングが埋もれるため。
                    error_tx
                        .unbounded_send(AgentEvent::Failed(format!("{error:#}")))
                        .ok();
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
        let turn_finished = matches!(event, AgentEvent::TurnEnded { .. } | AgentEvent::Failed(_));
        // 完了音・バンザイは**正常完了時のみ**（中断＝拒否/キャンセルや失敗では祝わない）。
        let turn_succeeded = matches!(
            event,
            AgentEvent::TurnEnded {
                reason: TurnEnd::Completed
            }
        );
        let permission_waiting = matches!(event, AgentEvent::PermissionRequest { .. });
        let mut files_touched: Option<(Vec<std::path::PathBuf>, Hsla)> = None;
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
                if let Some((_, name)) =
                    thread.available_modes.iter().find(|(id, _)| *id == current)
                {
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
            AgentEvent::Plan(items) => {
                // プランは毎回全量で届く（ACP 仕様）ので**置換**。常設チェックリストが追従する。
                thread.plan = items;
            }
            AgentEvent::PermissionRequest {
                title,
                diffs,
                options,
                respond,
            } => {
                // checkpoint は**書かれる前**にここで切る（M12-2）。手動でも AUTO_ALLOW でも一本道。
                // old_text が diff に無いツール（Claude の Write 等）は**ディスクの現内容**を読んで
                // 変更前として記録する。読み書きは背景（Host は UI スレッド禁止）＋ AUTO_ALLOW の
                // 応答はスナップショット完了**後**（エージェントが書く前に読む＝レース防止）。
                if !diffs.is_empty() {
                    // 色リンク（M12-4）は即記録。
                    let files: Vec<std::path::PathBuf> = diffs
                        .iter()
                        .map(|diff| std::path::PathBuf::from(&diff.path))
                        .collect();
                    for file in &files {
                        if !thread.touched_files.contains(file) {
                            thread.touched_files.push(file.clone());
                        }
                    }
                    files_touched = Some((files, thread.color));
                }
                let auto_allow = std::env::var_os("SHIRUSHI_AUTO_ALLOW").is_some();
                let auto_choice = options
                    .iter()
                    .position(|option| {
                        matches!(
                            option.kind,
                            PermissionKind::Allow | PermissionKind::AllowAlways
                        )
                    })
                    .unwrap_or(0);
                let snapshot_inputs: Vec<(std::path::PathBuf, Option<String>)> = diffs
                    .iter()
                    .map(|diff| (std::path::PathBuf::from(&diff.path), diff.old_text.clone()))
                    .collect();
                let storage = self.storage.clone();
                let host = self.dest_host.clone();
                let thread_id = thread.id.clone();
                let label =
                    i18n::t!("agent.permission_files", "title" => title, "n" => diffs.len());
                if !auto_allow {
                    // 遷移スナップショット（P1 素材①）: Blocked = 何の許可を待っているか。
                    thread.digest = Some(SharedString::from(title.clone()));
                    thread.pending_permission = Some(PendingPermission {
                        title: SharedString::from(title),
                        diffs,
                        options,
                        respond: respond.clone(),
                        since: std::time::Instant::now(),
                    });
                }
                if !snapshot_inputs.is_empty() && storage.is_some() {
                    let entry_label = label.clone();
                    let thread_id_for_entry = thread_id.clone();
                    cx.spawn(async move |panel, cx| {
                        let checkpoint_id = cx
                            .background_executor()
                            .spawn(async move {
                                let blobs = storage::default_blobs_dir()?;
                                let storage = storage?;
                                let snapshot: Vec<(std::path::PathBuf, Option<String>)> =
                                    snapshot_inputs
                                        .into_iter()
                                        .map(|(path, old_text)| {
                                            let content = old_text.or_else(|| {
                                                // diff に変更前が無い → 書かれる前の今、ディスクから読む。
                                                host.read_file(&path).ok().and_then(|content| {
                                                    String::from_utf8(content.bytes).ok()
                                                })
                                            });
                                            (path, content)
                                        })
                                        .collect();
                                storage
                                    .save_checkpoint(&thread_id, &label, snapshot, &blobs)
                                    .ok()
                            })
                            .await;
                        // スナップショットが済んでから許可を返す（自動許可の場合）。
                        if auto_allow {
                            respond.unbounded_send(auto_choice).ok();
                        }
                        if let Some(id) = checkpoint_id {
                            let _ = panel.update(cx, |panel, cx| {
                                if let Some(thread) = panel
                                    .threads
                                    .iter_mut()
                                    .find(|thread| thread.id == thread_id_for_entry)
                                {
                                    thread.entries.push(Entry::Checkpoint {
                                        id,
                                        label: SharedString::from(entry_label),
                                    });
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .detach();
                } else if auto_allow {
                    respond.unbounded_send(auto_choice).ok();
                }
                // 承認待ちの間もターンは継続中（running のまま＝ pulse で「待ち」を示す）。
            }
            AgentEvent::TurnEnded { reason } => {
                // この turn で積まれた entries を DB へ（M12-1）。
                // （thread 借用中なので index 経由で後段に回す）
                thread.running = false;
                thread.pending_permission = None; // 念のため（通常は応答時に消える）
                                                  // 裏のスレッドは「完了・未確認」を灯す（herdr の Done ラッチ＝見るまで残す）。
                                                  // アクティブ（今見ている）スレッドは即 Idle に落とす。
                if thread_index != active {
                    thread.done = Some(reason);
                }
                // 遷移スナップショット（P1 素材②）: Done = 最後の発言の末尾 1〜2 文。
                // 本文の無いターン（ツールのみ等）は直前の digest を保つ。
                let tail = thread.entries.iter().rev().find_map(|entry| match entry {
                    Entry::Agent(text) => digest_tail(text),
                    _ => None,
                });
                if tail.is_some() {
                    thread.digest = tail;
                }
                // アクティブスレッドの**正常完了**でマスコットがバンザイ（数秒だけ）。中断/失敗では祝わない。
                celebrate_now = thread_index == active && reason == TurnEnd::Completed;
            }
            AgentEvent::Failed(error) => {
                // 遷移スナップショット（P1 素材③）: Failed = エラー文字列（1 行に畳む）。
                thread.digest = digest_tail(&error).or(thread.digest.take());
                thread
                    .entries
                    .push(Entry::Agent(SharedString::from(format!("エラー: {error}"))));
                thread.running = false;
                // 失敗も「注意を要する終わり方」＝裏のスレッドは Done(中断) として灯す。
                if thread_index != active {
                    thread.done = Some(TurnEnd::Interrupted);
                }
            }
        }
        if let Some((files, color)) = files_touched {
            cx.emit(PanelEvent::FilesTouched { files, color });
        }
        if turn_finished {
            // 完了音（設定 on・成功時のみ）。**window 非アクティブ時のみ**鳴らす（見ている画面に
            // 音は要らない・裏の窓/他アプリ作業中の完了に気づくための音・P2）。ミュート中スレッドは鳴らさない。
            let thread_muted = self
                .threads
                .get(thread_index)
                .is_some_and(|thread| thread.muted);
            if turn_succeeded
                && !self.window_active
                && !thread_muted
                && settings::get(cx).completion_sound
            {
                play_completion_sound();
            }
            self.sync_running_registry(cx); // 実行中 → 完了をダッシュボードへ（M12-12）
            self.persist_thread(thread_index); // turn 確定分を DB へ（M12-1）
            self.maybe_auto_name(thread_index, cx); // 初回ターン後、既定名なら AI がタイトルを付ける（#6）
            self.maybe_tier2_summary(thread_index, cx); // ✳ 1 行要約（P4・Done/Failed 遷移のみ）
            if let Some(thread) = self.threads.get(thread_index) {
                let elapsed = thread
                    .turn_started_at
                    .map(|at| format!("{:.0}s", at.elapsed().as_secs_f32()))
                    .unwrap_or_default();
                let summary = if thread.touched_files.is_empty() {
                    SharedString::from(i18n::t!("agent.done", "elapsed" => elapsed))
                } else {
                    SharedString::from(
                        i18n::t!("agent.done_touched", "elapsed" => elapsed, "n" => thread.touched_files.len()),
                    )
                };
                cx.emit(PanelEvent::TurnEnded {
                    thread: thread.name.clone(),
                    color: thread.color,
                    summary,
                    digest: thread.digest.clone(),
                    muted: thread.muted,
                });
            }
        }
        if permission_waiting {
            self.sync_running_registry(cx); // Blocked をレール/フッター/⌘O のロールアップへ即時反映
            if let Some(thread) = self.threads.get(thread_index) {
                cx.emit(PanelEvent::PermissionWaiting {
                    thread: thread.name.clone(),
                    color: thread.color,
                    title: thread
                        .pending_permission
                        .as_ref()
                        .map(|pending| pending.title.clone())
                        .unwrap_or_default(),
                    muted: thread.muted,
                });
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
            self.follow_transcript_if_at_bottom();
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
                        let step = if step.abs() < 1.0 {
                            diff.signum()
                        } else {
                            step
                        };
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

    /// Tier 2 遷移スナップショット（✳ 1 行要約・P4）: Done/Failed 遷移の直後に oneshot で生成する。
    /// スレッド命名（`maybe_auto_name`）と同じ機構＝**既定 Agent の CLI**・対応外エージェントや失敗は
    /// 静かに Tier 1 のまま（UI が欠けない）。**要約は状態を上書きしない**（文を添えるだけ）。
    fn maybe_tier2_summary(&mut self, thread_index: usize, cx: &mut Context<Self>) {
        if !settings::get(cx).tier2_summaries {
            return;
        }
        let Some(thread) = self.threads.get(thread_index) else {
            return;
        };
        // 素材 = 最後の指示 + 最終応答の末尾（Failed はエラー行が最後の Agent entry に積まれている）。
        let last_user = thread.entries.iter().rev().find_map(|entry| match entry {
            Entry::User(text) => Some(text.chars().take(300).collect::<String>()),
            _ => None,
        });
        let Some(last_agent) = thread.entries.iter().rev().find_map(|entry| match entry {
            Entry::Agent(text) => {
                let tail: String = text
                    .chars()
                    .rev()
                    .take(1200)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                Some(tail)
            }
            _ => None,
        }) else {
            return; // 本文の無いターン（ツールのみ等）は Tier 1 のまま
        };
        let input = match last_user {
            Some(user) => format!("指示: {user}\n\n結果(末尾): {last_agent}"),
            None => format!("結果(末尾): {last_agent}"),
        };
        let Some(cwd) = self.dest_cwd.clone() else {
            return;
        };
        let host = self.dest_host.clone();
        let agent_label = settings::get(cx).default_agent.clone();
        let Some(template) =
            acp_client::AgentKind::by_label(&agent_label).and_then(|kind| kind.oneshot())
        else {
            return; // oneshot テンプレ無し → Tier 1 のみに自然フォールバック
        };
        let thread_id = thread.id.clone();
        cx.spawn(async move |panel, cx| {
            // 引用符 / $ / バッククォート禁止（sh -c 埋め込み・oneshot_line_on の約束）。
            let prompt = "入力はエージェントターンの指示と結果です。何をして結果どうなったかを日本語で要約して。40文字以内・1行だけ・記号や引用符や前置きなしで出力して。";
            let generated = cx
                .background_executor()
                .spawn(async move {
                    project::oneshot_line_on(host.as_ref(), &cwd, &input, template, prompt, 60)
                })
                .await;
            let Ok(line) = generated else {
                return; // CLI 未導入・失敗 → Tier 1 表示のまま（静かに諦める）
            };
            let _ = panel.update(cx, |panel, cx| {
                if let Some(index) = panel
                    .threads
                    .iter()
                    .position(|thread| thread.id == thread_id)
                {
                    let thread = &mut panel.threads[index];
                    if thread.running {
                        return; // 生成待ちの間に次ターンが始まった＝古い要約は捨てる
                    }
                    let line = SharedString::from(line);
                    thread.tier2 = Some(line.clone());
                    let name = thread.name.clone();
                    cx.emit(PanelEvent::SummaryReady { thread: name, tier2: line });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 承認待ちの権限リクエストに、選んだ選択肢の**添字**で応答する（許可/拒否ボタンのクリック）。
    /// respond へ送ると acp_client 側がブロックを解いてエージェントに `Selected(option_id)` を返す。
    /// checkpoint / 色リンクは PermissionRequest 受信時に記録済み（M12-2 の一本道）。
    fn answer_permission(&mut self, option_index: usize, cx: &mut Context<Self>) {
        self.respond_permission(self.active, option_index, cx); // 管制のインライン操作と同じ一本道（P3）
    }

    /// checkpoint へ巻き戻す（blob の変更前内容をディスクへ書き戻し・M12-2）。
    /// 書き込みは背景（Host は UI スレッド禁止）。watch が開いているバッファを自動リロードする。
    fn restore_checkpoint(&mut self, checkpoint_id: i64, cx: &mut Context<Self>) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        let Some(blobs) = storage::default_blobs_dir() else {
            return;
        };
        let host = self.dest_host.clone();
        cx.spawn(async move |panel, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let files = storage.load_checkpoint(checkpoint_id, &blobs)?;
                    let mut restored = 0usize;
                    for (path, content) in files {
                        match content {
                            Some(content) => {
                                host.write_file(
                                    &path,
                                    content.as_bytes(),
                                    host::WriteCondition::Any,
                                )?;
                                restored += 1;
                            }
                            None => {
                                // checkpoint 時点で存在しなかった = 巻き戻しでは消す（local のみ）。
                                let _ = std::fs::remove_file(&path);
                                restored += 1;
                            }
                        }
                    }
                    Ok::<usize, anyhow::Error>(restored)
                })
                .await;
            let _ = panel.update(cx, |_panel, cx| {
                match result {
                    Ok(count) => eprintln!("checkpoint 復元: {count} ファイルを書き戻した"),
                    Err(error) => eprintln!("checkpoint 復元に失敗: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ターンを失敗として畳む（エラー行を積み running を下ろす）。
    fn fail_turn(&mut self, thread_index: usize, message: &str, cx: &mut Context<Self>) {
        let failed = if let Some(thread) = self.threads.get_mut(thread_index) {
            thread.digest = digest_tail(message).or(thread.digest.take()); // P1 素材③
            thread.entries.push(Entry::Agent(SharedString::from(format!(
                "エラー: {message}"
            ))));
            thread.running = false;
            Some((thread.name.clone(), thread.color, thread.muted))
        } else {
            None
        };
        if let Some((thread, color, muted)) = failed {
            cx.emit(PanelEvent::TurnFailed {
                thread,
                color,
                message: SharedString::from(message.to_string()),
                muted,
            });
        }
        cx.notify();
    }

    // ── 描画 ──

    fn render_thread_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let active = self.active;
        // タブ改名（#4）: 編集中タブの index と入力欄（クローンして map 内で参照する）。
        let renaming_index = self.renaming.as_ref().map(|(index, _)| *index);
        let renaming_editor = self.renaming.as_ref().map(|(_, editor)| editor.clone());
        div()
            .flex()
            .items_stretch()
            .w_full()
            .h(px(THREAD_TABS_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(theme.border)
            // Σ 台帳チップ（M12-13）: 全スレッドのトークン累計（live）。DB 側累計は threads.tokens_used。
            .child({
                let total: u64 = self
                    .threads
                    .iter()
                    .map(|thread| thread.tokens_used as u64)
                    .sum();
                div()
                    .id("token-ledger")
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(format!(
                        "Σ {:.1}k",
                        total as f32 / 1000.0
                    )))
            })
            // タブ列だけを横スクロール領域にする（Σ と ＋ は両端で固定）。overflow_x_scroll 単体で
            // 縦ホイールも横送りにマップされる（GPUI div.rs: y 非スクロール時に delta.y→delta.x）。
            .child(
                div()
                    .id("thread-tabs-scroll")
                    .flex()
                    .items_stretch()
                    .flex_1()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tabs_scroll)
                    .children(self.threads.iter().enumerate().map(|(index, thread)| {
                        let is_active = index == active;
                        let color = thread.color;
                        let drop_highlight = theme.bg2;
                        let tab_name = thread.name.clone();
                        div()
                            .id(("thread-tab", index))
                            .flex()
                            .flex_col()
                            .flex_none() // スクロール領域では潰さず自然幅を保つ（数が増えたら溢れさせてスクロール）
                            .h_full()
                            .border_r_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            // Zed 流の即時 hover（bg1）。アクティブは常時 bg1。id で hover 再描画を保証。
                            .hover(|style| style.bg(theme.bg1))
                            .when(is_active, |element| element.bg(theme.bg1))
                            // アクティブスレッドタブ上線 = スレッド色（UI-SPEC §6）
                            .child(div().h(px(2.)).w_full().bg(if is_active {
                                color
                            } else {
                                theme.bg0
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .px(px(12.))
                                    .text_size(px(12.))
                                    .text_color(if is_active { theme.fg0 } else { theme.fg1 })
                                    .child(activity_dot(
                                        ("thtab-dot", index),
                                        8.0,
                                        color,
                                        thread.activity(),
                                    ))
                                    // どのエージェントで話しているか（Claude/Codex…）をアイコンで（設定画面と同じ在庫）。
                                    .child(agent_badge(&thread.agent, 14.0))
                                    .child(if renaming_index == Some(index) {
                                        // 改名中: ラベルを IME 対応の入力欄に差し替える（Enter 確定 / Esc 取消・#4）。
                                        div()
                                            .flex_none()
                                            .min_w(px(90.))
                                            .px(px(4.))
                                            .rounded(px(4.))
                                            .border_1()
                                            .border_color(color)
                                            .bg(theme.bg0)
                                            .on_key_down(cx.listener(
                                                |this, event: &KeyDownEvent, _window, cx| {
                                                    if event.keystroke.key.as_str() == "escape" {
                                                        this.cancel_rename(cx);
                                                    }
                                                },
                                            ))
                                            .children(renaming_editor.clone())
                                            .into_any_element()
                                    } else {
                                        thread.name.clone().into_any_element()
                                    })
                                    // × 閉じる（最後の1枚も閉じられる＝空状態へ）。クリックはタブ切替へ伝播させない。
                                    .child(
                                        div()
                                            .id(("thtab-close", index))
                                            .flex_none()
                                            .px(px(3.))
                                            .rounded(px(4.))
                                            .text_color(theme.fg2)
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style.text_color(theme.fg0).bg(theme.bg2)
                                            })
                                            .child("×")
                                            .tooltip(Tooltip::text(
                                                i18n::t!("agent.close_thread_tip"),
                                                theme.clone(),
                                            ))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _window, cx| {
                                                    cx.stop_propagation();
                                                    this.remove_thread(index, cx);
                                                }),
                                            ),
                                    ),
                            )
                            // 単クリック=切替 / ダブルクリック=インライン改名（#4）。判定は押下即発火の
                            // on_mouse_down で行う: 同居する on_drag の 2px 閾値がクリック合成を握り潰し、
                            // on_click 経由だと二重クリックが取りこぼされるため（div.rs の pending_mouse_down）。
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    if this
                                        .renaming
                                        .as_ref()
                                        .is_some_and(|(editing, _)| *editing == index)
                                    {
                                        return; // 改名中はクリックで切替やフォーカス移動をしない（入力を邪魔しない）
                                    }
                                    if event.click_count == 2 {
                                        this.start_rename(index, window, cx);
                                        return; // 改名開始時は composer へフォーカスを移さない
                                    }
                                    this.switch_thread(index, cx);
                                    this.focus_composer(window, cx); // ⌘W が Agent に効くようフォーカスを寄せる
                                }),
                            )
                            .tooltip(Tooltip::text(
                                i18n::t!("agent.rename_thread_tip"),
                                theme.clone(),
                            ))
                            // Chrome 風ドラッグ並べ替え: タブを掴んで別タブ上で離すと順序が入れ替わる。
                            .on_drag(
                                DraggedThreadTab {
                                    index,
                                    name: tab_name,
                                    color,
                                    theme: theme.clone(),
                                },
                                |dragged, _offset, _window, cx| cx.new(|_| dragged.clone()),
                            )
                            .drag_over::<DraggedThreadTab>(move |style, _dragged, _window, _cx| {
                                style.bg(drop_highlight)
                            })
                            .on_drop(cx.listener(
                                move |this, dragged: &DraggedThreadTab, _window, cx| {
                                    this.move_thread(dragged.index, index, cx);
                                },
                            ))
                    })),
            )
            // スレッド履歴（#5・Claude 拡張風）: クリックで過去スレッド一覧 Picker を開く。
            .child(
                div()
                    .id("thread-history")
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .w(px(30.))
                    .h_full()
                    .text_color(theme.fg2)
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                    .child("🕘")
                    .tooltip(Tooltip::text(i18n::t!("agent.history_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _, _window, cx| {
                            cx.emit(PanelEvent::OpenHistoryRequest)
                        }),
                    ),
            )
            .child(
                div()
                    .id("add-thread")
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .w(px(32.))
                    .h_full()
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                    .child("＋")
                    .tooltip(Tooltip::text(
                        i18n::t!("agent.new_thread_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.add_thread(cx)),
                    ),
            )
            .child(self.render_tabs_view_switcher(cx))
    }

    /// スレッド表示モードを切り替える（Bar ⇄ List）。選択は **user settings.json へ保存**して
    /// 次の起動でも保つ（`default_agent` と同経路。設定画面のトグル化は後続）。
    fn set_tabs_view(&mut self, view: AgentTabsView, cx: &mut Context<Self>) {
        if self.tabs_view != view {
            self.tabs_view = view;
            if let Some(path) = settings_core::user_settings_path() {
                let value = if matches!(view, AgentTabsView::List) { "list" } else { "bar" };
                if let Err(error) = settings_core::persist_user_value(
                    &path,
                    "agent_tabs_view",
                    serde_json::Value::String(value.to_string()),
                ) {
                    eprintln!("タブ表示モードの保存に失敗: {error:#}");
                }
            }
            cx.notify();
        }
    }

    /// Bar/List を切り替える小さなトグル（explorer のビュー切替 idiom。svg は色を直接指定）。
    /// 押すと**もう一方の**モードへ（アイコンは遷移先を示す）。
    fn render_tabs_view_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let (icon, tip, target) = match self.tabs_view {
            AgentTabsView::Bar => (
                "icons/list.svg",
                i18n::t!("agent.view_list"),
                AgentTabsView::List,
            ),
            AgentTabsView::List => (
                "icons/columns-3.svg",
                i18n::t!("agent.view_bar"),
                AgentTabsView::Bar,
            ),
        };
        div()
            .id("tabs-view-switch")
            .group("tabs-view-switch")
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .w(px(28.))
            .h_full()
            .cursor_pointer()
            .hover(|style| style.bg(theme.bg1))
            .child(
                svg()
                    .path(icon)
                    .size(px(14.))
                    .text_color(theme.fg2)
                    .group_hover("tabs-view-switch", |style| style.text_color(theme.fg0)),
            )
            .tooltip(Tooltip::text(tip, theme.clone()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| this.set_tabs_view(target, cx)),
            )
    }

    /// スレッドを縦リストで見せる（List ビュー・「チャットのスペース履歴」風）。Bar と排他。
    /// ヘッダ（Σ トークン + view 切替 + ＋）＋ 色ドット/名前/トークンの行。多数ならリスト内で縦スクロール。
    /// 行クリックでアクティブ切替（下の meta/transcript が追従）。
    fn render_thread_list(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let active = self.active;
        let renaming_index = self.renaming.as_ref().map(|(index, _)| *index);
        let renaming_editor = self.renaming.as_ref().map(|(_, editor)| editor.clone());
        let total: u64 = self
            .threads
            .iter()
            .map(|thread| thread.tokens_used as u64)
            .sum();
        let header = div()
            .flex()
            .items_center()
            .h(px(THREAD_TABS_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .px(px(10.))
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(format!(
                        "Σ {:.1}k",
                        total as f32 / 1000.0
                    ))),
            )
            .child(self.render_tabs_view_switcher(cx))
            .child(
                div()
                    .id("list-add-thread")
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .w(px(32.))
                    .h_full()
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0).bg(theme.bg1))
                    .child("＋")
                    .tooltip(Tooltip::text(
                        i18n::t!("agent.new_thread_tip"),
                        theme.clone(),
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.add_thread(cx)),
                    ),
            );
        let rows = self.threads.iter().enumerate().map(|(index, thread)| {
            let is_active = index == active;
            let color = thread.color;
            let group_name = SharedString::from(format!("thread-row-{index}"));
            div()
                .id(("thread-row", index))
                .group(group_name.clone())
                .flex()
                .items_center()
                .h(px(30.))
                .cursor_pointer()
                .when(is_active, |element| element.bg(theme.bg1))
                .hover(|style| style.bg(theme.bg1))
                // 左 2px 色バー（active のみ・UI-SPEC の選択左バー準拠）。
                .child(div().w(px(2.)).h_full().flex_none().bg(if is_active {
                    color
                } else {
                    theme.bg0
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .flex_1()
                        .min_w_0()
                        .px(px(8.))
                        .child(activity_dot(("row-dot", index), 8.0, color, thread.activity()))
                        .child(agent_badge(&thread.agent, 14.0))
                        .child(if renaming_index == Some(index) {
                            // 改名中: 名前を IME 対応の入力欄に差し替える（Bar タブと同型・#4）。
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(4.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(color)
                                .bg(theme.bg0)
                                .on_key_down(cx.listener(
                                    |this, event: &KeyDownEvent, _window, cx| {
                                        if event.keystroke.key.as_str() == "escape" {
                                            this.cancel_rename(cx);
                                        }
                                    },
                                ))
                                .children(renaming_editor.clone())
                                .into_any_element()
                        } else {
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.5))
                                .text_color(if is_active { theme.fg0 } else { theme.fg1 })
                                .child(thread.name.clone())
                                .into_any_element()
                        })
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(SharedString::from(format!(
                                    "{:.1}k",
                                    thread.tokens_used as f32 / 1000.0
                                ))),
                        )
                        .child(
                            div()
                                .id(("row-close", index))
                                .flex_none()
                                .px(px(3.))
                                .rounded(px(4.))
                                .text_color(theme.fg2)
                                .invisible()
                                .group_hover(group_name.clone(), |style| style.visible())
                                .cursor_pointer()
                                .hover(|style| style.text_color(theme.fg0).bg(theme.bg2))
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.remove_thread(index, cx);
                                    }),
                                ),
                        ),
                )
                // 単クリック=切替 / ダブルクリック=改名（Bar タブと同型・#4）。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        if this
                            .renaming
                            .as_ref()
                            .is_some_and(|(editing, _)| *editing == index)
                        {
                            return; // 改名中は入力を邪魔しない
                        }
                        if event.click_count == 2 {
                            this.start_rename(index, window, cx);
                            return;
                        }
                        this.switch_thread(index, cx);
                        this.focus_composer(window, cx);
                    }),
                )
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .max_h(px(220.))
            .bg(theme.bg0)
            .child(header)
            .child(
                div()
                    .id("thread-list-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.thread_list_scroll)
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_meta(&self, active: bool) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let thread = self.threads.get(self.active);
        let (shown, max) = thread
            .map(|thread| (thread.tokens_shown, thread.tokens_max))
            .unwrap_or((0.0, 0));
        let used = shown.round().max(0.0) as u32; // 表示は補間値（カウントアップ演出）
        let ratio = if max == 0 {
            0.0
        } else {
            (shown / max as f32).clamp(0.0, 1.0)
        };
        let running = thread.map(|thread| thread.running).unwrap_or(false);
        // 末尾が Thinking ブロック中か（ACP の実状態）＝考える中。
        let thinking = thread
            .and_then(|thread| thread.entries.last())
            .map(|entry| matches!(entry, Entry::Thinking(_)))
            .unwrap_or(false);
        // 承認待ちに入った時刻（あれば）＝マスコットの段階演出のトリガ。
        let blocked_since = thread
            .and_then(|thread| thread.pending_permission.as_ref().map(|pending| pending.since));
        // 開発用: SHIRUSHI_MASCOT でモーションを固定（スクショ検証用・release では未評価）。
        let forced = if cfg!(debug_assertions) {
            std::env::var("SHIRUSHI_MASCOT")
                .ok()
                .and_then(|value| match value.as_str() {
                    "plead" => Some(MascotMotion::Plead),
                    "worry" => Some(MascotMotion::Worry),
                    "typing" => Some(MascotMotion::Typing),
                    "think" => Some(MascotMotion::Think),
                    "celebrate" => Some(MascotMotion::Celebrate),
                    "idle" => Some(MascotMotion::Idle),
                    _ => None,
                })
        } else {
            None
        };
        // マスコットの状態: 承認待ち→祈る（15s 以上待たされたら頬に手であわあわへ段階変化）/ 考え中→考える /
        // 生成中→打鍵 / 直近成功→バンザイ / それ以外→寝落ち静止（0%）。
        let motion = forced.unwrap_or_else(|| {
            if let Some(since) = blocked_since {
                if since.elapsed().as_secs() >= 15 {
                    MascotMotion::Worry
                } else {
                    MascotMotion::Plead
                }
            } else if running {
                if thinking {
                    MascotMotion::Think
                } else {
                    MascotMotion::Typing
                }
            } else if self.celebrating {
                MascotMotion::Celebrate
            } else {
                MascotMotion::Idle
            }
        });
        // 左のステータス行（スカスカ対策＋「何が起きてるか」）: 状態テキスト＋実行中は経過秒。
        let elapsed = running
            .then(|| thread.and_then(|thread| thread.turn_started_at))
            .flatten()
            .map(|start| start.elapsed().as_secs_f32());
        let (status_text, status_dim) = if blocked_since.is_some() {
            (i18n::t!("agent.state_blocked"), false)
        } else if running {
            (
                if thinking {
                    i18n::t!("agent.thinking")
                } else {
                    i18n::t!("agent.generating")
                },
                false,
            )
        } else if self.celebrating {
            (i18n::t!("agent.celebrate"), false)
        } else {
            (i18n::t!("agent.idle"), true)
        };
        let status_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_color(if status_dim { theme.fg2 } else { color })
                    .child(status_text),
            )
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

    /// エージェントの実行プラン（`SessionUpdate::Plan`）を transcript 上部の**常設**
    /// チェックリストとして描く（M12-9・VSCode Claude Code 拡張踏襲）。
    /// ● = 進行中（スレッド色）/ ☒ = 完了 / ☐ = 未着手。プランが無ければ何も出さない。
    fn render_plan(&self) -> Option<gpui::AnyElement> {
        let thread = self.threads.get(self.active)?;
        if thread.plan.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        let color = thread.color;
        let total = thread.plan.len();
        let done = thread
            .plan
            .iter()
            .filter(|item| item.status == PlanStatus::Completed)
            .count();
        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(i18n::t!("agent.todos").to_string()),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(format!("{done}/{total}")),
            );
        let mut list = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(3.))
            .px(px(12.))
            .py(px(7.))
            .bg(theme.bg1)
            .border_b_1()
            .border_color(theme.border)
            .child(header);
        for item in &thread.plan {
            let (mark, mark_color, text_color) = match item.status {
                PlanStatus::Completed => ("☒", theme.fg2, theme.fg2),
                PlanStatus::InProgress => ("●", color, theme.fg0),
                PlanStatus::Pending => ("☐", theme.fg2, theme.fg1),
            };
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .text_size(px(11.5))
                    .child(div().flex_none().text_color(mark_color).child(mark))
                    .child(div().text_color(text_color).child(item.content.clone())),
            );
        }
        Some(list.into_any_element())
    }

    fn render_transcript(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let entries = self.threads.get(self.active).map(|thread| &thread.entries);
        // 選択リージョンはフレーム毎に作り直す（このあと render_entry 経由で push される）。
        self.transcript_regions.borrow_mut().clear();
        let mut list = div()
            .id("agent-transcript")
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(13.))
            .px(px(12.))
            .py(px(12.))
            .bg(theme.bg1)
            // ホイール/トラックパッドで遡れる（M13 UX）。ストリーミング追従は on_event 側。
            .overflow_y_scroll()
            .track_scroll(&self.transcript_scroll)
            // ドラッグ選択（M13）: down で開始・move で拡張・up で確定（クリックのみは解除）。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::on_transcript_mouse_down),
            )
            .on_mouse_move(cx.listener(Self::on_transcript_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_transcript_mouse_up));
        if let Some(entries) = entries {
            let count = entries.len();
            for (index, entry) in entries.iter().enumerate() {
                // 末尾エントリだけタイプライタ表示（reveal 文字まで）。他は全文。
                let reveal = if index + 1 == count {
                    Some(self.reveal)
                } else {
                    None
                };
                // 出現時に一度だけ fade in（id 固定なのでストリーミングの再描画では再発火しない）。
                let copy_button = div()
                    .id(("entry-copy", index))
                    .absolute()
                    .top(px(0.))
                    .right(px(0.))
                    .invisible()
                    .group_hover("transcript-entry", |style| style.visible())
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme.bg2)
                    .text_size(px(10.5))
                    .text_color(theme.fg2)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.fg0))
                    .child("⧉")
                    .tooltip(Tooltip::text(i18n::t!("agent.copy_tip"), theme.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            let entry = this
                                .threads
                                .get(this.active)
                                .and_then(|thread| thread.entries.get(index));
                            if let Some(entry) = entry {
                                cx.write_to_clipboard(ClipboardItem::new_string(entry_plain_text(
                                    entry,
                                )));
                            }
                        }),
                    );
                list = list.child(
                    div()
                        .relative()
                        .group("transcript-entry")
                        .child(self.render_entry(index, entry, color, reveal, cx))
                        .child(copy_button)
                        .with_animation(
                            ("transcript-entry", index),
                            Animation::new(std::time::Duration::from_millis(200)),
                            |element, delta| element.opacity(delta),
                        ),
                );
            }
        } else {
            // スレッドを全部閉じた空状態。＋ / ⌘⇧A / レールの ✳ で再開できる。
            list = list.items_center().justify_center().child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.fg2)
                    .child(SharedString::from(i18n::t!("agent.empty"))),
            );
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

    fn render_entry(
        &self,
        index: usize,
        entry: &Entry,
        color: Hsla,
        reveal: Option<usize>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = &self.theme;
        match entry {
            // checkpoint 行（⟲ この時点へ戻す・M12-2）。クリックで blob から全ファイルを書き戻す。
            Entry::Checkpoint { id, label } => {
                let checkpoint_id = *id;
                div()
                    .id(("checkpoint-row", checkpoint_id as usize))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(9.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(color.alpha(0.35))
                    .cursor_pointer()
                    .hover(|style| style.bg(color.alpha(0.10)))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(color)
                            .child("⟲"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(SharedString::from(
                                i18n::t!("agent.checkpoint_row", "label" => label),
                            )),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |panel, _, _window, cx| {
                            panel.restore_checkpoint(checkpoint_id, cx)
                        }),
                    )
                    .into_any_element()
            }
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
                        .child(self.selectable_text(text.clone())),
                )
                .into_any_element(),
            // 思考ブロック（Claude Code 風・印っぽく）: 既定は折り畳み（✳ Thought + 1 行プレビュー）で
            // クリックでいつでも全文展開。実行中の最終ブロックは自動展開 + ✳ pulse で「動いている」を明示。
            Entry::Thinking(text) => {
                let live = reveal.is_some() && self.active_thread_running();
                let expanded = live || self.is_thought_expanded(index);
                let star = if live {
                    div()
                        .flex_none()
                        .text_color(color)
                        .child("✳")
                        .with_animation(
                            ("thinking-star", index),
                            Animation::new(std::time::Duration::from_millis(1000))
                                .repeat()
                                .with_easing(pulsating_between(0.35, 1.0)),
                            |element, delta| element.opacity(delta),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex_none()
                        .text_color(color.alpha(0.7))
                        .child("✳")
                        .into_any_element()
                };
                let header = div()
                    .id(("thinking-header", index))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color.alpha(0.85))
                    .child(star)
                    .child(if live { "Thinking…" } else { "Thought" })
                    .child(
                        div()
                            .text_size(px(8.))
                            .text_color(theme.fg2)
                            .child(if expanded { "▾" } else { "▸" }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_thought(index, cx);
                        }),
                    );
                let mut column = div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(3.))
                    .pl(px(11.))
                    .child(header);
                if expanded {
                    let body = if live {
                        Self::revealed(text, reveal)
                    } else {
                        text.clone()
                    };
                    column = column.child(
                        div()
                            .text_size(px(11.5))
                            .italic()
                            .text_color(theme.fg2)
                            .child(self.push_selectable(body, Vec::new())),
                    );
                } else {
                    column = column.child(
                        div()
                            .text_size(px(11.))
                            .italic()
                            .text_color(theme.fg2)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(thought_preview(text)),
                    );
                }
                div()
                    .flex()
                    .items_stretch()
                    .child(div().w(px(2.)).flex_none().bg(color.alpha(0.45)))
                    .child(column)
                    .into_any_element()
            }
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
            // エージェント本文は markdown を解析してリッチ描画（VSCode Claude Code 拡張踏襲）。
            // タイプライタ（reveal）は解析前に文字数で切る（部分 markdown も pulldown-cmark は許容）。
            Entry::Agent(text) => div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(theme.fg0)
                .children(self.render_markdown(&Self::revealed(text, reveal)))
                .into_any_element(),
        }
    }

    /// markdown テキストをブロック列（GPUI 要素）へ描く。各ブロック = 1 選択リージョン（M13）で、
    /// 装飾は `combine_highlights` で選択背景と安全に合成される。インラインコードは mono 不可
    /// （`HighlightStyle` に font-family が無い）ため syn-mac 色 + 薄背景で表す。表・引用装飾は後続。
    fn render_markdown(&self, text: &str) -> Vec<gpui::AnyElement> {
        let theme = &self.theme;
        markdown::parse(text)
            .into_iter()
            .map(|block| match block {
                markdown::Block::Heading { level, text, spans } => {
                    let size = match level {
                        1 => 16.0,
                        2 => 14.5,
                        3 => 13.5,
                        _ => 12.5,
                    };
                    div()
                        .pt(px(2.))
                        .text_size(px(size))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg0)
                        .child(self.push_selectable(text.into(), self.md_highlights(&spans)))
                        .into_any_element()
                }
                markdown::Block::Paragraph { text, spans } => div()
                    .child(self.push_selectable(text.into(), self.md_highlights(&spans)))
                    .into_any_element(),
                markdown::Block::Code { text, .. } => div()
                    .rounded(px(6.))
                    .bg(theme.bg2)
                    .border_1()
                    .border_color(theme.border)
                    .px(px(9.))
                    .py(px(7.))
                    .font_family("Guguru Sans Code")
                    .text_size(px(11.5))
                    .text_color(theme.fg0)
                    .child(self.push_selectable(text.into(), Vec::new()))
                    .into_any_element(),
                markdown::Block::ListItem {
                    depth,
                    marker,
                    text,
                    spans,
                } => {
                    let bullet: SharedString = match marker {
                        markdown::ListMarker::Bullet => "•".into(),
                        markdown::ListMarker::Ordered(number) => format!("{number}.").into(),
                        markdown::ListMarker::Task(true) => "☑".into(),
                        markdown::ListMarker::Task(false) => "☐".into(),
                    };
                    div()
                        .flex()
                        .gap(px(7.))
                        .pl(px(2.0 + depth as f32 * 16.0))
                        .child(div().flex_none().text_color(theme.fg2).child(bullet))
                        .child(
                            div().flex_1().min_w_0().child(
                                self.push_selectable(text.into(), self.md_highlights(&spans)),
                            ),
                        )
                        .into_any_element()
                }
                markdown::Block::Rule => div()
                    .my(px(2.))
                    .h(px(1.))
                    .bg(theme.border)
                    .into_any_element(),
            })
            .collect()
    }

    /// markdown インラインスパン → GPUI ハイライト（テーマ色を当てる。リンク/コードは syntax 色）。
    fn md_highlights(&self, spans: &[markdown::Span]) -> Vec<(Range<usize>, HighlightStyle)> {
        let syntax = &self.theme.syntax;
        spans
            .iter()
            .map(|span| {
                let style = match span.kind {
                    markdown::SpanKind::Strong => HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                    markdown::SpanKind::Emphasis => HighlightStyle {
                        font_style: Some(gpui::FontStyle::Italic),
                        ..Default::default()
                    },
                    markdown::SpanKind::Strikethrough => HighlightStyle {
                        strikethrough: Some(gpui::StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(self.theme.fg2),
                        }),
                        ..Default::default()
                    },
                    markdown::SpanKind::Code => HighlightStyle {
                        color: Some(syntax.macro_),
                        background_color: Some(self.theme.bg3),
                        ..Default::default()
                    },
                    markdown::SpanKind::Link => HighlightStyle {
                        color: Some(syntax.function),
                        underline: Some(gpui::UnderlineStyle {
                            thickness: px(1.),
                            color: Some(syntax.function),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                };
                (span.range.clone(), style)
            })
            .collect()
    }

    /// アクティブスレッドが実行中か（Thinking の live 判定に使う）。
    fn active_thread_running(&self) -> bool {
        self.threads
            .get(self.active)
            .is_some_and(|thread| thread.running)
    }

    /// この Thinking エントリがユーザーによって展開されているか（thread.id + entry index キー）。
    fn is_thought_expanded(&self, index: usize) -> bool {
        self.threads
            .get(self.active)
            .is_some_and(|thread| self.expanded_thoughts.contains(&(thread.id.clone(), index)))
    }

    /// Thinking の折り畳み/展開をトグルする（ヘッダクリック）。
    fn toggle_thought(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .threads
            .get(self.active)
            .map(|thread| thread.id.clone())
        else {
            return;
        };
        let key = (id, index);
        if !self.expanded_thoughts.remove(&key) {
            self.expanded_thoughts.insert(key);
        }
        cx.notify();
    }

    /// Add context のドロップダウン（プロジェクトのファイル候補。クリックで添付）。
    fn render_context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let accent = self.active_color();
        let attached: Vec<SharedString> = self
            .threads
            .get(self.active)
            .map(|thread| thread.context.clone())
            .unwrap_or_default();
        // fuzzy 絞り込み（全ファイル対象・M12-7）: クエリの各文字が順に現れるものを前方一致優先で。
        let query = self.context_query.to_lowercase();
        let mut matches: Vec<SharedString> = self
            .context_files
            .iter()
            .filter(|path| fuzzy_matches(&path.to_lowercase(), &query))
            .cloned()
            .collect();
        matches.sort_by_key(|path| (!path.to_lowercase().contains(&query), path.len()));
        let display: SharedString = if self.context_query.is_empty() {
            SharedString::from(i18n::t!("agent.context_filter"))
        } else {
            SharedString::from(self.context_query.clone())
        };
        let query_color = if self.context_query.is_empty() {
            theme.fg2
        } else {
            theme.fg0
        };
        let mut menu = div()
            .absolute()
            .left(px(11.))
            .bottom(px(150.))
            .w(px(320.))
            .max_h(px(300.))
            .overflow_hidden()
            .bg(theme.bg2)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .p(px(4.))
            // 入力行（手書きキー流儀・fuzzy は打つたび即時）。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .mx(px(4.))
                    .mb(px(4.))
                    .h(px(24.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .bg(theme.bg1)
                    .border_1()
                    .border_color(accent)
                    .text_size(px(12.))
                    .text_color(query_color)
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(display),
                    )
                    .child(div().flex_none().w(px(1.5)).h(px(13.)).bg(accent)),
            );
        if let Some(focus) = &self.context_focus {
            menu = menu
                .track_focus(focus)
                .on_key_down(cx.listener(Self::on_context_key_down));
        }
        menu.children(
            matches
                .into_iter()
                .take(15)
                .enumerate()
                .map(|(file_index, path)| {
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
                            cx.listener(move |this, _, _window, cx| {
                                this.add_context(path.clone(), cx)
                            }),
                        )
                }),
        )
    }

    /// ＋context 絞り込みのキー入力（Esc 閉じ / backspace / 印字。Enter は先頭候補を添付）。
    fn on_context_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => {
                self.context_menu_open = false;
                self.context_focus = None;
                cx.notify();
            }
            "enter" => {
                let query = self.context_query.to_lowercase();
                let first = self
                    .context_files
                    .iter()
                    .find(|path| fuzzy_matches(&path.to_lowercase(), &query))
                    .cloned();
                if let Some(path) = first {
                    self.add_context(path, cx);
                }
            }
            "backspace" => {
                self.context_query.pop();
                cx.notify();
            }
            _ => {
                let modifiers = event.keystroke.modifiers;
                if modifiers.platform || modifiers.control || modifiers.function {
                    return;
                }
                if let Some(text) = &event.keystroke.key_char {
                    if !text.is_empty() && !text.chars().any(char::is_control) {
                        self.context_query.push_str(text);
                        cx.notify();
                    }
                }
            }
        }
    }

    /// 承認待ちの権限リクエストのカード（composer の直上）。ツール名・編集差分・許可/拒否ボタン。
    /// ターンをブロックしているので、transcript がスクロールしても常に見える位置に置く。
    fn render_permission_card(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let thread = self.threads.get(self.active)?;
        let pending = thread.pending_permission.as_ref()?;
        let theme = self.theme.clone();
        let color = thread.color;

        // 「エディタで開く」（M12-6）: 各 diff を unified で transient タブへ（compact 表示は要約に格下げ）。
        let diff_payloads: Vec<(String, String, String)> = pending
            .diffs
            .iter()
            .map(|diff| {
                let name = std::path::Path::new(&diff.path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| diff.path.clone());
                (
                    name,
                    diff.old_text.clone().unwrap_or_default(),
                    diff.new_text.clone(),
                )
            })
            .collect();
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
            .when(!diff_payloads.is_empty(), |card| {
                card.child(
                    div()
                        .id("permission-open-diff")
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .px(px(7.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(10.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child(SharedString::from(i18n::t!("agent.open_diff_tab")))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |_panel, _, _window, cx| {
                                for (name, old_text, new_text) in diff_payloads.clone() {
                                    cx.emit(PanelEvent::OpenDiffRequest {
                                        title: name,
                                        old_text,
                                        new_text,
                                    });
                                }
                            }),
                        ),
                )
            })
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
                            .child(SharedString::from(i18n::t!("agent.permission_needed"))),
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
        let buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .children(
                pending
                    .options
                    .iter()
                    .enumerate()
                    .map(|(index, option)| permission_button(index, option, color, &theme, cx)),
            );
        card = card.child(buttons);

        Some(card.into_any_element())
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let drop_glow = color.alpha(0.14); // ファイル D&D 中のハイライト
        let activity = self
            .threads
            .get(self.active)
            .map(|thread| thread.activity())
            .unwrap_or(ThreadActivity::Idle);
        let thread_name = self
            .threads
            .get(self.active)
            .map(|thread| thread.name.clone())
            .unwrap_or_default();
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
            .id("composer-drop")
            .flex_none()
            .px(px(12.))
            .py(px(10.))
            .bg(theme.bg1)
            .border_t_1()
            .border_color(theme.border)
            // ファイル D&D → @メンション参照。Finder（ExternalPaths）とエクスプローラ（DraggedFile）両対応。
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_glow))
            .drag_over::<DraggedFile>(move |style, _, _, _| style.bg(drop_glow))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                for path in paths.paths() {
                    this.add_context_path(path, cx);
                }
            }))
            .on_drop(cx.listener(|this, dragged: &DraggedFile, _window, cx| {
                this.add_context(dragged.path.clone(), cx);
            }))
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
                            .child(activity_dot("dest-dot", 8.0, color, activity))
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
                                    .tooltip(Tooltip::text(
                                        i18n::t!("agent.attach_tip"),
                                        theme.clone(),
                                    ))
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
                    .child(
                        div()
                            .h(px(COMPOSER_INPUT_HEIGHT))
                            .child(self.composer.clone()),
                    )
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
                                        SharedString::from(i18n::t!("agent.hint_submit_enter"))
                                    } else {
                                        SharedString::from(i18n::t!("agent.hint_submit_cmd"))
                                    })
                                    .tooltip(Tooltip::text(
                                        i18n::t!("agent.ime_tip"),
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
                                    .child(SharedString::from(if self.submit_on_enter {
                                        i18n::t!("agent.send_enter")
                                    } else {
                                        i18n::t!("agent.send_cmd")
                                    }))
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
        self.window_active = active; // 完了音の「非アクティブ時のみ」判定が読む（P2）
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
            // transcript 選択の ⌘C / Esc（選択がある時だけ composer より優先・M13）。
            .on_key_down(cx.listener(Self::on_panel_key_down))
            // Agent エリアのどこをクリックしても composer にフォーカス（＝⌘W がスレッドに効く）。
            // 子（タブ/ボタン）が処理した後の bubble で拾う。既に focus 済みなら no-op。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus_composer(window, cx)),
            )
            .child(match self.tabs_view {
                AgentTabsView::Bar => self.render_thread_tabs(cx).into_any_element(),
                AgentTabsView::List => self.render_thread_list(cx),
            })
            .child(self.render_meta(active))
            // エージェントの実行プラン（あれば transcript 上部に常設・M12-9）。
            .children(self.render_plan())
            .child(self.render_transcript(cx))
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
    /// 承認待ち＝祈る（手を組んで「頼む…通って」）。
    Plead,
    /// 承認待ちが長引いた＝頬に手であわあわ（より焦る）。
    Worry,
}

/// ローディング・マスコット（猫耳コーダー娘）。生成中/祝福中はフィルムストリップを `steps()` 再生
/// （overflow-hidden 枠を横スクロール）、アイドルは静止。アニメは実行/祝福中のみ＝idle CPU 0% を保つ
/// （[`pulsing_dot`] と同じ設計）。
///
/// フレームは **image→video（Kling v3.0・start=end でループ）でキャラ固定のまま生成 → 共通窓で切り出し
/// → 量子化** した「本物の中割り」なので、text→image を並べた時のような軸ブレ（シェイク）が無い。
fn render_mascot(motion: MascotMotion, active: bool) -> gpui::AnyElement {
    render_mascot_sized(motion, active, 64.0, "panel")
}

/// 編隊の集約気分マスコット（管制の監督バー・P3。JOURNAL 2026-07-23「集約気分 1 匹」の回収）。
/// 編隊の**最悪状態**に追従: 承認待ちあり→祈る（15s 超→頬に手であわあわ）/ 実行中→打鍵 / 静穏→うとうと。
/// `tag` はアニメ要素 id の名前空間（agent panel の常駐マスコットと衝突させない）。
pub fn fleet_mood_mascot(
    any_blocked: bool,
    blocked_long: bool,
    any_working: bool,
    active: bool,
    height: f32,
) -> gpui::AnyElement {
    let motion = if blocked_long {
        MascotMotion::Worry
    } else if any_blocked {
        MascotMotion::Plead
    } else if any_working {
        MascotMotion::Typing
    } else {
        MascotMotion::Idle
    };
    render_mascot_sized(motion, active, height, "fleet-mood")
}

fn render_mascot_sized(
    motion: MascotMotion,
    active: bool,
    height: f32,
    tag: &'static str,
) -> gpui::AnyElement {
    const N: usize = 15; // 各ストリップのコマ数
    #[allow(non_snake_case)]
    let H: f32 = height; // 表示高さ
    #[allow(non_snake_case)]
    let W: f32 = H * 60.0 / 72.0; // 1 コマのアスペクト（共通 60x72）
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/mascot");
    let window = |child: gpui::AnyElement| {
        div()
            .w(px(W))
            .h(px(H))
            .flex_none()
            .overflow_hidden()
            .child(child)
            .into_any_element()
    };
    // 単一フレーム画像（idle.png）を等倍表示。
    let single = |file: &str| {
        window(
            img(PathBuf::from(format!("{dir}/{file}")))
                .h(px(H))
                .w(px(W))
                .into_any_element(),
        )
    };
    // ストリップの先頭コマだけ静止表示（overflow 窓＝ml 0）。非アクティブ時の凍結用＝再描画ゼロ。
    let frame0 = |file: &str| {
        window(
            img(PathBuf::from(format!("{dir}/{file}")))
                .h(px(H))
                .w(px(W * N as f32))
                .into_any_element(),
        )
    };
    // ストリップを steps 再生（横スクロール）。id は tag で名前空間化（panel と管制の 2 匹が共存できる）。
    let anim = |file: &str, id: &'static str| {
        window(
            img(PathBuf::from(format!("{dir}/{file}")))
                .h(px(H))
                .w(px(W * N as f32))
                .with_animation(
                    SharedString::from(format!("{tag}-{id}")),
                    Animation::new(std::time::Duration::from_millis(100 * N as u64)).repeat(),
                    move |element, delta| {
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
        (MascotMotion::Plead, true) => anim("plead-strip.png", "mascot-plead"),
        (MascotMotion::Plead, false) => frame0("plead-strip.png"),
        (MascotMotion::Worry, true) => anim("worry-strip.png", "mascot-worry"),
        (MascotMotion::Worry, false) => frame0("worry-strip.png"),
    }
}

/// スレッド色のドット。実行中は breathing で pulse（mock: 1.6s）。停止中は静止。
/// スレッド表示の初期モード。**開発用 `SHIRUSHI_TABS_VIEW` を最優先**（list/bar・スクショ検証用）、
/// 無ければ**保存値**（settings.json の `agent_tabs_view`）、それも無ければ Bar。
fn initial_tabs_view(setting: &str) -> AgentTabsView {
    match std::env::var("SHIRUSHI_TABS_VIEW").ok().as_deref() {
        Some("list") => AgentTabsView::List,
        Some("bar") => AgentTabsView::Bar,
        _ if setting == "list" => AgentTabsView::List,
        _ => AgentTabsView::Bar,
    }
}

/// ターン完了の通知音（macOS の system sound を `afplay` で鳴らす・設定 `completion_sound`）。
/// 既存の shell-out（open/osascript/trash と同じ）で**依存ゼロ**。独自チャイム同梱は後続。
/// **短命スレッドで `status()` まで待って子を刈る**（zombie を残さない）ので UI スレッドは即戻る。
/// イベント駆動（完了時のみ）＝idle 予算に影響しない。
fn play_completion_sound() {
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        use std::process::{Command, Stdio};
        let result = Command::new("/usr/bin/afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(error) = result {
            eprintln!("完了音の再生に失敗: {error}");
        }
    });
}

/// 折り畳んだ Thinking の 1 行プレビュー（先頭行を ~64 文字で切る。続きがあれば … を付す）。
fn thought_preview(text: &str) -> SharedString {
    let first_line = text.lines().next().unwrap_or("").trim();
    let preview: String = first_line.chars().take(64).collect();
    let truncated = first_line.chars().count() > 64 || text.lines().nth(1).is_some();
    if truncated {
        format!("{preview}…").into()
    } else {
        preview.into()
    }
}

/// スレッド状態ドット（タブ / List / beacon / レール / フッター / ⌘O で共用・#）。**色相は常にスレッド
/// 識別色**（UI-SPEC §1.3）で、状態は「形 × 動き」の 2 軸で見せる（色を状態に使わない・mock のグリフに準拠）:
/// Idle=淡・静止（○）/ Working=満・脈動（●）/ Done=リング・静止（✓）/ Blocked=**半円**・速い脈動（◐・中断 Done は中空）。
/// リング枠のスペースは常に確保し、状態が変わってもレイアウトが揺れないようにする。
pub fn activity_dot(
    id: impl Into<gpui::ElementId>,
    diameter: f32,
    color: Hsla,
    activity: ThreadActivity,
) -> gpui::AnyElement {
    // Working = 点字スピナー（herdr 由来の「形と動き」・色はスレッド識別色のまま＝§1.3 維持・2026-07-25）。
    // 満円脈動よりも「進行中」が一瞬で読める。パターン比較は mock/working-anim-patterns.html。
    if matches!(activity, ThreadActivity::Working) {
        return working_spinner(id, diameter, color);
    }
    let ringed = matches!(activity, ThreadActivity::Done { .. });
    let half = matches!(activity, ThreadActivity::Blocked); // 承認待ち = 半円（mock ◐）
    let pulse = matches!(activity, ThreadActivity::Working | ThreadActivity::Blocked);
    let urgent = matches!(activity, ThreadActivity::Blocked);
    let dim = matches!(activity, ThreadActivity::Idle);
    let hollow = matches!(activity, ThreadActivity::Done { interrupted: true });

    let dot = if half {
        // 承認待ち（Blocked）= 半円（ドーム・底辺が直径）。状態を「色」でなく「形」で示す（§1.3・mock ◐）。
        // 角丸半径は小径で潰れる（7px で横カプセル化）ので、**フル円を高さ半分の箱で clip**して上半分だけ
        // 見せる＝どのサイズでもくっきり半円。フレーム内で縦中央に載る。
        div()
            .w(px(diameter))
            .h(px(diameter / 2.0))
            .overflow_hidden()
            .flex_none()
            .child(div().size(px(diameter)).rounded(px(diameter / 2.0)).bg(color))
    } else {
        let base = div()
            .size(px(diameter))
            .rounded(px(diameter / 2.0))
            .flex_none();
        if hollow {
            base.border_1().border_color(color) // 中断で終わった Done は中空（塗り無し・輪郭のみ）
        } else {
            base.bg(color)
        }
    };

    let outer = diameter + 6.0;
    let framed = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(outer))
        .rounded(px(outer / 2.0))
        .border_1()
        .border_color(if ringed { color } else { color.alpha(0.0) })
        .child(dot);

    if pulse {
        let period = if urgent { 900 } else { 1600 };
        let low = if urgent { 0.45 } else { 0.35 };
        framed
            .with_animation(
                id,
                Animation::new(std::time::Duration::from_millis(period))
                    .repeat()
                    .with_easing(pulsating_between(low, 1.0)),
                |element, delta| element.opacity(delta),
            )
            .into_any_element()
    } else if dim {
        framed.opacity(0.35).into_any_element()
    } else {
        framed.into_any_element()
    }
}

/// Working の点字スピナー（⠋⠙⠹…・herdr の表現を規律に翻訳: **色はスレッド識別色のまま**動きだけ借りる）。
/// GPUI はアニメでテキストを差し替えられないため、グリフ列を 1 コマ幅の窓でスライドさせる
/// （マスコットのフィルムストリップと同じ手法）。外枠は `activity_dot` の frame と同寸＝置換してもレイアウト不変。
pub fn working_spinner(
    id: impl Into<gpui::ElementId>,
    diameter: f32,
    color: Hsla,
) -> gpui::AnyElement {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let outer = diameter + 6.0; // activity_dot と同じリング枠寸
    let slot = diameter + 4.0; // 1 コマの箱（グリフを中央寄せ）
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(outer))
        .child(
            div().w(px(slot)).h(px(slot)).overflow_hidden().flex_none().child(
                div()
                    .flex()
                    .flex_none()
                    .w(px(slot * FRAMES.len() as f32))
                    .h(px(slot))
                    .children(FRAMES.iter().map(|frame| {
                        div()
                            .w(px(slot))
                            .h(px(slot))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(slot))
                            .line_height(px(slot))
                            .text_color(color)
                            .child(*frame)
                    }))
                    .with_animation(
                        id,
                        Animation::new(std::time::Duration::from_millis(1200)).repeat(),
                        move |element, delta| {
                            let index = ((delta * FRAMES.len() as f32).floor() as usize)
                                .min(FRAMES.len() - 1);
                            element.ml(px(-(slot * index as f32)))
                        },
                    ),
            ),
        )
        .into_any_element()
}

/// スレッドが話すエージェントのブランドアイコン（タブ / List 行用）。ブランド在庫が無いものは
/// モノグラム角丸で代替（設定画面と同じ見た目・出所は `AgentKind::brand`）。アイコン＝「どのエージェントか」
/// の識別で、スレッド色（＝どのスレッドか）とは別軸。svg は親の text_color を継承しないので直接指定する。
pub fn agent_badge(label: &str, size: f32) -> gpui::AnyElement {
    let (icon, monogram, brand) = AgentKind::by_label(label)
        .map(|agent| agent.brand())
        .unwrap_or((None, "✳", 0x88_88_88));
    match icon {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_none()
            .text_color(gpui::rgb(brand))
            .into_any_element(),
        None => div()
            .flex_none()
            .size(px(size))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgb(brand))
            .text_size(px(size * 0.55))
            .font_weight(FontWeight::BOLD)
            .text_color(gpui::white())
            .child(SharedString::from(monogram))
            .into_any_element(),
    }
}

/// 指定カテゴリの広告設定の選択肢名一覧（無ければ `None`＝静的既定にフォールバック）。
fn config_choice_names(thread: &Thread, category: ConfigCategory) -> Option<Vec<SharedString>> {
    let config = thread
        .configs
        .iter()
        .find(|config| config.category == category)?;
    if config.choices.is_empty() {
        return None;
    }
    Some(
        config
            .choices
            .iter()
            .map(|(_, name)| SharedString::from(name.clone()))
            .collect(),
    )
}

/// 選んだ表示名から value_id を引いて `session/set_config_option` を送る（広告設定 + セッションがある時のみ）。
/// 広告が無ければ何もしない（＝ラベル切替のみで実反映はできない）。
fn send_set_config(thread: &Thread, category: ConfigCategory, value_name: &SharedString) {
    let Some(config) = thread
        .configs
        .iter()
        .find(|config| config.category == category)
    else {
        return;
    };
    let Some((value_id, _)) = config
        .choices
        .iter()
        .find(|(_, name)| name.as_str() == value_name.as_ref())
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
        rows = rows.child(
            div()
                .text_color(tone_color)
                .child(format!("{prefix} {text}")),
        );
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
        out.push((tone, i18n::t!("agent.more_lines", "n" => lines.len() - max)));
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
pub fn human_tokens(count: u32) -> String {
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
/// 設定の既定エージェント（`AGENT_LABELS` にある表示名。無効/未設定なら "Claude Code"）。
fn default_agent_name(cx: &App) -> SharedString {
    let name = settings::get(cx).default_agent;
    if acp_client::AGENT_LABELS.contains(&name.as_str()) {
        SharedString::from(name)
    } else {
        SharedString::from("Claude Code")
    }
}

/// storage の1 turn 行 (role, content) を [`Entry`] へ（復元・#5。set_storage と同じ対応）。
fn entry_from_turn((role, content): (String, String)) -> Entry {
    match role.as_str() {
        "user" => Entry::User(content.into()),
        "thinking" => Entry::Thinking(content.into()),
        "step" => Entry::Step {
            tool: content.into(),
            args: SharedString::default(),
            result: None,
        },
        "checkpoint" => {
            let (id, label) = content.split_once('\t').unwrap_or(("0", "checkpoint"));
            Entry::Checkpoint {
                id: id.parse().unwrap_or(0),
                label: label.to_string().into(),
            }
        }
        _ => Entry::Agent(content.into()),
    }
}

/// storage の1スレッドを復元する（履歴オープン・#5）。id/name/色から Thread を作り直近 turn を積む。
fn thread_from_storage(
    storage: &storage::Storage,
    id: &str,
    name: &str,
    color_index: usize,
) -> Thread {
    let mut thread = Thread::empty(name.to_string(), color_index);
    thread.id = id.to_string();
    let turns = storage.load_recent_turns(id, 200).unwrap_or_default();
    thread.entries = turns.into_iter().map(entry_from_turn).collect();
    thread.persisted_entries = thread.entries.len();
    thread
}

fn seed_threads() -> Vec<Thread> {
    let rope = Thread {
        id: new_thread_id(),
        persisted_entries: 0,
        touched_files: Vec::new(),
        plan: Vec::new(),
        name_is_custom: false,
        name: "rope設計".into(),
        color: thread_color(0),
        running: false,
        done: None,
        turn_started_at: None,
        // 種スレッドにも実感のある時刻を（offscreen 検証で「開始/入力」表示が写る）。
        created_at_ms: now_unix_ms() - 2 * 60 * 60 * 1000,
        last_input_at_ms: Some(now_unix_ms() - 5 * 60 * 1000),
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
        // 遷移スナップショット（P1）の種: offscreen 検証で herd 行/セル帯に digest が写る。
        digest: Some("結論: MVP は ropey。Buffer trait で後から差し替え可能に。".into()),
        muted: false,
        tier2: None,
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
                r#"## 結論: MVP は **ropey**

Zed の `text` crate は CRDT（協調編集）前提の **sum-tree** で、単独編集の MVP には複雑さが釣り合いません。

- `ropey` は API が安定・docs も厚い
- undo 履歴を rope と独立に持てる → 移行コストが低い
- `Buffer` を trait で切れば後から差し替え可能

```rust
pub trait Buffer {
    fn insert(&mut self, offset: usize, text: &str);
}
```

後で sum-tree 系へ寄せる道も残せます。"#
                    .into(),
            ),
        ],
    };
    vec![
        rope,
        Thread::empty("tab色分け", 1),
        Thread::empty("gpui起動", 2),
    ]
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
    fn digest_tail_takes_last_sentences() {
        // 最終段落の末尾 2 文だけを 1 行に畳む（P1 素材②）。
        let text = "## 結論\n\n途中の説明。これは長い前置き。最後の判断はこう。次はテストを書く。";
        assert_eq!(
            digest_tail(text).unwrap().as_ref(),
            "最後の判断はこう。 次はテストを書く。"
        );
        // 英文はピリオド+空白で区切る（`src/main.rs` のようなパス内の `.` では切らない）。
        let english = "Refactored src/main.rs cleanly. All tests pass now.";
        assert_eq!(digest_tail(english).unwrap().as_ref(), english);
        // 空・空白のみは None。
        assert_eq!(digest_tail("  \n "), None);
        // 長文は末尾 140 字へ … 付きで畳む。改行は空白 1 つへ。
        let long = format!("{}。おわり", "あ".repeat(200));
        let digest = digest_tail(&long).unwrap();
        assert!(digest.starts_with('…'));
        assert!(digest.chars().count() <= 141);
        assert!(!digest_tail("一行目\nまだ続く").unwrap().contains('\n'));
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
        assert!(result
            .iter()
            .all(|(tone, _)| matches!(tone, DiffTone::Added)));
    }

    #[test]
    fn diff_caps_large_blocks_with_summary() {
        // 30 行の追加 → 14 行 + 「他 N 行」の要約 1 行（文言はロケール依存なので数字で見る）。
        let new: String = (0..30).map(|i| format!("line{i}\n")).collect();
        let result = compact_line_diff(Some(""), &new);
        assert_eq!(result.len(), 15); // 14 + 要約
        let summary = &result.last().unwrap().1;
        assert!(summary.contains("16"), "要約行に残り行数が入る: {summary}");
        assert!(
            !summary.starts_with("line"),
            "要約行は本文でない: {summary}"
        );
    }

    #[test]
    fn human_tokens_formats_thousands() {
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(23_400), "23.4k");
        assert_eq!(human_tokens(200_000), "200k");
    }
}
