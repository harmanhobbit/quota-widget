//! Anthropic organization spend via the documented Admin API:
//! `GET /v1/organizations/cost_report`.
//!
//! Needs an **admin** key (`sk-ant-admin...`), not a normal API key, and the
//! Admin API is unavailable on individual accounts — both are surfaced as
//! actionable errors rather than as a bare 401.
//!
//! **Amounts are in cents.** The API documents `amount` as "cost amount in
//! lowest currency units (e.g. cents) as a decimal string" — `"123.45"` in USD
//! is $1.23. Treating it as dollars overstates spend by 100×, which is the one
//! thing this adapter must not get wrong.

use super::spend::{month_bounds, monthly_budget, spend_snapshot};
use super::{as_f64, network_err, Provider, ProviderCtx};
use crate::model::{FetchError, UsageSnapshot};
use serde_json::Value;

pub struct AnthropicAdmin {
    pub key: String,
    pub label: Option<String>,
}
impl AnthropicAdmin {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

const COST_REPORT_URL: &str = "https://api.anthropic.com/v1/organizations/cost_report";
const CENTS_PER_USD: f64 = 100.0;

#[async_trait::async_trait]
impl Provider for AnthropicAdmin {
    fn kind(&self) -> &'static str {
        "anthropic_admin"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Anthropic Admin")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured(
                    "paste an Anthropic Admin API key (sk-ant-admin…) in Settings".into(),
                )
            })?;
        let url = ctx
            .config
            .provider_setting(&self.key, "cost_report_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| COST_REPORT_URL.to_string());

        let now = chrono::Utc::now();
        let (start, end) = month_bounds(now).ok_or_else(|| {
            FetchError::Parse("could not determine the current billing month".into())
        })?;

        let resp = ctx
            .http
            .get(&url)
            // Buckets are daily only, and `ending_at` is exclusive — asking for
            // the month so far means "now", not the month's end.
            .query(&[
                ("starting_at", start.to_rfc3339()),
                ("ending_at", now.to_rfc3339()),
                ("bucket_width", "1d".to_string()),
            ])
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "key rejected — the Admin API needs an sk-ant-admin key, and is \
                     unavailable on individual accounts"
                        .into(),
                ))
            }
            s => {
                return Err(FetchError::Network(format!(
                    "HTTP {s} from cost report endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        // A month with no spend yet is $0, not an error, so an empty report
        // parses rather than failing.
        let spend = total_spend_usd(&body);

        Ok(spend_snapshot(
            self.id(),
            self.name(),
            spend,
            monthly_budget(&ctx.config, &self.key),
            start,
            end,
        ))
    }
}

/// Month-to-date spend in USD, summed across every daily bucket's cost items.
///
/// Only `data[].results[].amount` is read — deliberately not a recursive walk
/// for any field named `amount`, which would double-count if the response ever
/// gained a nested total.
fn total_spend_usd(body: &Value) -> f64 {
    body.get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|bucket| bucket.get("results"))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|item| item.get("amount").and_then(as_f64))
        .sum::<f64>()
        / CENTS_PER_USD
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "data": [{
                "starting_at": "2026-08-01T00:00:00Z",
                "ending_at": "2026-08-02T00:00:00Z",
                "results": [
                    {"amount": "123.45", "currency": "USD", "cost_type": "tokens",
                     "token_type": "uncached_input_tokens", "model": "claude-opus-4-6"},
                    {"amount": "76.55", "currency": "USD", "cost_type": "tokens",
                     "token_type": "output_tokens", "model": "claude-opus-4-6"}
                ]
            }],
            "has_more": false
        })
    }

    #[test]
    fn amounts_are_cents_not_dollars() {
        // 123.45 + 76.55 = 200 cents = $2.00. Read as dollars this would be
        // $200 — the 100x error this test exists to catch.
        assert!((total_spend_usd(&sample()) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn sums_across_daily_buckets() {
        let mut body = sample();
        let extra = serde_json::json!({
            "starting_at": "2026-08-02T00:00:00Z",
            "ending_at": "2026-08-03T00:00:00Z",
            "results": [{"amount": "100", "currency": "USD"}]
        });
        body["data"].as_array_mut().unwrap().push(extra);
        assert!((total_spend_usd(&body) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_month_with_no_spend_is_zero() {
        assert_eq!(total_spend_usd(&serde_json::json!({"data": []})), 0.0);
        // A bucket that exists but reported nothing.
        assert_eq!(
            total_spend_usd(&serde_json::json!({"data": [{"results": []}]})),
            0.0
        );
    }

    #[test]
    fn ignores_amounts_outside_the_documented_path() {
        // A nested total elsewhere in the payload must not be double-counted.
        let mut body = sample();
        body["total"] = serde_json::json!({"amount": "999999"});
        assert!((total_spend_usd(&body) - 2.0).abs() < 1e-9);
    }
}
