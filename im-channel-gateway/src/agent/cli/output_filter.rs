use crate::config::CliRunnerConfig;

const CODEX_STDIN_MARKER: &str = "Reading prompt from stdin...";

/// Split raw CLI output into IM-facing text vs full text for logs.
pub fn filter_for_im(raw: &str, runner_name: &str, cfg: &CliRunnerConfig) -> (String, String) {
    let full = raw.to_string();
    let profile = cfg.output_profile.trim();
    let im = if profile == "codex" || (profile.is_empty() && runner_name == "codex") {
        filter_codex(raw)
    } else if !cfg.output_strip_from.is_empty() {
        strip_before_marker(raw, &cfg.output_strip_from)
    } else {
        raw.trim().to_string()
    };
    (im, full)
}

fn filter_codex(raw: &str) -> String {
    if let Some(before) = extract_before_marker(raw, CODEX_STDIN_MARKER) {
        if !before.is_empty() && !is_noise_only(&before) {
            return before;
        }
    }
    if let Some(block) = extract_codex_reply_block(raw) {
        return block;
    }
    strip_codex_noise_lines(raw)
}

fn extract_before_marker(raw: &str, marker: &str) -> Option<String> {
    let idx = raw.find(marker)?;
    let before = raw[..idx].trim().to_string();
    if before.is_empty() {
        None
    } else {
        Some(before)
    }
}

fn strip_before_marker(raw: &str, marker: &str) -> String {
    extract_before_marker(raw, marker).unwrap_or_else(|| raw.trim().to_string())
}

/// Extract text after the last `codex` label line until hooks / tokens footer.
fn extract_codex_reply_block(raw: &str) -> Option<String> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut last_start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "codex" {
            last_start = Some(i + 1);
        }
    }
    let start = last_start?;
    let mut out = Vec::new();
    for line in &lines[start..] {
        let trimmed = line.trim();
        if is_codex_footer_line(trimmed) {
            break;
        }
        if is_codex_metadata_line(trimmed) {
            continue;
        }
        out.push(*line);
    }
    let text = out.join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn strip_codex_noise_lines(raw: &str) -> String {
    let kept: Vec<&str> = raw
        .lines()
        .filter(|line| {
            let t = line.trim();
            !t.is_empty()
                && !is_codex_metadata_line(t)
                && !is_codex_footer_line(t)
                && t != CODEX_STDIN_MARKER
        })
        .collect();
    kept.join("\n").trim().to_string()
}

fn is_noise_only(text: &str) -> bool {
    text.lines()
        .all(|l| l.trim().is_empty() || l.trim().starts_with("warning:"))
}

fn is_codex_footer_line(line: &str) -> bool {
    line.starts_with("hook:")
        || line.starts_with("tokens used")
        || line == "--------"
        || line == CODEX_STDIN_MARKER
}

fn is_codex_metadata_line(line: &str) -> bool {
    line.starts_with("OpenAI Codex")
        || line.starts_with("workdir:")
        || line.starts_with("model:")
        || line.starts_with("provider:")
        || line.starts_with("approval:")
        || line.starts_with("sandbox:")
        || line.starts_with("reasoning ")
        || line.starts_with("session id:")
        || line == "user"
        || line == "codex"
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "可以。你把需求、现有代码位置、期望行为或报错贴给我，我可以直接在当前工作区里读代码、修改文件、运行测试并给你结果。
Reading prompt from stdin...
OpenAI Codex v0.135.0
--------
workdir: /tmp/ws
model: gpt-5.5
--------
user
你可以写代码么？
hook: SessionStart
hook: SessionStart Completed
codex
可以。你把需求、现有代码位置、期望行为或报错贴给我，我可以直接在当前工作区里读代码、修改文件、运行测试并给你结果。
hook: Stop
tokens used
3,353";

    #[test]
    fn keeps_text_before_stdin_marker() {
        let im = filter_codex(SAMPLE);
        assert!(im.starts_with("可以。你把需求"));
        assert!(!im.contains("Reading prompt from stdin"));
        assert!(!im.contains("hook:"));
        assert!(!im.contains("tokens used"));
    }

    #[test]
    fn extracts_codex_block_when_no_preamble() {
        let raw = "Reading prompt from stdin...
--------
user
hello
codex
这是回复。
hook: Stop
tokens used 1";
        let im = filter_codex(raw);
        assert_eq!(im, "这是回复。");
    }
}
