use chrono::{Duration, NaiveDate};
use renuxa_server::{AppState, app, auth::issue_token, subscriptions::billing_plan};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires DATABASE_URL with permission to create isolated test databases"]
async fn start_date_generates_paid_history_and_preserves_manual_records(pool: PgPool) {
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users(email,password_hash) VALUES('start@test.invalid','unused') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let today: NaiveDate =
        sqlx::query_scalar("SELECT (now() AT TIME ZONE timezone)::date FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap();
    let state = AppState {
        db: pool.clone(),
        jwt_secret: Arc::from("isolated-test-signing-key"),
        http: reqwest::Client::new(),
    };
    let token = issue_token(user, &state).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    let client = reqwest::Client::new();
    let start = today - Duration::days(205);
    let mut input = json!({"name":"Daily history","amount":"12.50","currency":"CNY","cadence_unit":"day","start_date":start});
    let response = client
        .post(format!("{base}/api/subscriptions"))
        .bearer_auth(&token)
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let sub: Value = response.json().await.unwrap();
    assert_eq!(sub["start_date"], start.to_string());
    assert_eq!(
        sub["next_billing_date"],
        (today + Duration::days(1)).to_string()
    );
    let id = sub["id"].as_str().unwrap();
    let page: Vec<Value> = client
        .get(format!("{base}/api/bills"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: Vec<Value> = client
        .get(format!("{base}/api/bills?offset=200"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page.len(), 200);
    assert_eq!(second.len(), 6);
    assert!(page.iter().all(|bill| bill["status"] == "paid"));
    for _ in 0..2 {
        let response = client
            .patch(format!("{base}/api/subscriptions/{id}"))
            .bearer_auth(&token)
            .json(&input)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM bills")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 206);
    let manual = second.last().unwrap()["id"].as_str().unwrap();
    assert_eq!(
        client
            .patch(format!("{base}/api/bills/{manual}"))
            .bearer_auth(&token)
            .json(&json!({"status":"skipped"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    input["start_date"] = json!(today - Duration::days(2));
    assert_eq!(
        client
            .patch(format!("{base}/api/subscriptions/{id}"))
            .bearer_auth(&token)
            .json(&input)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT status,source FROM bills ORDER BY due_date")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], ("skipped".into(), "manual".into()));
    // A price-only edit never rewrites historical paid amounts.
    input["amount"] = json!("99.00");
    assert_eq!(
        client
            .patch(format!("{base}/api/subscriptions/{id}"))
            .bearer_auth(&token)
            .json(&input)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let unchanged: bool = sqlx::query_scalar("SELECT bool_and(amount=12.50) FROM bills")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(unchanged);
    // Old clients remain compatible and don't silently create paid history.
    let legacy = json!({"name":"Legacy","amount":"8","currency":"CNY","cadence_unit":"month","next_billing_date":today+Duration::days(10)});
    let old: Value = client
        .post(format!("{base}/api/subscriptions"))
        .bearer_auth(&token)
        .json(&legacy)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(old["start_date"].is_null());
    let mut backfill = legacy.clone();
    backfill["start_date"] = json!(today - Duration::days(75));
    let migrated: Value = client
        .patch(format!(
            "{base}/api/subscriptions/{}",
            old["id"].as_str().unwrap()
        ))
        .bearer_auth(&token)
        .json(&backfill)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let plan = billing_plan(today - Duration::days(75), "month", 1, today).unwrap();
    assert_eq!(migrated["next_billing_date"], plan.next_date.to_string());
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM bills WHERE subscription_id=$1")
        .bind(Uuid::parse_str(old["id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(history as usize, plan.paid_dates.len());
    // Completed one-off subscriptions cannot generate another renewal.
    let once = json!({"name":"Once","amount":"1","currency":"CNY","cadence_unit":"once","start_date":today});
    let once: Value = client
        .post(format!("{base}/api/subscriptions"))
        .bearer_auth(&token)
        .json(&once)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(once["status"], "cancelled");

    // Simulate reaching the first renewal; no external services are contacted.
    let future = json!({"name":"Future worker","amount":"3","currency":"CNY","cadence_unit":"day","start_date":today+Duration::days(1)});
    let future: Value = client
        .post(format!("{base}/api/subscriptions"))
        .bearer_auth(&token)
        .json(&future)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let future_id = Uuid::parse_str(future["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE subscriptions SET start_date=$2,next_billing_date=$2 WHERE id=$1")
        .bind(future_id)
        .bind(today)
        .execute(&pool)
        .await
        .unwrap();
    let worker_state = AppState {
        db: pool.clone(),
        jwt_secret: Arc::from("isolated-worker-key"),
        http: reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap(),
    };
    renuxa_server::worker::run_cycle(&worker_state)
        .await
        .unwrap();
    renuxa_server::worker::run_cycle(&worker_state)
        .await
        .unwrap();
    let worker_bills: Vec<(String, String)> =
        sqlx::query_as("SELECT status,source FROM bills WHERE subscription_id=$1")
            .bind(future_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(worker_bills, vec![("paid".into(), "schedule".into())]);
    task.abort();
}
