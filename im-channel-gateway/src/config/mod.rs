mod schema;

pub use schema::*;

use std::path::{Path, PathBuf};

use crate::error::{GatewayError, Result};

const ENV_CONFIG: &str = "IM_GATEWAY_CONFIG";

/// Load config from path or `IM_GATEWAY_CONFIG` or `./config.toml`.
pub fn load_config(path: Option<&Path>) -> Result<AppConfig> {
    let path = resolve_config_path(path)?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| GatewayError::Config(format!("read {}: {e}", path.display())))?;
    let mut cfg: AppConfig = toml::from_str(&raw)
        .map_err(|e| GatewayError::Config(format!("parse {}: {e}", path.display())))?;
    if let Ok(token) = std::env::var("WECHAT_GATEWAY_TOKEN") {
        cfg.agent.bearer_token = token;
    }
    cfg.resolve_paths()?;
    Ok(cfg)
}

pub fn resolve_config_path(path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = path {
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = std::env::var(ENV_CONFIG) {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let local = PathBuf::from("config.toml");
    if local.is_file() {
        return Ok(local);
    }
    if let Some(dir) = dirs::config_dir() {
        let p = dir.join("im-channel-gateway").join("config.toml");
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(GatewayError::Config(
        "no config.toml found; run `im-channel-gateway init` or set IM_GATEWAY_CONFIG".into(),
    ))
}

pub fn default_config() -> AppConfig {
    AppConfig::default()
}

pub fn write_default_config(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let example = include_str!("../../config.example.toml");
    std::fs::write(path, example)?;
    Ok(())
}
