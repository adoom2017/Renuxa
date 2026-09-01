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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
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
