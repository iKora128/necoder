//! agent_panel — 右ドックのエージェントパネル（M4。差別化の本丸）。
//!
//! UI-SPEC §6: スレッド = 固有色のタブ。宛先チップ（スレッド名 + project ⎇ branch）と
//! トークン常時表示は必須要件（`docs/BACKGROUND.md` の痛点が原点）。アクティブスレッドの色が
//! タブ下線・トークンメーター・msg-user 左縁・thinking 左縁・composer 枠・送信ボタン・宛先チップの
//! ドットへ**一斉に貫通**する（= 混戦対策の本体）。
//!
//! composer は [`editor_view::EditorView`] の平坦モードを再利用する（IME・複数カーソル・undo を共通化）。
//! ACP との実接続（session/prompt/stream）は次段（B2）。現状はスレッド構造 + 送信でのユーザー発話追加まで。
//!
//! ## 伸縮テキストの作法（GPUI/taffy の罠・2026-08-09）
//!
//! flex **行**の中で折り返させたい本文には `flex_1()` と `min_w_0()` を**必ずセットで**付ける。
//! ACP のツール名・パス・承認タイトルはエージェント任せで長さの上限が無いため、ここを外すと崩れる。
//!
//! - `min_w_0()` だけ … 幅 0 に潰れて**1 文字ずつ改行**される（行が広い AI 全画面ほど出やすい）
//! - どちらも無し … GPUI のテキストは min-content を max-content と同値で返す＝縮まないので、
//!   パネルの外へはみ出して切れる
//!
//! 実測（AI 全画面・行幅 1364px・同一本文で style だけ変えた比較）:
//! `min_w_0` のみ → **0px × 2540px** / `flex_1` + `min_w_0` → 1260px × 20px / 無指定 → 826px × 20px。

use acp_client::{
    AgentEvent, AgentKind, ConfigCategory, ConfigOption, ElicitationField, PermissionChoice,
    PermissionDiff, PermissionKind, PlanItem, PlanStatus, SessionCommand, ToolCallInfo, TurnEnd,
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
    actions, canvas, div, list, prelude::*, px, svg, Animation, AnimationExt, App, ClipboardItem,
    Context, Corners, CursorStyle, Entity, ExternalPaths, FocusHandle, Focusable, FollowMode,
    FontWeight, HighlightStyle, Hsla, IntoElement, KeyDownEvent, ListAlignment, ListOffset,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, RenderImage,
    ScrollHandle, SharedString, StyleRefinement, StyledText, TextLayout, Window,
};
use host::{Host, LocalHost};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use theme_core::{claude_bullet, thread_color, Theme};
use ui::{DraggedFile, Tooltip};

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
/// composer 入力欄の高さ。上縁ハンドルのドラッグで [MIN, MAX] にリサイズできる。
/// 既定は ~3 行（固定 68px ＝ ~2 行は「狭すぎ・伸ばせない」、104px ＝ ~4 行は
/// 「若干広い」だった経緯・2026-08-26）。
const COMPOSER_INPUT_DEFAULT: f32 = 86.0;
const COMPOSER_INPUT_MIN: f32 = 46.0;
const COMPOSER_INPUT_MAX: f32 = 420.0;
/// composer のモデルセレクタに並べる候補（クリックでアクティブスレッドに設定）。
/// **これはフォールバック** — ACP エージェントが `session/set_config_option` で広告してきた
/// 一覧があればそちらが優先される（`selector_options`）。広告しないエージェント用の既定リスト。
/// 選択は `send_set_config` で実際にエージェントへ送る（表示だけではない）。
const CLAUDE_MODELS: &[&str] = &[
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-sonnet-5",
    "claude-haiku-4-5",
    "claude-fable-5",
];
/// Codex のセッション広告を受け取る前だけ使う候補。接続後は `config_options` の動的一覧に置き換わる。
/// 表示名は codex-acp の Model config（Codex `model/list` の `displayName`）に合わせる。
const CODEX_MODELS: &[&str] = &[
    "GPT-5.6-Sol",
    "GPT-5.6-Terra",
    "GPT-5.6-Luna",
    "GPT-5.5",
    "GPT-5.4",
    "GPT-5.4-Mini",
];
/// モデル在庫を静的に持てないエージェントは、ACP 広告が届くまで vendor の既定を使う。
const AUTO_MODELS: &[&str] = &["auto"];
/// 権限モード（Claude Code 相当。実配線は ACP の SessionMode／set_mode 経由で継続課題）。
const PERMISSION_MODES: &[&str] = &["default", "accept edits", "bypass permissions", "plan"];
/// 推論の effort（Zed 下部コントロール相当）。モデル候補と同じくエージェントの広告が優先。
/// `xhigh` は high と max の間（Opus 5 / Opus 4.8 / Sonnet 5 等。コーディング/エージェント用途の推奨値）。
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

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
            Selector::Agent => &[],
            Selector::Mode => PERMISSION_MODES,
            Selector::Model => CLAUDE_MODELS,
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
    /// ステップ（⏺ ツール名 + 引数 → ⎿ 結果）。Edit は before/after 差分、Bash 等は出力を持てる。
    Step {
        /// ACP の相関 ID（`ToolCallUpdate` で出力/差分を後追い反映するため）。復元/デモでは None。
        id: Option<SharedString>,
        tool: SharedString,
        args: SharedString,
        result: Option<SharedString>,
        /// ファイル編集の before/after（Edit 系。空なら差分表示なし）。
        diffs: Vec<PermissionDiff>,
    },
    /// エージェントの本文（結論など）。
    Agent(SharedString),
    /// checkpoint（この時点へ戻せる・M12-2）。承認直前の変更前内容が blob に入っている。
    Checkpoint { id: i64, label: SharedString },
}

/// 現在ビューポートの先頭より前にある、もっとも近いユーザー発話を返す。
/// transcript の直下は `Entry` と同じ順なので、返した index をそのまま `ScrollHandle` に渡せる。
fn previous_user_entry_index(entries: &[Entry], top_item: usize) -> Option<usize> {
    entries
        .iter()
        .take(top_item.min(entries.len()))
        .rposition(|entry| matches!(entry, Entry::User(_)))
}

/// transcript の選択可能リージョン（毎フレーム再構築・M13）。`layout` は同フレームの
/// paint 後に有効になる（transcript は全リージョン描画 = 登録したものは必ず paint される）。
/// リージョンの同一性は**登録順のインデックス**（= このベクタ内の位置）。1 エントリが markdown で
/// 複数ブロック（＝複数リージョン）に割れても選択が破綻しないよう、エントリ index ではなく
/// リージョン index を選択の基準にする。
struct SelectableRegion {
    /// 表示テキストそのもの（コピーはここから切る。offset はこのテキスト基準）。
    text: SharedString,
    /// hash cache 上の永続 View。hit test 時に最新の TextLayout を読む。
    styled: Entity<CachedStyledTextView>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SyntaxCacheKey {
    language: lang::LanguageId,
    content_hash: u64,
    len: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ContentCacheKey {
    content_hash: u64,
    len: usize,
}

fn content_cache_key(text: &str) -> ContentCacheKey {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    ContentCacheKey {
        content_hash: hasher.finish(),
        len: text.len(),
    }
}

/// CommonMark の解析結果を本文 hash で再利用する小さな LRU。ストリーム途中の断片も上限で
/// 古いものから捨てるため、長いターンでもキャッシュが無制限に増えない。
#[derive(Default)]
struct MarkdownBlockCache {
    blocks: HashMap<ContentCacheKey, (SharedString, Arc<Vec<markdown::Block>>)>,
    order: VecDeque<ContentCacheKey>,
}

impl MarkdownBlockCache {
    const CAPACITY: usize = 192;

    fn parse(&mut self, text: &str) -> Arc<Vec<markdown::Block>> {
        let key = content_cache_key(text);
        if let Some((source, blocks)) = self.blocks.get(&key) {
            if source.as_ref() == text {
                return blocks.clone();
            }
        }
        let blocks = Arc::new(markdown::parse(text));
        while self.order.len() >= Self::CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.blocks.remove(&expired);
            }
        }
        self.order.push_back(key);
        self.blocks
            .insert(key, (SharedString::from(text.to_owned()), blocks.clone()));
        blocks
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StyledTextCacheKey {
    content: ContentCacheKey,
    styles_hash: u64,
    styles_len: usize,
    /// 同じ本文が複数箇所にある場合も bounds を共有しないための描画順スロット。
    region: usize,
}

/// 本文 hash ごとの独立 View。Entity の Render cache に `StyledText` と shape/wrap 済み layout が
/// 残るため、親 transcript が1Hz表示などで再描画されても本文 Entity は再構築されない。
struct CachedStyledTextView {
    text: SharedString,
    base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    selection: Option<(Range<usize>, HighlightStyle)>,
    layout: TextLayout,
}

impl CachedStyledTextView {
    fn set_selection(
        &mut self,
        selection: Option<(Range<usize>, HighlightStyle)>,
        cx: &mut Context<Self>,
    ) {
        if self.selection == selection {
            return;
        }
        self.selection = selection;
        cx.notify();
    }
}

impl Render for CachedStyledTextView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let highlights = snap_highlights_to_char_boundaries(
            &self.text,
            gpui::combine_highlights(self.base_highlights.clone(), self.selection.clone())
                .collect(),
        );
        let mut styled = StyledText::new(self.text.clone());
        if !highlights.is_empty() {
            styled = styled.with_highlights(highlights);
        }
        self.layout = styled.layout().clone();
        styled
    }
}

#[derive(Default)]
struct StyledTextEntityCache {
    items: HashMap<
        StyledTextCacheKey,
        (
            SharedString,
            Vec<(Range<usize>, HighlightStyle)>,
            Entity<CachedStyledTextView>,
        ),
    >,
    order: VecDeque<StyledTextCacheKey>,
}

impl StyledTextEntityCache {
    const CAPACITY: usize = 512;
}

/// 生成中の本文で「今ここまで打った」接頭辞（先頭 `reveal` 文字）。全部見えていれば元の
/// `SharedString` をそのまま返し（clone だけで再確保しない）、途中なら char 単位で切る。
fn revealed_prefix(text: &SharedString, reveal: usize) -> SharedString {
    if reveal >= text.chars().count() {
        text.clone()
    } else {
        text.chars().take(reveal).collect::<String>().into()
    }
}

/// `index` 以下で最も近い UTF-8 文字境界（内側へ丸める）。
fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// `index` 以上で最も近い UTF-8 文字境界（内側へ丸める）。
fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    let len = text.len();
    if index >= len {
        return len;
    }
    while index < len && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// gpui の `compute_runs`→`layout_line` は run 境界が本文の UTF-8 文字境界に乗らない/範囲外だと
/// `split_at` でプロセスごと abort する（release では `compute_runs` の `debug_assert` が効かない）。
/// ハイライトの由来（tree-sitter / markdown / 描画をまたいで再利用される選択オフセット）が本文と
/// 完全一致する保証を全経路で持てないため、gpui へ渡す直前にここで各 run の端点を内側の文字境界へ
/// 丸め（start は次境界・end は前境界へ）、潰れた/範囲外の run は捨てる。gpui は AGPL 対象外の
/// pinned 依存で改変できないため、防御はこの seam に置く。
fn snap_highlights_to_char_boundaries(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let len = text.len();
    highlights
        .into_iter()
        .filter_map(|(range, style)| {
            let start = ceil_char_boundary(text, range.start.min(len));
            let end = floor_char_boundary(text, range.end.min(len));
            (start < end).then_some((start..end, style))
        })
        .collect()
}

/// ACP の再描画（ストリーム・マスコット時計）で同じコードを再解析しないための小さな LRU。
/// Query/grammar 自体も言語ごとに 1 回だけ構築する。
enum CachedSyntaxHighlighter {
    Standard(lang::Highlighter),
    Markdown(lang::IncrementalHighlighter),
}

impl CachedSyntaxHighlighter {
    fn new(language: lang::LanguageId) -> Option<Self> {
        if language == lang::LanguageId::Markdown {
            lang::IncrementalHighlighter::for_language(language).map(Self::Markdown)
        } else {
            lang::Highlighter::for_language(language).map(Self::Standard)
        }
    }

    fn highlight(&mut self, text: &str) -> Vec<lang::HighlightSpan> {
        match self {
            Self::Standard(highlighter) => highlighter.highlight(text),
            Self::Markdown(highlighter) => {
                highlighter.reparse_full(text);
                highlighter.spans(text, 0..text.len())
            }
        }
    }
}

#[derive(Default)]
struct SyntaxHighlightCache {
    highlighters: HashMap<lang::LanguageId, CachedSyntaxHighlighter>,
    spans: HashMap<SyntaxCacheKey, Vec<lang::HighlightSpan>>,
    order: VecDeque<SyntaxCacheKey>,
}

impl SyntaxHighlightCache {
    const CAPACITY: usize = 256;

    fn highlight(&mut self, language: lang::LanguageId, text: &str) -> Vec<lang::HighlightSpan> {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = SyntaxCacheKey {
            language,
            content_hash: hasher.finish(),
            len: text.len(),
        };
        if let Some(spans) = self.spans.get(&key) {
            return spans.clone();
        }
        if !self.highlighters.contains_key(&language) {
            let Some(highlighter) = CachedSyntaxHighlighter::new(language) else {
                return Vec::new();
            };
            self.highlighters.insert(language, highlighter);
        }
        let spans = self
            .highlighters
            .get_mut(&language)
            .map(|highlighter| highlighter.highlight(text))
            .unwrap_or_default();
        while self.order.len() >= Self::CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.spans.remove(&expired);
            }
        }
        self.order.push_back(key);
        self.spans.insert(key, spans.clone());
        spans
    }
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

/// `text` の byte `offset` を含む単語の byte 範囲（ダブルクリック選択・2026-08-18）。英数字/`_`/CJK が
/// 語、それ以外と空白が区切り。語の上でなければ空範囲 `(offset, offset)`。transcript の region は
/// Rope でなく `SharedString` なので、`editor_core::word_range_at` の同じ規律を `&str` へ移植した。
fn word_range_in(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let is_word = |character: char| {
        (character.is_alphanumeric() || character == '_') && !character.is_whitespace()
    };
    // キャレットの直前 or 直後が語文字でなければ選択しない（記号や空白の上でのダブルクリック）。
    let after = text[offset..].chars().next();
    let before = text[..offset].chars().next_back();
    if !(after.map(is_word).unwrap_or(false) || before.map(is_word).unwrap_or(false)) {
        return (offset, offset);
    }
    let mut start = offset;
    for character in text[..offset].chars().rev() {
        if is_word(character) {
            start -= character.len_utf8();
        } else {
            break;
        }
    }
    let mut end = offset;
    for character in text[offset..].chars() {
        if is_word(character) {
            end += character.len_utf8();
        } else {
            break;
        }
    }
    (start, end)
}

/// 承認待ちの権限リクエスト（`session/request_permission` を UI で保持する間の状態）。
/// `respond` に選んだ選択肢の添字を送ると acp_client が応答する（このスレッドの当該ターンは
/// それまでブロックしている）。ユーザーが答えるまで composer 上部にカードを出す。
/// エージェントが出した選択肢付き質問（Elicitation・単一選択のみ）。回答するまで composer 上部に
/// カードを出す（承認カードと同居しない前提。両方来たら承認カードを優先表示）。
struct PendingElicitation {
    /// 質問文（`CreateElicitationRequest.message`）。
    message: SharedString,
    /// 単一選択フィールド群（acp_client が非対応フォームを弾いた後のもの）。
    fields: Vec<ElicitationField>,
    /// 各フィールドの現在の選択（field.name → 選んだ value）。全部埋まると「これで回答」が有効。
    selections: std::collections::BTreeMap<String, String>,
    /// 回答チャネル。`Some(選択群)`=Accept / `None`=Decline。drop でも Decline。
    respond: mpsc::UnboundedSender<Option<Vec<(String, String)>>>,
}

struct PendingPermission {
    title: SharedString,
    diffs: Vec<PermissionDiff>,
    /// Diff の無いツール（Bash/Fetch/MCP）の実引数（整形 JSON）。承認カードで逐語表示して
    /// 「何が実行されるか」を必ず見せる（tool poisoning 対策・ACP #1979 / GHSA-f2g4）。
    raw_input: Option<SharedString>,
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
    TurnFailed {
        thread: SharedString,
        color: Hsla,
        message: SharedString,
        muted: bool,
    },
    /// 権限待ちで停止中（裏の窓でも気づけるように）。`title` = 何の許可か（digest 素材・P1）。
    /// `thread_index` = このパネル内でのスレッド添字（右下トーストのクリックで当該タブへ飛ぶのに使う）。
    PermissionWaiting {
        thread: SharedString,
        thread_index: usize,
        color: Hsla,
        title: SharedString,
        muted: bool,
    },
    /// Tier 2 の ✳ 1 行要約が生成できた（P4）。workspace が task_events へキャッシュし
    /// 管制の総括デバウンスを蹴る。**状態は運ばない**（文だけ・状態は事実層が別で流れている）。
    SummaryReady {
        thread: SharedString,
        tier2: SharedString,
    },
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
    /// AI パネルの全画面表示を切り替えたい（ヘッダの ⤢・2026-07-27）。
    /// パネル自身はレイアウトの持ち主ではないので、判断は workspace に委ねる（依存方向を守る）。
    ToggleFullScreenRequest,
}

/// エージェントスレッドの状態（herdr の 5 状態を necoder 流にマップ・#）。**色相は状態に使わない**
/// （UI-SPEC §1.3「色＝識別」）。ドット/beacon は常にスレッド識別色で、状態は形（リング/グリフ）で見せる。
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
    /// 未送信の composer 本文。タブは独立した作業面なので、切替時にスレッドごとに退避する。
    draft: String,
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
    /// 回答待ちの Elicitation（選択肢付き質問。あれば composer 上部にカードを出す・単一選択のみ）。
    pending_elicitation: Option<PendingElicitation>,
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
    /// 生成中に積んだ送信待ちの prompt（キュー・FIFO）。ターン完了で先頭から自動フラッシュする。
    /// 「今すぐ送信（steer）」はこのキューを介さず即 [`Self::send_prompt_text`] する。
    queued_prompts: Vec<String>,
}

impl Thread {
    /// 現在の状態（herdr の 5 状態マップ・#）。Blocked（承認待ち）は Working（実行中）より優先で、
    /// これまで `running: bool` に埋もれていた「待ち」を分離する。Done は未確認ラッチ。
    fn activity(&self) -> ThreadActivity {
        if self.pending_permission.is_some() || self.pending_elicitation.is_some() {
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
            draft: String::new(),
            entries: Vec::new(),
            tokens_used: 0,
            tokens_max: 200_000,
            tokens_shown: 0.0,
            command_tx: None,
            available_modes: Vec::new(),
            current_mode_id: SharedString::default(),
            configs: Vec::new(),
            pending_permission: None,
            pending_elicitation: None,
            persisted_entries: 0,
            touched_files: Vec::new(),
            plan: Vec::new(),
            name_is_custom: false,
            digest: None,
            muted: false,
            tier2: None,
            queued_prompts: Vec::new(),
        }
    }

    /// Working 中のライブ素材（Tier 1 の無料版・P1）: 実行中ツールの説明 + plan の進行中項目。
    /// 保存しない（描画のたびに最新を合成＝「今なにをしているか」が常に生）。
    fn live_digest(&self) -> Option<SharedString> {
        let tool = self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Step { tool, .. } => Some(flatten_digest_line(tool)),
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

/// digest 用の 1 行化: 複数行のツールタイトル（長いシェルコマンド等）は先頭行だけ残して
/// 「…」を付け、1 行でも長すぎれば切り詰める。稼働中ライン・生成中行・編隊セル・窓上部
/// ビーコンは全部この digest を表示するので、ここで畳めば縦に伸びない。
fn flatten_digest_line(text: &str) -> SharedString {
    const MAX_CHARS: usize = 100;
    let trimmed = text.trim();
    let first_line = trimmed.lines().next().unwrap_or("").trim_end();
    let mut flat: String = first_line.chars().take(MAX_CHARS).collect();
    if flat.len() < trimmed.len() {
        flat.push_str(" …");
    }
    SharedString::from(flat)
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
/// `ToolCallInfo`（開始）から Step エントリを組む。args=主なパス / result=出力 / diffs=差分。
fn build_step_entry(info: ToolCallInfo) -> Entry {
    let args = info.locations.first().cloned().unwrap_or_default();
    Entry::Step {
        id: (!info.id.is_empty()).then(|| SharedString::from(info.id)),
        tool: SharedString::from(info.title.unwrap_or_default()),
        args: SharedString::from(args),
        result: info
            .output
            .map(|output| SharedString::from(cap_output(&output))),
        diffs: info.diffs,
    }
}

/// ACP tool の location/path から、言語指定のない出力に使う言語を推測する。
/// `src/lib.rs:42:7` のような行列付き location と、引用符付き引数の両方を扱う。
fn infer_language_from_tool_argument(value: &str) -> Option<lang::LanguageId> {
    fn candidate(value: &str) -> Option<lang::LanguageId> {
        let mut value = value.trim_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ','
                )
        });
        while let Some((path, suffix)) = value.rsplit_once(':') {
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                value = path;
            } else {
                break;
            }
        }
        lang::language_for_path(Path::new(value))
    }

    candidate(value).or_else(|| value.split_whitespace().find_map(candidate))
}

/// ツール出力を transcript 用に丸める（末尾を優先し 24 行まで。溢れは先頭で省略表示）。
/// この行数（または [`STEP_COLLAPSE_MIN_BYTES`] バイト）を超えるツール結果は既定で折り畳む。
/// 短い出力（コマンドの成否・数行）は畳む価値がないのでそのまま見せる。
const STEP_COLLAPSE_MIN_LINES: usize = 6;
const STEP_COLLAPSE_MIN_BYTES: usize = 400;

/// AI 応答本文の fenced コードブロックを折り畳む閾値。これを超える行数のコードは既定で
/// 先頭 [`CODE_COLLAPSE_HEAD_LINES`] 行だけ見せ、残りはトグルで開く（入力欄直上の最新出力が
/// 長いコードで画面を専有するのを防ぐ）。短いコードは畳む価値がないのでそのまま出す。
const CODE_COLLAPSE_MIN_LINES: usize = 12;
const CODE_COLLAPSE_HEAD_LINES: usize = 8;

/// ステップ見出しでツール名とパスを 1 行に並べる上限（これを超えるとパスは次行へ）。
/// `Read README.md` + パスのような短い名前だけを同じ行に置き、長いタイトルは折返しに専念させる。
const STEP_TITLE_INLINE_MAX_CHARS: usize = 48;

/// この文字数を超える（または改行を含む）ツール引数（Bash のコマンド等）は既定で 1 行に畳み、
/// ▸ クリックで全文展開する（結果 ⎿・Thinking・コードと同じ流儀）。短いパス・短いコマンドは
/// 畳む価値がないのでそのまま見せる。
const STEP_ARGS_COLLAPSE_MIN_CHARS: usize = 80;

