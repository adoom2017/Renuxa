use crate::{
    error::ApiError,
    models::{CreateSubscription, Subscription},
};
use chrono::{Datelike, NaiveDate};

mod schedule;
pub use schedule::billing_plan;
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
        || input
            .start_date
            .or(input.next_billing_date)
            .is_none_or(|date| !(1900..=9999).contains(&date.year()))
    {
        return Err(ApiError::Validation("周期、间隔或扣款日期无效".into()));
    }
    if input
        .next_billing_date
        .is_some_and(|date| !(1900..=9999).contains(&date.year()))
    {
        return Err(ApiError::Validation("扣款日期超出支持范围".into()));
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
    let (next, history) = prepare(tx, user_id, &input).await?;
    let start = input.start_date;
    let anchor = start.unwrap_or(next).day() as i16;
    let row = sqlx::query_as::<_, Subscription>("INSERT INTO subscriptions (user_id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,category,payment_method,notes,icon_url,start_date) VALUES ($1,$2,$3,$4,upper($5),$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,start_date,anchor_day,status,category,payment_method,notes,icon_url,created_at,updated_at")
        .bind(user_id).bind(input.name.trim()).bind(input.plan_name).bind(input.amount).bind(input.currency)
        .bind(input.cadence_unit).bind(input.cadence_interval.unwrap_or(1)).bind(next).bind(anchor)
        .bind(input.category.unwrap_or_else(|| "其他".into())).bind(input.payment_method).bind(input.notes).bind(input.icon_url).bind(start)
        .fetch_one(&mut **tx).await?;
    for days in input.reminder_offsets.unwrap_or_else(|| vec![7, 3, 1]) {
        sqlx::query("INSERT INTO subscription_reminders (subscription_id, days_before) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(row.id).bind(days).execute(&mut **tx).await?;
    }
    sync_history(tx, &row, &history, false).await?;
    find(tx, user_id, row.id).await
}

pub async fn update(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    id: Uuid,
    input: CreateSubscription,
) -> Result<Subscription, ApiError> {
    let mut input = input;
    let previous: (Option<NaiveDate>, String, i32) = sqlx::query_as("SELECT start_date,cadence_unit,cadence_interval FROM subscriptions WHERE id=$1 AND user_id=$2 AND status <> 'archived' FOR UPDATE")
        .bind(id).bind(user_id).fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)?;
    input.start_date = input.start_date.or(previous.0);
    validate(&input)?;
    let changed = input.start_date != previous.0
        || input.cadence_unit != previous.1
        || input.cadence_interval.unwrap_or(1) != previous.2;
    let (next, history) = prepare(tx, user_id, &input).await?;
    let start = input.start_date;
    let anchor = start.unwrap_or(next).day() as i16;
    let row = sqlx::query_as::<_, Subscription>("UPDATE subscriptions SET name=$3,plan_name=$4,amount=$5,currency=upper($6),cadence_unit=$7,cadence_interval=$8,next_billing_date=CASE WHEN $16 OR start_date IS NULL THEN $9 ELSE next_billing_date END,anchor_day=$10,category=$11,payment_method=$12,notes=$13,icon_url=$14,start_date=$15,updated_at=now() WHERE id=$1 AND user_id=$2 AND status <> 'archived' RETURNING id,name,plan_name,amount,currency,cadence_unit,cadence_interval,next_billing_date,start_date,anchor_day,status,category,payment_method,notes,icon_url,created_at,updated_at")
        .bind(id).bind(user_id).bind(input.name.trim()).bind(input.plan_name).bind(input.amount).bind(input.currency)
        .bind(input.cadence_unit).bind(input.cadence_interval.unwrap_or(1)).bind(next).bind(anchor)
        .bind(input.category.unwrap_or_else(|| "其他".into())).bind(input.payment_method).bind(input.notes).bind(input.icon_url).bind(start).bind(changed)
        .fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)?;
    sqlx::query("DELETE FROM subscription_reminders WHERE subscription_id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    for days in input.reminder_offsets.unwrap_or_else(|| vec![7, 3, 1]) {
        sqlx::query("INSERT INTO subscription_reminders (subscription_id, days_before) VALUES ($1,$2) ON CONFLICT DO NOTHING")
            .bind(id).bind(days).execute(&mut **tx).await?;
    }
    if changed && start.is_some() {
        sync_history(tx, &row, &history, true).await?;
    }
    find(tx, user_id, row.id).await
}

async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    input: &CreateSubscription,
) -> Result<(NaiveDate, Vec<NaiveDate>), ApiError> {
    if let Some(start) = input.start_date {
        let today: NaiveDate =
            sqlx::query_scalar("SELECT (now() AT TIME ZONE timezone)::date FROM users WHERE id=$1")
                .bind(user)
                .fetch_one(&mut **tx)
                .await?;
        let plan = billing_plan(
            start,
            &input.cadence_unit,
            input.cadence_interval.unwrap_or(1),
            today,
        )?;
        Ok((plan.next_date, plan.paid_dates))
    } else {
        Ok((
            input
                .next_billing_date
                .ok_or_else(|| ApiError::Validation("缺少订阅开始日期".into()))?,
            vec![],
        ))
    }
}

async fn sync_history(
    tx: &mut Transaction<'_, Postgres>,
    sub: &Subscription,
    dates: &[NaiveDate],
    reconcile: bool,
) -> Result<(), ApiError> {
    if reconcile {
        sqlx::query("DELETE FROM bills WHERE subscription_id=$1 AND source='schedule' AND NOT (due_date=ANY($2))")
            .bind(sub.id).bind(dates).execute(&mut **tx).await?;
    }
    sqlx::query("INSERT INTO bills(user_id,subscription_id,amount,currency,due_date,status,source,idempotency_key) SELECT s.user_id,s.id,s.amount,s.currency,d,'paid','schedule','renewal:'||s.id::text||':'||d::text FROM subscriptions s CROSS JOIN unnest($2::date[]) d WHERE s.id=$1 ON CONFLICT(idempotency_key) DO NOTHING")
        .bind(sub.id).bind(dates).execute(&mut **tx).await?;
    if sub.cadence_unit == "once" && !dates.is_empty() {
        sqlx::query("UPDATE subscriptions SET status='cancelled' WHERE id=$1")
            .bind(sub.id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn find(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    id: Uuid,
) -> Result<Subscription, ApiError> {
    sqlx::query_as::<_, Subscription>("SELECT s.id,s.name,s.plan_name,s.amount,s.currency,s.cadence_unit,s.cadence_interval,s.next_billing_date,s.start_date,s.anchor_day,s.status,s.category,s.payment_method,s.notes,s.icon_url,coalesce((SELECT array_agg(r.days_before ORDER BY r.days_before DESC) FROM subscription_reminders r WHERE r.subscription_id=s.id),ARRAY[]::integer[]) reminder_offsets,s.created_at,s.updated_at FROM subscriptions s WHERE s.id=$1 AND s.user_id=$2 AND s.status <> 'archived'")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ApiError::NotFound)
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
