mod output_filter;
mod router;
mod runner;
mod stream;

pub(crate) use router::check_sender_allowed;
pub(crate) use stream::{error_events, help_events, request_text};

#[cfg(test)]
mod tests;

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::AgentConfig;
use crate::gateway::{AgentBackend, AgentEventStream};
use crate::types::ChannelRequest;

pub use router::{format_agents_list, format_help};
pub use runner::SessionLocks;

use router::{parse_command, ParsedCommand};
use runner::run_cli_agent;

#[derive(Clone)]
pub struct CliRouterBackend {
    cfg: AgentConfig,
    session_locks: Arc<SessionLocks>,
}

impl CliRouterBackend {
    pub fn from_config(cfg: &AgentConfig) -> Self {
        Self {
            cfg: cfg.clone(),
            session_locks: Arc::new(SessionLocks::new()),
        }
    }

    async fn handle_request(&self, request: ChannelRequest) -> Vec<crate::types::AgentEvent> {
        if let Err(msg) = check_sender_allowed(&self.cfg.security, &request.user_id) {
            return error_events(msg);
        }

        let text = request_text(&request);
        tracing::info!(
            channel = %request.channel,
            user_id = %request.user_id,
            session_id = %request.session_id,
            text_len = text.len(),
            text_preview = %preview_log(&text, 200),
            backend = %self.cfg.backend,
            "cli: inbound message"
        );
        let parsed = match parse_command(
            &text,
            &self.cfg.command_prefix,
            self.cfg.require_command_prefix,
            &self.cfg.default_runner,
            &self.cfg.runners,
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "cli: command parse failed");
                let hint = format_help(
                    &self.cfg.command_prefix,
                    &self.cfg.default_runner,
                    &self.cfg.runners,
                );
                return error_events(format!("{e}\n\n{hint}"));
            }
        };

        match parsed {
            ParsedCommand::Help => help_events(format_help(
                &self.cfg.command_prefix,
                &self.cfg.default_runner,
                &self.cfg.runners,
            )),
            ParsedCommand::ListAgents => help_events(format_agents_list(&self.cfg.runners)),
            ParsedCommand::Run { runner, prompt } => {
                tracing::info!(
                    runner = %runner,
                    prompt_len = prompt.len(),
                    prompt_preview = %preview_log(&prompt, 200),
                    "cli: spawning runner"
                );
                let Some(runner_cfg) = self.cfg.runners.get(&runner) else {
                    tracing::warn!(runner = %runner, "cli: runner not in config");
                    return error_events(format!("runner '{runner}' is not configured"));
                };
                match run_cli_agent(
                    &runner,
                    runner_cfg,
                    &self.cfg.security,
                    &request.channel,
                    &request.session_id,
                    &prompt,
                    &self.session_locks,
                )
                .await
                {
                    Ok(events) => {
                        let summary: Vec<String> = events
                            .iter()
                            .map(|e| format!("{:?}/{:?} len={}", e.kind, e.status, e.text.len()))
                            .collect();
                        tracing::info!(
                            runner = %runner,
                            event_count = events.len(),
                            events = %summary.join(", "),
                            "cli: runner finished"
                        );
                        events
                    }
                    Err(e) => {
                        tracing::error!(runner = %runner, error = %e, "cli: runner error");
                        error_events(e.to_string())
                    }
                }
            }
        }
    }
}

fn preview_log(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[async_trait]
impl AgentBackend for CliRouterBackend {
    async fn stream(&self, request: ChannelRequest) -> AgentEventStream {
        let this = self.clone();
        Box::pin(async_stream::stream! {
            for ev in this.handle_request(request).await {
                yield Ok(ev);
            }
        })
    }
}

/// Single-runner CLI backend (uses `default_runner` only, no command prefix routing).
#[derive(Clone)]
pub struct CliBackend {
    inner: CliRouterBackend,
}

impl CliBackend {
    pub fn from_config(cfg: &AgentConfig) -> Self {
        let mut cfg = cfg.clone();
        cfg.require_command_prefix = false;
        if cfg.default_runner.is_empty() {
            if let Some(name) = router::enabled_runner_names(&cfg.runners).first() {
                cfg.default_runner = name.clone();
            }
        }
        Self {
            inner: CliRouterBackend::from_config(&cfg),
        }
    }
}

#[async_trait]
impl AgentBackend for CliBackend {
    async fn stream(&self, request: ChannelRequest) -> AgentEventStream {
        self.inner.stream(request).await
    }
}
