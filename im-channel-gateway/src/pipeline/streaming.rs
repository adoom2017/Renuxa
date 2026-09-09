use std::collections::HashMap;

use async_trait::async_trait;
use futures::StreamExt;

use crate::acl::AccessControl;
use crate::error::Result;
use crate::gateway::SharedAgentBackend;
use crate::pipeline::{native_to_request, NativeMessage, ProcessedReply};
use crate::types::{AgentEvent, ChannelPart, EventKind, EventStatus};

const STREAMABLE: &[&str] = &["message", "reasoning"];

fn resolve_stream_type(event: &AgentEvent) -> &'static str {
    match event.kind {
        EventKind::Reasoning => "reasoning",
        EventKind::Message => "message",
        _ => "message",
    }
}

fn buffer_key_for_event(event: &AgentEvent, stream_type: &str) -> String {
    if !event.message_id.is_empty() {
        event.message_id.clone()
    } else if !event.msg_id.is_empty() {
        event.msg_id.clone()
    } else {
        stream_type.to_string()
    }
}

/// Platform-specific outbound (streaming + final reply).
#[async_trait]
pub trait StreamSink: Send {
    async fn on_stream_start(&mut self, stream_type: &str) -> Result<()>;
    async fn on_stream_delta(&mut self, stream_type: &str, accumulated: &str) -> Result<()>;
    async fn on_stream_end(&mut self, stream_type: &str, accumulated: &str) -> Result<()>;
    async fn on_completed(&mut self, reply: ProcessedReply) -> Result<()>;
}

#[allow(clippy::too_many_arguments)]
pub async fn process_with_sink(
    agent: &SharedAgentBackend,
    acl: &AccessControl,
    language: &str,
    dm_acl: bool,
    group_acl: bool,
    streaming: bool,
    filter_tool: bool,
    filter_thinking: bool,
    native: NativeMessage,
    sink: &mut dyn StreamSink,
) -> Result<()> {
    let is_group = native
        .meta
        .get("is_group")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if acl.check_blocked_with_flags(
        &native.channel_id,
        &native.sender_id,
        dm_acl,
        group_acl,
        is_group,
    ) {
        let msg = acl.deny_message(language, &native.sender_id);
        let reply = ProcessedReply {
            text: msg.clone(),
            parts: vec![ChannelPart::text_part(msg)],
            meta: native.meta.clone(),
            session_id: native.session_id.clone().unwrap_or_else(|| {
                crate::queue::session_key(&native.channel_id, &native.sender_id)
            }),
            user_id: native.sender_id.clone(),
        };
        return sink.on_completed(reply).await;
    }

    let request = native_to_request(&native)?;
    let mut msg_id_to_type: HashMap<String, String> = HashMap::new();
    let mut buffers: HashMap<String, String> = HashMap::new();
    let mut pending_segments: Vec<String> = Vec::new();
    let mut pending_parts: Vec<ChannelPart> = Vec::new();

    let mut stream = agent.stream(request).await;
    while let Some(item) = stream.next().await {
        let event = item?;
        if !event.error_message.is_empty() {
            return Err(crate::error::GatewayError::Other(event.error_message));
        }
        if filter_tool && !event.tool_name.is_empty() {
            continue;
        }

        if streaming {
            let _ = dispatch_streaming_event(
                &event,
                sink,
                filter_thinking,
                &mut msg_id_to_type,
                &mut buffers,
            )
            .await?;
        }

        if event.kind == EventKind::Message && event.status == EventStatus::Completed {
            let text = if event.text.is_empty() {
                event
                    .content
                    .iter()
                    .filter(|p| !p.text.is_empty())
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            } else {
                event.text.clone()
            };
            if !text.is_empty() {
                pending_segments.push(text.clone());
            }
            if event.content.is_empty() {
                if !text.is_empty() {
                    pending_parts.push(ChannelPart::text_part(text));
                }
            } else {
                pending_parts.extend(event.content.clone());
            }
        }
    }

    if !pending_segments.is_empty() || !pending_parts.is_empty() {
        let text = pending_segments.join("\n\n");
        let parts = if pending_parts.is_empty() {
            vec![ChannelPart::text_part(&text)]
        } else {
            pending_parts
        };
        let reply = ProcessedReply {
            text,
            parts,
            meta: native.meta.clone(),
            session_id: native.session_id.clone().unwrap_or_else(|| {
                crate::queue::session_key(&native.channel_id, &native.sender_id)
            }),
            user_id: native.sender_id.clone(),
        };
        return sink.on_completed(reply).await;
    }

    let fallback = ProcessedReply {
        text: "(no response)".to_string(),
        parts: vec![ChannelPart::text_part("(no response)")],
        meta: native.meta,
        session_id: native
            .session_id
            .clone()
            .unwrap_or_else(|| crate::queue::session_key(&native.channel_id, &native.sender_id)),
        user_id: native.sender_id,
    };
    sink.on_completed(fallback).await
}

