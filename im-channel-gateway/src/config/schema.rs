use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{GatewayError, Result};

pub const DEFAULT_WECHAT_BOT_TYPE: &str = "3";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub data: DataConfig,
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataConfig {
    #[serde(default = "default_data_dir")]
    pub dir: String,
}

fn default_data_dir() -> String {
    "~/.local/share/im-channel-gateway".to_string()
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            dir: default_data_dir(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "default_admin_listen")]
    pub listen: String,
}

fn default_admin_listen() -> String {
    "127.0.0.1:18765".to_string()
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            listen: default_admin_listen(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CliPromptVia {
    #[default]
    Stdin,
    LastArg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliRunnerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub prompt_via: CliPromptVia,
    #[serde(default)]
    pub cwd: String,
    /// IM output filter: `codex` strips CLI banners/hooks (full output stays in logs).
    #[serde(default)]
    pub output_profile: String,
    /// Keep only text before this marker (ignored when `output_profile = codex`).
    #[serde(default)]
    pub output_strip_from: String,
}

impl Default for CliRunnerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            prompt_via: CliPromptVia::Stdin,
            cwd: String::new(),
            output_profile: String::new(),
            output_strip_from: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSecurityConfig {
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    #[serde(default = "default_max_run_secs")]
    pub max_run_secs: u64,
    #[serde(default = "default_max_output_kb")]
    pub max_output_kb: usize,
    #[serde(default)]
    pub workspace_root: String,
}

fn default_max_run_secs() -> u64 {
    600
}

fn default_max_output_kb() -> usize {
    512
}

impl Default for CliSecurityConfig {
    fn default() -> Self {
        Self {
            allowed_senders: Vec::new(),
            max_run_secs: default_max_run_secs(),
            max_output_kb: default_max_output_kb(),
            workspace_root: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAppServerConfig {
    /// WebSocket URL, e.g. `ws://127.0.0.1:4500`
    #[serde(default = "default_codex_app_server_listen")]
    pub listen: String,
    /// Bearer token file when app-server uses `--ws-auth capability-token`
    #[serde(default)]
    pub token_file: String,
    #[serde(default = "default_codex_model")]
    pub model: String,
    /// Default cwd for `thread/start` when IM session has no binding yet
    #[serde(default)]
    pub default_cwd: String,
    #[serde(default = "default_command_prefix")]
    pub command_prefix: String,
    #[serde(
        default = "default_codex_sandbox",
        deserialize_with = "deserialize_codex_sandbox"
    )]
    pub sandbox: String,
    #[serde(default = "default_codex_approval")]
    pub approval_policy: String,
}

fn default_codex_app_server_listen() -> String {
    "ws://127.0.0.1:4500".to_string()
}

fn default_codex_model() -> String {
    "gpt-5.4".to_string()
}

fn default_codex_approval() -> String {
    "never".to_string()
}

fn default_codex_sandbox() -> String {
    "workspace-write".to_string()
}

pub fn normalize_codex_sandbox(sandbox: &str) -> String {
    match sandbox.trim() {
        "readOnly" => "read-only".to_string(),
        "workspaceWrite" => "workspace-write".to_string(),
        "dangerFullAccess" => "danger-full-access".to_string(),
        other => other.to_string(),
    }
}

fn deserialize_codex_sandbox<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(normalize_codex_sandbox(&value))
}

impl Default for CodexAppServerConfig {
    fn default() -> Self {
        Self {
            listen: default_codex_app_server_listen(),
            token_file: String::new(),
            model: default_codex_model(),
            default_cwd: String::new(),
            command_prefix: default_command_prefix(),
            sandbox: default_codex_sandbox(),
            approval_policy: default_codex_approval(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_agent_backend")]
    pub backend: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_process_path")]
    pub path: String,
    #[serde(default = "default_agent_id")]
    pub agent_id: String,
    #[serde(default)]
    pub bearer_token: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub default_runner: String,
    #[serde(default = "default_command_prefix")]
    pub command_prefix: String,
    #[serde(default)]
    pub require_command_prefix: bool,
    #[serde(default)]
    pub runners: HashMap<String, CliRunnerConfig>,
    #[serde(default)]
    pub security: CliSecurityConfig,
    #[serde(default)]
    pub codex_app_server: CodexAppServerConfig,
}

fn default_command_prefix() -> String {
    "/".to_string()
}

fn default_agent_backend() -> String {
    "echo".to_string()
}
fn default_base_url() -> String {
    "http://127.0.0.1:8088".to_string()
}
fn default_process_path() -> String {
    "/api/agent/process".to_string()
}
fn default_agent_id() -> String {
    "default".to_string()
}
fn default_language() -> String {
    "zh".to_string()
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            backend: default_agent_backend(),
            base_url: default_base_url(),
            path: default_process_path(),
            agent_id: default_agent_id(),
            bearer_token: String::new(),
            language: default_language(),
            default_runner: String::new(),
            command_prefix: default_command_prefix(),
            require_command_prefix: false,
            runners: HashMap::new(),
            security: CliSecurityConfig::default(),
            codex_app_server: CodexAppServerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub wechat: WeChatConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BaseChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_prefix: String,
    #[serde(default)]
    pub filter_tool_messages: bool,
    #[serde(default)]
    pub filter_thinking: bool,
    #[serde(default)]
    pub access_control_dm: bool,
    #[serde(default)]
    pub access_control_group: bool,
    #[serde(default)]
    pub media_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    #[serde(flatten)]
    pub base: BaseChannelConfig,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub bot_tokens: Vec<String>,
    #[serde(default)]
    pub http_proxy: String,
    #[serde(default)]
    pub http_proxy_auth: String,
    #[serde(default = "default_true")]
    pub show_typing: bool,
    #[serde(default)]
    pub streaming_enabled: bool,
    #[serde(default)]
    pub require_mention: bool,
    /// Maximum concurrently active Telegram bot accounts.
    #[serde(default = "default_telegram_max_accounts")]
    pub max_accounts: usize,
}

fn default_true() -> bool {
    true
}

fn default_telegram_max_accounts() -> usize {
    20
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            base: BaseChannelConfig::default(),
            bot_token: String::new(),
            bot_tokens: Vec::new(),
            http_proxy: String::new(),
            http_proxy_auth: String::new(),
            show_typing: true,
            streaming_enabled: false,
            require_mention: false,
            max_accounts: default_telegram_max_accounts(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatConfig {
    /// Forward verified images as data URLs, without writing originals to disk.
    #[serde(default)]
    pub inline_images: bool,
    #[serde(flatten)]
    pub base: BaseChannelConfig,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub bot_token_file: String,
    #[serde(default)]
    pub base_url: String,
    /// iLink app marker shown during QR scan.
    #[serde(default = "default_wechat_bot_type")]
    pub bot_type: String,
    /// Stream Codex/app-server replies back to IM (auto-on for `codex_app_server` backend).
    #[serde(default)]
    pub streaming_enabled: bool,
    #[serde(default)]
    pub message_merge_enabled: bool,
    #[serde(default)]
    pub message_merge_delay_ms: i64,
    /// Maximum concurrently active WeChat bot accounts (dynamic QR registration).
    #[serde(default = "default_wechat_max_accounts")]
    pub max_accounts: usize,
}

fn default_wechat_max_accounts() -> usize {
    20
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            inline_images: false,
            base: BaseChannelConfig::default(),
            bot_token: String::new(),
            bot_token_file: String::new(),
            base_url: String::new(),
            bot_type: default_wechat_bot_type(),
            streaming_enabled: false,
            message_merge_enabled: false,
            message_merge_delay_ms: 0,
            max_accounts: default_wechat_max_accounts(),
        }
    }
}

fn default_wechat_bot_type() -> String {
    DEFAULT_WECHAT_BOT_TYPE.to_string()
}

impl AppConfig {
    pub fn resolve_paths(&mut self) -> Result<()> {
        self.data.dir = expand_tilde(&self.data.dir)?;
        if self.channels.wechat.bot_token_file.is_empty() {
            self.channels.wechat.bot_token_file = self
                .data_path()
                .join("wechat_bot_token")
                .to_string_lossy()
                .into_owned();
        } else {
            self.channels.wechat.bot_token_file =
                expand_tilde(&self.channels.wechat.bot_token_file)?;
        }
        if self.channels.telegram.base.media_dir.is_empty() {
            self.channels.telegram.base.media_dir = self
                .data_path()
                .join("media")
                .join("telegram")
                .to_string_lossy()
                .into_owned();
        } else {
            self.channels.telegram.base.media_dir =
                expand_tilde(&self.channels.telegram.base.media_dir)?;
        }
        if self.channels.wechat.base.media_dir.is_empty() {
            self.channels.wechat.base.media_dir = self
                .data_path()
                .join("media")
                .join("wechat")
                .to_string_lossy()
                .into_owned();
        } else {
            self.channels.wechat.base.media_dir =
                expand_tilde(&self.channels.wechat.base.media_dir)?;
        }
        if self.agent.security.workspace_root.is_empty() {
            self.agent.security.workspace_root = self
                .data_path()
                .join("workspaces")
                .to_string_lossy()
                .into_owned();
        } else {
            self.agent.security.workspace_root = expand_tilde(&self.agent.security.workspace_root)?;
        }
        for runner in self.agent.runners.values_mut() {
            if !runner.cwd.is_empty() {
                runner.cwd = expand_tilde(&runner.cwd)?;
            }
        }
        if !self.agent.codex_app_server.token_file.is_empty() {
            self.agent.codex_app_server.token_file =
                expand_tilde(&self.agent.codex_app_server.token_file)?;
        }
        if !self.agent.codex_app_server.default_cwd.is_empty() {
            self.agent.codex_app_server.default_cwd =
                expand_tilde(&self.agent.codex_app_server.default_cwd)?;
        }
        Ok(())
    }

    pub fn data_path(&self) -> PathBuf {
        PathBuf::from(self.data.dir.as_str())
    }
}

pub fn expand_tilde(path: &str) -> Result<String> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs::home_dir()
            .ok_or_else(|| GatewayError::Config("cannot resolve home directory".into()))?;
        return Ok(home.join(rest).to_string_lossy().into_owned());
    }
    if path == "~" {
        let home = dirs::home_dir()
            .ok_or_else(|| GatewayError::Config("cannot resolve home directory".into()))?;
        return Ok(home.to_string_lossy().into_owned());
    }
    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_app_server_default_sandbox_uses_cli_spelling() {
        assert_eq!(CodexAppServerConfig::default().sandbox, "workspace-write");
    }

    #[test]
    fn codex_app_server_deserialize_missing_sandbox_uses_cli_spelling() {
        let cfg: AppConfig = toml::from_str(
            r#"
            [agent]
            backend = "codex_app_server"

            [agent.codex_app_server]
            listen = "ws://127.0.0.1:4500"
            default_cwd = "/tmp/project"
            "#,
        )
        .expect("config should deserialize");

        assert_eq!(cfg.agent.codex_app_server.sandbox, "workspace-write");
    }

    #[test]
    fn codex_app_server_deserialize_legacy_sandbox_uses_cli_spelling() {
        let cfg: AppConfig = toml::from_str(
            r#"
            [agent]
            backend = "codex_app_server"

            [agent.codex_app_server]
            sandbox = "workspaceWrite"
            "#,
        )
        .expect("config should deserialize");

        assert_eq!(cfg.agent.codex_app_server.sandbox, "workspace-write");
    }

    #[test]
    fn wechat_default_bot_type_matches_ilink_default() {
        assert_eq!(WeChatConfig::default().bot_type, DEFAULT_WECHAT_BOT_TYPE);
    }

    #[test]
    fn wechat_deserialize_missing_bot_type_uses_default() {
        let cfg: AppConfig = toml::from_str(
            r#"
            [channels.wechat]
            enabled = true
            "#,
        )
        .expect("config should deserialize");

        assert_eq!(cfg.channels.wechat.bot_type, DEFAULT_WECHAT_BOT_TYPE);
    }

    #[test]
    fn telegram_default_max_accounts_is_20() {
        assert_eq!(TelegramConfig::default().max_accounts, 20);
    }

    #[test]
    fn telegram_deserialize_missing_multi_account_fields_uses_defaults() {
        let cfg: AppConfig = toml::from_str(
            r#"
            [channels.telegram]
            enabled = true
            bot_token = "token"
            "#,
        )
        .expect("config should deserialize");

        assert!(cfg.channels.telegram.bot_tokens.is_empty());
        assert_eq!(cfg.channels.telegram.max_accounts, 20);
    }
}
