use axum::{Json, Router, routing::get};
use renuxa_server::{AppState, app, auth::issue_token};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, task)
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires DATABASE_URL with permission to create isolated test databases"]
async fn qr_binding_is_owned_expiring_and_idempotent(pool: PgPool) {
    let gateway = Router::new()
        .route("/api/channels/wechat/qrcode", get(|| async { Json(json!({"qr_session_id":Uuid::new_v4(),"image":"data:image/png;base64,test"})) }))
        .route("/api/channels/wechat/qrcode/status", get(|| async { Json(json!({"status":"confirmed","account_id":"bot-account","bot_user_id":"bot","user_id":"scanner"})) }));
    let (gateway_url, gateway_task) = serve(gateway).await;
    // This separate integration test binary owns its environment.
    unsafe {
        std::env::set_var("WECHAT_ENABLED", "true");
        std::env::set_var(
            "WECHAT_GATEWAY_TOKEN",
            "test-gateway-token-at-least-32-characters",
        );
        std::env::set_var("WECHAT_GATEWAY_URL", gateway_url);
    }
    let state = AppState {
        db: pool.clone(),
        jwt_secret: Arc::from("qr-test-secret"),
        http: reqwest::Client::new(),
    };
    let mut tokens = Vec::new();
    let mut users = Vec::new();
    for email in ["qr-one@test.invalid", "qr-two@test.invalid"] {
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users(email,password_hash) VALUES($1,'unused') RETURNING id",
        )
        .bind(email)
        .fetch_one(&pool)
        .await
        .unwrap();
        tokens.push(issue_token(user, &state).unwrap());
        users.push(user);
    }
    let (base, api_task) = serve(app(state)).await;
    let url = format!("{base}/api/integrations/wechat/qrcode");
    let http = reqwest::Client::new();
    assert_eq!(
        http.post(&url)
            .json(&json!({"timezone":"UTC"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(
        http.post(&url)
            .bearer_auth(&tokens[0])
            .json(&json!({"timezone":"UTC"}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let status_url = format!("{url}/status");
    let other: Value = http
        .get(&status_url)
        .bearer_auth(&tokens[1])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(other["status"], "expired");
    for _ in 0..2 {
        let result: Value = http
            .get(&status_url)
            .bearer_auth(&tokens[0])
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(result["status"], "confirmed");
    }
    let sender: String =
        sqlx::query_scalar("SELECT sender_id FROM wechat_bindings WHERE user_id=$1")
            .bind(users[0])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sender, "scanner");
    assert!(
        http.post(&url)
            .bearer_auth(&tokens[1])
            .json(&json!({"timezone":"UTC"}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        http.get(&status_url)
            .bearer_auth(&tokens[1])
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    sqlx::query("UPDATE wechat_qr_sessions SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let expired: Value = http
        .get(&status_url)
        .bearer_auth(&tokens[1])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(expired["status"], "expired");
    api_task.abort();
    gateway_task.abort();
}
