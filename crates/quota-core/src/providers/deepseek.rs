//! DeepSeek balance via the official, documented API: `GET /user/balance`.
//! A pasted key and nothing else, like OpenRouter.
//!
//! Two quirks worth knowing. The amounts come back as JSON *strings*
//! ("110.00"), which the shared `as_f64` helper already handles. And the
//! response is a list, one entry per currency — accounts are normally
//! single-currency, but the shape permits several.

use super::as_f64;
use super::simple_credits::CreditsSpec;
use crate::model::Credits;
use serde_json::Value;

pub const SPEC: CreditsSpec = CreditsSpec {
    kind: "deepseek",
    display_name: "DeepSeek",
    default_url: "https://api.deepseek.com/user/balance",
    url_setting: "balance_url",
    not_configured: "paste a DeepSeek API key in Settings",
    parse: parse_credits,
};

/// The first balance entry, preferring USD when the account reports several.
/// Picking by position alone would let the displayed currency change between
/// polls if the API reorders them.
fn parse_credits(body: &Value) -> Option<Credits> {
    let infos = body.get("balance_infos")?.as_array()?;
    let info = infos
        .iter()
        .find(|i| i.get("currency").and_then(Value::as_str) == Some("USD"))
        .or_else(|| infos.first())?;

    let balance = info.get("total_balance").and_then(as_f64)?;
    let granted = info.get("granted_balance").and_then(as_f64);
    let topped_up = info.get("topped_up_balance").and_then(as_f64);
    Some(Credits {
        balance,
        // A real draw-down balance, so it needs no explanatory label.
        label: None,
        unit: info
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD")
            .to_string(),
        // DeepSeek reports what remains, never what was spent. `used` stays
        // None rather than being invented from the two sub-balances, which sum
        // to the total rather than to any spend figure.
        used: None,
        granted: match (granted, topped_up) {
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
            "is_available": true,
            "balance_infos": [{
                "currency": "CNY",
                "total_balance": "110.00",
                "granted_balance": "10.00",
                "topped_up_balance": "100.00"
            }]
        })
    }

    #[test]
    fn parses_documented_shape_with_string_amounts() {
        let c = parse_credits(&sample()).unwrap();
        assert!((c.balance - 110.0).abs() < 1e-9);
        assert_eq!(c.unit, "CNY");
        assert_eq!(c.granted, Some(110.0));
        assert_eq!(c.used, None);
    }

    #[test]
    fn prefers_usd_when_several_currencies_are_reported() {
        let mut body = sample();
        body["balance_infos"] = serde_json::json!([
            {"currency": "CNY", "total_balance": "110.00"},
            {"currency": "USD", "total_balance": "15.50"}
        ]);
        let c = parse_credits(&body).unwrap();
        assert_eq!(c.unit, "USD");
        assert!((c.balance - 15.5).abs() < 1e-9);
    }

    #[test]
    fn exhausted_balance_is_zero_not_missing() {
        let mut body = sample();
        body["balance_infos"][0]["total_balance"] = serde_json::json!("0.00");
        assert_eq!(parse_credits(&body).unwrap().balance, 0.0);
    }

    #[test]
    fn empty_or_missing_list_is_none() {
        assert!(parse_credits(&serde_json::json!({"balance_infos": []})).is_none());
        assert!(parse_credits(&serde_json::json!({"is_available": true})).is_none());
    }
}
