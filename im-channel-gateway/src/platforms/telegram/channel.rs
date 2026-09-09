use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::adapter::PlatformChannel;
use crate::config::TelegramConfig;
use crate::context::GatewayContext;
use crate::error::{GatewayError, Result};
use crate::platforms::telegram::adapter::{TelegramAccountRunner, TelegramRunnerStart};
use crate::platforms::telegram::registry::{
    channel_id_for, AccountStatus, TelegramAccountRegistry,
};

struct RunnerSlot {
    runner: Arc<TelegramAccountRunner>,
}

pub struct TelegramChannel {
    cfg: TelegramConfig,
    media_base_dir: PathBuf,
    registry: Arc<TelegramAccountRegistry>,
    runners: Arc<Mutex<HashMap<String, RunnerSlot>>>,
    started: Arc<Mutex<bool>>,
    gateway_ctx: Arc<Mutex<Option<GatewayContext>>>,
}

impl TelegramChannel {
    pub fn new(cfg: TelegramConfig, data_dir: PathBuf) -> Result<Arc<Self>> {
        let registry = TelegramAccountRegistry::load(data_dir, cfg.max_accounts)?;
        let mut config_tokens = Vec::new();
        if !cfg.bot_token.trim().is_empty() {
            config_tokens.push(cfg.bot_token.clone());
        }
        config_tokens.extend(cfg.bot_tokens.iter().cloned());
        registry.register_config_tokens(&config_tokens)?;

        let media_base_dir = if cfg.base.media_dir.is_empty() {
            PathBuf::from("media/telegram")
        } else {
            PathBuf::from(&cfg.base.media_dir)
        };
        Ok(Arc::new(Self {
            cfg,
            media_base_dir,
            registry,
            runners: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(false)),
            gateway_ctx: Arc::new(Mutex::new(None)),
        }))
    }

    async fn prune_inactive_runners(&self) -> Result<()> {
        let active: HashSet<String> = self
            .registry
            .list_active()?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut runners = self.runners.lock().await;
        let stale: Vec<String> = runners
            .keys()
            .filter(|id| !active.contains(*id))
            .cloned()
            .collect();
        for account_id in stale {
            if let Some(slot) = runners.remove(&account_id) {
                slot.runner.stop();
            }
        }
        Ok(())
    }

    async fn start_runner(&self, ctx: &GatewayContext, account_id: &str) -> Result<String> {
        if self.runners.lock().await.contains_key(account_id) {
            return Ok(account_id.to_string());
        }
        let _entry = self
            .registry
            .get(account_id)?
            .filter(|e| e.status == AccountStatus::Active)
            .ok_or_else(|| {
                GatewayError::Other(format!(
                    "telegram account {account_id} not found or disabled"
                ))
            })?;
        let channel_id = channel_id_for(account_id);
        let runner = Arc::new(TelegramAccountRunner::new(
            account_id.to_string(),
            channel_id,
            self.cfg.clone(),
            self.media_base_dir.join(account_id),
            self.registry.clone(),
        )?);
        match runner.start(ctx.clone()).await? {
            TelegramRunnerStart::Started => {
                self.runners
                    .lock()
                    .await
                    .insert(account_id.to_string(), RunnerSlot { runner });
                Ok(account_id.to_string())
            }
            TelegramRunnerStart::Merged(existing_id) => Ok(existing_id),
        }
    }

    pub async fn register_and_start(&self, token: &str) -> Result<String> {
        let account_id = self.registry.register_from_token(token)?;
        if *self.started.lock().await {
            if let Some(ctx) = self.gateway_ctx.lock().await.clone() {
                return self.start_runner(&ctx, &account_id).await;
            }
        }
        Ok(account_id)
    }

    pub async fn remove_account(&self, account_id: &str) -> Result<()> {
        if let Some(slot) = self.runners.lock().await.remove(account_id) {
            slot.runner.stop();
        }
        self.registry.disable_account(account_id)
    }

    pub async fn list_accounts(&self) -> Result<Vec<Value>> {
        self.prune_inactive_runners().await?;
        let running: HashSet<String> = self.runners.lock().await.keys().cloned().collect();
        let accounts = self.registry.list_all()?;
        Ok(accounts
            .into_iter()
            .map(|(id, entry)| {
                serde_json::json!({
                    "account_id": id,
                    "channel_id": channel_id_for(&id),
                    "bot_user_id": entry.bot_user_id,
                    "username": entry.username,
                    "status": match entry.status {
                        AccountStatus::Active => "active",
                        AccountStatus::Disabled => "disabled",
                    },
                    "running": running.contains(&id),
                    "created_at": entry.created_at,
                })
            })
            .collect())
    }
}

#[async_trait]
impl PlatformChannel for TelegramChannel {
    fn channel_id(&self) -> &str {
        "telegram"
    }

    async fn start(&self, ctx: GatewayContext) -> Result<()> {
        *self.gateway_ctx.lock().await = Some(ctx.clone());
        *self.started.lock().await = true;
        let active = self.registry.list_active()?;
        for (account_id, _) in active {
            if let Err(e) = self.start_runner(&ctx, &account_id).await {
                tracing::error!(
                    account_id = %account_id,
                    error = %e,
                    "failed to start telegram account"
                );
            }
        }
        let count = self.runners.lock().await.len();
        tracing::info!(count, "telegram channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let runners: Vec<_> = self
            .runners
            .lock()
            .await
            .drain()
            .map(|(_, slot)| slot.runner)
            .collect();
        for runner in runners {
            runner.stop();
        }
        *self.started.lock().await = false;
        Ok(())
    }

    async fn health_check(&self) -> Value {
        let _ = self.prune_inactive_runners().await;
        let count = self.runners.lock().await.len();
        serde_json::json!({
            "channel": "telegram",
            "status": "healthy",
            "detail": format!("{count} active telegram account runner(s)"),
            "account_count": count,
            "streaming_enabled": self.cfg.streaming_enabled,
            "media_dir": self.media_base_dir.to_string_lossy(),
            "features": ["html", "streaming", "media_debounce", "polling_reconnect"],
        })
    }
}