/// 折り畳んだツール引数（⏺ の引数行）のヘッダ要約。1 行目だけを見せ、続きがあれば ⋯ を付ける
/// （複数行コマンドが transcript を専有しないように・横のあふれは overflow_hidden で切る）。
fn step_args_preview(args: &str) -> SharedString {
    let first_line = args.lines().next().unwrap_or("").trim_end();
    let has_more = args.trim_end() != first_line;
    if has_more {
        SharedString::from(format!("{first_line} ⋯"))
    } else {
        SharedString::from(first_line.to_string())
    }
}

/// 折り畳んだツール結果（⎿）のヘッダ要約。複数行は「N 行」、1 行なら中身を短く切って見せる。
fn step_result_summary(result: &str, line_count: usize) -> SharedString {
    if line_count > 1 {
        SharedString::from(i18n::t!("agent.result_lines", "n" => line_count))
    } else {
        let line = result.trim();
        if line.chars().count() > 72 {
            let head: String = line.chars().take(72).collect();
            SharedString::from(format!("{head}…"))
        } else {
            SharedString::from(line.to_string())
        }
    }
}

fn cap_output(text: &str) -> String {
    const MAX: usize = 24;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= MAX {
        text.trim_end().to_string()
    } else {
        let hidden = lines.len() - MAX;
        let shown = lines[hidden..].join("\n");
        format!(
            "{}\n{shown}",
            i18n::t!("agent.output_truncated", "n" => hidden)
        )
    }
}

