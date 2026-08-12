//! OneHop balance via `GET /v1/user/balance`.
//!
//! OneHop is a gateway — one key in front of many upstream models — billed from
//! a prepaid wallet, so what it reports is a draw-down balance like OpenRouter's
//! rather than a per-cycle allowance.
//!
//! **Undocumented.** OneHop's published docs describe the wallet but specify no
//! endpoint for it, and point at the web console instead. This path was found by
//! probing and is confirmed working, but it carries no compatibility promise —
//! treat it like the Claude and Codex endpoints, not like DeepSeek's. It is
//! nonetheless the *right* one to use: the console's own billing page is
//! authenticated with a full-access session cookie, whereas this answers to a
//! scoped API key.
//!
//! The response is small and unambiguous:
//!
//! ```json
//! { "balance": 0, "is_active": true, "currency": "USD" }
//! ```

use super::as_f64;
use super::simple_credits::CreditsSpec;
use crate::model::Credits;
use serde_json::Value;

pub const SPEC: CreditsSpec = CreditsSpec {
    kind: "onehop",
    display_name: "OneHop",
    default_url: "https://api.onehop.ai/v1/user/balance",
    url_setting: "balance_url",
    not_configured: "paste a OneHop API key in Settings",
    parse: parse_credits,
};

fn parse_credits(body: &Value) -> Option<Credits> {
    // `as_f64` rather than `Value::as_f64`: the observed response used a bare
    // number, but a zero balance cannot prove the field is never a string, and
    // the shared helper accepts both.
    let balance = body.get("balance").and_then(as_f64)?;
    Some(Credits {
        balance,
        // A real draw-down balance, so no explanatory label — unlike the spend
        // providers, which must say "Cost this month" to avoid being read as
        // money remaining.
        label: None,
        // Reported explicitly rather than assumed. The account probed was USD;
        // defaulting keeps a currency-less response readable instead of failing.
        unit: body
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD")
            .to_string(),
        // The endpoint reports only what remains. Nothing here describes spend
        // or an original grant, and inventing either would be a fiction.
        used: None,
        granted: None,
        est_tokens_remaining: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from a live account, so the happy path is pinned to reality
    /// rather than to an assumption about the shape.
    fn sample() -> Value {
        serde_json::json!({ "balance": 0, "is_active": true, "currency": "USD" })
    }

    #[test]
    fn parses_the_observed_shape() {
        let c = parse_credits(&sample()).unwrap();
        assert_eq!(c.balance, 0.0);
        assert_eq!(c.unit, "USD");
        assert_eq!(c.used, None);
        assert_eq!(c.granted, None);
        assert_eq!(c.label, None);
    }

    /// A funded account is the case the probe could not reach, so cover it here:
    /// a non-zero balance must survive as a fraction, not be truncated.
    #[test]
    fn parses_a_funded_balance() {
        let mut body = sample();
        body["balance"] = serde_json::json!(12.34);
        assert!((parse_credits(&body).unwrap().balance - 12.34).abs() < 1e-9);
    }

    /// Zero is a real balance — an exhausted wallet — and must not read as
    /// "no data", which is what a None here would render as.
    #[test]
    fn exhausted_wallet_is_zero_not_missing() {
        assert_eq!(parse_credits(&sample()).unwrap().balance, 0.0);
    }

    /// String amounts are not what this endpoint returned, but DeepSeek proves
    /// providers do this, and a zero-balance probe could not have revealed it.
    #[test]
    fn accepts_a_string_amount() {
        let mut body = sample();
        body["balance"] = serde_json::json!("7.50");
        assert!((parse_credits(&body).unwrap().balance - 7.5).abs() < 1e-9);
    }

    #[test]
    fn non_usd_currency_is_carried_through() {
        let mut body = sample();
        body["currency"] = serde_json::json!("EUR");
        assert_eq!(parse_credits(&body).unwrap().unit, "EUR");
    }

    /// An unrecognisable body must surface as a parse error rather than as a
    /// confident zero — the failure mode the README warns about.
    #[test]
    fn missing_balance_is_none() {
        assert!(parse_credits(&serde_json::json!({"is_active": true})).is_none());
        assert!(parse_credits(&serde_json::json!({})).is_none());
    }
}
