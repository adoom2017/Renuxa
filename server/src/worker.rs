use crate::AppState;
use chrono::{Datelike, Months, NaiveDate};
use sqlx::Row;
use uuid::Uuid;

pub async fn run_cycle(state: &AppState) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM wechat_drafts WHERE expires_at <= now()")
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM wechat_binding_codes WHERE expires_at <= now()")
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM wechat_rate_limits WHERE window_start < now()-interval '1 day'")
        .execute(&state.db)
        .await?;
    let mut tx = state.db.begin().await?;
    let reminders = sqlx::query("SELECT s.id,s.user_id,s.name,s.amount,s.currency,s.next_billing_date,r.days_before FROM subscriptions s JOIN subscription_reminders r ON r.subscription_id=s.id WHERE s.status='active' AND s.next_billing_date-r.days_before <= current_date AND s.next_billing_date >= current_date FOR UPDATE OF s SKIP LOCKED LIMIT 300")
        .fetch_all(&mut *tx).await?;
    for row in reminders {
        let id: Uuid = row.get("id");
        let user_id: Uuid = row.get("user_id");
        let due: NaiveDate = row.get("next_billing_date");
        let days: i32 = row.get("days_before");
        let notification_id: Option<Uuid> = sqlx::query_scalar("INSERT INTO notifications (user_id,subscription_id,title,body,kind,scheduled_for,idempotency_key) VALUES ($1,$2,$3,$4,'renewal',now(),$5) ON CONFLICT (idempotency_key) DO NOTHING RETURNING id")
            .bind(user_id)
            .bind(id)
            .bind(format!("{} 将在 {} 天后续费", row.get::<String, _>("name"), days))
            .bind(format!("预计扣款 {} {}。", row.get::<rust_decimal::Decimal, _>("amount"), row.get::<String, _>("currency")))
            .bind(format!("reminder:{id}:{due}:{days}"))
            .fetch_optional(&mut *tx).await?;
        if let Some(notification_id) = notification_id {
            let telegram_enabled: bool = sqlx::query_scalar(
                "SELECT coalesce((SELECT telegram_enabled FROM notification_settings WHERE user_id=$1),false)",
            )
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
            let mut channels = vec!["in_app"];
            if telegram_enabled {
                channels.push("telegram");
            }
            for channel in channels {
                sqlx::query("INSERT INTO notification_deliveries (notification_id,channel,status,next_attempt_at) VALUES ($1,$2,'pending',now())")
                    .bind(notification_id).bind(channel).execute(&mut *tx).await?;
            }
        }
    }
    let rows = sqlx::query("SELECT id,user_id,name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day,start_date FROM subscriptions WHERE status='active' AND next_billing_date <= (SELECT (now() AT TIME ZONE timezone)::date FROM users WHERE id=subscriptions.user_id) ORDER BY next_billing_date FOR UPDATE SKIP LOCKED LIMIT 100")
        .fetch_all(&mut *tx).await?;
    for row in rows {
        let id: Uuid = row.get("id");
        let user_id: Uuid = row.get("user_id");
        let due: NaiveDate = row.get("next_billing_date");
        let name: String = row.get("name");
        let unit: String = row.get("cadence_unit");
        let interval: i32 = row.get("cadence_interval");
        let anchor: i16 = row.get("anchor_day");
        let scheduled = row.get::<Option<NaiveDate>, _>("start_date").is_some();
        sqlx::query("INSERT INTO bills (user_id,subscription_id,amount,currency,due_date,status,idempotency_key,source) VALUES ($1,$2,$3,$4,$5,$7,$6,$8) ON CONFLICT (idempotency_key) DO NOTHING")
            .bind(user_id).bind(id).bind(row.get::<rust_decimal::Decimal,_>("amount")).bind(row.get::<String,_>("currency")).bind(due).bind(format!("renewal:{id}:{due}")).bind(if scheduled { "paid" } else { "estimated" }).bind(if scheduled { "schedule" } else { "renewal" }).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO notifications (user_id,subscription_id,title,body,kind,scheduled_for,idempotency_key) VALUES ($1,$2,$3,$4,'renewal',now(),$5) ON CONFLICT (idempotency_key) DO NOTHING")
            .bind(user_id).bind(id).bind(format!("{name} 今日续费")).bind(if scheduled { "已按订阅周期记账。" } else { "已生成预计账单，请确认实际扣款。" }).bind(format!("renewal-notice:{id}:{due}")).execute(&mut *tx).await?;
        if unit == "once" {
            sqlx::query("UPDATE subscriptions SET status='cancelled',updated_at=now() WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        } else {
            let next = advance(due, &unit, interval, anchor);
            sqlx::query(
                "UPDATE subscriptions SET next_billing_date=$2,updated_at=now() WHERE id=$1",
            )
            .bind(id)
            .bind(next)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    deliver_telegram(state).await;
    sync_rates(state).await;
    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        crate::exchange_rates::backfill(state),
    )
    .await
    {
        Ok(Ok(count)) if count > 0 => {
            tracing::info!(bills = count, "historical bill exchange rates updated")
        }
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!(error=%error, "historical bill conversion failed"),
        Err(_) => tracing::warn!("historical bill conversion timed out; continuing next cycle"),
    }
    Ok(())
}

async fn sync_rates(state: &AppState) {
    if let Err(error) = crate::exchange_rates::sync(state).await {
        tracing::warn!(provider = "frankfurter", error = %error, "exchange rate sync failed; keeping cached rates");
    }
}

async fn deliver_telegram(state: &AppState) {
    let rows = match sqlx::query("SELECT d.id,n.title,n.body,s.telegram_bot_token,s.telegram_chat_id FROM notification_deliveries d JOIN notifications n ON n.id=d.notification_id JOIN notification_settings s ON s.user_id=n.user_id WHERE d.channel='telegram' AND d.status='pending' AND d.next_attempt_at<=now() AND s.telegram_enabled ORDER BY d.created_at FOR UPDATE OF d SKIP LOCKED LIMIT 50")
        .fetch_all(&state.db).await { Ok(rows) => rows, Err(_) => return };
    for row in rows {
        let delivery_id: Uuid = row.get("id");
        let title: String = row.get("title");
        let body: String = row.get("body");
        let token: String = row
            .get::<Option<String>, _>("telegram_bot_token")
            .unwrap_or_default();
        let chat_id: String = row.get("telegram_chat_id");
        let endpoint = format!("https://api.telegram.org/bot{token}/sendMessage");
        let sent = state
            .http
            .post(&endpoint)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": format!("{title}\n\n{body}"),
            }))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success());
        record_delivery_result(state, delivery_id, sent).await;
    }
}