fn entry_plain_text(entry: &Entry) -> String {
    match entry {
        Entry::User(text) | Entry::Thinking(text) | Entry::Agent(text) => text.to_string(),
        Entry::Step {
            tool, args, result, ..
        } => match result {
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

/// 既定のスレッド名（"スレッド1" / "Thread 1"）。表示言語に追従する。
fn default_thread_name(index: usize) -> String {
    i18n::t!("agent.default_thread_name", "n" => index + 1)
}

/// 既定のプレースホルダ名か（空 or 既定スレッド名）。AI 自動命名の対象判定（#6）。
/// seed 名（"rope設計" 等）や既に付いた名前は対象外＝上書きしない。
///
/// 判定を**同梱ロケール全部**で回すのは、表示言語を切り替えても以前作ったタブが
/// 「まだ名前を付けていない」と見なされ続けるようにするため（名前自体は保存済みのユーザーデータ
/// なので、既に開いているタブを翻訳し直すことはしない）。
fn is_placeholder_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return true;
    }
    i18n::available_locales().into_iter().any(|locale| {
        let Some(template) = i18n::translate_in(locale, "agent.default_thread_name") else {
            return false;
        };
        let Some(prefix) = template.split("%{n}").next().filter(|p| !p.is_empty()) else {
            return false;
        };
        name.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    })
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
    format!(
        "{}-{}",
        now_unix_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
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
    /// 固定レイアウトのマスコット。フレーム時計と paint invalidation を親 transcript から分離する。
    mascot: Entity<MascotView>,
    /// 主点字スピナー。10fps時計と invalidation を親 AgentPanel から分離する。
    composer_spinner: Entity<BrailleSpinnerView>,
    transcript_spinner: Entity<BrailleSpinnerView>,
    /// 実行中の末尾本文で「今ここまで打った」文字数（タイプライタ演出）。Zed の Markdown.selection と
    /// 同じく、生成中の本文も通常の選択可能パス（`push_selectable`）で描き、この reveal で表示末尾だけ
    /// 伸ばす。別ビューを持たないので生成中も過去本文と同じ選択機構に載る（2026-08-26 一本化）。
    live_reveal: usize,
    /// 現在タイプ中のストリームの同一性（thread.id + entry index + kind）。変わったら reveal を仕切り直す。
    live_key: Option<SharedString>,
    /// タイプライタの 40ms 時計が稼働中か（多重起動防止）。追いついたら false に戻る。
    live_ticker: bool,
    /// 実行中の末尾本文が最後に描画された時刻。非表示になったら reveal を即完了させる（打ち切り）。
    live_rendered_at: Option<std::time::Instant>,
    /// タブ改名中の (対象 index, 入力欄)。ダブルクリックで開く・IME 正しい EditorView::plain（#4）。
    renaming: Option<(usize, Entity<EditorView>)>,
    /// transcript のスクロール（M13 UX: ホイールで遡れる。ストリーミング中は底に居る時だけ追従）。
    transcript_list: ListState,
    /// transcript 専用フォーカス。出力をドラッグした後の ⌘A / ⌘C を composer の選択状態から
    /// 独立させる（出力は read-only なのでテキスト入力先にはしない）。
    transcript_focus: FocusHandle,
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
    /// ドラッグ選択中の最新ポインタ位置（ビュー外へ引っ張った時の自動スクロールが読む）。
    transcript_drag_position: Option<gpui::Point<gpui::Pixels>>,
    /// transcript のドラッグ自動スクロール tick が走っているか（二重 spawn 防止）。
    transcript_autoscroll_running: bool,
    /// ACP のコードフェンス／ファイル出力用 tree-sitter 結果。描画ごとの再パースを避ける。
    syntax_cache: Rc<RefCell<SyntaxHighlightCache>>,
    /// Agent 本文の CommonMark 解析結果。マスコットや時計の再描画で再解析しない。
    markdown_cache: Rc<RefCell<MarkdownBlockCache>>,
    /// 選択されていない本文の `StyledText`（shape/wrap 済み TextLayout を含む）。
    styled_text_cache: Rc<RefCell<StyledTextEntityCache>>,
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
    /// 最後に親パネルが View tree へ描画された時刻。非表示 TaskSpace の本文・1Hz時計を止める。
    last_rendered_at: Option<std::time::Instant>,
    /// 経過秒・承認待ち段階だけを更新する1Hz時計。マスコット/本文の高頻度時計とは独立。
    second_ticker: bool,
    /// 折り畳んだ Thinking のうちユーザーが開いたもの（thread.id, entry index）。実行中の最終ブロックは常に展開。
    expanded_thoughts: std::collections::HashSet<(String, usize)>,
    /// 展開したツール結果（⎿）。既定は折り畳み＝長いコード読み込みで transcript が流れないように
    /// （`expanded_thoughts` と同じく `(thread.id, entry index)` を外部集合で保持し Entry は軽量に保つ）。
    expanded_steps: std::collections::HashSet<(String, usize)>,
    /// 展開した AI 応答内コードブロック（`(thread.id, entry index, block index)`）。既定は折り畳み。
    /// entry 内に複数コードが在り得るので block index まで鍵に含める。
    expanded_code: std::collections::HashSet<(String, usize, usize)>,
    /// 展開したツール引数（⏺ の引数行）。既定は折り畳み＝長い/複数行コマンドで transcript が
    /// 流れないように（`expanded_steps` と同じ `(thread.id, entry index)` 鍵）。
    expanded_args: std::collections::HashSet<(String, usize)>,
    /// この panel の window がアクティブか（render で更新・GPUI が activation 変化で再描画するので追従する）。
    /// 完了音は**非アクティブ時のみ**鳴らす（見ている画面に音は要らない・P2）。
    window_active: bool,
    /// composer 入力欄の高さ（px）。上縁ハンドルのドラッグで [MIN, MAX] に変える。
    composer_height: f32,
    /// composer 入力欄を上縁ドラッグでリサイズ中か（root の on_mouse_move が読む）。
    resizing_composer: bool,
    /// リサイズ開始時のマウス Y と高さ（ドラッグ量の基準）。
    composer_resize_start_y: f32,
    composer_resize_start_height: f32,
}

impl gpui::EventEmitter<PanelEvent> for AgentPanel {}

impl AgentPanel {
    fn is_visibly_active(&self) -> bool {
        self.window_active
            && self
                .last_rendered_at
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(1500))
    }

    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        // 設定は global（settings.json が真実）から取る。live-reload / CLI / MCP 変更に observe で追従。
        let submit_on_enter = settings::get(cx).submit_on_enter;
        let composer =
            cx.new(|cx| EditorView::plain(theme.clone(), thread_color(0), submit_on_enter, cx));
        let mascot = cx.new(|_cx| MascotView::new(64.0));
        let composer_spinner = cx.new(|_cx| BrailleSpinnerView::new(9.0, thread_color(0)));
        let transcript_spinner = cx.new(|_cx| BrailleSpinnerView::new(8.0, thread_color(0)));
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
        // 開発用: NECODER_ACP_PROBE があれば、少し待って空スレッドへ自動送信（実機ストリーミングの自己検証）。
        // **最初に生成されたパネル 1 枚だけ**が拾う（レールに複数スロットがあると全パネルで発火し、
        // claude セッションが並走してしまう — 2026-08-27 の LP 素材撮りで実際に 5 本走った）。
        static ACP_PROBE_CLAIMED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if let Ok(probe) = std::env::var("NECODER_ACP_PROBE") {
            if !probe.trim().is_empty()
                && !ACP_PROBE_CLAIMED.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                cx.spawn(async move |panel, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    panel
                        .update(cx, |panel, cx| {
                            panel.switch_thread(1, cx); // 種の無い空スレッドへ（応答が先頭で見える）
                            panel.send_prompt_text(probe, cx);
                            // 開発用: NECODER_OPEN_MENU=model|effort|mode|agent でセレクタを開いて撮る
                            // （広告設定が届くと再描画され実選択肢が出る）。
                            if let Ok(which) = std::env::var("NECODER_OPEN_MENU") {
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
        // 開発用: NECODER_TRANSCRIPT_SEL_PROBE=1 で transcript 選択を注入（M13 の描画+コピー検証）。
        if std::env::var("NECODER_TRANSCRIPT_SEL_PROBE").is_ok_and(|value| value == "1") {
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
        // 開発用: NECODER_COMPOSER_PROBE="<text>" で composer に下書きを流し込む（折り返し等の描画検証）。
        if let Ok(text) = std::env::var("NECODER_COMPOSER_PROBE") {
            if !text.is_empty() {
                composer.update(cx, |composer, cx| composer.set_plain_text(&text, cx));
            }
        }
        // 開発用: NECODER_PLAN_PROBE=1 で実行プランを直接注入（M12-9 常設チェックリストの描画検証。
        // 実 ACP を起動せずに ●/☒/☐ の 3 状態が出ることをオフスクリーンで確かめる）。
        if std::env::var("NECODER_PLAN_PROBE").is_ok_and(|value| value == "1") {
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
        // 通常起動は空スレッド 1 本から始める（デモ transcript を新プロジェクトごとに種付けしない）。
        // offscreen 検証プローブはモック内容・複数タブが前提なので、その時だけ M4 由来の種を使う。
        // 既定エージェント + 前回のモデル/思考量を適用して「そのまま開く」を実現。
        let mut threads = if demo_threads_requested() {
            seed_threads()
        } else {
            vec![Thread::empty(default_thread_name(0), 0)]
        };
        for thread in &mut threads {
            apply_thread_defaults(thread, cx);
        }
        // 開発用: NECODER_TABS_PROBE=<n> でタブを n 枚まで水増しし、横スクロール（溢れ挙動）を検証する。
        if let Ok(count) = std::env::var("NECODER_TABS_PROBE") {
            if let Ok(count) = count.trim().parse::<usize>() {
                while threads.len() < count {
                    let index = threads.len();
                    let mut thread = Thread::empty(default_thread_name(index), index);
                    apply_thread_defaults(&mut thread, cx);
                    threads.push(thread);
                }
            }
        }
        // 初期表示は末尾（最新）を見せる（スクロール化 M13 での回帰防止）。
        // 開発用: NECODER_SCROLL_TOP で先頭のまま（transcript 上部＝Thinking 等の目視撮影用）。
        let initial_item_count = threads.first().map_or(1, |thread| thread.entries.len());
        let transcript_list = ListState::new(initial_item_count, ListAlignment::Top, px(800.));
        transcript_list.set_follow_mode(FollowMode::Tail);
        if std::env::var_os("NECODER_SCROLL_TOP").is_none() {
            transcript_list.scroll_to_end();
        }
        let syntax_cache = Rc::new(RefCell::new(SyntaxHighlightCache::default()));
        let markdown_cache = Rc::new(RefCell::new(MarkdownBlockCache::default()));
        let styled_text_cache = Rc::new(RefCell::new(StyledTextEntityCache::default()));
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
            mascot,
            composer_spinner,
            transcript_spinner,
            live_reveal: 0,
            live_key: None,
            live_ticker: false,
            live_rendered_at: None,
            renaming: None,
            transcript_list,
            transcript_focus: cx.focus_handle(),
            tabs_scroll: ScrollHandle::new(),
            tabs_view: initial_tabs_view(&settings::get(cx).agent_tabs_view),
            thread_list_scroll: ScrollHandle::new(),
            transcript_regions: Rc::new(RefCell::new(Vec::new())),
            transcript_selection: None,
            transcript_drag_position: None,
            transcript_autoscroll_running: false,
            syntax_cache,
            markdown_cache,
            styled_text_cache,
            open_menu: None,
            context_files: Vec::new(),
            context_menu_open: false,
            context_query: String::new(),
            context_focus: None,
            submit_on_enter,
            token_ticker: false,
            celebrating: false,
            celebrate_gen: 0,
            last_rendered_at: None,
            second_ticker: false,
            expanded_thoughts: std::collections::HashSet::new(),
            expanded_steps: std::collections::HashSet::new(),
            expanded_code: std::collections::HashSet::new(),
            expanded_args: std::collections::HashSet::new(),
            window_active: true,
            composer_height: COMPOSER_INPUT_DEFAULT,
            resizing_composer: false,
            composer_resize_start_y: 0.0,
            composer_resize_start_height: 0.0,
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
        apply_thread_defaults(&mut thread, cx);
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
        // 過去バージョンが新プロジェクトへ種付けしたデモスレッドの残骸を掃除する（2026-08-17）。
        // 対象は「seed 名そのまま + 会話 0 件」だけ＝一度でも送信した/改名したスレッドは消さない。
        let (seed_leftovers, restored): (Vec<_>, Vec<_>) = restored.into_iter().partition(|row| {
            matches!(row.1.as_str(), "rope設計" | "tab色分け" | "gpui起動")
                && storage
                    .load_recent_turns(&row.0, 1)
                    .is_ok_and(|turns| turns.is_empty())
        });
        for row in &seed_leftovers {
            if let Err(error) = storage.delete_thread(&row.0) {
                eprintln!("デモスレッドの掃除に失敗（次回起動で再試行）: {error:#}");
            }
        }
        if !restored.is_empty() {
            let mut threads = Vec::new();
            for (
                id,
                name,
                color_index,
                _project,
                _branch,
                agent,
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
                // 旧 DB 行（agent=NULL）は保存当時の Agent を知り得ないため現在の明示既定へ。
                // 新しい行は thread 固有の Agent を復元し、Claude Code へ勝手に戻さない。
                thread.agent = agent
                    .filter(|agent| acp_client::AgentKind::by_label(agent).is_some())
                    .map(SharedString::from)
                    .unwrap_or_else(|| default_agent_name(cx));
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
                            id: None,
                            tool: content.into(),
                            args: SharedString::default(),
                            result: None,
                            diffs: Vec::new(),
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
                    Some(thread.agent.as_ref()),
                    Some(thread.model.as_ref()),
                    thread.tokens_used as i64,
                    thread.tokens_max as i64,
                );
            }
        }
        self.storage = Some(storage);
        self.storage_scope = scope;
        if std::env::var_os("NECODER_SCROLL_TOP").is_none() {
            self.reset_transcript_list(true); // 復元直後も末尾（最新）を見せる
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
            Some(thread.agent.as_ref()),
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
                    if let Some(index) = panel.thread_index_by_id(&thread_id) {
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
        self.threads
            .get(self.active)
            .map(|thread| thread.name.clone())
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
                    ThreadActivity::Working => {
                        thread.live_digest().or_else(|| thread.digest.clone())
                    }
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

    /// Elicitation の選択肢を1つ選ぶ（アクティブスレッド）。単一フィールドの質問（4 択）は
    /// **選んだ瞬間に確定送信**、複数フィールドなら選択を貯めて「これで回答」で確定する。
    fn choose_elicitation_option(
        &mut self,
        field_name: String,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let single_field = if let Some(thread) = self.threads.get_mut(self.active) {
            let Some(pending) = thread.pending_elicitation.as_mut() else {
                return;
            };
            pending.selections.insert(field_name, value);
            pending.fields.len() == 1
        } else {
            return;
        };
        if single_field {
            self.submit_elicitation(cx);
        } else {
            cx.notify();
        }
    }

    /// Elicitation を確定送信する（全フィールド選択済みの時のみ Accept を返す）。
    fn submit_elicitation(&mut self, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            let all_selected = thread.pending_elicitation.as_ref().is_some_and(|pending| {
                pending
                    .fields
                    .iter()
                    .all(|field| pending.selections.contains_key(&field.name))
            });
            if !all_selected {
                return; // 未選択のフィールドがある＝まだ確定しない
            }
            if let Some(pending) = thread.pending_elicitation.take() {
                let selections: Vec<(String, String)> = pending.selections.into_iter().collect();
                pending.respond.unbounded_send(Some(selections)).ok();
            }
        }
        self.sync_running_registry(cx);
        cx.notify();
    }

    /// Elicitation に答えない（Decline）。カードを畳み、エージェントには「回答なし」を返す。
    fn decline_elicitation(&mut self, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            if let Some(pending) = thread.pending_elicitation.take() {
                pending.respond.unbounded_send(None).ok();
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
    pub fn transcript_lines(
        &self,
        thread_index: usize,
        max: usize,
    ) -> Vec<(SharedString, SharedString)> {
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
        cx: &mut Context<Self>,
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
        let mut style_hasher = DefaultHasher::new();
        base_highlights.hash(&mut style_hasher);
        let key = StyledTextCacheKey {
            content: content_cache_key(text.as_ref()),
            styles_hash: style_hasher.finish(),
            styles_len: base_highlights.len(),
            region: region_index,
        };
        let cached = {
            let cache = self.styled_text_cache.borrow();
            if let Some((cached_text, cached_highlights, styled)) = cache.items.get(&key) {
                if cached_text == &text && cached_highlights == &base_highlights {
                    Some(styled.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };
        let styled = cached.unwrap_or_else(|| {
            let styled_text = text.clone();
            let styled_highlights = base_highlights.clone();
            let styled = cx.new(move |_cx| CachedStyledTextView {
                text: styled_text,
                base_highlights: styled_highlights,
                selection: None,
                layout: TextLayout::default(),
            });
            let mut cache = self.styled_text_cache.borrow_mut();
            while cache.order.len() >= StyledTextEntityCache::CAPACITY {
                if let Some(expired) = cache.order.pop_front() {
                    cache.items.remove(&expired);
                }
            }
            cache.order.push_back(key);
            cache
                .items
                .insert(key, (text.clone(), base_highlights, styled.clone()));
            styled
        });
        styled.update(cx, |styled, cx| {
            styled.set_selection(selection, cx);
        });
        self.transcript_regions.borrow_mut().push(SelectableRegion {
            text,
            styled: styled.clone(),
        });
        styled.into_any_element()
    }

    /// 装飾なしの選択可能テキスト（プレーンなエントリ用）。
    fn selectable_text(&self, text: SharedString, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.push_selectable(text, Vec::new(), cx)
    }

    /// リージョンに掛かる選択 byte range（無ければ None。offset はヒットテスト由来 = char 境界保証）。
    fn region_selection(&self, region: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.transcript_selection.as_ref()?.normalized();
        if region < start.region || region > end.region {
            return None;
        };
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
    fn transcript_point_at(
        &self,
        position: gpui::Point<gpui::Pixels>,
        cx: &App,
    ) -> Option<TranscriptPoint> {
        let regions = self.transcript_regions.borrow();
        let mut previous_end: Option<TranscriptPoint> = None;
        for (region_index, region) in regions.iter().enumerate() {
            let styled = region.styled.read(cx);
            let layout = &styled.layout;
            let bounds = layout.bounds();
            if position.y < bounds.top() {
                // このリージョンより上 = 直前の末尾（最上部より上なら先頭）。
                return Some(previous_end.unwrap_or(TranscriptPoint {
                    region: region_index,
                    offset: 0,
                }));
            }
            if position.y <= bounds.bottom() {
                let offset = match layout.index_for_position(position) {
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

    /// point を含む「単語」の範囲（同一 region 内）。ダブルクリック選択用。
    fn transcript_word_span(&self, point: TranscriptPoint) -> (TranscriptPoint, TranscriptPoint) {
        let regions = self.transcript_regions.borrow();
        let Some(region) = regions.get(point.region) else {
            return (point, point);
        };
        let (start, end) = word_range_in(region.text.as_ref(), point.offset);
        (
            TranscriptPoint {
                region: point.region,
                offset: start,
            },
            TranscriptPoint {
                region: point.region,
                offset: end,
            },
        )
    }

    /// region 全体（＝その markdown ブロック / 段落）の範囲。トリプルクリック選択用。
    fn transcript_region_span(&self, region_index: usize) -> (TranscriptPoint, TranscriptPoint) {
        let regions = self.transcript_regions.borrow();
        let length = regions
            .get(region_index)
            .map(|region| region.text.len())
            .unwrap_or(0);
        (
            TranscriptPoint {
                region: region_index,
                offset: 0,
            },
            TranscriptPoint {
                region: region_index,
                offset: length,
            },
        )
    }

    fn on_transcript_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 出力欄自身へフォーカスを置く。以前は bubble 先の root が composer を focus し、
        // ⌘C の成否が composer 側の選択状態に左右されていた。
        self.transcript_focus.focus(window, cx);
        let point = self.transcript_point_at(event.position, cx);
        // ダブルクリック=単語 / トリプル=そのブロック（段落）を選択（エディタ/ブラウザの所作・2026-08-18）。
        // 単一クリックは従来どおりキャレットを置き、ドラッグ選択の起点にする（`selecting = true`）。
        // 語/ブロック選択は `selecting = false`（確定選択＝そのままドラッグで広げない）。
        let selection = point.map(|point| match event.click_count {
            2 => {
                let (start, end) = self.transcript_word_span(point);
                TranscriptSelection {
                    start,
                    end,
                    selecting: false,
                }
            }
            count if count >= 3 => {
                let (start, end) = self.transcript_region_span(point.region);
                TranscriptSelection {
                    start,
                    end,
                    selecting: false,
                }
            }
            _ => TranscriptSelection {
                start: point,
                end: point,
                selecting: true,
            },
        });
        self.transcript_selection = selection;
        // root の「どこをクリックしても composer を focus」へ流さない。
        cx.stop_propagation();
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
        self.transcript_drag_position = Some(event.position);
        let point = self.transcript_point_at(event.position, cx);
        if let (Some(selection), Some(point)) = (self.transcript_selection.as_mut(), point) {
            if selection.end != point {
                selection.end = point;
                cx.notify();
            }
        }
        // ビュー外へ引っ張ったら、押している間スクロールし続ける（下=末尾側 / 上=先頭側）。
        if self.transcript_drag_overshoot(event.position) != gpui::px(0.) {
            self.start_transcript_autoscroll(cx);
        }
    }

    fn on_transcript_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transcript_drag_position = None;
        if let Some(selection) = self.transcript_selection.as_mut() {
            selection.selecting = false;
            if selection.start == selection.end {
                self.transcript_selection = None; // ただのクリック = 選択なし
            }
            cx.notify();
        }
    }

    /// ポインタの transcript ビューポートからの上下はみ出し量。ビュー内（未描画含む）は 0。
    fn transcript_drag_overshoot(&self, position: gpui::Point<gpui::Pixels>) -> gpui::Pixels {
        let viewport = self.transcript_list.viewport_bounds();
        if viewport.size.height <= gpui::px(0.) {
            return gpui::px(0.);
        }
        if position.y < viewport.top() {
            position.y - viewport.top()
        } else if position.y > viewport.bottom() {
            position.y - viewport.bottom()
        } else {
            gpui::px(0.)
        }
    }

    /// ドラッグ自動スクロールの tick ループを起動する（既に走っていれば何もしない）。
    fn start_transcript_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.transcript_autoscroll_running {
            return;
        }
        self.transcript_autoscroll_running = true;
        cx.spawn(async move |panel, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
                let keep_going = panel
                    .update(cx, |panel, cx| panel.transcript_autoscroll_tick(cx))
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// 自動スクロール 1 tick: はみ出し量に比例してリストを送り、選択端を追従させる。
    /// region 層は前フレームの可視域なので、端まで選択 → 次 tick でさらに延びる。
    /// 続行するなら true（選択終了・ビュー内復帰で自然停止）。
    fn transcript_autoscroll_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let selecting = self
            .transcript_selection
            .as_ref()
            .is_some_and(|selection| selection.selecting);
        let overshoot = match self.transcript_drag_position {
            Some(position) if selecting => self.transcript_drag_overshoot(position),
            _ => gpui::px(0.),
        };
        if overshoot == gpui::px(0.) {
            self.transcript_autoscroll_running = false;
            return false;
        }
        // 遠くへ引くほど速く（tick あたり最大 48px）。
        let step = f32::from(overshoot).clamp(-96.0, 96.0) * 0.5;
        self.transcript_list.scroll_by(gpui::px(step));
        if let Some(position) = self.transcript_drag_position {
            let point = self.transcript_point_at(position, cx);
            if let (Some(selection), Some(point)) = (self.transcript_selection.as_mut(), point) {
                selection.end = point;
            }
        }
        cx.notify();
        true
    }

    /// transcript 選択の ⌘C（M13）。⌘C は keymap で `editor_view::Copy` に解決され、フォーカスを
    /// 持つ composer に**先に**届く（GPUI の action ディスパッチは leaf→root・action は key_down より前）。
    /// composer は空選択なら `cx.propagate()` で譲るので、transcript を選択中なら root の此処が受けて
    /// コピーする。選択が無ければ更に上位へ譲る（他の Copy 利用者を邪魔しない）。
    fn on_copy_selection(
        &mut self,
        _: &editor_view::Copy,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.transcript_selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        } else {
            cx.propagate();
        }
    }

    /// transcript にフォーカスがある時の ⌘A。composer がフォーカス中なら EditorView が先に
    /// 消費するため、通常の入力欄の全選択はそのまま保たれる。
    fn on_select_all_transcript(
        &mut self,
        _: &editor_view::SelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let regions = self.transcript_regions.borrow();
        let Some(last) = regions.last() else {
            cx.propagate();
            return;
        };
        self.transcript_selection = Some(TranscriptSelection {
            start: TranscriptPoint {
                region: 0,
                offset: 0,
            },
            end: TranscriptPoint {
                region: regions.len() - 1,
                offset: last.text.len(),
            },
            selecting: false,
        });
        drop(regions);
        cx.notify();
    }

    /// パネル全域のキー: Esc で transcript 選択解除／実行中なら中断（⌘C は `on_copy_selection` 側）。
    fn on_panel_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" {
            // 選択の解除が先（transcript を読んでいる最中の Esc でターンを止めない）。
            if self.transcript_selection.take().is_some() {
                cx.notify();
                return;
            }
            // 実行中なら Esc = 中断（Claude Code / Zed と同じ体感）。
            if self
                .threads
                .get(self.active)
                .is_some_and(|thread| thread.running)
            {
                self.cancel_turn(self.active, cx);
                cx.stop_propagation();
            }
        }
    }

    /// 実行中のターンを中断する。ACP の `session/cancel` を送り、エージェントが
    /// `StopReason::Cancelled` を返すのを待つ（＝`TurnEnded { Interrupted }` で畳まれる）。
    ///
    /// **送信できなかったときはローカルで畳む**のが肝。セッションが既に死んでいると通知は
    /// どこにも届かず、待っても終端イベントは来ない＝「稼働中」に取り残される（M14 の実バグ）。
    pub fn cancel_turn(&mut self, thread_index: usize, cx: &mut Context<Self>) {
        let sent = self
            .threads
            .get(thread_index)
            .filter(|thread| thread.running)
            .and_then(|thread| thread.command_tx.as_ref())
            .is_some_and(|command_tx| {
                command_tx
                    .unbounded_send(acp_client::SessionCommand::Cancel)
                    .is_ok()
            });
        if sent {
            // 応答（Cancelled）を待つ。UI は稼働中のまま＝二重押しは無害。
            return;
        }
        self.abandon_turn(thread_index, &i18n::t!("agent.err_cancel_session_lost"), cx);
    }

    /// このパネルで走っている全スレッドを中断する（編隊セルの「エージェントを止める」・2026-07-27）。
    /// 止めた本数を返す（0 = 走っていなかった）。
    pub fn cancel_all_turns(&mut self, cx: &mut Context<Self>) -> usize {
        let running: Vec<usize> = self
            .threads
            .iter()
            .enumerate()
            .filter(|(_, thread)| thread.running)
            .map(|(index, _)| index)
            .collect();
        for index in &running {
            self.cancel_turn(*index, cx);
        }
        running.len()
    }

    /// このパネルに実行中のスレッドがあるか（編隊セルの片付けメニューが「止める」を出すかの判定）。
    pub fn has_running_thread(&self) -> bool {
        self.threads.iter().any(|thread| thread.running)
    }

    /// 終端イベントが期待できないターンを**ローカルで**畳む（セッション断・強制中断の後始末）。
    /// `running` を落として「中断で終わった」ラッチを立て、送信路も捨てる（次の送信で貼り直す）。
    fn abandon_turn(&mut self, thread_index: usize, message: &str, cx: &mut Context<Self>) {
        let abandoned = if let Some(thread) = self.threads.get_mut(thread_index) {
            if !thread.running {
                return; // 既に畳まれている（終端イベントが先に来た等）
            }
            thread.running = false;
            thread.pending_permission = None;
            thread.command_tx = None; // 死んだ送信路は捨てる（次の prompt で再起動）
            thread.done = Some(TurnEnd::Interrupted);
            thread.digest = digest_tail(message).or(thread.digest.take());
            thread
                .entries
                .push(Entry::Agent(SharedString::from(message.to_string())));
            Some((thread.name.clone(), thread.color, thread.muted))
        } else {
            None
        };
        if let Some((thread, color, muted)) = abandoned {
            cx.emit(PanelEvent::TurnFailed {
                thread,
                color,
                message: SharedString::from(message.to_string()),
                muted,
            });
        }
        self.sync_running_registry(cx);
        cx.notify();
    }

    /// ストリーミング追従: **底に居る時だけ**最下部へ張り付く（遡って読んでいる間は動かさない）。
    /// `FollowMode::Tail` と同じ状態を読み、ユーザーが上へ戻った後は追従を再開しない。
    fn follow_transcript_if_at_bottom(&self) {
        if self.transcript_list.is_scrolled_to_end().unwrap_or(true) {
            self.transcript_list.scroll_to_end();
        }
    }

    /// transcript の仮想リスト項目数。Entry に加え、送信直後など本文がまだ無い間だけ
    /// generating 行を末尾の仮想項目として数える。スレッドが無い空状態も 1 項目で描く。
    fn transcript_item_count(&self) -> usize {
        let Some(thread) = self.threads.get(self.active) else {
            return 1;
        };
        let tail_is_streaming = thread
            .entries
            .last()
            .is_some_and(|entry| matches!(entry, Entry::Agent(_) | Entry::Thinking(_)));
        thread.entries.len() + usize::from(thread.running && !tail_is_streaming)
    }

    fn reset_transcript_list(&self, scroll_to_end: bool) {
        self.transcript_list.reset(self.transcript_item_count());
        if scroll_to_end {
            self.transcript_list.scroll_to_end();
        }
    }

    /// 現在見えている位置から、ひとつ前のユーザー指示をビューポート先頭へ揃える。
    /// ボタンを繰り返し押せば、指示単位で会話を遡れる。
    fn jump_to_previous_user_entry(&mut self, cx: &mut Context<Self>) {
        let top_item = self.transcript_list.logical_scroll_top().item_ix;
        let target = self
            .threads
            .get(self.active)
            .and_then(|thread| previous_user_entry_index(&thread.entries, top_item));
        if let Some(index) = target {
            self.transcript_list.scroll_to(ListOffset {
                item_ix: index,
                offset_in_item: px(0.),
            });
        }
        // scroll_to_top_of_item は prepaint で反映されるため、次の描画でボタンの遡り先も更新する。
        cx.notify();
    }

    fn switch_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.threads.len() || index == self.active {
            return;
        }
        self.save_active_draft(cx);
        self.renaming = None; // 別タブへ切替えたら編集中の改名は破棄する
        self.active = index;
        if let Some(thread) = self.threads.get_mut(index) {
            thread.done = None; // 見た＝herdr の Done ラッチ（完了・未確認）を解除
        }
        self.reset_transcript_list(true); // 切替先は最新（末尾）を見せる
        let color = self.active_color();
        let draft = self.threads[index].draft.clone();
        self.composer.update(cx, |composer, cx| {
            composer.set_plain_text(&draft, cx);
            composer.set_accent(color, cx);
        });
        self.sync_running_registry(cx); // Done 解除をロールアップ（フッター/レール/⌘O）へ反映
                                        // 切替先が idle で送信待ちを持っていれば流す（裏で完了したスレッドのキューをここで消化）。
        self.flush_queued_prompt(cx);
        cx.notify();
    }

    /// スレッドタブを閉じる（× ボタン / ⌘W）。**最後の 1 枚も閉じられる**（空状態＝＋で再開）。
    /// ACP セッション（`command_tx`）は畳むが、会話履歴ごと closed_threads に退避し ⌘⇧T で復元可能。
    fn remove_thread(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.threads.len() {
            return;
        }
        self.save_active_draft(cx);
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
        let color = self.active_color();
        let draft = self
            .threads
            .get(self.active)
            .map(|thread| thread.draft.clone())
            .unwrap_or_default();
        self.composer.update(cx, |composer, cx| {
            composer.set_plain_text(&draft, cx);
            composer.set_accent(color, cx);
        });
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
            self.save_active_draft(cx);
            self.threads.push(thread);
            self.active = self.threads.len() - 1;
            let color = self.active_color();
            let draft = self.threads[self.active].draft.clone();
            self.composer.update(cx, |composer, cx| {
                composer.set_plain_text(&draft, cx);
                composer.set_accent(color, cx);
            });
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
        self.save_active_draft(cx);
        let mut thread = thread_from_storage(&storage, id, name, color_index, cx);
        thread.created_at_ms = created_at_ms;
        thread.last_input_at_ms = last_input_at_ms;
        self.threads.push(thread);
        self.active = self.threads.len() - 1;
        self.renaming = None;
        let color = self.active_color();
        self.composer.update(cx, |composer, cx| {
            composer.set_plain_text("", cx);
            composer.set_accent(color, cx);
        });
        self.reset_transcript_list(true);
        cx.notify();
        Some(self.active)
    }

    /// composer（入力欄）にキーボードフォーカスを移す。タブ操作時に呼び「Agent にいる」状態にする。
    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// パネル内のいずれかの面（composer / transcript / ＋context / タブ改名）にフォーカスがあるか。
    /// プロジェクト切替が「Agent に居た」ことを覚え、切替先の composer へフォーカスを追従させるのに使う
    /// （旧 session のパネルは切替でツリーから外れ、フォーカスが迷子になると全キーが死ぬため）。
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.composer
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
            || self.transcript_focus.contains_focused(window, cx)
            || self
                .context_focus
                .as_ref()
                .is_some_and(|focus| focus.contains_focused(window, cx))
            || self.renaming.as_ref().is_some_and(|(_, editor)| {
                editor
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
    }

    fn save_active_draft(&mut self, cx: &App) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            thread.draft = self.composer.read(cx).plain_text();
        }
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
        let mut thread = Thread::empty(default_thread_name(index), index);
        // 既定エージェント + 前回のモデル/思考量（§8 の sticky）で開く。
        apply_thread_defaults(&mut thread, cx);
        // 権限モードは agent 広告依存で settings に sticky を持たないため、直前まで見ていたタブから
        // 引き継ぐ（「タブを開くたびに mode が default に戻る」を断つ）。
        if let Some(previous) = self.threads.get(self.active) {
            thread.permission_mode = previous.permission_mode.clone();
        }
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
    /// 開発用: 管制タブの受入シナリオ（P3・`NECODER_CONTROL_PROBE`）。Task パネル（1 thread）へ
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
                    id: None,
                    tool: "Bash(cargo test -p necoder)".into(),
                    args: SharedString::default(),
                    result: None,
                    diffs: Vec::new(),
                });
                thread.plan = vec![
                    PlanItem {
                        content: "設計を確認".into(),
                        status: PlanStatus::Completed,
                    },
                    PlanItem {
                        content: "実装".into(),
                        status: PlanStatus::Completed,
                    },
                    PlanItem {
                        content: "テスト修正".into(),
                        status: PlanStatus::InProgress,
                    },
                    PlanItem {
                        content: "docs 更新".into(),
                        status: PlanStatus::Pending,
                    },
                    PlanItem {
                        content: "スクショ検証".into(),
                        status: PlanStatus::Pending,
                    },
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
                    // Diff 無しツールの実引数表示（tool poisoning 対策）を描画検証で写すためのデモ値。
                    raw_input: Some(SharedString::from(
                        "{\n  \"command\": \"cargo publish\",\n  \"cwd\": \"/Users/daichi/Work/experience/necoder\"\n}",
                    )),
                    options: vec![
                        PermissionChoice {
                            label: "許可".into(),
                            kind: PermissionKind::Allow,
                        },
                        PermissionChoice {
                            label: "常に許可".into(),
                            kind: PermissionKind::AllowAlways,
                        },
                        PermissionChoice {
                            label: "拒否".into(),
                            kind: PermissionKind::Reject,
                        },
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
                        id: None,
                        tool: "Bash cargo test -p necoder".into(),
                        args: SharedString::default(),
                        result: None,
                        diffs: Vec::new(),
                    });
                    thread.plan = vec![
                        PlanItem {
                            content: "設計を確認".into(),
                            status: PlanStatus::Completed,
                        },
                        PlanItem {
                            content: "テストを直す".into(),
                            status: PlanStatus::InProgress,
                        },
                        PlanItem {
                            content: "docs 更新".into(),
                            status: PlanStatus::Pending,
                        },
                    ];
                }
                1 => {
                    let (respond, _rx) = mpsc::unbounded(); // Blocked（承認待ち）
                    let title = SharedString::from("workspace.rs への書き込みを許可しますか");
                    thread.digest = Some(title.clone()); // 実経路（on_event）と同じ素材①
                    thread.pending_permission = Some(PendingPermission {
                        title,
                        diffs: Vec::new(),
                        raw_input: None,
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
        *self.styled_text_cache.borrow_mut() = StyledTextEntityCache::default();
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
    /// 名前で固定スレッドを引き当てる（無ければ作る）。`aliases` は同一スレッドとみなす別名で、
    /// 表示言語を切り替えた後に前の言語の名前で残っているスレッドを拾い直すために使う。
    pub fn ensure_named_thread(
        &mut self,
        name: &str,
        aliases: &[String],
        agent: &str,
        cx: &mut Context<Self>,
    ) -> usize {
        if let Some(index) = self.threads.iter().position(|thread| {
            thread.name.as_ref() == name
                || aliases.iter().any(|alias| alias == thread.name.as_ref())
        }) {
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
        if selector == Selector::Agent && self.agent_selector_locked() {
            self.open_menu = None;
            cx.notify();
            return;
        }
        self.open_menu = if self.open_menu == Some(selector) {
            None
        } else {
            Some(selector)
        };
        cx.notify();
    }

    /// Agent は会話の送り先そのもの。1 発でも会話した後に差し替えると、同じ transcript に
    /// 別 AI の文脈が混ざり、復元時にも「このスレッドは誰か」を一意にできないため固定する。
    fn agent_selector_locked(&self) -> bool {
        self.threads.get(self.active).is_some_and(|thread| {
            !thread.entries.is_empty() || thread.command_tx.is_some() || thread.running
        })
    }

    /// セレクタのドロップダウンに出す選択肢。エージェントが広告した実選択肢（モード/モデル/effort）が
    /// あればそれを、無ければ静的な既定ラベルを返す。
    fn selector_options(&self, selector: Selector) -> Vec<SharedString> {
        if selector == Selector::Agent {
            let current = self.selector_value(Selector::Agent);
            let mut options: Vec<SharedString> = acp_client::authenticated_agent_labels()
                .into_iter()
                .map(SharedString::from)
                .collect();
            // 明示既定/復元値は、起動直後の背景認証確認より強い。現在値を選択肢から消さない。
            if !options.contains(&current) {
                options.insert(0, current);
            }
            return options;
        }
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
            let fallback = if selector == Selector::Model {
                self.threads
                    .get(self.active)
                    .map(|thread| fallback_models(thread.agent.as_ref()))
                    .unwrap_or(CLAUDE_MODELS)
            } else {
                selector.options()
            };
            fallback
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

    /// 選択ピルの値を**このスレッドに**反映（Model/Effort は `session/set_config_option`、
    /// Mode は `session/set_mode` を実際に送る）。
    ///
    /// Agent だけは**グローバル既定（`default_agent`）を触らない** — どのエージェントを使うかは
    /// 環境の選択で、1 スレッドの都合で全体が動くと事故になる（ドリフト禁止・DECISIONS §8）。
    /// 一方 **Model / Effort / Mode は agent ごとに sticky**（2026-08-17）: 作業のたびに選び直す種類の
    /// 設定なので、選んだ値を `agent_defaults[今の agent]` へ書き戻し、その agent の次スレッドが
    /// それで開く。agent を跨いでも値が混ざらない（Claude=Opus / Codex=GPT を各々保つ）。
    /// 既定を「動かさない」（§8＝default_agent）ことと「覚える」（per-agent sticky）の線をここで引いている。
    fn select_option(&mut self, selector: Selector, value: SharedString, cx: &mut Context<Self>) {
        if selector == Selector::Agent && self.agent_selector_locked() {
            self.open_menu = None;
            cx.notify();
            return;
        }
        // sticky の宛先は「今アクティブなスレッドの agent」。選んだ値をその agent にだけ紐づける。
        let active_agent = self
            .threads
            .get(self.active)
            .map(|thread| thread.agent.clone());
        if let Some(agent) = active_agent {
            match selector {
                Selector::Model => settings::set_agent_default(cx, &agent, "model", &value),
                Selector::Effort => settings::set_agent_default(cx, &agent, "effort", &value),
                Selector::Mode => settings::set_agent_default(cx, &agent, "mode", &value),
                Selector::Agent => {}
            }
        }
        if let Some(thread) = self.threads.get_mut(self.active) {
            match selector {
                Selector::Agent => {
                    // 未送信スレッドのエージェントだけ差し替える。会話開始後は上の guard で固定。
                    // default_agent は巻き添え保存しない（グローバル既定のドリフト防止）。
                    thread.agent = value.clone();
                    // 切り替え先 agent の sticky（モデル/思考量/モード）で開き直す。無ければ vendor
                    // フォールバック — Claude の sticky を Codex へ持ち込むとメニューが Claude 一色になる。
                    apply_agent_sticky(thread, cx);
                }
                Selector::Mode => {
                    thread.permission_mode = value.clone();
                    // 表示名 → mode_id を引いて `session/set_mode` を送る（広告モードがある時）。
                    let mode_id = thread
                        .available_modes
                        .iter()
                        .find(|(_, name)| name.eq_ignore_ascii_case(&value))
                        .map(|(id, _)| id.to_string());
                    if let (Some(mode_id), Some(command_tx)) = (mode_id, &thread.command_tx) {
                        command_tx
                            .unbounded_send(SessionCommand::SetMode(mode_id))
                            .ok();
                    }
                    // 承認待ちの最中に bypass へ切り替えたら、その場で Allow を返す。
                    // `set_mode` はターン中 deferred（acp_client）で今のターンには効かないため、
                    // ここで畳まないと「bypass にしたのに止まったまま」になる。checkpoint は
                    // リクエスト受信時に記録済み＝手動 Allow と同じ一本道。
                    if value.to_lowercase().contains("bypass") {
                        if let Some(pending) = thread.pending_permission.take() {
                            let choice = pending
                                .options
                                .iter()
                                .position(|option| {
                                    matches!(
                                        option.kind,
                                        PermissionKind::Allow | PermissionKind::AllowAlways
                                    )
                                })
                                .unwrap_or(0);
                            pending.respond.unbounded_send(choice).ok();
                        }
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
        self.sync_running_registry(cx); // bypass 切替で承認待ちを畳んだ場合の Blocked 表示更新
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
        let locked = selector == Selector::Agent && self.agent_selector_locked();
        let accent = self.active_color();
        let (id, mut tip): (&'static str, String) = match selector {
            Selector::Agent => ("pill-agent", i18n::t!("agent.pill_agent")),
            Selector::Mode => ("pill-mode", i18n::t!("agent.pill_mode")),
            Selector::Model => ("pill-model", i18n::t!("agent.pill_model")),
            Selector::Effort => ("pill-effort", i18n::t!("agent.pill_effort")),
        };
        if locked {
            tip = i18n::t!("agent.pill_agent_locked");
        }
        let label_color = if is_open { accent } else { theme.fg1 };
        let mut pill = div()
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
            .when(is_open, |element| element.bg(theme.bg2))
            // エージェントピルだけブランドバッジ（タブ / List 行と同一在庫）。開かなくても宛先が一目になる。
            .when(selector == Selector::Agent, |element| {
                element.child(agent_badge(&value, 12.))
            })
            .child(value.clone())
            .tooltip(Tooltip::text(tip, theme.clone()));
        if !locked {
            pill = pill
                .cursor_pointer()
                // 塗りは hover と open の時だけ薄く。テキストは hover で少し明るく。
                .hover(|style| {
                    style
                        .bg(theme.bg2)
                        .text_color(if is_open { accent } else { theme.fg0 })
                })
                .child(div().text_size(px(8.)).text_color(theme.fg2).child("▾"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        // menu の mouse-down-out が先に閉じても、開いていたピルの再 open を防ぐ。
                        if is_open {
                            this.open_menu = None;
                            cx.notify();
                        } else {
                            this.toggle_menu(selector, cx);
                        }
                    }),
                );
        }
        // 開いている時、このピルの真上にドロップダウンを出す（ピル基準なのでズレない）。
        pill.when(is_open, |element| {
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
            // タブ、transcript、別のピル、パネル外を含め、どこか他を押したら閉じる。
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open_menu = None;
                cx.notify();
            }))
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
                            .gap(px(6.))
                            .px(px(9.))
                            .py(px(5.))
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .text_color(if selected { theme.fg0 } else { theme.fg1 })
                            .cursor_pointer()
                            .when(selected, |element| element.bg(theme.bg3))
                            .hover(|style| style.bg(theme.bg3))
                            // エージェント選択だけブランドバッジ付き（Mode/Model/Effort は文字のみ）
                            .when(selector == Selector::Agent, |element| {
                                element.child(agent_badge(&option, 14.))
                            })
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
        if let Some(thread) = self.threads.get_mut(self.active) {
            thread.draft.clear();
        }
        // 生成中（Working / Blocked）は即送信せずキューへ積む。ターン完了で先頭から自動フラッシュ。
        // 割り込んで今すぐ送りたい時は宛先チップ横の「今すぐ」（steer）を使う（キューを介さない）。
        let running = self
            .threads
            .get(self.active)
            .is_some_and(|thread| thread.running);
        if running {
            if let Some(thread) = self.threads.get_mut(self.active) {
                thread.queued_prompts.push(prompt);
            }
            cx.notify();
            return;
        }
        self.send_prompt_text(prompt, cx);
    }

    /// キューに積んだ prompt があれば先頭を送る（ターン完了時・スレッド切替時に呼ぶ）。
    /// **アクティブスレッドが idle の時だけ**流す（`send_prompt_text` は active 宛のため）。
    fn flush_queued_prompt(&mut self, cx: &mut Context<Self>) {
        let next = self.threads.get_mut(self.active).and_then(|thread| {
            if thread.running || thread.queued_prompts.is_empty() {
                None
            } else {
                Some(thread.queued_prompts.remove(0))
            }
        });
        if let Some(prompt) = next {
            self.send_prompt_text(prompt, cx);
        }
    }

    /// キュー内の prompt を今すぐ送る（割り込み送信）。生成中なら**現ターンを中断してから**送る。
    /// ACP は生成中に届いた prompt を deferred に回す（＝そのままでは割り込めず、しかも UI の
    /// running が取り残される）ため、cancel を挟んで確実に新ターンとして送るのが正しい。
    /// idle の時は即送信。宛先チップ横のボタンから。
    fn send_queued_now(&mut self, queue_index: usize, cx: &mut Context<Self>) {
        let running = self.threads.get(self.active).and_then(|thread| {
            (queue_index < thread.queued_prompts.len()).then_some(thread.running)
        });
        let Some(running) = running else {
            return;
        };
        if running {
            // 対象を先頭へ寄せ、現ターンを中断。中断完了（TurnEnded）で `flush_queued_prompt` が
            // 先頭＝この prompt を新ターンとして送る（running も正しく立つ）。
            if let Some(thread) = self.threads.get_mut(self.active) {
                let prompt = thread.queued_prompts.remove(queue_index);
                thread.queued_prompts.insert(0, prompt);
            }
            self.cancel_turn(self.active, cx);
        } else {
            // idle: 即送れる。
            if let Some(prompt) = self
                .threads
                .get_mut(self.active)
                .map(|thread| thread.queued_prompts.remove(queue_index))
            {
                self.send_prompt_text(prompt, cx);
            }
        }
    }

    /// キューから prompt を取り消す（宛先チップ横の ✕ から）。
    fn remove_queued_prompt(&mut self, queue_index: usize, cx: &mut Context<Self>) {
        if let Some(thread) = self.threads.get_mut(self.active) {
            if queue_index < thread.queued_prompts.len() {
                thread.queued_prompts.remove(queue_index);
                cx.notify();
            }
        }
    }

    /// 文脈の圧縮（`/compact`）をエージェントへ依頼する（Claude Code の slash コマンドを turn として送る）。
    /// 実行中は無視（ターンの最中には送らない）。トークンメーター横のボタンから呼ぶ。
    fn compact_context(&mut self, cx: &mut Context<Self>) {
        let running = self
            .threads
            .get(self.active)
            .map(|thread| thread.running)
            .unwrap_or(false);
        if running {
            return;
        }
        self.send_prompt_text("/compact".to_string(), cx);
    }

    /// prompt テキストをアクティブスレッドへ積み、常駐 ACP セッションへ送る（composer 非依存）。
    /// 開発時の自動プローブ（`NECODER_ACP_PROBE`）からも使う。
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

    /// スレッドの**安定 id** から現在の位置（index）を引く。タブは D&D 並べ替え（[`Self::move_thread`]）
    /// や クローズ（[`Self::remove_thread`]）で index が動くので、生成時の index を握る長寿命の非同期
    /// コールバック（セッションのイベントポンプ・生成物の書き戻し）は index を保持せず**毎回ここで引き直す**。
    /// 閉じられたスレッドには `None`＝そのイベントは捨てる（別タブへ誤配送しない）。
    fn thread_index_by_id(&self, id: &str) -> Option<usize> {
        self.threads.iter().position(|thread| thread.id == id)
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
        // このセッションの配送先は**位置ではなく安定 id で追う**。タブは並べ替え/クローズで index が
        // 動くので、生成時の thread_index を握ると走行中セッションのイベント（特に `TurnEnded {
        // Interrupted }`）が今その位置に居る別タブへ着弾する（＝「他を中断したら別タブも中断」の正体）。
        let thread_id = self
            .threads
            .get(thread_index)
            .map(|thread| thread.id.clone())
            .unwrap_or_default();
        let host = self.dest_host.clone();
        let command =
            acp_client::AgentKind::by_label(&agent_label)?.command_on(host.as_ref(), cwd)?;
        // スレッドの希望モード（sticky/前タブ）を起動時に渡す＝初回 prompt 前に適用される。
        // UI からの後追い SetMode だと Prompt に先を越され、初回ターンが既定モードで走る。
        let desired_mode = self
            .threads
            .get(thread_index)
            .map(|thread| thread.permission_mode.to_string())
            .filter(|mode| !mode.is_empty());
        let (command_tx, prompt_rx) = mpsc::unbounded::<SessionCommand>();
        let (event_tx, mut event_rx) = mpsc::unbounded::<AgentEvent>();
        let error_tx = event_tx.clone();

        cx.background_executor()
            .spawn(async move {
                if let Err(error) =
                    acp_client::run_session_on(host, command, desired_mode, prompt_rx, event_tx)
                        .await
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
                // 生成時の index ではなく、id で**現在位置**を引き直して配送する（並べ替え/クローズ耐性）。
                // スレッドが閉じられていればイベントは捨てる（誤ったタブへ書かない）。
                let delivered = panel.update(cx, |panel, cx| {
                    if let Some(index) = panel.thread_index_by_id(&thread_id) {
                        panel.on_event(index, event, cx);
                    }
                });
                if delivered.is_err() {
                    return; // パネル破棄済み（ウィンドウを閉じた等）＝後始末も不要
                }
            }
            // **安全網**: チャネルが閉じた＝セッションが終わった。終端イベント（TurnEnded/Failed）
            // が来ないまま終わる経路（エージェントのプロセスが正常終了・stdout が閉じた 等）が
            // あり、そのままだと `running` が落ちず永久に「稼働中」になる（M14 の実バグ）。
            // 終端が来ていれば `abandon_turn` は running=false を見て何もしない。
            panel
                .update(cx, |panel, cx| {
                    if let Some(index) = panel.thread_index_by_id(&thread_id) {
                        panel.abandon_turn(index, &i18n::t!("agent.err_session_ended"), cx);
                    }
                })
                .ok();
        })
        .detach();

        Some(command_tx)
    }

    /// `run_session` の [`AgentEvent`] を transcript へ逐次反映する（ストリーミングの心臓部）。
    /// 増分テキストは直前の同種エントリへ連結（新ターンは先頭に User があるので自然に区切れる）。
    fn on_event(&mut self, thread_index: usize, event: AgentEvent, cx: &mut Context<Self>) {
        let active = self.active;
        let panel_visibly_active = self.is_visibly_active();
        // reduce motion では reveal を仕切り直さず即全開示（reset で 0 に落として一瞬空にしない）。
        let animate_visible_stream =
            thread_index == active && panel_visibly_active && !cx.reduce_motion();
        let mut start_token_ticker = false;
        let mut celebrate_now = false;
        let mut ensure_reveal = false; // アクティブが Agent/Thinking をストリーム → タイプライタ稼働
        let mut reveal_reset = false; // 新しいストリームエントリ開始 → 先頭から打つ
        let mut stream_updated = false;
        let Some(thread) = self.threads.get_mut(thread_index) else {
            return;
        };
        let turn_finished = matches!(event, AgentEvent::TurnEnded { .. } | AgentEvent::Failed(_));
        let turn_started = matches!(event, AgentEvent::TurnStarted);
        // 完了音・バンザイは**正常完了時のみ**（中断＝拒否/キャンセルや失敗では祝わない）。
        let turn_succeeded = matches!(
            event,
            AgentEvent::TurnEnded {
                reason: TurnEnd::Completed
            }
        );
        let permission_waiting = matches!(event, AgentEvent::PermissionRequest { .. });
        let elicitation_waiting = matches!(event, AgentEvent::ElicitationRequest { .. });
        let mut files_touched: Option<(Vec<std::path::PathBuf>, Hsla)> = None;
        match event {
            AgentEvent::TurnStarted => {
                // ACP が実際に prompt を送った＝ここから生成中。楽観 UI が取りこぼした場合
                // （deferred から走った 2 本目など）でもここで確実に running を立て直す。
                thread.running = true;
                thread.done = None;
                if thread.turn_started_at.is_none() {
                    thread.turn_started_at = Some(std::time::Instant::now());
                }
            }
            AgentEvent::AgentChunk(text) => {
                stream_updated = true;
                match thread.entries.last_mut() {
                    Some(Entry::Agent(existing)) => {
                        let mut combined = existing.to_string();
                        combined.push_str(&text);
                        *existing = combined.into();
                    }
                    _ => {
                        thread.entries.push(Entry::Agent(text.into()));
                        reveal_reset = animate_visible_stream; // 見えている新エントリだけ先頭から打つ
                    }
                }
                ensure_reveal = animate_visible_stream;
            }
            AgentEvent::ThoughtChunk(text) => {
                stream_updated = true;
                match thread.entries.last_mut() {
                    Some(Entry::Thinking(existing)) => {
                        let mut combined = existing.to_string();
                        combined.push_str(&text);
                        *existing = combined.into();
                    }
                    _ => {
                        thread.entries.push(Entry::Thinking(text.into()));
                        reveal_reset = animate_visible_stream;
                    }
                }
                ensure_reveal = animate_visible_stream;
            }
            AgentEvent::ToolStarted(info) => thread.entries.push(build_step_entry(info)),
            AgentEvent::ToolUpdated(info) => {
                // 同 ID の直近 Step に、後追いの出力・差分・パスを反映する（Bash 出力や完了差分）。
                for entry in thread.entries.iter_mut().rev() {
                    if let Entry::Step {
                        id: Some(id),
                        tool,
                        args,
                        result,
                        diffs,
                    } = entry
                    {
                        if id.as_ref() == info.id.as_str() {
                            if let Some(title) = &info.title {
                                if !title.is_empty() {
                                    *tool = SharedString::from(title.clone());
                                }
                            }
                            if args.is_empty() {
                                if let Some(path) = info.locations.first() {
                                    *args = SharedString::from(path.clone());
                                }
                            }
                            if !info.diffs.is_empty() {
                                *diffs = info.diffs.clone();
                            }
                            if let Some(output) = &info.output {
                                *result = Some(SharedString::from(cap_output(output)));
                            }
                            break;
                        }
                    }
                }
            }
            AgentEvent::Usage { used, size } => {
                thread.tokens_used = used.min(u32::MAX as u64) as u32;
                if size > 0 {
                    thread.tokens_max = size.min(u32::MAX as u64) as u32;
                }
                // アクティブ表示中のスレッドは生成中もカウントアップ補間で寄せる（UsageUpdate が
                // まばらでも数字が滑らかに増える）。非表示スレッド・非アクティブ窓は演出不要なので
                // 即時同期し idle 0% を保つ。
                if thread_index == active && panel_visibly_active {
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
                let advertised_current = SharedString::from(current);
                // Configs と同じく、スレッドが望むモード（前タブから引き継いだ `permission_mode`）を正とする。
                // 広告に在れば `set_mode` でエージェントを合わせ、無ければ広告 current を採用する。
                let desired = thread.permission_mode.clone();
                // 静的候補（小文字 "bypass permissions"）と広告名（Title Case "Bypass Permissions"）は
                // 表記が食い違うため、完全一致だと必ず None に落ちて default へ戻っていた（bypass が効かない元凶）。
                // 大文字小文字を無視して照合し、一致したら permission_mode を広告名へ正規化してピル表示も揃える。
                let desired_id = thread
                    .available_modes
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(&desired))
                    .map(|(id, name)| (id.clone(), name.clone()));
                match desired_id {
                    Some((mode_id, canonical_name)) => {
                        if mode_id != advertised_current {
                            if let Some(command_tx) = &thread.command_tx {
                                command_tx
                                    .unbounded_send(SessionCommand::SetMode(mode_id.to_string()))
                                    .ok();
                            }
                        }
                        thread.current_mode_id = mode_id;
                        thread.permission_mode = canonical_name; // 広告名へ正規化（以後の照合は完全一致で通る）
                    }
                    None => {
                        // desired を提供しないエージェント → 広告 current を採用（フォールバック）。
                        if let Some((_, name)) = thread
                            .available_modes
                            .iter()
                            .find(|(id, _)| *id == advertised_current)
                        {
                            thread.permission_mode = name.clone();
                        }
                        thread.current_mode_id = advertised_current;
                    }
                }
            }
            AgentEvent::ModeChanged(id) => {
                let id = SharedString::from(id);
                if let Some((_, name)) = thread.available_modes.iter().find(|(mid, _)| *mid == id) {
                    thread.permission_mode = name.clone();
                }
                thread.current_mode_id = id;
            }
            AgentEvent::Configs(configs) => {
                // 新セッションは**自分の既定**を current として広告してくる。これを鵜呑みにすると
                // apply_thread_defaults が載せた sticky（前回選んだモデル/思考量）が毎回上書きされる＝
                // 「タブを開くたびに設定が戻る」の正体。スレッド側の選択を正とし、広告に在れば
                // `set_config` でエージェントを合わせ、**無いときだけ**広告 current を採用する。
                thread.configs = configs;
                for category in [ConfigCategory::Model, ConfigCategory::ThoughtLevel] {
                    let desired = match category {
                        ConfigCategory::Model => thread.model.clone(),
                        ConfigCategory::ThoughtLevel => thread.effort.clone(),
                        ConfigCategory::Other => continue,
                    };
                    // 広告に desired があるか / エージェントの current 表示名は何か、を借用を残さず取り出す。
                    let Some((desired_offered, current_name)) = thread
                        .configs
                        .iter()
                        .find(|config| config.category == category)
                        .map(|config| {
                            let offered = config
                                .choices
                                .iter()
                                .any(|(_, name)| name.as_str() == desired.as_ref());
                            let current_name = config
                                .choices
                                .iter()
                                .find(|(id, _)| *id == config.current)
                                .map(|(_, name)| SharedString::from(name.clone()));
                            (offered, current_name)
                        })
                    else {
                        continue;
                    };
                    if desired_offered {
                        // エージェントの current がまだ違えば合わせに行く（一致していれば無送信）。
                        let already = current_name
                            .as_ref()
                            .is_some_and(|name| name.as_ref() == desired.as_ref());
                        if !already {
                            send_set_config(thread, category, &desired);
                        }
                    } else if let Some(name) = current_name {
                        // このエージェントが提供しない選択 → 広告 current を採用（フォールバック）。
                        match category {
                            ConfigCategory::Model => thread.model = name,
                            ConfigCategory::ThoughtLevel => thread.effort = name,
                            ConfigCategory::Other => {}
                        }
                    }
                }
            }
            AgentEvent::Plan(items) => {
                // プランは毎回全量で届く（ACP 仕様）ので**置換**。常設チェックリストが追従する。
                thread.plan = items;
            }
            AgentEvent::ElicitationRequest {
                message,
                fields,
                respond,
            } => {
                // 選択肢付き質問。回答するまで composer 上部にカードを出す（承認カードと同じ Blocked）。
                thread.digest = digest_tail(&message).or(thread.digest.take());
                thread.pending_elicitation = Some(PendingElicitation {
                    message: SharedString::from(message),
                    fields,
                    selections: std::collections::BTreeMap::new(),
                    respond,
                });
            }
            AgentEvent::PermissionRequest {
                title,
                diffs,
                raw_input,
                options,
                respond,
            } => {
                // checkpoint は**書かれる前**にここで切る（M12-2）。手動でも自動 Allow でも一本道。
                // 変更前内容は**常にディスクから全文を読む**。diff の old_text は Edit ツールだと
                // **置換ハンクの断片**であり、全文として保存すると restore がファイルを断片へ
                // 破壊する（2026-08-28 に agent_panel.rs 8700 行 → 5 行の実害）。読めない＝
                // checkpoint 時点で不在（新規作成）として None を残す（restore は削除で戻す）。
                // 読み書きは背景（Host は UI スレッド禁止）＋ 自動 Allow の応答はスナップショット
                // 完了**後**（エージェントが書く前に読む＝レース防止）。
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
                // bypass permissions 選択中は UI 側でも自動 Allow する。エージェント側への
                // `set_mode` 反映にはレースがあり（初回ターン・ターン中切替は次ターンまで既定
                // モードのまま＝JOURNAL 2026-08-28）、その窓で届いた許可リクエストでユーザーを
                // 止めない。応答はスナップショット完了後＝checkpoint の一本道は env 自動と共通。
                let bypass_mode = thread.permission_mode.to_lowercase().contains("bypass");
                let auto_allow = bypass_mode || std::env::var_os("NECODER_AUTO_ALLOW").is_some();
                let auto_choice = options
                    .iter()
                    .position(|option| {
                        matches!(
                            option.kind,
                            PermissionKind::Allow | PermissionKind::AllowAlways
                        )
                    })
                    .unwrap_or(0);
                let snapshot_paths: Vec<std::path::PathBuf> = diffs
                    .iter()
                    .map(|diff| std::path::PathBuf::from(&diff.path))
                    .collect();
                let storage = self.storage.clone();
                let host = self.dest_host.clone();
                let thread_id = thread.id.clone();
                let label =
                    i18n::t!("agent.permission_files", "title" => title, "n" => diffs.len());
                if !auto_allow {
                    // 遷移スナップショット（P1 素材①）: Blocked = 何の許可を待っているか。
                    thread.digest = Some(flatten_digest_line(&title));
                    thread.pending_permission = Some(PendingPermission {
                        title: SharedString::from(title),
                        diffs,
                        raw_input: raw_input.map(SharedString::from),
                        options,
                        respond: respond.clone(),
                        since: std::time::Instant::now(),
                    });
                }
                if !snapshot_paths.is_empty() && storage.is_some() {
                    let entry_label = label.clone();
                    let thread_id_for_entry = thread_id.clone();
                    cx.spawn(async move |panel, cx| {
                        let checkpoint_id =
                            cx.background_executor()
                                .spawn(async move {
                                    let blobs = storage::default_blobs_dir()?;
                                    let storage = storage?;
                                    let snapshot: Vec<(std::path::PathBuf, Option<String>)> =
                                        snapshot_paths
                                            .into_iter()
                                            .map(|path| {
                                                // 書かれる前の今、ディスクの全文を読む（old_text は断片の
                                                // ことがあるので使わない）。
                                                let content = host.read_file(&path).ok().and_then(
                                                    |content| String::from_utf8(content.bytes).ok(),
                                                );
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
                // 承認待ちの間もターンは継続中（running のまま＝祈るマスコットで「待ち」を示す）。
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
                    .push(Entry::Agent(SharedString::from(i18n::t!(
                        "agent.error_prefix",
                        "message" => &error
                    ))));
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
                    thread_index,
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
        if elicitation_waiting {
            self.sync_running_registry(cx); // Blocked（回答待ち）をレール/フッター/⌘O へ即時反映
        }
        if start_token_ticker {
            self.ensure_token_ticker(cx);
        }
        if celebrate_now {
            self.start_celebrate(cx);
        }
        if stream_updated && thread_index == active {
            self.sync_live_reveal(reveal_reset, ensure_reveal, cx);
            if ensure_reveal {
                self.follow_transcript_if_at_bottom();
            }
        }
        // ターン開始（ACP 実送信）をレール/フッター/⌘O のロールアップへ即時反映（生成中を灯す）。
        if turn_started {
            self.sync_running_registry(cx);
        }
        // ターン完了で、アクティブスレッドに送信待ちがあれば次を自動フラッシュ（キュー→新ターン）。
        // active 限定なのは `flush_queued_prompt`（＝`send_prompt_text`）が active 宛のため。
        // 裏スレッドのキューは、そのタブへ切り替えた時に `switch_thread` が流す。
        if turn_finished && thread_index == active {
            self.flush_queued_prompt(cx);
        }
        cx.notify();
    }

    /// 実行中の末尾が Agent/Thinking なら (表示文字数, ストリーム同一鍵) を返す。鍵は
    /// thread.id + entry index + kind で、変わったら別ストリーム＝タイプライタを仕切り直す。
    fn live_stream_target(&self) -> Option<(usize, SharedString)> {
        let thread = self.threads.get(self.active)?;
        if !thread.running {
            return None;
        }
        let (text, kind_tag) = match thread.entries.last()? {
            Entry::Agent(text) => (text, "agent"),
            Entry::Thinking(text) => (text, "thinking"),
            _ => return None,
        };
        let key = SharedString::from(format!(
            "{}:{}:{kind_tag}",
            thread.id,
            thread.entries.len() - 1
        ));
        Some((text.chars().count(), key))
    }

    /// タイプライタの reveal（今どこまで打ったか）を現在の末尾本文へ同期する。別ビューを廃し
    /// 生成中も `push_selectable` の選択可能パスで描くため、パネル側は表示末尾だけを持つ
    /// （旧 sync_streaming_text の後継・2026-08-26 一本化）。`animate=false` は即全開示。
    fn sync_live_reveal(&mut self, reset: bool, animate: bool, cx: &mut Context<Self>) {
        let Some((target, key)) = self.live_stream_target() else {
            self.live_key = None;
            return;
        };
        let new_stream = self.live_key.as_ref() != Some(&key);
        self.live_key = Some(key);
        if !animate {
            self.live_reveal = target;
        } else if reset {
            self.live_reveal = 0;
        } else if new_stream {
            // 既に長い本文へタブを切り替えた時は打ち直さず、短い新エントリだけ先頭から打つ。
            self.live_reveal = if target <= 256 { 0 } else { target };
        } else {
            self.live_reveal = self.live_reveal.min(target);
        }
        if animate {
            self.ensure_live_ticker(cx);
        }
    }

    /// タイプライタの 40ms 時計。reveal を末尾へ寄せ、パネルへ notify する。旧 StreamingTextView の
    /// 別クロックと違い通知先はパネル本体なので、生成中の本文も過去本文と同じ選択機構に載る。
    /// 多重起動しない・追いついたら止まる・パネルが非表示になったら即全開示して止まる。
    fn ensure_live_ticker(&mut self, cx: &mut Context<Self>) {
        if self.live_ticker {
            return;
        }
        let Some((target, _)) = self.live_stream_target() else {
            return;
        };
        if self.live_reveal >= target {
            return;
        }
        self.live_ticker = true;
        cx.spawn(async move |panel, cx| {
            loop {
                let done = panel
                    .update(cx, |panel, cx| {
                        let Some((target, _)) = panel.live_stream_target() else {
                            return true;
                        };
                        // パネルが最近描画されていない（タブ切替/非表示）なら演出を打ち切り即全開示。
                        let visible = panel.window_active
                            && panel.live_rendered_at.is_some_and(|at| {
                                at.elapsed() < std::time::Duration::from_millis(750)
                            });
                        if !visible {
                            panel.live_reveal = target;
                            return true;
                        }
                        if panel.live_reveal < target {
                            let remaining = target - panel.live_reveal;
                            // 初速を上げ末尾でも 16 文字/40ms（＝400 字/秒）＝ ACP の到着に UI が遅れない。
                            let step = (remaining / 3).max(16);
                            panel.live_reveal = panel.live_reveal.saturating_add(step).min(target);
                            panel.follow_transcript_if_at_bottom();
                            cx.notify();
                        }
                        panel.live_reveal >= target
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(40))
                    .await;
            }
            panel
                .update(cx, |panel, cx| {
                    panel.live_ticker = false;
                    // 停止と最後のチャンク到着が競っていたら打ち直しをかける。
                    if let Some((target, _)) = panel.live_stream_target() {
                        if panel.live_reveal < target {
                            cx.notify();
                        }
                    }
                })
                .ok();
        })
        .detach();
    }

    fn ensure_second_ticker(&mut self, cx: &mut Context<Self>) {
        if self.second_ticker || !self.active_thread_running() || !self.window_active {
            return;
        }
        self.second_ticker = true;
        cx.spawn(async move |panel, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            let keep_running = panel
                .update(cx, |panel, cx| {
                    let keep_running = panel.is_visibly_active() && panel.active_thread_running();
                    if keep_running {
                        cx.notify();
                    } else {
                        panel.second_ticker = false;
                    }
                    keep_running
                })
                .unwrap_or(false);
            if !keep_running {
                break;
            }
        })
        .detach();
    }

    /// 成功直後にマスコットをバンザイさせる。約2.4秒で Idle に戻す（世代番号で古いタイマーを無効化）。
    /// バンザイ中も共有の離散時計だけで再生し、独立した描画ループは増やさない。
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

    /// トークン表示のカウントアップ補間を回す（多重起動しない）。アクティブ表示中は生成中も含め、
    /// `tokens_shown` を `tokens_used` へ 20fps で寄せる（UsageUpdate がまばらでも滑らかに増える）。
    /// 追いついたら停止し、次の UsageUpdate で再起動する（非表示・非アクティブ時は即時同期）。
    fn ensure_token_ticker(&mut self, cx: &mut Context<Self>) {
        if self.token_ticker {
            return;
        }
        self.token_ticker = true;
        cx.spawn(async move |panel, cx| {
            loop {
                let done = panel
                    .update(cx, |panel, cx| {
                        if !panel.is_visibly_active() || cx.reduce_motion() {
                            if let Some(thread) = panel.threads.get_mut(panel.active) {
                                thread.tokens_shown = thread.tokens_used as f32;
                            }
                            return true;
                        }
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
                    .timer(std::time::Duration::from_millis(50))
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
        let result_line = i18n::t!("agent.summary_input_result", "result" => &last_agent);
        let input = match last_user {
            Some(user) => format!(
                "{}\n\n{result_line}",
                i18n::t!("agent.summary_input", "instruction" => user)
            ),
            None => result_line,
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
            let prompt = i18n::t!("agent.summary_prompt");
            let generated = cx
                .background_executor()
                .spawn(async move {
                    project::oneshot_line_on(host.as_ref(), &cwd, &input, template, &prompt, 60)
                })
                .await;
            let Ok(line) = generated else {
                return; // CLI 未導入・失敗 → Tier 1 表示のまま（静かに諦める）
            };
            let _ = panel.update(cx, |panel, cx| {
                if let Some(index) = panel.thread_index_by_id(&thread_id) {
                    let thread = &mut panel.threads[index];
                    if thread.running {
                        return; // 生成待ちの間に次ターンが始まった＝古い要約は捨てる
                    }
                    let line = SharedString::from(line);
                    thread.tier2 = Some(line.clone());
                    let name = thread.name.clone();
                    cx.emit(PanelEvent::SummaryReady {
                        thread: name,
                        tier2: line,
                    });
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
            thread
                .entries
                .push(Entry::Agent(SharedString::from(i18n::t!(
                    "agent.error_prefix",
                    "message" => message
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
            .child(self.render_full_screen_toggle(cx))
    }

    /// ⤢ = AI パネルの全画面（⌘⇧⏎ と同じ）。「ファイルを見ずに vibe coding」する人のための入口を
    /// キーバインドだけに隠さない。実際の切替は workspace（レイアウトの持ち主）が行う。
    fn render_full_screen_toggle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        div()
            .id("agent-fullscreen")
            .group("agent-fullscreen")
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
                    .path("icons/maximize.svg")
                    .size(px(14.))
                    .text_color(theme.fg2)
                    .group_hover("agent-fullscreen", |style| style.text_color(theme.fg0)),
            )
            .tooltip(Tooltip::text(
                i18n::t!("agent.full_screen_tip"),
                theme.clone(),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _window, cx| cx.emit(PanelEvent::ToggleFullScreenRequest)),
            )
    }

    /// スレッド表示モードを切り替える（Bar ⇄ List）。選択は **user settings.json へ保存**して
    /// 次の起動でも保つ（`default_agent` と同経路。設定画面のトグル化は後続）。
    fn set_tabs_view(&mut self, view: AgentTabsView, cx: &mut Context<Self>) {
        if self.tabs_view != view {
            self.tabs_view = view;
            if let Some(path) = settings_core::user_settings_path() {
                let value = if matches!(view, AgentTabsView::List) {
                    "list"
                } else {
                    "bar"
                };
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
                        .child(activity_dot(
                            ("row-dot", index),
                            8.0,
                            color,
                            thread.activity(),
                        ))
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

    fn current_mascot_motion(&self) -> MascotMotion {
        if cfg!(debug_assertions) {
            if let Ok(value) = std::env::var("NECODER_MASCOT") {
                if let Some(motion) = match value.as_str() {
                    "plead" => Some(MascotMotion::Plead),
                    "worry" => Some(MascotMotion::Worry),
                    "typing" => Some(MascotMotion::Typing),
                    "think" => Some(MascotMotion::Think),
                    "celebrate" => Some(MascotMotion::Celebrate),
                    "idle" => Some(MascotMotion::Idle),
                    _ => None,
                } {
                    return motion;
                }
            }
        }
        let Some(thread) = self.threads.get(self.active) else {
            return MascotMotion::Idle;
        };
        if let Some(pending) = thread.pending_permission.as_ref() {
            return if pending.since.elapsed().as_secs() >= 15 {
                MascotMotion::Worry
            } else {
                MascotMotion::Plead
            };
        }
        if thread.running {
            return if thread
                .entries
                .last()
                .is_some_and(|entry| matches!(entry, Entry::Thinking(_)))
            {
                MascotMotion::Think
            } else {
                MascotMotion::Typing
            };
        }
        if self.celebrating {
            MascotMotion::Celebrate
        } else {
            MascotMotion::Idle
        }
    }

    /// 直近のユーザー発話（＝いまの問い）を transcript 上部に固定表示する帯。長い回答を
    /// スクロールしても「何を頼んだか」を見失わないための常設リマインダで、tail-follow で
    /// 空きがちな上部の余白も埋める。クリックで最新（回答の末尾）へスクロールする。
    fn render_pinned_prompt(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let entries = &self.threads.get(self.active)?.entries;
        let last_user = entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::User(_)))?;
        // 直近の問いがビューポート上端より上に隠れている時だけ固定表示する。まだ見えている
        // （＝上部に余白が無い短いスレッド）なら transcript の User エントリと重複させない。
        let top_item = self.transcript_list.logical_scroll_top().item_ix;
        if last_user >= top_item {
            return None;
        }
        let prompt = match &entries[last_user] {
            Entry::User(text) => text.clone(),
            _ => return None,
        };
        let theme = self.theme.clone();
        let color = self.active_color();
        Some(
            div()
                .id("pinned-prompt")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .bg(theme.bg1)
                .border_b_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|style| style.bg(theme.bg2))
                // 左の色バーはユーザー発話（transcript の User エントリ）と同じ識別色で「あなたの問い」を示す。
                .child(
                    div()
                        .flex_none()
                        .w(px(2.))
                        .h(px(14.))
                        .rounded(px(1.))
                        .bg(color.alpha(0.7)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate() // 1 行に省略（全文は transcript 側にある）
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .child(prompt),
                )
                .tooltip(Tooltip::text(
                    i18n::t!("agent.jump_to_latest"),
                    theme.clone(),
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.transcript_list.scroll_to_end();
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    fn render_meta(&self, active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        self.transcript_spinner
            .update(cx, |spinner, cx| spinner.set_color(color, cx));
        self.composer_spinner
            .update(cx, |spinner, cx| spinner.set_color(color, cx));
        let thread = self.threads.get(self.active);
        let (shown, max) = thread
            .map(|thread| (thread.tokens_shown, thread.tokens_max))
            .unwrap_or((0.0, 0));
        let used = shown.round().max(0.0) as u32; // 表示は補間値（カウントアップ演出）
                                                  // コンパクト（/compact）: セッションがあり文脈が溜まっていて、実行中でない時だけ出す。
        let has_session = thread
            .map(|thread| thread.command_tx.is_some())
            .unwrap_or(false);
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
        let blocked_since = thread.and_then(|thread| {
            thread
                .pending_permission
                .as_ref()
                .map(|pending| pending.since)
        });
        // マスコットの状態: 承認待ち→祈る（15s 以上待たされたら頬に手であわあわへ段階変化）/ 考え中→考える /
        // 生成中→打鍵 / 直近成功→バンザイ / それ以外→低頻度の居眠り。
        let motion = self.current_mascot_motion();
        self.mascot.update(cx, |mascot, cx| {
            mascot.set_state(motion, active, cx);
        });
        // 左のステータス行（スカスカ対策＋「何が起きてるか」）: 状態テキスト＋実行中は経過秒。
        let elapsed = running
            .then(|| thread.and_then(|thread| thread.turn_started_at))
            .flatten()
            .map(|start| start.elapsed().as_secs());
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
                        .child(format!("{secs}s")),
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
            .child(
                self.mascot.clone().cached(
                    StyleRefinement::default()
                        .w(px(64.0 * 60.0 / 72.0))
                        .h(px(64.)),
                ),
            )
            // トークンは常時可視（Zed+ACP で見えなかった痛点）
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    // 右側クラスタを固定幅にする。値の桁数や compact ボタンの出没で、左隣の猫が
                    // 一瞬横へ跳ねると親 surface 全体の再配置を招くため、全スロットを常時確保する。
                    .w(px(174.))
                    .flex_none()
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
                            .w(px(72.))
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.fg2)
                            .font_family("Guguru Sans Code")
                            .child(format!("{}/{}", human_tokens(used), human_tokens(max))),
                    )
                    // コンパクト（/compact）ボタン: トークンの真横。文脈が溜まっていて実行中でない時だけ。
                    .child(if has_session && !running && used > 0 {
                        let theme = theme.clone();
                        div()
                            .id("compact-context")
                            .group("compact-context")
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_none()
                            .size(px(18.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.bg3))
                            .child(
                                svg()
                                    .path("icons/minimize.svg")
                                    .size(px(12.))
                                    .text_color(theme.fg2)
                                    .group_hover("compact-context", |style| {
                                        style.text_color(theme.fg0)
                                    }),
                            )
                            .tooltip(Tooltip::text(i18n::t!("agent.compact_tip"), theme.clone()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _window, cx| this.compact_context(cx)),
                            )
                            .into_any_element()
                    } else {
                        // 非表示でも同じ 18px を占有し、猫とメーターの位置を不変にする。
                        div().size(px(18.)).flex_none().into_any_element()
                    }),
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

    /// 「最新へ」ボタン（transcript を遡って読んでいる間だけ右下に浮かべる・出現時フェード＋せり上がり）。
    /// エージェントがどんどん進んで下に新着が溜まった時に、1 タップで最下部（最新）へ戻す。底に居る時は出さない。
    /// 判定は `follow_transcript_if_at_bottom` と同系（offset.y は下ほど負・GPUI）。閾はヒステリシスのため広め。
    fn render_jump_to_latest(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        // 底から十分離れている閾（follow の 60px より広め＝ヒステリシスで出没のチラつきを防ぐ）。
        let _scrolled_up_threshold = px(140.);
        // ListState は可変高項目の tail-follow 状態を保持する。ユーザーが底から離れた時だけ出す。
        let scrolled_up = self.transcript_list.is_scrolled_to_end() == Some(false);
        if !scrolled_up {
            return None;
        }
        let theme = self.theme.clone();
        Some(
            div()
                .absolute()
                .bottom(px(12.))
                // 上（前の指示へ）ボタンの左隣の固定スロット。上ボタンが端で不動なので、
                // このボタンが出没しても上ボタンは動かない。
                .right(px(50.))
                .child(
                    // 色は識別に集約する規律に従い、ボタン自体は中立色（bg2/border/fg1）。ホバーで少し持ち上げる。
                    div()
                        .id("jump-to-latest")
                        .size(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(3.),
                            gpui::hsla(0., 0., 0., 0.35),
                        )
                        .blur_radius(px(10.))])
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        // 上ボタンの「↑」と対にした素の下矢印。DL アイコン（arrow-down-to-line）は
                        // ダウンロードに見えて紛らわしいため、テキスト矢印で最新へ戻る意味を素直に表す。
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg1)
                        .child("↓")
                        .tooltip(Tooltip::text(
                            i18n::t!("agent.jump_to_latest"),
                            theme.clone(),
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                cx.stop_propagation(); // transcript のドラッグ選択開始と二重発火しない
                                this.transcript_list.scroll_to_end();
                                cx.notify();
                            }),
                        ),
                )
                .with_animation(
                    "jump-to-latest-in",
                    Animation::new(std::time::Duration::from_millis(140))
                        .with_easing(gpui::ease_out_quint()),
                    |element, delta| {
                        element
                            .opacity(delta)
                            .bottom(px(12.0 - 6.0 * (1.0 - delta)))
                    },
                )
                .into_any_element(),
        )
    }

    /// 「前の指示へ」ボタン。長いツール出力や回答を、ユーザー発話単位で一気に遡る。
    fn render_jump_to_previous_user(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let entries = &self.threads.get(self.active)?.entries;
        let target =
            previous_user_entry_index(entries, self.transcript_list.logical_scroll_top().item_ix);
        target?;

        // 常に右端（14px）固定。連打で前の指示へ遡る間、下ボタンの出没でこのボタンが
        // 横にずれてカーソルから逃げないようにする（下ボタンは左隣の 50px スロットへ出す）。
        let theme = self.theme.clone();
        Some(
            div()
                .absolute()
                .bottom(px(12.))
                .right(px(14.))
                .child(
                    div()
                        .id("jump-to-previous-user")
                        .size(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .shadow(vec![gpui::BoxShadow::new(
                            px(0.),
                            px(3.),
                            gpui::hsla(0., 0., 0., 0.35),
                        )
                        .blur_radius(px(10.))])
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg1)
                        .child("↑")
                        .tooltip(Tooltip::text(
                            i18n::t!("agent.jump_to_previous_user"),
                            theme,
                        ))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| {
                                cx.stop_propagation();
                                this.jump_to_previous_user_entry(cx);
                            }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_transcript(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme.clone();
        if self.active_thread_running() {
            // 生成中の本文が今フレーム描かれる＝タイプライタを進め、描画時刻を記録する
            // （非表示になったら ticker がこの時刻の古さを見て即全開示する）。
            self.sync_live_reveal(false, self.window_active && !cx.reduce_motion(), cx);
            self.live_rendered_at = Some(std::time::Instant::now());
        }
        // 可変高 ListState は可視範囲だけ request_layout/prepaint/paint する。件数の差分だけ splice し、
        // 既に測った過去 Entry の高さとスクロール位置は保持する。
        let item_count = self.transcript_item_count();
        let previous_count = self.transcript_list.item_count();
        if item_count > previous_count {
            self.transcript_list
                .splice(previous_count..previous_count, item_count - previous_count);
        } else if item_count < previous_count {
            self.transcript_list.splice(item_count..previous_count, 0);
        }
        if self.active_thread_running() {
            let last = item_count.saturating_sub(1);
            self.transcript_list.remeasure_items(last..item_count);
        }
        // 選択リージョンは可視 Entry だけをフレーム毎に作り直す。巨大 transcript でも offscreen の
        // StyledText/TextLayout を構築しない（ドラッグ選択は現在のビューポート内を対象にする）。
        self.transcript_regions.borrow_mut().clear();
        div()
            .id("agent-transcript")
            .track_focus(&self.transcript_focus)
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(theme.bg1)
            .cursor(CursorStyle::IBeam)
            // List が wheel を処理し、親は浮遊ボタンの出没だけ再評価する。
            .on_scroll_wheel(cx.listener(|_, _: &gpui::ScrollWheelEvent, _, cx| cx.notify()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::on_transcript_mouse_down),
            )
            // move は root 側で拾う（`on_mouse_move` は hover 中しか発火せず、composer 上へ
            // 引っ張った途端に途切れるため。composer リサイズと同じ作法）。
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_transcript_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_transcript_mouse_up))
            .child(
                list(
                    self.transcript_list.clone(),
                    cx.processor(Self::render_transcript_item),
                )
                .flex_1(),
            )
    }

    fn render_transcript_item(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme.clone();
        let color = self.active_color();
        let Some(thread) = self.threads.get(self.active) else {
            return div()
                .h(px(180.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .text_color(theme.fg2)
                .child(SharedString::from(i18n::t!("agent.empty")))
                .into_any_element();
        };
        let entry_count = thread.entries.len();
        if index >= entry_count {
            let digest = thread
                .live_digest()
                .unwrap_or_else(|| SharedString::from(i18n::t!("agent.running_thinking")));
            return div()
                .px(px(12.))
                .pt(px(6.))
                .pb(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.5))
                .text_color(theme.fg2)
                .child(
                    self.transcript_spinner
                        .clone()
                        .cached(StyleRefinement::default().size(px(14.))),
                )
                .child(digest)
                .into_any_element();
        }

        let live_stream = index + 1 == entry_count
            && thread.running
            && matches!(thread.entries[index], Entry::Agent(_) | Entry::Thinking(_));
        let entry = &thread.entries[index];
        let rendered_entry = self.render_entry(index, entry, color, live_stream, cx);
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
            .tooltip(Tooltip::text(i18n::t!("agent.copy_tip"), theme))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    cx.stop_propagation();
                    if let Some(entry) = this
                        .threads
                        .get(this.active)
                        .and_then(|thread| thread.entries.get(index))
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(entry_plain_text(entry)));
                    }
                }),
            );
        div()
            .px(px(12.))
            .pt(if index == 0 { px(12.) } else { px(0.) })
            .pb(px(13.))
            .child(
                div()
                    .relative()
                    .group("transcript-entry")
                    .child(rendered_entry)
                    .child(copy_button)
                    .with_animation(
                        ("transcript-entry", index),
                        Animation::new(std::time::Duration::from_millis(200)),
                        |element, delta| element.opacity(delta),
                    ),
            )
            .into_any_element()
    }

    fn render_entry(
        &self,
        index: usize,
        entry: &Entry,
        color: Hsla,
        live_stream: bool,
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
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.fg0)
                        .child(self.selectable_text(text.clone(), cx)),
                )
                .into_any_element(),
            // 思考ブロック（Claude Code 風・印っぽく）: 既定は折り畳み（✳ Thought + 1 行プレビュー）で
            // クリックでいつでも全文展開。実行中の最終ブロックは自動展開し、状態文とマスコットで
            // 「動いている」を示す。ここでは独立した無限 pulse を増やさない。
            Entry::Thinking(text) => {
                let live = live_stream;
                let expanded = live || self.is_thought_expanded(index);
                let star = if live {
                    div()
                        .flex_none()
                        .text_color(color)
                        .child("✳")
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
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(3.))
                    .pl(px(11.))
                    .child(header);
                if expanded {
                    // 生成中も完了後も同じ選択可能パス。生成中は reveal 分の接頭辞だけ見せる。
                    let shown = if live {
                        revealed_prefix(text, self.live_reveal)
                    } else {
                        text.clone()
                    };
                    let body = self.push_selectable(shown, Vec::new(), cx);
                    column = column.child(
                        div()
                            .text_size(px(11.5))
                            .italic()
                            .text_color(theme.fg2)
                            .child(body),
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
            Entry::Step {
                tool,
                args,
                result,
                diffs,
                ..
            } => {
                let result_language = infer_language_from_tool_argument(args.as_ref());
                // Claude の Write/Edit はタイトル自体にパスを載せる（`Write /very/long/path`）ので、
                // 同じパスを args としてもう一度出さない（同じ文字列が 1 行に 2 回並ぶのを防ぐ）。
                let show_args = !args.is_empty() && !tool.contains(args.as_ref());
                // 長い/複数行の引数（Bash のコマンド等）は既定で 1 行に畳み、クリックで展開する
                // （結果 ⎿ と同じ流儀）。短いパス・短いコマンドはそのまま見せる。
                let args_collapsible = show_args
                    && (args.contains('\n') || args.chars().count() > STEP_ARGS_COLLAPSE_MIN_CHARS);
                // ツール名もパスも長さの上限が無い（エージェント任せ）。**必ずパネル内に収める**ため、
                // 「残りを取って折返す」役を 1 行にちょうど 1 つだけ置く（= `flex_1` + `min_w_0`。
                // モジュール冒頭「伸縮テキストの作法」）。短いツール名かつ折り畳み不要な引数の時だけ
                // パスをその直後に並べる（折り畳む引数は下の専用行へ回す）。
                let inline_args = show_args
                    && !args_collapsible
                    && tool.chars().count() <= STEP_TITLE_INLINE_MAX_CHARS;
                let title = if inline_args {
                    div().flex_none() // 自然幅＝パスがタイトルの直後に来る
                } else {
                    div().flex_1().min_w_0() // 残り全幅を取って折返す
                };
                let mut body = div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            // baseline 揃えだとパスが折返した時にツール名が 1 行ぶん沈む（⏺ とズレる）。
                            .items_start()
                            .gap(px(6.))
                            .child(
                                title
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.fg0)
                                    .child(self.selectable_text(tool.clone(), cx)),
                            )
                            .when(inline_args, |element| {
                                element.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .font_family("Guguru Sans Code")
                                        .text_size(px(11.))
                                        .text_color(theme.fg2)
                                        .child(self.selectable_text(args.clone(), cx)),
                                )
                            }),
                    );
                // 引数を専用行で出す（inline に収めなかった場合）。
                if show_args && !inline_args {
                    if args_collapsible {
                        // 長い/複数行コマンドは ▸ + 1 行プレビューに畳み、クリックで全文展開。
                        let expanded = self.is_args_expanded(index);
                        let header = div()
                            .id(("step-args", index))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .font_family("Guguru Sans Code")
                            .text_size(px(11.))
                            .text_color(theme.fg2)
                            .child(div().flex_none().text_size(px(8.)).child(if expanded {
                                "▾"
                            } else {
                                "▸"
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(step_args_preview(args.as_ref())),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.toggle_args(index, cx);
                                }),
                            );
                        let mut column = div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(3.))
                            .child(header);
                        if expanded {
                            column = column.child(
                                div()
                                    .min_w_0()
                                    .font_family("Guguru Sans Code")
                                    .text_size(px(11.))
                                    .text_color(theme.fg2)
                                    .child(self.selectable_text(args.clone(), cx)),
                            );
                        }
                        body = body.child(column);
                    } else {
                        // 長いツール名 + 別のパス＝1 行に収まらない。パスは次の行へ落とす（横に切らない）。
                        body = body.child(
                            div().flex().child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family("Guguru Sans Code")
                                    .text_size(px(11.))
                                    .text_color(theme.fg2)
                                    .child(self.selectable_text(args.clone(), cx)),
                            ),
                        );
                    }
                }
                // Edit 系: before/after 差分を transcript にインライン表示（権限カードと同じ描画を再利用）。
                for diff in diffs {
                    body = body.child(render_diff(diff, &theme));
                }
                // Bash/Read 等: 出力を ⎿ 行で（cap_output で末尾 24 行に丸め済み）。長い出力は
                // 既定で折り畳み、要約行クリックで全文展開する（コード読み込みの結果で transcript が
                // 流れないように・Thinking と同じ流儀）。
                if let Some(result) = result {
                    let line_count = result.lines().count().max(1);
                    let collapsible = line_count > STEP_COLLAPSE_MIN_LINES
                        || result.len() > STEP_COLLAPSE_MIN_BYTES;
                    let expanded = self.is_step_expanded(index);
                    // result 全体を 1 region に保ち、複数行コピー時の改行を原文どおりにする。
                    let mut row = div()
                        .flex()
                        .items_start()
                        .gap(px(4.))
                        .pt(px(3.))
                        .font_family("Guguru Sans Code")
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .child(div().flex_none().child("⎿"));
                    if collapsible {
                        let header = div()
                            .id(("step-result", index))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .cursor_pointer()
                            .child(div().flex_none().text_size(px(8.)).child(if expanded {
                                "▾"
                            } else {
                                "▸"
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(step_result_summary(result.as_ref(), line_count)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.toggle_step(index, cx);
                                }),
                            );
                        let mut column = div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(3.))
                            .child(header);
                        if expanded {
                            column = column.child(div().min_w_0().child(self.push_selectable(
                                result.clone(),
                                self.syntax_highlights(result_language, result.as_ref()),
                                cx,
                            )));
                        }
                        row = row.child(column);
                    } else {
                        row = row.child(div().flex_1().min_w_0().child(self.push_selectable(
                            result.clone(),
                            self.syntax_highlights(result_language, result.as_ref()),
                            cx,
                        )));
                    }
                    body = body.child(row);
                }
                div()
                    .flex()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .child(div().flex_none().text_color(claude_bullet()).child("⏺"))
                    .child(body)
                    .into_any_element()
            }
            // 生成中も完了後も同じ選択可能 Markdown パス（Zed の Markdown.selection 相当の一本化・
            // 2026-08-26）。生成中は reveal 分の接頭辞だけ描き、タイプライタの演出を出す。
            Entry::Agent(text) => {
                let shown = if live_stream {
                    revealed_prefix(text, self.live_reveal)
                } else {
                    text.clone()
                };
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(theme.fg0)
                    .children(self.render_markdown(index, shown.as_ref(), cx))
                    .into_any_element()
            }
        }
    }

    /// markdown テキストをブロック列（GPUI 要素）へ描く。各ブロック = 1 選択リージョン（M13）で、
    /// 装飾は `combine_highlights` で選択背景と安全に合成される。インラインコードは mono 不可
    /// （`HighlightStyle` に font-family が無い）ため syn-mac 色 + 薄背景で表す。表・引用装飾は後続。
    fn render_markdown(
        &self,
        entry_index: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = &self.theme;
        let blocks = self.markdown_cache.borrow_mut().parse(text);
        blocks
            .iter()
            .cloned()
            .enumerate()
            .map(|(block_index, block)| match block {
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
                        .child(self.push_selectable(text.into(), self.md_highlights(&spans), cx))
                        .into_any_element()
                }
                markdown::Block::Paragraph { text, spans } => div()
                    .child(self.push_selectable(text.into(), self.md_highlights(&spans), cx))
                    .into_any_element(),
                markdown::Block::Code { lang, text } => {
                    let language = lang.as_deref().and_then(lang::LanguageId::from_name);
                    let total_lines = text.lines().count();
                    let collapsible = total_lines > CODE_COLLAPSE_MIN_LINES;
                    let expanded = self.is_code_expanded(entry_index, block_index);
                    // 長いコードは既定で先頭 CODE_COLLAPSE_HEAD_LINES 行だけ見せ、残りはトグルで開く。
                    let shown_text: SharedString = if !collapsible || expanded {
                        text.clone().into()
                    } else {
                        text.lines()
                            .take(CODE_COLLAPSE_HEAD_LINES)
                            .collect::<Vec<_>>()
                            .join("\n")
                            .into()
                    };
                    let highlights = self.syntax_highlights(language, shown_text.as_ref());
                    let mut card = div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .rounded(px(6.))
                        .bg(theme.bg2)
                        .border_1()
                        .border_color(theme.border)
                        .px(px(9.))
                        .py(px(7.))
                        .font_family("Guguru Sans Code")
                        .text_size(px(11.5))
                        .text_color(theme.fg0)
                        .child(self.push_selectable(shown_text, highlights, cx));
                    if collapsible {
                        let hidden = total_lines.saturating_sub(CODE_COLLAPSE_HEAD_LINES);
                        let (glyph, label) = if expanded {
                            ("▾", SharedString::from(i18n::t!("agent.code_collapse")))
                        } else {
                            (
                                "▸",
                                SharedString::from(i18n::t!("agent.code_show_more", "n" => hidden)),
                            )
                        };
                        card = card.child(
                            div()
                                .id(("code-fold", entry_index * 256 + block_index))
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .pt(px(1.))
                                .cursor_pointer()
                                .text_size(px(11.))
                                .text_color(theme.fg2)
                                .child(glyph)
                                .child(label)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.toggle_code(entry_index, block_index, cx);
                                    }),
                                ),
                        );
                    }
                    card.into_any_element()
                }
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
                        .child(div().flex_1().min_w_0().child(self.push_selectable(
                            text.into(),
                            self.md_highlights(&spans),
                            cx,
                        )))
                        .into_any_element()
                }
                markdown::Block::Rule => div()
                    .my(px(2.))
                    .h(px(1.))
                    .bg(theme.border)
                    .into_any_element(),
                // transcript は選択リージョン機構（M13）と結合するため v1 はテキスト表現に留める
                // （パス/URL が ⌘C で取れる方が実用的。インライン画像描画は .md プレビュー側）。
                markdown::Block::Image { source, alt } => {
                    let label: SharedString = if alt.is_empty() {
                        source.into()
                    } else {
                        format!("{alt} — {source}").into()
                    };
                    let link = (
                        0..label.len(),
                        HighlightStyle {
                            color: Some(theme.syntax.function),
                            ..Default::default()
                        },
                    );
                    div()
                        .flex()
                        .gap(px(7.))
                        .child(div().flex_none().text_color(theme.fg2).child("🖼"))
                        .child(div().flex_1().min_w_0().child(self.push_selectable(
                            label,
                            vec![link],
                            cx,
                        )))
                        .into_any_element()
                }
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

    /// tree-sitter の意味種別を ACP の現在テーマへ変換する。結果は `(言語, 本文 hash)` でキャッシュ済み。
    fn syntax_highlights(
        &self,
        language: Option<lang::LanguageId>,
        text: &str,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(language) = language else {
            return Vec::new();
        };
        let syntax = &self.theme.syntax;
        self.syntax_cache
            .borrow_mut()
            .highlight(language, text)
            .into_iter()
            .map(|span| {
                let color = match span.kind {
                    lang::HighlightKind::Keyword | lang::HighlightKind::Heading => syntax.keyword,
                    lang::HighlightKind::Function | lang::HighlightKind::Link => syntax.function,
                    lang::HighlightKind::Type | lang::HighlightKind::Strong => syntax.type_,
                    lang::HighlightKind::String | lang::HighlightKind::Code => syntax.string,
                    lang::HighlightKind::Number => syntax.number,
                    lang::HighlightKind::Comment => syntax.comment,
                    lang::HighlightKind::Macro | lang::HighlightKind::Emphasis => syntax.macro_,
                    lang::HighlightKind::Punctuation => syntax.punctuation,
                };
                (
                    span.range,
                    HighlightStyle {
                        color: Some(color),
                        ..Default::default()
                    },
                )
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

    fn is_step_expanded(&self, index: usize) -> bool {
        self.threads
            .get(self.active)
            .is_some_and(|thread| self.expanded_steps.contains(&(thread.id.clone(), index)))
    }

    /// ツール結果（⎿）の折り畳み/展開をトグルする（要約行クリック）。
    fn toggle_step(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .threads
            .get(self.active)
            .map(|thread| thread.id.clone())
        else {
            return;
        };
        let key = (id, index);
        if !self.expanded_steps.remove(&key) {
            self.expanded_steps.insert(key);
        }
        cx.notify();
    }

    fn is_args_expanded(&self, index: usize) -> bool {
        self.threads
            .get(self.active)
            .is_some_and(|thread| self.expanded_args.contains(&(thread.id.clone(), index)))
    }

    /// ツール引数（⏺ の引数行）の折り畳み/展開をトグルする（引数行クリック）。
    fn toggle_args(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .threads
            .get(self.active)
            .map(|thread| thread.id.clone())
        else {
            return;
        };
        let key = (id, index);
        if !self.expanded_args.remove(&key) {
            self.expanded_args.insert(key);
        }
        cx.notify();
    }

    fn is_code_expanded(&self, entry_index: usize, block_index: usize) -> bool {
        self.threads.get(self.active).is_some_and(|thread| {
            self.expanded_code
                .contains(&(thread.id.clone(), entry_index, block_index))
        })
    }

    /// AI 応答内コードブロックの折り畳み/展開をトグルする（フッタ行クリック）。
    fn toggle_code(&mut self, entry_index: usize, block_index: usize, cx: &mut Context<Self>) {
        let Some(id) = self
            .threads
            .get(self.active)
            .map(|thread| thread.id.clone())
        else {
            return;
        };
        let key = (id, entry_index, block_index);
        if !self.expanded_code.remove(&key) {
            self.expanded_code.insert(key);
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
    /// Elicitation（選択肢付き質問・単一選択のみ）を composer 上部にカードで出す。質問文 + 各
    /// フィールドの選択肢ボタン + フッタ（複数フィールドなら「これで回答」・常に「答えない」）。
    /// 単一フィールドは選んだ瞬間に確定送信する。空なら None。
    fn render_elicitation_card(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let thread = self.threads.get(self.active)?;
        let pending = thread.pending_elicitation.as_ref()?;
        let theme = self.theme.clone();
        let color = thread.color;
        let message = pending.message.clone();
        let fields = pending.fields.clone();
        let selections = pending.selections.clone();
        let multi = fields.len() > 1;
        let all_selected = fields
            .iter()
            .all(|field| selections.contains_key(&field.name));

        let mut card = div()
            .id("elicitation-card")
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
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.fg0)
                    .child(message),
            );

        let mut option_id = 0usize;
        for field in &fields {
            let mut group = div().flex().flex_col().gap(px(4.));
            if multi {
                group = group.child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.fg2)
                        .child(SharedString::from(field.label.clone())),
                );
            }
            let mut options_row = div().flex().flex_wrap().gap(px(6.));
            for option in &field.options {
                let selected = selections.get(&field.name) == Some(&option.value);
                let field_name = field.name.clone();
                let value = option.value.clone();
                option_id += 1;
                options_row = options_row.child(
                    div()
                        .id(("elicit-option", option_id))
                        .px(px(9.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if selected { color } else { theme.border })
                        .bg(if selected { theme.bg3 } else { theme.bg1 })
                        .text_size(px(11.5))
                        .text_color(if selected { theme.fg0 } else { theme.fg1 })
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child(SharedString::from(option.title.clone()))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |panel, _, _window, cx| {
                                cx.stop_propagation();
                                panel.choose_elicitation_option(
                                    field_name.clone(),
                                    value.clone(),
                                    cx,
                                );
                            }),
                        ),
                );
            }
            group = group.child(options_row);
            card = card.child(group);
        }

        let mut footer = div().flex().items_center().gap(px(8.));
        if multi {
            footer = footer.child(
                div()
                    .id("elicitation-submit")
                    .px(px(9.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(if all_selected { color } else { theme.border })
                    .text_size(px(11.))
                    .text_color(if all_selected { theme.fg0 } else { theme.fg2 })
                    .when(all_selected, |element| {
                        element.cursor_pointer().hover(|style| style.bg(theme.bg3))
                    })
                    .child(SharedString::from(i18n::t!("agent.elicitation_submit")))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|panel, _, _window, cx| {
                            cx.stop_propagation();
                            panel.submit_elicitation(cx);
                        }),
                    ),
            );
        }
        footer = footer.child(
            div()
                .id("elicitation-decline")
                .text_size(px(11.))
                .text_color(theme.fg2)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.fg0))
                .child(SharedString::from(i18n::t!("agent.elicitation_decline")))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|panel, _, _window, cx| {
                        cx.stop_propagation();
                        panel.decline_elicitation(cx);
                    }),
                ),
        );
        card = card.child(footer);
        Some(card.into_any_element())
    }

    /// 送信待ちキュー（生成中に積んだ prompt）を composer 上部に並べる。各チップは 1 行プレビュー +
    /// 「今すぐ」（steer・割り込み送信）+ ✕（取消）。空なら None（描画しない）。
    fn render_queued_prompts(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let thread = self.threads.get(self.active)?;
        if thread.queued_prompts.is_empty() {
            return None;
        }
        let theme = self.theme.clone();
        let color = thread.color;
        let prompts = thread.queued_prompts.clone();
        let mut card = div()
            .id("queued-prompts")
            .mx(px(12.))
            .mb(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(
                        i18n::t!("agent.queued_title", "n" => prompts.len()),
                    )),
            );
        for (index, prompt) in prompts.iter().enumerate() {
            let row = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(px(7.))
                .bg(theme.bg2)
                .border_1()
                .border_color(color.alpha(0.4))
                .px(px(9.))
                .py(px(5.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .text_color(theme.fg1)
                        .child(step_args_preview(prompt)),
                )
                .child(
                    div()
                        .id(("queue-send-now", index))
                        .flex_none()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(10.5))
                        .text_color(theme.fg1)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                        .child(SharedString::from(i18n::t!("agent.queue_send_now")))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |panel, _, _window, cx| {
                                cx.stop_propagation();
                                panel.send_queued_now(index, cx);
                            }),
                        ),
                )
                .child(
                    div()
                        .id(("queue-remove", index))
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(theme.fg2)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.fg0))
                        .child("✕")
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |panel, _, _window, cx| {
                                cx.stop_propagation();
                                panel.remove_queued_prompt(index, cx);
                            }),
                        ),
                );
            card = card.child(row);
        }
        Some(card.into_any_element())
    }

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
            // 折返す側（タイトル）は `flex_1` + `min_w_0` のセット（モジュール冒頭「伸縮テキストの作法」）。
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(7.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(SharedString::from(i18n::t!("agent.permission_needed"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.5))
                            .text_color(theme.fg0)
                            .child(pending.title.clone()),
                    ),
            );

        // Diff の無いツール（Bash/Fetch/MCP）は実引数（rawInput）を逐語表示する。これが無いと
        // agent 任せの title だけで承認＝tool poisoning を検知できない（ACP #1979 / GHSA-f2g4）。
        // 行ごとに child 化して改行を確実に反映（長い行はカード幅で折り返す）。
        if let Some(raw_input) = &pending.raw_input {
            let mut block = div()
                .id("permission-raw-input")
                .flex()
                .flex_col()
                .max_h(px(168.))
                .overflow_hidden()
                .rounded(px(6.))
                .bg(theme.bg1)
                .border_1()
                .border_color(theme.border)
                .px(px(8.))
                .py(px(6.))
                .font_family("Guguru Sans Code")
                .text_size(px(11.))
                .text_color(theme.fg1);
            for line in raw_input.lines() {
                block = block.child(SharedString::from(line.to_string()));
            }
            card = card.child(block);
        }

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

    /// composer 高さのドラッグ中に呼ばれる（root の on_mouse_move）。上へ動かすと高くなる。
    fn on_composer_resize_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.resizing_composer {
            return;
        }
        // 左ボタンを離した状態で来た move（枠外 up 等）は終了扱いにする。
        if event.pressed_button != Some(MouseButton::Left) {
            self.resizing_composer = false;
            cx.notify();
            return;
        }
        let dy = f32::from(event.position.y) - self.composer_resize_start_y;
        // 上縁ハンドルを上へ（dy 負）動かすと高くなる。
        self.composer_height =
            (self.composer_resize_start_height - dy).clamp(COMPOSER_INPUT_MIN, COMPOSER_INPUT_MAX);
        cx.notify();
    }

    /// composer 高さのドラッグ終了（root の on_mouse_up）。
    fn on_composer_resize_end(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.resizing_composer {
            self.resizing_composer = false;
            cx.notify();
        }
    }

    /// 親の Agent dock 幅が変わった時、composer の EditorElement を再レイアウトする。
    /// EditorView は prepaint の実 bounds から折返し桁数を計算するため、幅変更時に明示的に
    /// notify すれば、以前の dock 幅で作った wrap map をそのまま使い続けない。
    pub fn parent_width_changed(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |_composer, cx| cx.notify());
        cx.notify();
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
        // 稼働中ラインの素材（実行中のみ描く）。digest は P1 の live 合成をそのまま使う。
        let running = matches!(activity, ThreadActivity::Working);
        let active_thread = self.threads.get(self.active);
        let running_elapsed = active_thread
            .and_then(|thread| thread.turn_started_at)
            .map(|started| started.elapsed().as_secs());
        let running_tokens = active_thread.map(|thread| thread.tokens_used).unwrap_or(0);
        let running_digest = active_thread.and_then(|thread| thread.live_digest());

        div()
            .id("composer-drop")
            .flex_none()
            .w_full()
            .min_w_0()
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
                    .w_full()
                    .min_w_0()
                    .rounded(px(9.))
                    .bg(theme.bg0)
                    .border_1()
                    .border_color(color.alpha(0.5))
                    .px(px(11.))
                    .py(px(8.))
                    // 上縁リサイズハンドル: 掴んで上下ドラッグで入力欄の高さを変える（固定 68px の解消）。
                    .child(
                        div()
                            .id("composer-resize")
                            .w_full()
                            .h(px(7.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::ResizeUpDown)
                            .child(
                                div()
                                    .w(px(26.))
                                    .h(px(2.))
                                    .rounded_full()
                                    .bg(theme.fg2.alpha(0.45)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    this.resizing_composer = true;
                                    this.composer_resize_start_y = f32::from(event.position.y);
                                    this.composer_resize_start_height = this.composer_height;
                                    cx.notify();
                                }),
                            ),
                    )
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
                    // 高さは上縁ハンドルで可変（composer_height）。長文入力は内部スクロールする。
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .h(px(self.composer_height))
                            .overflow_hidden()
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
                            .child(if running {
                                // 実行中は送信ボタンを**停止**に差し替える（Esc と同じ動作）。
                                // 中立の警告色ではなく縁取りにして、識別色の意味を汚さない（§1.3）。
                                div()
                                    .id("stop-button")
                                    .px(px(12.))
                                    .py(px(2.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(theme.fg2)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.bg3).text_color(theme.fg0))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.fg1)
                                    .child(SharedString::from(i18n::t!("agent.stop")))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            this.cancel_turn(this.active, cx)
                                        }),
                                    )
                            } else {
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
                                    )
                            }),
                    )
                    // 稼働中ライン: 静止点字 + 経過秒 + トークン + live digest + 「esc で中断」。
                    // Claude Code のくるくるに相当するが、**気の利いた動詞は置かない** — 同じ場所に
                    // live digest（実際に走っているツール + plan の進行中項目）を出す方が情報量が上。
                    // 人格はマスコットが担う（出口を 2 つに割らない）。DECISIONS / UI-SPEC §11。
                    .when(running, |parent| {
                        parent.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .pt(px(6.))
                                .text_size(px(10.5))
                                .text_color(theme.fg2)
                                .child(
                                    self.composer_spinner
                                        .clone()
                                        .cached(StyleRefinement::default().size(px(15.))),
                                )
                                .child(SharedString::from(match running_elapsed {
                                    Some(secs) => format!("{secs}s"),
                                    None => String::new(),
                                }))
                                // トークンは 0 のときは出さない（ターン開始直後の「0」は情報ゼロ）。
                                // 上部 render_meta には used/max のメーターがあるのでそちらが正。
                                .children(
                                    (running_tokens > 0)
                                        .then(|| SharedString::from(human_tokens(running_tokens))),
                                )
                                .child(
                                    // digest が無いターン（起動直後・思考のみ）は中立語で埋める。
                                    div().flex_1().truncate().text_color(theme.fg1).child(
                                        running_digest.unwrap_or_else(|| {
                                            SharedString::from(i18n::t!("agent.running_thinking"))
                                        }),
                                    ),
                                )
                                .child(SharedString::from(i18n::t!("agent.running_hint"))),
                        )
                    }),
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
        self.last_rendered_at = Some(std::time::Instant::now());
        let caret_blink_enabled = !self.active_thread_running();
        self.composer.update(cx, |composer, cx| {
            composer.set_caret_blink_enabled(caret_blink_enabled, cx);
        });
        if active && self.active_thread_running() {
            self.ensure_second_ticker(cx);
        }
        div()
            .relative() // モデルメニューの絶対配置の基準
            .flex()
            .flex_col()
            .size_full() // 幅は workspace の可変ドックコンテナが決める
            .min_w_0()
            .bg(theme.bg0)
            .text_color(theme.fg0)
            // agent フォーカス時のキー割当（keymap の "AgentPanel" context に一致）。⌘W=スレッド閉じ。
            .key_context("AgentPanel")
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_close_thread))
            // transcript が focus 中の ⌘A。composer focus 中は EditorView 側が先に消費する。
            .on_action(cx.listener(Self::on_select_all_transcript))
            // transcript 選択の ⌘C（composer が空選択で譲った Copy を root で受ける・M13）。
            .on_action(cx.listener(Self::on_copy_selection))
            // Esc = transcript 選択解除／実行中の中断。
            .on_key_down(cx.listener(Self::on_panel_key_down))
            // Agent エリアのどこをクリックしても composer にフォーカス（＝⌘W がスレッドに効く）。
            // 子（タブ/ボタン）が処理した後の bubble で拾う。既に focus 済みなら no-op。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus_composer(window, cx)),
            )
            // composer 上縁ハンドルのドラッグ（root で move/up を拾い、枠外まで追従させる）。
            .on_mouse_move(cx.listener(Self::on_composer_resize_move))
            // transcript のドラッグ選択も root で拾う（composer 上へはみ出しても選択を延ばし、
            // ビュー外なら自動スクロールを起動する）。
            .on_mouse_move(cx.listener(Self::on_transcript_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_composer_resize_end))
            .child(match self.tabs_view {
                AgentTabsView::Bar => self.render_thread_tabs(cx).into_any_element(),
                AgentTabsView::List => self.render_thread_list(cx),
            })
            .child(self.render_meta(active, cx))
            // いまの問い（直近のユーザー発話）を上部に固定（長い回答でも見失わない・上部余白も埋める）。
            .children(self.render_pinned_prompt(cx))
            // エージェントの実行プラン（あれば transcript 上部に常設・M12-9）。
            .children(self.render_plan())
            // transcript は relative コンテナに入れ、右下へ「最新へ」ボタンを浮かべる（scroll 面の外＝
            // ビューポート右下に固定される。中身と一緒にスクロールしない）。
            .child(
                div()
                    .relative()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(self.render_transcript(cx))
                    .children(self.render_jump_to_previous_user(cx))
                    .children(self.render_jump_to_latest(cx)),
            )
            // 承認待ちの権限リクエスト（あれば composer の直上に常時表示）。
            .children(self.render_permission_card(cx))
            .children(self.render_elicitation_card(cx))
            .children(self.render_queued_prompts(cx))
            .child(self.render_composer(cx))
            // セレクタのドロップダウンは各ピルの子として描く（render_selector_pill 内）。
            .when(self.context_menu_open, |element| {
                element.child(self.render_context_menu(cx))
            })
    }
}

// ── 自由関数 ──

/// マスコットの状態（スレッド状態から算出し [`render_mascot`] へ渡す）。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MascotMotion {
    /// アイドル（待機）＝低頻度（5fps）の居眠りループ。
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

struct MascotAtlases {
    idle: Arc<RenderImage>,
    doze: Arc<RenderImage>,
    typing: Arc<RenderImage>,
    think: Arc<RenderImage>,
    celebrate: Arc<RenderImage>,
    plead: Arc<RenderImage>,
    worry: Arc<RenderImage>,
}

/// マスコットの PNG を **バイナリに埋め込む**（`assets/mascot/` から）。
///
/// **実行時にディスクから読んではいけない。** 2026-08-25 まで、ここは
/// `env!("CARGO_MANIFEST_DIR")` を**実行時のパス**として `image::open` に渡していた。
/// これは**ビルド機のパスを焼き込むだけ**なので、配布物は他人の PC で必ず panic する:
///
/// ```text
/// panicked at crates\agent_panel\src\agent_panel.rs:6946:
/// mascot asset D:\a\necoder\necoder\crates\agent_panel/assets/mascot\idle.png:
/// 指定されたパスが見つかりません。 (os error 3)
/// ```
///
/// `D:\a\necoder\necoder` は GitHub Actions ランナーのパス。**v0.1.0 / v0.1.1 の
/// `.dmg` と `.zip` が実際にこれで起動しなかった**（zip をダウンロードして実機で確認）。
/// 開発機ではそのパスが実在するので**手元では絶対に気づけない**種類のバグ。
/// フォント（`necoder/src/main.rs` の `load_fonts`）とアイコン（同 `Assets`）は
/// 最初から `include_bytes!` で埋め込んでいたので、マスコットだけが取り残されていた。
macro_rules! mascot_bytes {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/mascot/",
            $name
        ))
        .as_slice()
    };
}

