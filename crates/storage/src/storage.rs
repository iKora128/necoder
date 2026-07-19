//! storage — ローカル永続化 DB（Turso = SQLite の pure-Rust 再実装・MIT）。
//!
//! ARCHITECTURE §7 / DECISIONS 決定ログ 2026-07-16。用途は hot exit / スレッド永続化 /
//! トークン台帳 / checkpoint メタに**限定**（設定・todos.md は「ファイルが真実」のまま）。
//! Turso の型はこの crate の外に漏らさない（成熟度問題が出たら rusqlite へ 1 crate 差し替え）。
//!
//! **スレッドモデル**: Turso の API は async だが、GPUI に runtime を持ち込まないため
//! 専用ワーカースレッド 1 本 + チャネルで直列化し、外へは**ブロッキング API** を見せる。
//! 呼び出し側（workspace）は GPUI の background executor から呼ぶこと（UI スレッド禁止 — §9）。

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

/// ワーカーへ送る 1 ジョブ（DB 接続上で実行するクロージャ）。
type Job = Box<dyn FnOnce(&turso::Connection) + Send>;

/// ローカル DB のハンドル。クローンは薄い（内部はチャネルの送信側）。
#[derive(Clone)]
pub struct Storage {
    sender: mpsc::Sender<Job>,
}

/// 既定の DB パス（macOS）。state.json と同じディレクトリ。
pub fn default_db_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Shirushi/shirushi.db"))
}

/// 既定の blob ディレクトリ（checkpoint の content-addressed 本体・M12-2）。
pub fn default_blobs_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Shirushi/blobs"))
}

/// 内容を SHA-256 で blob へ書く（既にあれば書かない = 重複排除）。hash（hex）を返す。
fn write_blob(blobs_dir: &Path, content: &str) -> Result<String> {
    let hash = sha256_hex(content.as_bytes());
    let dir = blobs_dir.join(&hash[..2]);
    let path = dir.join(&hash);
    if !path.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("blob ディレクトリを作れない: {}", dir.display()))?;
        // 一時ファイル → rename（書きかけ blob を残さない）。
        let temp = dir.join(format!("{hash}.tmp"));
        std::fs::write(&temp, content).with_context(|| format!("blob の書き込みに失敗: {hash}"))?;
        std::fs::rename(&temp, &path).with_context(|| format!("blob の確定に失敗: {hash}"))?;
    }
    Ok(hash)
}

fn read_blob(blobs_dir: &Path, hash: &str) -> Result<String> {
    let path = blobs_dir.join(&hash[..2]).join(hash);
    std::fs::read_to_string(&path).with_context(|| format!("blob が読めない: {hash}"))
}

/// 依存なしの SHA-256（FIPS 180-4 素直実装）。checkpoint の content-address 用。
fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

