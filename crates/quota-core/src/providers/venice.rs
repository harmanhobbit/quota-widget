//! Venice balances and rate limits via the official, documented API:
//! `GET /api/v1/api_keys/rate_limits`.
//!
//! Not a `simple_credits` provider, for two reasons. Venice reports **two**
//! balances — USD and DIEM — and `Credits` holds one, so which is shown is a
//! per-account choice that a `CreditsSpec` parse fn cannot see (it receives the
//! body alone, with no config). And the same response carries per-model rate
//! limits, which are a genuinely different quantity from a balance.
//!
//! The documented shape:
//!
//! ```json
//! { "data": {
//!     "accessPermitted": true,
//!     "apiTier": { "id": "paid", "isCharged": true },
//!     "balances": { "USD": 50.23, "DIEM": 100.023 },
//!     "keyExpiration": "2025-06-01T00:00:00.000Z",
//!     "nextEpochBegins": "2025-05-07T00:00:00.000Z",
//!     "rateLimits": [ { "apiModelId": "…",
//!                       "rateLimits": [ { "amount": 100, "type": "RPM" } ] } ] } }
//! ```
//!
//! `nextEpochBegins` is a real reset instant rather than an inferred one, so it
//! is carried onto the window it belongs to.

use super::{as_f64, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot, UsageWindow};
use serde_json::Value;

pub struct Venice {
    pub key: String,
    pub label: Option<String>,
}
impl Venice {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

const RATE_LIMITS_URL: &str = "https://api.venice.ai/api/v1/api_keys/rate_limits";

/// Which balance heads the card. Venice funds calls from both, so neither is
/// universally "the" balance — a DIEM staker and a USD top-up user each want a
/// different one.
fn preferred_currency(ctx: &ProviderCtx, account: &str) -> String {
    ctx.config
        .provider_setting(account, "balance_currency")
        .and_then(|v| v.as_str().map(str::to_ascii_uppercase))
        .unwrap_or_else(|| "USD".into())
}

#[async_trait::async_trait]
impl Provider for Venice {
    fn kind(&self) -> &'static str {
        "venice"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Venice")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured("paste a Venice API key in Settings".into())
            })?;
        let url = ctx
            .config
            .provider_setting(&self.key, "rate_limits_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| RATE_LIMITS_URL.to_string());

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
            // 402 is Venice saying the key is valid but out of funds. Reporting
            // that as a network fault would hide the one thing this widget
            // exists to show, so it is surfaced as a zero balance instead.
            402 => {
                return Ok(UsageSnapshot::ok(
                    self.id(),
                    self.name(),
                    vec![],
                    Some(Credits {
                        balance: 0.0,
                        label: None,
                        unit: preferred_currency(ctx, &self.key),
                        used: None,
                        granted: None,
                        est_tokens_remaining: None,
                    }),
                ))
            }
            s => return Err(FetchError::Network(format!("HTTP {s} from rate limits"))),
        }

        let body: Value = resp.json().await.map_err(network_err)?;
        let want = preferred_currency(ctx, &self.key);
        let (credits, windows) = parse(&body, &want)
            .ok_or_else(|| FetchError::Parse("rate limits response missing balances".into()))?;
        Ok(UsageSnapshot::ok(self.id(), self.name(), windows, credits))
    }
}

