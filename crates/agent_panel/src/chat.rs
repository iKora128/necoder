//! chat — AgentPanel の **Chat モード**（`docs/CHAT.md`）。
//!
//! Chat は新しいチャットエンジンを持たない。transcript・composer・承認カード・チェックポイントは
//! 既存のまま使い、違うのは 4 つだけ。それをここに集める（判断の中身は UI 非依存の `chat_core`）:
//!
//! 1. **セッションの作り方** — Chat 用のプロンプト・絞った道具・設定を持ち込まないプリセット
//! 2. **cwd** — プロジェクトではなく、チャットごとのフォルダ（最初の送信時に作る）
//! 3. **権限** — 「ファイルを渡す = 触ってよいと伝える」の裁定
//! 4. **一覧** — 開いていないチャットは DB の行として持つだけ（スレッドもエージェントも起こさない）

use super::*;
use chat_core::policy::{DenyReason, Scope, Verdict};

/// チャット 1 本の、スレッドに付く状態。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChatThreadState {
    /// チャットのフォルダ（＝エージェントの cwd）。最初の送信で決まり、以後は変えない。
    pub(crate) dir: Option<PathBuf>,
    /// 添付のうち、書き込みを「このチャットでは以後許可」にしたもの。
    pub(crate) write_grants: Vec<PathBuf>,
    pub(crate) pinned: bool,
}

/// 一覧の 1 行（workspace が左の列に描く）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRow {
    pub id: String,
    pub name: SharedString,
    pub color: Hsla,
    /// 開いている（スレッドとして実体がある）チャットだけ状態を持つ。閉じていれば `None`
    /// ＝エージェントは起きていない。
    pub activity: Option<ThreadActivity>,
    pub active: bool,
    pub pinned: bool,
    /// 最後に入力した時刻（無ければ作成時刻）。日付グループと並び順に使う。
    pub sort_at_ms: i64,
    pub artifacts: usize,
    /// 全文検索で一致した本文の抜粋（検索中だけ）。
    pub snippet: Option<SharedString>,
}

/// 権限リクエストを Chat としてどう扱うか。
pub(crate) enum ChatPermission {
    /// 聞かずに許可する。
    Allow,
    /// カードを出して聞く。`grantable` は「以後許可」を選んだ時に覚える添付。
    Ask { grantable: Vec<PathBuf> },
    /// 聞かずに拒否する。`notice` は transcript に出す 1 行。
    Deny { notice: String },
}

impl AgentPanel {
    /// Chat モードのパネル。スレッドのタブ行は出さない（一覧は workspace の左の列）。
    /// **先張りしない** — フォルダは最初のメッセージから名前が決まるので、送るまで cwd が無い。
    pub fn new_chat(theme: Theme, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new(theme, cx);
        panel.chat_mode = true;
        panel.prewarm_allowed = false;
        panel.embedded = true;
        panel.dest_project = SharedString::from(i18n::t!("chat.destination"));
        panel.threads = vec![new_chat_thread(0, cx)];
        panel.active = 0;
        panel
    }

    pub fn is_chat(&self) -> bool {
        self.chat_mode
    }

    /// 永続化 DB を受け取る。**過去のチャットはスレッドとして復元しない** — 何百本あっても起動時に
    /// 読むのは一覧の行だけで、本文（直近 200 turn）は開いた時に読む。
    pub fn set_storage_for_chat(&mut self, storage: storage::Storage, cx: &mut Context<Self>) {
        self.storage = Some(storage);
        self.storage_scope = Some(chat_core::STORAGE_SCOPE.to_string());
        self.sweep_empty_chat_dirs();
        self.refresh_chat_rows(cx);
    }

    /// 一覧（ピン留め → 新しい順）。描画のたびに DB を読まないよう、変化の節目で作り直して持つ。
    pub fn chat_rows(&self) -> &[ChatRow] {
        &self.chat_rows
    }

    pub fn chat_search_query(&self) -> &str {
        &self.chat_search
    }

    /// 一覧の絞り込み。タイトルの部分一致に加えて、本文の全文検索（`turns`）の一致も出す。
    pub fn set_chat_search(&mut self, query: String, cx: &mut Context<Self>) {
        if self.chat_search != query {
            self.chat_search = query;
            self.refresh_chat_rows(cx);
        }
    }

