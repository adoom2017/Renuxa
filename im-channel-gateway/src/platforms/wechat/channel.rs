use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::adapter::PlatformChannel;
use crate::config::WeChatConfig;
use crate::context::GatewayContext;
use crate::error::Result;
use crate::platforms::wechat::adapter::WeChatAccountRunner;
use crate::platforms::wechat::registry::{channel_id_for, AccountStatus, WeChatAccountRegistry};

/// How often the background reaper removes runners for merged/disabled accounts.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

struct RunnerSlot {
    runner: Arc<WeChatAccountRunner>,
}

pub struct WeChatChannel {
    cfg: WeChatConfig,
    data_dir: PathBuf,
    registry: Arc<WeChatAccountRegistry>,
    runners: Arc<Mutex<HashMap<String, RunnerSlot>>>,
    started: Arc<Mutex<bool>>,
    gateway_ctx: Arc<Mutex<Option<GatewayContext>>>,
}

impl WeChatChannel {
    pub fn new(cfg: WeChatConfig, data_dir: PathBuf) -> Result<Arc<Self>> {
        let registry = WeChatAccountRegistry::load(data_dir.clone(), cfg.max_accounts)?;
        registry.migrate_inline_config_token(&cfg.bot_token, &cfg.bot_token_file, &cfg.base_url)?;
        Ok(Arc::new(Self {
            cfg,
            data_dir,
            registry,
            runners: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(false)),
            gateway_ctx: Arc::new(Mutex::new(None)),
        }))
    }

    pub fn registry(&self) -> Arc<WeChatAccountRegistry> {
        self.registry.clone()
    }

    async fn prune_inactive_runners(&self) -> Result<()> {
        Self::prune_runners(&self.registry, &self.runners).await
    }

    async fn prune_runners(
        registry: &Arc<WeChatAccountRegistry>,
        runners: &Arc<Mutex<HashMap<String, RunnerSlot>>>,
    ) -> Result<()> {
        let active: HashSet<String> = registry
            .list_active()?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut runners = runners.lock().await;
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

    async fn start_runner(&self, ctx: &GatewayContext, account_id: &str) -> Result<()> {
        if self.runners.lock().await.contains_key(account_id) {
            return Ok(());
        }
        let entry = self
            .registry
            .get(account_id)?
            .filter(|e| e.status == AccountStatus::Active)
            .ok_or_else(|| {
                crate::error::GatewayError::Other(format!(
                    "wechat account {account_id} not found or disabled"
                ))
            })?;
        let channel_id = channel_id_for(account_id);
        let runner = Arc::new(WeChatAccountRunner::new(
            account_id.to_string(),
            channel_id,
            self.cfg.clone(),
            entry,
            self.data_dir.clone(),
            self.registry.clone(),
        )?);
        runner.start(ctx.clone()).await?;
        self.runners
            .lock()
            .await
            .insert(account_id.to_string(), RunnerSlot { runner });
        Ok(())
    }

    pub async fn register_and_start(
        &self,
        token: &str,
        base_url: &str,
        bot_user_id: Option<&str>,
    ) -> Result<String> {
        let account_id =
            self.registry
                .register_from_login_with_bot_user_id(token, base_url, bot_user_id)?;
        // Recreate the client with the newly issued token and API base URL.
        if let Some(slot) = self.runners.lock().await.remove(&account_id) {
            slot.runner.stop();
        }
        if *self.started.lock().await {
            if let Some(ctx) = self.gateway_ctx.lock().await.clone() {
                self.start_runner(&ctx, &account_id).await?;
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
                    "status": match entry.status {
                        AccountStatus::Active => "active",
                        AccountStatus::Disabled => "disabled",
                    },
                    "base_url": entry.base_url,
                    "running": running.contains(&id),
                    "created_at": entry.created_at,
                })
            })
            .collect())
    }
}

#[async_trait]
impl PlatformChannel for WeChatChannel {
    fn channel_id(&self) -> &str {
        "wechat"
    }

    async fn start(&self, ctx: GatewayContext) -> Result<()> {
        *self.gateway_ctx.lock().await = Some(ctx.clone());
        *self.started.lock().await = true;
        let active = self.registry.list_active()?;
        for (account_id, _) in active {
            if let Err(e) = self.start_runner(&ctx, &account_id).await {
                tracing::error!(account_id = %account_id, error = %e, "failed to start wechat account");
            }
        }
        let count = self.runners.lock().await.len();
        tracing::info!(count, "wechat channel started");

        // Periodically reap runners whose account was merged away (self-exited
        // via `break 'poll`) or disabled, so stale handles don't linger between
        // admin API calls.
        let registry = self.registry.clone();
        let runners = self.runners.clone();
        let started = self.started.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(PRUNE_INTERVAL);
            tick.tick().await;
            loop {
                tick.tick().await;
                if !*started.lock().await {
                    break;
                }
                if let Err(e) = Self::prune_runners(&registry, &runners).await {
                    tracing::warn!(error = %e, "wechat prune runners failed");
                }
            }
        });
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
            "channel": "wechat",
            "status": "healthy",
            "detail": format!("{count} active wechat account runner(s)"),
            "account_count": count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent::echo::echo_backend, config::AppConfig};

    #[tokio::test]
    async fn relogin_replaces_runner_and_preserves_account_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.data.dir = dir.path().to_string_lossy().into_owned();
        let channel =
            WeChatChannel::new(config.channels.wechat.clone(), dir.path().to_path_buf()).unwrap();
        let ctx = GatewayContext::new(config, echo_backend()).unwrap();
        channel.start(ctx).await.unwrap();
        let id = channel
            .register_and_start("old-token", "http://127.0.0.1:1", Some("bot-id"))
            .await
            .unwrap();
        let old = channel
            .runners
            .lock()
            .await
            .get(&id)
            .unwrap()
            .runner
            .clone();
        let refreshed = channel
            .register_and_start("fresh-token", "http://127.0.0.1:2", Some("bot-id"))
            .await
            .unwrap();
        let new = channel
            .runners
            .lock()
            .await
            .get(&id)
            .unwrap()
            .runner
            .clone();
        assert_eq!(refreshed, id);
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(channel.registry.load_token(&id).unwrap(), "fresh-token");
        assert_eq!(
            channel.registry.get(&id).unwrap().unwrap().base_url,
            "http://127.0.0.1:2"
        );
        channel.stop().await.unwrap();
    }
}
