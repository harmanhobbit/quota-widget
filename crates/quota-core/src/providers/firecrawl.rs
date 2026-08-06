//! Firecrawl credit allowance via the official, documented API. Like
//! OpenRouter it needs nothing but a pasted key. The quantity it reports is a
//! per-cycle plan allowance rather than a balance, so — following ElevenLabs —
//! it renders as a usage window rather than as `Credits`.
//!
//! Unlike ElevenLabs, the response carries both ends of the billing period, so
//! the period-progress marker is exact rather than inferred.

use super::{as_f64, calendar_month_start, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{Allowance, FetchError, UsageSnapshot, UsageWindow};
use serde_json::Value;

pub struct Firecrawl {
    pub key: String,
    pub label: Option<String>,
}
impl Firecrawl {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

const CREDIT_USAGE_URL: &str = "https://api.firecrawl.dev/v2/team/credit-usage";

#[async_trait::async_trait]
impl Provider for Firecrawl {
    fn kind(&self) -> &'static str {
        "firecrawl"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Firecrawl")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured("paste a Firecrawl API key in Settings".into())
            })?;
        let url = ctx
            .config
            .provider_setting(&self.key, "credit_usage_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| CREDIT_USAGE_URL.to_string());

        let resp = ctx
            .http
            .get(&url)
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
            s => {
                return Err(FetchError::Network(format!(
                    "HTTP {s} from credit usage endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let window = parse_window(&body).ok_or_else(|| {
            FetchError::Parse("credit usage response missing plan credits".into())
        })?;
        Ok(UsageSnapshot::ok(
            self.id(),
            self.name(),
            vec![window],
            None,
        ))
    }
}

/// The plan allowance for the current billing cycle. A zero or missing
/// `planCredits` is treated as unparseable rather than as 100% used — it means
/// no allowance shape we can render.
fn parse_window(body: &Value) -> Option<UsageWindow> {
    let data = body.get("data").unwrap_or(body);
    let plan = data.get("planCredits").and_then(as_f64)?;
    let remaining = data.get("remainingCredits").and_then(as_f64)?;
    if plan <= 0.0 {
        return None;
    }
    let resets_at = data.get("billingPeriodEnd").and_then(parse_timestamp);
    // Both period fields are nullable. When only the end is known, fall back to
    // the same calendar-month inference ElevenLabs relies on, so the progress
    // marker still renders.
    let period_start = data
        .get("billingPeriodStart")
        .and_then(parse_timestamp)
        .or_else(|| resets_at.and_then(calendar_month_start));
    Some(UsageWindow {
        metric_id: "monthly_credits".into(),
        label: "Credits".into(),
        // Credits spent, as a share of the plan's grant. Overage is possible
        // in principle. A bonus or rollover can also put `remaining` above
        // the nominal plan: it is still shown in `allowance`, but cannot mean
        // negative usage.
        used_pct: ((plan - remaining) / plan * 100.0).max(0.0),
        resets_at,
        period_start,
        allowance: Some(Allowance {
            remaining,
            total: plan,
            unit: "credits".into(),
        }),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "success": true,
            "data": {
                "remainingCredits": 400_000,
                "planCredits": 500_000,
                "billingPeriodStart": "2026-08-01T00:00:00Z",
                "billingPeriodEnd": "2026-08-31T23:59:59Z"
            }
        })
    }

    #[test]
    fn parses_documented_shape() {
        let w = parse_window(&sample()).unwrap();
        assert_eq!(w.metric_id, "monthly_credits");
        assert_eq!(w.label, "Credits");
        assert!((w.used_pct - 20.0).abs() < 1e-9);
        assert!(!w.informational);
        assert_eq!(
            w.allowance,
            Some(Allowance {
                remaining: 400_000.0,
                total: 500_000.0,
                unit: "credits".into(),
            })
        );
        // Both ends come straight from the response, no inference.
        assert_eq!(
            w.period_start,
            Some("2026-08-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(w.resets_at, Some("2026-08-31T23:59:59Z".parse().unwrap()));
    }

    #[test]
    fn exhausted_allowance_reads_full() {
        let mut body = sample();
        body["data"]["remainingCredits"] = serde_json::json!(0);
        assert!((parse_window(&body).unwrap().used_pct - 100.0).abs() < 1e-9);
    }

    #[test]
    fn overage_reports_past_full() {
        let mut body = sample();
        body["data"]["remainingCredits"] = serde_json::json!(-25_000);
        assert!(parse_window(&body).unwrap().used_pct > 100.0);
    }

    #[test]
    fn bonus_credits_keep_their_exact_amount_without_negative_usage() {
        let mut body = sample();
        body["data"]["remainingCredits"] = serde_json::json!(1_025);
        body["data"]["planCredits"] = serde_json::json!(1_000);
        let window = parse_window(&body).unwrap();
        assert_eq!(window.used_pct, 0.0);
        assert_eq!(window.allowance.unwrap().remaining, 1_025.0);
    }

    #[test]
    fn missing_period_start_falls_back_to_calendar_month() {
        let mut body = sample();
        body["data"]["billingPeriodStart"] = Value::Null;
        let w = parse_window(&body).unwrap();
        assert_eq!(
            w.period_start,
            w.resets_at
                .unwrap()
                .checked_sub_months(chrono::Months::new(1))
        );
    }

    #[test]
    fn absent_period_leaves_the_window_renderable() {
        let body = serde_json::json!({
            "data": {"remainingCredits": 1, "planCredits": 2}
        });
        let w = parse_window(&body).unwrap();
        assert!(w.resets_at.is_none());
        assert!(w.period_start.is_none());
        assert!((w.used_pct - 50.0).abs() < 1e-9);
    }

    #[test]
    fn missing_or_zero_plan_is_none() {
        assert!(parse_window(&serde_json::json!({"data": {"remainingCredits": 5}})).is_none());
        let mut body = sample();
        body["data"]["planCredits"] = serde_json::json!(0);
        assert!(parse_window(&body).is_none());
    }
}
