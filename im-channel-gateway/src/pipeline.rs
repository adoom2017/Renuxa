use std::collections::HashMap;

use futures::StreamExt;
use serde_json::Value;

use crate::acl::AccessControl;
use crate::error::{GatewayError, Result};
use crate::gateway::SharedAgentBackend;
use crate::queue::{priority_for_query, session_key};
use crate::types::{ChannelPart, ChannelRequest, EventKind, EventStatus, PartKind};

pub mod streaming;

#[derive(Clone)]
pub struct NativeMessage {
    pub channel_id: String,
    pub sender_id: String,
    pub session_id: Option<String>,
    pub content_parts: Vec<ChannelPart>,
    pub meta: HashMap<String, Value>,
}

pub async fn process_native(
    agent: &SharedAgentBackend,
    acl: &AccessControl,
    language: &str,
    dm_acl: bool,
    group_acl: bool,
    native: NativeMessage,
) -> Result<ProcessedReply> {
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
        return Ok(ProcessedReply {
            text: msg.clone(),
            parts: vec![ChannelPart::text_part(msg)],
            meta: native.meta,
            session_id: native
                .session_id
                .clone()
                .unwrap_or_else(|| session_key(&native.channel_id, &native.sender_id)),
            user_id: native.sender_id.clone(),
        });
    }

    let request = native_to_request(&native)?;
    let session_id = request.session_id.clone();
    let query_preview = extract_query(&native);
    tracing::info!(
        channel = %native.channel_id,
        user_id = %native.sender_id,
        session_id = %session_id,
        query_len = query_preview.len(),
        "pipeline: dispatching to agent"
    );

    let mut last_text = String::new();
    let mut parts: Vec<ChannelPart> = Vec::new();
    let mut event_count: u32 = 0;

    let mut stream = agent.stream(request).await;
    while let Some(item) = stream.next().await {
        let event = item?;
        event_count += 1;
        tracing::debug!(
            channel = %native.channel_id,
            event_no = event_count,
            kind = ?event.kind,
            status = ?event.status,
            text_len = event.text.len(),
            has_content = !event.content.is_empty(),
            error_message = %event.error_message,
            "pipeline: agent event"
        );
        if !event.error_message.is_empty() {
            tracing::warn!(
                channel = %native.channel_id,
                user_id = %native.sender_id,
                error = %event.error_message,
                "pipeline: agent returned error_message"
            );
            return Err(GatewayError::Other(event.error_message));
        }
        if event.kind == EventKind::Message
            && matches!(event.status, EventStatus::Completed | EventStatus::Failed)
        {
            if !event.text.is_empty() {
                last_text = event.text.clone();
            }
            if !event.content.is_empty() {
                parts = event.content;
            }
        }
    }

    if last_text.is_empty() && parts.is_empty() {
        tracing::warn!(
            channel = %native.channel_id,
            user_id = %native.sender_id,
            session_id = %session_id,
            event_count,
            "pipeline: no usable agent text (reply will be '(no response)')"
        );
        if native.channel_id.starts_with("wechat:") {
            return Err(GatewayError::Other("incomplete backend response".into()));
        }
        last_text = "(no response)".to_string();
        parts = vec![ChannelPart::text_part(&last_text)];
    } else if parts.is_empty() {
        parts = vec![ChannelPart::text_part(&last_text)];
    }

    tracing::info!(
        channel = %native.channel_id,
        user_id = %native.sender_id,
        reply_len = last_text.len(),
        event_count,
        "pipeline: reply ready"
    );

    Ok(ProcessedReply {
        text: last_text,
        parts,
        meta: native.meta,
        session_id: native
            .session_id
            .clone()
            .unwrap_or_else(|| session_key(&native.channel_id, &native.sender_id)),
        user_id: native.sender_id,
    })
}

#[derive(Clone)]
pub struct ProcessedReply {
    pub text: String,
    pub parts: Vec<ChannelPart>,
    pub meta: HashMap<String, Value>,
    pub session_id: String,
    pub user_id: String,
}

pub(crate) fn native_to_request(native: &NativeMessage) -> Result<ChannelRequest> {
    Ok(ChannelRequest {
        channel: native.channel_id.clone(),
        session_id: native
            .session_id
            .clone()
            .unwrap_or_else(|| session_key(&native.channel_id, &native.sender_id)),
        user_id: native.sender_id.clone(),
        content: native.content_parts.clone(),
        meta: native.meta.clone(),
    })
}

pub fn extract_query(native: &NativeMessage) -> String {
    native
        .content_parts
        .iter()
        .find(|p| p.kind == PartKind::Text)
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

pub fn native_from_json(channel_id: &str, payload: Value) -> Result<NativeMessage> {
    let obj = payload
        .as_object()
        .ok_or_else(|| GatewayError::Other("native payload must be a JSON object".into()))?;
    let sender_id = obj
        .get("sender_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let session_id = obj
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let meta: HashMap<String, Value> = obj
        .get("meta")
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let mut content_parts = Vec::new();
    if let Some(parts) = obj.get("content_parts").and_then(|v| v.as_array()) {
        for p in parts {
            let t = p.get("type").and_then(|v| v.as_str()).unwrap_or("text");
            let text = p
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            content_parts.push(match t {
                "image" => ChannelPart {
                    kind: PartKind::Image,
                    text,
                    url: p
                        .get("image_url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    data: p.get("data").cloned(),
                    extra: p
                        .get("extra")
                        .and_then(|v| v.as_object())
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                },
                "video" => ChannelPart {
                    kind: PartKind::Video,
                    text,
                    url: p
                        .get("video_url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    data: p.get("data").cloned(),
                    extra: p
                        .get("extra")
                        .and_then(|v| v.as_object())
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                },
                "file" => ChannelPart {
                    kind: PartKind::File,
                    text,
                    url: p
                        .get("file_url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    data: p.get("data").cloned(),
                    extra: p
                        .get("extra")
                        .and_then(|v| v.as_object())
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                },
                "audio" => ChannelPart {
                    kind: PartKind::Audio,
                    text,
                    url: p
                        .get("audio_url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    data: p.get("data").cloned(),
                    extra: p
                        .get("extra")
                        .and_then(|v| v.as_object())
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default(),
                },
                _ => ChannelPart::text_part(text),
            });
        }
    }

    Ok(NativeMessage {
        channel_id: channel_id.to_string(),
        sender_id,
        session_id,
        content_parts,
        meta,
    })
}

pub fn message_priority(native: &NativeMessage) -> i32 {
    priority_for_query(&extract_query(native))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_from_json_preserves_non_text_part_text_and_metadata() {
        let native = native_from_json(
            "wechat",
            json!({
                "sender_id": "wx_user",
                "session_id": "wechat:wx_user",
                "content_parts": [{
                    "type": "image",
                    "text": "[WeChat image message]",
                    "data": {"type": 2},
                    "extra": {"wechat_item_label": "image"}
                }],
                "meta": {}
            }),
        )
        .unwrap();

        assert_eq!(native.content_parts[0].kind, PartKind::Image);
        assert_eq!(native.content_parts[0].text, "[WeChat image message]");
        assert_eq!(
            native.content_parts[0]
                .extra
                .get("wechat_item_label")
                .and_then(|v| v.as_str()),
            Some("image")
        );
    }
}
