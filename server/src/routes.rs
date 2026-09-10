mod icons;

use crate::{
    AppState,
    auth::{CurrentUser, issue_token, verify_password},
    error::ApiError,
    models::*,
};
use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use serde_json::{Value, json};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(crate::wechat::router())
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route(
            "/subscriptions",
            get(list_subscriptions).post(create_subscription),
        )
        .route(
            "/subscriptions/{id}",
            patch(update_subscription).delete(archive_subscription),
        )
        .route("/bills", get(list_bills))
        .route("/bills/{id}", patch(update_bill))
        .route("/notifications", get(list_notifications))
        .route("/notifications/{id}/read", post(mark_notification_read))
        .route(
            "/notification-settings",
            get(get_notification_settings).put(update_notification_settings),
        )
        .merge(icons::router())
        .route("/exchange-rates", get(exchange_rates))
        .route("/dashboard", get(dashboard))
}

async fn register(
    State(state): State<AppState>,
    Json(input): Json<Credentials>,
) -> Result<(StatusCode, Json<AuthResponse>), ApiError> {
    if !input.email.contains('@') || input.password.len() < 10 {
        return Err(ApiError::Validation(
            "邮箱格式不正确，密码至少需要 10 位".into(),
        ));
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(input.password.as_bytes(), &salt)
        .map_err(|_| ApiError::Internal)?
        .to_string();
    let row: (Uuid, String) = sqlx::query_as(
        "INSERT INTO users (email, password_hash) VALUES (lower($1), $2) RETURNING id, email",
    )
    .bind(&input.email)
    .bind(hash)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref d) if d.is_unique_violation() => {
            ApiError::Validation("该邮箱已注册".into())
        }
        _ => e.into(),
    })?;
    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            access_token: issue_token(row.0, &state)?,
            user_id: row.0,
            email: row.1,
        }),
    ))
}

async fn login(
    State(state): State<AppState>,
    Json(input): Json<Credentials>,
) -> Result<Json<AuthResponse>, ApiError> {
    let row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, email, password_hash FROM users WHERE email = lower($1) AND deleted_at IS NULL",
    )
    .bind(&input.email)
    .fetch_optional(&state.db)
    .await?;
    let (id, email, _hash) = row
        .filter(|r| verify_password(&r.2, &input.password))
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(AuthResponse {
        access_token: issue_token(id, &state)?,
        user_id: id,
        email,
    }))
}