impl Storage {
    /// DB を開く（無ければ作成・スキーマ初期化込み）。ワーカースレッドを 1 本起動する。
    pub fn open(path: &Path) -> Result<Storage> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("DB ディレクトリを作れない: {}", parent.display()))?;
        }
        let path = path.to_path_buf();
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_sender, ready_receiver) = mpsc::channel::<Result<()>>();
        std::thread::Builder::new()
            .name("shirushi-storage".into())
            .spawn(move || {
                let opened = futures::executor::block_on(async {
                    let db = turso::Builder::new_local(path.to_string_lossy().as_ref())
                        .build()
                        .await
                        .with_context(|| format!("DB を開けない: {}", path.display()))?;
                    let conn = db.connect().context("DB 接続に失敗")?;
                    initialize_schema(&conn).await?;
                    Ok::<turso::Connection, anyhow::Error>(conn)
                });
                let conn = match opened {
                    Ok(conn) => {
                        let _ = ready_sender.send(Ok(()));
                        conn
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                // ジョブループ（送信側が全て drop されたら終了）。
                while let Ok(job) = receiver.recv() {
                    job(&conn);
                }
            })
            .context("storage ワーカースレッドを起動できない")?;
        ready_receiver
            .recv()
            .context("storage ワーカーが応答しない")??;
        Ok(Storage { sender })
    }

    /// ジョブをワーカーで実行して結果を待つ（ブロッキング。background executor から呼ぶ）。
    fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&turso::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = mpsc::channel();
        self.sender
            .send(Box::new(move |conn| {
                let _ = sender.send(job(conn));
            }))
            .ok()
            .context("storage ワーカーが停止している")?;
        receiver.recv().context("storage ワーカーが応答しない")?
    }

    // ── hot exit（M10・dirty バッファのスナップショット） ──

    /// dirty バッファのスナップショットを upsert する。
    pub fn save_hot_exit(&self, path: &Path, content: &str) -> Result<()> {
        let path = path.to_string_lossy().to_string();
        let content = content.to_string();
        let now = unix_ms();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO hot_exit (path, content, saved_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(path) DO UPDATE SET content = ?2, saved_at = ?3",
                    (path.as_str(), content.as_str(), now),
                )
                .await
                .context("hot_exit の書き込みに失敗")?;
                Ok(())
            })
        })
    }

    /// スナップショットを 1 件消す（保存された・タブを閉じた・clean になった）。
    pub fn remove_hot_exit(&self, path: &Path) -> Result<()> {
        let path = path.to_string_lossy().to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute("DELETE FROM hot_exit WHERE path = ?1", (path.as_str(),))
                    .await
                    .context("hot_exit の削除に失敗")?;
                Ok(())
            })
        })
    }

    /// 全スナップショットを読む（起動時の復元提案用）。
    pub fn load_hot_exit_all(&self) -> Result<Vec<(PathBuf, String)>> {
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query("SELECT path, content FROM hot_exit ORDER BY path", ())
                    .await
                    .context("hot_exit の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("hot_exit の行取得に失敗")? {
                    let path: String = row.get_value(0)?.as_text().context("path が文字列でない")?.clone();
                    let content: String =
                        row.get_value(1)?.as_text().context("content が文字列でない")?.clone();
                    result.push((PathBuf::from(path), content));
                }
                Ok(result)
            })
        })
    }

    /// 全スナップショットを破棄する（正常終了・復元の「破棄」）。
    pub fn clear_hot_exit(&self) -> Result<()> {
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute("DELETE FROM hot_exit", ())
                    .await
                    .context("hot_exit のクリアに失敗")?;
                Ok(())
            })
        })
    }

    // ── スレッド永続化（M12-1・turn 毎 INSERT の追記型） ──

    /// スレッドのメタを upsert する（作成・改名・トークン累計の更新）。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_thread(
        &self,
        id: &str,
        name: &str,
        color_index: i64,
        project: &str,
        branch: Option<&str>,
        model: Option<&str>,
        tokens_used: i64,
        tokens_limit: i64,
    ) -> Result<()> {
        let (id, name, project) = (id.to_string(), name.to_string(), project.to_string());
        let branch = branch.map(str::to_string);
        let model = model.map(str::to_string);
        let now = unix_ms();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO threads (id, name, color_index, project, branch, model, tokens_used, tokens_limit, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                     ON CONFLICT(id) DO UPDATE SET
                        name = ?2, color_index = ?3, project = ?4, branch = ?5,
                        model = ?6, tokens_used = ?7, tokens_limit = ?8, updated_at = ?9",
                    (
                        id.as_str(),
                        name.as_str(),
                        color_index,
                        project.as_str(),
                        branch.as_deref(),
                        model.as_deref(),
                        tokens_used,
                        tokens_limit,
                        now,
                    ),
                )
                .await
                .context("threads の upsert に失敗")?;
                Ok(())
            })
        })
    }

    /// turn を 1 行追記する（ストリーミング確定後に呼ぶ）。
    pub fn insert_turn(&self, thread_id: &str, role: &str, content: &str) -> Result<()> {
        let (thread_id, role, content) = (thread_id.to_string(), role.to_string(), content.to_string());
        let now = unix_ms();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO turns (thread_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
                    (thread_id.as_str(), role.as_str(), content.as_str(), now),
                )
                .await
                .context("turns の追記に失敗")?;
                Ok(())
            })
        })
    }

    /// 全スレッドのメタ（updated_at 降順）。
    /// 戻り値: (id, name, color_index, project, branch, model, tokens_used, tokens_limit)。
    #[allow(clippy::type_complexity)]
    pub fn load_threads(
        &self,
    ) -> Result<Vec<(String, String, i64, String, Option<String>, Option<String>, i64, i64)>> {
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT id, name, color_index, project, branch, model, tokens_used, tokens_limit
                         FROM threads WHERE archived = 0 ORDER BY updated_at DESC",
                        (),
                    )
                    .await
                    .context("threads の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("threads 行の取得に失敗")? {
                    result.push((
                        row.get_value(0)?.as_text().context("id")?.clone(),
                        row.get_value(1)?.as_text().context("name")?.clone(),
                        *row.get_value(2)?.as_integer().context("color")?,
                        row.get_value(3)?.as_text().context("project")?.clone(),
                        row.get_value(4)?.as_text().cloned(),
                        row.get_value(5)?.as_text().cloned(),
                        *row.get_value(6)?.as_integer().context("used")?,
                        *row.get_value(7)?.as_integer().context("limit")?,
                    ));
                }
                Ok(result)
            })
        })
    }

    /// 履歴ビュー用: 全スレッドのメタ（**アーカイブ済みも含む**・updated_at 降順・#5）。
    /// 戻り値: (id, name, color_index, project, branch, tokens_used, archived)。
    #[allow(clippy::type_complexity)]
    pub fn load_all_threads(
        &self,
    ) -> Result<Vec<(String, String, i64, String, Option<String>, i64, bool)>> {
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT id, name, color_index, project, branch, tokens_used, archived
                         FROM threads ORDER BY updated_at DESC",
                        (),
                    )
                    .await
                    .context("全 threads の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("threads 行の取得に失敗")? {
                    result.push((
                        row.get_value(0)?.as_text().context("id")?.clone(),
                        row.get_value(1)?.as_text().context("name")?.clone(),
                        *row.get_value(2)?.as_integer().context("color")?,
                        row.get_value(3)?.as_text().context("project")?.clone(),
                        row.get_value(4)?.as_text().cloned(),
                        *row.get_value(5)?.as_integer().context("used")?,
                        *row.get_value(6)?.as_integer().context("archived")? != 0,
                    ));
                }
                Ok(result)
            })
        })
    }

    /// 1 スレッドの直近 `limit` turn（古い順で返す = transcript にそのまま積める）。
    pub fn load_recent_turns(&self, thread_id: &str, limit: i64) -> Result<Vec<(String, String)>> {
        let thread_id = thread_id.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT role, content FROM (
                            SELECT id, role, content FROM turns WHERE thread_id = ?1
                            ORDER BY id DESC LIMIT ?2
                         ) ORDER BY id ASC",
                        (thread_id.as_str(), limit),
                    )
                    .await
                    .context("turns の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("turns 行の取得に失敗")? {
                    result.push((
                        row.get_value(0)?.as_text().context("role")?.clone(),
                        row.get_value(1)?.as_text().context("content")?.clone(),
                    ));
                }
                Ok(result)
            })
        })
    }

    /// スレッドをアーカイブ（一覧から消す・行は残す = 台帳集計は生きる）。
    pub fn archive_thread(&self, thread_id: &str) -> Result<()> {
        let thread_id = thread_id.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute("UPDATE threads SET archived = 1 WHERE id = ?1", (thread_id.as_str(),))
                    .await
                    .context("threads のアーカイブに失敗")?;
                Ok(())
            })
        })
    }

    // ── checkpoint（M12-2・content-addressed。DECISIONS 決定ログ 2026-07-17） ──

    /// checkpoint を 1 つ記録する。`files` は (パス, その時点の内容。None = 当時ファイルが無かった)。
    /// 内容は SHA-256 で blob ディレクトリへ（同一内容は 1 個 = 重複排除）。checkpoint id を返す。
    pub fn save_checkpoint(
        &self,
        thread_id: &str,
        label: &str,
        files: Vec<(PathBuf, Option<String>)>,
        blobs_dir: &Path,
    ) -> Result<i64> {
        let (thread_id, label) = (thread_id.to_string(), label.to_string());
        let blobs_dir = blobs_dir.to_path_buf();
        let now = unix_ms();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO checkpoints (thread_id, label, created_at) VALUES (?1, ?2, ?3)",
                    (thread_id.as_str(), label.as_str(), now),
                )
                .await
                .context("checkpoints の追記に失敗")?;
                let mut rows = conn
                    .query("SELECT last_insert_rowid()", ())
                    .await
                    .context("checkpoint id の取得に失敗")?;
                let id = match rows.next().await.context("id 行の取得に失敗")? {
                    Some(row) => *row.get_value(0)?.as_integer().context("id が整数でない")?,
                    None => anyhow::bail!("checkpoint id が返らない"),
                };
                for (path, content) in files {
                    let hash = match content {
                        Some(content) => Some(write_blob(&blobs_dir, &content)?),
                        None => None,
                    };
                    conn.execute(
                        "INSERT OR REPLACE INTO checkpoint_files (checkpoint_id, path, hash) VALUES (?1, ?2, ?3)",
                        (id, path.to_string_lossy().as_ref(), hash.as_deref()),
                    )
                    .await
                    .context("checkpoint_files の追記に失敗")?;
                }
                Ok(id)
            })
        })
    }

    /// checkpoint の中身（パス, 当時の内容。None = 当時無かった → 復元では削除）。
    pub fn load_checkpoint(
        &self,
        checkpoint_id: i64,
        blobs_dir: &Path,
    ) -> Result<Vec<(PathBuf, Option<String>)>> {
        let blobs_dir = blobs_dir.to_path_buf();
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT path, hash FROM checkpoint_files WHERE checkpoint_id = ?1",
                        (checkpoint_id,),
                    )
                    .await
                    .context("checkpoint_files の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("checkpoint 行の取得に失敗")? {
                    let path = PathBuf::from(row.get_value(0)?.as_text().context("path")?.clone());
                    let content = match row.get_value(1)? {
                        turso::Value::Text(hash) => Some(read_blob(&blobs_dir, &hash)?),
                        _ => None,
                    };
                    result.push((path, content));
                }
                Ok(result)
            })
        })
    }

    /// スレッドの checkpoint 一覧（新しい順）: (id, label, created_at)。
    pub fn list_checkpoints(&self, thread_id: &str) -> Result<Vec<(i64, String, i64)>> {
        let thread_id = thread_id.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT id, label, created_at FROM checkpoints WHERE thread_id = ?1 ORDER BY id DESC",
                        (thread_id.as_str(),),
                    )
                    .await
                    .context("checkpoints の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("checkpoints 行の取得に失敗")? {
                    result.push((
                        *row.get_value(0)?.as_integer().context("id")?,
                        row.get_value(1)?.as_text().context("label")?.clone(),
                        *row.get_value(2)?.as_integer().context("at")?,
                    ));
                }
                Ok(result)
            })
        })
    }

    /// トークン台帳（M12-13）: スレッド別の累計と、今日（unix ms で日付一致）の turn 数。
    /// 台帳の本体は threads.tokens_used（ACP の実測累計）で、ここでは一覧をそのまま返す。
    pub fn token_ledger(&self) -> Result<Vec<(String, String, i64)>> {
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT id, name, tokens_used FROM threads ORDER BY tokens_used DESC",
                        (),
                    )
                    .await
                    .context("台帳の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("台帳行の取得に失敗")? {
                    result.push((
                        row.get_value(0)?.as_text().context("id")?.clone(),
                        row.get_value(1)?.as_text().context("name")?.clone(),
                        *row.get_value(2)?.as_integer().context("tokens")?,
                    ));
                }
                Ok(result)
            })
        })
    }

    // ── ホスト別の窓色（M13・リモートの色をローカルに保持。`.shirushi` はリモート側にあり identity には使えない） ──

    /// リモートホスト（ssh 別名や `ssh://…` 識別子）に割り当てた窓色（0xRRGGBB）を読む。無ければ None。
    pub fn host_color(&self, host: &str) -> Result<Option<u32>> {
        let host = host.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query("SELECT color FROM host_colors WHERE host = ?1", (host.as_str(),))
                    .await
                    .context("host_colors の読み出しに失敗")?;
                match rows.next().await.context("host_colors 行の取得に失敗")? {
                    Some(row) => {
                        let color = *row.get_value(0)?.as_integer().context("color が整数でない")?;
                        Ok(Some(color as u32))
                    }
                    None => Ok(None),
                }
            })
        })
    }

    /// リモートホストに窓色（0xRRGGBB）を割り当てて保存する（upsert）。
    pub fn set_host_color(&self, host: &str, color: u32) -> Result<()> {
        let host = host.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO host_colors (host, color) VALUES (?1, ?2)
                     ON CONFLICT(host) DO UPDATE SET color = ?2",
                    (host.as_str(), color as i64),
                )
                .await
                .context("host_colors の書き込みに失敗")?;
                Ok(())
            })
        })
    }

    /// リモートホストで最後に開いたプロジェクトパスを読む（SSH ピッカーの即接続用・M13 #2d）。
    pub fn host_last_path(&self, host: &str) -> Result<Option<String>> {
        let host = host.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query("SELECT path FROM host_last_path WHERE host = ?1", (host.as_str(),))
                    .await
                    .context("host_last_path の読み出しに失敗")?;
                match rows.next().await.context("host_last_path 行の取得に失敗")? {
                    Some(row) => {
                        Ok(Some(row.get_value(0)?.as_text().context("path が文字列でない")?.clone()))
                    }
                    None => Ok(None),
                }
            })
        })
    }

    /// リモートホストで開いたプロジェクトパスを記録する（upsert・M13 #2d）。
    pub fn set_host_last_path(&self, host: &str, path: &str) -> Result<()> {
        let host = host.to_string();
        let path = path.to_string();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO host_last_path (host, path) VALUES (?1, ?2)
                     ON CONFLICT(host) DO UPDATE SET path = ?2",
                    (host.as_str(), path.as_str()),
                )
                .await
                .context("host_last_path の書き込みに失敗")?;
                Ok(())
            })
        })
    }

    /// リモートで開いたプロジェクトを記録する（host+path で upsert・opened_at 更新・SSH 履歴）。
    /// ルート = remote $HOME の「ブラウズ接続」は呼び出し側で除外する（プロジェクトだけ残す）。
    pub fn record_remote_project(&self, host: &str, path: &str, name: &str) -> Result<()> {
        let host = host.to_string();
        let path = path.to_string();
        let name = name.to_string();
        let now = unix_ms();
        self.run(move |conn| {
            futures::executor::block_on(async {
                conn.execute(
                    "INSERT INTO remote_projects (host, path, name, opened_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(host, path) DO UPDATE SET name = ?3, opened_at = ?4",
                    (host.as_str(), path.as_str(), name.as_str(), now),
                )
                .await
                .context("remote_projects の書き込みに失敗")?;
                Ok(())
            })
        })
    }

    /// 最近開いたリモートプロジェクト（opened_at 降順）。SSH ピッカーの2階層目に出す。
    /// 戻り値: (host, path, name, opened_at)。UI は host で束ねて表示する。
    #[allow(clippy::type_complexity)]
    pub fn recent_remote_projects(&self) -> Result<Vec<(String, String, String, i64)>> {
        self.run(move |conn| {
            futures::executor::block_on(async {
                let mut rows = conn
                    .query(
                        "SELECT host, path, name, opened_at FROM remote_projects ORDER BY opened_at DESC",
                        (),
                    )
                    .await
                    .context("remote_projects の読み出しに失敗")?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.context("remote_projects 行の取得に失敗")? {
                    result.push((
                        row.get_value(0)?.as_text().context("host")?.clone(),
                        row.get_value(1)?.as_text().context("path")?.clone(),
                        row.get_value(2)?.as_text().context("name")?.clone(),
                        *row.get_value(3)?.as_integer().context("opened_at")?,
                    ));
                }
                Ok(result)
            })
        })
    }
}

