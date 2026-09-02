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
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use chrono::Datelike;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
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
        .route("/icons/search", get(search_icons))
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
    let rows = sqlx::query_as::<_, Subscription>("SELECT id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,status,category,payment_method,notes,icon_url,created_at,updated_at FROM subscriptions WHERE user_id=$1 AND status <> 'archived' ORDER BY next_billing_date")
        .bind(user.0).fetch_all(&state.db).await?;
    Ok(Json(rows))
}

async fn create_subscription(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<CreateSubscription>,
) -> Result<(StatusCode, Json<Subscription>), ApiError> {
    if input.name.trim().is_empty() || input.amount.is_sign_negative() || input.currency.len() != 3
    {
        return Err(ApiError::Validation("请检查名称、金额和货币".into()));
    }
    let interval = input.cadence_interval.unwrap_or(1).clamp(1, 120);
    let mut tx = state.db.begin().await?;
    let row = sqlx::query_as::<_, Subscription>("INSERT INTO subscriptions (user_id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,category,payment_method,notes,icon_url) VALUES ($1,$2,$3,$4,upper($5),$6,$7,$8,$9,$10,$11,$12,$13) RETURNING id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,status,category,payment_method,notes,icon_url,created_at,updated_at")
        .bind(user.0).bind(input.name.trim()).bind(input.plan_name).bind(input.amount).bind(input.currency)
        .bind(input.cadence_unit).bind(interval).bind(input.next_billing_date).bind(input.next_billing_date.day() as i16)
        .bind(input.category.unwrap_or_else(|| "其他".into())).bind(input.payment_method).bind(input.notes).bind(input.icon_url)
        .fetch_one(&mut *tx).await?;
    for days in input.reminder_offsets.unwrap_or_else(|| vec![7, 3, 1]) {
        sqlx::query("INSERT INTO subscription_reminders (subscription_id, days_before) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(row.id).bind(days.clamp(0,365)).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_subscription(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(patch): Json<Value>,
) -> Result<Json<Subscription>, ApiError> {
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
        "SELECT telegram_enabled,coalesce(length(telegram_bot_token)>0,false) telegram_bot_token_configured,telegram_chat_id,email_enabled,smtp_host,smtp_port,smtp_tls,smtp_from,smtp_username,coalesce(length(smtp_password)>0,false) smtp_password_configured FROM notification_settings WHERE user_id=$1",
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
    if !(1..=65535).contains(&input.smtp_port) {
        return Err(ApiError::Validation(
            "SMTP 端口必须在 1 到 65535 之间".into(),
        ));
    }
    let existing: Option<(bool, bool)> = sqlx::query_as(
        "SELECT coalesce(length(telegram_bot_token)>0,false),coalesce(length(smtp_password)>0,false) FROM notification_settings WHERE user_id=$1",
    )
    .bind(user.0)
    .fetch_optional(&state.db)
    .await?;
    let has_telegram_token = input
        .telegram_bot_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || existing.is_some_and(|value| value.0);
    if input.telegram_enabled && (!has_telegram_token || input.telegram_chat_id.trim().is_empty()) {
        return Err(ApiError::Validation(
            "启用 Telegram 前需要填写 Bot Token 和 Chat ID".into(),
        ));
    }
    if input.email_enabled
        && (input.smtp_host.trim().is_empty()
            || input.smtp_from.trim().is_empty()
            || input.smtp_from.parse::<lettre::message::Mailbox>().is_err())
    {
        return Err(ApiError::Validation(
            "启用邮件前需要填写有效的 SMTP 主机和发件人".into(),
        ));
    }

    sqlx::query("INSERT INTO notification_settings (user_id,telegram_enabled,telegram_bot_token,telegram_chat_id,email_enabled,smtp_host,smtp_port,smtp_tls,smtp_from,smtp_username,smtp_password) VALUES ($1,$2,nullif($3,''),$4,$5,$6,$7,$8,$9,$10,nullif($11,'')) ON CONFLICT (user_id) DO UPDATE SET telegram_enabled=excluded.telegram_enabled,telegram_bot_token=coalesce(excluded.telegram_bot_token,notification_settings.telegram_bot_token),telegram_chat_id=excluded.telegram_chat_id,email_enabled=excluded.email_enabled,smtp_host=excluded.smtp_host,smtp_port=excluded.smtp_port,smtp_tls=excluded.smtp_tls,smtp_from=excluded.smtp_from,smtp_username=excluded.smtp_username,smtp_password=coalesce(excluded.smtp_password,notification_settings.smtp_password),updated_at=now()")
        .bind(user.0)
        .bind(input.telegram_enabled)
        .bind(input.telegram_bot_token.unwrap_or_default().trim().to_owned())
        .bind(input.telegram_chat_id.trim())
        .bind(input.email_enabled)
        .bind(input.smtp_host.trim())
        .bind(input.smtp_port)
        .bind(input.smtp_tls)
        .bind(input.smtp_from.trim())
        .bind(input.smtp_username.trim())
        .bind(input.smtp_password.unwrap_or_default())
        .execute(&state.db)
        .await?;
    get_notification_settings(user, State(state)).await
}

#[derive(Deserialize)]
struct IconQuery {
    q: String,
    country: Option<String>,
}
async fn search_icons(
    State(state): State<AppState>,
    Query(query): Query<IconQuery>,
) -> Result<Json<Vec<IconCandidate>>, ApiError> {
    if query.q.trim().is_empty() {
        return Ok(Json(vec![]));
    }
    let response: Value = state
        .http
        .get("https://itunes.apple.com/search")
        .query(&[
            ("term", query.q.as_str()),
            ("entity", "software"),
            ("limit", "8"),
            ("country", query.country.as_deref().unwrap_or("cn")),
        ])
        .send()
        .await
        .map_err(|_| ApiError::Upstream)?
        .json()
        .await
        .map_err(|_| ApiError::Upstream)?;
    let results = response["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(IconCandidate {
                name: v["trackName"].as_str()?.into(),
                developer: v["sellerName"].as_str().unwrap_or_default().into(),
                icon_url: v["artworkUrl512"]
                    .as_str()
                    .or_else(|| v["artworkUrl100"].as_str())?
                    .into(),
                store_url: v["trackViewUrl"].as_str().unwrap_or_default().into(),
                bundle_id: v["bundleId"].as_str().unwrap_or_default().into(),
            })
        })
        .collect();
    Ok(Json(results))
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