impl MascotAtlases {
    fn load() -> Self {
        Self {
            idle: load_mascot_atlas("idle.png", mascot_bytes!("idle.png"), 1),
            doze: load_mascot_atlas("doze-strip.png", mascot_bytes!("doze-strip.png"), 15),
            typing: load_mascot_atlas("typing-strip.png", mascot_bytes!("typing-strip.png"), 15),
            think: load_mascot_atlas("think-strip.png", mascot_bytes!("think-strip.png"), 15),
            celebrate: load_mascot_atlas(
                "celebrate-strip.png",
                mascot_bytes!("celebrate-strip.png"),
                15,
            ),
            plead: load_mascot_atlas("plead-strip.png", mascot_bytes!("plead-strip.png"), 15),
            worry: load_mascot_atlas("worry-strip.png", mascot_bytes!("worry-strip.png"), 15),
        }
    }

    fn image(&self, motion: MascotMotion, active: bool) -> &Arc<RenderImage> {
        match (motion, active) {
            (MascotMotion::Idle, false) => &self.idle,
            (MascotMotion::Idle, true) => &self.doze,
            (MascotMotion::Typing, _) => &self.typing,
            (MascotMotion::Think, _) => &self.think,
            (MascotMotion::Celebrate, _) => &self.celebrate,
            (MascotMotion::Plead, _) => &self.plead,
            (MascotMotion::Worry, _) => &self.worry,
        }
    }
}

