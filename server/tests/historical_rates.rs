use chrono::NaiveDate;
use renuxa_server::{AppState, exchange_rates::apply_history};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Requires DATABASE_URL with permission to create isolated test databases"]
async fn backfills_historical_rates_without_changing_original_bills(pool: PgPool) {
    let user:Uuid=sqlx::query_scalar("INSERT INTO users(email,password_hash) VALUES('historical@test.invalid','unused') RETURNING id").fetch_one(&pool).await.unwrap();
    let due: NaiveDate = "2026-08-23".parse().unwrap();
    let trading: NaiveDate = "2026-08-21".parse().unwrap();
    let sub:Uuid=sqlx::query_scalar("INSERT INTO subscriptions(user_id,name,amount,currency,cadence_unit,next_billing_date,anchor_day) VALUES($1,'Historical',10,'USD','month',$2,23) RETURNING id").bind(user).bind(due).fetch_one(&pool).await.unwrap();
    for (currency, status) in [("USD", "paid"), ("CNY", "refunded")] {
        sqlx::query("INSERT INTO bills(user_id,subscription_id,amount,currency,due_date,status) VALUES($1,$2,10,$3,$4,$5)").bind(user).bind(sub).bind(currency).bind(due).bind(status).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO bills(user_id,subscription_id,amount,currency,due_date,status,base_amount,base_currency,exchange_rate,exchange_rate_date) VALUES($1,$2,10,'USD',$3,'skipped',99,'CNY',9.9,$4)").bind(user).bind(sub).bind(due).bind(trading).execute(&pool).await.unwrap();
    let state = AppState {
        db: pool.clone(),
        jwt_secret: Arc::from("isolated-history-key"),
        http: reqwest::Client::new(),
    };
    let rates = vec![
        ("EUR".into(), Decimal::ONE),
        ("USD".into(), "1.2".parse().unwrap()),
        ("CNY".into(), "8.4".parse().unwrap()),
    ];
    assert_eq!(
        apply_history(&state, due, trading, &rates).await.unwrap(),
        2
    );
    assert_eq!(
        apply_history(&state, due, trading, &rates).await.unwrap(),
        0
    );
    let rows: Vec<(String, Decimal, Decimal, NaiveDate)> = sqlx::query_as(
        "SELECT status,amount,base_amount,exchange_rate_date FROM bills ORDER BY status",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("paid".into(), 10.into(), 70.into(), trading),
            ("refunded".into(), 10.into(), 10.into(), trading),
            ("skipped".into(), 10.into(), 99.into(), trading)
        ]
    );
    // Invalid snapshots fail as one transaction instead of partially populating the cache.
    let invalid = vec![
        ("EUR".into(), Decimal::ONE),
        ("BAD".into(), "999999999999999".parse().unwrap()),
    ];
    assert!(apply_history(&state, due, due, &invalid).await.is_err());
    let partial: i64 = sqlx::query_scalar("SELECT count(*) FROM exchange_rates WHERE rate_date=$1")
        .bind(due)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(partial, 0);
}