    pub(crate) fn refresh_chat_rows(&mut self, cx: &mut Context<Self>) {
        if !self.chat_mode {
            return;
        }
        let records = self
            .storage
            .as_ref()
            .and_then(|storage| storage.load_thread_chats().ok())
            .unwrap_or_default();
        let stored = self
            .storage
            .as_ref()
            .and_then(|storage| storage.load_threads().ok())
            .unwrap_or_default();
        let mut rows: Vec<ChatRow> = Vec::new();
        for (index, thread) in self.threads.iter().enumerate() {
            let chat = thread.chat.clone().unwrap_or_default();
            rows.push(ChatRow {
                id: thread.id.clone(),
                name: thread.name.clone(),
                color: thread.color,
                activity: Some(thread.activity()),
                active: index == self.active,
                pinned: chat.pinned,
                sort_at_ms: thread.last_input_at_ms.unwrap_or(thread.created_at_ms),
                artifacts: chat
                    .dir
                    .as_deref()
                    .map(|dir| chat_core::folder::list_artifacts(dir).len())
                    .unwrap_or(0),
                snippet: None,
            });
        }
        for (id, name, color_index, project, _, _, _, _, _, created_at, last_input_at) in stored {
            if project != chat_core::STORAGE_SCOPE || rows.iter().any(|row| row.id == id) {
                continue;
            }
            let record = records.iter().find(|record| record.thread_id == id);
            rows.push(ChatRow {
                id,
                name: SharedString::from(name),
                color: thread_color(color_index.max(0) as usize),
                activity: None,
                active: false,
                pinned: record.is_some_and(|record| record.pinned),
                sort_at_ms: last_input_at.unwrap_or(created_at),
                artifacts: record
                    .and_then(|record| record.dir.as_deref())
                    .map(|dir| chat_core::folder::list_artifacts(Path::new(dir)).len())
                    .unwrap_or(0),
                snippet: None,
            });
        }

        let query = self.chat_search.trim().to_lowercase();
        if !query.is_empty() {
            let hits = self
                .storage
                .as_ref()
                .and_then(|storage| {
                    storage
                        .search_turns(chat_core::STORAGE_SCOPE, self.chat_search.trim(), 200)
                        .ok()
                })
                .unwrap_or_default();
            rows.retain_mut(|row| {
                let hit = hits.iter().find(|(thread_id, _)| *thread_id == row.id);
                row.snippet = hit.map(|(_, content)| snippet_around(content, &query));
                row.name.to_lowercase().contains(&query) || hit.is_some()
            });
        }
        rows.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then(right.sort_at_ms.cmp(&left.sort_at_ms))
        });
        self.chat_rows = rows;
        cx.emit(PanelEvent::ChatRowsChanged);
        cx.notify();
    }

    /// 新しいチャットを開く。**まだ何も送っていない空のチャットが既にあればそれへ移る** —
    /// ⌘N を連打しても「新しいチャット」が積み上がらない。
    pub fn open_new_chat(&mut self, cx: &mut Context<Self>) {
        if !self.chat_mode {
            return;
        }
        if let Some(index) = self
            .threads
            .iter()
            .position(|thread| thread.entries.is_empty() && !thread.running)
        {
            self.switch_thread(index, cx);
        } else {
            self.save_active_draft(cx);
            self.threads
                .push(new_chat_thread(self.next_chat_color(), cx));
            let index = self.threads.len() - 1;
            self.switch_thread(index, cx);
        }
        self.refresh_chat_rows(cx);
    }

    /// 色は「いま一覧に見えている直近のチャットと被らない」ように巡回で振る。
    fn next_chat_color(&self) -> usize {
        self.chat_rows.len() + self.threads.len()
    }

    /// 一覧からチャットを開く。閉じていれば DB から本文を読んでスレッドにする（エージェントは
    /// 起こさない。次の送信で `session/load` が前回の会話を引き継ぐ）。
    pub fn open_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.chat_mode {
            return;
        }
        let Some(row) = self.chat_rows.iter().find(|row| row.id == id).cloned() else {
            return;
        };
        if self.threads.iter().all(|thread| thread.id != id) {
            let Some(storage) = self.storage.clone() else {
                return;
            };
            let color_index = (0..12)
                .find(|index| thread_color(*index) == row.color)
                .unwrap_or(0);
            let mut thread = thread_from_storage(&storage, id, &row.name, color_index, cx);
            thread.agent = "Claude Code".into();
            thread.permission_mode = SharedString::from(chat_core::preset::PERMISSION_MODE);
            thread.last_input_at_ms = Some(row.sort_at_ms);
            thread.name_is_custom = true; // 既に名前がある＝自動命名で上書きしない
            let record = storage
                .load_thread_chats()
                .unwrap_or_default()
                .into_iter()
                .find(|record| record.thread_id == id);
            if let Some(record) = &record {
                thread.context = record
                    .attachments
                    .iter()
                    .map(|path| SharedString::from(path.clone()))
                    .collect();
            }
            thread.chat = Some(ChatThreadState {
                dir: record
                    .as_ref()
                    .and_then(|record| record.dir.clone())
                    .map(PathBuf::from),
                write_grants: record
                    .as_ref()
                    .map(|record| record.write_grants.iter().map(PathBuf::from).collect())
                    .unwrap_or_default(),
                pinned: record.is_some_and(|record| record.pinned),
            });
            self.save_active_draft(cx);
            self.threads.push(thread);
        }
        if let Some(index) = self.threads.iter().position(|thread| thread.id == id) {
            self.switch_thread(index, cx);
            self.reset_transcript_list(true);
        }
        self.refresh_chat_rows(cx);
    }

    pub fn toggle_chat_pinned(&mut self, id: &str, cx: &mut Context<Self>) {
        let pinned = !self
            .chat_rows
            .iter()
            .find(|row| row.id == id)
            .is_some_and(|row| row.pinned);
        if let Some(chat) = self
            .threads
            .iter_mut()
            .find(|thread| thread.id == id)
            .and_then(|thread| thread.chat.as_mut())
        {
            chat.pinned = pinned;
        }
        if let Some(storage) = &self.storage {
            if let Err(error) = storage.set_thread_chat_pinned(id, pinned) {
                eprintln!("ピン留めの保存に失敗: {error:#}");
            }
        }
        self.refresh_chat_rows(cx);
    }

    /// このチャットのフォルダ（無ければ `None` ＝まだ何も送っていない）。
    pub fn chat_dir(&self, id: &str) -> Option<PathBuf> {
        if let Some(thread) = self.threads.iter().find(|thread| thread.id == id) {
            return thread.chat.as_ref().and_then(|chat| chat.dir.clone());
        }
        self.storage
            .as_ref()?
            .load_thread_chats()
            .ok()?
            .into_iter()
            .find(|record| record.thread_id == id)?
            .dir
            .map(PathBuf::from)
    }

    /// いま開いているチャットの成果物（新しい順）。
    pub fn active_chat_artifacts(&self) -> Vec<PathBuf> {
        self.threads
            .get(self.active)
            .and_then(|thread| thread.chat.as_ref())
            .and_then(|chat| chat.dir.as_deref())
            .map(chat_core::folder::list_artifacts)
            .unwrap_or_default()
    }

    pub fn active_chat_dir(&self) -> Option<PathBuf> {
        self.threads
            .get(self.active)
            .and_then(|thread| thread.chat.as_ref())
            .and_then(|chat| chat.dir.clone())
    }

    /// チャットを消す。履歴の行は必ず消える。フォルダは `trash_files` の時だけ**ゴミ箱へ移す**
    /// （完全削除はしない）。ファイルの無いフォルダは聞くまでもなく片付ける。
    pub fn delete_chat(&mut self, id: &str, trash_files: bool, cx: &mut Context<Self>) {
        if !self.chat_mode {
            return;
        }
        let dir = self.chat_dir(id);
        if let Some(index) = self.threads.iter().position(|thread| thread.id == id) {
            if let Some(thread) = self.threads.get_mut(index) {
                thread.command_tx = None; // エージェントを止める
            }
            self.threads.remove(index);
            if self.threads.is_empty() {
                self.threads
                    .push(new_chat_thread(self.next_chat_color(), cx));
            }
            self.active = self.active.min(self.threads.len() - 1);
            if index <= self.active && self.active > 0 && index != self.active {
                self.active -= 1;
            }
            let active = self.active;
            self.switch_thread(active, cx);
            self.reset_transcript_list(true);
        }
        if let Some(storage) = &self.storage {
            if let Err(error) = storage.delete_thread(id) {
                eprintln!("チャットの削除に失敗: {error:#}");
            }
        }
        if let Some(dir) = dir {
            if !chat_core::folder::remove_if_empty(&dir) && trash_files {
                if let Err(error) = project::trash_local(&dir) {
                    eprintln!("フォルダをゴミ箱へ移せません: {error:#}");
                }
            }
        }
        self.sync_running_registry(cx);
        self.refresh_chat_rows(cx);
    }

    /// 消す前に聞くべきか（＝フォルダにファイルが残っているか）。
    pub fn chat_has_files(&self, id: &str) -> bool {
        self.chat_dir(id).is_some_and(|dir| {
            !chat_core::folder::list_artifacts(&dir).is_empty()
                || std::fs::read_dir(&dir).is_ok_and(|entries| {
                    entries.flatten().any(|entry| {
                        let name = entry.file_name();
                        name != ".DS_Store" && name != chat_core::folder::ARTIFACTS_DIR
                    })
                })
        })
    }

    /// 送信の直前に呼ぶ。フォルダがまだ無ければ最初のメッセージから名前を決めて作り、あれば
    /// （空で片付けられていても）**同じパスで**用意する。戻り値がこのセッションの cwd。
    pub(crate) fn ensure_chat_dir(
        &mut self,
        thread_index: usize,
        first_message: &str,
        cx: &App,
    ) -> Result<PathBuf, String> {
        let existing = self
            .threads
            .get(thread_index)
            .and_then(|thread| thread.chat.as_ref())
            .and_then(|chat| chat.dir.clone());
        let dir = match existing {
            Some(dir) => {
                chat_core::folder::ensure(&dir).map_err(
                    |error| i18n::t!("chat.err_folder", "message" => &error.to_string()),
                )?;
                dir
            }
            None => {
                let configured = settings::get(cx).chat.directory;
                let root = chat_core::folder::chats_root(Some(&configured))
                    .ok_or_else(|| i18n::t!("chat.err_no_documents"))?;
                let (today, hour, minute) = chat_core::date::local_now();
                let name = chat_core::folder::folder_name(today, hour, minute, first_message);
                chat_core::folder::reserve(&root, &name)
                    .map_err(|error| i18n::t!("chat.err_folder", "message" => &error.to_string()))?
            }
        };
        if let Some(thread) = self.threads.get_mut(thread_index) {
            thread.chat.get_or_insert_with(ChatThreadState::default).dir = Some(dir.clone());
        }
        self.persist_chat_state(thread_index);
        Ok(dir)
    }

    /// フォルダ・添付・書き込み許可を DB へ（再起動をまたいで同じ裁定をするため）。
    pub(crate) fn persist_chat_state(&self, thread_index: usize) {
        let (Some(storage), Some(thread)) = (&self.storage, self.threads.get(thread_index)) else {
            return;
        };
        let Some(chat) = &thread.chat else {
            return;
        };
        let attachments: Vec<String> = thread.context.iter().map(|path| path.to_string()).collect();
        let grants: Vec<String> = chat
            .write_grants
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        let dir = chat
            .dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().to_string());
        if let Err(error) =
            storage.set_thread_chat(&thread.id, dir.as_deref(), &attachments, &grants)
        {
            eprintln!("チャットの状態の保存に失敗: {error:#}");
        }
        if chat.pinned {
            if let Err(error) = storage.set_thread_chat_pinned(&thread.id, true) {
                eprintln!("ピン留めの保存に失敗: {error:#}");
            }
        }
    }

    /// Chat のスレッドなら、セッションの希望を Chat 用に組み替える。
    ///
    /// - 権限モードは常に `default`（bypass だとエージェントが聞いてこず、裁定が素通りになる）
    /// - MCP は渡さない（ユーザー設定の MCP をチャットへ持ち込まない）
    /// - プリセットは `session/new` と `session/load` の両方に乗る（`acp_client` 側の責務）
    pub(crate) fn chat_session_preferences(
        &self,
        thread_index: usize,
        mut preferences: acp_client::SessionPreferences,
        cx: &App,
    ) -> acp_client::SessionPreferences {
        let Some(dir) = self
            .threads
            .get(thread_index)
            .and_then(|thread| thread.chat.as_ref())
            .and_then(|chat| chat.dir.as_deref())
        else {
            return preferences;
        };
        preferences.mode = Some(chat_core::preset::PERMISSION_MODE.to_string());
        preferences.mcp_servers = Vec::new();
        preferences.preset = chat_core::preset::session_preset(
            dir,
            chat_core::date::Date::today(),
            &settings::get(cx).chat.instructions,
        );
        preferences
    }

    /// 起動時の掃除: 相談だけで終わったチャットの空フォルダを消す（記録したパスは残す＝再開時に
    /// 同じパスで作り直せる）。ファイルが 1 つでもあれば触らない。
    fn sweep_empty_chat_dirs(&self) {
        let Some(storage) = &self.storage else {
            return;
        };
        for record in storage.load_thread_chats().unwrap_or_default() {
            if let Some(dir) = record.dir {
                chat_core::folder::remove_if_empty(Path::new(&dir));
            }
        }
    }

    /// エージェントが止まった時の掃除（そのチャットのフォルダが空なら消す）。
    pub(crate) fn tidy_chat_dir(&self, thread_index: usize) {
        if let Some(dir) = self
            .threads
            .get(thread_index)
            .and_then(|thread| thread.chat.as_ref())
            .and_then(|chat| chat.dir.as_deref())
        {
            chat_core::folder::remove_if_empty(dir);
        }
    }

    /// 使っていないチャットのエージェントを止める（`chat.idle_stop_minutes`）。会話は失われない —
    /// 次の送信で `session/load` が同じ会話を引き継ぐ。走行中・承認待ち・いま見ているチャットは対象外。
    pub fn stop_idle_chat_agents(&mut self, cx: &mut Context<Self>) {
        let minutes = settings::get(cx).chat.idle_stop_minutes;
        if !self.chat_mode || minutes == 0 {
            return;
        }
        let cutoff = now_unix_ms() - (minutes as i64) * 60_000;
        let active = self.active;
        let mut stopped = Vec::new();
        for (index, thread) in self.threads.iter_mut().enumerate() {
            let idle = thread.command_tx.is_some()
                && index != active
                && thread.activity() == ThreadActivity::Idle
                && thread.last_input_at_ms.unwrap_or(thread.created_at_ms) < cutoff;
            if idle {
                thread.command_tx = None;
                stopped.push(index);
            }
        }
        for index in stopped {
            self.tidy_chat_dir(index);
        }
    }

    /// 権限リクエストの裁定。Chat のスレッドでなければ `None`（＝従来どおり）。
    pub(crate) fn chat_permission(
        thread: &Thread,
        kind: Option<acp_client::ToolCallKind>,
        paths: &[String],
    ) -> Option<ChatPermission> {
        let chat = thread.chat.as_ref()?;
        let chat_dir = chat.dir.as_deref()?;
        let attachments: Vec<PathBuf> = thread
            .context
            .iter()
            .map(|path| PathBuf::from(path.as_ref()))
            .collect();
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let scope = Scope {
            chat_dir,
            attachments: &attachments,
            write_grants: &chat.write_grants,
        };
        Some(match chat_core::policy::judge(kind, &paths, scope) {
            Verdict::Allow => ChatPermission::Allow,
            Verdict::Ask => ChatPermission::Ask {
                grantable: chat_core::policy::grants_for(&paths, scope),
            },
            Verdict::Deny(DenyReason::WriteOutsideScope(path)) => ChatPermission::Deny {
                notice: i18n::t!("chat.denied_write", "path" => &path.display().to_string()),
            },
            Verdict::Deny(DenyReason::Execute) => ChatPermission::Deny {
                notice: i18n::t!("chat.denied_execute"),
            },
        })
    }
}

