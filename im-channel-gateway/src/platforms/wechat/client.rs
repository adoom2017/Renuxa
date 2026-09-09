use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::config::DEFAULT_WECHAT_BOT_TYPE;
use crate::error::{GatewayError, Result};
use crate::platforms::wechat::crypto::{aes_ecb_decrypt, make_headers};

pub const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const CHANNEL_VERSION: &str = "2.0.1";
const GETUPDATES_TIMEOUT: Duration = Duration::from_secs(45);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const QRCODE_STATUS_TIMEOUT: Duration = Duration::from_secs(60);

/// Conservative iLink text limit (Unicode chars); see community protocol notes.
pub const WECHAT_TEXT_CHUNK_CHARS: usize = 2000;
/// Delay between consecutive sends to reduce silent drops under burst delivery.
pub const WECHAT_SEND_INTERVAL: Duration = Duration::from_millis(300);
pub const WECHAT_TYPING_STATUS_TYPING: i64 = 1;
pub const WECHAT_TYPING_STATUS_CLEANUP: i64 = 2;

pub fn check_ilink_ret(response: &Value) -> Result<()> {
    let ret = response
        .get("ret")
        .and_then(|v| v.as_i64())
        .or_else(|| response.get("errcode").and_then(|v| v.as_i64()));
    match ret {
        None | Some(0) => Ok(()),
        Some(code) => {
            let msg = response
                .get("errmsg")
                .or_else(|| response.get("message"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let message = match msg {
                Some(m) => format!("ret={code}: {m}"),
                None => format!("ret={code}: ilink api error; response={response}"),
            };
            Err(GatewayError::Channel {
                channel: "wechat".into(),
                message,
            })
        }
    }
}

pub fn ilink_ret_code(error: &GatewayError) -> Option<i64> {
    match error {
        GatewayError::Channel { message, .. } => message
            .strip_prefix("ret=")
            .and_then(|rest| rest.split(':').next())
            .and_then(|code| code.parse().ok()),
        _ => None,
    }
}

pub fn is_ilink_auth_error(error: &GatewayError) -> bool {
    match error {
        GatewayError::Api { status, .. } => matches!(*status, 401 | 403),
        GatewayError::Channel { channel, message } if channel == "wechat" => {
            let msg = message.to_ascii_lowercase();
            msg.contains("token")
                || msg.contains("auth")
                || msg.contains("credential")
                || msg.contains("unauthorized")
                || msg.contains("forbidden")
                || msg.contains("expired")
                || msg.contains("invalid")
        }
        _ => false,
    }
}

fn preview_str(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

fn redacted_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let redacted = if matches!(
                        lower.as_str(),
                        "context_token"
                            | "typing_ticket"
                            | "bot_token"
                            | "aes_key"
                            | "encrypt_query_param"
                            | "qrcode"
                            | "text"
                            | "item_list"
                            | "aeskey"
                    ) {
                        Value::String("***".to_string())
                    } else {
                        redacted_value(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redacted_value).collect()),
        _ => value.clone(),
    }
}

fn preview_json(value: &Value) -> String {
    preview_str(
        &serde_json::to_string(&redacted_value(value)).unwrap_or_else(|_| value.to_string()),
        500,
    )
}

fn preview_response_body(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => preview_json(&value),
        Err(_) => "[non-JSON response redacted]".into(),
    }
}

fn preview_params(params: &[(&str, String)]) -> String {
    let value = Value::Object(
        params
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
            .collect(),
    );
    preview_json(&value)
}

fn is_getupdates_path(path: &str) -> bool {
    path.trim_start_matches('/') == "ilink/bot/getupdates"
}

fn response_has_messages(value: &Value) -> bool {
    const MESSAGE_KEYS: &[&str] = &["msgs", "msg_list", "message_list", "messages", "updates"];
    let objects = value
        .as_object()
        .into_iter()
        .chain(value.get("data").and_then(|v| v.as_object()));
    for obj in objects {
        for key in MESSAGE_KEYS {
            if obj
                .get(*key)
                .and_then(|v| v.as_array())
                .is_some_and(|items| !items.is_empty())
            {
                return true;
            }
        }
        if obj.get("msg").is_some_and(|v| v.is_object()) {
            return true;
        }
    }
    false
}