/// Split the response into the headline balance and any informational windows.
///
/// `want` selects between the reported currencies. An unknown or absent one
/// falls back to whatever the account actually has rather than failing: a key
/// configured for DIEM on an account that only holds USD should still show a
/// number.
fn parse(body: &Value, want: &str) -> Option<(Option<Credits>, Vec<UsageWindow>)> {
    let data = body.get("data").unwrap_or(body);
    let balances = data.get("balances")?.as_object()?;

    // Exact match first, then USD, then anything present, so the card degrades
    // to a real figure instead of an empty one.
    let (unit, amount) = balances
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(want))
        .or_else(|| balances.iter().find(|(k, _)| k.eq_ignore_ascii_case("USD")))
        .or_else(|| balances.iter().next())?;
    let balance = as_f64(amount)?;

    let credits = Some(Credits {
        balance,
        // A real draw-down balance: no explanatory label.
        label: None,
        unit: unit.to_ascii_uppercase(),
        used: None,
        granted: None,
        est_tokens_remaining: None,
    });

    let mut windows = Vec::new();

    // A revoked or exhausted key still returns 200 with accessPermitted:false.
    // Without this the card would look healthy while every call was failing.
    if data.get("accessPermitted").and_then(Value::as_bool) == Some(false) {
        windows.push(UsageWindow {
            metric_id: "access".into(),
            label: "API access suspended".into(),
            used_pct: 100.0,
            informational: true,
            period_start: None,
            resets_at: data.get("nextEpochBegins").and_then(parse_timestamp),
            allowance: None,
        });
    }

    // The other reported balance, shown for reference only. Informational, so
    // it never colours the tray or fires an alert — it is not the quantity the
    // user chose as their headline.
    for (other_unit, other_amount) in balances.iter() {
        if other_unit.eq_ignore_ascii_case(unit) {
            continue;
        }
        let Some(value) = as_f64(other_amount) else {
            continue;
        };
        windows.push(UsageWindow {
            metric_id: format!("balance_{}", other_unit.to_ascii_lowercase()),
            label: format!("{} balance: {:.2}", other_unit.to_ascii_uppercase(), value),
            // A balance has no meaningful "percent used" — Venice reports what
            // remains, never an original grant — so this stays at zero and
            // informational, carrying its number in the label.
            used_pct: 0.0,
            informational: true,
            period_start: None,
            resets_at: None,
            allowance: None,
        });
    }

    Some((credits, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented shape, field for field.
    fn sample() -> Value {
        serde_json::json!({
            "data": {
                "accessPermitted": true,
                "apiTier": { "id": "paid", "isCharged": true },
                "balances": { "USD": 50.23, "DIEM": 100.023 },
                "keyExpiration": "2025-06-01T00:00:00.000Z",
                "nextEpochBegins": "2025-05-07T00:00:00.000Z",
                "rateLimits": [{
                    "apiModelId": "zai-org-glm-5-1",
                    "rateLimits": [{ "amount": 100, "type": "RPM" }]
                }]
            }
        })
    }

    #[test]
    fn defaults_to_usd_and_shows_diem_for_reference() {
        let (credits, windows) = parse(&sample(), "USD").unwrap();
        let c = credits.unwrap();
        assert!((c.balance - 50.23).abs() < 1e-9);
        assert_eq!(c.unit, "USD");
        assert_eq!(c.label, None);
        // DIEM appears as an informational row, never as a second headline.
        let diem = windows
            .iter()
            .find(|w| w.metric_id == "balance_diem")
            .unwrap();
        assert!(diem.informational);
        assert!(diem.label.contains("100.02"));
    }

    #[test]
    fn honours_a_diem_preference() {
        let (credits, windows) = parse(&sample(), "DIEM").unwrap();
        let c = credits.unwrap();
        assert!((c.balance - 100.023).abs() < 1e-9);
        assert_eq!(c.unit, "DIEM");
        assert!(windows.iter().any(|w| w.metric_id == "balance_usd"));
    }

    /// A preference for a currency the account does not hold must still show a
    /// number rather than an empty card.
    #[test]
    fn unknown_preference_falls_back_rather_than_failing() {
        let mut body = sample();
        body["data"]["balances"] = serde_json::json!({ "USD": 4.0 });
        let (credits, _) = parse(&body, "DIEM").unwrap();
        assert_eq!(credits.unwrap().unit, "USD");
    }

    #[test]
    fn suspended_access_is_surfaced_not_hidden() {
        let mut body = sample();
        body["data"]["accessPermitted"] = serde_json::json!(false);
        let (_, windows) = parse(&body, "USD").unwrap();
        let access = windows.iter().find(|w| w.metric_id == "access").unwrap();
        assert!(access.informational);
        assert!(
            access.resets_at.is_some(),
            "nextEpochBegins should carry over"
        );
    }

    /// Zero is a real balance — an exhausted wallet — and must not read as
    /// missing data.
    #[test]
    fn exhausted_balance_is_zero_not_missing() {
        let mut body = sample();
        body["data"]["balances"] = serde_json::json!({ "USD": 0 });
        assert_eq!(parse(&body, "USD").unwrap().0.unwrap().balance, 0.0);
    }

    /// String amounts are not documented here, but DeepSeek proves providers do
    /// it, and the shared helper accepts both.
    #[test]
    fn accepts_string_amounts() {
        let mut body = sample();
        body["data"]["balances"] = serde_json::json!({ "USD": "12.50" });
        assert!((parse(&body, "USD").unwrap().0.unwrap().balance - 12.5).abs() < 1e-9);
    }

    /// Tolerate the envelope being dropped, since the widget cannot control
    /// whether Venice keeps wrapping in `data`.
    #[test]
    fn accepts_an_unwrapped_body() {
        let unwrapped = serde_json::json!({ "balances": { "USD": 1.5 } });
        assert_eq!(parse(&unwrapped, "USD").unwrap().0.unwrap().balance, 1.5);
    }

    #[test]
    fn missing_balances_is_none() {
        assert!(parse(&serde_json::json!({"data": {}}), "USD").is_none());
        assert!(parse(&serde_json::json!({}), "USD").is_none());
    }
}