fn mascot_atlases() -> &'static MascotAtlases {
    static ATLASES: std::sync::OnceLock<MascotAtlases> = std::sync::OnceLock::new();
    ATLASES.get_or_init(MascotAtlases::load)
}

/// 埋め込んだ PNG（`mascot_bytes!`）をフレーム列へ展開する。
///
/// `name` は診断用の表示名だけに使う（**パスとして解決しない**）。ここで panic するのは
/// 「リポジトリに入れた PNG が壊れている」＝ビルド時に確定するプログラミングエラーのときだけで、
/// **実行環境には依存しない**（以前は「ファイルが無い」で落ちていたので環境依存だった）。
fn load_mascot_atlas(name: &str, bytes: &'static [u8], frame_count: usize) -> Arc<RenderImage> {
    let mut source = image::load_from_memory(bytes)
        .unwrap_or_else(|error| panic!("mascot asset {name}: {error}"))
        .into_rgba8();
    // GPUI の RenderImage は BGRA バイト順を期待するため、image が返す RGBA を入れ替える
    // （怠ると R と B が反転して暖色マスコットが青く写る）。参考: gpui elements/img.rs
    for pixel in source.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let frame_width = source.width() / frame_count as u32;
    let mut frames = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let image = image::imageops::crop_imm(
            &source,
            frame as u32 * frame_width,
            0,
            frame_width,
            source.height(),
        )
        .to_image();
        frames.push(image::Frame::new(image));
    }
    Arc::new(RenderImage::new(frames))
}

