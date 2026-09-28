//! auto_prompt — **necoder の知らせ**（人ではなく necoder が書いてスレッドへ送った発話・FLEET-V2 §3.6）。
//!
//! エージェントには人の発話と同じ user ターンとして届くが、transcript では人の発話と分けて灰色のカード
//! （`イベント · 14:05（台帳）`）で描く。AgentPanel は Fleet を知らないので、何の知らせか（出所の名前）は
//! 呼び手が渡す（Captain の台帳の未読なら workspace が「台帳」を渡す・`SeatPolicy` と同じ流儀）。
//!
//! 見分けは再起動と再生（`session/load`）を越えて残す:
//! - エージェントへ送る本文は `<necoder-event source="…" time="…">` … `</necoder-event>` で囲む。
//!   モデルには「necoder からの知らせ」と読めるだけで害は無い。DB（role `auto_prompt`）にも同じ形で残す
//! - 再生された user ターンにこの印があれば、人の発話ではなく知らせのカードに戻す
//! - スレッドの前置き（[`AgentPanel::set_prompt_context`]・Captain の役割と現況）は人の発話にも知らせにも
//!   毎回付くので、カードにはしない（transcript を倍にするだけ）。送る本文では `<necoder-context>` …
//!   `</necoder-context>` で囲み、再生ではそこを外す（人の発話に necoder の前置きを混ぜない）

use super::*;

/// DB の `turns.role`。人の発話（`user`）とは分ける＝全文検索と「最終入力」の対象にならない。
pub(crate) const AUTO_PROMPT_ROLE: &str = "auto_prompt";
const EVENT_OPEN: &str = "<necoder-event";
const EVENT_CLOSE: &str = "</necoder-event>";
const CONTEXT_OPEN: &str = "<necoder-context>";
const CONTEXT_CLOSE: &str = "</necoder-context>";

/// necoder の知らせ 1 通（transcript の [`Entry::AutoPrompt`]）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AutoPrompt {
    /// 出所の名前（カードの括弧の中・呼び手が決める）。空なら括弧を出さない。
    pub(crate) source: SharedString,
    /// 送った時刻（ローカルの `YYYY-MM-DD HH:MM`）。カードには時刻だけを出す。空 = 分からない。
    pub(crate) sent_at: SharedString,
    /// 本文（前置き・添付・印を除いた、呼び手が渡したそのもの）。
    pub(crate) text: SharedString,
}

impl AutoPrompt {
    /// いま送る知らせ。
    pub(crate) fn now(source: SharedString, text: String) -> AutoPrompt {
        let (date, hour, minute) = chat_core::date::local_now();
        AutoPrompt {
            source,
            sent_at: SharedString::from(format!("{date} {hour:02}:{minute:02}")),
            text: SharedString::from(text),
        }
    }

    /// エージェントへ送る本文（印つき）。DB にもこの形で残す（復元も再生も [`Self::parse`] 1 つで戻す）。
    pub(crate) fn wire(&self) -> String {
        format!(
            "{EVENT_OPEN} source=\"{}\" time=\"{}\">\n{}\n{EVENT_CLOSE}",
            attribute_value(&self.source),
            attribute_value(&self.sent_at),
            self.text.trim_end()
        )
    }

    /// 印つきの本文から戻す（DB の復元と、再生された user ターン）。印の前の前置き・添付と、閉じの印の
    /// 後ろ（再生でエージェントが足した物）は捨てる。開きの印が行頭に無い・閉じの印が無い本文は
    /// 知らせではない（`None`）。
    pub(crate) fn parse(text: &str) -> Option<AutoPrompt> {
        let text = strip_prompt_context(text).unwrap_or_else(|| text.to_string());
        let start = line_start(&text, EVENT_OPEN)?;
        let rest = &text[start + EVENT_OPEN.len()..];
        let header_end = rest.find('>')?;
        let body = &rest[header_end + 1..];
        let body = &body[..body.rfind(EVENT_CLOSE)?];
        Some(AutoPrompt {
            source: SharedString::from(attribute(&rest[..header_end], "source")),
            sent_at: SharedString::from(attribute(&rest[..header_end], "time")),
            text: SharedString::from(
                body.strip_prefix('\n')
                    .unwrap_or(body)
                    .trim_end()
                    .to_string(),
            ),
        })
    }

