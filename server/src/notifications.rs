use crate::{AppState, auth::CurrentUser, error::ApiError, models::TestNotificationSettings};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use std::time::Duration;

pub async fn test_notification_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<TestNotificationSettings>,
) -> Result<StatusCode, ApiError> {
    let saved_token = if non_empty(input.telegram_bot_token.as_deref()).is_none() {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT telegram_bot_token FROM notification_settings WHERE user_id=$1",
        )
        .bind(user.0)
        .fetch_optional(&state.db)
        .await?
        .flatten()
    } else {
        None
    };
    let (token, chat_id) = test_credentials(&input, saved_token.as_deref())?;
    send_telegram(
        &state.http,
        token,
        chat_id,
        "Renuxa 通知测试\n\n这是一条测试消息，Telegram 通知配置可用。",
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn test_credentials<'a>(
    input: &'a TestNotificationSettings,
    saved_token: Option<&'a str>,
) -> Result<(&'a str, &'a str), ApiError> {
    let token = non_empty(input.telegram_bot_token.as_deref())
        .or_else(|| non_empty(saved_token))
        .ok_or_else(|| ApiError::Validation("测试 Telegram 前需要填写 Bot Token".into()))?;
    let chat_id = non_empty(Some(&input.telegram_chat_id))
        .ok_or_else(|| ApiError::Validation("测试 Telegram 前需要填写 Chat ID".into()))?;
    Ok((token, chat_id))
}

pub async fn send_telegram(
    http: &reqwest::Client,
    token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), ApiError> {
    let endpoint = format!("https://api.telegram.org/bot{token}/sendMessage");
    send_message(http, &endpoint, chat_id, text, Duration::from_secs(10)).await
}

#[derive(Deserialize)]
struct TelegramResponse {
    ok: bool,
    error_code: Option<u16>,
}

async fn send_message(
    http: &reqwest::Client,
    endpoint: &str,
    chat_id: &str,
    text: &str,
    timeout: Duration,
) -> Result<(), ApiError> {
    // Request errors contain the bot token in the URL; expose only fixed messages.
    let response = http
        .post(endpoint)
        .timeout(timeout)
        .json(&serde_json::json!({ "chat_id": chat_id, "text": text }))
        .send()
        .await
        .map_err(transport_error)?;
    if !response.status().is_success() {
        return Err(delivery_error(response.status().as_u16()));
    }
    let body = response.json::<TelegramResponse>().await.map_err(|error| {
        if error.is_timeout() {
            transport_error(error)
        } else {
            ApiError::NotificationDelivery("Telegram 返回了无效响应，请稍后重试")
        }
    })?;
    if !body.ok {
        return Err(delivery_error(body.error_code.unwrap_or(502)));
    }
    Ok(())
}

fn transport_error(error: reqwest::Error) -> ApiError {
    ApiError::NotificationDelivery(if error.is_timeout() {
        "Telegram 请求超时，请检查服务端网络后重试"
    } else {
        "无法连接 Telegram，请检查服务端网络后重试"
    })
}

fn delivery_error(code: u16) -> ApiError {
    ApiError::NotificationDelivery(match code {
        400 => "Telegram 无法发送消息，请检查 Chat ID，并确认已向机器人发送 /start 或将其加入群组",
        401 | 404 => "Telegram Bot Token 无效，请检查后重试",
        403 => "Telegram 拒绝发送消息，请确认机器人未被屏蔽，且有权向目标会话发送消息",
        429 => "Telegram 请求过于频繁，请稍后重试",
        _ => "Telegram 服务暂不可用，请稍后重试",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::post};
    use serde_json::{Value, json};

    #[test]
    fn test_uses_draft_credentials_and_falls_back_only_for_blank_tokens() {
        let mut input = TestNotificationSettings {
            telegram_bot_token: Some(" draft-token ".into()),
            telegram_chat_id: " -123456 ".into(),
        };
        let (token, chat) = test_credentials(&input, Some("saved-token")).unwrap();
        assert!(token == "draft-token");
        assert_eq!(chat, "-123456");
        for blank in [None, Some(String::new()), Some("  ".into())] {
            input.telegram_bot_token = blank;
            assert!(test_credentials(&input, Some("saved-token")).unwrap().0 == "saved-token");
            assert!(matches!(
                test_credentials(&input, None),
                Err(ApiError::Validation(_))
            ));
        }
        input.telegram_chat_id = "  ".into();
        assert!(matches!(
            test_credentials(&input, Some("saved-token")),
            Err(ApiError::Validation(_))
        ));
    }

    async fn mock_server(
        status: StatusCode,
        body: &'static str,
        delay: Duration,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let router = Router::new().route(
            "/sendMessage",
            post(move |Json(payload): Json<Value>| async move {
                assert_eq!(
                    payload,
                    json!({"chat_id":"-123456", "text":"notification test"})
                );
                tokio::time::sleep(delay).await;
                (status, body)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/sendMessage", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (endpoint, task)
    }

    #[tokio::test]
    async fn checks_http_status_and_telegram_result_without_exposing_response_text() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (status, body, expected) in [
            (200, r#"{"ok":true}"#, None),
            (
                200,
                r#"{"ok":false,"error_code":400,"description":"private-upstream-detail"}"#,
                Some("Chat ID"),
            ),
            (401, "private-upstream-detail", Some("Bot Token")),
            (403, "private-upstream-detail", Some("拒绝")),
            (429, "private-upstream-detail", Some("频繁")),
            (500, "private-upstream-detail", Some("暂不可用")),
            (200, "private-upstream-detail", Some("无效响应")),
            (200, r#"{"ok":false}"#, Some("暂不可用")),
        ] {
            let (endpoint, task) =
                mock_server(StatusCode::from_u16(status).unwrap(), body, Duration::ZERO).await;
            let result = send_message(
                &client,
                &endpoint,
                "-123456",
                "notification test",
                Duration::from_secs(2),
            )
            .await;
            task.abort();
            match expected {
                None => assert!(result.is_ok()),
                Some(expected) => {
                    let message = result.unwrap_err().to_string();
                    assert!(message.contains(expected));
                    assert!(!message.contains("private-upstream-detail"));
                    assert!(!message.contains(&endpoint));
                }
            }
        }
    }

    #[tokio::test]
    async fn reports_timeouts_and_connection_failures_without_urls() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let (endpoint, task) =
            mock_server(StatusCode::OK, r#"{"ok":true}"#, Duration::from_secs(1)).await;
        let error = send_message(
            &client,
            &endpoint,
            "-123456",
            "notification test",
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("超时"));
        assert!(!error.to_string().contains(&endpoint));
        task.abort();
        let _ = task.await;
        let error = send_message(
            &client,
            &endpoint,
            "-123456",
            "notification test",
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("无法连接"));
        assert!(!error.to_string().contains(&endpoint));
    }
}