impl AgentPanel {
    /// いま見ているチャットの id（Chat モードでなければ `None`）。
    pub fn active_chat_id(&self) -> Option<String> {
        self.chat_mode
            .then(|| {
                self.threads
                    .get(self.active)
                    .map(|thread| thread.id.clone())
            })
            .flatten()
    }

    /// いま見ているチャットのスレッド色（右のタブ下線・キャレットの識別色）。
    pub fn active_chat_color(&self) -> Option<Hsla> {
        self.chat_mode
            .then(|| self.threads.get(self.active).map(|thread| thread.color))
            .flatten()
    }

    /// 会話を Markdown に書き出す。置き場はそのチャットのフォルダ**直下**（`artifacts/` の外＝
    /// 成果物として数えない）。まだ 1 通も送っていないチャットにはフォルダが無いので書き出せない。
    pub fn export_chat_markdown(&mut self, id: &str, cx: &App) -> Result<PathBuf, String> {
        let dir = self
            .chat_dir(id)
            .ok_or_else(|| i18n::t!("chat.err_export_empty"))?;
        let (name, markdown) = match self.threads.iter().find(|thread| thread.id == id) {
            Some(thread) => (
                thread.name.to_string(),
                (!thread.entries.is_empty()).then(|| chat_markdown(&thread.name, &thread.entries)),
            ),
            None => {
                let storage = self
                    .storage
                    .clone()
                    .ok_or_else(|| i18n::t!("chat.err_export_empty"))?;
                let name = self
                    .chat_rows
                    .iter()
                    .find(|row| row.id == id)
                    .map(|row| row.name.to_string())
                    .unwrap_or_default();
                let entries = thread_from_storage(&storage, id, &name, 0, cx).entries;
                let markdown = (!entries.is_empty()).then(|| chat_markdown(&name, &entries));
                (name, markdown)
            }
        };
        let _ = name;
        let markdown = markdown.ok_or_else(|| i18n::t!("chat.err_export_empty"))?;
        chat_core::folder::ensure(&dir)
            .map_err(|error| i18n::t!("chat.err_folder", "message" => &error.to_string()))?;
        let path = dir.join(i18n::t!("chat.export_file_name"));
        std::fs::write(&path, markdown)
            .map_err(|error| i18n::t!("chat.err_folder", "message" => &error.to_string()))?;
        Ok(path)
    }
}