async fn dispatch_streaming_event(
    event: &AgentEvent,
    sink: &mut dyn StreamSink,
    filter_thinking: bool,
    msg_id_to_type: &mut HashMap<String, String>,
    buffers: &mut HashMap<String, String>,
) -> Result<bool> {
    let stream_type = resolve_stream_type(event);

    if event.kind == EventKind::Message && event.status == EventStatus::InProgress {
        if !STREAMABLE.contains(&stream_type) {
            return Ok(false);
        }
        let key = buffer_key_for_event(event, stream_type);
        if !event.message_id.is_empty() {
            msg_id_to_type.insert(event.message_id.clone(), stream_type.to_string());
        }
        if stream_type == "reasoning" && filter_thinking {
            return Ok(true);
        }
        buffers.insert(key, String::new());
        sink.on_stream_start(stream_type).await?;
        return Ok(true);
    }

    if event.kind == EventKind::Content && event.status == EventStatus::InProgress && event.delta {
        let st = if !event.msg_id.is_empty() {
            msg_id_to_type.get(&event.msg_id).cloned()
        } else {
            None
        };
        let Some(st) = st else {
            return Ok(false);
        };
        if !STREAMABLE.contains(&st.as_str()) {
            return Ok(false);
        }
        if st == "reasoning" && filter_thinking {
            return Ok(true);
        }
        let key = if !event.msg_id.is_empty() {
            event.msg_id.clone()
        } else {
            st.clone()
        };
        if !buffers.contains_key(&key) {
            return Ok(false);
        }
        let acc = buffers.entry(key).or_default();
        acc.push_str(&event.text);
        let accumulated = acc.clone();
        sink.on_stream_delta(&st, &accumulated).await?;
        return Ok(true);
    }

    if event.kind == EventKind::Message && event.status == EventStatus::Completed {
        let key = buffer_key_for_event(event, stream_type);
        if !event.message_id.is_empty() {
            msg_id_to_type.remove(&event.message_id);
        }
        if !STREAMABLE.contains(&stream_type) {
            return Ok(false);
        }
        let acc = buffers.remove(&key);
        let final_text = match acc {
            Some(acc) if !acc.is_empty() => acc,
            _ => event.text.clone(),
        };
        if final_text.is_empty() {
            return Ok(false);
        }
        if stream_type == "reasoning" && filter_thinking {
            return Ok(true);
        }
        sink.on_stream_end(stream_type, &final_text).await?;
        return Ok(true);
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::AccessControl;
    use crate::gateway::{AgentBackend, AgentEventStream, SharedAgentBackend};
    use crate::types::{AgentEvent, ChannelRequest, PartKind};
    use std::sync::Arc;
    use std::sync::Mutex;

    struct RecordingSink {
        events: Mutex<Vec<String>>,
        replies: Mutex<Vec<ProcessedReply>>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                replies: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn replies(&self) -> Vec<ProcessedReply> {
            self.replies.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl StreamSink for RecordingSink {
        async fn on_stream_start(&mut self, stream_type: &str) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{stream_type}"));
            Ok(())
        }

        async fn on_stream_delta(&mut self, stream_type: &str, accumulated: &str) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("delta:{stream_type}:{}", accumulated));
            Ok(())
        }

        async fn on_stream_end(&mut self, stream_type: &str, accumulated: &str) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("end:{stream_type}:{}", accumulated));
            Ok(())
        }

        async fn on_completed(&mut self, reply: ProcessedReply) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("completed:{}", reply.text));
            self.replies.lock().unwrap().push(reply);
            Ok(())
        }
    }

    struct StaticBackend {
        events: Vec<AgentEvent>,
    }

    #[async_trait]
    impl AgentBackend for StaticBackend {
        async fn stream(&self, _request: ChannelRequest) -> AgentEventStream {
            let events = self.events.clone();
            Box::pin(async_stream::stream! {
                for event in events {
                    yield Ok(event);
                }
            })
        }
    }

    #[tokio::test]
    async fn dispatches_multiple_message_buffers_by_id() {
        let mut sink = RecordingSink::new();
        let mut msg_id_to_type = HashMap::new();
        let mut buffers = HashMap::new();

        let start1 = AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::InProgress,
            message_id: "item_1".into(),
            ..Default::default()
        };
        assert!(dispatch_streaming_event(
            &start1,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        let delta1 = AgentEvent {
            kind: EventKind::Content,
            status: EventStatus::InProgress,
            text: "first".into(),
            msg_id: "item_1".into(),
            delta: true,
            ..Default::default()
        };
        assert!(dispatch_streaming_event(
            &delta1,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        let end1 = AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::Completed,
            text: "first".into(),
            message_id: "item_1".into(),
            ..Default::default()
        };
        assert!(dispatch_streaming_event(
            &end1,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        let start2 = AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::InProgress,
            message_id: "item_2".into(),
            ..Default::default()
        };
        assert!(dispatch_streaming_event(
            &start2,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        let delta2 = AgentEvent {
            kind: EventKind::Content,
            status: EventStatus::InProgress,
            text: "second".into(),
            msg_id: "item_2".into(),
            delta: true,
            ..Default::default()
        };
        assert!(dispatch_streaming_event(
            &delta2,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        let end2 = AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::Completed,
            text: "second".into(),
            message_id: "item_2".into(),
            ..Default::default()
        };
        assert!(dispatch_streaming_event(
            &end2,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        let events = sink.events();
        assert_eq!(events.iter().filter(|e| e.starts_with("start:")).count(), 2);
        assert_eq!(events.iter().filter(|e| e.starts_with("end:")).count(), 2);
        assert!(events.contains(&"end:message:first".to_string()));
        assert!(events.contains(&"end:message:second".to_string()));
    }

    #[tokio::test]
    async fn completed_message_without_active_buffer_still_streams_end() {
        let mut sink = RecordingSink::new();
        let mut msg_id_to_type = HashMap::new();
        let mut buffers = HashMap::new();

        let completed = AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::Completed,
            text: "done".into(),
            message_id: "item_1".into(),
            ..Default::default()
        };

        assert!(dispatch_streaming_event(
            &completed,
            &mut sink,
            false,
            &mut msg_id_to_type,
            &mut buffers
        )
        .await
        .unwrap());

        assert!(sink.events().contains(&"end:message:done".to_string()));
    }

    #[tokio::test]
    async fn process_with_sink_preserves_non_text_completed_parts() {
        let image = ChannelPart {
            kind: PartKind::Image,
            url: "file:///tmp/image.png".into(),
            ..Default::default()
        };
        let backend: SharedAgentBackend = Arc::new(StaticBackend {
            events: vec![AgentEvent {
                kind: EventKind::Message,
                status: EventStatus::Completed,
                text: "caption".into(),
                content: vec![ChannelPart::text_part("caption"), image.clone()],
                message_id: "item_1".into(),
                ..Default::default()
            }],
        });
        let temp = tempfile::tempdir().unwrap();
        let acl = AccessControl::load_or_create(temp.path()).unwrap();
        let native = NativeMessage {
            channel_id: "telegram".into(),
            sender_id: "user_1".into(),
            session_id: Some("telegram:user_1".into()),
            content_parts: vec![ChannelPart::text_part("prompt")],
            meta: HashMap::new(),
        };
        let mut sink = RecordingSink::new();

        process_with_sink(
            &backend, &acl, "en", false, false, true, false, false, native, &mut sink,
        )
        .await
        .unwrap();

        let replies = sink.replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, "caption");
        assert!(replies[0]
            .parts
            .iter()
            .any(|part| part.kind == PartKind::Image && part.url == image.url));
    }
}
