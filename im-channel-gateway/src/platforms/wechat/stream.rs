use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;

use crate::context::GatewayContext;
use crate::error::{GatewayError, Result};
use crate::pipeline::streaming::{process_with_sink, StreamSink};
use crate::pipeline::{NativeMessage, ProcessedReply};
use crate::platforms::wechat::adapter::{
    ContextTokenStore, SentEchoStore, WeChatAccountRunner, WeChatRuntimeStores,
};
use crate::platforms::wechat::client::{
    ILinkClient, WECHAT_SEND_INTERVAL, WECHAT_TEXT_CHUNK_CHARS,
};
use crate::text::split_text_chars;

pub fn wechat_streaming_enabled(ctx: &GatewayContext, cfg_enabled: bool) -> bool {
    cfg_enabled || ctx.config.agent.backend == "codex_app_server"
}

pub(crate) async fn process_wechat_message(
    ctx: &GatewayContext,
    client: Arc<ILinkClient>,
    native: NativeMessage,
    prefix: &str,
    streaming_enabled: bool,
    stores: WeChatRuntimeStores,
) -> Result<()> {
    let (dm_acl, group_acl) = (
        ctx.config.channels.wechat.base.access_control_dm,
        ctx.config.channels.wechat.base.access_control_group,
    );
    let filter_tool = ctx.config.channels.wechat.base.filter_tool_messages;
    let filter_thinking = ctx.config.channels.wechat.base.filter_thinking;
    let streaming = wechat_streaming_enabled(ctx, streaming_enabled);
    let typing_user = native
        .meta
        .get("reply_to_user_id")
        .and_then(|v| v.as_str())
        .or_else(|| native.meta.get("from_user_id").and_then(|v| v.as_str()))
        .unwrap_or(&native.sender_id)
        .to_string();
    let typing_ticket = native
        .meta
        .get("typing_ticket")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let typing_session_key = native
        .meta
        .get("typing_session_key")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let result = if streaming {
        let stub = ProcessedReply {
            text: String::new(),
            parts: vec![],
            meta: native.meta.clone(),
            session_id: native.session_id.clone().unwrap_or_else(|| {
                crate::queue::session_key(&native.channel_id, &native.sender_id)
            }),
            user_id: native.sender_id.clone(),
        };
        let mut sink = WeChatStreamSink::new(
            client.clone(),
            &stub,
            prefix,
            stores.sent_echoes.clone(),
            stores.context_tokens.clone(),
        );
        process_with_sink(
            &ctx.agent,
            &ctx.acl,
            &ctx.language,
            dm_acl,
            group_acl,
            true,
            filter_tool,
            filter_thinking,
            native,
            &mut sink,
        )
        .await
        .and_then(|_| {
            if let Some(summary) = sink.send_failure_summary() {
                Err(GatewayError::Channel {
                    channel: "wechat".into(),
                    message: summary,
                })
            } else {
                Ok(())
            }
        })
    } else {
        match crate::pipeline::process_native(
            &ctx.agent,
            &ctx.acl,
            &ctx.language,
            dm_acl,
            group_acl,
            native,
        )
        .await
        {
            Ok(reply) => {
                WeChatAccountRunner::send_reply(
                    &client,
                    &reply,
                    prefix,
                    Some(&stores.sent_echoes),
                    Some(&stores.context_tokens),
                )
                .await
            }
            Err(e) => Err(e),
        }
    };

    WeChatAccountRunner::finish_typing_session(
        &client,
        &stores.typing_tickets,
        &stores.typing_sessions,
        typing_session_key.as_deref(),
        &typing_user,
        typing_ticket.as_deref(),
    )
    .await;
    result
}

pub struct WeChatStreamSink {
    client: Arc<ILinkClient>,
    stub: ProcessedReply,
    prefix: String,
    sent_echoes: SentEchoStore,
    context_tokens: ContextTokenStore,
    seen_segments: HashSet<String>,
    pending_buffer: String,
    prefix_sent: bool,
    message_streamed: bool,
    failed_sends: u32,
    last_error: Option<String>,
}

impl WeChatStreamSink {
    pub(crate) fn new(
        client: Arc<ILinkClient>,
        stub: &ProcessedReply,
        prefix: &str,
        sent_echoes: SentEchoStore,
        context_tokens: ContextTokenStore,
    ) -> Self {
        Self {
            client,
            stub: stub.clone(),
            prefix: prefix.to_string(),
            sent_echoes,
            context_tokens,
            seen_segments: HashSet::new(),
            pending_buffer: String::new(),
            prefix_sent: false,
            message_streamed: false,
            failed_sends: 0,
            last_error: None,
        }
    }

    pub(crate) fn send_failure_summary(&self) -> Option<String> {
        if self.failed_sends == 0 {
            return None;
        }
        let detail = self
            .last_error
            .as_deref()
            .map(|e| format!("; last error: {e}"))
            .unwrap_or_default();
        Some(format!(
            "{} wechat segment(s) failed to send{detail}",
            self.failed_sends
        ))
    }

