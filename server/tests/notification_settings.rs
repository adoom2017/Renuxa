use axum::{Router, http::StatusCode};
use renuxa_server::{AppState, app, auth::issue_token};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires DATABASE_URL with permission to create isolated test databases"]
async fn notification_tests_are_authenticated_account_scoped_and_do_not_save(pool: PgPool) {
    let first: Uuid = sqlx::query_scalar("INSERT INTO users(email,password_hash) VALUES('notification-one@test.invalid','unused') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let second: Uuid = sqlx::query_scalar("INSERT INTO users(email,password_hash) VALUES('notification-two@test.invalid','unused') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO notification_settings(user_id,telegram_enabled,telegram_bot_token,telegram_chat_id) VALUES($1,false,'saved-fixture','saved-chat')")
        .bind(first).execute(&pool).await.unwrap();

    // Reject Telegram connections locally so this test never delivers a real message.
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let proxy_task = tokio::spawn(async move {
        axum::serve(
            proxy,
            Router::new().fallback(|| async { StatusCode::BAD_GATEWAY }),
        )
        .await
        .unwrap();
    });
    let state = AppState {
        db: pool.clone(),
        jwt_secret: Arc::from("isolated-notification-signing-key"),
        http: reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(proxy_url).unwrap())
            .build()
            .unwrap(),
    };
    let first_auth = issue_token(first, &state).unwrap();
    let second_auth = issue_token(second, &state).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!(
        "http://{}/api/notification-settings",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    let response = http
        .post(format!("{base}/test"))
        .json(&json!({"telegram_chat_id":"123456"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    for (auth, input, status, message) in [
        (
            &first_auth,
            json!({"telegram_chat_id":"  "}),
            422,
            "Chat ID",
        ),
        (
            &second_auth,
            json!({"telegram_bot_token":null,"telegram_chat_id":"123456"}),
            422,
            "Bot Token",
        ),
        (
            &second_auth,
            json!({"telegram_bot_token":"draft-fixture","telegram_chat_id":" "}),
            422,
            "Chat ID",
        ),
        (
            &first_auth,
            json!({"telegram_bot_token":"  ","telegram_chat_id":"123456"}),
            502,
            "无法连接",
        ),
        (
            &second_auth,
            json!({"telegram_bot_token":"draft-fixture","telegram_chat_id":"123456"}),
            502,
            "无法连接",
        ),
    ] {
        let response = http
            .post(format!("{base}/test"))
            .bearer_auth(auth)
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        let body: Value = response.json().await.unwrap();
        assert!(body["error"]["message"].as_str().unwrap().contains(message));
        assert!(!body.to_string().contains("fixture"));
    }

    let settings: Value = http
        .get(&base)
        .bearer_auth(first_auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        settings,
        json!({"telegram_enabled":false,"telegram_bot_token_configured":true,"telegram_chat_id":"saved-chat"})
    );
    let unchanged: bool = sqlx::query_scalar("SELECT telegram_bot_token='saved-fixture' AND telegram_chat_id='saved-chat' AND NOT telegram_enabled FROM notification_settings WHERE user_id=$1")
        .bind(first).fetch_one(&pool).await.unwrap();
    assert!(unchanged);
    let settings_count: i64 = sqlx::query_scalar("SELECT count(*) FROM notification_settings")
        .fetch_one(&pool)
        .await
        .unwrap();
    let notice_count: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(settings_count, 1);
    assert_eq!(notice_count, 0);
    server.abort();
    proxy_task.abort();
}