fn mascot_canvas(motion: MascotMotion, active: bool, height: f32, tick: u64) -> gpui::AnyElement {
    const FRAME_COUNT: usize = 15;
    let width = height * 60.0 / 72.0;
    let image = mascot_atlases().image(motion, active).clone();
    let frame = if active {
        tick as usize % FRAME_COUNT
    } else {
        0
    };
    div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .child(
            canvas(
                |_bounds, _window, _cx| (),
                move |bounds, (), window, _cx| {
                    let _ =
                        window.paint_image(bounds, bounds, Corners::default(), image, frame, false);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// 固定サイズの点字スピナー。タイマーが通知するのはこの Entity だけで、親 AgentPanel や
/// transcript の可視 Entry を再構築しない。reduce motion と非アクティブ窓では先頭コマで静止する。
struct BrailleSpinnerView {
    diameter: f32,
    color: Hsla,
    tick: usize,
    ticker: bool,
    last_rendered_at: Option<std::time::Instant>,
}

impl BrailleSpinnerView {
    const FRAMES: [&'static str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    fn new(diameter: f32, color: Hsla) -> Self {
        Self {
            diameter,
            color,
            tick: 0,
            ticker: false,
            last_rendered_at: None,
        }
    }

    fn set_color(&mut self, color: Hsla, cx: &mut Context<Self>) {
        if self.color != color {
            self.color = color;
            cx.notify();
        }
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker || cx.reduce_motion() {
            return;
        }
        self.ticker = true;
        cx.spawn(async move |spinner, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            let next = spinner.update(cx, |spinner, cx| {
                let visible = !cx.reduce_motion()
                    && spinner
                        .last_rendered_at
                        .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(350));
                if !visible {
                    spinner.tick = 0;
                    spinner.ticker = false;
                    return false;
                }
                spinner.tick = (spinner.tick + 1) % Self::FRAMES.len();
                cx.notify();
                true
            });
            let Ok(true) = next else {
                break;
            };
        })
        .detach();
    }
}

impl Render for BrailleSpinnerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = window.is_window_active() && !cx.reduce_motion();
        self.last_rendered_at = Some(std::time::Instant::now());
        if active {
            self.ensure_ticker(cx);
        } else {
            self.tick = 0;
        }
        let outer = self.diameter + 6.0;
        let slot = self.diameter + 4.0;
        div()
            .size(px(outer))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size(px(slot))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(slot))
                    .line_height(px(slot))
                    .text_color(self.color)
                    .child(Self::FRAMES[self.tick]),
            )
    }
}

