/// Split long text for IM platforms (byte-length, line-boundary aware).
pub fn split_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        if end < text.len() {
            if let Some(rel) = text[start..end].rfind('\n') {
                end = start + rel + 1;
            }
        }
        chunks.push(text[start..end].to_string());
        start = end;
    }
    chunks
}

const SENTENCE_END_CHARS: &[char] = &['。', '．', '.', '！', '!', '？', '?', '；', ';', '…'];

/// Split by Unicode character count. Prefers logical boundaries near `max_chars`:
/// `\n\n` → `\n` → sentence-ending punctuation → space → hard cut.
pub fn split_text_chars(text: &str, max_chars: usize) -> Vec<String> {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if rest.chars().count() <= max_chars {
            chunks.push(rest.to_string());
            break;
        }
        let hard_end = rest
            .char_indices()
            .nth(max_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(rest.len());
        let window = &rest[..hard_end];
        let split_end = find_split_end(window, hard_end, max_chars);
        chunks.push(rest[..split_end].to_string());
        rest = &rest[split_end..];
    }
    chunks
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

fn last_sentence_break_end(window: &str) -> Option<usize> {
    let mut last_end = None;
    for (idx, ch) in window.char_indices() {
        if SENTENCE_END_CHARS.contains(&ch) {
            last_end = Some(idx + ch.len_utf8());
        }
    }
    last_end
}

fn find_split_end(window: &str, hard_end: usize, max_chars: usize) -> usize {
    if let Some(pos) = window.rfind("\n\n") {
        return pos + 2;
    }
    if let Some(pos) = window.rfind('\n') {
        return pos + 1;
    }

    // Prefer sentence ends when the chunk stays close to the limit.
    let min_chars = max_chars.saturating_sub(150).max(max_chars * 2 / 3);
    if let Some(end) = last_sentence_break_end(window) {
        if char_count(&window[..end]) >= min_chars {
            return end;
        }
    }

    if let Some(pos) = window.rfind(' ') {
        let end = pos + ' '.len_utf8();
        if char_count(&window[..end]) >= min_chars {
            return end;
        }
    }

    hard_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_at_paragraph_boundary() {
        let para1 = "甲".repeat(1800);
        let para2 = "乙".repeat(500);
        let text = format!("{para1}\n\n{para2}");
        let chunks = split_text_chars(&text, 2000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with("\n\n"));
        assert!(chunks[1].starts_with('乙'));
    }

    #[test]
    fn split_at_sentence_period_near_limit() {
        let body = "这是测试句子。".repeat(300);
        let chunks = split_text_chars(&body, 2000);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].ends_with('。'));
        assert!(chunks[0].chars().count() >= 1700);
        assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
        assert_eq!(chunks.concat().chars().count(), body.chars().count());
    }

    #[test]
    fn split_skips_early_sentence_break_for_larger_chunk() {
        let early = "短句。";
        let rest = "x".repeat(1900);
        let text = format!("{early}{rest}");
        let chunks = split_text_chars(&text, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chars().count(), text.chars().count());
    }
}
