use crate::AppState;
use chrono::{Datelike, Months, NaiveDate};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use sqlx::Row;
use std::env;
use uuid::Uuid;

pub async fn run_cycle(state: &AppState) -> Result<(), sqlx::Error> {
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
            for channel in ["in_app", "email"] {
                sqlx::query("INSERT INTO notification_deliveries (notification_id,channel,status,next_attempt_at) VALUES ($1,$2,'pending',now())")
                    .bind(notification_id).bind(channel).execute(&mut *tx).await?;
            }
        }
    }
    let rows = sqlx::query("SELECT id,user_id,name,amount,currency,cadence_unit,cadence_interval,next_billing_date,anchor_day FROM subscriptions WHERE status='active' AND next_billing_date <= current_date ORDER BY next_billing_date FOR UPDATE SKIP LOCKED LIMIT 100")
        .fetch_all(&mut *tx).await?;
    for row in rows {
        let id: Uuid = row.get("id");
        let user_id: Uuid = row.get("user_id");
        let due: NaiveDate = row.get("next_billing_date");
        let name: String = row.get("name");
        let unit: String = row.get("cadence_unit");
        let interval: i32 = row.get("cadence_interval");
        let anchor: i16 = row.get("anchor_day");
        sqlx::query("INSERT INTO bills (user_id,subscription_id,amount,currency,due_date,status,idempotency_key) VALUES ($1,$2,$3,$4,$5,'estimated',$6) ON CONFLICT (idempotency_key) DO NOTHING")
            .bind(user_id).bind(id).bind(row.get::<rust_decimal::Decimal,_>("amount")).bind(row.get::<String,_>("currency")).bind(due).bind(format!("renewal:{id}:{due}")).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO notifications (user_id,subscription_id,title,body,kind,scheduled_for,idempotency_key) VALUES ($1,$2,$3,$4,'renewal',now(),$5) ON CONFLICT (idempotency_key) DO NOTHING")
            .bind(user_id).bind(id).bind(format!("{name} 今日续费")).bind("已生成预计账单，请确认实际扣款。".to_string()).bind(format!("renewal-notice:{id}:{due}")).execute(&mut *tx).await?;
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
    deliver_email(state).await;
    sync_rates(state).await;
    Ok(())
}

async fn sync_rates(state: &AppState) {
    let fresh: bool = sqlx::query_scalar(
        "SELECT coalesce(max(rate_date)>=current_date,false) FROM exchange_rates",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    if fresh {
        return;
    }
    let Ok(response) = state
        .http
        .get("https://api.frankfurter.app/latest?from=EUR")
        .send()
        .await
    else {
        return;
    };
    let Ok(payload) = response.json::<serde_json::Value>().await else {
        return;
    };
    let Some(date) = payload["date"]
        .as_str()
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
    else {
        return;
    };
    let Some(rates) = payload["rates"].as_object() else {
        return;
    };
    for (currency, value) in rates {
        let Some(rate) = value
            .as_f64()
            .and_then(rust_decimal::Decimal::from_f64_retain)
        else {
            continue;
        };
        let _ = sqlx::query("INSERT INTO exchange_rates(rate_date,base_currency,quote_currency,rate,provider) VALUES($1,'EUR',$2,$3,'frankfurter') ON CONFLICT(rate_date,base_currency,quote_currency) DO UPDATE SET rate=excluded.rate,provider=excluded.provider")
            .bind(date).bind(currency).bind(rate).execute(&state.db).await;
    }
    let _ = sqlx::query("INSERT INTO exchange_rates(rate_date,base_currency,quote_currency,rate,provider) VALUES($1,'EUR','EUR',1,'frankfurter') ON CONFLICT DO NOTHING").bind(date).execute(&state.db).await;
}

async fn deliver_email(state: &AppState) {
    let Ok(host) = env::var("SMTP_HOST") else {
        return;
    };
    let from =
        env::var("SMTP_FROM").unwrap_or_else(|_| "Renuxa <notifications@renuxa.local>".into());
    let mut builder = if env::var("SMTP_TLS").as_deref() == Ok("true") {
        let Ok(builder) = AsyncSmtpTransport::<Tokio1Executor>::relay(&host) else {
            return;
        };
        builder
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&host)
    };
    if let (Ok(username), Ok(password)) = (env::var("SMTP_USERNAME"), env::var("SMTP_PASSWORD")) {
        if !username.is_empty() {
            builder = builder.credentials(Credentials::new(username, password));
        }
    }
    let mailer = builder
        .port(
            env::var("SMTP_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(587),
        )
        .build();
    let rows = match sqlx::query("SELECT d.id,u.email,n.title,n.body FROM notification_deliveries d JOIN notifications n ON n.id=d.notification_id JOIN users u ON u.id=n.user_id WHERE d.channel='email' AND d.status='pending' AND d.next_attempt_at<=now() ORDER BY d.created_at FOR UPDATE SKIP LOCKED LIMIT 50")
        .fetch_all(&state.db).await { Ok(rows) => rows, Err(_) => return };
    for row in rows {
        let delivery_id: Uuid = row.get("id");
        let message = Message::builder()
            .from(match from.parse() {
                Ok(value) => value,
                Err(_) => return,
            })
            .to(match row.get::<String, _>("email").parse() {
                Ok(value) => value,
                Err(_) => continue,
            })
            .subject(row.get::<String, _>("title"))
            .body(row.get::<String, _>("body"));
        let sent = match message {
            Ok(message) => mailer.send(message).await.is_ok(),
            Err(_) => false,
        };
        if sent {
            let _ = sqlx::query("UPDATE notification_deliveries SET status='sent',attempts=attempts+1,updated_at=now() WHERE id=$1").bind(delivery_id).execute(&state.db).await;
        } else {
            let _ = sqlx::query("UPDATE notification_deliveries SET attempts=attempts+1,next_attempt_at=now()+(interval '1 minute'*least(60,power(2,attempts+1))),updated_at=now() WHERE id=$1").bind(delivery_id).execute(&state.db).await;
        }
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
