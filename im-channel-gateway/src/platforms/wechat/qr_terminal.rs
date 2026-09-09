//! Decode WeChat login QR PNG (base64) or scan URL and render a scannable QR in the terminal.

use std::path::Path;

use base64::Engine as _;
use image::ImageReader;
use qrcode::{EcLevel, QrCode};
use rqrr::PreparedImage;

use crate::config::DEFAULT_WECHAT_BOT_TYPE;
use crate::error::{GatewayError, Result};

const LITEAPP_QR_PATH: &str = "https://liteapp.weixin.qq.com/q/7GiQu1";

/// Resolve the URL encoded in the login QR (matches Python `WeChatQRCodeAuthHandler`).
pub fn resolve_scan_url(qrcode_token: &str, img_content: &str, bot_type: &str) -> String {
    let trimmed = img_content.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return trimmed.to_string();
    }
    if !qrcode_token.is_empty() {
        let bot_type = if bot_type.trim().is_empty() {
            DEFAULT_WECHAT_BOT_TYPE
        } else {
            bot_type.trim()
        };
        return format!(
            "{LITEAPP_QR_PATH}?qrcode={}&bot_type={}",
            urlencoding::encode(qrcode_token),
            urlencoding::encode(bot_type)
        );
    }
    String::new()
}

fn strip_data_url_prefix(input: &str) -> &str {
    let trimmed = input.trim();
    trimmed
        .split_once("base64,")
        .map(|(_, data)| data)
        .unwrap_or(trimmed)
}

fn decode_png_bytes(input: &str) -> Result<Vec<u8>> {
    let payload = strip_data_url_prefix(input);
    if payload.is_empty() {
        return Err(GatewayError::Other("empty qrcode_img_content".into()));
    }
    base64::engine::general_purpose::STANDARD
        .decode(payload.as_bytes())
        .map_err(|e| GatewayError::Other(format!("invalid qrcode base64: {e}")))
}

/// Decode the first QR payload from a base64-encoded PNG (`qrcode_img_content`).
pub fn decode_payload_from_png_b64(b64: &str) -> Result<String> {
    let bytes = decode_png_bytes(b64)?;
    decode_payload_from_png_bytes(&bytes)
}

pub fn decode_payload_from_png_bytes(png: &[u8]) -> Result<String> {
    let img = ImageReader::new(std::io::Cursor::new(png))
        .with_guessed_format()
        .map_err(|e| GatewayError::Other(format!("PNG read failed: {e}")))?
        .decode()
        .map_err(|e| GatewayError::Other(format!("PNG decode failed: {e}")))?
        .to_luma8();
    let mut prepared = PreparedImage::prepare(img);
    let grids = prepared.detect_grids();
    let grid = grids
        .first()
        .ok_or_else(|| GatewayError::Other("no QR code found in login image".into()))?;
    let (_meta, content) = grid
        .decode()
        .map_err(|e| GatewayError::Other(format!("QR decode failed: {e}")))?;
    if content.is_empty() {
        return Err(GatewayError::Other("decoded QR payload is empty".into()));
    }
    Ok(content)
}

/// Render QR text to stdout (two columns per module for easier scanning).
pub fn print_payload_to_stdout(payload: &str) -> Result<()> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M)
        .map_err(|e| GatewayError::Other(format!("terminal QR encode failed: {e}")))?;
    let art = code
        .render::<char>()
        .quiet_zone(true)
        .module_dimensions(2, 1)
        .build();
    println!("{art}");
    Ok(())
}

/// Write a PNG QR for *scan_url* to *path*.
pub fn save_png_for_scan_url(path: &Path, scan_url: &str) -> Result<()> {
    let code = QrCode::with_error_correction_level(scan_url.as_bytes(), EcLevel::M)
        .map_err(|e| GatewayError::Other(format!("QR PNG encode failed: {e}")))?;
    let img = code.render::<image::Luma<u8>>().build();
    let dyn_img = image::DynamicImage::ImageLuma8(img);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = std::fs::File::create(path)?;
    dyn_img
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| GatewayError::Other(format!("QR PNG write failed: {e}")))?;
    Ok(())
}

/// Save API PNG bytes when present; otherwise generate from *scan_url*. Returns whether a file was written.
pub fn save_login_qr_png(path: &Path, img_content: &str, scan_url: &str) -> Result<bool> {
    if !img_content.trim().is_empty() {
        if let Ok(bytes) = decode_png_bytes(img_content) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, bytes)?;
            return Ok(true);
        }
    }
    if scan_url.is_empty() {
        return Ok(false);
    }
    save_png_for_scan_url(path, scan_url)?;
    Ok(true)
}

/// Show login QR in the terminal; returns the scan URL / payload string.
pub fn print_login_qr(qrcode_token: &str, img_content: &str, bot_type: &str) -> Result<String> {
    let trimmed = img_content.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        print_payload_to_stdout(trimmed)?;
        return Ok(trimmed.to_string());
    }
    if !trimmed.is_empty() {
        if let Ok(payload) = decode_payload_from_png_b64(trimmed) {
            print_payload_to_stdout(&payload)?;
            return Ok(payload);
        }
    }
    let scan_url = resolve_scan_url(qrcode_token, img_content, bot_type);
    if scan_url.is_empty() {
        return Err(GatewayError::Other("no WeChat scan URL available".into()));
    }
    print_payload_to_stdout(&scan_url)?;
    Ok(scan_url)
}

/// Decode PNG base64 from iLink and print a terminal QR; returns the payload string.
pub fn print_from_png_b64(b64: &str) -> Result<String> {
    let payload = decode_payload_from_png_b64(b64)?;
    print_payload_to_stdout(&payload)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_http_img_content() {
        let url = "https://liteapp.weixin.qq.com/q/test?qrcode=abc&bot_type=3";
        assert_eq!(resolve_scan_url("token", url, "8"), url);
    }

    #[test]
    fn resolves_liteapp_url_from_token() {
        let url = resolve_scan_url("mytoken", "", "8");
        assert!(url.contains("qrcode=mytoken"));
        assert!(url.contains("bot_type=8"));
    }

    #[test]
    fn resolves_liteapp_url_with_default_bot_type() {
        let url = resolve_scan_url("mytoken", "", "");
        assert!(url.contains("bot_type=3"));
    }

    #[test]
    fn print_login_qr_accepts_http_url() {
        let url = "https://example.com/wechat-login-test";
        let shown = print_login_qr("", url, "8").unwrap();
        assert_eq!(shown, url);
    }

    #[test]
    fn renders_terminal_qr_art() {
        let art = QrCode::new(b"https://example.com/wechat-login-test")
            .unwrap()
            .render::<char>()
            .quiet_zone(true)
            .module_dimensions(2, 1)
            .build();
        assert!(art.lines().count() > 10);
        assert!(art.contains('█'));
    }

    #[test]
    fn roundtrip_png_decode() {
        let payload = "https://example.com/wechat-login-test";
        let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::M).unwrap();
        let png = {
            let img = code.render::<image::Luma<u8>>().build();
            let mut buf = Vec::new();
            let dyn_img = image::DynamicImage::ImageLuma8(img);
            dyn_img
                .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                .unwrap();
            buf
        };
        let decoded = decode_payload_from_png_bytes(&png).unwrap();
        assert_eq!(decoded, payload);
    }
}
