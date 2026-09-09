use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::config::{AppConfig, WeChatConfig};
use crate::error::{GatewayError, Result};
use crate::platforms::wechat::client::{ILinkClient, DEFAULT_BASE_URL};
use crate::platforms::wechat::qr_terminal;
use crate::platforms::wechat::registry::WeChatAccountRegistry;

#[derive(Clone, Debug)]
pub struct QrLoginState {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

#[derive(Clone, Debug)]
pub struct QrLoginStatus {
    pub user_id: Option<String>,
    pub status: String,
    pub bot_token: Option<String>,
    pub base_url: Option<String>,
    pub bot_user_id: Option<String>,
}

pub async fn start_qr_login(cfg: &WeChatConfig) -> Result<QrLoginState> {
    let base = if cfg.base_url.is_empty() {
        DEFAULT_BASE_URL
    } else {
        &cfg.base_url
    };
    let client = ILinkClient::new("", base);
    let data = client.get_bot_qrcode(&cfg.bot_type).await?;
    let qrcode = data
        .get("qrcode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let img = data
        .get("qrcode_img_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(QrLoginState {
        qrcode,
        qrcode_img_content: img,
    })
}

pub async fn poll_qr_status(qrcode: &str, cfg: &WeChatConfig) -> Result<String> {
    Ok(poll_qr_login_status(qrcode, cfg).await?.status)
}

pub async fn poll_qr_login_status(qrcode: &str, cfg: &WeChatConfig) -> Result<QrLoginStatus> {
    let base = if cfg.base_url.is_empty() {
        DEFAULT_BASE_URL
    } else {
        &cfg.base_url
    };
    let client = ILinkClient::new("", base);
    let data = client.get_qrcode_status(qrcode).await?;
    qr_login_status_from_response(&data)
}

fn qr_login_status_from_response(data: &Value) -> Result<QrLoginStatus> {
    let status = data
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("waiting")
        .to_string();
    let bot_token = data
        .get("bot_token")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    if status == "confirmed" && bot_token.is_none() {
        return Err(GatewayError::Channel {
            channel: "wechat".into(),
            message: "QR confirmed without bot_token".into(),
        });
    }
    let base_url = data
        .get("baseurl")
        .or_else(|| data.get("base_url"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let bot_user_id = extract_string_field(data, &["ilink_bot_id", "bot_user_id", "botUserId"]);
    Ok(QrLoginStatus {
        user_id: extract_string_field(data, &["ilink_user_id", "user_id"]),
        status,
        bot_token,
        base_url,
        bot_user_id,
    })
}

fn extract_string_field(data: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| {
            data.get(*key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
        .map(str::to_string)
}

pub async fn run_cli_login(cfg: &mut AppConfig) -> Result<()> {
    cfg.resolve_paths()?;
    let client = ILinkClient::new(
        "",
        if cfg.channels.wechat.base_url.is_empty() {
            DEFAULT_BASE_URL
        } else {
            &cfg.channels.wechat.base_url
        },
    );
    let data = client.get_bot_qrcode(&cfg.channels.wechat.bot_type).await?;
    let qrcode = data
        .get("qrcode")
        .and_then(|v| v.as_str())
        .ok_or_else(|| crate::error::GatewayError::Other("missing qrcode".into()))?;
    let img_content = data
        .get("qrcode_img_content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let scan_url =
        qr_terminal::resolve_scan_url(qrcode, img_content, &cfg.channels.wechat.bot_type);
    let path = cfg.data_path().join("wechat_qrcode.png");
    if qr_terminal::save_login_qr_png(&path, img_content, &scan_url)? {
        tracing::info!("QR image saved to {}", path.display());
    }
    println!("\nScan the QR code below with WeChat:\n");
    match qr_terminal::print_login_qr(qrcode, img_content, &cfg.channels.wechat.bot_type) {
        Ok(shown) => {
            println!();
            if shown.starts_with("http://") || shown.starts_with("https://") {
                tracing::info!("Scan URL: {shown}");
            }
        }
        Err(e) => {
            tracing::warn!(
                "could not render QR in terminal ({e}); open {}",
                path.display()
            );
            if !scan_url.is_empty() {
                tracing::info!("Or open this URL in WeChat: {scan_url}");
            }
        }
    }
    tracing::info!("Waiting for scan (poll until confirmed)…");
    let login = client
        .wait_for_login(qrcode, Duration::from_secs(2), Duration::from_secs(300))
        .await?;
    let registry = WeChatAccountRegistry::load(cfg.data_path(), cfg.channels.wechat.max_accounts)?;
    let account_id = registry.register_from_login_with_bot_user_id(
        &login.bot_token,
        &login.base_url,
        login.bot_user_id.as_deref(),
    )?;
    println!("WeChat login OK; account_id={account_id}");
    tracing::info!(
        account_id = %account_id,
        "WeChat login registered new account (start gateway to begin polling)"
    );
    Ok(())
}

pub fn save_token_file(path: &Path, token: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, token)?;
    Ok(())
}

pub fn load_token_file(path: &Path) -> Result<String> {
    if path.is_file() {
        Ok(std::fs::read_to_string(path)?.trim().to_string())
    } else {
        Ok(String::new())
    }
}

pub fn load_cursor(data_dir: &Path) -> String {
    let p = data_dir.join("wechat_cursor");
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn save_cursor(data_dir: &Path, cursor: &str) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(data_dir.join("wechat_cursor"), cursor)?;
    Ok(())
}

pub fn parse_qr_response(data: &Value) -> Option<QrLoginState> {
    let qrcode = data.get("qrcode")?.as_str()?.to_string();
    let img = data
        .get("qrcode_img_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(QrLoginState {
        qrcode,
        qrcode_img_content: img,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn qr_login_status_extracts_confirmed_token() {
        let status = qr_login_status_from_response(&json!({
            "status": "confirmed",
            "bot_token": " token_1 ",
            "baseurl": "https://example.test"
        }))
        .unwrap();

        assert_eq!(status.status, "confirmed");
        assert_eq!(status.bot_token.as_deref(), Some("token_1"));
        assert_eq!(status.base_url.as_deref(), Some("https://example.test"));
        assert_eq!(status.bot_user_id, None);
    }

    #[test]
    fn qr_login_keeps_scanner_separate_from_bot() {
        let status = qr_login_status_from_response(&json!({"status":"confirmed","bot_token":"token","bot_user_id":"bot","user_id":"scanner"})).unwrap();
        assert_eq!(status.user_id.as_deref(), Some("scanner"));
        assert_eq!(status.bot_user_id.as_deref(), Some("bot"));
        let missing = qr_login_status_from_response(
            &json!({"status":"confirmed","bot_token":"token","bot_user_id":"bot"}),
        )
        .unwrap();
        assert_eq!(missing.user_id, None);
    }

    #[test]
    fn qr_login_accepts_ilink_identities_without_confusing_scanner_and_bot() {
        let status = qr_login_status_from_response(&json!({"status":"confirmed","bot_token":"fresh-token","ilink_bot_id":" bot@im.bot ","ilink_user_id":" scanner@im.wechat ","user_id":"legacy-scanner"})).unwrap();
        assert_eq!(status.bot_user_id.as_deref(), Some("bot@im.bot"));
        assert_eq!(status.user_id.as_deref(), Some("scanner@im.wechat"));
        let scanner_only = qr_login_status_from_response(
            &json!({"status":"confirmed","bot_token":"token","user_id":"scanner"}),
        )
        .unwrap();
        assert_eq!(scanner_only.bot_user_id, None);
    }

    #[test]
    fn qr_login_status_extracts_bot_user_id_aliases() {
        let status = qr_login_status_from_response(&json!({
            "status": "confirmed",
            "bot_token": "token_1",
            "base_url": "https://example.test",
            "botUserId": " wx_bot "
        }))
        .unwrap();

        assert_eq!(status.base_url.as_deref(), Some("https://example.test"));
        assert_eq!(status.bot_user_id.as_deref(), Some("wx_bot"));
    }

    #[test]
    fn qr_login_status_rejects_confirmed_without_token() {
        let err = qr_login_status_from_response(&json!({"status": "confirmed"})).unwrap_err();
        assert!(err.to_string().contains("bot_token"));
    }
}
