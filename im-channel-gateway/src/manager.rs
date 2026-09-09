use std::sync::Arc;

use crate::adapter::PlatformChannel;
use crate::config::AppConfig;
use crate::context::GatewayContext;
use crate::error::Result;
use crate::gateway::SharedAgentBackend;
use crate::platforms::telegram::TelegramChannel;
use crate::platforms::wechat::WeChatChannel;

pub struct MultiChannelManager {
    ctx: GatewayContext,
    channels: Vec<Arc<dyn PlatformChannel>>,
    telegram: Option<Arc<TelegramChannel>>,
    wechat: Option<Arc<WeChatChannel>>,
}

impl MultiChannelManager {
    pub fn new(config: AppConfig, agent: SharedAgentBackend) -> Result<Self> {
        let ctx = GatewayContext::new(config, agent)?;
        let mut channels: Vec<Arc<dyn PlatformChannel>> = Vec::new();
        let mut telegram = None;
        let mut wechat = None;

        if ctx.config.channels.telegram.base.enabled {
            let ch =
                TelegramChannel::new(ctx.config.channels.telegram.clone(), ctx.config.data_path())?;
            telegram = Some(ch.clone());
            channels.push(ch);
        }
        if ctx.config.channels.wechat.base.enabled {
            let ch =
                WeChatChannel::new(ctx.config.channels.wechat.clone(), ctx.config.data_path())?;
            wechat = Some(ch.clone());
            channels.push(ch);
        }

        Ok(Self {
            ctx,
            channels,
            telegram,
            wechat,
        })
    }

    pub async fn start_all(&self) -> Result<()> {
        for ch in &self.channels {
            tracing::info!("starting channel: {}", ch.channel_id());
            ch.start(self.ctx.clone()).await?;
        }
        Ok(())
    }

    pub async fn stop_all(&self) -> Result<()> {
        for ch in &self.channels {
            ch.stop().await?;
        }
        Ok(())
    }

    pub fn context(&self) -> &GatewayContext {
        &self.ctx
    }

    pub fn wechat_channel(&self) -> Option<Arc<WeChatChannel>> {
        self.wechat.clone()
    }

    pub fn telegram_channel(&self) -> Option<Arc<TelegramChannel>> {
        self.telegram.clone()
    }
}