async fn list_subscriptions(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<Subscription>>, ApiError> {
    let rows = sqlx::query_as::<_, Subscription>("SELECT s.id,s.name,s.plan_name,s.amount,s.currency,s.cadence_unit,s.cadence_interval,s.next_billing_date,s.anchor_day,s.status,s.category,s.payment_method,s.notes,s.icon_url,coalesce((SELECT array_agg(r.days_before ORDER BY r.days_before DESC) FROM subscription_reminders r WHERE r.subscription_id=s.id),ARRAY[]::integer[]) reminder_offsets,s.created_at,s.updated_at FROM subscriptions s WHERE s.user_id=$1 AND s.status <> 'archived' ORDER BY s.next_billing_date")
        .bind(user.0).fetch_all(&state.db).await?;
    Ok(Json(rows))
}

async fn create_subscription(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<CreateSubscription>,
) -> Result<(StatusCode, Json<Subscription>), ApiError> {
    let mut tx = state.db.begin().await?;
    let row = crate::subscriptions::create(&mut tx, user.0, input).await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_subscription(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(patch): Json<Value>,
) -> Result<Json<Subscription>, ApiError> {
    if patch.get("name").is_some() {
        let input: CreateSubscription = serde_json::from_value(patch)
            .map_err(|_| ApiError::Validation("订阅信息不完整".into()))?;
        let mut tx = state.db.begin().await?;
        let row = crate::subscriptions::update(&mut tx, user.0, id, input).await?;
        tx.commit().await?;
        return Ok(Json(row));
    }
    let status = patch
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("active");
    if !["active", "paused", "cancelled"].contains(&status) {
        return Err(ApiError::Validation("无效订阅状态".into()));
    }
    let row = sqlx::query_as::<_, Subscription>("UPDATE subscriptions SET status=$3, updated_at=now() WHERE id=$1 AND user_id=$2 AND status <> 'archived' RETURNING id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,status,category,payment_method,notes,icon_url,created_at,updated_at")
        .bind(id).bind(user.0).bind(status).fetch_optional(&state.db).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(row))
}

async fn archive_subscription(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query(
        "UPDATE subscriptions SET status='archived', updated_at=now() WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user.0)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_bills(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<Bill>>, ApiError> {
    let rows = sqlx::query_as::<_, Bill>("SELECT b.id,b.subscription_id,s.name subscription_name,b.amount,b.currency,b.due_date,b.status,b.base_amount,b.base_currency,b.created_at FROM bills b JOIN subscriptions s ON s.id=b.subscription_id WHERE b.user_id=$1 ORDER BY b.due_date DESC LIMIT 200").bind(user.0).fetch_all(&state.db).await?;
    Ok(Json(rows))
}

async fn update_bill(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(patch): Json<Value>,
) -> Result<StatusCode, ApiError> {
    let status = patch
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::Validation("缺少账单状态".into()))?;
    if !["estimated", "paid", "skipped", "refunded"].contains(&status) {
        return Err(ApiError::Validation("无效账单状态".into()));
    }
    let result =
        sqlx::query("UPDATE bills SET status=$3, updated_at=now() WHERE id=$1 AND user_id=$2")
            .bind(id)
            .bind(user.0)
            .bind(status)
            .execute(&state.db)
            .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_notifications(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<Notification>>, ApiError> {
    Ok(Json(sqlx::query_as::<_, Notification>("SELECT id,title,body,kind,scheduled_for,read_at FROM notifications WHERE user_id=$1 ORDER BY scheduled_for DESC LIMIT 100").bind(user.0).fetch_all(&state.db).await?))
}

async fn mark_notification_read(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    sqlx::query("UPDATE notifications SET read_at=now() WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_notification_settings(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<NotificationSettings>, ApiError> {
    let settings = sqlx::query_as::<_, NotificationSettings>(
        "SELECT telegram_enabled,coalesce(length(telegram_bot_token)>0,false) telegram_bot_token_configured,telegram_chat_id FROM notification_settings WHERE user_id=$1",
    )
    .bind(user.0)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_default();
    Ok(Json(settings))
}

async fn update_notification_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<UpdateNotificationSettings>,
) -> Result<Json<NotificationSettings>, ApiError> {
    let existing: Option<bool> = sqlx::query_scalar(
        "SELECT coalesce(length(telegram_bot_token)>0,false) FROM notification_settings WHERE user_id=$1",
    )
    .bind(user.0)
    .fetch_optional(&state.db)
    .await?;
    let has_telegram_token = input
        .telegram_bot_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || existing.unwrap_or(false);
    if input.telegram_enabled && (!has_telegram_token || input.telegram_chat_id.trim().is_empty()) {
        return Err(ApiError::Validation(
            "启用 Telegram 前需要填写 Bot Token 和 Chat ID".into(),
        ));
    }
    sqlx::query("INSERT INTO notification_settings (user_id,telegram_enabled,telegram_bot_token,telegram_chat_id) VALUES ($1,$2,nullif($3,''),$4) ON CONFLICT (user_id) DO UPDATE SET telegram_enabled=excluded.telegram_enabled,telegram_bot_token=coalesce(excluded.telegram_bot_token,notification_settings.telegram_bot_token),telegram_chat_id=excluded.telegram_chat_id,updated_at=now()")
        .bind(user.0)
        .bind(input.telegram_enabled)
        .bind(input.telegram_bot_token.unwrap_or_default().trim().to_owned())
        .bind(input.telegram_chat_id.trim())
        .execute(&state.db)
        .await?;
    get_notification_settings(user, State(state)).await
}

async fn dashboard(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM subscriptions WHERE user_id=$1 AND status='active'",
    )
    .bind(user.0)
    .fetch_one(&state.db)
    .await?;
    let upcoming: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE user_id=$1 AND status='active' AND next_billing_date <= current_date + 14").bind(user.0).fetch_one(&state.db).await?;
    Ok(Json(
        json!({"active_subscriptions":active,"upcoming_14_days":upcoming}),
    ))
}

async fn exchange_rates(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<(String, rust_decimal::Decimal, chrono::NaiveDate)> = sqlx::query_as("SELECT quote_currency,rate,rate_date FROM exchange_rates WHERE base_currency='EUR' AND rate_date=(SELECT max(rate_date) FROM exchange_rates) ORDER BY quote_currency")
        .fetch_all(&state.db).await?;
    Ok(Json(
        json!({"base":"EUR","rates":rows.into_iter().map(|(currency,rate,date)|json!({"currency":currency,"rate":rate,"date":date})).collect::<Vec<_>>()}),
    ))
}
