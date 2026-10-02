//! secrets — 接続の API キーの置き場（OS のキーチェーン・issue #38 H3）。
//!
//! settings.json は平文で、同期・バックアップ・dotfiles のリポジトリで外へ出る。キーはここにだけ置く。
//! 項目の名前は `necoder.connection.<接続の id>`（[`service_name`]）、アカウントは `api-key`。
//!
//! - **macOS** = ログインのキーチェーン（キーチェーンアクセスで見える・消せる）。キーの有無は**中身を
//!   読まずに**確かめる（項目の属性だけを引く＝「キーチェーンの使用を許可しますか」を出さない）。中身を
//!   読むのはエージェントを起こす時だけ
//! - **Windows** = 資格情報マネージャ（汎用の資格情報）
//! - **Linux** はまだ持続するキーチェーン（Secret Service）に繋いでいない（D-Bus のビルド依存が要る）＝
//!   保存できない旨を [`KeychainUnavailable`] で返す
//!
//! テストと隔離した offscreen の起動は [`MemoryStore`] を使う（本物のキーチェーンに触れない）。
//! どちらを使うかは呼び手（アプリの起動）が決める — この crate は既定でキーチェーンへ行かない。

use anyhow::Result;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// キーの置き場。
pub trait SecretStore: Send + Sync {
    /// キーがあるか。macOS のキーチェーンは中身を読まずに確かめる（許可のダイアログを出さない）。
    fn contains(&self, connection_id: &str) -> Result<bool>;
    /// キーを読む（無ければ `None`）。エージェントを起こす時だけ呼ぶ（背景のスレッドから）。
    fn get(&self, connection_id: &str) -> Result<Option<String>>;
    /// キーを書く（あれば置き換える）。
    fn set(&self, connection_id: &str, secret: &str) -> Result<()>;
    /// キーを消す（無くても成功）。
    fn delete(&self, connection_id: &str) -> Result<()>;
}

/// キーチェーンの項目の名前（サービス名）。
pub fn service_name(connection_id: &str) -> String {
    format!("necoder.connection.{connection_id}")
}

/// キーチェーンの項目のアカウント名（どの接続も同じ。接続はサービス名で分ける）。
pub const ACCOUNT: &str = "api-key";

/// この OS ではキーチェーンに置けない（Linux はまだ繋いでいない）。UI はこれを見て文を出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeychainUnavailable;

impl std::fmt::Display for KeychainUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "この OS のキーチェーンにはまだ対応していない")
    }
}

impl std::error::Error for KeychainUnavailable {}

/// OS のキーチェーン（macOS / Windows）。
#[derive(Debug, Default, Clone, Copy)]
pub struct Keychain;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use super::{service_name, ACCOUNT};
    use anyhow::{Context as _, Result};

    fn entry(connection_id: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&service_name(connection_id), ACCOUNT)
            .context("キーチェーンの項目を作れない")
    }

    pub fn get(connection_id: &str) -> Result<Option<String>> {
        match entry(connection_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error).context("キーチェーンからキーを読めない"),
        }
    }

    pub fn set(connection_id: &str, secret: &str) -> Result<()> {
        entry(connection_id)?
            .set_password(secret)
            .context("キーチェーンへキーを書けない")
    }

    pub fn delete(connection_id: &str) -> Result<()> {
        match entry(connection_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error).context("キーチェーンのキーを消せない"),
        }
    }

    /// 中身を読まずに項目の有無を引く（属性だけの検索＝キーチェーンの許可を求めない）。
    #[cfg(target_os = "macos")]
    pub fn contains(connection_id: &str) -> Result<bool> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        // 項目が無い時の errSecItemNotFound。
        const NOT_FOUND: i32 = -25300;
        let result = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(&service_name(connection_id))
            .account(ACCOUNT)
            .load_attributes(true)
            .limit(Limit::Max(1))
            .search();
        match result {
            Ok(found) => Ok(!found.is_empty()),
            Err(error) if error.code() == NOT_FOUND => Ok(false),
            Err(error) => Err(error).context("キーチェーンを引けない"),
        }
    }

    /// Windows の資格情報マネージャは読むのに許可が要らないので、そのまま読む。
    #[cfg(target_os = "windows")]
    pub fn contains(connection_id: &str) -> Result<bool> {
        Ok(get(connection_id)?.is_some())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SecretStore for Keychain {
    fn contains(&self, connection_id: &str) -> Result<bool> {
        native::contains(connection_id)
    }

    fn get(&self, connection_id: &str) -> Result<Option<String>> {
        native::get(connection_id)
    }

    fn set(&self, connection_id: &str, secret: &str) -> Result<()> {
        native::set(connection_id, secret)
    }

    fn delete(&self, connection_id: &str) -> Result<()> {
        native::delete(connection_id)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl SecretStore for Keychain {
    fn contains(&self, _connection_id: &str) -> Result<bool> {
        Err(KeychainUnavailable.into())
    }

    fn get(&self, _connection_id: &str) -> Result<Option<String>> {
        Err(KeychainUnavailable.into())
    }

    fn set(&self, _connection_id: &str, _secret: &str) -> Result<()> {
        Err(KeychainUnavailable.into())
    }

    fn delete(&self, _connection_id: &str) -> Result<()> {
        Ok(())
    }
}

/// メモリだけの置き場（テストと隔離した offscreen の起動。プロセスが終われば消える）。
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: Mutex<BTreeMap<String, String>>,
}

impl MemoryStore {
    fn secrets(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        // 中で panic した前の持ち主がいても、値（ただの写し）はそのまま使える。
        self.secrets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SecretStore for MemoryStore {
    fn contains(&self, connection_id: &str) -> Result<bool> {
        Ok(self.secrets().contains_key(connection_id))
    }

    fn get(&self, connection_id: &str) -> Result<Option<String>> {
        Ok(self.secrets().get(connection_id).cloned())
    }

    fn set(&self, connection_id: &str, secret: &str) -> Result<()> {
        self.secrets()
            .insert(connection_id.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, connection_id: &str) -> Result<()> {
        self.secrets().remove(connection_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_keeps_keys_per_connection() {
        let store = MemoryStore::default();
        assert!(!store.contains("glm").expect("見られる"));
        store.set("glm", "zk-1").expect("書ける");
        store.set("glm", "zk-2").expect("置き換える");
        store.set("ds", "sk").expect("書ける");
        assert!(store.contains("glm").expect("見られる"));
        assert_eq!(store.get("glm").expect("読める").as_deref(), Some("zk-2"));
        store.delete("glm").expect("消せる");
        store.delete("glm").expect("無くても成功");
        assert_eq!(store.get("glm").expect("読める"), None);
        assert_eq!(store.get("ds").expect("読める").as_deref(), Some("sk"));
    }

    #[test]
    fn keychain_items_are_named_per_connection() {
        assert_eq!(service_name("glm"), "necoder.connection.glm");
    }
}
