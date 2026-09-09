use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::config::{AppConfig, WeChatConfig};
use crate::error::Result;
use crate::platforms::telegram::TelegramChannel;
use crate::platforms::wechat::login::QrLoginState;
use crate::platforms::wechat::WeChatChannel;

#[derive(Clone)]
struct AdminState {
    qr: Arc<RwLock<HashMap<String, WeChatQrSession>>>,
    wechat_registration_lock: Arc<Mutex<()>>,
    wechat_cfg: WeChatConfig,
    telegram_channel: Option<Arc<TelegramChannel>>,
    wechat_channel: Option<Arc<WeChatChannel>>,
}

#[derive(Clone)]
struct WeChatQrSession {
    created_at: std::time::Instant,
    user_id: Option<String>,
    qr: QrLoginState,
    account_id: Option<String>,
    base_url: Option<String>,
    bot_user_id: Option<String>,
}

#[derive(Deserialize)]
struct TelegramRegisterRequest {
    bot_token: String,
}

#[derive(Debug, Default, Deserialize)]
struct WeChatQrStatusQuery {
    qr_session_id: Option<String>,
    session_id: Option<String>,
}

pub async fn serve(
    cfg: &AppConfig,
    telegram_channel: Option<Arc<TelegramChannel>>,
    wechat_channel: Option<Arc<WeChatChannel>>,
) -> Result<()> {
    let state = AdminState {
        qr: Arc::new(RwLock::new(HashMap::new())),
        wechat_registration_lock: Arc::new(Mutex::new(())),
        wechat_cfg: cfg.channels.wechat.clone(),
        telegram_channel,
        wechat_channel,
    };
    let app = build_router(state).layer(axum::middleware::from_fn(admin_auth));

    let listener = tokio::net::TcpListener::bind(&cfg.admin.listen).await?;
    tracing::info!("admin API listening on {}", cfg.admin.listen);
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_router(state: AdminState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(
            "/api/channels/telegram/accounts",
            get(telegram_accounts).post(telegram_register_account),
        )
        .route(
            "/api/channels/telegram/accounts/{account_id}",
            delete(telegram_remove_account),
        )
        .route("/api/channels/wechat/accounts", get(wechat_accounts))
        .route(
            "/api/channels/wechat/accounts/{account_id}",
            delete(wechat_remove_account),
        )
        .route("/api/channels/wechat/qrcode", get(wechat_qrcode))
        .route(
            "/api/channels/wechat/qrcode/status",
            get(wechat_qrcode_status),
        )
        .with_state(state)
}

async fn admin_auth(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use sha2::{Digest, Sha256};
    let expected = std::env::var("WECHAT_GATEWAY_TOKEN").unwrap_or_default();
    let supplied = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let mismatch = Sha256::digest(expected.as_bytes())
        .iter()
        .zip(Sha256::digest(supplied.as_bytes()))
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if expected.len() < 32 || mismatch != 0 {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn telegram_accounts(State(state): State<AdminState>) -> Json<Value> {
    let Some(ch) = state.telegram_channel.as_ref() else {
        return Json(json!({ "error": "telegram channel not enabled" }));
    };
    match ch.list_accounts().await {
        Ok(accounts) => Json(json!({ "accounts": accounts })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn telegram_register_account(
    State(state): State<AdminState>,
    Json(payload): Json<TelegramRegisterRequest>,
) -> Json<Value> {
    let Some(ch) = state.telegram_channel.as_ref() else {
        return Json(json!({ "error": "telegram channel not enabled" }));
    };
    match ch.register_and_start(&payload.bot_token).await {
        Ok(account_id) => Json(json!({ "status": "registered", "account_id": account_id })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn telegram_remove_account(
    State(state): State<AdminState>,
    Path(account_id): Path<String>,
) -> Json<Value> {
    let Some(ch) = state.telegram_channel.as_ref() else {
        return Json(json!({ "error": "telegram channel not enabled" }));
    };
    match ch.remove_account(&account_id).await {
        Ok(()) => Json(json!({ "status": "removed", "account_id": account_id })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn wechat_accounts(State(state): State<AdminState>) -> Json<Value> {
    let Some(ch) = state.wechat_channel.as_ref() else {
        return Json(json!({ "error": "wechat channel not enabled" }));
    };
    match ch.list_accounts().await {
        Ok(accounts) => Json(json!({ "accounts": accounts })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn wechat_remove_account(
    State(state): State<AdminState>,
    Path(account_id): Path<String>,
) -> Json<Value> {
    let Some(ch) = state.wechat_channel.as_ref() else {
        return Json(json!({ "error": "wechat channel not enabled" }));
    };
    match ch.remove_account(&account_id).await {
        Ok(()) => Json(json!({ "status": "removed", "account_id": account_id })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn wechat_qrcode(State(state): State<AdminState>) -> Json<Value> {
    state
        .qr
        .write()
        .await
        .retain(|_, session| session.created_at.elapsed().as_secs() < 300);
    match crate::platforms::wechat::login::start_qr_login(&state.wechat_cfg).await {
        Ok(qr) => {
            use base64::Engine;
            let scan_url = crate::platforms::wechat::qr_terminal::resolve_scan_url(
                &qr.qrcode,
                &qr.qrcode_img_content,
                &state.wechat_cfg.bot_type,
            );
            let payload = crate::platforms::wechat::qr_terminal::decode_payload_from_png_b64(
                &qr.qrcode_img_content,
            )
            .unwrap_or(scan_url);
            let image = (|| -> Option<String> {
                let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
                let image =
                    image::DynamicImage::ImageLuma8(code.render::<image::Luma<u8>>().build());
                let mut bytes = std::io::Cursor::new(Vec::new());
                image.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
                Some(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
                ))
            })();
            let Some(image) = image else {
                return Json(json!({"error":"QR image generation failed"}));
            };
            let qr_session_id = Uuid::new_v4().to_string();
            let body = json!({
                "qr_session_id": qr_session_id,
                "qrcode": qr.qrcode,
                "qrcode_img_content": qr.qrcode_img_content,
                "image": image,
            });
            state.qr.write().await.insert(
                qr_session_id,
                WeChatQrSession {
                    created_at: std::time::Instant::now(),
                    user_id: None,
                    qr,
                    account_id: None,
                    base_url: None,
                    bot_user_id: None,
                },
            );
            Json(body)
        }
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

async fn wechat_qrcode_status(
    State(state): State<AdminState>,
    Query(query): Query<WeChatQrStatusQuery>,
) -> Json<Value> {
    let qr_session_id = match resolve_qr_session_id(&state, &query).await {
        Ok(Some(id)) => id,
        Ok(None) => return Json(json!({ "status": "idle" })),
        Err(e) => return Json(json!({ "status": "error", "error": e })),
    };
    let (qrcode, existing_account_id, existing_base_url, existing_bot_user_id) = {
        let guard = state.qr.read().await;
        let Some(session) = guard.get(&qr_session_id) else {
            return Json(json!({ "status": "idle" }));
        };
        if session.created_at.elapsed().as_secs() >= 300 {
            return Json(json!({"status":"expired"}));
        }
        (
            session.qr.qrcode.clone(),
            session.account_id.clone(),
            session.base_url.clone(),
            session.bot_user_id.clone(),
        )
    };
    if let Some(account_id) = existing_account_id {
        let user_id = state
            .qr
            .read()
            .await
            .get(&qr_session_id)
            .and_then(|s| s.user_id.clone());
        return Json(json!({
            "user_id": user_id,
            "status": "confirmed",
            "qr_session_id": qr_session_id,
            "account_id": account_id,
            "base_url": existing_base_url,
            "bot_user_id": existing_bot_user_id,
        }));
    };
    match crate::platforms::wechat::login::poll_qr_login_status(&qrcode, &state.wechat_cfg).await {
        Ok(s) => {
            let mut account_id = None;
            if s.status == "confirmed" {
                let _registration_guard = state.wechat_registration_lock.lock().await;
                {
                    let guard = state.qr.read().await;
                    if let Some(session) = guard.get(&qr_session_id) {
                        if session.qr.qrcode == qrcode {
                            if let Some(existing) = session.account_id.clone() {
                                return Json(json!({
                                    "user_id": session.user_id,
                                    "status": "confirmed",
                                    "qr_session_id": qr_session_id,
                                    "account_id": existing,
                                    "base_url": session.base_url,
                                    "bot_user_id": session.bot_user_id,
                                }));
                            }
                        } else {
                            return Json(json!({ "status": "stale" }));
                        }
                    } else {
                        return Json(json!({ "status": "idle" }));
                    }
                }
                if let (Some(token), Some(ch)) =
                    (s.bot_token.as_deref(), state.wechat_channel.as_ref())
                {
                    let base_url = s.base_url.as_deref().unwrap_or("");
                    match ch
                        .register_and_start(token, base_url, s.bot_user_id.as_deref())
                        .await
                    {
                        Ok(id) => {
                            account_id = Some(id);
                            tracing::info!(
                                account_id = %account_id.as_ref().unwrap(),
                                base_url,
                                bot_user_id = ?s.bot_user_id,
                                "wechat admin QR login registered account"
                            );
                        }
                        Err(e) => {
                            return Json(json!({
                                "status": "error",
                                "error": e.to_string()
                            }));
                        }
                    }
                } else if state.wechat_channel.is_none() {
                    return Json(json!({
                        "status": "error",
                        "error": "wechat channel not enabled; start gateway with channels.wechat.enabled = true"
                    }));
                }
                if let Some(id) = account_id.clone() {
                    let mut guard = state.qr.write().await;
                    if let Some(session) = guard.get_mut(&qr_session_id) {
                        if session.qr.qrcode == qrcode {
                            session.account_id = Some(id);
                            session.user_id = s.user_id.clone();
                            session.base_url = s.base_url.clone();
                            session.bot_user_id = s.bot_user_id.clone();
                        }
                    }
                }
            }
            Json(json!({
                "user_id": s.user_id,
                "status": s.status,
                "qr_session_id": qr_session_id,
                "account_id": account_id,
                "base_url": s.base_url,
                "bot_user_id": s.bot_user_id,
            }))
        }
        Err(e) => Json(json!({ "status": "error", "error": e.to_string() })),
    }
}

async fn resolve_qr_session_id(
    state: &AdminState,
    query: &WeChatQrStatusQuery,
) -> std::result::Result<Option<String>, String> {
    let requested = query
        .qr_session_id
        .as_deref()
        .or(query.session_id.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let guard = state.qr.read().await;
    if let Some(id) = requested {
        if guard.contains_key(id) {
            return Ok(Some(id.to_string()));
        }
        return Err(format!("qr_session_id not found: {id}"));
    }
    match guard.len() {
        0 => Ok(None),
        1 => Ok(guard.keys().next().cloned()),
        _ => Err(
            "multiple QR sessions exist; pass qr_session_id from /api/channels/wechat/qrcode"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_router_builds_with_axum_08_path_params() {
        let state = AdminState {
            qr: Arc::new(RwLock::new(HashMap::new())),
            wechat_registration_lock: Arc::new(Mutex::new(())),
            wechat_cfg: WeChatConfig::default(),
            telegram_channel: None,
            wechat_channel: None,
        };

        let _router = build_router(state);
    }

    #[tokio::test]
    async fn resolve_qr_session_requires_id_when_multiple_sessions_exist() {
        let state = test_admin_state();
        {
            let mut guard = state.qr.write().await;
            guard.insert("qr_a".into(), test_qr_session("a"));
            guard.insert("qr_b".into(), test_qr_session("b"));
        }

        let err = resolve_qr_session_id(&state, &WeChatQrStatusQuery::default())
            .await
            .unwrap_err();

        assert!(err.contains("multiple QR sessions"));
    }

    #[tokio::test]
    async fn resolve_qr_session_uses_requested_id() {
        let state = test_admin_state();
        {
            let mut guard = state.qr.write().await;
            guard.insert("qr_a".into(), test_qr_session("a"));
            guard.insert("qr_b".into(), test_qr_session("b"));
        }

        let id = resolve_qr_session_id(
            &state,
            &WeChatQrStatusQuery {
                qr_session_id: Some("qr_b".into()),
                session_id: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(id.as_deref(), Some("qr_b"));
    }

    fn test_admin_state() -> AdminState {
        AdminState {
            qr: Arc::new(RwLock::new(HashMap::new())),
            wechat_registration_lock: Arc::new(Mutex::new(())),
            wechat_cfg: WeChatConfig::default(),
            telegram_channel: None,
            wechat_channel: None,
        }
    }

    fn test_qr_session(qrcode: &str) -> WeChatQrSession {
        WeChatQrSession {
            created_at: std::time::Instant::now(),
            user_id: None,
            qr: QrLoginState {
                qrcode: qrcode.to_string(),
                qrcode_img_content: String::new(),
            },
            account_id: None,
            base_url: None,
            bot_user_id: None,
        }
    }
}
