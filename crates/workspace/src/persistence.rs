//! 窓セッション（開プロジェクト列・各プロジェクトの開タブ列・アクティブ）の永続化。
//!
//! 保存先は necoder.db の `window_sessions`（1 窓 = 1 行・payload = ここで定義する JSON）。
//! 旧 `state.json`（全窓共有の 1 ファイルを丸ごと上書き）は「最後に書いた窓が勝つ」ため、別窓で
//! 閉じたタブが次回起動で復活していた（2026-09-03）。互換経路は持たない＝`state.json` は読みも書きもしない。
//!
//! 書き込みは窓ごとの [`WindowSessionWriter`] が**合流**する: UI スレッドは最新 payload を郵便受けに
//! 置くだけで、background の 1 本の書き手が「その時点の最新」だけを DB へ流す。並列 spawn だと
//! 古い payload が新しいものを追い越して上書きし得る（閉じたタブが戻る同じ事故の再来）ので、
//! 順序はここで保証する。

use gpui::{App, BackgroundExecutor, Window};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedProject {
    pub(crate) root: PathBuf,
    #[serde(default)]
    pub(crate) open_files: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) active_file: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) remote_uri: Option<String>,
}

/// 窓 1 つ分の payload（`window_sessions.payload` の JSON）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PersistedState {
    pub(crate) projects: Vec<PersistedProject>,
    #[serde(default)]
    pub(crate) active: usize,
}

#[derive(Debug, Clone)]
pub struct SavedProject {
    pub root: PathBuf,
    pub open_files: Vec<PathBuf>,
    pub active_file: usize,
    pub remote_uri: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RestoredTabs {
    pub files: Vec<PathBuf>,
    pub active: usize,
}

impl RestoredTabs {
    pub fn single(file: PathBuf) -> Self {
        Self {
            files: vec![file],
            active: 0,
        }
    }
}

/// 窓 1 つ分の永続化ハンドル。`window_id` が None なら DB は使うが窓セッションは書かない
/// （offscreen 撮影・プローブ: ユーザーの通常セッションを汚さない）。
#[derive(Clone)]
pub struct WindowPersistence {
    /// `None` は呼び出し側が「DB は利用不能 / 利用しない」と判断済み。
    /// `Option<WindowPersistence>` の外側の None は旧 API 互換で「必要なら自前で開く」を表す。
    pub storage: Option<storage::Storage>,
    pub window_id: Option<String>,
}

/// 新しい窓の ID。プロセスをまたいで衝突しなければよい（時刻 + プロセス内乱数）。
pub fn new_window_session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{now:x}-{random:016x}")
}

/// ローカル DB を開く（NECODER_DB でパス上書き・テストでは開かない）。失敗は標準エラーへ出して None。
pub fn open_default_storage() -> Option<storage::Storage> {
    let db_path = std::env::var("NECODER_DB")
        .map(PathBuf::from)
        .ok()
        .or_else(|| (!cfg!(test)).then(storage::default_db_path).flatten())?;
    match storage::Storage::open(&db_path) {
        Ok(storage) => Some(storage),
        Err(error) => {
            eprintln!("ローカル DB を開けない（hot exit・窓セッション無効）: {error:#}");
            None
        }
    }
}

/// `window_sessions.payload` を復元用の型へ。壊れていれば None（その行は捨てる）。
pub fn decode_window_session(payload: &str) -> Option<(Vec<SavedProject>, usize)> {
    let state: PersistedState = serde_json::from_str(payload).ok()?;
    if state.projects.is_empty() {
        return None;
    }
    let projects = state
        .projects
        .into_iter()
        .map(|project| SavedProject {
            root: project.root,
            open_files: project.open_files,
            active_file: project.active_file,
            remote_uri: project.remote_uri,
        })
        .collect();
    Some((projects, state.active))
}

pub(crate) fn encode_window_session(state: &PersistedState) -> Option<String> {
    match serde_json::to_string(state) {
        Ok(payload) => Some(payload),
        Err(error) => {
            eprintln!("窓セッションを JSON にできない: {error}");
            None
        }
    }
}

/// ⌘Q 中か。終了時は OS が窓を閉じるが、それは「ユーザーが窓を閉じた」ではないので閉じ印を付けない
/// （付けると次回起動で何も戻らない）。
static QUITTING: AtomicBool = AtomicBool::new(false);

pub fn mark_quitting() {
    QUITTING.store(true, Ordering::SeqCst);
}

pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// 窓セッションの合流書き手（1 窓 1 つ）。
pub(crate) struct WindowSessionWriter {
    storage: storage::Storage,
    window_id: String,
    /// 最新 payload の郵便受け。書き手はここから取り出した「その時点の最新」だけを書く。
    mailbox: Arc<Mutex<Option<String>>>,
    /// 書き手が走っているか（二重起動を避ける）。
    running: Arc<AtomicBool>,
    /// 窓が閉じられた後は書かない（閉じ印の後に遅れて上書きしない）。
    closed: Arc<AtomicBool>,
    /// background save と終了時 flush の順序を固定する。
    write_gate: Arc<Mutex<()>>,
}

impl WindowSessionWriter {
    pub(crate) fn window_id(&self) -> &str {
        &self.window_id
    }

    pub(crate) fn new(storage: storage::Storage, window_id: String) -> Self {
        Self {
            storage,
            window_id,
            mailbox: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
            write_gate: Arc::new(Mutex::new(())),
        }
    }

