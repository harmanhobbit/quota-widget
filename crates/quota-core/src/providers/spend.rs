//! Shared shape for providers that report **spend over a period** rather than a
//! balance or an allowance.
//!
//! There is no remaining quantity to draw down, so there is nothing to make a
//! percentage from unless the user says what they consider a full month's
//! worth. Every spend provider therefore offers the same optional
//! `monthly_budget` setting (settled with Ian when Fireworks landed):
//!
//! - with a budget: a `UsageWindow` over the calendar month, so the tray,
//!   thresholds and period marks work as they do everywhere else;
//! - without one: a labelled `Credits` figure carrying month-to-date spend,
//!   which the card renders as "Cost this month: …" rather than as a balance.

use super::as_f64;
use crate::config::Config;
use crate::model::{Credits, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Datelike, TimeZone, Utc};

/// First instant of `now`'s calendar month, and the first instant of the next.
/// The exclusive end doubles as the reset the UI counts down to.
pub(crate) fn month_bounds(now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()?;
    let end = start.checked_add_months(chrono::Months::new(1))?;
    Some((start, end))
}

/// The configured monthly budget, if the user set a positive one.
pub(crate) fn monthly_budget(config: &Config, key: &str) -> Option<f64> {
    config
        .provider_setting(key, "monthly_budget")
        .and_then(|v| as_f64(&v))
        .filter(|b| *b > 0.0)
}

/// Render month-to-date `spend` (USD) as a snapshot, in whichever of the two
/// shapes the account is configured for.
pub(crate) fn spend_snapshot(
    id: &str,
    name: &str,
    spend: f64,
    budget: Option<f64>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> UsageSnapshot {
    match budget {
        Some(budget) => UsageSnapshot::ok(
            id,
            name,
            vec![monthly_spend_window(spend, budget, start, end)],
            None,
        ),
        None => UsageSnapshot::ok(
            id,
            name,
            vec![],
            Some(Credits {
                balance: spend,
                // Without the label this would read as money remaining.
                label: Some("Cost this month".into()),
                unit: "USD".into(),
                used: None,
                granted: None,
                est_tokens_remaining: None,
            }),
        ),
    }
}

/// The target window shared by spend-only providers and providers which keep
/// an actual credit balance alongside a user-set monthly target.
pub(crate) fn monthly_spend_window(
    spend: f64,
    budget: f64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> UsageWindow {
    UsageWindow {
        metric_id: "monthly_spend".into(),
        label: "Monthly spend".into(),
        // Overspend is entirely possible — a budget is the user's intention,
        // not a cap the provider enforces.
        used_pct: spend / budget * 100.0,
        resets_at: Some(end),
        period_start: Some(start),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_bounds_span_the_calendar_month() {
        let now: DateTime<Utc> = "2026-08-04T12:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        assert_eq!(
            start,
            "2026-08-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            end,
            "2026-09-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn december_rolls_into_the_next_year() {
        let now: DateTime<Utc> = "2026-12-20T00:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        assert_eq!(
            start,
            "2026-12-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(
            end,
            "2027-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn a_budget_becomes_a_window_with_period_bounds() {
        let now: DateTime<Utc> = "2026-08-04T12:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        let snap = spend_snapshot("k", "N", 50.0, Some(200.0), start, end);
        assert!(snap.credits.is_none());
        let w = &snap.windows[0];
        assert_eq!(w.metric_id, "monthly_spend");
        assert!((w.used_pct - 25.0).abs() < 1e-9);
        assert_eq!(w.period_start, Some(start));
        assert_eq!(w.resets_at, Some(end));
        assert!(!w.informational);
    }

    #[test]
    fn overspending_a_budget_reads_past_full() {
        let now: DateTime<Utc> = "2026-08-04T12:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        let snap = spend_snapshot("k", "N", 250.0, Some(200.0), start, end);
        assert!(snap.windows[0].used_pct > 100.0);
    }

    #[test]
    fn no_budget_becomes_a_labelled_cost_figure() {
        let now: DateTime<Utc> = "2026-08-04T12:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        let snap = spend_snapshot("k", "N", 8.75, None, start, end);
        assert!(snap.windows.is_empty());
        let c = snap.credits.unwrap();
        assert!((c.balance - 8.75).abs() < 1e-9);
        assert_eq!(c.label.as_deref(), Some("Cost this month"));
        assert_eq!(c.unit, "USD");
        // The label already says "cost"; repeating it as `used` would print the
        // same number twice on the card.
        assert_eq!(c.used, None);
    }
}
