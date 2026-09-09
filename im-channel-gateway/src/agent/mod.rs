pub mod cli;
pub mod codex_app_server;
pub mod echo;
pub mod http_sse;

use std::sync::Arc;

use crate::config::AppConfig;
use crate::error::{GatewayError, Result};
use crate::gateway::SharedAgentBackend;

pub fn build_agent_backend(cfg: &AppConfig) -> Result<SharedAgentBackend> {
    match cfg.agent.backend.as_str() {
        "echo" => Ok(echo::echo_backend()),
        "http_sse" | "agentscope_compatible" | "agentscope" => {
            Ok(Arc::new(http_sse::HttpSseBackend::from_config(&cfg.agent)?))
        }
        "cli" => Ok(Arc::new(cli::CliBackend::from_config(&cfg.agent))),
        "cli_router" => Ok(Arc::new(cli::CliRouterBackend::from_config(&cfg.agent))),
        "codex_app_server" => Ok(Arc::new(codex_app_server::CodexAppServerBackend::from_config(
            &cfg.agent,
            cfg.data_path(),
        )?)),
        other => Err(GatewayError::Config(format!(
            "unknown agent.backend '{other}'; use echo, http_sse, cli, cli_router, or codex_app_server"
        ))),
    }
}
