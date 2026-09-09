use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{GatewayError, Result};

const REGISTRY_FILE: &str = "telegram_accounts.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    #[default]
    Active,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramAccountEntry {
    #[serde(default)]
    pub bot_user_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub status: AccountStatus,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    accounts: HashMap<String, TelegramAccountEntry>,
}

pub fn channel_id_for(account_id: &str) -> String {
    format!("telegram:{account_id}")
}

pub fn account_dir(data_dir: &Path, account_id: &str) -> PathBuf {
    data_dir.join("telegram").join(account_id)
}

pub fn token_file_path(data_dir: &Path, account_id: &str) -> PathBuf {
    account_dir(data_dir, account_id).join("bot_token")
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn save_token_file(path: &Path, token: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, token.trim())?;
    Ok(())
}

fn load_token_file(path: &Path) -> Result<String> {
    if path.is_file() {
        Ok(std::fs::read_to_string(path)?.trim().to_string())
    } else {
        Ok(String::new())
    }
}

pub struct TelegramAccountRegistry {
    data_dir: PathBuf,
    max_accounts: usize,
    inner: Mutex<RegistryFile>,
}

impl TelegramAccountRegistry {
    pub fn load(data_dir: PathBuf, max_accounts: usize) -> Result<Arc<Self>> {
        let path = data_dir.join(REGISTRY_FILE);
        let inner = if path.is_file() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            RegistryFile::default()
        };
        Ok(Arc::new(Self {
            data_dir,
            max_accounts,
            inner: Mutex::new(inner),
        }))
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
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn list_active(&self) -> Result<Vec<(String, TelegramAccountEntry)>> {
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

    pub fn list_all(&self) -> Result<Vec<(String, TelegramAccountEntry)>> {
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

    pub fn get(&self, account_id: &str) -> Result<Option<TelegramAccountEntry>> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        Ok(guard.accounts.get(account_id).cloned())
    }

    fn find_by_token(&self, token: &str) -> Result<Option<String>> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        let accounts = self.list_all()?;
        for (id, _) in accounts {
            if load_token_file(&token_file_path(&self.data_dir, &id))? == token {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    fn active_count(&self) -> Result<usize> {
        Ok(self.list_active()?.len())
    }

    pub fn register_from_token(&self, token: &str) -> Result<String> {
        let token = token.trim();
        if token.is_empty() {
            return Err(GatewayError::Config(
                "telegram bot_token must not be empty".into(),
            ));
        }
        if let Some(existing_id) = self.find_by_token(token)? {
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| GatewayError::Other(e.to_string()))?;
            if let Some(entry) = guard.accounts.get_mut(&existing_id) {
                entry.status = AccountStatus::Active;
            }
            drop(guard);
            self.save()?;
            return Ok(existing_id);
        }
        if self.active_count()? >= self.max_accounts {
            return Err(GatewayError::Config(format!(
                "telegram account limit reached ({})",
                self.max_accounts
            )));
        }
        let account_id = format!("tg_{}", &Uuid::new_v4().simple().to_string()[..8]);
        let entry = TelegramAccountEntry {
            bot_user_id: None,
            username: None,
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

    pub fn register_config_tokens(&self, tokens: &[String]) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for token in tokens {
            if token.trim().is_empty() {
                continue;
            }
            ids.push(self.register_from_token(token)?);
        }
        Ok(ids)
    }

    pub fn upsert_bot_identity(
        &self,
        account_id: &str,
        bot_user_id: &str,
        username: &str,
    ) -> Result<Option<String>> {
        if bot_user_id.is_empty() {
            return Ok(None);
        }
        let existing = {
            let guard = self
                .inner
                .lock()
                .map_err(|e| GatewayError::Other(e.to_string()))?;
            guard
                .accounts
                .iter()
                .find(|(id, e)| {
                    id.as_str() != account_id
                        && e.status == AccountStatus::Active
                        && e.bot_user_id.as_deref() == Some(bot_user_id)
                })
                .map(|(id, _)| id.clone())
        };
        if let Some(existing_id) = existing {
            let token = load_token_file(&token_file_path(&self.data_dir, account_id))?;
            if !token.is_empty() {
                save_token_file(&token_file_path(&self.data_dir, &existing_id), &token)?;
            }
            self.disable_account(account_id)?;
            return Ok(Some(existing_id));
        }

        let mut guard = self
            .inner
            .lock()
            .map_err(|e| GatewayError::Other(e.to_string()))?;
        if let Some(entry) = guard.accounts.get_mut(account_id) {
            let changed = entry.bot_user_id.as_deref() != Some(bot_user_id)
                || entry.username.as_deref() != Some(username);
            if changed {
                entry.bot_user_id = Some(bot_user_id.to_string());
                entry.username = if username.is_empty() {
                    None
                } else {
                    Some(username.to_string())
                };
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_creates_account_with_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TelegramAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let id = registry.register_from_token("token_a").unwrap();
        assert!(id.starts_with("tg_"));
        assert_eq!(registry.load_token(&id).unwrap(), "token_a");
        assert_eq!(
            registry.get(&id).unwrap().unwrap().status,
            AccountStatus::Active
        );
    }

    #[test]
    fn duplicate_token_reuses_existing_account() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TelegramAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let first = registry.register_from_token("token_a").unwrap();
        let second = registry.register_from_token("token_a").unwrap();
        assert_eq!(first, second);
        assert_eq!(registry.list_active().unwrap().len(), 1);
    }

    #[test]
    fn upsert_bot_identity_merges_duplicate_account() {
        let dir = tempfile::tempdir().unwrap();
        let registry = TelegramAccountRegistry::load(dir.path().to_path_buf(), 20).unwrap();
        let id_a = registry.register_from_token("token_a").unwrap();
        registry.upsert_bot_identity(&id_a, "123", "bot_a").unwrap();
        let id_b = registry.register_from_token("token_b").unwrap();
        let merged = registry.upsert_bot_identity(&id_b, "123", "bot_a").unwrap();
        assert_eq!(merged.as_deref(), Some(id_a.as_str()));
        assert_eq!(registry.load_token(&id_a).unwrap(), "token_b");
        assert_eq!(
            registry.get(&id_b).unwrap().unwrap().status,
            AccountStatus::Disabled
        );
    }

    #[test]
    fn channel_id_for_formats_account_channel() {
        assert_eq!(channel_id_for("tg_abc"), "telegram:tg_abc");
    }
}
