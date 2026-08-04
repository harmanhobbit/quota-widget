//! Moonshot / Kimi balance via the official, documented API:
//! `GET /v1/users/me/balance`.
//!
//! **Keys are platform-specific.** `platform.kimi.ai` and `platform.kimi.com`
//! issue independent keys, and using one against the other's host returns 401 —
//! which is why the base URL is an overridable per-account setting rather than
//! a constant. The `.ai` host is the default; an account whose key came from
//! the `.com` platform sets `balance_url` accordingly.

use super::as_f64;
use super::simple_credits::CreditsSpec;
use crate::model::Credits;
use serde_json::Value;

pub const SPEC: CreditsSpec = CreditsSpec {
    kind: "moonshot",
    display_name: "Moonshot",
    default_url: "https://api.moonshot.ai/v1/users/me/balance",
    url_setting: "balance_url",
    not_configured: "paste a Moonshot API key in Settings",
    parse: parse_credits,
};

/// `available_balance` is the figure that actually gates calls: Moonshot
/// documents that requests fail with `exceeded_current_quota_error` once it
/// reaches zero. The cash/voucher split is how that total is funded, so it is
/// reported as the grant rather than as separate quantities.
fn parse_credits(body: &Value) -> Option<Credits> {
    let data = body.get("data").unwrap_or(body);
    let balance = data.get("available_balance").and_then(as_f64)?;
    let cash = data.get("cash_balance").and_then(as_f64);
    let voucher = data.get("voucher_balance").and_then(as_f64);
    Some(Credits {
        balance,
        // Moonshot bills in USD on the .ai platform; the response carries no
        // currency field to read instead.
        unit: "USD".into(),
        // Only remaining amounts are reported, never spend.
        used: None,
        granted: match (cash, voucher) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        },
        est_tokens_remaining: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "code": 0,
            "data": {
                "available_balance": 49.58894,
                "voucher_balance": 46.58893,
                "cash_balance": 3.00001
            },
            "scode": "0x0",
            "status": true
        })
    }

    #[test]
    fn parses_documented_shape() {
        let c = parse_credits(&sample()).unwrap();
        assert!((c.balance - 49.58894).abs() < 1e-9);
        assert_eq!(c.unit, "USD");
        // Cash and voucher fund the available total.
        assert!((c.granted.unwrap() - 49.58894).abs() < 1e-5);
        assert_eq!(c.used, None);
    }

    #[test]
    fn exhausted_balance_is_zero_not_missing() {
        let mut body = sample();
        body["data"]["available_balance"] = serde_json::json!(0);
        assert_eq!(parse_credits(&body).unwrap().balance, 0.0);
    }

    #[test]
    fn negative_balance_survives() {
        let mut body = sample();
        body["data"]["available_balance"] = serde_json::json!(-1.5);
        assert!(parse_credits(&body).unwrap().balance < 0.0);
    }

    #[test]
    fn missing_available_balance_is_none() {
        assert!(parse_credits(&serde_json::json!({"data": {"cash_balance": 1}})).is_none());
        assert!(parse_credits(&serde_json::json!({"code": 0})).is_none());
    }
}
