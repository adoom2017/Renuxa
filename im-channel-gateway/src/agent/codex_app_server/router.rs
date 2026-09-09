#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    Help,
    Session,
    Thread,
    Threads {
        loaded_only: bool,
        limit: usize,
    },
    New,
    Bind {
        thread_id: String,
    },
    Rename {
        name: String,
    },
    Approve {
        request_id: Option<String>,
    },
    Deny {
        request_id: Option<String>,
    },
    Answer {
        request_id: Option<String>,
        text: String,
    },
    Prompt {
        text: String,
    },
}

pub fn parse_command(text: &str, command_prefix: &str) -> Result<ParsedCommand, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty message".into());
    }

    let prefix = if command_prefix.is_empty() {
        "/"
    } else {
        command_prefix
    };

    if is_no_arg_command(trimmed, prefix, "help") {
        return Ok(ParsedCommand::Help);
    }
    if is_no_arg_command(trimmed, prefix, "session")
        || is_no_arg_command(trimmed, prefix, "sessions")
    {
        return Ok(ParsedCommand::Session);
    }
    if is_no_arg_command(trimmed, prefix, "thread") {
        return Ok(ParsedCommand::Thread);
    }
    if let Some(rest) =
        command_args(trimmed, prefix, "threads").or_else(|| command_args(trimmed, prefix, "list"))
    {
        let mut loaded_only = false;
        let mut limit = 10;
        for part in rest.split_whitespace() {
            if part == "loaded" {
                loaded_only = true;
            } else if let Ok(n) = part.parse::<usize>() {
                limit = n.clamp(1, 20);
            } else {
                return Err(format!("usage: {prefix}threads [loaded] [limit]"));
            }
        }
        return Ok(ParsedCommand::Threads { loaded_only, limit });
    }
    if is_no_arg_command(trimmed, prefix, "new") {
        return Ok(ParsedCommand::New);
    }
    if let Some(rest) = command_args(trimmed, prefix, "bind") {
        let thread_id = rest.trim();
        if thread_id.is_empty() {
            return Err(format!("usage: {prefix}bind <threadId>"));
        }
        return Ok(ParsedCommand::Bind {
            thread_id: thread_id.to_string(),
        });
    }
    if let Some(rest) = command_args(trimmed, prefix, "rename") {
        let name = rest.trim();
        if name.is_empty() {
            return Err(format!("usage: {prefix}rename <name>"));
        }
        return Ok(ParsedCommand::Rename {
            name: name.to_string(),
        });
    }
    if let Some(rest) = command_args(trimmed, prefix, "approve") {
        let request_id = optional_single_arg(rest);
        return Ok(ParsedCommand::Approve { request_id });
    }
    if let Some(rest) = command_args(trimmed, prefix, "deny") {
        let request_id = optional_single_arg(rest);
        return Ok(ParsedCommand::Deny { request_id });
    }
    if let Some(rest) = command_args(trimmed, prefix, "answer") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err(format!("usage: {prefix}answer [requestId] <answer>"));
        }
        let (request_id, text) = split_optional_request_id(rest);
        if text.is_empty() {
            return Err(format!("usage: {prefix}answer [requestId] <answer>"));
        }
        return Ok(ParsedCommand::Answer {
            request_id,
            text: text.to_string(),
        });
    }

    if trimmed.starts_with(prefix) {
        let command = trimmed.split_whitespace().next().unwrap_or(trimmed);
        return Err(format!("unknown command: {command}"));
    }

    Ok(ParsedCommand::Prompt {
        text: trimmed.to_string(),
    })
}

fn optional_single_arg(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn split_optional_request_id(rest: &str) -> (Option<String>, &str) {
    let Some((first, tail)) = rest.split_once(char::is_whitespace) else {
        return (None, rest.trim());
    };
    if first.starts_with("req_") || first.starts_with("appr_") {
        (Some(first.to_string()), tail.trim())
    } else {
        (None, rest.trim())
    }
}

fn is_no_arg_command(text: &str, prefix: &str, name: &str) -> bool {
    command_args(text, prefix, name).is_some_and(|rest| rest.trim().is_empty())
}

fn command_args<'a>(text: &'a str, prefix: &str, name: &str) -> Option<&'a str> {
    let command = format!("{prefix}{name}");
    let rest = text.strip_prefix(&command)?;
    if rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