    /// カードの見出し（`イベント · 14:05（台帳）`）。時刻・出所が分からなければその分を省く。
    pub(crate) fn label(&self) -> String {
        let mut label = i18n::t!("agent.auto_prompt");
        let clock = self.sent_at.rsplit(' ').next().unwrap_or_default();
        if !clock.is_empty() {
            label.push_str(" · ");
            label.push_str(clock);
        }
        if self.source.is_empty() {
            label
        } else {
            i18n::t!(
                "agent.auto_prompt_source",
                "label" => label,
                "source" => self.source.as_ref()
            )
        }
    }
}

/// 印の属性に入れられない文字（引用符・山括弧・改行）を落とす。出所は呼び手の語なので本来入らない。
fn attribute_value(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(character, '"' | '<' | '>' | '\n' | '\r'))
        .collect()
}

/// 開きの印の中の `name="…"` の値（無ければ空）。
fn attribute(header: &str, name: &str) -> String {
    let key = format!("{name}=\"");
    header
        .find(&key)
        .and_then(|start| {
            let value = &header[start + key.len()..];
            value.find('"').map(|end| value[..end].to_string())
        })
        .unwrap_or_default()
}

/// `needle` が行頭に現れる最初の位置。
fn line_start(text: &str, needle: &str) -> Option<usize> {
    text.match_indices(needle)
        .map(|(position, _)| position)
        .find(|position| *position == 0 || text[..*position].ends_with('\n'))
}

/// スレッドの前置きを、送る本文の中で見分けられる形にする（再生で外すため）。
pub(crate) fn wrap_prompt_context(context: &str) -> String {
    format!("{CONTEXT_OPEN}\n{}\n{CONTEXT_CLOSE}\n", context.trim_end())
}

/// 再生された user ターンから前置き（`<necoder-context>` の塊）を外す。前置きが無ければ `None`。
fn strip_prompt_context(text: &str) -> Option<String> {
    let start = line_start(text, CONTEXT_OPEN)?;
    let end = start + text[start..].find(CONTEXT_CLOSE)? + CONTEXT_CLOSE.len();
    Some(format!("{}{}", &text[..start], &text[end..]))
}

/// 再生された user ターンを transcript のエントリへ（O15 の再生・FLEET-V2 §3.6）。知らせの印があれば
/// カードに、無ければ人の発話にする（前置きは外す）。前置きしか無い（中身が空の）ターンは出さない。
pub(crate) fn replayed_user_entry(text: String) -> Option<Entry> {
    if let Some(auto) = AutoPrompt::parse(&text) {
        return Some(Entry::AutoPrompt(auto));
    }
    match strip_prompt_context(&text) {
        Some(rest) => {
            let rest = rest.trim();
            (!rest.is_empty()).then(|| Entry::User(SharedString::from(rest.to_string())))
        }
        None => Some(Entry::User(SharedString::from(text))),
    }
}

/// DB の 1 行（role `auto_prompt`）から戻す。印が読めなければ本文だけのカードにする（人の発話にはしない）。
pub(crate) fn stored_auto_prompt(content: String) -> Entry {
    Entry::AutoPrompt(AutoPrompt::parse(&content).unwrap_or_else(|| AutoPrompt {
        source: SharedString::default(),
        sent_at: SharedString::default(),
        text: SharedString::from(content),
    }))
}

impl AgentPanel {
    /// 表示中のタブを切り替えずに、necoder の知らせを送る（Captain を起こす台帳の未読など）。
    /// `source` はカードの括弧に出す出所の名前（呼び手が決める・AgentPanel は何の知らせかを知らない）。
    /// エージェントには user ターンとして届く。「頼んだこと」と「最終入力」は人の発話だけで決めるので動かさない。
    pub fn send_auto_prompt_to(
        &mut self,
        thread_index: usize,
        source: &str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.send_prompt_entry(
            thread_index,
            text,
            Some(SharedString::from(source.to_string())),
            cx,
        );
    }