/// 固定サイズ・独立 invalidation 境界のマスコット。親 AgentPanel / Workspace を通知せず、
/// 自分の atlas frame だけを居眠り5fps・作業10fpsで再 paint する。
pub struct MascotView {
    motion: MascotMotion,
    active: bool,
    height: f32,
    tick: u64,
    ticker: bool,
    last_rendered_at: Option<std::time::Instant>,
}

impl MascotView {
    pub fn new(height: f32) -> Self {
        Self {
            motion: MascotMotion::Idle,
            active: true,
            height,
            tick: 0,
            ticker: false,
            last_rendered_at: None,
        }
    }

    fn set_state(&mut self, motion: MascotMotion, active: bool, cx: &mut Context<Self>) {
        let active = active && !cx.reduce_motion();
        let needs_restart = active && !self.ticker;
        if active {
            // 親がこの Entity を実際の View tree に差し込んだ印。非表示中は set_state 自体が来ない。
            self.last_rendered_at = Some(std::time::Instant::now());
        }
        if self.motion == motion && self.active == active && !needs_restart {
            return;
        }
        self.motion = motion;
        self.active = active;
        if !active {
            self.tick = 0;
        }
        cx.notify();
    }

    pub fn set_fleet_state(
        &mut self,
        any_blocked: bool,
        blocked_long: bool,
        any_working: bool,
        active: bool,
        cx: &mut Context<Self>,
    ) {
        let motion = if blocked_long {
            MascotMotion::Worry
        } else if any_blocked {
            MascotMotion::Plead
        } else if any_working {
            MascotMotion::Typing
        } else {
            MascotMotion::Idle
        };
        self.set_state(motion, active, cx);
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker || !self.active {
            return;
        }
        self.ticker = true;
        cx.spawn(async move |mascot, cx| loop {
            let next_delay = mascot.update(cx, |mascot, cx| {
                let visible = mascot.active
                    && mascot
                        .last_rendered_at
                        .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(750));
                if !visible {
                    mascot.ticker = false;
                    return None;
                }
                mascot.tick = mascot.tick.wrapping_add(1);
                cx.notify();
                Some(std::time::Duration::from_millis(
                    if mascot.motion == MascotMotion::Idle {
                        200
                    } else {
                        100
                    },
                ))
            });
            let Ok(Some(delay)) = next_delay else {
                break;
            };
            cx.background_executor().timer(delay).await;
        })
        .detach();
    }
}

impl Render for MascotView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = window.is_window_active() && self.active && !cx.reduce_motion();
        if active != self.active {
            self.active = active;
        }
        self.last_rendered_at = Some(std::time::Instant::now());
        if active {
            self.ensure_ticker(cx);
        }
        mascot_canvas(self.motion, active, self.height, self.tick)
    }
}

/// スレッド表示の初期モード。**開発用 `NECODER_TABS_VIEW` を最優先**（list/bar・スクショ検証用）、
/// 無ければ**保存値**（settings.json の `agent_tabs_view`）、それも無ければ Bar。
fn initial_tabs_view(setting: &str) -> AgentTabsView {
    match std::env::var("NECODER_TABS_VIEW").ok().as_deref() {
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
/// 識別色**（UI-SPEC §1.3）で、状態は形で見せる（色を状態に使わない・mock のグリフに準拠）:
/// Idle=淡（○）/ Working=**回る点字**（⠋・生成中の可視化）/ Done=リング（✓）/ Blocked=**半円**（◐・中断 Done は中空）。
/// 動きは Working スピナーとマスコットに限り、他状態のドットは静止する。
/// リング枠のスペースは常に確保し、状態が変わってもレイアウトが揺れないようにする。
pub fn activity_dot(
    id: impl Into<gpui::ElementId>,
    diameter: f32,
    color: Hsla,
    activity: ThreadActivity,
) -> gpui::AnyElement {
    // Working = 回る点字スピナー（herdr 由来の形・色はスレッド識別色のまま＝§1.3 維持）。
    if matches!(activity, ThreadActivity::Working) {
        return working_spinner(id, diameter, color);
    }
    let ringed = matches!(activity, ThreadActivity::Done { .. });
    let half = matches!(activity, ThreadActivity::Blocked); // 承認待ち = 半円（mock ◐）
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
            .child(
                div()
                    .size(px(diameter))
                    .rounded(px(diameter / 2.0))
                    .bg(color),
            )
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

    if dim {
        framed.opacity(0.35).into_any_element()
    } else {
        framed.into_any_element()
    }
}

/// Working の点字スピナー。「生成中」の主表示なので**回す**（他状態のドットは静止・動きの主役は
/// マスコット）。`with_animation` の repeat で、この要素が描かれている間だけ回る＝Idle へ遷移して
/// activity_dot が別グリフに変わればアニメも止まり idle 0% を保つ。外枠は `activity_dot` と同寸で
/// 状態遷移してもレイアウト不変。`id` はドット毎に一意（複数スピナーを独立に回すため）。
pub fn working_spinner(
    id: impl Into<gpui::ElementId>,
    diameter: f32,
    color: Hsla,
) -> gpui::AnyElement {
    // 点字 10 コマ（U+280B 系）を一定周期で送る。delta 0→1 の repeat をコマ番号へ量子化する。
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
            div()
                .w(px(slot))
                .h(px(slot))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(slot))
                .line_height(px(slot))
                .text_color(color)
                .with_animation(
                    id.into(),
                    Animation::new(std::time::Duration::from_millis(900))
                        .repeat()
                        .with_max_fps(10.0),
                    |element, delta| {
                        let frame = ((delta * FRAMES.len() as f32) as usize).min(FRAMES.len() - 1);
                        element.child(FRAMES[frame])
                    },
                ),
        )
        .into_any_element()
}

/// スレッドが話すエージェントのブランドアイコン（タブ / List 行 / composer のエージェントピル・選択メニュー用）。ブランド在庫が無いものは
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
/// 長いラベルを中央省略で丸める（`Always Allow access to /very/long/path` 等）。
/// パスは頭も尻も意味を持つので中央を落とす。全文は tooltip で見せる。
fn ellipsize_middle(text: &str, max_chars: usize) -> SharedString {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return SharedString::from(text.to_string());
    }
    let head = max_chars / 2;
    let tail = max_chars - head - 1;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    SharedString::from(out)
}

/// 承認ボタン 1 個。ラベルは ACP エージェントが決める（長さの上限が無い）ため、
/// ドック幅でもカードからはみ出さないよう中央省略する。
const PERMISSION_LABEL_MAX_CHARS: usize = 42;

fn permission_button(
    index: usize,
    option: &PermissionChoice,
    color: Hsla,
    theme: &Theme,
    cx: &mut Context<AgentPanel>,
) -> gpui::AnyElement {
    let label = ellipsize_middle(&option.label, PERMISSION_LABEL_MAX_CHARS);
    let truncated = label.as_ref() != option.label;
    let base = div()
        .id(("permission-option", index))
        .flex()
        .flex_none()
        .items_center()
        .px(px(11.))
        .py(px(3.))
        .rounded(px(6.))
        .text_size(px(11.5))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(label)
        .when(truncated, |element| {
            element.tooltip(Tooltip::text(option.label.clone(), theme.clone()))
        })
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

/// ACP が `config_options` を返す前に見せるモデル候補。モデル一覧を持つ二つだけ vendor 別にし、
/// provider が任意のエージェントは一時的に `auto`（接続後に広告 current へ置換）とする。
fn fallback_models(agent: &str) -> &'static [&'static str] {
    match agent {
        "Claude Code" => CLAUDE_MODELS,
        "Codex" => CODEX_MODELS,
        _ => AUTO_MODELS,
    }
}

/// モデル文字列が指定エージェントの vendor に属するか。sticky やグローバル土台が別 vendor の値なら
/// 捨ててフォールバック先頭へ戻すための判定。同じ vendor なら静的一覧に無い新モデルでも保ち、
/// 接続後の ACP 広告で検証する。
fn model_belongs_to_agent(agent: &str, model: &str) -> bool {
    match agent {
        "Claude Code" => model.starts_with("claude-"),
        "Codex" => !model.is_empty() && !model.starts_with("claude-"),
        _ => model == "auto",
    }
}

/// エージェントの sticky 既定を settings から引く（`agent_defaults[agent]` を最優先）。
/// モデルは per-agent → グローバル `default_model`（土台）→ vendor フォールバック先頭の順。
/// 思考量は per-agent → グローバル `default_effort` → `None`（＝Thread の初期値を保つ）。
/// モードは per-agent → `None`（広告 or 前タブ引き継ぎに委ねる）。
fn agent_sticky_defaults(
    agent: &str,
    cx: &App,
) -> (SharedString, Option<SharedString>, Option<SharedString>) {
    let settings = settings::get(cx);
    let per_agent = settings.agent_defaults.get(agent);

    let model_candidate = per_agent
        .and_then(|defaults| defaults.model.clone())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| settings.default_model.clone());
    let model = if model_belongs_to_agent(agent, &model_candidate) {
        SharedString::from(model_candidate)
    } else {
        SharedString::from(fallback_models(agent).first().copied().unwrap_or("auto"))
    };

    let effort = per_agent
        .and_then(|defaults| defaults.effort.clone())
        .or_else(|| Some(settings.default_effort.clone()))
        .filter(|effort| !effort.is_empty())
        .map(SharedString::from);

    let mode = per_agent
        .and_then(|defaults| defaults.mode.clone())
        .filter(|mode| !mode.is_empty())
        .map(SharedString::from);

    (model, effort, mode)
}

/// 指定 agent の sticky（モデル/思考量/モード）をスレッドへ載せる。Agent ピル切替と新規スレッド生成が
/// 共有する。`thread.agent` は呼び出し側で確定済みにしておく（この関数は agent を変えない）。
fn apply_agent_sticky(thread: &mut Thread, cx: &App) {
    let (model, effort, mode) = agent_sticky_defaults(thread.agent.as_ref(), cx);
    thread.model = model;
    if let Some(effort) = effort {
        thread.effort = effort;
    }
    if let Some(mode) = mode {
        thread.permission_mode = mode;
    }
}

/// 新規スレッドへ「前回選んだ状態」を載せる（2026-07-27 ユーザー要望「前に設定した状態を保持して」）。
/// エージェント＝グローバル既定（Settings 画面でだけ変わる・§8）。モデル/思考量/モード＝**その agent で
/// ピルで選んだ最後の値**（`select_option` が `agent_defaults` へ書き戻す）。毎回 fable-5/high に戻る挙動を断つ。
fn apply_thread_defaults(thread: &mut Thread, cx: &App) {
    thread.agent = default_agent_name(cx);
    apply_agent_sticky(thread, cx);
}

