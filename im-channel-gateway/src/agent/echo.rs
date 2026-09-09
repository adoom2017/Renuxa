use async_trait::async_trait;
use futures::stream;
use std::sync::Arc;

use crate::gateway::{AgentBackend, AgentEventStream};
use crate::types::{AgentEvent, ChannelRequest, EventKind, EventStatus};

pub struct EchoBackend;

#[async_trait]
impl AgentBackend for EchoBackend {
    async fn stream(&self, request: ChannelRequest) -> AgentEventStream {
        let text = request
            .content
            .iter()
            .map(|p| p.text.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let reply = if text.is_empty() {
            "(echo: empty message)".to_string()
        } else {
            format!("[echo] {text}")
        };
        let events = vec![
            Ok(AgentEvent {
                kind: EventKind::Message,
                status: EventStatus::InProgress,
                text: String::new(),
                ..Default::default()
            }),
            Ok(AgentEvent::assistant_text(reply)),
            Ok(AgentEvent::response_completed()),
        ];
        Box::pin(stream::iter(events))
    }
}

pub fn echo_backend() -> Arc<dyn AgentBackend> {
    Arc::new(EchoBackend)
}
