use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::Result;
use crate::types::{AgentEvent, ChannelRequest};

pub type AgentEventStream = Pin<Box<dyn Stream<Item = Result<AgentEvent>> + Send>>;

#[async_trait]
pub trait AgentBackend: Send + Sync {
    async fn stream(&self, request: ChannelRequest) -> AgentEventStream;
}

pub type SharedAgentBackend = Arc<dyn AgentBackend>;
