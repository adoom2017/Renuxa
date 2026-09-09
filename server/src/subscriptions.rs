use crate::{
    error::ApiError,
    models::{CreateSubscription, Subscription},
};
use chrono::Datelike;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub fn validate(input: &CreateSubscription) -> Result<(), ApiError> {
    if input.name.trim().is_empty()
        || input.name.len() > 200
        || input.amount.is_sign_negative()
        || input.amount.scale() > 6
        || input.amount >= rust_decimal::Decimal::from(100_000_000_000_000u64)
        || input.currency.len() != 3
        || !input.currency.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return Err(ApiError::Validation("请检查名称、金额和货币".into()));
    }
    if !["day", "week", "month", "quarter", "year", "once"].contains(&input.cadence_unit.as_str())
        || !(1..=120).contains(&input.cadence_interval.unwrap_or(1))
        || (input.cadence_unit == "once" && input.cadence_interval.unwrap_or(1) != 1)
        || !(1900..=9999).contains(&input.next_billing_date.year())
    {
        return Err(ApiError::Validation("周期、间隔或扣款日期无效".into()));
    }
    if input
        .reminder_offsets
        .as_ref()
        .is_some_and(|v| v.len() > 366 || v.iter().any(|d| !(0..=365).contains(d)))
    {
        return Err(ApiError::Validation(
            "提醒提前天数必须在 0 到 365 之间".into(),
        ));
    }
    Ok(())
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    input: CreateSubscription,
) -> Result<Subscription, ApiError> {
    validate(&input)?;
    let row = sqlx::query_as::<_, Subscription>("INSERT INTO subscriptions (user_id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,category,payment_method,notes,icon_url) VALUES ($1,$2,$3,$4,upper($5),$6,$7,$8,$9,$10,$11,$12,$13) RETURNING id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,status,category,payment_method,notes,icon_url,created_at,updated_at")
        .bind(user_id).bind(input.name.trim()).bind(input.plan_name).bind(input.amount).bind(input.currency)
        .bind(input.cadence_unit).bind(input.cadence_interval.unwrap_or(1)).bind(input.next_billing_date).bind(input.next_billing_date.day() as i16)
        .bind(input.category.unwrap_or_else(|| "其他".into())).bind(input.payment_method).bind(input.notes).bind(input.icon_url)
        .fetch_one(&mut **tx).await?;
    for days in input.reminder_offsets.unwrap_or_else(|| vec![7, 3, 1]) {
        sqlx::query("INSERT INTO subscription_reminders (subscription_id, days_before) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(row.id).bind(days).execute(&mut **tx).await?;
    }
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_values_without_clamping() {
        let mut input: CreateSubscription = serde_json::from_value(serde_json::json!({"name":"Test","amount":"9.99","currency":"CNY","cadence_unit":"month","next_billing_date":"2026-09-30"})).unwrap();
        assert!(validate(&input).is_ok());
        input.cadence_interval = Some(0);
        assert!(validate(&input).is_err());
        input.cadence_interval = Some(1);
        input.reminder_offsets = Some(vec![366]);
        assert!(validate(&input).is_err());
        input.reminder_offsets = None;
        for unit in ["day", "week", "month", "quarter", "year", "once"] {
            input.cadence_unit = unit.into();
            assert!(validate(&input).is_ok());
        }
        input.currency = "$$$".into();
        assert!(validate(&input).is_err());
    }
}