    /// Drain chunks once the buffer reaches the iLink conservative text limit.
    /// Chunks may be shorter than `max_chars` when `split_text_chars` finds a
    /// natural boundary near the limit.
    fn take_full_chunks(buffer: &mut String, max_chars: usize) -> Vec<String> {
        let mut out = Vec::new();
        while buffer.chars().count() >= max_chars {
            let chunks = split_text_chars(buffer, max_chars);
            out.push(chunks[0].clone());
            *buffer = if chunks.len() > 1 {
                chunks[1..].concat()
            } else {
                String::new()
            };
        }
        out
    }

    async fn append_segment(&mut self, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        if !self.seen_segments.insert(text.to_string()) {
            tracing::debug!(
                text_preview = %Self::preview_str(text),
                "wechat stream: skip duplicate segment"
            );
            return Ok(());
        }
        if !self.pending_buffer.is_empty() {
            self.pending_buffer.push_str("\n\n");
        }
        self.pending_buffer.push_str(text);
        tracing::debug!(
            pending_chars = self.pending_buffer.chars().count(),
            "wechat stream: buffered agent segment"
        );
        self.flush_full_chunks().await
    }

    async fn flush_full_chunks(&mut self) -> Result<()> {
        for chunk in Self::take_full_chunks(&mut self.pending_buffer, WECHAT_TEXT_CHUNK_CHARS) {
            self.dispatch_send(&chunk).await?;
        }
        Ok(())
    }

    async fn flush_remainder(&mut self) -> Result<()> {
        if self.pending_buffer.is_empty() {
            return Ok(());
        }
        let remainder = std::mem::take(&mut self.pending_buffer);
        self.dispatch_send(&remainder).await
    }

    async fn dispatch_send(&mut self, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        if self.message_streamed {
            tokio::time::sleep(WECHAT_SEND_INTERVAL).await;
        }
        let outbound_prefix = if self.prefix_sent {
            ""
        } else {
            self.prefix.as_str()
        };
        tracing::info!(
            text_len = text.len(),
            text_chars = text.chars().count(),
            text_preview = %Self::preview_str(text),
            "wechat stream: sending batched chunk"
        );
        let mut reply = self.stub.clone();
        reply.text = text.to_string();
        match WeChatAccountRunner::send_reply(
            &self.client,
            &reply,
            outbound_prefix,
            Some(&self.sent_echoes),
            Some(&self.context_tokens),
        )
        .await
        {
            Ok(()) => {
                self.prefix_sent = true;
                self.message_streamed = true;
            }
            Err(e) => {
                self.failed_sends += 1;
                self.last_error = Some(e.to_string());
                tracing::error!(
                    error = %e,
                    failed_sends = self.failed_sends,
                    text_len = text.len(),
                    text_preview = %Self::preview_str(text),
                    "wechat stream: send chunk failed; continuing with remaining content"
                );
            }
        }
        Ok(())
    }

    fn preview_str(text: &str) -> String {
        const MAX: usize = 200;
        if text.chars().count() <= MAX {
            return text.to_string();
        }
        let mut end = MAX;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    }
}

#[async_trait]
impl StreamSink for WeChatStreamSink {
    async fn on_stream_start(&mut self, _stream_type: &str) -> Result<()> {
        Ok(())
    }

    async fn on_stream_delta(&mut self, _stream_type: &str, _accumulated: &str) -> Result<()> {
        Ok(())
    }

    async fn on_stream_end(&mut self, _stream_type: &str, accumulated: &str) -> Result<()> {
        self.append_segment(accumulated).await
    }

    async fn on_completed(&mut self, reply: ProcessedReply) -> Result<()> {
        if !self.message_streamed && self.pending_buffer.is_empty() && !reply.text.trim().is_empty()
        {
            self.append_segment(&reply.text).await?;
        }
        self.flush_remainder().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_full_chunks_drains_only_when_at_limit() {
        let mut buffer = "a".repeat(1999);
        assert!(WeChatStreamSink::take_full_chunks(&mut buffer, 2000).is_empty());
        assert_eq!(buffer.chars().count(), 1999);

        buffer.push('b');
        let chunks = WeChatStreamSink::take_full_chunks(&mut buffer, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chars().count(), 2000);
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_full_chunks_leaves_remainder_under_limit() {
        let mut buffer = "x".repeat(2500);
        let chunks = WeChatStreamSink::take_full_chunks(&mut buffer, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chars().count(), 2000);
        assert_eq!(buffer.chars().count(), 500);
    }

    #[test]
    fn take_full_chunks_drains_multiple_full_slices() {
        let mut buffer = "y".repeat(4500);
        let chunks = WeChatStreamSink::take_full_chunks(&mut buffer, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chars().count(), 2000);
        assert_eq!(chunks[1].chars().count(), 2000);
        assert_eq!(buffer.chars().count(), 500);
    }
}
