use crate::{AppState, error::ApiError};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde_json::Value;
use std::time::Duration;

const LATEST_URL: &str = "https://api.frankfurter.dev/v1/latest?from=EUR";

type RateSnapshot = (NaiveDate, Vec<(String, Decimal)>);

fn parse_snapshot(payload: Value) -> Result<RateSnapshot, ApiError> {
    if payload["base"].as_str() != Some("EUR") || payload["amount"].as_f64() != Some(1.0) {
        return Err(ApiError::Upstream);
    }
    let date = payload["date"]
        .as_str()
        .and_then(|value| value.parse::<NaiveDate>().ok())
        .ok_or(ApiError::Upstream)?;
    let quotes = payload["rates"]
        .as_object()
        .filter(|rates| !rates.is_empty())
        .ok_or(ApiError::Upstream)?;
    let mut rates = Vec::with_capacity(quotes.len() + 1);
    for (currency, value) in quotes {
        if currency.len() != 3
            || !currency.bytes().all(|byte| byte.is_ascii_uppercase())
            || !value.is_number()
        {
            return Err(ApiError::Upstream);
        }
        let rate = value
            .to_string()
            .parse::<Decimal>()
            .map_err(|_| ApiError::Upstream)?;
        if rate <= Decimal::ZERO || (currency == "EUR" && rate != Decimal::ONE) {
            return Err(ApiError::Upstream);
        }
        if currency != "EUR" {
            rates.push((currency.clone(), rate));
        }
    }
    if rates.is_empty() {
        return Err(ApiError::Upstream);
    }
    rates.push(("EUR".into(), Decimal::ONE));
    Ok((date, rates))
}

async fn fetch_snapshot(http: &reqwest::Client, url: &str) -> Result<RateSnapshot, ApiError> {
    let response = http
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| ApiError::Upstream)?;
    if !response.status().is_success() {
        return Err(ApiError::Upstream);
    }
    parse_snapshot(response.json().await.map_err(|_| ApiError::Upstream)?)
}

pub async fn sync(state: &AppState) -> Result<(), ApiError> {
    let fresh: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM exchange_rates WHERE base_currency='EUR' AND quote_currency<>'EUR' AND rate_date>=current_date)")
        .fetch_one(&state.db).await?;
    if fresh {
        return Ok(());
    }
    let (date, rates) = fetch_snapshot(&state.http, LATEST_URL).await?;
    // Commit the whole snapshot together. A failed write must not leave a partial latest date.
    let mut tx = state.db.begin().await?;
    save_snapshot(&mut tx, date, &rates).await?;
    tx.commit().await?;
    tracing::debug!(provider="frankfurter", %date, currencies=rates.len(), "exchange rates synced");
    Ok(())
}

async fn save_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    date: NaiveDate,
    rates: &[(String, Decimal)],
) -> Result<(), ApiError> {
    for (currency, rate) in rates {
        sqlx::query("INSERT INTO exchange_rates(rate_date,base_currency,quote_currency,rate,provider) VALUES($1,'EUR',$2,$3,'frankfurter') ON CONFLICT(rate_date,base_currency,quote_currency) DO NOTHING")
            .bind(date).bind(currency).bind(rate).execute(&mut **tx).await?;
    }
    Ok(())
}

// Historical bills use the provider's trading-day snapshot, never today's exchange rate.
// Bound each cycle so importing a long subscription history doesn't delay renewals indefinitely.
pub async fn backfill(state: &AppState) -> Result<u64, ApiError> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar("SELECT DISTINCT due_date FROM bills WHERE due_date<=current_date AND (base_amount IS NULL OR exchange_rate IS NULL OR exchange_rate_date IS NULL) ORDER BY due_date LIMIT 5")
        .fetch_all(&state.db).await?;
    let mut updated = 0;
    for due in dates {
        let snapshot = fetch_snapshot(
            &state.http,
            &format!("https://api.frankfurter.dev/v1/{due}?from=EUR"),
        )
        .await;
        let Ok((date, rates)) = snapshot else {
            tracing::warn!(provider="frankfurter", %due, "historical exchange rate unavailable; retrying next cycle");
            continue;
        };
        if date > due {
            return Err(ApiError::Upstream);
        }
        updated += apply_history(state, due, date, &rates).await?;
    }
    Ok(updated)
}

pub async fn apply_history(
    state: &AppState,
    due: NaiveDate,
    date: NaiveDate,
    rates: &[(String, Decimal)],
) -> Result<u64, ApiError> {
    let mut tx = state.db.begin().await?;
    save_snapshot(&mut tx, date, rates).await?;
    let result=sqlx::query("UPDATE bills b SET base_amount=round(b.amount*target.rate/source.rate,6),base_currency=coalesce(b.base_currency,u.base_currency),exchange_rate=target.rate/source.rate,exchange_rate_date=$2 FROM users u,exchange_rates source,exchange_rates target WHERE b.user_id=u.id AND b.due_date=$1 AND (b.base_amount IS NULL OR b.exchange_rate IS NULL OR b.exchange_rate_date IS NULL) AND source.base_currency='EUR' AND source.rate_date=$2 AND source.quote_currency=b.currency AND target.base_currency=source.base_currency AND target.rate_date=source.rate_date AND target.quote_currency=coalesce(b.base_currency,u.base_currency)")
        .bind(due).bind(date).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_exact_rates_and_adds_eur_base() {
        let (date,rates)=parse_snapshot(json!({"base":"EUR","amount":1.0,"date":"2026-09-09","rates":{"CNY":7.8159,"USD":1.1652}})).unwrap();
        assert_eq!(date, "2026-09-09".parse::<NaiveDate>().unwrap());
        assert!(rates.contains(&("CNY".into(), "7.8159".parse().unwrap())));
        assert!(rates.contains(&("EUR".into(), Decimal::ONE)));
    }

    #[test]
    fn rejects_invalid_snapshots_instead_of_saving_partial_data() {
        for rates in [
            json!({}),
            json!({"USD":0}),
            json!({"USD":-1}),
            json!({"USD":"invalid"}),
            json!({"USD":1.1,"CNY":null}),
            json!({"usd":1.1}),
            json!({"EUR":2}),
        ] {
            assert!(
                parse_snapshot(json!({"base":"EUR","amount":1,"date":"2026-09-09","rates":rates}))
                    .is_err()
            );
        }
        assert!(
            parse_snapshot(json!({"base":"USD","amount":1,"date":"2026-09-09","rates":{"CNY":7}}))
                .is_err()
        );
        assert!(
            parse_snapshot(
                json!({"base":"EUR","amount":100,"date":"2026-09-09","rates":{"USD":110}})
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn fetches_json_and_rejects_redirects() {
        use axum::{Json, Router, http::StatusCode, routing::get};
        let app = Router::new()
            .route(
                "/latest",
                get(|| async {
                    Json(
                        json!({"base":"EUR","amount":1,"date":"2026-09-09","rates":{"USD":1.1652}}),
                    )
                }),
            )
            .route(
                "/old",
                get(|| async { (StatusCode::MOVED_PERMANENTLY, [("location", "/latest")]) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        assert!(
            fetch_snapshot(&http, &format!("{base}/latest"))
                .await
                .is_ok()
        );
        assert!(fetch_snapshot(&http, &format!("{base}/old")).await.is_err());
        task.abort();
    }
}