    /// 最新 payload を置き、書き手が居なければ background で起こす。UI スレッドで DB を待たない。
    pub(crate) fn save(&self, payload: String, executor: &BackgroundExecutor) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        *lock_mailbox(&self.mailbox) = Some(payload);
        if self.running.swap(true, Ordering::SeqCst) {
            return; // 走っている書き手が郵便受けの最新を拾う
        }
        let storage = self.storage.clone();
        let window_id = self.window_id.clone();
        let mailbox = self.mailbox.clone();
        let running = self.running.clone();
        let closed = self.closed.clone();
        let write_gate = self.write_gate.clone();
        executor
            .spawn(async move {
                loop {
                    let next = lock_mailbox(&mailbox).take();
                    let Some(payload) = next else {
                        running.store(false, Ordering::SeqCst);
                        // 空を見た直後に置かれた分を取りこぼさない（置いた側は running=true を見て帰っている）。
                        if lock_mailbox(&mailbox).is_some() && !running.swap(true, Ordering::SeqCst)
                        {
                            continue;
                        }
                        break;
                    };
                    let _write_guard = write_gate
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if closed.load(Ordering::SeqCst) {
                        running.store(false, Ordering::SeqCst);
                        break;
                    }
                    if let Err(error) = storage.upsert_window_session(&window_id, &payload) {
                        eprintln!("窓セッションを保存できない: {error:#}");
                    }
                }
            })
            .detach();
    }

    /// ⌘Q / 再起動の直前に、その瞬間の完全な payload を同期保存する。
    ///
    /// detached background task はプロセス終了までの完了を保証できない。先に `closed` を立て、
    /// 実行中の書き込みと gate で合流してから最新値を最後に書くことで、古い payload の追い越しも防ぐ。
    pub(crate) fn flush_for_quit(&self, payload: String) {
        self.closed.store(true, Ordering::SeqCst);
        lock_mailbox(&self.mailbox).take();
        let _write_guard = self
            .write_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(error) = self
            .storage
            .upsert_window_session(&self.window_id, &payload)
        {
            eprintln!("終了時に窓セッションを保存できない: {error:#}");
        }
    }

    /// ユーザーが窓を閉じた: 以後の書き込みを止め、行に閉じ印を付ける（二度目以降は no-op）。
    /// 窓を閉じた直後に ⌘Q されても印が残るよう、ここだけは同期で書く（1 UPDATE・§9 の例外）。
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        lock_mailbox(&self.mailbox).take();
        if let Err(error) = self.storage.mark_window_session_closed(&self.window_id) {
            eprintln!("窓セッションに閉じ印を付けられない: {error:#}");
        }
    }
}

fn lock_mailbox(mailbox: &Mutex<Option<String>>) -> std::sync::MutexGuard<'_, Option<String>> {
    mailbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 窓を閉じるときに自分の行へ閉じ印を付けるフックを登録する（窓を開いた直後に呼ぶ）。
/// OS の閉じるボタン / ⌘⇧W が通る経路。⌘Q による窓の破棄では付けない（[`mark_quitting`]）。
/// 自前 titlebar の × は `remove_window()` 直叩きでここを通らないので、
/// `Workspace::mark_window_closed` を先に呼ぶ。
pub fn install_window_close_hook(window: &Window, cx: &App, persistence: &WindowPersistence) {
    let (Some(storage), Some(window_id)) =
        (persistence.storage.clone(), persistence.window_id.clone())
    else {
        return;
    };
    let writer = WindowSessionWriter::new(storage, window_id);
    window.on_window_should_close(cx, move |_window, _cx| {
        if !is_quitting() {
            writer.close();
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_session_payload_round_trip() {
        let state = PersistedState {
            projects: vec![
                PersistedProject {
                    root: PathBuf::from("/tmp/one"),
                    open_files: vec![
                        PathBuf::from("/tmp/one/a.rs"),
                        PathBuf::from("/tmp/one/b.rs"),
                    ],
                    active_file: 1,
                    remote_uri: None,
                },
                PersistedProject {
                    root: PathBuf::from("/tmp/remote"),
                    open_files: Vec::new(),
                    active_file: 0,
                    remote_uri: Some("ssh://host/tmp/remote".to_string()),
                },
            ],
            active: 1,
        };
        let payload = encode_window_session(&state).expect("JSON にできる");
        let (projects, active) = decode_window_session(&payload).expect("復元できる");
        assert_eq!(active, 1);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].open_files, state.projects[0].open_files);
        assert_eq!(projects[0].active_file, 1);
        assert_eq!(
            projects[1].remote_uri.as_deref(),
            Some("ssh://host/tmp/remote")
        );
    }

    #[test]
    fn empty_or_broken_payload_is_dropped() {
        assert!(decode_window_session(r#"{"projects":[],"active":0}"#).is_none());
        assert!(decode_window_session("not json").is_none());
    }

    #[test]
    fn window_session_ids_do_not_collide() {
        let first = new_window_session_id();
        let second = new_window_session_id();
        assert_ne!(first, second);
    }

    #[test]
    fn quit_flush_persists_the_latest_window_session_synchronously() {
        let suffix = new_window_session_id();
        let db_path = std::env::temp_dir().join(format!("necoder-quit-flush-{suffix}.db"));
        let storage = storage::Storage::open(&db_path).unwrap();
        let writer = WindowSessionWriter::new(storage.clone(), "window-1".to_string());
        let payload = r#"{"projects":[{"root":"/tmp/latest"}]}"#.to_string();

        writer.flush_for_quit(payload.clone());

        assert_eq!(
            storage.claim_window_sessions().unwrap(),
            vec![("window-1".to_string(), payload)]
        );
        drop(writer);
        drop(storage);
        let _ = std::fs::remove_file(db_path);
    }
}
