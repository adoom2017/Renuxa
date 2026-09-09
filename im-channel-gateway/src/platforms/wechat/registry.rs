use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{GatewayError, Result};
use crate::platforms::wechat::login::{load_token_file, save_token_file};

const REGISTRY_FILE: &str = "wechat_accounts.json";
const LEGACY_ACCOUNT_ID: &str = "acc_legacy";
const LEGACY_TOKEN_FILE: &str = "wechat_bot_token";
const LEGACY_CURSOR_FILE: &str = "wechat_cursor";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    #[default]
    Active,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatAccountEntry {
    #[serde(default)]
    pub bot_user_id: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub status: AccountStatus,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    accounts: HashMap<String, WeChatAccountEntry>,
}

pub fn channel_id_for(account_id: &str) -> String {
    format!("wechat:{account_id}")
}

pub fn account_dir(data_dir: &Path, account_id: &str) -> PathBuf {
    data_dir.join("wechat").join(account_id)
}

pub fn token_file_path(data_dir: &Path, account_id: &str) -> PathBuf {
    account_dir(data_dir, account_id).join("bot_token")
}

pub fn cursor_file_path(data_dir: &Path, account_id: &str) -> PathBuf {
    account_dir(data_dir, account_id).join("cursor")
}

pub fn load_cursor_for_account(data_dir: &Path, account_id: &str) -> String {
    let p = cursor_file_path(data_dir, account_id);
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn save_cursor_for_account(data_dir: &Path, account_id: &str, cursor: &str) -> Result<()> {
    let dir = account_dir(data_dir, account_id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("cursor"), cursor)?;
    Ok(())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct WeChatAccountRegistry {
    data_dir: PathBuf,
    max_accounts: usize,
    inner: Mutex<RegistryFile>,
}

impl WeChatAccountRegistry {
    pub fn load(data_dir: PathBuf, max_accounts: usize) -> Result<Arc<Self>> {
        let path = data_dir.join(REGISTRY_FILE);
        let inner = if path.is_file() {
            let raw = std::fs::read_to_string(&path)?;
            match serde_json::from_str(&raw) {
                Ok(parsed) => parsed,
                Err(e) => {
                    // Don't silently drop existing account mappings: preserve the
                    // corrupt file for manual recovery instead of overwriting it.
                    let backup = path.with_extension("json.corrupt");
                    let _ = std::fs::rename(&path, &backup);
                    tracing::error!(
                        path = %path.display(),
                        backup = %backup.display(),
                        error = %e,
                        "wechat_accounts.json is corrupt; backed up and starting empty registry"
                    );
                    RegistryFile::default()
                }
            }
        } else {
            RegistryFile::default()
        };
        let registry = Arc::new(Self {
            data_dir,
            max_accounts,
            inner: Mutex::new(inner),
        });
        registry.migrate_legacy_token()?;
        Ok(registry)
    }

    fn save(&self) -> Result<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        let path = self.data_dir.join(REGISTRY_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&*guard)?;
        // Atomic write: write to a temp file then rename, so a crash mid-write
        // can never truncate the live registry.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn list_active(&self) -> Result<Vec<(String, WeChatAccountEntry)>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        Ok(guard
            .accounts
            .iter()
            .filter(|(_, e)| e.status == AccountStatus::Active)
            .map(|(id, e)| (id.clone(), e.clone()))
            .collect())
    }

    pub fn list_all(&self) -> Result<Vec<(String, WeChatAccountEntry)>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        Ok(guard
            .accounts
            .iter()
            .map(|(id, e)| (id.clone(), e.clone()))
            .collect())
    }

    pub fn get(&self, account_id: &str) -> Result<Option<WeChatAccountEntry>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        Ok(guard.accounts.get(account_id).cloned())
    }

    pub fn find_by_bot_user_id(&self, bot_user_id: &str) -> Result<Option<String>> {
        if bot_user_id.is_empty() {
            return Ok(None);
        }
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        Ok(guard
            .accounts
            .iter()
            .find(|(_, e)| {
                e.status == AccountStatus::Active && e.bot_user_id.as_deref() == Some(bot_user_id)
            })
            .map(|(id, _)| id.clone()))
    }

    pub fn register_from_login(&self, token: &str, base_url: &str) -> Result<String> {
        self.register_from_login_with_bot_user_id(token, base_url, None)
    }

    pub fn register_from_login_with_bot_user_id(
        &self,
        token: &str,
        base_url: &str,
        bot_user_id: Option<&str>,
    ) -> Result<String> {
        let bot_user_id = bot_user_id.map(str::trim).filter(|v| !v.is_empty());
        if let Some(bot_user_id) = bot_user_id {
            if let Some(existing_id) = self.find_by_bot_user_id(bot_user_id)? {
                self.update_existing_login(&existing_id, token, base_url)?;
                return Ok(existing_id);
            }
        }
        let active_count = self.list_active()?.len();
        if active_count >= self.max_accounts {
            return Err(GatewayError::Config(format!(
                "wechat account limit reached ({})",
                self.max_accounts
            )));
        }
        let account_id = format!("acc_{}", &Uuid::new_v4().simple().to_string()[..8]);
        let entry = WeChatAccountEntry {
            bot_user_id: bot_user_id.map(str::to_string),
            base_url: if base_url.is_empty() {
                crate::platforms::wechat::client::DEFAULT_BASE_URL.to_string()
            } else {
                base_url.to_string()
            },
            status: AccountStatus::Active,
            created_at: now_unix(),
        };
        save_token_file(&token_file_path(&self.data_dir, &account_id), token)?;
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| GatewayError::Other(e.to_string()))?;
            guard.accounts.insert(account_id.clone(), entry);
        }
        self.save()?;
        Ok(account_id)
    }

    fn update_existing_login(&self, account_id: &str, token: &str, base_url: &str) -> Result<()> {
        save_token_file(&token_file_path(&self.data_dir, account_id), token)?;
        if !base_url.is_empty() {
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| GatewayError::Other(e.to_string()))?;
            if let Some(entry) = guard.accounts.get_mut(account_id) {
                if entry.base_url != base_url {
                    entry.base_url = base_url.to_string();
                    drop(guard);
                    self.save()?;
                }
            }
        }
        Ok(())
    }

    pub fn update_token(&self, account_id: &str, token: &str) -> Result<()> {
        save_token_file(&token_file_path(&self.data_dir, account_id), token)?;
        Ok(())
    }

    pub fn upsert_bot_user_id(
        &self,
        account_id: &str,
        bot_user_id: &str,
    ) -> Result<Option<String>> {
        if bot_user_id.is_empty() {
            return Ok(None);
        }
        let existing = self.find_by_bot_user_id(bot_user_id)?;
        if let Some(existing_id) = existing {
            if existing_id != account_id {
                let token = load_token_file(&token_file_path(&self.data_dir, account_id))?;
                if !token.is_empty() {
                    save_token_file(&token_file_path(&self.data_dir, &existing_id), &token)?;
                }
                self.disable_account(account_id)?;
                return Ok(Some(existing_id));
            }
        }
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        if let Some(entry) = guard.accounts.get_mut(account_id) {
            if entry.bot_user_id.as_deref() != Some(bot_user_id) {
                entry.bot_user_id = Some(bot_user_id.to_string());
                drop(guard);
                self.save()?;
            }
        }
        Ok(None)
    }

    pub fn disable_account(&self, account_id: &str) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        if let Some(entry) = guard.accounts.get_mut(account_id) {
            entry.status = AccountStatus::Disabled;
        }
        drop(guard);
        self.save()
    }

    pub fn load_token(&self, account_id: &str) -> Result<String> {
        load_token_file(&token_file_path(&self.data_dir, account_id))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn migrate_legacy_token(&self) -> Result<()> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        if !guard.accounts.is_empty() {
            return Ok(());
        }
        drop(guard);

        let legacy_token_path = self.data_dir.join(LEGACY_TOKEN_FILE);
        let legacy_token = load_token_file(&legacy_token_path)?;
        if legacy_token.is_empty() {
            return Ok(());
        }

        let entry = WeChatAccountEntry {
            bot_user_id: None,
            base_url: crate::platforms::wechat::client::DEFAULT_BASE_URL.to_string(),
            status: AccountStatus::Active,
            created_at: now_unix(),
        };
        save_token_file(
            &token_file_path(&self.data_dir, LEGACY_ACCOUNT_ID),
            &legacy_token,
        )?;

        let legacy_cursor_path = self.data_dir.join(LEGACY_CURSOR_FILE);
        if legacy_cursor_path.is_file() {
            let cursor = std::fs::read_to_string(&legacy_cursor_path).unwrap_or_default();
            save_cursor_for_account(&self.data_dir, LEGACY_ACCOUNT_ID, cursor.trim())?;
        }

        let mut guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        guard.accounts.insert(LEGACY_ACCOUNT_ID.to_string(), entry);
        drop(guard);
        self.save()?;
        tracing::info!(
            account_id = LEGACY_ACCOUNT_ID,
            "migrated legacy wechat_bot_token into account registry"
        );
        Ok(())
    }

    pub fn migrate_inline_config_token(
        &self,
        inline_token: &str,
        inline_token_file: &str,
        base_url: &str,
    ) -> Result<()> {
        if inline_token.is_empty() && inline_token_file.is_empty() {
            return Ok(());
        }
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        if !guard.accounts.is_empty() {
            return Ok(());
        }
        drop(guard);

        let token = if !inline_token.is_empty() {
            inline_token.to_string()
        } else {
            load_token_file(Path::new(inline_token_file))?
        };
        if token.is_empty() {
            return Ok(());
        }
        let entry = WeChatAccountEntry {
            bot_user_id: None,
            base_url: if base_url.is_empty() {
                crate::platforms::wechat::client::DEFAULT_BASE_URL.to_string()
            } else {
                base_url.to_string()
            },
            status: AccountStatus::Active,
            created_at: now_unix(),
        };
        save_token_file(&token_file_path(&self.data_dir, LEGACY_ACCOUNT_ID), &token)?;
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        guard.accounts.insert(LEGACY_ACCOUNT_ID.to_string(), entry);
        drop(guard);
        self.save()?;
        tracing::info!(
            account_id = LEGACY_ACCOUNT_ID,
            "migrated inline wechat config token into account registry"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_creates_account_with_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let id = registry
            .register_from_login("token_a", "https://example.test")
            .unwrap();
        assert!(id.starts_with("acc_"));
        let token = registry.load_token(&id).unwrap();
        assert_eq!(token, "token_a");
        let entry = registry.get(&id).unwrap().unwrap();
        assert_eq!(entry.base_url, "https://example.test");
        assert_eq!(entry.status, AccountStatus::Active);
    }

    #[test]
    fn upsert_bot_user_id_merges_duplicate_account() {
        let dir = tempfile::tempdir().unwrap();
        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let id_a = registry.register_from_login("token_a", "").unwrap();
        registry.upsert_bot_user_id(&id_a, "wx_bot_1").unwrap();
        let id_b = registry.register_from_login("token_b", "").unwrap();
        let merged = registry.upsert_bot_user_id(&id_b, "wx_bot_1").unwrap();
        assert_eq!(merged.as_deref(), Some(id_a.as_str()));
        assert_eq!(registry.load_token(&id_a).unwrap(), "token_b");
        assert_eq!(
            registry.get(&id_b).unwrap().unwrap().status,
            AccountStatus::Disabled
        );
    }

    #[test]
    fn register_from_login_stores_bot_user_id() {
        let dir = tempfile::tempdir().unwrap();
        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let id = registry
            .register_from_login_with_bot_user_id("token_a", "", Some(" wx_bot_1 "))
            .unwrap();

        let entry = registry.get(&id).unwrap().unwrap();
        assert_eq!(entry.bot_user_id.as_deref(), Some("wx_bot_1"));
    }

    #[test]
    fn register_from_login_reuses_existing_bot_user_id() {
        let dir = tempfile::tempdir().unwrap();
        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let id_a = registry
            .register_from_login_with_bot_user_id("token_a", "https://old.test", Some("wx_bot_1"))
            .unwrap();
        let id_b = registry
            .register_from_login_with_bot_user_id("token_b", "https://new.test", Some("wx_bot_1"))
            .unwrap();

        assert_eq!(id_a, id_b);
        assert_eq!(registry.list_active().unwrap().len(), 1);
        assert_eq!(registry.load_token(&id_a).unwrap(), "token_b");
        assert_eq!(
            registry.get(&id_a).unwrap().unwrap().base_url,
            "https://new.test"
        );
    }

    #[test]
    fn migrate_legacy_token_imports_single_account() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(LEGACY_TOKEN_FILE), "legacy_token").unwrap();
        std::fs::write(dir.path().join(LEGACY_CURSOR_FILE), "cursor_1").unwrap();
        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let accounts = registry.list_active().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].0, LEGACY_ACCOUNT_ID);
        assert_eq!(
            registry.load_token(LEGACY_ACCOUNT_ID).unwrap(),
            "legacy_token"
        );
        assert_eq!(
            load_cursor_for_account(dir.path(), LEGACY_ACCOUNT_ID),
            "cursor_1"
        );
    }

    #[test]
    fn channel_id_for_formats_account_channel() {
        assert_eq!(channel_id_for("acc_abc"), "wechat:acc_abc");
    }

    #[test]
    fn save_survives_reload_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
            registry.register_from_login("token_a", "").unwrap()
        };
        let reloaded = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        assert!(reloaded.get(&id).unwrap().is_some());
        assert_eq!(reloaded.load_token(&id).unwrap(), "token_a");
    }

    #[test]
    fn load_backs_up_corrupt_registry_instead_of_dropping_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REGISTRY_FILE);
        std::fs::write(&path, "{ this is not valid json").unwrap();

        let registry = WeChatAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();

        // Starts empty rather than panicking, and preserves the corrupt file.
        assert!(registry.list_all().unwrap().is_empty());
        assert!(path.with_extension("json.corrupt").is_file());
    }
}
