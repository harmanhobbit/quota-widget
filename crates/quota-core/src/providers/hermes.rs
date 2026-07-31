//! Hermes Portal (Nous Research). There is no public balance API (open feature
//! request on the hermes-agent repo), so this adapter calls a portal endpoint
//! with a pasted session cookie and scans the JSON leniently for balance-like
//! fields. The endpoint is configurable in settings (`endpoint`) because it
//! must be discovered from the portal dashboard's own network traffic and may
//! change. If a per-token price is configured (`token_price`), the card also
//! shows an estimated-tokens-remaining figure.

use super::{as_f64, network_err, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot};
use serde_json::Value;

pub struct Hermes;

const DEFAULT_ENDPOINT: &str = "https://portal.nousresearch.com/api/user/credits";

#[async_trait::async_trait]
impl Provider for Hermes {
    fn id(&self) -> &'static str {
        "hermes"
    }
    fn name(&self) -> &'static str {
        "Hermes Portal"
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let cookie = ctx.secrets.get("hermes").filter(|c| !c.is_empty()).ok_or_else(|| {
            FetchError::NotConfigured(
                "paste your portal.nousresearch.com session cookie in Settings".into(),
            )
        })?;
        let endpoint = ctx
            .config
            .provider_setting("hermes", "endpoint")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

        let resp = ctx
            .http
            .get(&endpoint)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "session cookie expired — log in to the portal and re-paste it".into(),
                ))
            }
            s => {
                return Err(FetchError::Network(format!(
                    "HTTP {s} from {endpoint} — the portal endpoint may have changed; \
                     capture the current one via browser DevTools and set it in Settings"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let token_price = ctx.config.provider_setting("hermes", "token_price").and_then(|v| as_f64(&v));
        let credits = parse_credits(&body, token_price).ok_or_else(|| {
            FetchError::Parse(
                "no balance-like field found in response — the endpoint shape may have changed"
                    .into(),
            )
        })?;
        Ok(UsageSnapshot::ok(self.id(), self.name(), vec![], Some(credits)))
    }
}

/// Field names that plausibly hold the remaining balance / usage, in priority
/// order. Matched case-insensitively against every key in the JSON tree.
const BALANCE_KEYS: &[&str] =
    &["credits_remaining", "creditsremaining", "remaining_credits", "credit_balance", "balance", "credits"];
const USED_KEYS: &[&str] = &["credits_used", "creditsused", "used_credits", "usage", "used"];

fn parse_credits(body: &Value, token_price: Option<f64>) -> Option<Credits> {
    let balance = find_numeric(body, BALANCE_KEYS)?;
    let used = find_numeric(body, USED_KEYS);
    let est_tokens_remaining = token_price.filter(|p| *p > 0.0).map(|p| balance / p);
    Some(Credits { balance, unit: "credits".into(), used, granted: None, est_tokens_remaining })
}

/// Depth-first search for the first numeric value whose key matches `keys`
/// (earlier entries in `keys` win over later ones at any depth).
fn find_numeric(v: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(n) = find_by_key(v, key) {
            return Some(n);
        }
    }
    None
}

fn find_by_key(v: &Value, key: &str) -> Option<f64> {
    match v {
        Value::Object(obj) => {
            for (k, val) in obj {
                if k.to_ascii_lowercase() == key {
                    if let Some(n) = as_f64(val) {
                        return Some(n);
                    }
                }
            }
            obj.values().find_map(|val| find_by_key(val, key))
        }
        Value::Array(arr) => arr.iter().find_map(|val| find_by_key(val, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_balance_and_estimates_tokens() {
        let body = serde_json::json!({
            "user": {"plan": "hermes-pro"},
            "billing": {"credits_remaining": 1234.5, "credits_used": 765.5}
        });
        let c = parse_credits(&body, Some(0.000002)).unwrap();
        assert_eq!(c.balance, 1234.5);
        assert_eq!(c.used, Some(765.5));
        assert!((c.est_tokens_remaining.unwrap() - 617_250_000.0).abs() < 1.0);
    }

    #[test]
    fn priority_order_prefers_specific_keys() {
        // "balance" exists, but "credits_remaining" is more specific and wins.
        let body = serde_json::json!({"balance": 1.0, "credits_remaining": 42.0});
        assert_eq!(parse_credits(&body, None).unwrap().balance, 42.0);
    }

    #[test]
    fn no_balance_field_is_none() {
        assert!(parse_credits(&serde_json::json!({"ok": true}), None).is_none());
    }
}
