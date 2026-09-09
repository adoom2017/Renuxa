use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use teloxide::prelude::*;
use teloxide::types::MessageId;

use crate::error::Result;
use crate::pipeline::streaming::{process_with_sink, StreamSink};
use crate::pipeline::{NativeMessage, ProcessedReply};
use crate::platforms::telegram::send::{
    send_reply, TelegramSender, MAX_MESSAGE_LENGTH, SEND_CHUNK_SIZE,
};

const STREAM_EDIT_INTERVAL: Duration = Duration::from_millis(1500);

pub struct TelegramStreamSink {
    sender: TelegramSender,
    stream_enabled: bool,
    message_ids: HashMap<String, MessageId>,
    last_edit: HashMap<String, Instant>,
    buffers: HashMap<String, String>,
    message_streamed: bool,
}

impl TelegramStreamSink {
    pub fn new(
        bot: Bot,
        reply: &ProcessedReply,
        prefix: &str,
        stream_enabled: bool,
    ) -> Result<Self> {
        Ok(Self {
            sender: TelegramSender::from_reply(bot, reply, prefix)?,
            stream_enabled,
            message_ids: HashMap::new(),
            last_edit: HashMap::new(),
            buffers: HashMap::new(),
            message_streamed: false,
        })
    }
}

#[async_trait]
impl StreamSink for TelegramStreamSink {
    async fn on_stream_start(&mut self, stream_type: &str) -> Result<()> {
        if !self.stream_enabled {
            return Ok(());
        }
        if let Some(id) = self.sender.send_placeholder(stream_type).await? {
            self.message_ids.insert(stream_type.to_string(), id);
            self.last_edit
                .insert(stream_type.to_string(), Instant::now());
            self.buffers.insert(stream_type.to_string(), String::new());
        }
        Ok(())
    }

    async fn on_stream_delta(&mut self, stream_type: &str, accumulated: &str) -> Result<()> {
        if !self.stream_enabled {
            return Ok(());
        }
        let Some(msg_id) = self.message_ids.get(stream_type) else {
            return Ok(());
        };
        let now = Instant::now();
        if let Some(last) = self.last_edit.get(stream_type) {
            if now.duration_since(*last) < STREAM_EDIT_INTERVAL {
                return Ok(());
            }
        }
        let prefix = if stream_type == "reasoning" {
            "💭 "
        } else {
            ""
        };
        let mut display = if prefix.is_empty() {
            accumulated.to_string()
        } else {
            format!("{prefix}{accumulated}")
        };
        if display.len() > MAX_MESSAGE_LENGTH {
            display = format!(
                "...{}",
                &display[display.len().saturating_sub(MAX_MESSAGE_LENGTH - 4)..]
            );
        }
        if self.sender.edit_stream_message(*msg_id, &display).await? {
            self.last_edit.insert(stream_type.to_string(), now);
        }
        Ok(())
    }

    async fn on_stream_end(&mut self, stream_type: &str, accumulated: &str) -> Result<()> {
        if !self.stream_enabled {
            return Ok(());
        }
        let msg_id = self.message_ids.remove(stream_type);
        self.last_edit.remove(stream_type);
        self.buffers.remove(stream_type);

        let prefix = if stream_type == "reasoning" {
            "💭 "
        } else {
            ""
        };
        let final_text = if prefix.is_empty() {
            accumulated.to_string()
        } else {
            format!("{prefix}{accumulated}")
        };

        let Some(msg_id) = msg_id else {
            self.sender.send_text(&final_text).await?;
            return Ok(());
        };

        if stream_type == "message" {
            self.message_streamed = true;
        }

        if final_text.len() <= SEND_CHUNK_SIZE {
            let _ = self
                .sender
                .edit_stream_message_final(msg_id, &final_text)
                .await?;
            return Ok(());
        }

        self.sender.delete_message(msg_id).await?;
        self.sender.send_text(&final_text).await?;
        Ok(())
    }

    async fn on_completed(&mut self, reply: ProcessedReply) -> Result<()> {
        if !self.message_streamed && !reply.text.is_empty() {
            self.sender.send_text(&reply.text).await?;
        }
        for part in &reply.parts {
            if part.kind != crate::types::PartKind::Text {
                self.sender.send_media_part(part).await?;
            }
        }
        Ok(())
    }
}

pub async fn process_telegram_message(
    ctx: &crate::context::GatewayContext,
    bot: Bot,
    native: NativeMessage,
    prefix: &str,
    streaming_enabled: bool,
    filter_tool: bool,
    filter_thinking: bool,
) -> Result<()> {
    let (dm_acl, group_acl) = match native.channel_id.as_str() {
        "telegram" => (
            ctx.config.channels.telegram.base.access_control_dm,
            ctx.config.channels.telegram.base.access_control_group,
        ),
        id if id.starts_with("telegram:") => (
            ctx.config.channels.telegram.base.access_control_dm,
            ctx.config.channels.telegram.base.access_control_group,
        ),
        _ => (false, false),
    };

    if streaming_enabled {
        let stub = ProcessedReply {
            text: String::new(),
            parts: vec![],
            meta: native.meta.clone(),
            session_id: native.session_id.clone().unwrap_or_else(|| {
                crate::queue::session_key(&native.channel_id, &native.sender_id)
            }),
            user_id: native.sender_id.clone(),
        };
        let mut sink = TelegramStreamSink::new(bot.clone(), &stub, prefix, true)?;
        process_with_sink(
            &ctx.agent,
            &ctx.acl,
            &ctx.language,
            dm_acl,
            group_acl,
            streaming_enabled,
            filter_tool,
            filter_thinking,
            native,
            &mut sink,
        )
        .await
    } else {
        let reply = crate::pipeline::process_native(
            &ctx.agent,
            &ctx.acl,
            &ctx.language,
            dm_acl,
            group_acl,
            native,
        )
        .await?;
        send_reply(bot, &reply, prefix).await
    }
}
