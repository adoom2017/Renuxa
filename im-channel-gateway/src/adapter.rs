use async_trait::async_trait;
use serde_json::Value;

use crate::context::GatewayContext;
use crate::error::Result;

#[async_trait]
pub trait PlatformChannel: Send + Sync {
    fn channel_id(&self) -> &str;

    async fn start(&self, ctx: GatewayContext) -> Result<()>;

    async fn stop(&self) -> Result<()>;

    async fn health_check(&self) -> Value;
}
