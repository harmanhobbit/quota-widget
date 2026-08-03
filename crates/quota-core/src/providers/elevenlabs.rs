//! ElevenLabs credit allowance via the official, documented API. Like
//! OpenRouter it needs nothing but a pasted key — but the quantity it reports
//! is a per-cycle allowance rather than a balance, so it renders as a usage
//! window (Claude's weekly cap) rather than as `Credits`.

use super::{as_f64, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{FetchError, UsageSnapshot, UsageWindow};
use serde_json::Value;

pub struct ElevenLabs {
    pub key: String,
    pub label: Option<String>,
}
impl ElevenLabs {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

const SUBSCRIPTION_URL: &str = "https://api.elevenlabs.io/v1/user/subscription";

#[async_trait::async_trait]
impl Provider for ElevenLabs {
    fn kind(&self) -> &'static str {
        "elevenlabs"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("ElevenLabs")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured("paste an ElevenLabs API key in Settings".into())
            })?;
        let url = ctx
            .config
            .provider_setting(&self.key, "subscription_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| SUBSCRIPTION_URL.to_string());

        let resp = ctx
            .http
            .get(&url)
            .header("xi-api-key", key)
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
                    "HTTP {s} from subscription endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let window = parse_window(&body).ok_or_else(|| {
            FetchError::Parse("subscription response missing character counts".into())
        })?;
        Ok(UsageSnapshot::ok(
            self.id(),
            self.name(),
            vec![window],
            None,
        ))
    }
}

/// The credit allowance for the current billing cycle. ElevenLabs calls these
/// "credits" in its billing UI but still names the JSON fields
/// `character_count`/`character_limit`. A zero limit is treated as unparseable
/// rather than as 100% used — it means no allowance shape we can render.
fn parse_window(body: &Value) -> Option<UsageWindow> {
    let used = body.get("character_count").and_then(as_f64)?;
    let limit = body.get("character_limit").and_then(as_f64)?;
    if limit <= 0.0 {
        return None;
    }
    let tier = body.get("tier").and_then(|v| v.as_str());
    Some(UsageWindow {
        metric_id: "monthly_credits".into(),
        label: match tier {
            Some(t) if !t.is_empty() => format!("Credits ({t})"),
            _ => "Credits".into(),
        },
        // Plans with credit-limit extension enabled can exceed the limit; the
        // model tolerates >100 and the UI clamps its own bars.
        used_pct: used / limit * 100.0,
        resets_at: body
            .get("next_character_count_reset_unix")
            .and_then(parse_timestamp),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "tier": "starter",
            "character_count": 1000,
            "character_limit": 10000,
            "status": "active",
            "next_character_count_reset_unix": 1_738_356_858i64,
            "billing_period": "monthly_period"
        })
    }

    #[test]
    fn parses_documented_shape() {
        let w = parse_window(&sample()).unwrap();
        assert_eq!(w.metric_id, "monthly_credits");
        assert_eq!(w.label, "Credits (starter)");
        assert!((w.used_pct - 10.0).abs() < 1e-9);
        assert!(w.resets_at.is_some());
        assert!(!w.informational);
    }

    #[test]
    fn overage_reports_past_full() {
        let mut body = sample();
        body["character_count"] = serde_json::json!(12_500);
        assert!(parse_window(&body).unwrap().used_pct > 100.0);
    }

    #[test]
    fn untiered_response_still_labels() {
        let mut body = sample();
        body["tier"] = serde_json::json!("");
        assert_eq!(parse_window(&body).unwrap().label, "Credits");
    }

    #[test]
    fn missing_or_zero_limit_is_none() {
        assert!(parse_window(&serde_json::json!({"character_count": 5})).is_none());
        let mut body = sample();
        body["character_limit"] = serde_json::json!(0);
        assert!(parse_window(&body).is_none());
    }
}
