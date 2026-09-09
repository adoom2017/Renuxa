use std::collections::HashMap;

use crate::config::{CliRunnerConfig, CliSecurityConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    Run { runner: String, prompt: String },
    Help,
    ListAgents,
}

/// Parse IM text into a CLI routing decision.
pub fn parse_command(
    text: &str,
    command_prefix: &str,
    require_command_prefix: bool,
    default_runner: &str,
    runners: &HashMap<String, CliRunnerConfig>,
) -> Result<ParsedCommand, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("empty message".into());
    }

    let prefix = if command_prefix.is_empty() {
        "/"
    } else {
        command_prefix
    };

    if trimmed == format!("{prefix}help") || trimmed == format!("{prefix}agents") {
        if trimmed.ends_with("agents") {
            return Ok(ParsedCommand::ListAgents);
        }
        return Ok(ParsedCommand::Help);
    }

    if let Some(rest) = trimmed.strip_prefix(prefix) {
        let rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(ParsedCommand::Help);
        }
        let (runner_name, prompt) = rest
            .split_once(char::is_whitespace)
            .map(|(r, p)| (r.trim(), p.trim()))
            .unwrap_or((rest, ""));
        if runner_name.is_empty() {
            return Ok(ParsedCommand::Help);
        }
        if !runners.contains_key(runner_name) {
            return Err(format!("unknown runner '{runner_name}'"));
        }
        if prompt.is_empty() {
            return Err(format!("missing prompt for runner '{runner_name}'"));
        }
        return Ok(ParsedCommand::Run {
            runner: runner_name.to_string(),
            prompt: prompt.to_string(),
        });
    }

    if require_command_prefix {
        return Err(format!(
            "messages must start with '{prefix}<runner> <prompt>' (see {prefix}help)"
        ));
    }

    let runner = if default_runner.is_empty() {
        enabled_runner_names(runners)
            .first()
            .cloned()
            .ok_or_else(|| "no default_runner and no enabled runners".to_string())?
    } else {
        if !runners.contains_key(default_runner) {
            return Err(format!(
                "default_runner '{default_runner}' is not configured"
            ));
        }
        default_runner.to_string()
    };

    Ok(ParsedCommand::Run {
        runner,
        prompt: trimmed.to_string(),
    })
}

pub fn enabled_runner_names(runners: &HashMap<String, CliRunnerConfig>) -> Vec<String> {
    let mut names: Vec<String> = runners
        .iter()
        .filter(|(_, r)| r.enabled && !r.command.is_empty())
        .map(|(k, _)| k.clone())
        .collect();
    names.sort();
    names
}

pub fn format_help(
    command_prefix: &str,
    default_runner: &str,
    runners: &HashMap<String, CliRunnerConfig>,
) -> String {
    let prefix = if command_prefix.is_empty() {
        "/"
    } else {
        command_prefix
    };
    let enabled = enabled_runner_names(runners);
    let mut lines = vec![
        "CLI agent commands:".to_string(),
        format!("  {prefix}help — show this message"),
        format!("  {prefix}agents — list enabled runners"),
        format!("  {prefix}<runner> <prompt> — run a CLI agent"),
    ];
    if !default_runner.is_empty() {
        lines.push(format!(
            "  (no prefix) — use default runner '{default_runner}'"
        ));
    }
    if enabled.is_empty() {
        lines.push("Enabled runners: (none)".to_string());
    } else {
        lines.push(format!("Enabled runners: {}", enabled.join(", ")));
    }
    lines.join("\n")
}

pub fn format_agents_list(runners: &HashMap<String, CliRunnerConfig>) -> String {
    let enabled = enabled_runner_names(runners);
    if enabled.is_empty() {
        return "No enabled CLI runners. Enable one under [agent.runners.<name>] in config.".into();
    }
    let mut lines = vec!["Enabled CLI runners:".to_string()];
    for name in enabled {
        let cfg = &runners[&name];
        lines.push(format!("  {name}: {} {}", cfg.command, cfg.args.join(" ")));
    }
    lines.join("\n")
}

pub fn check_sender_allowed(security: &CliSecurityConfig, user_id: &str) -> Result<(), String> {
    if security.allowed_senders.is_empty() {
        return Ok(());
    }
    if security.allowed_senders.iter().any(|s| s == user_id) {
        Ok(())
    } else {
        Err(format!("sender '{user_id}' is not in allowed_senders"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_runners() -> HashMap<String, CliRunnerConfig> {
        let mut m = HashMap::new();
        m.insert(
            "codex".into(),
            CliRunnerConfig {
                enabled: true,
                command: "codex".into(),
                ..Default::default()
            },
        );
        m.insert(
            "echo".into(),
            CliRunnerConfig {
                enabled: true,
                command: "echo".into(),
                ..Default::default()
            },
        );
        m
    }

    #[test]
    fn parses_prefixed_runner() {
        let runners = sample_runners();
        let cmd = parse_command("/codex fix login", "/", false, "codex", &runners).unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Run {
                runner: "codex".into(),
                prompt: "fix login".into(),
            }
        );
    }

    #[test]
    fn parses_help() {
        let runners = sample_runners();
        assert_eq!(
            parse_command("/help", "/", false, "codex", &runners).unwrap(),
            ParsedCommand::Help
        );
    }

    #[test]
    fn rejects_unknown_runner() {
        let runners = sample_runners();
        assert!(parse_command("/unknown x", "/", false, "codex", &runners).is_err());
    }

    #[test]
    fn default_runner_without_prefix() {
        let runners = sample_runners();
        let cmd = parse_command("hello world", "/", false, "echo", &runners).unwrap();
        assert_eq!(
            cmd,
            ParsedCommand::Run {
                runner: "echo".into(),
                prompt: "hello world".into(),
            }
        );
    }

    #[test]
    fn require_prefix_blocks_plain_text() {
        let runners = sample_runners();
        assert!(parse_command("hello", "/", true, "echo", &runners).is_err());
    }
}
