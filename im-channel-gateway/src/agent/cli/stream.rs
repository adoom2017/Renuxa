use crate::types::{AgentEvent, ChannelRequest};

/// Extract plain text from a channel request (first text parts joined).
pub fn request_text(request: &ChannelRequest) -> String {
    request
        .content
        .iter()
        .map(|p| p.text.as_str())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn help_events(text: impl Into<String>) -> Vec<AgentEvent> {
    let text = text.into();
    vec![
        AgentEvent::assistant_text(text),
        AgentEvent::response_completed(),
    ]
}

pub fn error_events(message: impl Into<String>) -> Vec<AgentEvent> {
    vec![
        AgentEvent::assistant_text(message),
        AgentEvent::response_completed(),
    ]
}