/// transcript を読み物の Markdown へ。ツールの実行は 1 行の引用に畳む（出力や差分は載せない）。
fn chat_markdown(name: &str, entries: &[Entry]) -> String {
    let mut markdown = format!("# {name}\n");
    for entry in entries {
        match entry {
            Entry::User(text) | Entry::LedgerEvent(text) => {
                markdown.push_str(&format!("\n## {}\n\n{text}\n", i18n::t!("chat.export_you")));
            }
            Entry::Agent(text) => {
                markdown.push_str(&format!(
                    "\n## {}\n\n{text}\n",
                    i18n::t!("chat.export_assistant")
                ));
            }
            Entry::Step { tool, .. } => markdown.push_str(&format!("\n> ⏺ {tool}\n")),
            Entry::Thinking(_) | Entry::Checkpoint { .. } => {}
        }
    }
    markdown
}

#[cfg(debug_assertions)]
impl AgentPanel {
    /// 開発用（`NECODER_CHAT_PROBE=seed`）: エージェントを起こさずに、相談 → 成果物 → 修正の会話と
    /// 実ファイルの成果物を作る。フォルダの作成・一覧・`▣ プレビュー`・右ペインの経路は本物を通る。
    pub fn debug_seed_chat(&mut self, cx: &mut Context<Self>) {
        let active = self.active;
        let Ok(dir) = self.ensure_chat_dir(
            active,
            "ポモドーロタイマー作って。作業 25 分・休憩 5 分で",
            cx,
        ) else {
            return;
        };
        let artifact = chat_core::folder::artifacts_dir(&dir).join("pomodoro.html");
        if let Some(parent) = artifact.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("CHAT_PROBE: artifacts を作れない: {error}");
                return;
            }
        }
        let page = "<!doctype html><meta charset=utf-8><title>Pomodoro</title>\
<body style='font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0;background:#f7f8fb'>\
<main style='text-align:center'><p style='letter-spacing:.2em;color:#6b7487'>作業</p>\
<div style='font-size:64px;font-weight:600'>25:00</div>\
<button style='margin-top:20px;padding:9px 22px;border-radius:9px;border:0;background:#2f6fed;color:#fff'>開始</button>\
</main>";
        // `NECODER_CHAT_PROBE_PAGE=<html ファイル>` があれば、それを成果物として置く（隔離の検証ページ用）。
        let custom_page = std::env::var_os("NECODER_CHAT_PROBE_PAGE")
            .and_then(|path| std::fs::read_to_string(path).ok());
        let page = custom_page.as_deref().unwrap_or(page);
        if let Err(error) = std::fs::write(&artifact, page) {
            eprintln!("CHAT_PROBE: 成果物を書けない: {error}");
            return;
        }
        let path = artifact.display().to_string();
        let step = |tool: &str, old_text: Option<&str>, new_text: &str| Entry::Step {
            id: None,
            tool: SharedString::from(tool.to_string()),
            args: SharedString::from(path.clone()),
            result: None,
            result_lines: 0,
            diffs: vec![PermissionDiff {
                path: path.clone(),
                old_text: old_text.map(str::to_string),
                new_text: new_text.to_string(),
            }],
        };
        let Some(thread) = self.threads.get_mut(active) else {
            return;
        };
        thread.name = "ポモドーロタイマー".into();
        thread.name_is_custom = true;
        thread.last_input_at_ms = Some(now_unix_ms());
        thread.tokens_used = 12_400;
        thread.entries = vec![
            Entry::User("WebView の evict って 15 分が妥当？短くすると何が困る？".into()),
            Entry::Agent(
                "妥当な範囲です。短くして困るのは、タブを往復するたびに再生成の白フレームが出ることと、\
ページ内の状態（入力中のフォームやタイマー）が消えることです。\n\n\
メモリを優先するなら 5 分、作業中の往復が多いなら 15 分のままを勧めます。"
                    .into(),
            ),
            Entry::User("ポモドーロタイマー作って。作業 25 分・休憩 5 分で".into()),
            step("Write artifacts/pomodoro.html", None, "<!doctype html>…"),
            Entry::Agent(
                "作りました。開始・一時停止・リセットができ、作業と休憩が自動で切り替わります。".into(),
            ),
            Entry::User("アクセントを青にして".into()),
            step(
                "Edit artifacts/pomodoro.html",
                Some("background:#d9534f"),
                "background:#2f6fed",
            ),
            Entry::Agent("青にしました。".into()),
        ];
        self.persist_thread(active);
        self.preview_exists.borrow_mut().clear();
        self.reset_transcript_list(true);
        self.refresh_chat_rows(cx);
        if let Some(thread) = self.threads.get(active) {
            cx.emit(PanelEvent::FilesTouched {
                files: vec![artifact],
                color: thread.color,
            });
        }
        cx.notify();
    }

    /// 開発用: 過去のチャットの行を並べる（一覧の日付グループ・輪郭だけの ●・ピンの見え方）。
    pub fn debug_seed_chat_history(&mut self, cx: &mut Context<Self>) {
        let Some(storage) = self.storage.clone() else {
            return;
        };
        let names = [
            "LP のコピー案を 5 本",
            "確定申告の経費の分け方",
            "AGPL §13 の対応ソースの範囲",
            "料金表の比較図",
            "リリースノートの英訳",
        ];
        for (index, name) in names.iter().enumerate() {
            let id = format!("probe-chat-{index}");
            if let Err(error) = storage.upsert_thread(
                &id,
                name,
                (index + 1) as i64,
                chat_core::STORAGE_SCOPE,
                None,
                Some("Claude Code"),
                None,
                0,
                200_000,
            ) {
                eprintln!("CHAT_PROBE: 行を足せない: {error:#}");
            }
            if let Err(error) = storage.insert_turn(&id, "user", name) {
                eprintln!("CHAT_PROBE: turn を足せない: {error:#}");
            }
        }
        if let Err(error) = storage.set_thread_chat_pinned("probe-chat-3", true) {
            eprintln!("CHAT_PROBE: ピン留めに失敗: {error:#}");
        }
        self.refresh_chat_rows(cx);
    }
}

