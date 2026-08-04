//! Fireworks AI spend via the official, documented API:
//! `GET /v1/accounts/{account_id}/billingUsage`.
//!
//! This is the first adapter for a provider that reports **spend over a
//! period** rather than a balance or an allowance. There is no remaining
//! quantity to draw down, so there is nothing to make a percentage from unless
//! the user says what they consider a full month's worth. Hence the optional
//! `monthly_budget` setting (confirmed with Ian):
//!
//! - with a budget: a `UsageWindow` over the calendar month, so the tray,
//!   thresholds and period marks all work as they do everywhere else;
//! - without one: a `Credits` figure carrying month-to-date spend and nothing
//!   else, which the card renders as a plain cost.
//!
//! Two shape notes. The account id goes in the *path*, so it is a required
//! per-account setting rather than an endpoint override. And costs come back in
//! nano-USD (1e-9) across three separate arrays — serverless, dedicated and
//! training — which are summed: the question being answered is "what is this
//! account costing me this month".

use super::{as_f64, network_err, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde_json::Value;

pub struct Fireworks {
    pub key: String,
    pub label: Option<String>,
}
impl Fireworks {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

const DEFAULT_BASE: &str = "https://api.fireworks.ai";
const NANO_USD: f64 = 1e-9;

#[async_trait::async_trait]
impl Provider for Fireworks {
    fn kind(&self) -> &'static str {
        "fireworks"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Fireworks")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured("paste a Fireworks API key in Settings".into())
            })?;
        // Unlike every other adapter here the account id is part of the path,
        // so it is required configuration rather than an optional override.
        let account_id = ctx
            .config
            .provider_setting(&self.key, "account_id")
            .and_then(|v| v.as_str().map(str::trim).map(String::from))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured("set the Fireworks account ID in Settings".into())
            })?;
        let base = ctx
            .config
            .provider_setting(&self.key, "base_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| DEFAULT_BASE.to_string());
        let budget = ctx
            .config
            .provider_setting(&self.key, "monthly_budget")
            .and_then(|v| as_f64(&v))
            .filter(|b| *b > 0.0);

        let now = Utc::now();
        let (start, end) = month_bounds(now).ok_or_else(|| {
            FetchError::Parse("could not determine the current billing month".into())
        })?;
        let url = format!(
            "{}/v1/accounts/{}/billingUsage",
            base.trim_end_matches('/'),
            account_id
        );

        let resp = ctx
            .http
            .get(&url)
            // The window is exclusive of endTime, so "now" is the right upper
            // bound: it asks for the month so far.
            .query(&[
                ("startTime", start.to_rfc3339()),
                ("endTime", now.to_rfc3339()),
            ])
            .bearer_auth(key)
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "API key rejected — re-check it in Settings".into(),
                ))
            }
            404 => {
                return Err(FetchError::NotConfigured(
                    "no such Fireworks account — check the account ID in Settings".into(),
                ))
            }
            s => {
                return Err(FetchError::Network(format!(
                    "HTTP {s} from billing usage endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let spend = total_spend_usd(&body);

        Ok(match budget {
            Some(budget) => UsageSnapshot::ok(
                self.id(),
                self.name(),
                vec![spend_window(spend, budget, start, end)],
                None,
            ),
            // No budget configured: report the cost itself and leave the
            // percentage machinery out of it entirely. The label keeps it from
            // reading as a balance — this is money spent, not money left.
            None => UsageSnapshot::ok(
                self.id(),
                self.name(),
                vec![],
                Some(Credits {
                    balance: spend,
                    label: Some("Cost this month".into()),
                    unit: "USD".into(),
                    used: None,
                    granted: None,
                    est_tokens_remaining: None,
                }),
            ),
        })
    }
}

/// First instant of `now`'s calendar month, and the first instant of the next —
/// the exclusive end the API expects and the reset the UI counts down to.
fn month_bounds(now: DateTime<Utc>) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()?;
    let end = start.checked_add_months(chrono::Months::new(1))?;
    Some((start, end))
}

/// Month-to-date spend in USD, summed across all three cost categories.
/// Missing arrays contribute nothing: an account with no dedicated deployments
/// simply has no dedicated costs, which is zero rather than an error.
fn total_spend_usd(body: &Value) -> f64 {
    ["serverlessCosts", "dedicatedCosts", "trainingCosts"]
        .iter()
        .filter_map(|k| body.get(*k))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|entry| entry.get("costNanoUsd").and_then(as_f64))
        .sum::<f64>()
        * NANO_USD
}

fn spend_window(
    spend: f64,
    budget: f64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> UsageWindow {
    UsageWindow {
        metric_id: "monthly_spend".into(),
        label: "Monthly spend".into(),
        // Overspend is entirely possible — a budget is the user's intention,
        // not a cap the provider enforces. The model tolerates >100.
        used_pct: spend / budget * 100.0,
        resets_at: Some(end),
        period_start: Some(start),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "serverlessCosts": [
                {"costNanoUsd": 1_000_000_000i64, "usageType": "SERVERLESS"},
                {"costNanoUsd": 500_000_000i64}
            ],
            "dedicatedCosts": [{"costNanoUsd": 2_000_000_000i64}],
            "trainingCosts": []
        })
    }

    #[test]
    fn sums_nano_usd_across_all_three_categories() {
        assert!((total_spend_usd(&sample()) - 3.5).abs() < 1e-9);
    }

    #[test]
    fn absent_categories_contribute_nothing() {
        let body = serde_json::json!({"serverlessCosts": [{"costNanoUsd": 1_000_000_000i64}]});
        assert!((total_spend_usd(&body) - 1.0).abs() < 1e-9);
        // An account that spent nothing is 0.0, not an error.
        assert_eq!(total_spend_usd(&serde_json::json!({})), 0.0);
    }

    #[test]
    fn string_costs_parse_like_every_other_adapter() {
        let body = serde_json::json!({"serverlessCosts": [{"costNanoUsd": "1500000000"}]});
        assert!((total_spend_usd(&body) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn budget_window_spans_the_calendar_month() {
        let now: DateTime<Utc> = "2026-08-04T12:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        assert_eq!(start, "2026-08-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
        assert_eq!(end, "2026-09-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap());

        let w = spend_window(50.0, 200.0, start, end);
        assert_eq!(w.metric_id, "monthly_spend");
        assert!((w.used_pct - 25.0).abs() < 1e-9);
        assert_eq!(w.period_start, Some(start));
        assert_eq!(w.resets_at, Some(end));
        assert!(!w.informational);
    }

    #[test]
    fn december_rolls_into_the_next_year() {
        let now: DateTime<Utc> = "2026-12-20T00:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        assert_eq!(start, "2026-12-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
        assert_eq!(end, "2027-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap());
    }

    #[test]
    fn overspending_a_budget_reads_past_full() {
        let now: DateTime<Utc> = "2026-08-04T12:00:00Z".parse().unwrap();
        let (start, end) = month_bounds(now).unwrap();
        assert!(spend_window(250.0, 200.0, start, end).used_pct > 100.0);
    }
}
