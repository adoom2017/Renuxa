use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId, ParseMode, ThreadId};
use teloxide::RequestError;

use crate::platforms::telegram::format_html::{markdown_to_telegram_html, strip_markdown};

use crate::error::{GatewayError, Result};
use crate::pipeline::ProcessedReply;
use crate::platforms::telegram::media::{check_file_size, local_path_from_url};
use crate::text::split_text;
use crate::types::{ChannelPart, PartKind};

pub const SEND_CHUNK_SIZE: usize = 4000;
pub const MAX_MESSAGE_LENGTH: usize = 4096;
const STREAM_PLACEHOLDER: &str = "⏳";

pub struct TelegramSender {
    pub bot: Bot,
    pub chat_id: ChatId,
    pub thread_id: Option<ThreadId>,
    pub prefix: String,
}

impl TelegramSender {
    pub fn from_reply(bot: Bot, reply: &ProcessedReply, prefix: &str) -> Result<Self> {
        let chat_id = reply
            .meta
            .get("chat_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .map(ChatId)
            .ok_or_else(|| GatewayError::Other("telegram: missing chat_id".into()))?;
        let thread_id = reply
            .meta
            .get("message_thread_id")
            .and_then(|v| v.as_i64())
            .map(|id| ThreadId(MessageId(id as i32)));
        Ok(Self {
            bot,
            chat_id,
            thread_id,
            prefix: prefix.to_string(),
        })
    }

    fn input_file(url: &str) -> Result<InputFile> {
        if let Some(path) = local_path_from_url(url) {
            if !path.exists() {
                return Err(GatewayError::Channel {
                    channel: "telegram".into(),
                    message: format!("media not found: {}", path.display()),
                });
            }
            check_file_size(&path)?;
            return Ok(InputFile::file(path));
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            let parsed = ::url::Url::parse(url).map_err(|e| GatewayError::Other(e.to_string()))?;
            return Ok(InputFile::url(parsed));
        }
        Err(GatewayError::Channel {
            channel: "telegram".into(),
            message: format!("unsupported media url: {url}"),
        })
    }

    pub async fn send_text(&self, text: &str) -> Result<()> {
        let mut body = text.to_string();
        if !self.prefix.is_empty() {
            body = format!("{}  {}", self.prefix, body);
        }
        for chunk in split_text(&body, SEND_CHUNK_SIZE) {
            self.send_message_plain(&chunk).await?;
        }
        Ok(())
    }

    pub async fn send_message_plain(&self, text: &str) -> Result<()> {
        self.send_message_with_html(text, true).await
    }

    async fn send_plain(&self, text: &str) -> Result<()> {
        let mut req = self.bot.send_message(self.chat_id, text);
        if let Some(tid) = self.thread_id {
            req = req.message_thread_id(tid);
        }
        req.await.map_err(map_req_err)?;
        Ok(())
    }

    /// Send with Markdown→HTML; fallback to plain text on BadRequest (Python `send`).
    pub async fn send_message_with_html(&self, text: &str, use_html: bool) -> Result<()> {
        if !use_html {
            return self.send_plain(text).await;
        }
        let html = markdown_to_telegram_html(text);
        let mut req = self
            .bot
            .send_message(self.chat_id, html)
            .parse_mode(ParseMode::Html);
        if let Some(tid) = self.thread_id {
            req = req.message_thread_id(tid);
        }
        match req.await {
            Ok(_) => Ok(()),
            Err(RequestError::Api(e)) => {
                tracing::warn!("telegram HTML send failed, plain fallback: {e}");
                self.send_plain(&strip_markdown(text)).await
            }
            Err(e) => Err(map_req_err(e)),
        }
    }

    pub async fn send_placeholder(&self, stream_type: &str) -> Result<Option<MessageId>> {
        let prefix = if stream_type == "reasoning" {
            "💭 "
        } else {
            ""
        };
        let text = format!("{prefix}{STREAM_PLACEHOLDER}");
        let mut req = self.bot.send_message(self.chat_id, text);
        if let Some(tid) = self.thread_id {
            req = req.message_thread_id(tid);
        }
        match req.await {
            Ok(msg) => Ok(Some(msg.id)),
            Err(e) => {
                tracing::debug!("telegram placeholder failed: {e}");
                Ok(None)
            }
        }
    }

    pub async fn edit_stream_message(&self, message_id: MessageId, text: &str) -> Result<bool> {
        self.edit_stream_message_inner(message_id, text, false)
            .await
    }

    pub async fn edit_stream_message_final(
        &self,
        message_id: MessageId,
        text: &str,
    ) -> Result<bool> {
        self.edit_stream_message_inner(message_id, text, true).await
    }

    async fn edit_plain(&self, message_id: MessageId, body: &str) -> Result<bool> {
        match self
            .bot
            .edit_message_text(self.chat_id, message_id, body)
            .await
        {
            Ok(_) => Ok(true),
            Err(RequestError::Api(e)) if e.to_string().to_lowercase().contains("not modified") => {
                Ok(true)
            }
            Err(e) => {
                tracing::debug!("telegram edit failed: {e}");
                Ok(false)
            }
        }
    }

    async fn edit_stream_message_inner(
        &self,
        message_id: MessageId,
        text: &str,
        final_html: bool,
    ) -> Result<bool> {
        let body = if text.trim().is_empty() {
            STREAM_PLACEHOLDER.to_string()
        } else if final_html {
            markdown_to_telegram_html(text)
        } else {
            text.to_string()
        };

        if !final_html {
            return self.edit_plain(message_id, &body).await;
        }

        match self
            .bot
            .edit_message_text(self.chat_id, message_id, body)
            .parse_mode(ParseMode::Html)
            .await
        {
            Ok(_) => Ok(true),
            Err(RequestError::Api(e)) if e.to_string().to_lowercase().contains("not modified") => {
                Ok(true)
            }
            Err(RequestError::Api(e)) => {
                tracing::debug!("telegram HTML edit failed, plain: {e}");
                let plain = if text.trim().is_empty() {
                    STREAM_PLACEHOLDER.to_string()
                } else {
                    strip_markdown(text)
                };
                self.edit_plain(message_id, &plain).await
            }
            Err(e) => {
                tracing::debug!("telegram edit failed: {e}");
                Ok(false)
            }
        }
    }

    pub async fn delete_message(&self, message_id: MessageId) -> Result<()> {
        let _ = self.bot.delete_message(self.chat_id, message_id).await;
        Ok(())
    }

    pub async fn send_media_part(&self, part: &ChannelPart) -> Result<()> {
        if part.url.is_empty() {
            return Ok(());
        }
        let file = Self::input_file(&part.url)?;
        match part.kind {
            PartKind::Image => {
                let mut req = self.bot.send_photo(self.chat_id, file);
                if let Some(tid) = self.thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map_err(map_req_err)?;
            }
            PartKind::Video => {
                let mut req = self.bot.send_video(self.chat_id, file);
                if let Some(tid) = self.thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map_err(map_req_err)?;
            }
            PartKind::Audio => {
                let mut req = self.bot.send_audio(self.chat_id, file);
                if let Some(tid) = self.thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map_err(map_req_err)?;
            }
            PartKind::File => {
                let mut req = self.bot.send_document(self.chat_id, file);
                if let Some(tid) = self.thread_id {
                    req = req.message_thread_id(tid);
                }
                req.await.map_err(map_req_err)?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn map_req_err(e: RequestError) -> GatewayError {
    GatewayError::Channel {
        channel: "telegram".into(),
        message: e.to_string(),
    }
}

pub async fn send_reply(bot: Bot, reply: &ProcessedReply, prefix: &str) -> Result<()> {
    let sender = TelegramSender::from_reply(bot, reply, prefix)?;
    let mut sent_text = false;
    for part in &reply.parts {
        if part.kind == PartKind::Text && !part.text.is_empty() {
            sender.send_text(&part.text).await?;
            sent_text = true;
        }
    }
    if !sent_text && !reply.text.is_empty() {
        sender.send_text(&reply.text).await?;
    }
    for part in &reply.parts {
        if part.kind != PartKind::Text {
            sender.send_media_part(part).await?;
        }
    }
    Ok(())
}