/// storage の1 turn 行 (role, content) を [`Entry`] へ（復元・#5。set_storage と同じ対応）。
fn entry_from_turn((role, content): (String, String)) -> Entry {
    match role.as_str() {
        "user" => Entry::User(content.into()),
        "thinking" => Entry::Thinking(content.into()),
        "step" => Entry::Step {
            id: None,
            tool: content.into(),
            args: SharedString::default(),
            result: None,
            diffs: Vec::new(),
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
    cx: &App,
) -> Thread {
    let mut thread = Thread::empty(name.to_string(), color_index);
    // 復元スレッドも「前回選んだモデル/思考量」で開く（毎回 fable-5/high に戻らない）。
    apply_thread_defaults(&mut thread, cx);
    thread.id = id.to_string();
    let turns = storage.load_recent_turns(id, 200).unwrap_or_default();
    thread.entries = turns.into_iter().map(entry_from_turn).collect();
    thread.persisted_entries = thread.entries.len();
    thread
}

/// offscreen 検証プローブはモック transcript（markdown/Step 描画）や複数タブ（ACP_PROBE は添字 1 の
/// 空スレへ送信・ACTIVITY_PROBE は状態一覧）を前提にする。通常起動では種付けしない（新プロジェクトが
/// デモ 3 タブで始まる混乱を断つ・2026-08-17）。単独でモックが欲しい時は NECODER_DEMO_THREADS=1。
fn demo_threads_requested() -> bool {
    [
        "NECODER_DEMO_THREADS",
        "NECODER_ACP_PROBE",
        "NECODER_ACTIVITY_PROBE",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
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
        draft: String::new(),
        tokens_used: 23_400,
        tokens_max: 200_000,
        tokens_shown: 23_400.0,
        command_tx: None,
        available_modes: Vec::new(),
        current_mode_id: SharedString::default(),
        configs: Vec::new(),
        pending_permission: None,
        pending_elicitation: None,
        // 遷移スナップショット（P1）の種: offscreen 検証で herd 行/セル帯に digest が写る。
        digest: Some("結論: MVP は ropey。Buffer trait で後から差し替え可能に。".into()),
        muted: false,
        tier2: None,
        queued_prompts: Vec::new(),
        entries: vec![
            Entry::User("MVPのバッファ、ropey と Zed の sum-tree どっちに寄せるべき？".into()),
            Entry::Thinking(
                "Zed の text crate は sum-tree ベースで CRDT 前提の設計。協調編集を MVP で切るなら過剰。\
                 ropey は API が安定していて docs も厚い。undo 履歴は rope と独立に持てるので移行コストも低い…"
                    .into(),
            ),
            Entry::Step {
                id: None,
                tool: "Read".into(),
                args: "(zed/crates/text/src/text.rs)".into(),
                result: Some("1,842 行 — SumTree<Chunk> / anchor / clock::Global を確認".into()),
                diffs: Vec::new(),
            },
            Entry::Step {
                id: None,
                tool: "Update Todos".into(),
                args: "".into(),
                result: Some("☒ text crate の設計を調査\n☐ Buffer trait の切り方を決める".into()),
                diffs: Vec::new(),
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
    fn snap_highlights_never_splits_multibyte_chars() {
        // クラッシュ再現: 全角 '（'(bytes 8..11) の内側 byte 9 を跨ぐ run。gpui の layout_line は
        // この run を `split_at(9)` して abort する。サニタイズ後は全端点が文字境界に乗ること。
        let text = " TextRun（=フォントラン）のまま）";
        let style = HighlightStyle::default();
        let dirty = vec![
            (0..9, style),              // '（' の途中で終わる（mid-char）
            (9..text.len() + 5, style), // mid-char 始まり + 範囲外終わり
            (3..3, style),              // 空（捨てる）
        ];
        let snapped = snap_highlights_to_char_boundaries(text, dirty);
        for (range, _) in &snapped {
            assert!(
                text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
                "run {range:?} が文字境界に乗らない",
            );
            assert!(range.start < range.end, "空 run が残っている");
            assert!(range.end <= text.len(), "範囲外");
        }
        // 先頭 run は '（' の手前（byte 8）へ縮む。
        assert_eq!(snapped[0].0, 0..8);
    }

    #[test]
    fn streaming_overlapping_markdown_spans_stay_within_text() {
        // 再発クラッシュ: ストリーミング中の nested markdown（`***太字***` は Strong と Emphasis を
        // 同一レンジで二重に、`**`code`**` は Strong が Code を内包）は run が重なり、snap だけでは
        // 合計 run 長が本文を超えて gpui の layout_line が split_at で abort する。描画前に
        // combine→snap を通せば、run は非重複・昇順・文字境界に乗り、合計 ≤ 本文長に収まること。
        let text = "太字コード";
        let style = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        // `***太字コード***` 相当: Strong と Emphasis が全域で重なる（合計 2×本文長）。
        let overlapping = vec![(0..text.len(), style), (0..text.len(), style)];
        let combined =
            gpui::combine_highlights(overlapping, std::iter::empty()).collect::<Vec<_>>();
        let runs = snap_highlights_to_char_boundaries(text, combined);

        let mut covered = 0usize;
        let mut prev_end = 0usize;
        for (range, _) in &runs {
            assert!(
                text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
                "run {range:?} が文字境界に乗らない",
            );
            assert!(range.start < range.end, "空 run");
            assert!(range.end <= text.len(), "範囲外 run {range:?}");
            assert!(range.start >= prev_end, "run が重なっている {range:?}");
            prev_end = range.end;
            covered += range.len();
        }
        assert!(
            covered <= text.len(),
            "run 合計 {covered} が本文長 {} を超過（layout_line が abort する）",
            text.len(),
        );
    }

    #[test]
    fn word_range_in_selects_word_cjk_and_underscore() {
        // 英単語: 途中でも語直後の境界でも同じ語（editor_core::word_range_at と同じ規律）。
        assert_eq!(word_range_in("hello world", 2), (0, 5));
        assert_eq!(word_range_in("hello world", 5), (0, 5)); // 直後の境界は手前の語
        assert_eq!(word_range_in("hello world", 8), (6, 11));
        // アンダースコアは語に含む（識別子を 1 語で選ぶ）。
        assert_eq!(word_range_in("foo_bar", 3), (0, 7));
        // CJK は連続する塊を 1 語として選ぶ。
        assert_eq!(word_range_in("認証API", 0), (0, "認証API".len()));
        // 記号/空白の上では選択なし（空範囲）。
        assert_eq!(word_range_in("a - b", 2), (2, 2));
        // offset は末尾へクランプ（範囲外でも panic しない）。
        assert_eq!(word_range_in("hi", 99), (0, 2));
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
    fn flatten_digest_line_folds_multiline_tool_titles() {
        // 複数行のシェルコマンドは先頭行 + … に畳む（稼働中ラインが縦に伸びない保証）。
        assert_eq!(
            flatten_digest_line("cargo build \\\n  --release \\\n  -p necoder").as_ref(),
            "cargo build \\ …"
        );
        // 1 行でも 100 字を超えたら … 付きで切り詰める（生成中行は truncate が無く折り返すため）。
        let long = "x".repeat(200);
        let flat = flatten_digest_line(&long);
        assert_eq!(flat.chars().count(), 100 + " …".chars().count());
        // 短い 1 行はそのまま。前後の空白は落とす。
        assert_eq!(flatten_digest_line("  Bash(ls)  ").as_ref(), "Bash(ls)");
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

    #[test]
    fn tool_location_infers_language_with_line_and_quotes() {
        assert_eq!(
            infer_language_from_tool_argument("src/main.rs:42:7"),
            Some(lang::LanguageId::Rust)
        );
        assert_eq!(
            infer_language_from_tool_argument("Read (config/app.yaml)"),
            Some(lang::LanguageId::Yaml)
        );
        assert_eq!(infer_language_from_tool_argument("cargo test"), None);
    }

    #[test]
    fn acp_syntax_cache_reuses_and_bounds_entries() {
        let mut cache = SyntaxHighlightCache::default();
        let source = "fn main() { let value = 1; }";
        let first = cache.highlight(lang::LanguageId::Rust, source);
        let second = cache.highlight(lang::LanguageId::Rust, source);
        assert!(!first.is_empty());
        assert_eq!(first, second);
        assert_eq!(cache.highlighters.len(), 1);
        assert_eq!(cache.spans.len(), 1);

        let markdown = cache.highlight(
            lang::LanguageId::Markdown,
            "# Title\n\nA **strong** value.\n",
        );
        assert!(markdown
            .iter()
            .any(|span| span.kind == lang::HighlightKind::Heading));

        for index in 0..(SyntaxHighlightCache::CAPACITY + 8) {
            cache.highlight(
                lang::LanguageId::Rust,
                &format!("fn generated_{index}() {{}}"),
            );
        }
        assert!(cache.spans.len() <= SyntaxHighlightCache::CAPACITY);
    }

    #[test]
    fn markdown_block_cache_reuses_and_bounds_entries() {
        let mut cache = MarkdownBlockCache::default();
        let source = "# Result\n\nA **cached** paragraph.\n";
        let first = cache.parse(source);
        let second = cache.parse(source);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.blocks.len(), 1);

        for index in 0..(MarkdownBlockCache::CAPACITY + 8) {
            cache.parse(&format!("## Generated {index}\n"));
        }
        assert!(cache.blocks.len() <= MarkdownBlockCache::CAPACITY);
    }

    #[test]
    fn previous_user_entry_follows_prompt_boundaries() {
        let entries = vec![
            Entry::User("first".into()),
            Entry::Agent("answer".into()),
            Entry::User("second".into()),
            Entry::Agent("long answer".into()),
        ];

        assert_eq!(previous_user_entry_index(&entries, 4), Some(2));
        assert_eq!(previous_user_entry_index(&entries, 2), Some(0));
        assert_eq!(previous_user_entry_index(&entries, 0), None);
        assert_eq!(previous_user_entry_index(&entries, usize::MAX), Some(2));
    }

    #[gpui::test]
    fn transcript_focus_dispatches_copy_to_panel(cx: &mut gpui::TestAppContext) {
        let settings_path = std::env::temp_dir().join(format!(
            "necoder_agent_copy_{}_{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::write(&settings_path, r#"{"onboarded":true}"#).unwrap();
        cx.update(|cx| settings::init(Some(settings_path.clone()), None, cx));

        // 通常起動の transcript は空（デモ種の廃止・2026-08-17）なので、コピー対象の本文は
        // テストが自前で積む（初回描画前に入れれば transcript_regions に載る）。
        let (panel, cx) = cx.add_window_view(|_window, cx| {
            let mut panel = AgentPanel::new(Theme::dark(), cx);
            if let Some(thread) = panel.threads.get_mut(panel.active) {
                thread.entries.push(Entry::Agent("コピー検証の本文".into()));
            }
            panel
        });
        let expected = panel.update(cx, |panel, _cx| {
            let regions = panel.transcript_regions.borrow();
            let first = regions.first().expect("transcript に選択可能な本文がある");
            let text = first.text.to_string();
            panel.transcript_selection = Some(TranscriptSelection {
                start: TranscriptPoint {
                    region: 0,
                    offset: 0,
                },
                end: TranscriptPoint {
                    region: 0,
                    offset: first.text.len(),
                },
                selecting: false,
            });
            text
        });
        let focus = panel.read_with(cx, |panel, _cx| panel.transcript_focus.clone());
        cx.update(|window, cx| {
            focus.focus(window, cx);
            focus.dispatch_action(&editor_view::Copy, window, cx);
        });

        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some(expected)
        );
        let _ = std::fs::remove_file(settings_path);
    }

    /// 回帰: **生成中**の末尾本文も過去本文と同じ選択可能パス（push_selectable）に載り、
    /// ドラッグ選択 → ⌘C でコピーできる（旧: 生成中は別ビュー StreamingTextView で選択不可・
    /// 2026-08-26 一本化）。長文（>256 文字）にして reveal を即全開示させ、タイプライタ時計の
    /// 非決定性を避ける（新ストリームは 256 文字超なら打ち直さず全表示・reduce/非アクティブでも全表示）。
    #[gpui::test]
    fn live_streaming_text_is_selectable(cx: &mut gpui::TestAppContext) {
        let settings_path = init_test_settings(cx, "live_select");
        let live_text = "あ".repeat(300); // 単一段落・300 文字（>256 で即全開示）
        let (panel, cx) = cx.add_window_view(|_window, cx| {
            let mut panel = AgentPanel::new(Theme::dark(), cx);
            if let Some(thread) = panel.threads.get_mut(panel.active) {
                thread.running = true; // 生成中を模擬
                thread.entries.push(Entry::Agent(live_text.clone().into()));
            }
            panel
        });
        let copied = panel.update(cx, |panel, _cx| {
            assert!(panel.live_reveal >= 300, "長い新ストリームは即全開示される");
            let regions = panel.transcript_regions.borrow();
            let live = regions
                .last()
                .expect("生成中の本文が選択可能リージョンに載る");
            assert_eq!(
                live.text.as_ref(),
                live_text,
                "生成中の本文が丸ごと 1 リージョンとして選択可能"
            );
            let end = live.text.len();
            let last_region = regions.len() - 1;
            drop(regions);
            panel.transcript_selection = Some(TranscriptSelection {
                start: TranscriptPoint {
                    region: 0,
                    offset: 0,
                },
                end: TranscriptPoint {
                    region: last_region,
                    offset: end,
                },
                selecting: false,
            });
            panel.transcript_selected_text()
        });
        assert!(
            copied.is_some_and(|text| text.contains(&live_text)),
            "生成中の本文が選択テキストに含まれる（= ⌘C でコピーできる）"
        );
        let _ = std::fs::remove_file(settings_path);
    }

    /// 生成中（running）の submit はキューへ積み、即送信しない（User entry を増やさない）。
    /// 取消でキューが空になる。ターン完了時の自動フラッシュは session を要するのでここでは検証しない。
    #[gpui::test]
    fn submit_queues_prompt_while_running(cx: &mut gpui::TestAppContext) {
        let settings_path = init_test_settings(cx, "queue");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.threads[active].running = true; // 生成中を模擬
            let entries_before = panel.threads[active].entries.len();
            panel
                .composer
                .update(cx, |composer, cx| composer.set_plain_text("次の指示", cx));
            panel.submit(cx);
            let thread = &panel.threads[active];
            assert_eq!(thread.queued_prompts, vec!["次の指示".to_string()]);
            assert_eq!(thread.entries.len(), entries_before, "即送信していない");
            assert!(thread.running, "running のまま");
            panel.remove_queued_prompt(0, cx);
            assert!(panel.threads[active].queued_prompts.is_empty());
        });
        let _ = std::fs::remove_file(settings_path);
    }

    /// bypass permissions 選択中に届いた許可リクエストは UI 側で即 Allow を返す
    /// （エージェント側の set_mode 反映レースの窓を塞ぐ）。Blocked には積まない。
    #[gpui::test]
    fn permission_request_auto_allows_in_bypass_mode(cx: &mut gpui::TestAppContext) {
        let settings_path = init_test_settings(cx, "bypass_auto");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        let (respond_tx, mut respond_rx) = mpsc::unbounded::<usize>();
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.threads[active].permission_mode = "Bypass Permissions".into();
            panel.on_event(
                active,
                AgentEvent::PermissionRequest {
                    title: "Bash: cargo test".into(),
                    diffs: Vec::new(), // diff 無し＝スナップショット不要の同期応答経路
                    raw_input: None,
                    options: vec![
                        PermissionChoice {
                            label: "拒否".into(),
                            kind: PermissionKind::Reject,
                        },
                        PermissionChoice {
                            label: "許可".into(),
                            kind: PermissionKind::Allow,
                        },
                    ],
                    respond: respond_tx,
                },
                cx,
            );
            assert!(
                panel.threads[active].pending_permission.is_none(),
                "bypass 中は Blocked に積まない"
            );
        });
        assert_eq!(
            respond_rx.try_recv().ok(),
            Some(1),
            "Allow の添字で即応答する"
        );
        let _ = std::fs::remove_file(settings_path);
    }

    /// 承認待ちの最中に bypass へ切り替えたら、その場で Allow が返って待ちが解ける
    /// （set_mode はターン中 deferred なので、これが無いと bypass にしても止まったまま）。
    #[gpui::test]
    fn switching_to_bypass_flushes_pending_permission(cx: &mut gpui::TestAppContext) {
        let settings_path = init_test_settings(cx, "bypass_flush");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        let (respond_tx, mut respond_rx) = mpsc::unbounded::<usize>();
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.threads[active].pending_permission = Some(PendingPermission {
                title: "Edit: main.rs".into(),
                diffs: Vec::new(),
                raw_input: None,
                options: vec![
                    PermissionChoice {
                        label: "拒否".into(),
                        kind: PermissionKind::Reject,
                    },
                    PermissionChoice {
                        label: "許可".into(),
                        kind: PermissionKind::Allow,
                    },
                ],
                respond: respond_tx,
                since: std::time::Instant::now(),
            });
            panel.select_option(Selector::Mode, "bypass permissions".into(), cx);
            assert!(
                panel.threads[active].pending_permission.is_none(),
                "切り替えで待ちが解ける"
            );
        });
        assert_eq!(
            respond_rx.try_recv().ok(),
            Some(1),
            "Allow の添字で応答する"
        );
        let _ = std::fs::remove_file(settings_path);
    }

    fn init_test_settings(cx: &mut gpui::TestAppContext, label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "necoder_agent_{label}_{}_{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::write(&path, r#"{"onboarded":true}"#).unwrap();
        cx.update(|cx| settings::init(Some(path.clone()), None, cx));
        path
    }

    fn config(category: ConfigCategory, current: &str, choices: &[(&str, &str)]) -> ConfigOption {
        ConfigOption {
            config_id: format!("{category:?}"),
            category,
            current: current.to_string(),
            choices: choices
                .iter()
                .map(|(id, name)| (id.to_string(), name.to_string()))
                .collect(),
        }
    }

    /// 新セッションの Configs 広告（＝エージェント既定）が、スレッドが選んでいる sticky を上書きしない。
    /// 広告に在る限り選択を保ち（＝タブを開くたびに戻らない）、無い時だけ広告 current にフォールバックする。
    #[gpui::test]
    fn configs_advertisement_keeps_thread_selection(cx: &mut gpui::TestAppContext) {
        let path = init_test_settings(cx, "configs");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            let thread = &mut panel.threads[active];
            thread.model = "Opus".into();
            thread.effort = "xhigh".into();
            // エージェントは自分の既定（Sonnet / high）を current として広告してくる。
            let configs = vec![
                config(
                    ConfigCategory::Model,
                    "sonnet-id",
                    &[("opus-id", "Opus"), ("sonnet-id", "Sonnet")],
                ),
                config(
                    ConfigCategory::ThoughtLevel,
                    "high-id",
                    &[("high-id", "high"), ("xhigh-id", "xhigh")],
                ),
            ];
            panel.on_event(active, AgentEvent::Configs(configs), cx);
            let thread = &panel.threads[active];
            assert_eq!(thread.model.as_ref(), "Opus", "選択したモデルを保つ");
            assert_eq!(thread.effort.as_ref(), "xhigh", "選択した思考量を保つ");
            assert_eq!(thread.configs.len(), 2, "広告は取り込む");

            // フォールバック: 広告に無い選択は広告 current の表示名を採用する。
            panel.threads[active].model = "Haiku".into();
            let configs = vec![config(
                ConfigCategory::Model,
                "sonnet-id",
                &[("opus-id", "Opus"), ("sonnet-id", "Sonnet")],
            )];
            panel.on_event(active, AgentEvent::Configs(configs), cx);
            assert_eq!(
                panel.threads[active].model.as_ref(),
                "Sonnet",
                "提供されない選択は広告 current へフォールバック"
            );
        });
        let _ = std::fs::remove_file(path);
    }

    /// Agent は未送信スレッドでだけ選べる。会話開始後に別 AI へ差し替えて transcript を混ぜない。
    #[gpui::test]
    fn agent_is_locked_after_conversation_starts(cx: &mut gpui::TestAppContext) {
        let path = init_test_settings(cx, "agent-lock");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            panel.select_option(Selector::Agent, "Codex".into(), cx);
            assert_eq!(panel.threads[panel.active].agent.as_ref(), "Codex");

            panel.threads[panel.active]
                .entries
                .push(Entry::User("開始済み".into()));
            panel.select_option(Selector::Agent, "Claude Code".into(), cx);
            assert_eq!(
                panel.threads[panel.active].agent.as_ref(),
                "Codex",
                "開始済みスレッドの Agent は変わらない"
            );
            panel.toggle_menu(Selector::Agent, cx);
            assert!(panel.open_menu.is_none(), "開始後は Agent menu も開かない");
        });
        let _ = std::fs::remove_file(path);
    }

    /// 未接続スレッドでも、Model メニューは選択中 Agent の vendor に合う候補だけを出す。
    #[gpui::test]
    fn model_fallback_follows_selected_agent(cx: &mut gpui::TestAppContext) {
        let path = init_test_settings(cx, "agent-model-fallback");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            panel.select_option(Selector::Agent, "Codex".into(), cx);

            let thread = &panel.threads[panel.active];
            assert_eq!(thread.model.as_ref(), CODEX_MODELS[0]);
            let options = panel.selector_options(Selector::Model);
            assert_eq!(options.len(), CODEX_MODELS.len());
            assert!(options.iter().all(|model| !model.starts_with("claude-")));
            assert!(options.iter().any(|model| model.as_ref() == "GPT-5.6-Sol"));
        });
        let _ = std::fs::remove_file(path);
    }

    /// Modes 広告も同じ規律。加えて新規タブは直前タブの権限モードを引き継ぐ（default に戻らない）。
    #[gpui::test]
    fn mode_persists_and_new_tab_inherits(cx: &mut gpui::TestAppContext) {
        let path = init_test_settings(cx, "modes");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.threads[active].permission_mode = "Accept Edits".into();
            let modes = vec![
                ("ask".to_string(), "Always Ask".to_string()),
                ("accept".to_string(), "Accept Edits".to_string()),
            ];
            panel.on_event(
                active,
                AgentEvent::Modes {
                    modes,
                    current: "ask".to_string(),
                },
                cx,
            );
            let thread = &panel.threads[active];
            assert_eq!(
                thread.permission_mode.as_ref(),
                "Accept Edits",
                "選択したモードを保つ（広告 current では上書きしない）"
            );
            assert_eq!(thread.current_mode_id.as_ref(), "accept");

            // 新規タブは直前タブのモードを引き継ぐ。
            panel.add_thread(cx);
            assert_eq!(
                panel.threads[panel.active].permission_mode.as_ref(),
                "Accept Edits",
                "新規タブは前タブの権限モードを引き継ぐ"
            );

            // フォールバック: 提供されないモードは広告 current の表示名を採用する。
            let active = panel.active;
            panel.threads[active].permission_mode = "Plan".into();
            let modes = vec![
                ("ask".to_string(), "Always Ask".to_string()),
                ("accept".to_string(), "Accept Edits".to_string()),
            ];
            panel.on_event(
                active,
                AgentEvent::Modes {
                    modes,
                    current: "ask".to_string(),
                },
                cx,
            );
            assert_eq!(
                panel.threads[active].permission_mode.as_ref(),
                "Always Ask",
                "提供されないモードは広告 current へフォールバック"
            );
        });
        let _ = std::fs::remove_file(path);
    }

    /// Model/Effort/Mode は **agent ごとに** sticky（DEFAULT §8 の default_agent とは別レイヤ）。
    /// ある agent で選んだ値は別 agent へ漏れず、agent を切り替えると各自の最後の選択で開き直す。
    #[gpui::test]
    fn sticky_defaults_are_per_agent(cx: &mut gpui::TestAppContext) {
        let path = init_test_settings(cx, "per-agent-sticky");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            // 既定 agent（Claude Code）でモデル・思考量を選ぶ → その agent にだけ紐づいて永続化。
            panel.select_option(Selector::Model, "claude-opus-5".into(), cx);
            panel.select_option(Selector::Effort, "xhigh".into(), cx);
            let defaults = settings::get(cx).agent_defaults;
            assert_eq!(
                defaults["Claude Code"].model.as_deref(),
                Some("claude-opus-5")
            );
            assert_eq!(defaults["Claude Code"].effort.as_deref(), Some("xhigh"));
            assert!(!defaults.contains_key("Codex"), "他 agent には書かない");

            // Codex へ切替 → Claude の sticky を持ち込まず vendor フォールバックで開く。
            panel.select_option(Selector::Agent, "Codex".into(), cx);
            assert_eq!(panel.threads[panel.active].agent.as_ref(), "Codex");
            assert_eq!(panel.threads[panel.active].model.as_ref(), CODEX_MODELS[0]);

            // Codex で別モデルを選ぶ → Codex にだけ紐づき、Claude 側は不変。
            panel.select_option(Selector::Model, CODEX_MODELS[1].into(), cx);
            let defaults = settings::get(cx).agent_defaults;
            assert_eq!(defaults["Codex"].model.as_deref(), Some(CODEX_MODELS[1]));
            assert_eq!(
                defaults["Claude Code"].model.as_deref(),
                Some("claude-opus-5"),
                "Codex の選択は Claude の sticky を汚さない"
            );

            // Claude へ戻す → Claude の sticky（opus / xhigh）で開き直す。
            panel.select_option(Selector::Agent, "Claude Code".into(), cx);
            assert_eq!(panel.threads[panel.active].model.as_ref(), "claude-opus-5");
            assert_eq!(panel.threads[panel.active].effort.as_ref(), "xhigh");

            // §8: グローバル既定 default_agent はピル操作で動かない。
            assert_eq!(settings::get(cx).default_agent, "Claude Code");
        });
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn mascot_atlases_decode_from_embedded_bytes() {
        // 回帰テスト: マスコットは埋め込みバイトからデコードできる（実行時にビルド機の
        // パスを読まない）こと。フレーム分割もアトラス定義どおりであること。
        let atlases = MascotAtlases::load();
        for (name, image, frame_count) in [
            ("idle", &atlases.idle, 1),
            ("doze", &atlases.doze, 15),
            ("typing", &atlases.typing, 15),
            ("think", &atlases.think, 15),
            ("celebrate", &atlases.celebrate, 15),
            ("plead", &atlases.plead, 15),
            ("worry", &atlases.worry, 15),
        ] {
            assert_eq!(image.frame_count(), frame_count, "{name} のフレーム数");
            let size = image.size(0);
            assert!(size.width.0 > 0 && size.height.0 > 0, "{name} が空画像");
        }
    }
}
