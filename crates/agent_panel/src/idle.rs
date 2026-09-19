//! idle — **使っていないスレッドのエージェントを止める**（idle メモリ予算の回収弁）。
//!
//! エージェントは 1 本で 350〜700 MB を握る（実測 2026-09-19: `npm exec` のラッパー 48 MB +
//! アダプタの node 65 MB + claude 本体 230〜570 MB）。しかも一度起こすとスレッドを閉じるまで
//! 生き続けるので、開きっぱなしのスレッドが 20 本ある窓では necoder 本体 291 MB に対して
//! エージェントが 7.8 GB を握っていた。
//!
//! 止めても会話は失われない — 次の送信で `session/load` が同じ会話を引き継ぐ（復帰の代償は
//! 初回応答が数秒遅れることだけ）。だから止めてよいのは**会話を引き継げるスレッドだけ**:
//! エージェントが `loadSession` を広告していて、セッション id を控えてある場合。
//!
//! 常時回る時計は持たない。ターンが終わるたびに一発のタイマーを仕掛け、猶予が明けた時点で
//! まだ使っていなければそこで止める（何も起きていない間は CPU を使わない）。

use super::*;

impl AgentPanel {
    /// このパネルの猶予（分）。`0` = 止めない。Chat は何本も開いたままになりがちなので別の設定を持つ。
    fn idle_stop_minutes(&self, cx: &App) -> u64 {
        let settings = settings::get(cx);
        if self.chat_mode {
            settings.chat.idle_stop_minutes
        } else {
            settings.agent_idle_stop_minutes
        }
    }

    /// 猶予が明けた頃に見回る予約（ターン終了ごとに 1 回）。
    pub(crate) fn schedule_idle_agent_sweep(&mut self, cx: &mut Context<Self>) {
        let minutes = self.idle_stop_minutes(cx);
        if minutes == 0 {
            return;
        }
        let delay = std::time::Duration::from_secs(minutes * 60 + 5);
        cx.spawn(async move |panel, cx| {
            cx.background_executor().timer(delay).await;
            let _ = panel.update(cx, |panel, cx| panel.stop_idle_agents(cx));
        })
        .detach();
    }

    /// 猶予を過ぎて静かなスレッドのエージェントを止める。止めた本数を返す。
    ///
    /// 止めないもの: 走行中・承認待ち・回答待ち・送信待ちの入力があるスレッド、会話を引き継げない
    /// スレッド（止めると文脈が失われる）、まだ猶予の内のスレッド。
    pub fn stop_idle_agents(&mut self, cx: &mut Context<Self>) -> usize {
        let minutes = self.idle_stop_minutes(cx);
        if minutes == 0 {
            return 0;
        }
        let cutoff = now_unix_ms() - (minutes as i64) * 60_000;
        let mut stopped = Vec::new();
        for (index, thread) in self.threads.iter_mut().enumerate() {
            let quiet = matches!(
                thread.activity(),
                ThreadActivity::Idle | ThreadActivity::Done { .. }
            ) && thread.queued_prompts.is_empty();
            let resumable = thread.session_resumable && thread.acp_session_id.is_some();
            if thread.command_tx.is_some()
                && quiet
                && resumable
                && thread.last_active_at_ms < cutoff
            {
                thread.command_tx = None;
                // 自分で止めたセッションの終了を「切れました」と報告させない（先張りの畳みと同じ作法）。
                thread.session_serial = 0;
                stopped.push(index);
            }
        }
        if stopped.is_empty() {
            return 0;
        }
        for index in &stopped {
            self.tidy_chat_dir(*index);
        }
        self.sync_running_registry(cx);
        self.refresh_chat_rows(cx);
        cx.notify();
        stopped.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_for_test(cx: &mut gpui::TestAppContext, label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "necoder_agent_idle_{label}_{}_{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::write(
            &path,
            r#"{"onboarded":true,"agent_prewarm":false,"agent_idle_stop_minutes":15}"#,
        )
        .unwrap();
        cx.update(|cx| settings::init(Some(path.clone()), None, cx));
        path
    }

    /// 送信路だけ持つ（実エージェントを起こさない）、会話を引き継げるスレッドにする。
    fn live_thread(name: &str, last_active_at_ms: i64) -> Thread {
        let (command_tx, _commands) = mpsc::unbounded::<SessionCommand>();
        let mut thread = Thread::empty(name.to_string(), 0);
        thread.command_tx = Some(command_tx);
        thread.acp_session_id = Some(format!("session-{name}"));
        thread.session_resumable = true;
        thread.last_active_at_ms = last_active_at_ms;
        thread
    }

    #[gpui::test]
    fn only_quiet_resumable_threads_past_the_grace_are_stopped(cx: &mut gpui::TestAppContext) {
        let settings_path = settings_for_test(cx, "rules");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            let long_ago = now_unix_ms() - 60 * 60_000;
            let mut running = live_thread("running", long_ago);
            running.running = true;
            let mut queued = live_thread("queued", long_ago);
            queued.queued_prompts.push("次にこれ".into());
            let mut cannot_resume = live_thread("no-load-session", long_ago);
            cannot_resume.session_resumable = false;
            let recent = live_thread("recent", now_unix_ms());
            let mut finished = live_thread("finished-unseen", long_ago);
            finished.done = Some(TurnEnd::Completed);
            panel.threads = vec![
                live_thread("quiet", long_ago),
                running,
                queued,
                cannot_resume,
                recent,
                finished,
            ];
            panel.active = 0;

            assert_eq!(panel.stop_idle_agents(cx), 2);
            let alive: Vec<&str> = panel
                .threads
                .iter()
                .filter(|thread| thread.command_tx.is_some())
                .map(|thread| thread.name.as_ref())
                .collect();
            assert_eq!(
                alive,
                vec!["running", "queued", "no-load-session", "recent"],
                "止まるのは、静かで・引き継げて・猶予を過ぎたスレッドだけ"
            );
            let quiet = &panel.threads[0];
            assert!(
                !quiet.session_lost,
                "自分で止めたものを「切れました」にしない"
            );
            assert_eq!(
                quiet.acp_session_id.as_deref(),
                Some("session-quiet"),
                "次の送信で同じ会話を引き継ぐための id は残す"
            );
        });
        let _ = std::fs::remove_file(settings_path);
    }

    #[gpui::test]
    fn zero_minutes_turns_the_valve_off(cx: &mut gpui::TestAppContext) {
        let path = std::env::temp_dir().join(format!(
            "necoder_agent_idle_off_{}_{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        std::fs::write(&path, r#"{"onboarded":true,"agent_idle_stop_minutes":0}"#).unwrap();
        cx.update(|cx| settings::init(Some(path.clone()), None, cx));
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            panel.threads = vec![live_thread("quiet", 0)];
            panel.active = 0;
            assert_eq!(panel.stop_idle_agents(cx), 0);
            assert!(panel.threads[0].command_tx.is_some());
        });
        let _ = std::fs::remove_file(path);
    }
}