async fn record_delivery_result(state: &AppState, delivery_id: Uuid, sent: bool) {
    if sent {
        let _ = sqlx::query("UPDATE notification_deliveries SET status='sent',attempts=attempts+1,updated_at=now() WHERE id=$1").bind(delivery_id).execute(&state.db).await;
    } else {
        let _ = sqlx::query("UPDATE notification_deliveries SET attempts=attempts+1,next_attempt_at=now()+(interval '1 minute'*least(60,power(2,attempts+1))),updated_at=now() WHERE id=$1").bind(delivery_id).execute(&state.db).await;
    }
}

pub fn advance(date: NaiveDate, unit: &str, interval: i32, anchor_day: i16) -> NaiveDate {
    match unit {
        "day" => date + chrono::Duration::days(interval as i64),
        "week" => date + chrono::Duration::weeks(interval as i64),
        "year" => anchored_month(date, interval * 12, anchor_day),
        "quarter" => anchored_month(date, interval * 3, anchor_day),
        "month" => anchored_month(date, interval, anchor_day),
        _ => date + chrono::Duration::days(interval as i64),
    }
}

fn anchored_month(date: NaiveDate, months: i32, anchor: i16) -> NaiveDate {
    let first = date
        .with_day(1)
        .unwrap()
        .checked_add_months(Months::new(months.max(1) as u32))
        .unwrap();
    let next_month = first.checked_add_months(Months::new(1)).unwrap();
    let last_day = (next_month - chrono::Duration::days(1)).day();
    first.with_day((anchor as u32).min(last_day)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_month_anchor() {
        let jan = NaiveDate::from_ymd_opt(2025, 1, 31).unwrap();
        let feb = advance(jan, "month", 1, 31);
        assert_eq!(feb, NaiveDate::from_ymd_opt(2025, 2, 28).unwrap());
        assert_eq!(
            advance(feb, "month", 1, 31),
            NaiveDate::from_ymd_opt(2025, 3, 31).unwrap()
        );
    }
    #[test]
    fn handles_leap_year() {
        assert_eq!(
            advance(NaiveDate::from_ymd_opt(2024, 2, 29).unwrap(), "year", 1, 29),
            NaiveDate::from_ymd_opt(2025, 2, 28).unwrap()
        );
    }
}
