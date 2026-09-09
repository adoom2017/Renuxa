use axum::{Json, Router, routing::post};
use renuxa_server::{AppState, app, auth::issue_token};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, task)
}

async fn model(Json(body): Json<Value>) -> Json<Value> {
    let text = body["messages"][1]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    let prompt = body["messages"][0]["content"].as_str().unwrap();
    let old = prompt.split("当前草稿：").nth(1).unwrap();
    assert!(serde_json::from_str::<Value>(old).unwrap().is_object());
    let mut fields;
    if text == "invalid-json" {
        return Json(json!({"choices":[{"message":{"content":"broken"}}]}));
    }
    if text == "missing" {
        fields = json!({"name":"Netflix"});
    } else {
        fields = json!({"name":"Netflix","amount":"10.50","currency":"USD","cadence_unit":"month","cadence_interval":1,"next_billing_date":"2027-01-31"});
    }
    if text == "modify" {
        fields["amount"] = json!("20.25");
    }
    let extracted = json!({"fields":fields,"question":if text=="missing"{"请补充金额、币种、周期及日期"}else{""},"multiple":text=="multiple"});
    Json(json!({"choices":[{"message":{"content":extracted.to_string()}}]}))
}

struct Client {
    http: reqwest::Client,
    url: String,
    jwt: String,
}
impl Client {
    async fn binding_code(&self) -> String {
        self.http
            .post(format!("{}/api/integrations/wechat/binding-code", self.url))
            .bearer_auth(&self.jwt)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["code"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn message(&self, sender: &str, id: &str, text: &str) -> String {
        self.request(sender, id, vec![json!({"type":"text","text":text})], false)
            .await
    }
    async fn request(&self, sender: &str, id: &str, content: Vec<Value>, group: bool) -> String {
        self.http.post(format!("{}/api/integrations/wechat/process",self.url)).bearer_auth("test-gateway-token-at-least-32-characters").json(&json!({"channel":"wechat:bot","session_id":format!("wechat:bot:dm:{sender}"),"user_id":sender,"meta":{"message_id":id,"account_id":"bot","is_group":group},"input":[{"role":"user","content":content}]})).send().await.unwrap().error_for_status().unwrap().text().await.unwrap()
    }
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires DATABASE_URL with permission to create isolated test databases"]
async fn binding_draft_and_concurrent_confirmation(pool: PgPool) {
    let (model_url, model_task) = serve(Router::new().route("/", post(model))).await;
    // This integration test is a separate binary with a single test and owns its environment.
    unsafe {
        std::env::set_var("WECHAT_ENABLED", "true");
        std::env::set_var(
            "WECHAT_GATEWAY_TOKEN",
            "test-gateway-token-at-least-32-characters",
        );
        std::env::set_var("WECHAT_MODEL_URL", model_url);
        std::env::set_var("WECHAT_MODEL_NAME", "mock");
        std::env::set_var("WECHAT_MODEL_API_KEY", "test");
    }
    let state = AppState {
        db: pool.clone(),
        jwt_secret: Arc::from("test-secret"),
        http: reqwest::Client::new(),
    };
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users(email,password_hash) VALUES('one@test.invalid','unused') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: Uuid = sqlx::query_scalar(
        "INSERT INTO users(email,password_hash) VALUES('two@test.invalid','unused') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let jwt = issue_token(user, &state).unwrap();
    let jwt2 = issue_token(other, &state).unwrap();
    let (url, api_task) = serve(app(state)).await;
    let client = Client {
        http: reqwest::Client::new(),
        url: url.clone(),
        jwt,
    };
    let client2 = Client {
        http: reqwest::Client::new(),
        url,
        jwt: jwt2,
    };
    let code = client.binding_code().await;
    assert!(
        client
            .message("alice", "bind", &code)
            .await
            .contains("绑定成功")
    );
    assert!(client.message("bob", "reuse", &code).await.contains("无效"));
    assert!(
        client
            .message("stranger", "unbound", "确认")
            .await
            .contains("绑定码")
    );
    let code2 = client2.binding_code().await;
    assert!(
        client2
            .message("bob", "bind2", &code2)
            .await
            .contains("绑定成功")
    );
    assert!(
        client
            .request(
                "alice",
                "group",
                vec![json!({"type":"text","text":"complete"})],
                true
            )
            .await
            .contains("仅支持")
    );
    assert!(
        client
            .message("alice", "missing", "missing")
            .await
            .contains("请补充")
    );
    assert!(
        client
            .message("alice", "early", "确认")
            .await
            .contains("尚未完整")
    );
    assert!(
        client
            .message("alice", "complete", "complete")
            .await
            .contains("订阅预览")
    );
    let before:Value=sqlx::query_scalar("SELECT fields FROM wechat_drafts d JOIN wechat_bindings b ON b.id=d.binding_id WHERE b.user_id=$1").bind(user).fetch_one(&pool).await.unwrap();
    assert!(
        client
            .message("alice", "failed", "invalid-json")
            .await
            .contains("草稿已保留")
    );
    let after:Value=sqlx::query_scalar("SELECT fields FROM wechat_drafts d JOIN wechat_bindings b ON b.id=d.binding_id WHERE b.user_id=$1").bind(user).fetch_one(&pool).await.unwrap();
    assert_eq!(before, after);
    // Reset only the abuse window so the scenario can exercise more than 20 messages.
    sqlx::query("DELETE FROM wechat_rate_limits")
        .execute(&pool)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        client.message("alice", "confirm-1", "确认"),
        client.message("alice", "confirm-2", "确认")
    );
    assert!(a.contains("已添加订阅"));
    assert!(b.contains("已添加订阅"));
    assert!(
        client
            .message("alice", "confirm-1", "确认")
            .await
            .contains("已添加订阅")
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE user_id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE user_id=$1")
        .bind(other)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        client2
            .message("bob", "isolated", "确认")
            .await
            .contains("没有待确认")
    );
    client.message("alice", "new", "complete").await;
    assert!(
        client
            .message("alice", "duplicate", "确认")
            .await
            .contains("仍然添加")
    );
    assert!(
        client
            .message("alice", "force", "仍然添加")
            .await
            .contains("已添加订阅")
    );
    client.message("alice", "modify", "modify").await;
    assert!(
        client
            .message("alice", "cancel", "取消")
            .await
            .contains("已取消")
    );
    assert!(
        client
            .message("alice", "cancel-confirm", "确认")
            .await
            .contains("没有待确认")
    );
    client.message("alice", "expire", "complete").await;
    sqlx::query("UPDATE wechat_drafts SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        client
            .message("alice", "expired", "确认")
            .await
            .contains("过期")
    );
    client.message("alice", "unbind-draft", "complete").await;
    client
        .http
        .delete(format!("{}/api/integrations/wechat/binding", client.url))
        .bearer_auth(&client.jwt)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(
        client
            .message("alice", "unbound-confirm", "确认")
            .await
            .contains("绑定码")
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE user_id=$1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    sqlx::query("DELETE FROM wechat_rate_limits")
        .execute(&pool)
        .await
        .unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    use base64::Engine;
    let image_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png.into_inner())
    );
    for id in ["screenshot-1", "screenshot-2"] {
        assert!(
            client2
                .request(
                    "bob",
                    id,
                    vec![json!({"type":"image","image_url":image_url})],
                    false
                )
                .await
                .contains("订阅预览")
        );
    }
    let version:i32=sqlx::query_scalar("SELECT version FROM wechat_drafts d JOIN wechat_bindings b ON b.id=d.binding_id WHERE b.user_id=$1").bind(other).fetch_one(&pool).await.unwrap();
    assert_eq!(
        version, 2,
        "continuous images must not be deduplicated by placeholder text"
    );
    assert!(
        client2
            .message("bob", "image-modify", "modify")
            .await
            .contains("20.25")
    );
    assert!(
        client2
            .message("bob", "image-confirm", "确认")
            .await
            .contains("20.25")
    );
    let invalid = client2.http.post(format!("{}/api/integrations/wechat/process", client2.url)).bearer_auth("test-gateway-token-at-least-32-characters").json(&json!({"channel":"wechat:bot","session_id":"session","user_id":"bob","meta":{"message_id":"invalid-image","account_id":"bot","is_group":false},"input":[{"role":"user","content":[{"type":"image","image_url":"data:image/png;base64,aGVsbG8="}]}]})).send().await.unwrap();
    assert_eq!(invalid.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    // Rebind Alice to a different Renuxa user; an old result must not leak across ownership.
    client2
        .http
        .delete(format!("{}/api/integrations/wechat/binding", client2.url))
        .bearer_auth(&client2.jwt)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let new_code = client2.binding_code().await;
    assert!(
        client2
            .message("alice", "rebind", &new_code)
            .await
            .contains("绑定成功")
    );
    assert!(
        client2
            .message("alice", "confirm-1", "确认")
            .await
            .contains("已解除的绑定")
    );
    api_task.abort();
    model_task.abort();
}
