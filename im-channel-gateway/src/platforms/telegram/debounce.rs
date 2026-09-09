use std::collections::HashMap;
use std::sync::Mutex;

use crate::types::{ChannelPart, PartKind};

/// Per-session buffer for media-only messages until text arrives.
pub struct MediaDebounce {
    pending: Mutex<HashMap<String, Vec<ChannelPart>>>,
}

impl Default for MediaDebounce {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }
}

fn has_text(parts: &[ChannelPart]) -> bool {
    parts
        .iter()
        .any(|p| p.kind == PartKind::Text && !p.text.trim().is_empty())
}

fn has_media_non_text(parts: &[ChannelPart]) -> bool {
    parts.iter().any(|p| {
        matches!(
            p.kind,
            PartKind::Image | PartKind::Video | PartKind::File | PartKind::Audio
        )
    })
}

fn has_audio_only(parts: &[ChannelPart]) -> bool {
    !has_text(parts)
        && parts.iter().any(|p| p.kind == PartKind::Audio)
        && parts
            .iter()
            .all(|p| matches!(p.kind, PartKind::Audio | PartKind::Text))
}

/// Python `TelegramChannel._apply_no_text_debounce` + `BaseChannel` logic.
pub fn apply_debounce(
    debounce: &MediaDebounce,
    session_id: &str,
    content_parts: &[ChannelPart],
) -> Option<Vec<ChannelPart>> {
    if has_media_non_text(content_parts) {
        let mut guard = debounce.pending.lock().unwrap();
        let pending = guard.remove(session_id).unwrap_or_default();
        let mut merged = pending;
        merged.extend(content_parts.iter().cloned());
        return Some(merged);
    }

    if has_text(content_parts) {
        let mut guard = debounce.pending.lock().unwrap();
        let pending = guard.remove(session_id).unwrap_or_default();
        let mut merged = pending;
        merged.extend(content_parts.iter().cloned());
        return Some(merged);
    }

    if has_audio_only(content_parts) {
        let mut guard = debounce.pending.lock().unwrap();
        let pending = guard.remove(session_id).unwrap_or_default();
        let mut merged = pending;
        merged.extend(content_parts.iter().cloned());
        return Some(merged);
    }

    if !content_parts.is_empty() {
        let mut guard = debounce.pending.lock().unwrap();
        guard
            .entry(session_id.to_string())
            .or_default()
            .extend(content_parts.iter().cloned());
        tracing::debug!(
            "telegram debounce: buffered session={}",
            &session_id[..session_id.len().min(24)]
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_triggers_immediate_merge() {
        let d = MediaDebounce::default();
        let sid = "telegram:1";
        let img = ChannelPart {
            kind: PartKind::Image,
            url: "file:///tmp/a.jpg".into(),
            ..Default::default()
        };
        let merged = apply_debounce(&d, sid, &[img]).unwrap();
        assert_eq!(merged.len(), 1);
        let merged2 = apply_debounce(&d, sid, &[ChannelPart::text_part("hi")]).unwrap();
        assert_eq!(merged2.len(), 1);
    }
}
