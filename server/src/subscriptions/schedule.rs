use crate::error::ApiError;
use chrono::{Datelike, Duration, Months, NaiveDate};

pub struct BillingPlan {
    pub paid_dates: Vec<NaiveDate>,
    pub next_date: NaiveDate,
}

pub fn billing_plan(
    start: NaiveDate,
    unit: &str,
    interval: i32,
    today: NaiveDate,
) -> Result<BillingPlan, ApiError> {
    if interval < 1 {
        return Err(ApiError::Validation("周期间隔无效".into()));
    }
    let mut date = start;
    let mut paid_dates = Vec::new();
    while date <= today {
        paid_dates.push(date);
        if unit == "once" {
            break;
        }
        date = next_date(date, unit, interval, start.day())?;
    }
    Ok(BillingPlan {
        paid_dates,
        next_date: date,
    })
}

fn next_date(
    date: NaiveDate,
    unit: &str,
    interval: i32,
    anchor: u32,
) -> Result<NaiveDate, ApiError> {
    let invalid = || ApiError::Validation("计费日期超出支持范围".into());
    let next = match unit {
        "day" => date.checked_add_signed(Duration::days(interval.into())),
        "week" => date.checked_add_signed(Duration::weeks(interval.into())),
        "month" | "quarter" | "year" => {
            let months = interval
                .checked_mul(match unit {
                    "quarter" => 3,
                    "year" => 12,
                    _ => 1,
                })
                .ok_or_else(invalid)?;
            let first = date
                .with_day(1)
                .unwrap()
                .checked_add_months(Months::new(months as u32))
                .ok_or_else(invalid)?;
            let last = first
                .checked_add_months(Months::new(1))
                .and_then(|d| d.pred_opt())
                .ok_or_else(invalid)?;
            first.with_day(anchor.min(last.day()))
        }
        _ => return Err(ApiError::Validation("扣费周期无效".into())),
    }
    .ok_or_else(invalid)?;
    if next.year() > 9999 {
        return Err(invalid());
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date(value: &str) -> NaiveDate {
        value.parse().unwrap()
    }

    #[test]
    fn historical_cycles_and_month_end_anchor() {
        let plan = billing_plan(date("2026-01-31"), "month", 1, date("2026-03-31")).unwrap();
        assert_eq!(
            plan.paid_dates,
            vec![date("2026-01-31"), date("2026-02-28"), date("2026-03-31")]
        );
        assert_eq!(plan.next_date, date("2026-04-30"));
        let leap = billing_plan(date("2024-02-29"), "year", 1, date("2028-02-29")).unwrap();
        assert_eq!(*leap.paid_dates.last().unwrap(), date("2028-02-29"));
    }

    #[test]
    fn future_once_and_interval_schedules() {
        let future = billing_plan(date("2026-10-01"), "month", 1, date("2026-09-10")).unwrap();
        assert!(future.paid_dates.is_empty());
        assert_eq!(future.next_date, date("2026-10-01"));
        let once = billing_plan(date("2026-01-01"), "once", 1, date("2026-09-10")).unwrap();
        assert_eq!(once.paid_dates, vec![date("2026-01-01")]);
        for (unit, interval, count) in [
            ("day", 2, 16),
            ("week", 2, 3),
            ("quarter", 2, 1),
            ("year", 2, 1),
        ] {
            assert_eq!(
                billing_plan(date("2026-01-01"), unit, interval, date("2026-01-31"))
                    .unwrap()
                    .paid_dates
                    .len(),
                count
            );
        }
    }

    #[test]
    fn rejects_overflow_without_panicking() {
        assert!(billing_plan(date("9999-12-31"), "year", 120, date("9999-12-31")).is_err());
    }
}
