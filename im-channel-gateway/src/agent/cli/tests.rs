use std::collections::HashMap;

use futures::StreamExt;

use crate::agent::cli::router::{self, ParsedCommand};
use crate::agent::cli::CliRouterBackend;
use crate::config::{AgentConfig, CliPromptVia, CliRunnerConfig, CliSecurityConfig};
use crate::gateway::AgentBackend;
use crate::types::{ChannelPart, ChannelRequest};

fn echo_runner_config() -> AgentConfig {
    let mut runners = HashMap::new();
    runners.insert(
        "echo".into(),
        CliRunnerConfig {
            enabled: true,
            command: "echo".into(),
            args: vec![],
            prompt_via: CliPromptVia::LastArg,
            ..Default::default()
        },
    );
    AgentConfig {
        backend: "cli_router".into(),
        default_runner: "echo".into(),
        command_prefix: "/".into(),
        require_command_prefix: false,
        runners,
        security: CliSecurityConfig {
            max_run_secs: 30,
            max_output_kb: 64,
            workspace_root: std::env::temp_dir()
                .join("im-channel-gateway-test")
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn test_request(text: &str) -> ChannelRequest {
    ChannelRequest {
        channel: "wechat".into(),
        session_id: "wechat:testuser".into(),
        user_id: "testuser".into(),
        content: vec![ChannelPart::text_part(text)],
        meta: HashMap::new(),
    }
}

#[tokio::test]
async fn cli_router_runs_echo_runner() {
    let backend = CliRouterBackend::from_config(&echo_runner_config());
    let mut stream = backend.stream(test_request("hello from im")).await;
    let mut last_text = String::new();
    while let Some(item) = stream.next().await {
        let ev = item.unwrap();
        if !ev.text.is_empty() {
            last_text = ev.text;
        }
    }
    assert!(last_text.contains("hello from im"));
}

#[test]
fn parse_echo_prefixed_command() {
    let cfg = echo_runner_config();
    let cmd = router::parse_command(
        "/echo ping",
        &cfg.command_prefix,
        cfg.require_command_prefix,
        &cfg.default_runner,
        &cfg.runners,
    )
    .unwrap();
    assert_eq!(
        cmd,
        ParsedCommand::Run {
            runner: "echo".into(),
            prompt: "ping".into(),
        }
    );
}