pub fn format_help(command_prefix: &str) -> String {
    let prefix = if command_prefix.is_empty() {
        "/"
    } else {
        command_prefix
    };
    [
        "Codex app-server commands:".to_string(),
        format!("  {prefix}help — show this message"),
        format!("  {prefix}session — show current IM session id and binding info"),
        format!("  {prefix}thread — show bound Codex thread id for this IM session"),
        format!(
            "  {prefix}threads [limit] — list recent Codex threads plus loaded remote threads"
        ),
        format!("  {prefix}threads loaded [limit] — list currently loaded app-server thread ids"),
        format!("  {prefix}bind <threadId|number> — bind to a thread id or a number from {prefix}threads"),
        format!("  {prefix}rename <name> — rename the bound Codex thread"),
        format!("  {prefix}approve [requestId] — approve a pending Codex request"),
        format!("  {prefix}deny [requestId] — deny a pending Codex request"),
        format!("  {prefix}answer [requestId] <answer> — answer a pending Codex input request"),
        format!("  {prefix}new — clear binding; next message starts a new thread"),
        "  <text> — send prompt to bound (or new) thread via turn/start".to_string(),
        "".to_string(),
        "Prerequisite: run `codex app-server --listen ws://127.0.0.1:4500` on this Mac."
            .to_string(),
        "Optional TUI: `codex --remote ws://127.0.0.1:4500`".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bind() {
        let cmd = parse_command("/bind thr_abc", "/").unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Bind {
                thread_id: "thr_abc".into()
            }
        );
    }

    #[test]
    fn parses_rename() {
        let cmd = parse_command("/rename rust-agent wechat", "/").unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Rename {
                name: "rust-agent wechat".into()
            }
        );
    }

    #[test]
    fn rejects_empty_rename() {
        let err = parse_command("/rename   ", "/").unwrap_err();
        assert_eq!(err, "usage: /rename <name>");
    }

    #[test]
    fn parses_approval_commands() {
        assert_eq!(
            parse_command("/approve appr_1", "/").unwrap(),
            ParsedCommand::Approve {
                request_id: Some("appr_1".into())
            }
        );
        assert_eq!(
            parse_command("/deny", "/").unwrap(),
            ParsedCommand::Deny { request_id: None }
        );
        assert_eq!(
            parse_command("/answer appr_1 yes", "/").unwrap(),
            ParsedCommand::Answer {
                request_id: Some("appr_1".into()),
                text: "yes".into()
            }
        );
        assert_eq!(
            parse_command("/answer use option A", "/").unwrap(),
            ParsedCommand::Answer {
                request_id: None,
                text: "use option A".into()
            }
        );
    }

    #[test]
    fn parses_session() {
        let cmd = parse_command("/session", "/").unwrap();
        assert_eq!(cmd, ParsedCommand::Session);
    }

    #[test]
    fn parses_sessions_alias() {
        let cmd = parse_command("/sessions", "/").unwrap();
        assert_eq!(cmd, ParsedCommand::Session);
    }

    #[test]
    fn rejects_unknown_slash_command_instead_of_forwarding_as_prompt() {
        let err = parse_command("/sessionsx", "/").unwrap_err();
        assert!(err.contains("unknown command"));
    }

    #[test]
    fn parses_threads() {
        let cmd = parse_command("/threads 5", "/").unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Threads {
                loaded_only: false,
                limit: 5
            }
        );
    }

    #[test]
    fn parses_loaded_threads() {
        let cmd = parse_command("/threads loaded 3", "/").unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Threads {
                loaded_only: true,
                limit: 3
            }
        );
    }

    #[test]
    fn parses_prompt() {
        let cmd = parse_command("continue fixing tests", "/").unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Prompt {
                text: "continue fixing tests".into()
            }
        );
    }
}
