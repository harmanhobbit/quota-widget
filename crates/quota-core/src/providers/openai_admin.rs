//! OpenAI organization spend via the documented Admin API:
//! `GET /v1/organization/costs`.
//!
//! Needs an **admin** key (an Organization key with admin scope), not a normal
//! API key.
//!
//! Note the contrast with `anthropic_admin`: OpenAI reports `amount` as a
//! nested object, `{"value": 0.06, "currency": "usd"}`, and the value is in
//! **dollars** — no cents conversion. The two admin APIs disagree on both the
//! shape and the unit, which is why they don't share a parser.

use super::spend::{month_bounds, monthly_budget, spend_snapshot};
use super::{as_f64, network_err, Provider, ProviderCtx};
use crate::model::{FetchError, UsageSnapshot};
use serde_json::Value;

pub struct OpenAiAdmin {
    pub key: String,
    pub label: Option<String>,
}
impl OpenAiAdmin {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

const COSTS_URL: &str = "https://api.openai.com/v1/organization/costs";

#[async_trait::async_trait]
impl Provider for OpenAiAdmin {
    fn kind(&self) -> &'static str {
        "openai_admin"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("OpenAI Admin")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured(
                    "paste an OpenAI Admin API key in Settings — a normal API key will not work"
                        .into(),
                )
            })?;
        let url = ctx
            .config
            .provider_setting(&self.key, "costs_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| COSTS_URL.to_string());

        let now = chrono::Utc::now();
        let (start, end) = month_bounds(now).ok_or_else(|| {
            FetchError::Parse("could not determine the current billing month".into())
        })?;

        let resp = ctx
            .http
            .get(&url)
            // start_time is Unix seconds here, not RFC 3339. The default page
            // size is small, so ask for enough buckets to cover a long month.
            .query(&[
                ("start_time", start.timestamp().to_string()),
                ("bucket_width", "1d".to_string()),
                ("limit", "31".to_string()),
            ])
            .bearer_auth(key)
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "key rejected — this endpoint needs an admin key, not a normal API key".into(),
                ))
            }
            s => {
                return Err(FetchError::Network(format!(
                    "HTTP {s} from organization costs endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
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

/// Month-to-date spend in USD, summed across every bucket's results.
///
/// `amount` is `{"value": <number>, "currency": "usd"}` and already in dollars.
/// The value is also accepted as a bare number so a shape change doesn't
/// silently report zero spend.
fn total_spend_usd(body: &Value) -> f64 {
    body.get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|bucket| bucket.get("results"))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|item| item.get("amount"))
        .filter_map(|amount| {
            amount
                .get("value")
                .and_then(as_f64)
                .or_else(|| as_f64(amount))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "object": "page",
            "data": [{
                "object": "bucket",
                "start_time": 1_785_542_400i64,
                "end_time": 1_785_628_800i64,
                "results": [
                    {"object": "organization.costs.result",
                     "amount": {"value": 0.06, "currency": "usd"},
                     "line_item": null, "project_id": null},
                    {"object": "organization.costs.result",
                     "amount": {"value": 1.44, "currency": "usd"},
                     "line_item": null, "project_id": null}
                ]
            }],
            "has_more": false,
            "next_page": null
        })
    }

    #[test]
    fn parses_documented_shape_in_dollars() {
        // Dollars, unlike the Anthropic admin report's cents.
        assert!((total_spend_usd(&sample()) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn sums_across_buckets() {
        let mut body = sample();
        let extra = serde_json::json!({
            "object": "bucket",
            "results": [{"amount": {"value": 2.5, "currency": "usd"}}]
        });
        body["data"].as_array_mut().unwrap().push(extra);
        assert!((total_spend_usd(&body) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn a_month_with_no_spend_is_zero() {
        assert_eq!(total_spend_usd(&serde_json::json!({"data": []})), 0.0);
        assert_eq!(
            total_spend_usd(&serde_json::json!({"data": [{"results": []}]})),
            0.0
        );
    }

    #[test]
    fn a_bare_numeric_amount_still_parses() {
        let body = serde_json::json!({"data": [{"results": [{"amount": 3.25}]}]});
        assert!((total_spend_usd(&body) - 3.25).abs() < 1e-9);
    }
}
