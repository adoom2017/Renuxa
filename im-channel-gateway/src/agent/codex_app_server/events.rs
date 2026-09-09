use std::collections::HashMap;

use serde_json::Value;

use crate::types::{AgentEvent, EventKind, EventStatus};

const LOG_PREVIEW_CHARS: usize = 500;

/// Maps Codex app-server JSON-RPC notifications to gateway `AgentEvent`s.
/// IM delivery uses `item/completed` agentMessage text only; deltas are ignored.
pub struct TurnEventMapper {
    item_text: HashMap<String, String>,
    /// Combined agent text across all completed items (used as turn fallback).
    pub accumulated: String,
    turn_done: bool,
    any_delivered: bool,
}

impl TurnEventMapper {
    pub fn new() -> Self {
        Self {
            item_text: HashMap::new(),
            accumulated: String::new(),
            turn_done: false,
            any_delivered: false,
        }
    }

    pub fn is_turn_done(&self) -> bool {
        self.turn_done
    }

    /// Whether any agentMessage was delivered via item/completed (vs. a silent turn).
    pub fn emitted_any(&self) -> bool {
        self.any_delivered || !self.accumulated.is_empty()
    }

    pub fn push(&mut self, msg: &Value) -> Vec<AgentEvent> {
        if msg.get("id").is_some() {
            tracing::trace!(
                id = ?msg.get("id"),
                "codex app-server: skip json-rpc response in mapper"
            );
            return Vec::new();
        }
        let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
            tracing::debug!(
                raw = %preview_json(msg),
                "codex app-server: skip frame without method"
            );
            return Vec::new();
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        if !is_delta_notification(method) {
            log_raw_notification(method, &params);
        }

        let events = match method {
            "item/started" => self.on_item_started(&params),
            "item/agentMessage/delta" => self.on_agent_delta(&params),
            "item/reasoning/textDelta" => self.on_reasoning_delta(&params),
            "item/completed" => self.on_item_completed(&params),
            "turn/completed" => self.on_turn_completed(),
            other => {
                tracing::info!(
                    method = other,
                    params = %preview_json(&params),
                    "codex app-server: unmapped notification"
                );
                Vec::new()
            }
        };

        if events.is_empty() {
            tracing::debug!(
                method,
                "codex app-server: notification produced no AgentEvent"
            );
        } else {
            for (idx, ev) in events.iter().enumerate() {
                tracing::info!(
                    method,
                    event_no = idx + 1,
                    event_total = events.len(),
                    kind = ?ev.kind,
                    status = ?ev.status,
                    message_id = %ev.message_id,
                    text_len = ev.text.len(),
                    text_preview = %preview_str(&ev.text),
                    "codex app-server: mapped AgentEvent"
                );
            }
        }

        events
    }

    fn on_item_started(&mut self, params: &Value) -> Vec<AgentEvent> {
        let Some(item) = params.get("item") else {
            return Vec::new();
        };
        if item_type(item) != Some("agentMessage") {
            return Vec::new();
        }
        let item_id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if item_id.is_empty() {
            return Vec::new();
        }
        self.item_text.entry(item_id).or_default().clear();
        Vec::new()
    }

    fn on_agent_delta(&mut self, params: &Value) -> Vec<AgentEvent> {
        let delta = params.get("delta").and_then(|v| v.as_str()).unwrap_or("");
        if delta.is_empty() {
            return Vec::new();
        }
        Vec::new()
    }

    fn on_reasoning_delta(&mut self, _params: &Value) -> Vec<AgentEvent> {
        Vec::new()
    }

    fn on_item_completed(&mut self, params: &Value) -> Vec<AgentEvent> {
        let Some(item) = params.get("item") else {
            return Vec::new();
        };
        if item_type(item) != Some("agentMessage") {
            return Vec::new();
        }
        let message_id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if message_id.is_empty() {
            return Vec::new();
        }

        let text = agent_message_text(item).unwrap_or_default();
        if text.is_empty() {
            return Vec::new();
        }

        self.item_text.insert(message_id.clone(), text.clone());
        if !self.accumulated.is_empty() {
            self.accumulated.push_str("\n\n");
        }
        self.accumulated.push_str(&text);
        self.any_delivered = true;

        vec![AgentEvent {
            kind: EventKind::Message,
            status: EventStatus::Completed,
            text,
            message_id,
            ..Default::default()
        }]
    }

    fn on_turn_completed(&mut self) -> Vec<AgentEvent> {
        self.turn_done = true;
        let mut out = Vec::new();
        if !self.accumulated.is_empty() && !self.any_delivered {
            out.push(AgentEvent::assistant_text(self.accumulated.clone()));
        }
        out.push(AgentEvent::response_completed());
        out
    }
}

fn item_type(item: &Value) -> Option<&str> {
    item.get("type").and_then(|v| v.as_str())
}