pub struct ILinkClient {
    bot_token: RwLock<String>,
    token_file: Option<PathBuf>,
    pub base_url: String,
    http: Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeChatLoginResult {
    pub bot_token: String,
    pub base_url: String,
    pub bot_user_id: Option<String>,
}

impl ILinkClient {
    pub fn new(bot_token: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::build(bot_token, base_url, None)
    }

    pub fn new_with_token_file(
        bot_token: impl Into<String>,
        base_url: impl Into<String>,
        token_file: PathBuf,
    ) -> Self {
        Self::build(bot_token, base_url, Some(token_file))
    }

    fn build(
        bot_token: impl Into<String>,
        base_url: impl Into<String>,
        token_file: Option<PathBuf>,
    ) -> Self {
        let http = Client::builder()
            .timeout(GETUPDATES_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            bot_token: RwLock::new(bot_token.into()),
            token_file,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn bot_token_snapshot(&self) -> String {
        self.bot_token
            .read()
            .map(|token| token.clone())
            .unwrap_or_default()
    }

    pub fn set_bot_token(&self, token: impl Into<String>) {
        if let Ok(mut current) = self.bot_token.write() {
            *current = token.into();
        }
    }

    fn reload_bot_token_from_file(&self) -> Result<bool> {
        let Some(path) = self.token_file.as_ref() else {
            return Ok(false);
        };
        if !path.is_file() {
            return Ok(false);
        }
        let token = std::fs::read_to_string(path)?.trim().to_string();
        if token.is_empty() || token == self.bot_token_snapshot() {
            return Ok(false);
        }
        self.set_bot_token(token);
        tracing::warn!(
            token_file = %path.display(),
            "wechat: reloaded bot token from file"
        );
        Ok(true)
    }

    async fn get_once(
        &self,
        path: &str,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        let token = self.bot_token_snapshot();
        let started = Instant::now();
        tracing::info!(
            method = "GET",
            path,
            timeout_ms = timeout.as_millis(),
            params = %preview_params(params),
            "wechat ilink api request"
        );
        let resp = match self
            .http
            .get(self.url(path))
            .headers(make_headers(&token))
            .query(params)
            .timeout(timeout)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(
                    method = "GET",
                    path,
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %e,
                    "wechat ilink api request failed"
                );
                return Err(e.into());
            }
        };
        let status = resp.status();
        let body = resp.text().await?;
        tracing::info!(
            method = "GET",
            path,
            status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis(),
            response = %preview_response_body(&body),
            "wechat ilink api response"
        );
        if !status.is_success() {
            return Err(GatewayError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let value: Value = serde_json::from_str(&body)?;
        if let Err(e) = check_ilink_ret(&value) {
            tracing::warn!(
                method = "GET",
                path,
                error = %e,
                response = %preview_json(&value),
                "wechat ilink api returned nonzero ret"
            );
            return Err(e);
        }
        Ok(value)
    }

    async fn get(&self, path: &str, params: &[(&str, String)], timeout: Duration) -> Result<Value> {
        match self.get_once(path, params, timeout).await {
            Err(e) if is_ilink_auth_error(&e) && self.reload_bot_token_from_file()? => {
                self.get_once(path, params, timeout).await
            }
            other => other,
        }
    }

    async fn post_once(&self, path: &str, body: Value, timeout: Duration) -> Result<Value> {
        let token = self.bot_token_snapshot();
        let started = Instant::now();
        if is_getupdates_path(path) {
            tracing::debug!(
                method = "POST",
                path,
                timeout_ms = timeout.as_millis(),
                request = %preview_json(&body),
                "wechat ilink api request"
            );
        } else {
            tracing::info!(
                method = "POST",
                path,
                timeout_ms = timeout.as_millis(),
                request = %preview_json(&body),
                "wechat ilink api request"
            );
        }
        let resp = match self
            .http
            .post(self.url(path))
            .headers(make_headers(&token))
            .json(&body)
            .timeout(timeout)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(
                    method = "POST",
                    path,
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %e,
                    "wechat ilink api request failed"
                );
                return Err(e.into());
            }
        };
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            tracing::info!(
                method = "POST",
                path,
                status = status.as_u16(),
                elapsed_ms = started.elapsed().as_millis(),
                response = %preview_response_body(&text),
                "wechat ilink api response"
            );
            return Err(GatewayError::Api {
                status: status.as_u16(),
                body: text,
            });
        }
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                tracing::info!(
                    method = "POST",
                    path,
                    status = status.as_u16(),
                    elapsed_ms = started.elapsed().as_millis(),
                    response = %preview_response_body(&text),
                    "wechat ilink api response"
                );
                return Err(e.into());
            }
        };
        if is_getupdates_path(path) && !response_has_messages(&value) {
            tracing::debug!(
                method = "POST",
                path,
                status = status.as_u16(),
                elapsed_ms = started.elapsed().as_millis(),
                response = %preview_json(&value),
                "wechat ilink api response"
            );
        } else {
            tracing::info!(
                method = "POST",
                path,
                status = status.as_u16(),
                elapsed_ms = started.elapsed().as_millis(),
                response = %preview_json(&value),
                "wechat ilink api response"
            );
        }
        if let Err(e) = check_ilink_ret(&value) {
            tracing::warn!(
                method = "POST",
                path,
                error = %e,
                response = %preview_json(&value),
                "wechat ilink api returned nonzero ret"
            );
            return Err(e);
        }
        Ok(value)
    }

    async fn post(&self, path: &str, body: Value, timeout: Duration) -> Result<Value> {
        match self.post_once(path, body.clone(), timeout).await {
            Err(e) if is_ilink_auth_error(&e) && self.reload_bot_token_from_file()? => {
                self.post_once(path, body, timeout).await
            }
            other => other,
        }
    }

    pub async fn get_bot_qrcode(&self, bot_type: &str) -> Result<Value> {
        let bot_type = if bot_type.trim().is_empty() {
            DEFAULT_WECHAT_BOT_TYPE
        } else {
            bot_type.trim()
        };
        self.get(
            "ilink/bot/get_bot_qrcode",
            &[("bot_type", bot_type.to_string())],
            DEFAULT_TIMEOUT,
        )
        .await
    }

    pub async fn get_qrcode_status(&self, qrcode: &str) -> Result<Value> {
        let token = self.bot_token_snapshot();
        let mut headers = make_headers(&token);
        headers.insert(
            "iLink-App-ClientVersion",
            reqwest::header::HeaderValue::from_static("1"),
        );
        let path = "ilink/bot/get_qrcode_status";
        let started = Instant::now();
        tracing::info!(
            method = "GET",
            path,
            timeout_ms = QRCODE_STATUS_TIMEOUT.as_millis(),
            params = %preview_params(&[("qrcode", qrcode.to_string())]),
            "wechat ilink api request"
        );
        let resp = match self
            .http
            .get(self.url(path))
            .headers(headers)
            .query(&[("qrcode", qrcode.to_string())])
            .timeout(QRCODE_STATUS_TIMEOUT)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(
                    method = "GET",
                    path,
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %e,
                    "wechat ilink api request failed"
                );
                return Err(e.into());
            }
        };
        let status = resp.status();
        let body = resp.text().await?;
        tracing::info!(
            method = "GET",
            path,
            status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis(),
            response = %preview_response_body(&body),
            "wechat ilink api response"
        );
        if !status.is_success() {
            return Err(GatewayError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let value: Value = serde_json::from_str(&body)?;
        if let Err(e) = check_ilink_ret(&value) {
            tracing::warn!(
                method = "GET",
                path,
                error = %e,
                response = %preview_json(&value),
                "wechat ilink api returned nonzero ret"
            );
            return Err(e);
        }
        Ok(value)
    }

    pub async fn wait_for_login(
        &self,
        qrcode: &str,
        poll_interval: Duration,
        max_wait: Duration,
    ) -> Result<WeChatLoginResult> {
        let mut elapsed = Duration::ZERO;
        while elapsed < max_wait {
            let data = self.get_qrcode_status(qrcode).await?;
            let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status == "confirmed" {
                let token = data
                    .get("bot_token")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| GatewayError::Channel {
                        channel: "wechat".into(),
                        message: "QR confirmed without bot_token".into(),
                    })?
                    .to_string();
                let base = data
                    .get("baseurl")
                    .or_else(|| data.get("base_url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&self.base_url)
                    .to_string();
                let bot_user_id = data
                    .get("ilink_bot_id")
                    .or_else(|| data.get("bot_user_id"))
                    .or_else(|| data.get("botUserId"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string);
                return Ok(WeChatLoginResult {
                    bot_token: token,
                    base_url: base,
                    bot_user_id,
                });
            }
            if status == "expired" {
                return Err(GatewayError::Channel {
                    channel: "wechat".into(),
                    message: "QR code expired".into(),
                });
            }
            tokio::time::sleep(poll_interval).await;
            elapsed += poll_interval;
        }
        Err(GatewayError::Other(format!(
            "WeChat QR not confirmed within {}s",
            max_wait.as_secs()
        )))
    }

    pub async fn getupdates(&self, cursor: &str) -> Result<Value> {
        let body = json!({
            "get_updates_buf": cursor,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        self.post("ilink/bot/getupdates", body, GETUPDATES_TIMEOUT)
            .await
    }

    pub async fn getconfig(&self, ilink_user_id: &str, context_token: &str) -> Result<Value> {
        let body = json!({
            "ilink_user_id": ilink_user_id,
            "context_token": context_token,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        self.post("ilink/bot/getconfig", body, DEFAULT_TIMEOUT)
            .await
    }

    pub async fn sendtyping(
        &self,
        ilink_user_id: &str,
        typing_ticket: &str,
        status: i64,
    ) -> Result<Value> {
        let body = json!({
            "ilink_user_id": ilink_user_id,
            "typing_ticket": typing_ticket,
            "status": status,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        self.post("ilink/bot/sendtyping", body, DEFAULT_TIMEOUT)
            .await
    }

    pub async fn sendmessage(&self, msg: Value) -> Result<Value> {
        let body = json!({
            "msg": msg,
            "base_info": { "channel_version": CHANNEL_VERSION },
        });
        self.post("ilink/bot/sendmessage", body, DEFAULT_TIMEOUT)
            .await
    }

    pub async fn send_text(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: &str,
    ) -> Result<Value> {
        self.sendmessage(json!({
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": Uuid::new_v4().to_string(),
            "message_type": 2,
            "message_state": 2,
            "context_token": context_token,
            "item_list": [{"type": 1, "text_item": {"text": text}}],
        }))
        .await
    }

    pub async fn download_media(
        &self,
        url: &str,
        aes_key_b64: &str,
        encrypt_query_param: &str,
    ) -> Result<Vec<u8>> {
        let download_url = if !encrypt_query_param.is_empty() {
            let enc = urlencoding::encode(encrypt_query_param);
            format!("https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param={enc}")
        } else if url.starts_with("http") {
            url.to_string()
        } else {
            return Err(GatewayError::Channel {
                channel: "wechat".into(),
                message: format!("invalid media url: {url}"),
            });
        };
        let parsed = reqwest::Url::parse(&download_url)
            .map_err(|_| GatewayError::Other("invalid media URL".into()))?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("novac2c.cdn.weixin.qq.com")
            || parsed.port().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(GatewayError::Other("untrusted media URL".into()));
        }
        let media_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let started = Instant::now();
        tracing::info!(
            method = "GET",
            path = "c2c/download",
            timeout_ms = Duration::from_secs(60).as_millis(),
            has_aes_key = !aes_key_b64.is_empty(),
            has_encrypt_query_param = !encrypt_query_param.is_empty(),
            "wechat media api request"
        );
        let mut resp = match media_http
            .get(&download_url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(
                    method = "GET",
                    path = "c2c/download",
                    elapsed_ms = started.elapsed().as_millis(),
                    "wechat media api request failed"
                );
                return Err(e.without_url().into());
            }
        };
        let status = resp.status();
        if !status.is_success() {
            return Err(GatewayError::Other("media download failed".into()));
        }
        const LIMIT: usize = 8 * 1024 * 1024;
        if resp
            .content_length()
            .is_some_and(|n| n > (LIMIT + 16) as u64)
        {
            return Err(GatewayError::Other("image too large".into()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            if bytes.len() + chunk.len() > LIMIT + 16 {
                return Err(GatewayError::Other("image too large".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        tracing::info!(
            method = "GET",
            path = "c2c/download",
            status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis(),
            bytes = bytes.len(),
            "wechat media api response"
        );
        let bytes = if aes_key_b64.is_empty() {
            bytes
        } else {
            aes_ecb_decrypt(&bytes, aes_key_b64)?
        };
        if bytes.len() > LIMIT {
            return Err(GatewayError::Other("image too large".into()));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn one_response_server(body: String) -> String {
        response_server(vec![body]).await
    }

    async fn response_server(responses: Vec<String>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        tokio::spawn(async move {
            for body in responses {
                let (mut stream, _) = listener.accept().await.expect("accept request");
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });
        format!("http://{addr}")
    }

    async fn capture_request_server(body: String) -> (String, Arc<Mutex<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server addr");
        let captured = Arc::new(Mutex::new(String::new()));
        let captured2 = captured.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut buf = [0_u8; 4096];
            let n = stream.read(&mut buf).await.expect("read request");
            *captured2.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{addr}"), captured)
    }

    #[test]
    fn check_ilink_ret_accepts_success_and_empty() {
        assert!(check_ilink_ret(&json!({})).is_ok());
        assert!(check_ilink_ret(&json!({"ret": 0})).is_ok());
    }

    #[test]
    fn check_ilink_ret_rejects_nonzero() {
        let err = check_ilink_ret(&json!({"ret": -2, "errmsg": "bad params"})).unwrap_err();
        assert!(err.to_string().contains("-2"));
        assert!(err.to_string().contains("bad params"));
    }

    #[test]
    fn check_ilink_ret_includes_response_when_errmsg_missing() {
        let err = check_ilink_ret(&json!({"ret": -2, "detail": "stale token"})).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("-2"));
        assert!(msg.contains("response="));
        assert!(msg.contains("stale token"));
    }

    #[test]
    fn ilink_ret_code_parses_channel_error() {
        let err = check_ilink_ret(&json!({"ret": -2})).unwrap_err();
        assert_eq!(ilink_ret_code(&err), Some(-2));
    }

    #[test]
    fn response_preview_redacts_sensitive_fields() {
        let preview = preview_response_body(
            r#"{"ret":0,"bot_token":"bot_secret","typing_ticket":"ticket_secret","context_token":"ctx_secret"}"#,
        );

        assert!(preview.contains("***"));
        assert!(!preview.contains("bot_secret"));
        assert!(!preview.contains("ticket_secret"));
        assert!(!preview.contains("ctx_secret"));
    }

    #[test]
    fn response_has_messages_accepts_empty_and_nested_batches() {
        assert!(!response_has_messages(&json!({"msgs": []})));
        assert!(!response_has_messages(&json!({"data": {"msg_list": []}})));
        assert!(response_has_messages(
            &json!({"msgs": [{"message_type": 1}]})
        ));
        assert!(response_has_messages(
            &json!({"data": {"msg_list": [{"message_type": 1}]}})
        ));
        assert!(response_has_messages(
            &json!({"data": {"msg": {"message_type": 1}}})
        ));
    }

    #[tokio::test]
    async fn get_rejects_nonzero_ret() {
        let base_url =
            one_response_server(json!({"ret": -2, "errmsg": "bad params"}).to_string()).await;
        let client = ILinkClient::new("token", base_url);

        let err = client.get_bot_qrcode("1").await.unwrap_err();

        assert!(err.to_string().contains("-2"));
    }

    #[tokio::test]
    async fn qrcode_status_sends_client_version_header() {
        let (base_url, captured) =
            capture_request_server(json!({"status": "wait"}).to_string()).await;
        let client = ILinkClient::new("", base_url);

        let data = client.get_qrcode_status("qr_1").await.unwrap();

        assert_eq!(data.get("status").and_then(|v| v.as_str()), Some("wait"));
        let raw = captured.lock().unwrap().to_ascii_lowercase();
        assert!(raw.contains("ilink-app-clientversion: 1"));
    }

    #[tokio::test]
    async fn wait_for_login_returns_bot_user_id() {
        let base_url = one_response_server(
            json!({
                "status": "confirmed",
                "bot_token": "token_1",
                "base_url": "https://example.test",
                "botUserId": " wx_bot "
            })
            .to_string(),
        )
        .await;
        let client = ILinkClient::new("", base_url);

        let login = client
            .wait_for_login("qr_1", Duration::from_millis(1), Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(login.bot_token, "token_1");
        assert_eq!(login.base_url, "https://example.test");
        assert_eq!(login.bot_user_id.as_deref(), Some("wx_bot"));
    }

    #[tokio::test]
    async fn post_reloads_token_file_once_on_auth_error() {
        let dir = tempfile::tempdir().unwrap();
        let token_file = dir.path().join("wechat_bot_token");
        std::fs::write(&token_file, "new_token").unwrap();
        let base_url = response_server(vec![
            json!({"ret": -1, "errmsg": "invalid token"}).to_string(),
            json!({"ret": 0, "msgs": []}).to_string(),
        ])
        .await;
        let client = ILinkClient::new_with_token_file("old_token", base_url, token_file);

        let data = client.getupdates("").await.unwrap();

        assert_eq!(data.get("ret").and_then(|v| v.as_i64()), Some(0));
    }

    #[tokio::test]
    async fn getconfig_sends_typing_config_body() {
        let (base_url, captured) =
            capture_request_server(json!({"ret": 0, "typing_ticket": "ticket_1"}).to_string())
                .await;
        let client = ILinkClient::new("token", base_url);

        let data = client.getconfig("wx_user", "ctx_1").await.unwrap();

        assert_eq!(
            data.get("typing_ticket").and_then(|v| v.as_str()),
            Some("ticket_1")
        );
        let raw = captured.lock().unwrap().clone();
        assert!(raw.starts_with("POST /ilink/bot/getconfig "));
        let body: Value =
            serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).expect("json body");
        assert_eq!(body["ilink_user_id"], "wx_user");
        assert_eq!(body["context_token"], "ctx_1");
        assert_eq!(body["base_info"]["channel_version"], CHANNEL_VERSION);
    }

    #[tokio::test]
    async fn sendtyping_sends_status_body() {
        let (base_url, captured) = capture_request_server(json!({"ret": 0}).to_string()).await;
        let client = ILinkClient::new("token", base_url);

        let data = client
            .sendtyping("wx_user", "ticket_1", WECHAT_TYPING_STATUS_TYPING)
            .await
            .unwrap();

        assert_eq!(data.get("ret").and_then(|v| v.as_i64()), Some(0));
        let raw = captured.lock().unwrap().clone();
        assert!(raw.starts_with("POST /ilink/bot/sendtyping "));
        let body: Value =
            serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).expect("json body");
        assert_eq!(body["ilink_user_id"], "wx_user");
        assert_eq!(body["typing_ticket"], "ticket_1");
        assert_eq!(body["status"], WECHAT_TYPING_STATUS_TYPING);
        assert_eq!(body["base_info"]["channel_version"], CHANNEL_VERSION);
    }
}
