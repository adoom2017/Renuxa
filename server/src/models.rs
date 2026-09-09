use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct Subscription {
    pub id: Uuid,
    pub name: String,
    pub plan_name: Option<String>,
    pub amount: Decimal,
    pub currency: String,
    pub cadence_unit: String,
    pub cadence_interval: i32,
    pub next_billing_date: NaiveDate,
    pub anchor_day: i16,
    pub status: String,
    pub category: String,
    pub payment_method: Option<String>,
    pub notes: Option<String>,
    pub icon_url: Option<String>,
    #[sqlx(default)]
    pub reminder_offsets: Vec<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSubscription {
    pub name: String,
    pub plan_name: Option<String>,
    pub amount: Decimal,
    pub currency: String,
    pub cadence_unit: String,
    pub cadence_interval: Option<i32>,
    pub next_billing_date: NaiveDate,
    pub category: Option<String>,
    pub payment_method: Option<String>,
    pub notes: Option<String>,
    pub icon_url: Option<String>,
    pub reminder_offsets: Option<Vec<i32>>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Bill {
    pub id: Uuid,
    pub subscription_id: Uuid,
    pub subscription_name: String,
    pub amount: Decimal,
    pub currency: String,
    pub due_date: NaiveDate,
    pub status: String,
    pub base_amount: Option<Decimal>,
    pub base_currency: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateNotificationSettings {
    pub telegram_enabled: bool,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct NotificationSettings {
    pub telegram_enabled: bool,
    pub telegram_bot_token_configured: bool,
    pub telegram_chat_id: String,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            telegram_enabled: false,
            telegram_bot_token_configured: false,
            telegram_chat_id: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub user_id: Uuid,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IconCandidate {
    pub name: String,
    pub developer: String,
    pub icon_url: String,
    pub store_url: String,
    pub bundle_id: String,
}