fn new_chat_thread(color_index: usize, cx: &App) -> Thread {
    let mut thread = Thread::empty(i18n::t!("chat.new_chat"), color_index);
    apply_thread_defaults(&mut thread, cx);
    // Chat はプロンプトの差し替えが `claude-agent-acp` 固有なので、当面 Claude 限定（DECISIONS 2026-09-18）。
    thread.agent = "Claude Code".into();
    thread.permission_mode = SharedString::from(chat_core::preset::PERMISSION_MODE);
    thread.chat = Some(ChatThreadState::default());
    thread
}

/// 一致した語の前後を 1 行に畳んで返す（一覧の行に収まる長さ）。
fn snippet_around(content: &str, lowercase_query: &str) -> SharedString {
    let flat: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let characters: Vec<char> = flat.chars().collect();
    let lowered: Vec<char> = flat.to_lowercase().chars().collect();
    let needle: Vec<char> = lowercase_query.chars().collect();
    let position = (needle.len() <= lowered.len())
        .then(|| {
            (0..=lowered.len() - needle.len())
                .find(|start| lowered[*start..*start + needle.len()] == needle[..])
        })
        .flatten()
        .unwrap_or(0);
    // 小文字化で文字数が変わる言語では位置がずれうるので、範囲は必ず元の長さに収める。
    let start = position.saturating_sub(12).min(characters.len());
    let end = (position + needle.len() + 36).min(characters.len());
    let mut snippet: String = characters[start..end].iter().collect();
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < characters.len() {
        snippet.push('…');
    }
    SharedString::from(snippet)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        settings: PathBuf,
    }

    impl Fixture {
        /// チャットの置き場を一時ディレクトリへ向けた設定で始める（実ユーザーの書類を触らない）。
        fn new(cx: &mut gpui::TestAppContext, label: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!(
                "necoder_chat_panel_{label}_{}_{}",
                std::process::id(),
                now_unix_ms()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let settings = root.join("settings.json");
            let chats = root.join("chats");
            std::fs::write(
                &settings,
                serde_json::json!({
                    "onboarded": true,
                    "chat": { "directory": chats, "instructions": "敬語は使わない" },
                })
                .to_string(),
            )
            .unwrap();
            cx.update(|cx| settings::init(Some(settings.clone()), None, cx));
            Fixture { root, settings }
        }

        fn chats(&self) -> PathBuf {
            self.root.join("chats")
        }

        fn storage(&self) -> storage::Storage {
            storage::Storage::open(&self.root.join("necoder.db")).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.settings);
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn choices() -> Vec<PermissionChoice> {
        vec![
            PermissionChoice {
                label: "Always Allow".into(),
                kind: PermissionKind::AllowAlways,
            },
            PermissionChoice {
                label: "Allow".into(),
                kind: PermissionKind::Allow,
            },
            PermissionChoice {
                label: "Reject".into(),
                kind: PermissionKind::Reject,
            },
        ]
    }

    fn edit_request(path: &Path) -> (AgentEvent, mpsc::UnboundedReceiver<usize>) {
        let (respond, answers) = mpsc::unbounded::<usize>();
        let event = AgentEvent::PermissionRequest {
            title: format!("Edit {}", path.display()),
            kind: Some(acp_client::ToolCallKind::Edit),
            paths: vec![path.display().to_string()],
            diffs: Vec::new(), // 差分なし＝チェックポイントを挟まない同期応答の経路
            raw_input: None,
            options: choices(),
            respond,
        };
        (event, answers)
    }

    /// 送信路だけ差し込む（実エージェントを起こさずに「送った prompt」を見る）。
    fn attach_fake_session(panel: &mut AgentPanel) -> mpsc::UnboundedReceiver<SessionCommand> {
        let (command_tx, commands) = mpsc::unbounded::<SessionCommand>();
        let active = panel.active;
        panel.threads[active].command_tx = Some(command_tx);
        commands
    }

    #[gpui::test]
    fn the_first_send_creates_the_folder_and_lists_the_chat(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "first_send");
        let storage = fixture.storage();
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        let mut commands = panel.update(cx, |panel, cx| {
            panel.set_storage_for_chat(storage.clone(), cx);
            assert!(
                panel.active_chat_dir().is_none(),
                "開いただけではフォルダを作らない"
            );
            assert!(!fixture.chats().exists());
            let commands = attach_fake_session(panel);
            panel.send_prompt_text("ポモドーロタイマー作って".into(), cx);
            commands
        });

        let dir = panel
            .read_with(cx, |panel, _| panel.active_chat_dir())
            .expect("フォルダ");
        let (today, _, _) = chat_core::date::local_now();
        assert_eq!(
            dir,
            fixture
                .chats()
                .join(format!("{today} ポモドーロタイマー作って"))
        );
        assert!(dir.is_dir());
        assert!(matches!(
            commands.try_recv(),
            Ok(SessionCommand::Prompt(prompt)) if prompt == "ポモドーロタイマー作って"
        ));

        // 一覧にすぐ出る（ターン終了を待たない）。フォルダの場所は DB に残る＝再開で同じ cwd。
        panel.read_with(cx, |panel, _| {
            let rows = panel.chat_rows();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].active);
            assert_eq!(rows[0].activity, Some(ThreadActivity::Working));
        });
        let records = storage.load_thread_chats().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].dir.as_deref(),
            Some(dir.to_string_lossy().as_ref())
        );

        // 2 通目は同じフォルダ（名前を付け直さない）。
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.threads[active].running = false;
            panel.send_prompt_text("青にして".into(), cx);
            assert_eq!(panel.active_chat_dir(), Some(dir.clone()));
        });
        assert_eq!(std::fs::read_dir(fixture.chats()).unwrap().count(), 1);
    }

    #[gpui::test]
    fn the_session_is_built_from_the_chat_preset(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "preset");
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            // ユーザーの既定が bypass でも、Chat は引き継がない。
            panel.threads[active].permission_mode = "bypassPermissions".into();
            let dir = panel.ensure_chat_dir(active, "相談", cx).expect("フォルダ");
            let preferences = panel.chat_session_preferences(
                active,
                acp_client::SessionPreferences {
                    mode: Some("bypassPermissions".into()),
                    mcp_servers: vec![acp_client::mcp::McpServerConfig {
                        name: "figma".into(),
                        source: acp_client::mcp::McpSource::Necoder,
                        enabled: true,
                        transport: acp_client::mcp::McpTransport::Http {
                            url: "https://example.invalid/mcp".into(),
                            headers: Default::default(),
                        },
                    }],
                    ..acp_client::SessionPreferences::default()
                },
                cx,
            );
            assert_eq!(preferences.mode.as_deref(), Some("default"));
            assert!(
                preferences.mcp_servers.is_empty(),
                "ユーザー設定の MCP を持ち込まない"
            );
            let prompt = preferences
                .preset
                .system_prompt
                .expect("プロンプトを差し替える");
            assert!(prompt.contains(&dir.join("artifacts").display().to_string()));
            assert!(
                prompt.ends_with("敬語は使わない\n"),
                "設定のカスタム指示が末尾に付く"
            );
            assert_eq!(preferences.preset.setting_sources, Some(Vec::new()));
        });
        drop(fixture);
    }

    #[gpui::test]
    fn writes_are_judged_by_where_they_land(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "policy");
        let note = fixture.root.join("note.md");
        let stranger = fixture.root.join("secret.txt");
        std::fs::write(&note, "# note").unwrap();
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        let dir = panel.update(cx, |panel, cx| {
            let active = panel.active;
            let dir = panel
                .ensure_chat_dir(active, "校正して", cx)
                .expect("フォルダ");
            panel.add_context_path(&note, cx);
            dir
        });

        // チャットのフォルダの中 → 聞かずに「今回だけ許可」で答える（「常に許可」は返さない）。
        let (event, mut answers) = edit_request(&dir.join("artifacts/timer.html"));
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.on_event(active, event, cx);
            assert!(panel.threads[active].pending_permission.is_none());
        });
        assert_eq!(answers.try_recv().ok(), Some(1));

        // 渡していない場所 → 聞かずに拒否し、理由を transcript に 1 行。
        let (event, mut answers) = edit_request(&stranger);
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.on_event(active, event, cx);
            assert!(panel.threads[active].pending_permission.is_none());
            assert!(matches!(
                panel.threads[active].entries.last(),
                Some(Entry::Agent(text)) if text.contains("secret.txt")
            ));
        });
        assert_eq!(answers.try_recv().ok(), Some(2));

        // 渡したファイル → 初回だけ聞く。「このチャットでは以後許可」は necoder が覚え、
        // エージェントには「今回だけ」で返す。
        let (event, mut answers) = edit_request(&note);
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.on_event(active, event, cx);
            let pending = panel.threads[active]
                .pending_permission
                .as_ref()
                .expect("確認カード");
            assert_eq!(pending.chat_grantable, vec![note.clone()]);
            assert_eq!(pending.options.len(), 3, "添字を保つため選択肢は間引かない");
            panel.respond_permission(active, 0, cx);
            assert_eq!(
                panel.threads[active].chat.as_ref().unwrap().write_grants,
                vec![note.clone()]
            );
        });
        assert_eq!(answers.try_recv().ok(), Some(1), "Allow（今回だけ）で返す");

        // 2 回目は聞かない。
        let (event, mut answers) = edit_request(&note);
        panel.update(cx, |panel, cx| {
            let active = panel.active;
            panel.on_event(active, event, cx);
            assert!(panel.threads[active].pending_permission.is_none());
        });
        assert_eq!(answers.try_recv().ok(), Some(1));
    }

    #[gpui::test]
    fn reopening_a_chat_restores_its_folder_attachments_and_grants(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "reopen");
        let storage = fixture.storage();
        let note = fixture.root.join("note.md");
        std::fs::write(&note, "# note").unwrap();
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        let (id, dir) = panel.update(cx, |panel, cx| {
            panel.set_storage_for_chat(storage.clone(), cx);
            let _commands = attach_fake_session(panel);
            panel.add_context_path(&note, cx);
            panel.send_prompt_text("校正して".into(), cx);
            let active = panel.active;
            panel.threads[active].chat.as_mut().unwrap().write_grants = vec![note.clone()];
            panel.persist_chat_state(active);
            (
                panel.threads[active].id.clone(),
                panel.active_chat_dir().unwrap(),
            )
        });

        // 別のパネル＝再起動後。スレッドは復元されず、一覧の行だけがある。
        let (reopened, cx) =
            cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        reopened.update(cx, |panel, cx| {
            panel.set_storage_for_chat(storage.clone(), cx);
            assert_eq!(
                panel.threads.len(),
                1,
                "過去のチャットをスレッドとして起こさない"
            );
            let row = panel
                .chat_rows()
                .iter()
                .find(|row| row.id == id)
                .expect("一覧の行");
            assert_eq!(
                row.activity, None,
                "閉じているチャットはエージェントを持たない"
            );
            panel.open_chat(&id, cx);
            let thread = &panel.threads[panel.active];
            assert_eq!(thread.id, id);
            let chat = thread.chat.as_ref().expect("Chat の状態");
            assert_eq!(chat.dir.as_ref(), Some(&dir), "cwd は前回と同じパス");
            assert_eq!(chat.write_grants, vec![note.clone()]);
            assert_eq!(
                thread.context,
                vec![SharedString::from(note.display().to_string())]
            );
            assert!(
                matches!(thread.entries.first(), Some(Entry::User(text)) if text == "校正して")
            );
        });
    }

    #[gpui::test]
    fn new_chat_reuses_the_empty_one_and_deleting_cleans_up(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "new_delete");
        let storage = fixture.storage();
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        panel.update(cx, |panel, cx| {
            panel.set_storage_for_chat(storage.clone(), cx);
            panel.open_new_chat(cx);
            panel.open_new_chat(cx);
            assert_eq!(panel.threads.len(), 1, "空のチャットを積み上げない");

            let _commands = attach_fake_session(panel);
            panel.send_prompt_text("タイマー作って".into(), cx);
            let active = panel.active;
            panel.threads[active].running = false;
            let id = panel.threads[active].id.clone();
            let dir = panel.active_chat_dir().unwrap();
            std::fs::create_dir_all(dir.join("artifacts")).unwrap();
            std::fs::write(dir.join("artifacts/timer.html"), "<p>").unwrap();
            assert!(panel.chat_has_files(&id));

            panel.open_new_chat(cx);
            assert_eq!(panel.threads.len(), 2);

            // ファイルを残して消す: 履歴の行は消え、フォルダは残る。
            panel.delete_chat(&id, false, cx);
            assert!(panel.chat_rows().iter().all(|row| row.id != id));
            assert!(storage
                .load_thread_chats()
                .unwrap()
                .iter()
                .all(|record| record.thread_id != id));
            assert!(
                dir.join("artifacts/timer.html").exists(),
                "成果物はユーザーのもの"
            );
        });
        drop(fixture);
    }

    #[gpui::test]
    fn the_preview_chip_needs_a_real_standalone_file(cx: &mut gpui::TestAppContext) {
        let fixture = Fixture::new(cx, "chip");
        let page = fixture.root.join("timer.html");
        std::fs::write(&page, "<p>").unwrap();
        let (panel, cx) = cx.add_window_view(|_window, cx| AgentPanel::new_chat(Theme::dark(), cx));
        let diff = |path: &Path| PermissionDiff {
            path: path.display().to_string(),
            old_text: None,
            new_text: "x".into(),
        };
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.step_preview_path(&[diff(&page)]), Some(page.clone()));
            assert_eq!(
                panel.step_preview_path(&[diff(&fixture.root.join("missing.html"))]),
                None
            );
            assert_eq!(
                panel.step_preview_path(&[diff(&fixture.root.join("main.rs"))]),
                None
            );
            assert_eq!(
                panel.step_preview_path(&[]),
                None,
                "差分の無い Step（Read など）には出さない"
            );
        });
    }

    #[test]
    fn the_snippet_centers_on_the_match_and_marks_the_cut_ends() {
        let content = "最初に前置きが長く続いて、そのあとでポモドーロタイマーの話になり、さらに後ろにも文が続いていく。ここからは抜粋に入りきらない長さの続きが書いてあって、末尾は省略される";
        let snippet = snippet_around(content, "タイマー");
        assert!(snippet.contains("タイマー"), "{snippet}");
        assert!(
            snippet.starts_with('…') && snippet.ends_with('…'),
            "{snippet}"
        );
        // 短い本文はそのまま。
        assert_eq!(
            snippet_around("タイマー作って", "タイマー").as_ref(),
            "タイマー作って"
        );
        // 一致が無くても落ちない（先頭から切る）。
        assert!(!snippet_around("abc", "zzz").is_empty());
    }
}
