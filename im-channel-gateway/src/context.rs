use std::sync::Arc;

use crate::acl::AccessControl;
use crate::config::AppConfig;
use crate::error::Result;
use crate::gateway::SharedAgentBackend;
use crate::pipeline::{process_native, NativeMessage, ProcessedReply};
use crate::queue::UnifiedQueue;

#[derive(Clone)]
pub struct GatewayContext {
    pub config: Arc<AppConfig>,
    pub agent: SharedAgentBackend,
    pub acl: Arc<AccessControl>,
    pub queue: Arc<UnifiedQueue>,
    pub language: String,
}

impl GatewayContext {
    pub fn new(config: AppConfig, agent: SharedAgentBackend) -> Result<Self> {
        let config = Arc::new(config);
        let acl = Arc::new(AccessControl::load_or_create(&config.data_path())?);
        Ok(Self {
            agent,
            acl,
            queue: Arc::new(UnifiedQueue::new(256)),
            language: config.agent.language.clone(),
            config,
        })
    }

    fn acl_flags(&self, channel_id: &str) -> (bool, bool) {
        match channel_id {
            "telegram" => (
                self.config.channels.telegram.base.access_control_dm,
                self.config.channels.telegram.base.access_control_group,
            ),
            id if id.starts_with("telegram:") => (
                self.config.channels.telegram.base.access_control_dm,
                self.config.channels.telegram.base.access_control_group,
            ),
            "wechat" => (
                self.config.channels.wechat.base.access_control_dm,
                self.config.channels.wechat.base.access_control_group,
            ),
            id if id.starts_with("wechat:") => (
                self.config.channels.wechat.base.access_control_dm,
                self.config.channels.wechat.base.access_control_group,
            ),
            _ => (false, false),
        }
    }

    pub async fn handle_native(&self, native: NativeMessage) -> Result<ProcessedReply> {
        let (dm_acl, group_acl) = self.acl_flags(&native.channel_id);
        process_native(
            &self.agent,
            &self.acl,
            &self.language,
            dm_acl,
            group_acl,
            native,
        )
        .await
    }
}