    /// 知らせのカード（mock `fleet-v2.html` の `.tr .u` の灰色の行）。地は bg1・左縁は fg2（識別色を
    /// 使わない＝UI-SPEC §1.3）・見出し 10px fg2・本文は人の発話より一段控えめ（fg1・標準の太さ）。
    /// 長い本文は人の発話と同じ閾値で畳む。
    pub(crate) fn render_auto_prompt_entry(
        &self,
        index: usize,
        auto: &AutoPrompt,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let foldable = user_entry_foldable(&auto.text);
        let expanded = !foldable || self.is_input_expanded(index);
        let shown = if expanded {
            auto.text.clone()
        } else {
            user_entry_preview(&auto.text)
        };
        let mut column = div()
            .flex_1()
            .min_w_0()
            .px(px(11.))
            .py(px(7.))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(theme.fg2)
                    .child(SharedString::from(auto.label())),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(12.5))
                    .text_color(theme.fg1)
                    .child(self.selectable_text(shown, cx)),
            );
        if foldable && !expanded {
            column =
                column.child(self.render_input_fold_chip(index, auto.text.lines().count(), cx));
        }
        div()
            .flex()
            .items_stretch()
            .rounded(px(7.))
            .overflow_hidden()
            .bg(theme.bg1)
            .border_1()
            .border_color(theme.border)
            .child(div().w(px(2.)).flex_none().bg(theme.fg2))
            .child(column)
            .into_any_element()
    }

    /// 開発用: スレッドに会話の見本（人の発話・知らせのカード・本文）とトークンを仕込む
    /// （`NECODER_FLEET_PROBE=…;captain-transcript` の offscreen 撮影用・エージェントには送らない）。
    #[cfg(debug_assertions)]
    pub fn debug_seed_auto_prompts(
        &mut self,
        thread_index: usize,
        source: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(thread) = self.threads.get_mut(thread_index) else {
            return;
        };
        let sent = |clock: &str, text: &str| {
            Entry::AutoPrompt(AutoPrompt {
                source: SharedString::from(source.to_string()),
                sent_at: SharedString::from(format!("2026-09-28 {clock}")),
                text: SharedString::from(text.to_string()),
            })
        };
        thread.entries.extend([
            Entry::User(
                "M2 の編集コアを仕上げたい。buffer を rope にして undo を Transaction 単位に、\
                 gpui の起動経路を通す。並走でいい。"
                    .into(),
            ),
            Entry::Agent(
                "2 本に分けて提案しました（rope と undo は同じ buffer.rs を触るので 1 本に束ねる）。\
                 承認を待ちます。"
                    .into(),
            ),
            sent(
                "14:05",
                "台帳の新しい出来事（前回の続き。#通し番号 Task-id 題名: 何が起きたか）:\n\
                 #41 space-gpui gpui起動: → review_ready — window/app/main の起動経路を通した · 5 files",
            ),
            Entry::Step {
                id: None,
                tool: "fleet_review_task".into(),
                args: "gpui起動".into(),
                result: Some("radar ✓ 衝突なし → merge_ready".into()),
                result_lines: 1,
                diffs: Vec::new(),
                parent: None,
            },
            Entry::Agent(
                "gpui起動 は確認だけで統合できます。Integrate はあなたの判断です（要対応に出しました）。".into(),
            ),
            sent(
                "14:12",
                "台帳の新しい出来事（前回の続き。#通し番号 Task-id 題名: 何が起きたか）:\n\
                 #44 space-rope rope設計: 人間が直接指示した「テストも足して」",
            ),
        ]);
        thread.tokens_used = 4_200;
        thread.tokens_shown = 4_200.0;
        thread.tokens_max = 200_000;
        if thread_index == self.active {
            self.reset_transcript_list(true);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AutoPrompt {
        AutoPrompt {
            source: "台帳".into(),
            sent_at: "2026-09-28 14:05".into(),
            text: "台帳の新しい出来事:\n#12 space-a rope: → review_ready".into(),
        }
    }

    /// 送る本文の印から、同じ知らせ（出所・時刻・本文）に戻せる。前置き・添付が前に付いていても戻る。
    #[test]
    fn a_marked_body_round_trips_even_behind_the_preamble() {
        let auto = sample();
        let wire = auto.wire();
        assert!(wire.starts_with("<necoder-event source=\"台帳\" time=\"2026-09-28 14:05\">\n"));
        assert!(wire.ends_with("\n</necoder-event>"));
        assert_eq!(AutoPrompt::parse(&wire), Some(auto.clone()));

        let full = format!(
            "@src/lib.rs\n{}\n{wire}",
            wrap_prompt_context("あなたは Captain です。\n現況:\nspace-a | rope | working |")
        );
        assert_eq!(AutoPrompt::parse(&full), Some(auto.clone()));
        // 再生でエージェントが閉じの印の後ろに足した物は捨てる（知らせのまま）。
        assert_eq!(
            AutoPrompt::parse(&format!(
                "{wire}\n<system-reminder>足された</system-reminder>"
            )),
            Some(auto)
        );
    }

    /// 印の無い本文・途中で途切れた本文・文中に印の語があるだけの本文は、知らせにしない（人の発話のまま）。
    #[test]
    fn text_without_the_whole_mark_is_not_an_auto_prompt() {
        assert_eq!(AutoPrompt::parse("README を直して"), None);
        assert_eq!(
            AutoPrompt::parse("<necoder-event source=\"台帳\" time=\"\">\n途中まで"),
            None,
            "閉じの印が無い（途中で途切れた）"
        );
        assert_eq!(
            AutoPrompt::parse("説明: <necoder-event> と </necoder-event>"),
            None,
            "開きの印が行頭に無い"
        );
    }

    /// 出所に印を壊す文字が入っても、読み戻しで壊れない（落として送る）。
    #[test]
    fn attribute_values_cannot_break_the_mark() {
        let auto = AutoPrompt {
            source: "台\"帳>\n".into(),
            ..sample()
        };
        let parsed = AutoPrompt::parse(&auto.wire()).expect("読み戻せる");
        assert_eq!(parsed.source.as_ref(), "台帳");
        assert_eq!(parsed.text, auto.text);
    }

    /// 見出しは `イベント · 時刻（出所）`。時刻・出所が分からなければその分を省く。
    #[test]
    fn the_card_label_names_the_time_and_the_source() {
        let label = sample().label();
        assert!(label.contains("14:05"), "{label}");
        assert!(label.contains("台帳"), "{label}");
        assert!(!label.contains("2026"), "日付は出さない: {label}");
        let bare = AutoPrompt {
            source: SharedString::default(),
            sent_at: SharedString::default(),
            ..sample()
        };
        assert_eq!(bare.label(), i18n::t!("agent.auto_prompt"));
    }

    /// 再生された user ターン: 知らせの印があればカード、前置き付きの人の発話は前置きを外して人の発話、
    /// 前置きしか無いターンは出さない、前置きの無い発話は今までどおりそのまま。
    #[test]
    fn replayed_user_turns_keep_the_kind_and_drop_the_preamble() {
        let context = wrap_prompt_context("あなたは Captain です。");
        let replayed_event = format!("{context}\n{}", sample().wire());
        assert!(matches!(
            replayed_user_entry(replayed_event),
            Some(Entry::AutoPrompt(auto)) if auto == sample()
        ));
        assert!(matches!(
            replayed_user_entry(format!("{context}\n目標: rope に置き換える")),
            Some(Entry::User(text)) if text.as_ref() == "目標: rope に置き換える"
        ));
        assert!(replayed_user_entry(context).is_none());
        assert!(matches!(
            replayed_user_entry(" そのまま ".to_string()),
            Some(Entry::User(text)) if text.as_ref() == " そのまま "
        ));
    }

    /// 送る・保存・復元・再生の一巡（FLEET-V2 §3.6）: 知らせは人の発話と別の種別で積まれ、
    /// 「頼んだこと」「最終入力」を動かさず、前置きの後ろに印つきで届く。DB から戻しても、同じ本文を
    /// エージェントが再生しても、知らせは知らせ・人の発話は人の発話のまま（前置きは外れる）。
    #[gpui::test]
    fn an_auto_prompt_stays_apart_from_human_turns_across_restore_and_replay(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!(
            "necoder_agent_auto_prompt_{}_{}",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::create_dir_all(&root).expect("作業ディレクトリを作れる");
        let settings_path = root.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"onboarded":true,"agent_prewarm":false}"#,
        )
        .expect("設定を書ける");
        cx.update(|cx| settings::init(Some(settings_path), None, cx));
        let storage = storage::Storage::open(&root.join("necoder.db")).expect("DB を開ける");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            panel.set_storage(storage.clone(), cx);
            let active = panel.active;
            let (command_tx, mut command_rx) = mpsc::unbounded::<SessionCommand>();
            panel.threads[active].command_tx = Some(command_tx);
            panel.dest_cwd = Some(root.clone());
            panel.set_prompt_context(active, "ROLE\n".to_string());
            let mut sent = |panel: &mut AgentPanel| {
                panel.threads[active].running = false;
                match command_rx.try_recv() {
                    Ok(SessionCommand::Prompt(text)) => text,
                    other => panic!("prompt が送られていない: {other:?}"),
                }
            };

            panel.send_auto_prompt_to(
                active,
                "台帳",
                "#12 space-a rope: → review_ready".into(),
                cx,
            );
            let wire = sent(panel);
            let auto = match panel.threads[active].entries.last() {
                Some(Entry::AutoPrompt(auto)) => auto.clone(),
                _ => panic!("知らせは人の発話と別の種別で積む"),
            };
            assert_eq!(auto.source.as_ref(), "台帳");
            assert_eq!(auto.text.as_ref(), "#12 space-a rope: → review_ready");
            assert_eq!(
                wire,
                format!(
                    "<necoder-context>\nROLE\n</necoder-context>\n\n{}",
                    auto.wire()
                ),
                "前置きの後ろに印つきで届く"
            );
            let thread = &panel.threads[active];
            assert!(
                thread.last_prompt.is_none(),
                "知らせは「頼んだこと」ではない"
            );
            assert!(
                thread.last_input_at_ms.is_none(),
                "知らせは人の入力ではない"
            );

            panel.send_prompt_text("テストも足して".into(), cx);
            let human_wire = sent(panel);
            assert_eq!(
                panel.threads[active].last_prompt.as_deref(),
                Some("テストも足して")
            );

            // 保存 → 復元（再起動と同じ `thread_from_storage`）。
            panel.persist_thread(active);
            let id = panel.threads[active].id.clone();
            let roles: Vec<String> = storage
                .load_recent_turns(&id, 10)
                .expect("turns を読める")
                .into_iter()
                .map(|(role, _)| role)
                .collect();
            assert_eq!(
                roles,
                vec![AUTO_PROMPT_ROLE.to_string(), "user".to_string()]
            );
            let restored = thread_from_storage(&storage, &id, "Captain", 0, RESTORED_TURNS, cx);
            assert!(matches!(
                restored.entries.as_slice(),
                [Entry::AutoPrompt(back), Entry::User(human)]
                    if *back == auto && human.as_ref() == "テストも足して"
            ));

            // 同じ本文をエージェントが再生した（`session/load` で開いた会話）。
            let index = panel.threads.len();
            let mut replayed = Thread::empty("再生".to_string(), index);
            replayed.replay_pending = true;
            panel.threads.push(replayed);
            panel.on_event(
                index,
                AgentEvent::HistoryReplayed {
                    items: vec![
                        acp_client::history::ReplayItem::User(wire),
                        acp_client::history::ReplayItem::Agent("gpui起動 は統合できます".into()),
                        acp_client::history::ReplayItem::User(human_wire),
                    ],
                    omitted: 0,
                },
                cx,
            );
            assert!(matches!(
                panel.threads[index].entries.as_slice(),
                [Entry::AutoPrompt(back), Entry::Agent(_), Entry::User(human)]
                    if *back == auto && human.as_ref() == "テストも足して"
            ));
            assert_eq!(
                panel.threads[index].last_prompt.as_deref(),
                Some("テストも足して"),
                "再生でも「頼んだこと」は人の発話から"
            );
        });
        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("一時ディレクトリを消せない: {error}");
        }
    }

    /// DB の行は印つきの本文。読めない行も人の発話にはせず、本文だけのカードにする。
    #[test]
    fn stored_rows_come_back_as_cards() {
        assert!(matches!(
            stored_auto_prompt(sample().wire()),
            Entry::AutoPrompt(auto) if auto == sample()
        ));
        assert!(matches!(
            stored_auto_prompt("印の無い本文".to_string()),
            Entry::AutoPrompt(auto) if auto.text.as_ref() == "印の無い本文" && auto.source.is_empty()
        ));
    }
}
