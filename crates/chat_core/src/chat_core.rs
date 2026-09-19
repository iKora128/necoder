//! chat_core — **Chat モードの UI 非依存ロジック**（設計の正は `docs/CHAT.md`）。
//!
//! Chat は新しいチャットエンジンを持たない。UI は `agent_panel`、表示は既存のエディタ領域と
//! プレビューで、Chat 固有なのは次の 4 つだけ。それをここに集める:
//!
//! - [`preset`] — セッションの作り方（プロンプト・道具・設定の継承）。`acp_client` の器に詰める
//! - [`folder`] — チャットのフォルダ（書類フォルダの `necoder/<日付 最初の文>/`）の名前と寿命
//! - [`policy`] — 「ファイルを渡す = 触ってよいと伝える」を権限リクエストの裁定に落とす
//! - [`date`]   — フォルダ名とプロンプトに入れるローカルの日付
//!
//! どれも純粋関数（＋最小の IO）で、GPUI も設定スキーマも知らない。呼び手は `agent_panel`。

pub mod date;
pub mod folder;
pub mod policy;
pub mod preset;

/// スレッドの保存先スコープ（`threads.project` 列）。プロジェクトの TaskSpace id と衝突しない固定値。
pub const STORAGE_SCOPE: &str = "necoder:chat";
