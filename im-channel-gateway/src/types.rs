use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    #[default]
    Text,
    Image,
    Video,
    Audio,
    File,
    Refusal,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    #[default]
    Message,
    Content,
    Response,
    Reasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    #[default]
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelPart {
    pub kind: PartKind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub extra: HashMap<String, Value>,
}

impl ChannelPart {
    pub fn text_part(text: impl Into<String>) -> Self {
        Self {
            kind: PartKind::Text,
            text: text.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRequest {
    pub channel: String,
    pub session_id: String,
    pub user_id: String,
    pub content: Vec<ChannelPart>,
    #[serde(default)]
    pub meta: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentEvent {
    pub kind: EventKind,
    pub status: EventStatus,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub content: Vec<ChannelPart>,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub msg_id: String,
    #[serde(default)]
    pub delta: bool,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_output: Option<Value>,
    #[serde(default)]
    pub error_message: String,
    #[serde(default)]
    pub meta: HashMap<String, Value>,
    #[serde(default)]
    pub raw: Option<Value>,
}

impl AgentEvent {
    pub fn assistant_text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            kind: EventKind::Message,
            status: EventStatus::Completed,
            text: text.clone(),
            content: vec![ChannelPart::text_part(text)],
            message_id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        }
    }

    pub fn response_completed() -> Self {
        Self {
            kind: EventKind::Response,
            status: EventStatus::Completed,
            ..Default::default()
        }
    }
}