/// スキーマ初期化（冪等）。将来のマイグレーションは schema_version を見て足す。
async fn initialize_schema(conn: &turso::Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)",
        (),
    )
    .await
    .context("schema_version 作成に失敗")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS hot_exit (
            path TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            saved_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("hot_exit 作成に失敗")?;
    // スレッド永続化（M12-1）。turn 毎 INSERT の追記型。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS threads (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            color_index INTEGER NOT NULL,
            project TEXT NOT NULL,
            branch TEXT,
            model TEXT,
            tokens_used INTEGER NOT NULL DEFAULT 0,
            tokens_limit INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("threads 作成に失敗")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS turns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("turns 作成に失敗")?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS turns_thread ON turns (thread_id, id)",
        (),
    )
    .await
    .context("turns index 作成に失敗")?;
    // checkpoint（M12-2・content-addressed。blob 本体はファイル、ここはメタのみ）。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS checkpoints (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            label TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("checkpoints 作成に失敗")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS checkpoint_files (
            checkpoint_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            hash TEXT,
            PRIMARY KEY (checkpoint_id, path)
        )",
        (),
    )
    .await
    .context("checkpoint_files 作成に失敗")?;
    // ホスト別の窓色（M13・リモートの色をローカルに保持）。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS host_colors (
            host TEXT PRIMARY KEY,
            color INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("host_colors 作成に失敗")?;
    // ホスト別の前回パス（M13 #2d・SSH ピッカーで即接続）。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS host_last_path (
            host TEXT PRIMARY KEY,
            path TEXT NOT NULL
        )",
        (),
    )
    .await
    .context("host_last_path 作成に失敗")?;
    // リモートで開いたプロジェクトの履歴（SSH ピッカーの2階層目・host+path でユニーク）。
    conn.execute(
        "CREATE TABLE IF NOT EXISTS remote_projects (
            host TEXT NOT NULL,
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            opened_at INTEGER NOT NULL,
            PRIMARY KEY (host, path)
        )",
        (),
    )
    .await
    .context("remote_projects 作成に失敗")?;
    Ok(())
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("shirushi_storage_{}_{}.db", tag, std::process::id()))
    }

    #[test]
    fn hot_exit_round_trip() {
        let path = temp_db("roundtrip");
        let _ = std::fs::remove_file(&path);
        let storage = Storage::open(&path).expect("DB を開ける");

        let file_a = PathBuf::from("/tmp/a.rs");
        let file_b = PathBuf::from("/tmp/b.rs");
        storage.save_hot_exit(&file_a, "content A").unwrap();
        storage.save_hot_exit(&file_b, "内容 B（日本語）").unwrap();
        // upsert（同じ path へ上書き）
        storage.save_hot_exit(&file_a, "content A v2").unwrap();

        let all = storage.load_hot_exit_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], (file_a.clone(), "content A v2".to_string()));
        assert_eq!(all[1], (file_b.clone(), "内容 B（日本語）".to_string()));

        storage.remove_hot_exit(&file_a).unwrap();
        assert_eq!(storage.load_hot_exit_all().unwrap().len(), 1);

        storage.clear_hot_exit().unwrap();
        assert!(storage.load_hot_exit_all().unwrap().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn host_colors_round_trip() {
        let path = temp_db("hostcolors");
        let _ = std::fs::remove_file(&path);
        let storage = Storage::open(&path).unwrap();

        // 未登録は None。
        assert_eq!(storage.host_color("azure_h100").unwrap(), None);
        storage.set_host_color("azure_h100", 0xef7d9b).unwrap();
        storage.set_host_color("azure_a10", 0x61afef).unwrap();
        assert_eq!(storage.host_color("azure_h100").unwrap(), Some(0xef7d9b));
        assert_eq!(storage.host_color("azure_a10").unwrap(), Some(0x61afef));
        // upsert（同じ host へ上書き）。
        storage.set_host_color("azure_h100", 0x85c46c).unwrap();
        assert_eq!(storage.host_color("azure_h100").unwrap(), Some(0x85c46c));

        // 前回パス（#2d）も同じ DB に持てる（別テーブル・upsert）。
        assert_eq!(storage.host_last_path("azure_h100").unwrap(), None);
        storage.set_host_last_path("azure_h100", "/home/daichi/proj").unwrap();
        assert_eq!(
            storage.host_last_path("azure_h100").unwrap(),
            Some("/home/daichi/proj".to_string())
        );
        storage.set_host_last_path("azure_h100", "/home/daichi/other").unwrap();
        assert_eq!(
            storage.host_last_path("azure_h100").unwrap(),
            Some("/home/daichi/other".to_string())
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remote_projects_round_trip() {
        let path = temp_db("remote_projects");
        let _ = std::fs::remove_file(&path);
        let storage = Storage::open(&path).unwrap();

        // 未登録は空。
        assert!(storage.recent_remote_projects().unwrap().is_empty());
        storage.record_remote_project("azure_h100", "/home/daichi/proj", "proj").unwrap();
        storage.record_remote_project("aws_web", "/srv/app", "app").unwrap();
        let recent = storage.recent_remote_projects().unwrap();
        assert_eq!(recent.len(), 2);
        assert!(recent
            .iter()
            .any(|row| row.0 == "azure_h100" && row.1 == "/home/daichi/proj" && row.2 == "proj"));
        // 同じ host+path を再記録 → 件数は増えず name だけ更新（upsert）。
        storage.record_remote_project("azure_h100", "/home/daichi/proj", "proj改名").unwrap();
        let recent2 = storage.recent_remote_projects().unwrap();
        assert_eq!(recent2.len(), 2);
        assert!(recent2.iter().any(|row| row.1 == "/home/daichi/proj" && row.2 == "proj改名"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn threads_and_turns_round_trip() {
        let path = temp_db("threads");
        let _ = std::fs::remove_file(&path);
        let storage = Storage::open(&path).unwrap();
        storage
            .upsert_thread("t1", "rope設計", 0, "shirushi", Some("main"), Some("claude"), 1200, 200_000)
            .unwrap();
        storage.insert_turn("t1", "user", "1+1は？").unwrap();
        storage.insert_turn("t1", "agent", "2").unwrap();
        storage
            .upsert_thread("t1", "rope設計（改名）", 0, "shirushi", Some("main"), Some("claude"), 2400, 200_000)
            .unwrap();
        storage.upsert_thread("t2", "別スレッド", 1, "probe", None, None, 0, 0).unwrap();

        let threads = storage.load_threads().unwrap();
        assert_eq!(threads.len(), 2);
        // updated_at 降順 = 直近更新の t2 or t1（同時刻あり得るので集合で確認）
        assert!(threads.iter().any(|t| t.0 == "t1" && t.1 == "rope設計（改名）" && t.6 == 2400));
        let turns = storage.load_recent_turns("t1", 10).unwrap();
        assert_eq!(turns, vec![
            ("user".to_string(), "1+1は？".to_string()),
            ("agent".to_string(), "2".to_string()),
        ]);
        // limit が効く（直近だけ・古い順）
        storage.insert_turn("t1", "user", "3つ目").unwrap();
        let last_two = storage.load_recent_turns("t1", 2).unwrap();
        assert_eq!(last_two[0].1, "2");
        assert_eq!(last_two[1].1, "3つ目");
        // アーカイブで一覧から消える
        storage.archive_thread("t2").unwrap();
        assert_eq!(storage.load_threads().unwrap().len(), 1);
        // 台帳
        let ledger = storage.token_ledger().unwrap();
        assert_eq!(ledger[0].2, 2400);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_all_threads_includes_archived() {
        let path = temp_db("all_threads");
        let _ = std::fs::remove_file(&path);
        let storage = Storage::open(&path).unwrap();
        storage
            .upsert_thread("t1", "生きてる", 0, "shirushi", Some("main"), Some("claude"), 1200, 200_000)
            .unwrap();
        storage.upsert_thread("t2", "閉じた", 1, "probe", None, None, 500, 0).unwrap();
        storage.archive_thread("t2").unwrap();
        // load_threads は archived を除外（1 件）。
        assert_eq!(storage.load_threads().unwrap().len(), 1);
        // load_all_threads は archived も含む（履歴ビュー・#5）＝ 2 件。
        let all = storage.load_all_threads().unwrap();
        assert_eq!(all.len(), 2);
        let archived = all.iter().find(|row| row.0 == "t2").expect("t2 が居る");
        assert_eq!(archived.1, "閉じた"); // name
        assert_eq!(archived.5, 500); // tokens_used
        assert!(archived.6, "t2 は archived=true");
        let live = all.iter().find(|row| row.0 == "t1").expect("t1 が居る");
        assert!(!live.6, "t1 は archived=false");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn checkpoint_round_trip_with_dedup() {
        let path = temp_db("checkpoint");
        let blobs = std::env::temp_dir().join(format!("shirushi_blobs_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&blobs);
        let storage = Storage::open(&path).unwrap();

        let a = PathBuf::from("/proj/a.rs");
        let b = PathBuf::from("/proj/b.rs");
        let c = PathBuf::from("/proj/new.rs");
        let id1 = storage
            .save_checkpoint(
                "t1",
                "ターン 1 の前",
                vec![
                    (a.clone(), Some("A v1".to_string())),
                    (b.clone(), Some("B v1".to_string())),
                    (c.clone(), None), // 当時は存在しなかった
                ],
                &blobs,
            )
            .unwrap();
        // 同一内容の再記録 = blob は増えない（重複排除）
        let id2 = storage
            .save_checkpoint("t1", "ターン 2 の前", vec![(a.clone(), Some("A v1".to_string()))], &blobs)
            .unwrap();
        assert!(id2 > id1);
        let blob_count = walkdir_count(&blobs);
        assert_eq!(blob_count, 2, "A v1 と B v1 の 2 blob だけのはず");

        let restored = storage.load_checkpoint(id1, &blobs).unwrap();
        assert_eq!(restored.len(), 3);
        assert!(restored.iter().any(|(p, content)| p == &a && content.as_deref() == Some("A v1")));
        assert!(restored.iter().any(|(p, content)| p == &c && content.is_none()));

        let list = storage.list_checkpoints("t1").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1, "ターン 2 の前"); // 新しい順

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&blobs);
    }

    fn walkdir_count(dir: &Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    count += walkdir_count(&entry.path());
                } else {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn survives_reopen_like_a_crash() {
        let path = temp_db("reopen");
        let _ = std::fs::remove_file(&path);
        {
            let storage = Storage::open(&path).expect("DB を開ける");
            storage.save_hot_exit(&PathBuf::from("/tmp/x.rs"), "unsaved!").unwrap();
        } // drop = プロセス死の代わり（flush されていること）
        let storage = Storage::open(&path).expect("再オープンできる");
        let all = storage.load_hot_exit_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].1, "unsaved!");
        let _ = std::fs::remove_file(&path);
    }
}
