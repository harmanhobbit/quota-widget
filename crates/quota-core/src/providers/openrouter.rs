//! OpenRouter credits via the official, documented API. The one adapter with
//! no caveats: paste an API key in settings and it works.

use super::{as_f64, network_err, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot};
use serde_json::Value;

pub struct OpenRouter;

const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";

#[async_trait::async_trait]
impl Provider for OpenRouter {
    fn id(&self) -> &'static str {
        "openrouter"
    }
    fn name(&self) -> &'static str {
        "OpenRouter"
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx.secrets.get("openrouter").filter(|k| !k.is_empty()).ok_or_else(|| {
            FetchError::NotConfigured("paste an OpenRouter API key in Settings".into())
        })?;
        let url = ctx
            .config
            .provider_setting("openrouter", "credits_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| CREDITS_URL.to_string());

        let resp = ctx.http.get(&url).bearer_auth(key).send().await.map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired("API key rejected — re-check it in Settings".into()))
            }
            s => return Err(FetchError::Network(format!("HTTP {s} from credits endpoint"))),
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let credits = parse_credits(&body)
            .ok_or_else(|| FetchError::Parse("credits response missing totals".into()))?;
        Ok(UsageSnapshot::ok(self.id(), self.name(), vec![], Some(credits)))
    }
}

fn parse_credits(body: &Value) -> Option<Credits> {
    let data = body.get("data").unwrap_or(body);
    let granted = data.get("total_credits").and_then(as_f64)?;
    let used = data.get("total_usage").and_then(as_f64).unwrap_or(0.0);
    Some(Credits {
        balance: granted - used,
        unit: "USD".into(),
        used: Some(used),
        granted: Some(granted),
        est_tokens_remaining: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_shape() {
        let body = serde_json::json!({"data": {"total_credits": 25.0, "total_usage": 21.58}});
        let c = parse_credits(&body).unwrap();
        assert!((c.balance - 3.42).abs() < 1e-9);
        assert_eq!(c.granted, Some(25.0));
        assert_eq!(c.unit, "USD");
    }

    #[test]
    fn missing_totals_is_none() {
        assert!(parse_credits(&serde_json::json!({"data": {}})).is_none());
    }
}
