use std::collections::HashMap;
use std::path::Path;

use regex::Regex;
use serde_json::{json, Value};
use teloxide::prelude::*;
use teloxide::types::{Message, MessageEntityKind};

use crate::pipeline::NativeMessage;
use crate::platforms::telegram::media::download_telegram_file;
use crate::types::{ChannelPart, PartKind};

pub struct BotIdentity {
    pub id: u64,
    pub username: String,
}

pub async fn fetch_bot_identity(bot: &Bot) -> Result<BotIdentity, teloxide::RequestError> {
    let me = bot.get_me().await?;
    Ok(BotIdentity {
        id: me.id.0 as u64,
        username: me.username.clone().unwrap_or_default(),
    })
}

pub fn check_group_mention(
    require_mention: bool,
    is_group: bool,
    meta: &HashMap<String, Value>,
) -> bool {
    if !is_group || !require_mention {
        return true;
    }
    meta.get("bot_mentioned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || meta
            .get("has_bot_command")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

/// Build native message from Telegram update (text + media).
pub async fn build_native_from_message(
    bot: &Bot,
    msg: &Message,
    media_dir: &Path,
    identity: &BotIdentity,
    account_id: &str,
    channel_id: &str,
) -> Option<NativeMessage> {
    let chat = &msg.chat;
    let chat_id = chat.id.to_string();
    let user_id = msg
        .from
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_else(|| chat_id.clone());
    let is_group = chat.is_group() || chat.is_supergroup() || chat.is_channel();

    let (mut text, has_bot_command, bot_mentioned) = extract_text_and_mentions(msg, identity);

    if bot_mentioned && !identity.username.is_empty() && !text.is_empty() {
        let pat = format!(r"(?i)@{}", regex::escape(&identity.username));
        if let Ok(re) = Regex::new(&pat) {
            text = re.replace_all(&text, "").trim().to_string();
        }
    }

    let mut content_parts: Vec<ChannelPart> = Vec::new();
    if !text.is_empty() {
        content_parts.push(ChannelPart::text_part(text));
    }

    if let Some(photos) = msg.photo() {
        if let Some(largest) = photos.last() {
            if let Some(uri) =
                download_telegram_file(bot, largest.file.id.as_str(), media_dir, "photo.jpg")
                    .await
                    .ok()
                    .flatten()
            {
                content_parts.push(ChannelPart {
                    kind: PartKind::Image,
                    url: uri,
                    ..Default::default()
                });
            }
        }
    }

    if let Some(doc) = msg.document() {
        push_media_part(
            bot,
            media_dir,
            doc.file.id.as_str(),
            doc.file_name.as_deref().unwrap_or("document"),
            PartKind::File,
            &mut content_parts,
        )
        .await;
    }
    if let Some(video) = msg.video() {
        push_media_part(
            bot,
            media_dir,
            video.file.id.as_str(),
            "video.mp4",
            PartKind::Video,
            &mut content_parts,
        )
        .await;
    }
    if let Some(voice) = msg.voice() {
        push_media_part(
            bot,
            media_dir,
            voice.file.id.as_str(),
            "voice.ogg",
            PartKind::Audio,
            &mut content_parts,
        )
        .await;
    }
    if let Some(audio) = msg.audio() {
        push_media_part(
            bot,
            media_dir,
            audio.file.id.as_str(),
            audio.file_name.as_deref().unwrap_or("audio.mp3"),
            PartKind::Audio,
            &mut content_parts,
        )
        .await;
    }

    if content_parts.is_empty() {
        return None;
    }

    let mut meta: HashMap<String, Value> = HashMap::new();
    meta.insert("account_id".into(), json!(account_id));
    meta.insert("bot_user_id".into(), json!(identity.id.to_string()));
    meta.insert("bot_username".into(), json!(identity.username.clone()));
    meta.insert("chat_id".into(), json!(chat_id));
    meta.insert("user_id".into(), json!(user_id));
    meta.insert(
        "username".into(),
        json!(msg
            .from
            .as_ref()
            .and_then(|u| u.username.clone())
            .unwrap_or_default()),
    );
    meta.insert("is_group".into(), json!(is_group));
    meta.insert("message_id".into(), json!(msg.id.0));
    meta.insert("has_bot_command".into(), json!(has_bot_command));
    meta.insert("bot_mentioned".into(), json!(bot_mentioned));
    if let Some(thread) = msg.thread_id {
        meta.insert("message_thread_id".into(), json!(thread.0));
    }

    Some(NativeMessage {
        channel_id: channel_id.to_string(),
        sender_id: user_id,
        session_id: Some(format!("telegram:{account_id}:chat:{chat_id}")),
        content_parts,
        meta,
    })
}

async fn push_media_part(
    bot: &Bot,
    media_dir: &Path,
    file_id: &str,
    hint: &str,
    kind: PartKind,
    parts: &mut Vec<ChannelPart>,
) {
    if let Ok(Some(uri)) = download_telegram_file(bot, file_id, media_dir, hint).await {
        parts.push(ChannelPart {
            kind,
            url: uri,
            ..Default::default()
        });
    }
}

fn extract_text_and_mentions(msg: &Message, identity: &BotIdentity) -> (String, bool, bool) {
    let text = msg
        .text()
        .or_else(|| msg.caption())
        .unwrap_or("")
        .to_string();
    let entities = msg
        .entities()
        .or_else(|| msg.caption_entities())
        .unwrap_or_default();

    let mut has_bot_command = false;
    let mut bot_mentioned = false;
    for ent in entities {
        match &ent.kind {
            MessageEntityKind::BotCommand => has_bot_command = true,
            MessageEntityKind::Mention if !identity.username.is_empty() => {
                let start = ent.offset;
                let end = ent.offset + ent.length;
                if text
                    .get(start..end)
                    .map(|m| m.eq_ignore_ascii_case(&format!("@{}", identity.username)))
                    == Some(true)
                {
                    bot_mentioned = true;
                }
            }
            MessageEntityKind::TextMention { user } if user.id.0 == identity.id => {
                bot_mentioned = true;
            }
            _ => {}
        }
    }
    (text.trim().to_string(), has_bot_command, bot_mentioned)
}