fn log_raw_notification(method: &str, params: &Value) {
    let item_type = params
        .pointer("/item/type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let item_id = params
        .pointer("/item/id")
        .or_else(|| params.get("itemId"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let item_text = params
        .pointer("/item/text")
        .and_then(|v| v.as_str())
        .map(preview_str)
        .unwrap_or_default();

    tracing::info!(
        method,
        item_type,
        item_id,
        item_text_preview = %item_text,
        params = %preview_json(params),
        "codex app-server: raw notification"
    );
}

fn is_delta_notification(method: &str) -> bool {
    matches!(
        method,
        "item/agentMessage/delta" | "item/reasoning/textDelta"
    )
}

fn preview_json(value: &Value) -> String {
    let raw = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    preview_str(&raw)
}

fn preview_str(text: &str) -> String {
    if text.chars().count() <= LOG_PREVIEW_CHARS {
        return text.to_string();
    }
    let mut end = LOG_PREVIEW_CHARS;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn agent_message_text(item: &Value) -> Option<String> {
    if item_type(item)? != "agentMessage" {
        return None;
    }
    let text = item.get("text").and_then(|v| v.as_str())?;
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ignores_deltas_and_emits_on_item_completed() {
        let mut mapper = TurnEventMapper::new();

        assert!(mapper
            .push(&json!({
                "method": "item/started",
                "params": { "item": { "id": "item_1", "type": "agentMessage", "text": "" } }
            }))
            .is_empty());

        assert!(mapper
            .push(&json!({
                "method": "item/agentMessage/delta",
                "params": { "delta": "hello", "itemId": "item_1" }
            }))
            .is_empty());

        let completed = mapper.push(&json!({
            "method": "item/completed",
            "params": {
                "item": {
                    "id": "item_1",
                    "type": "agentMessage",
                    "text": "hello world"
                }
            }
        }));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].status, EventStatus::Completed);
        assert_eq!(completed[0].text, "hello world");

        let done = mapper.push(&json!({
            "method": "turn/completed",
            "params": { "threadId": "t", "turn": {} }
        }));
        assert!(mapper.is_turn_done());
        assert!(done.iter().any(|e| e.kind == EventKind::Response));
        assert_eq!(mapper.accumulated, "hello world");
    }

    #[test]
    fn maps_multiple_agent_messages_on_completed_only() {
        let mut mapper = TurnEventMapper::new();
        let mut events: Vec<AgentEvent> = Vec::new();

        for note in [
            json!({"method": "item/started", "params": {"item": {"id": "item_1", "type": "agentMessage", "text": ""}}}),
            json!({"method": "item/agentMessage/delta", "params": {"delta": "first", "itemId": "item_1"}}),
            json!({"method": "item/completed", "params": {"item": {"id": "item_1", "type": "agentMessage", "text": "first part"}}}),
            json!({"method": "item/started", "params": {"item": {"id": "item_2", "type": "agentMessage", "text": ""}}}),
            json!({"method": "item/agentMessage/delta", "params": {"delta": "second", "itemId": "item_2"}}),
            json!({"method": "item/completed", "params": {"item": {"id": "item_2", "type": "agentMessage", "text": "second part"}}}),
        ] {
            events.extend(mapper.push(&note));
        }

        let completed: Vec<_> = events
            .iter()
            .filter(|e| e.kind == EventKind::Message && e.status == EventStatus::Completed)
            .collect();
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].text, "first part");
        assert_eq!(completed[1].text, "second part");
        assert_eq!(mapper.accumulated, "first part\n\nsecond part");

        assert!(events
            .iter()
            .all(|e| e.kind != EventKind::Content && e.status != EventStatus::InProgress));
    }

    #[test]
    fn turn_event_counts_for_completed_only_mode() {
        let mut mapper = TurnEventMapper::new();
        let mut all_events: Vec<AgentEvent> = Vec::new();

        for note in [
            json!({"method": "item/started", "params": {"item": {"id": "m1", "type": "agentMessage", "text": ""}}}),
            json!({"method": "item/agentMessage/delta", "params": {"delta": "Checking…", "itemId": "m1"}}),
            json!({"method": "item/completed", "params": {"item": {"id": "m1", "type": "agentMessage", "text": "Checking…"}}}),
            json!({"method": "item/started", "params": {"item": {"id": "m2", "type": "agentMessage", "text": ""}}}),
            json!({"method": "item/completed", "params": {"item": {"id": "m2", "type": "agentMessage", "text": "Done: all good"}}}),
            json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {}}}),
        ] {
            all_events.extend(mapper.push(&note));
        }

        let completed = all_events
            .iter()
            .filter(|e| e.kind == EventKind::Message && e.status == EventStatus::Completed)
            .count();
        assert_eq!(completed, 2);
        assert_eq!(mapper.accumulated, "Checking…\n\nDone: all good");
    }
}
