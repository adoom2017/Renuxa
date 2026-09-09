use serde_json::{json, Value};

use crate::types::{AgentEvent, ChannelPart, ChannelRequest, EventKind, EventStatus, PartKind};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwards_identity_without_reply_tokens_or_media_keys() {
        let request = ChannelRequest {
            channel: "wechat:bot".into(),
            session_id: "session".into(),
            user_id: "sender".into(),
            content: vec![ChannelPart::text_part("hello")],
            meta: std::collections::HashMap::from([
                ("message_id".into(), json!("id")),
                ("account_id".into(), json!("bot")),
                ("is_group".into(), json!(false)),
                ("context_token".into(), json!("secret")),
                ("aes_key".into(), json!("secret")),
            ]),
        };
        let body = build_process_request_body(&request);
        assert_eq!(body["meta"]["message_id"], "id");
        assert_eq!(body["meta"]["is_group"], false);
        assert!(!body.to_string().contains("secret"));
        let event=agent_event_from_runtime_json(&json!({"object":"message","status":"completed","message":{"content":[{"type":"text","text":"已添加"}]}})).unwrap();
        assert_eq!(event.text, "已添加");
    }
}

pub fn build_process_request_body(request: &ChannelRequest) -> Value {
    let content: Vec<Value> = request
        .content
        .iter()
        .map(|part| match part.kind {
            PartKind::Text | PartKind::Refusal => {
                json!({"type": "text", "text": part.text})
            }
            PartKind::Image => json!({"type": "image", "image_url": part.url}),
            PartKind::Video => json!({"type": "video", "video_url": part.url}),
            PartKind::Audio => json!({"type": "audio", "data": part.data}),
            PartKind::File => json!({
                "type": "file",
                "file_url": part.url,
                "file_id": part.extra.get("file_id"),
            }),
            PartKind::Data => json!({"type": "data", "data": part.data}),
        })
        .collect();

    let input_content = if content.is_empty() {
        vec![json!({"type": "text", "text": " "})]
    } else {
        content
    };

    json!({
        "input": [{
            "role": "user",
            "content": input_content,
        }],
        "session_id": request.session_id,
        "user_id": request.user_id,
        "channel": request.channel,
        "meta": {
            "message_id": request.meta.get("message_id"),
            "account_id": request.meta.get("account_id"),
            "is_group": request.meta.get("is_group"),
        },
    })
}

fn status_from_str(s: &str) -> EventStatus {
    let lower = s.to_lowercase();
    if lower.contains("in_progress") || lower == "inprogress" {
        return EventStatus::InProgress;
    }
    if lower.contains("fail") {
        return EventStatus::Failed;
    }
    EventStatus::Completed
}

fn parts_from_output(output: &Value) -> Vec<ChannelPart> {
    let mut parts = Vec::new();
    let Some(messages) = output.as_array() else {
        return parts;
    };
    for msg in messages {
        let Some(content) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                let text = block
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !text.is_empty() {
                    parts.push(ChannelPart::text_part(text));
                }
            }
        }
    }
    parts
}

fn message_type_str(value: &Value) -> String {
    value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("message")
        .to_string()
}

pub fn agent_event_from_runtime_json(value: &Value) -> Option<AgentEvent> {
    let object = value.get("object").and_then(|v| v.as_str()).unwrap_or("");
    let status = status_from_str(
        value
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("completed"),
    );

    if object == "content" {
        let text = value
            .get("text")
            .and_then(|v| v.as_str())
            .or_else(|| {
                value
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();
        return Some(AgentEvent {
            kind: EventKind::Content,
            status,
            text,
            msg_id: value
                .get("msg_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            delta: value.get("delta").and_then(|v| v.as_bool()).unwrap_or(true),
            raw: Some(value.clone()),
            ..Default::default()
        });
    }

    if object == "response" {
        if status == EventStatus::Failed {
            let err = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .or_else(|| value.get("error").and_then(|e| e.as_str()))
                .unwrap_or("unknown error");
            return Some(AgentEvent {
                kind: EventKind::Response,
                status: EventStatus::Failed,
                error_message: err.to_string(),
                raw: Some(value.clone()),
                ..Default::default()
            });
        }
        if let Some(output) = value.get("output") {
            let parts = parts_from_output(output);
            let text = parts
                .iter()
                .map(|p| p.text.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("");
            if !text.is_empty() || status == EventStatus::Completed {
                return Some(AgentEvent {
                    kind: EventKind::Message,
                    status,
                    text: text.clone(),
                    content: parts,
                    message_id: value
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    raw: Some(value.clone()),
                    ..Default::default()
                });
            }
        }
        if status == EventStatus::Completed {
            return Some(AgentEvent {
                kind: EventKind::Response,
                status: EventStatus::Completed,
                raw: Some(value.clone()),
                ..Default::default()
            });
        }
        return None;
    }

    if object == "message" {
        let msg = value.get("message")?;
        let parts: Vec<ChannelPart> = msg
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                            Some(ChannelPart::text_part(
                                block.get("text").and_then(|v| v.as_str()).unwrap_or(""),
                            ))
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let text = parts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("");
        let type_str = message_type_str(value);
        let kind = if type_str == "reasoning" {
            EventKind::Reasoning
        } else {
            EventKind::Message
        };
        return Some(AgentEvent {
            kind,
            status,
            text,
            content: parts,
            message_id: value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            delta: value
                .get("delta")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            raw: Some(value.clone()),
            ..Default::default()
        });
    }

    None
}
