//! Z.ai (Zhipu AI) GLM Coding Plan usage via the same quota endpoint the
//! open-source Pi coding agent's quota extension calls:
//! `GET https://api.z.ai/api/monitor/usage/quota/limit` with a bearer API key.
//!
//! This is an observed, provider-internal endpoint — not documented public
//! API — so its URL and response-shape assumptions live only here and the
//! parsing is defensive: malformed or changed responses fail as
//! `FetchError::Parse` rather than presenting guessed usage. The endpoint is
//! a fixed `https://` constant; there is no override setting, so the pasted
//! key can never be sent anywhere else.
//!
//! The response is `{"data": {"level": …, "limits": […]}}` where each limit
//! entry is one of:
//!
//! - `TOKENS_LIMIT` — plan token utilisation as a bare `percentage` (0–100)
//!   with **no** absolute used/grant counts, an epoch-millisecond
//!   `nextResetTime`, and the window length as `unit` + `number`.
//!   Observed: unit 3 = hour (the rolling 5-hour window), unit 6 = week
//!   (the rolling 7-day weekly window). These are percentage-only usage
//!   windows: inventing a token total would misstate what the provider
//!   actually reports.
//! - `CREDIT_LIMIT` — observed live (issue #183 production report) carrying
//!   exactly the same shape: unit 3/number 5 and unit 6/number 1, a bare
//!   `percentage`, no absolute counts. It is the same plan-window
//!   utilisation under a different type tag, so it is parsed with the
//!   identical percentage-only 5-hour/weekly semantics — still no invented
//!   total, despite the name.
//! - `TIME_LIMIT` — a monthly count window (`usage` = entitlement,
//!   `currentValue` = consumed), observed as web-search/tool usage. It is an
//!   **informational** window: exhausting it does not block model calls, so
//!   it must never colour the card or the tray.
//!
//! The account is the Z.ai Coding Plan subscription, not an individual GLM
//! model; the endpoint reports plan windows with no per-model split, so no
//! GLM-5.3 vs GLM-5.3-Flash accounting is attempted here.

use super::{as_f64, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{Allowance, FetchError, UsageSnapshot, UsageWindow};
use chrono::Duration;
use serde_json::Value;

pub struct Zai {
    pub key: String,
    pub label: Option<String>,
}
impl Zai {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
}

/// The fixed trusted endpoint (see the module comment). Not a setting.
const QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

#[async_trait::async_trait]
impl Provider for Zai {
    fn kind(&self) -> &'static str {
        "zai"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Z.ai")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| FetchError::NotConfigured("paste a Z.ai API key in Settings".into()))?;
        let resp = ctx
            .http
            .get(QUOTA_URL)
            .bearer_auth(key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;
        // The whole post-socket path goes through `fetch_parse` so tests can
        // exercise the real status mapping, parsing and snapshot semantics
        // without the network.
        self.fetch_parse(
            resp.status().as_u16(),
            resp.json().await.map_err(network_err)?,
        )
    }
}

impl Zai {
    /// Status handling + response parsing: everything after the HTTP exchange.
    /// Split from `fetch` as the deterministic test seam (see `refresh.rs`:
    /// adapters are tested without mocking HTTP). Error strings name the
    /// endpoint, never the request, so the bearer key cannot reach a
    /// diagnostic.
    fn fetch_parse(&self, status: u16, body: Value) -> Result<UsageSnapshot, FetchError> {
        match status {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "API key rejected — re-check it in Settings".into(),
                ))
            }
            s => {
                return Err(FetchError::Network(format!(
                    "HTTP {s} from usage quota endpoint"
                )))
            }
        }
        let windows = parse_limits(&body);
        if windows.is_empty() {
            return Err(FetchError::Parse(
                "usage response had no usable limit entries".into(),
            ));
        }
        Ok(UsageSnapshot::ok(self.id(), self.name(), windows, None))
    }
}

/// One limit entry: the fields this adapter reads, anything else tolerated
/// and ignored. Parsed defensively throughout — the endpoint is observed, not
/// documented, and a changed shape must fail clearly rather than fabricate
/// usage.
fn parse_limits(body: &Value) -> Vec<UsageWindow> {
    // The observed shape wraps the collection in `data`; the bare form is
    // accepted too so a future envelope change stays a parse-level concern.
    let limits = body
        .get("data")
        .and_then(|d| d.get("limits"))
        .or_else(|| body.get("limits"))
        .and_then(Value::as_array);
    let Some(limits) = limits else {
        return vec![];
    };
    let mut windows = Vec::new();
    for entry in limits {
        // `TOKENS_LIMIT` and `CREDIT_LIMIT` carry the identical
        // percentage-only window shape (CREDIT_LIMIT observed live, issue
        // #183), so both parse through the same path.
        match entry.get("type").and_then(Value::as_str) {
            Some("TOKENS_LIMIT" | "CREDIT_LIMIT") => {
                if let Some(w) = parse_tokens_limit(entry) {
                    windows.push(w);
                }
            }
            Some("TIME_LIMIT") => {
                if let Some(w) = parse_time_limit(entry) {
                    windows.push(w);
                }
            }
            // An unknown entry type must not hide the known windows, but is
            // also not silently reinterpreted as one of them.
            _ => {}
        }
    }
    // Shortest window first (5h → week → month), matching the other
    // providers' order. Unknown-unit token windows carry no length and sort
    // last, after every window whose length is known.
    windows.sort_by_key(sort_rank);
    windows
}

/// A token-utilisation window: percentage-only, reset at epoch millis, length
/// given as `unit` + `number`. Returns `None` for entries with no usable
/// percentage — a window with nothing to show is skipped rather than invented.
///
/// Metric ids mirror Claude's (`five_hour`, `weekly`) so headline selection
/// and ordering read the same way across providers; other lengths get their
/// own ids rather than overloading those names.
fn parse_tokens_limit(entry: &Value) -> Option<UsageWindow> {
    let pct = entry.get("percentage").and_then(as_f64)?;
    // Provider-controlled count. `f64 as i64` saturates (never wraps), so a
    // hostile or broken huge value lands on `i64::MAX` rather than a wrapped
    // negative — and the checked Duration constructors below reject that
    // instead of panicking. (NaN is already excluded by the positivity
    // filter; JSON cannot carry infinities, only their float parses.)
    let number = entry
        .get("number")
        .and_then(as_f64)
        .filter(|n| *n > 0.0)
        .map(|n| n as i64);
    let resets_at = entry.get("nextResetTime").and_then(parse_timestamp);
    let unit = entry.get("unit").and_then(Value::as_i64);

    // Window length from the observed (unit, number) pair. chrono's
    // `Duration::hours`/`weeks` *panic* on an out-of-range count (they are
    // `expect` over the checked variants), so the checked constructors
    // decide: a count the length math cannot survive degrades to the same
    // generic window an unknown unit gets — percentage and reset still
    // surface, nothing is invented. The refresh loop must be able to
    // survive any provider response.
    let (metric_id, label, period_len) = match (unit, number) {
        (Some(3), Some(5)) => ("five_hour".into(), "5-hour".into(), Duration::try_hours(5)),
        (Some(3), Some(n)) => Duration::try_hours(n)
            .map(|len| ("hours".into(), format!("{n}-hour"), Some(len)))
            .unwrap_or_else(generic_token_window),
        (Some(6), Some(1)) => ("weekly".into(), "Weekly".into(), Duration::try_weeks(1)),
        (Some(6), Some(n)) => Duration::try_weeks(n)
            .map(|len| ("weeks".into(), format!("{n}-week"), Some(len)))
            .unwrap_or_else(generic_token_window),
        _ => generic_token_window(),
    };
    Some(UsageWindow {
        metric_id,
        label,
        used_pct: pct,
        resets_at,
        period_start: resets_at.and_then(|r| period_len.map(|len| r - len)),
        ..Default::default()
    })
}

/// The safe fallback for a token window whose length cannot be trusted:
/// the percentage still surfaces, with no invented length or marker.
fn generic_token_window() -> (String, String, Option<Duration>) {
    ("token_usage".into(), "Token usage".into(), None)
}

/// A monthly count window (observed: web searches / tool calls). Only a
/// positive entitlement is shown, and it is informational: exhausting the
/// tool allowance does not block model calls, so it never drives status,
/// alerts or the tray.
fn parse_time_limit(entry: &Value) -> Option<UsageWindow> {
    let granted = entry.get("usage").and_then(as_f64).filter(|g| *g > 0.0)?;
    let used = entry.get("currentValue").and_then(as_f64).unwrap_or(0.0);
    let resets_at = entry.get("nextResetTime").and_then(parse_timestamp);
    Some(UsageWindow {
        metric_id: "web_tool_month".into(),
        label: "Web/tool this month".into(),
        used_pct: used / granted * 100.0,
        resets_at,
        period_start: resets_at.and_then(super::calendar_month_start),
        allowance: Some(Allowance {
            remaining: granted - used,
            total: granted,
            unit: "calls".into(),
        }),
        informational: true,
    })
}

/// Sort order over the produced windows. The rank is fixed per window
/// identity — 5-hour, then weekly, then everything else (monthly counts,
/// unknown-unit and unusable-length windows) in parse order — not by measured
/// length, so a shorter-than-5h window from a future unit still cannot
/// displace the conventional order other providers display.
fn sort_rank(w: &UsageWindow) -> (u8, u8) {
    match w.metric_id.as_str() {
        "five_hour" => (0, 0),
        "weekly" => (0, 1),
        _ => (1, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::Status;
    use crate::providers::calendar_month_start;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;

    const KEY: &str = "zai-test-key-that-must-never-appear-in-errors";

    /// The observed response shape (pi-quotas fixtures): a 5-hour token
    /// limit, a weekly token limit, and a monthly web/tool time limit.
    fn observed_body() -> Value {
        serde_json::json!({
            "data": {
                "level": "lite",
                "limits": [
                    {
                        "type": "TIME_LIMIT",
                        "unit": 5,
                        "number": 1,
                        "usage": 100,
                        "currentValue": 25,
                        "remaining": 75,
                        "percentage": 25,
                        "nextResetTime": 1785048370995i64,
                        "usageDetails": [
                            {"modelCode": "search-prime", "usage": 20},
                            {"modelCode": "web-reader", "usage": 5}
                        ]
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 3,
                        "number": 5,
                        "percentage": 8,
                        "nextResetTime": 1782932874304i64
                    },
                    {
                        "type": "TOKENS_LIMIT",
                        "unit": 6,
                        "number": 1,
                        "percentage": 52,
                        "nextResetTime": 1783061170994i64
                    }
                ]
            }
        })
    }

    fn adapter() -> Zai {
        Zai::new("zai".into(), None)
    }

    fn ok_snapshot(body: Value) -> UsageSnapshot {
        adapter().fetch_parse(200, body).unwrap()
    }

    /// Story 8–12 through the real adapter path: the 5-hour, weekly and
    /// monthly windows come out in shortest-first order, percentages are
    /// preserved as percentages (never turned into invented token totals),
    /// and epoch-millisecond resets are converted correctly.
    #[test]
    fn normal_response_orders_windows_and_preserves_provider_figures() {
        let snap = ok_snapshot(observed_body());
        assert_eq!(snap.provider_id, "zai");
        assert_eq!(snap.provider_name, "Z.ai");
        assert_eq!(snap.error, None);
        assert_eq!(snap.windows.len(), 3);

        let five = &snap.windows[0];
        assert_eq!(five.metric_id, "five_hour");
        assert_eq!(five.label, "5-hour");
        assert_eq!(five.used_pct, 8.0);
        assert_eq!(
            five.resets_at,
            Some("2026-07-01T19:07:54.304Z".parse::<DateTime<Utc>>().unwrap())
        );
        assert_eq!(
            five.period_start,
            Some("2026-07-01T14:07:54.304Z".parse::<DateTime<Utc>>().unwrap())
        );
        assert!(
            five.allowance.is_none(),
            "percentage-only: no invented totals"
        );
        assert!(!five.informational, "token windows gate model calls");

        let weekly = &snap.windows[1];
        assert_eq!(weekly.metric_id, "weekly");
        assert_eq!(weekly.label, "Weekly");
        assert_eq!(weekly.used_pct, 52.0);
        assert!(weekly.resets_at.is_some());

        let monthly = &snap.windows[2];
        assert_eq!(monthly.metric_id, "web_tool_month");
        assert_eq!(monthly.used_pct, 25.0);
        let a = monthly.allowance.as_ref().unwrap();
        assert!((a.remaining - 75.0).abs() < 1e-9);
        assert!((a.total - 100.0).abs() < 1e-9);
        assert!(monthly.informational, "web/tool usage does not gate calls");
        assert!(monthly.resets_at.is_some());
        assert_eq!(
            monthly.period_start,
            monthly.resets_at.and_then(calendar_month_start),
            "calendar month, not a fixed 30 days"
        );
    }

    /// Story 14 + 20: the informational monthly window never colours status,
    /// even at 100%, while the token windows do.
    #[test]
    fn informational_monthly_never_drives_status_token_windows_do() {
        let mut body = observed_body();
        body["data"]["limits"][0]["currentValue"] = serde_json::json!(100);
        let snap = ok_snapshot(body);
        assert_eq!(
            snap.status(80.0, 95.0, None),
            Status::Ok,
            "exhausted web/tool usage is not a blocked quota"
        );

        let mut body = observed_body();
        body["data"]["limits"][0]["currentValue"] = serde_json::json!(0);
        body["data"]["limits"][1]["percentage"] = serde_json::json!(96);
        let snap = ok_snapshot(body);
        assert_eq!(snap.status(80.0, 95.0, None), Status::Critical);
    }

    /// Story 16 through the same status mapping the live fetch uses.
    #[test]
    fn http_status_maps_to_the_shared_error_taxonomy() {
        let auth = adapter().fetch_parse(401, observed_body()).unwrap_err();
        assert!(matches!(auth, FetchError::AuthExpired(_)), "{auth:?}");
        let auth = adapter().fetch_parse(403, observed_body()).unwrap_err();
        assert!(matches!(auth, FetchError::AuthExpired(_)), "{auth:?}");
        let other = adapter().fetch_parse(500, observed_body()).unwrap_err();
        assert!(
            matches!(&other, FetchError::Network(m) if m.contains("500")),
            "{other:?}"
        );
    }

    /// Story 27: the API key is sent to the endpoint but never appears in a
    /// diagnostic. The fetch-side guarantee is structural — the URL is a
    /// fixed constant and error strings are built from the status alone, so
    /// no error path can echo the credential. Asserted here over every
    /// failure this adapter can produce.
    #[test]
    fn no_error_diagnostic_ever_carries_the_api_key() {
        let bodies = [
            observed_body(),
            serde_json::json!({}),
            Value::String("not an object".into()),
        ];
        for status in [400, 401, 403, 429, 500, 503] {
            for body in &bodies {
                if let Err(e) = adapter().fetch_parse(status, body.clone()) {
                    assert!(!e.to_string().contains(KEY), "{e} leaked the API key");
                }
            }
        }
        let parse_err = adapter()
            .fetch_parse(200, serde_json::json!({}))
            .unwrap_err();
        assert!(!parse_err.to_string().contains(KEY));
    }

    /// Story 7 through the live fetch path: no stored secret is
    /// NotConfigured, before any request is attempted.
    #[tokio::test]
    async fn missing_key_is_not_configured_without_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ProviderCtx::new(
            dir.path().into(),
            dir.path().into(),
            HashMap::new(),
            Config::default(),
        );
        let err = adapter().fetch(&ctx).await.unwrap_err();
        assert!(matches!(err, FetchError::NotConfigured(_)), "{err:?}");
    }

    // ---- parser cases (stories 17, 28, 29) ----------------------------------

    #[test]
    fn missing_empty_and_non_object_limits_are_not_usable() {
        // Nothing at all / no collection / not an array.
        assert!(parse_limits(&serde_json::json!({})).is_empty());
        assert!(parse_limits(&serde_json::json!({"data": {}})).is_empty());
        assert!(parse_limits(&serde_json::json!({"data": {"limits": {}}})).is_empty());
        // An empty collection parses to no windows, which fetch_parse turns
        // into a clear Parse error rather than a healthy empty snapshot.
        assert!(parse_limits(&serde_json::json!({"data": {"limits": []}})).is_empty());
        // Non-object entries are skipped, known siblings still parsed.
        let w = parse_limits(&serde_json::json!({
            "data": {"limits": ["nope", 42, null,
                {"type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 9}]}
        }));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].metric_id, "five_hour");
    }

    #[test]
    fn unknown_limit_types_are_skipped_without_hiding_known_windows() {
        let w = parse_limits(&serde_json::json!({
            "data": {"limits": [
                {"type": "SOMETHING_NEW", "percentage": 70},
                {"type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 30}
            ]}
        }));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].metric_id, "weekly");
    }

    /// Story 28: tolerate additional or reordered entries — the extra entry
    /// must not shift the known windows' identity or order.
    #[test]
    fn additional_and_reordered_entries_keep_windows_stable() {
        let mut body = observed_body();
        body["data"]["limits"]
            .as_array_mut()
            .unwrap()
            .rotate_left(1);
        body["data"]["limits"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"type": "TOKENS_LIMIT", "unit": 9, "percentage": 40}));
        let w = parse_limits(&body);
        assert_eq!(w.len(), 4);
        assert_eq!(w[0].metric_id, "five_hour");
        assert_eq!(w[1].metric_id, "weekly");
        assert_eq!(w[2].metric_id, "web_tool_month");
        assert_eq!(w[3].metric_id, "token_usage", "unknown unit sorts last");
    }

    /// Story 29: unknown units are handled explicitly — surfaced as a safe
    /// generic window when reset information is usable, never dropped.
    #[test]
    fn unknown_unit_keeps_a_generic_window_when_reset_is_usable() {
        let w = parse_limits(&serde_json::json!({
            "data": {"limits": [
                {"type": "TOKENS_LIMIT", "unit": 99, "number": 3,
                 "percentage": 40, "nextResetTime": 1783061170994i64}
            ]}
        }));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].metric_id, "token_usage");
        assert_eq!(w[0].label, "Token usage");
        assert_eq!(w[0].used_pct, 40.0);
        assert!(w[0].resets_at.is_some(), "reset carried, not dropped");
        assert!(w[0].period_start.is_none(), "no length: no guessed marker");
    }

    #[test]
    fn token_entries_without_usable_percentages_are_skipped() {
        for missing in [
            serde_json::json!({"type": "TOKENS_LIMIT", "unit": 3, "number": 5}),
            serde_json::json!({"type": "TOKENS_LIMIT", "unit": 3, "number": 5,
                               "percentage": "lots"}),
            serde_json::json!({"type": "TOKENS_LIMIT", "unit": 3, "number": 5,
                               "percentage": null}),
        ] {
            assert!(parse_limits(&serde_json::json!({"data": {"limits": [missing]}})).is_empty());
        }
    }

    /// Stories 12, 17: reset stamps that no plausible clock reading could
    /// produce (out of both the seconds and milliseconds range) are not
    /// guessed into a reset time; the percentage survives, the reset is
    /// simply unknown.
    #[test]
    fn malformed_reset_timestamps_are_not_fatal_and_not_guessed() {
        for bad in [
            Value::from(""),
            Value::from("not-a-time"),
            // Negative stamps parse as instants before the epoch (chrono
            // accepts them), so they are not "malformed" at the shared
            // helper's level. These produce no readable instant at all:
            Value::from(true),
            Value::Object(Default::default()),
            Value::Array(vec![]),
        ] {
            let w = parse_limits(&serde_json::json!({
                "data": {"limits": [
                    {"type": "TOKENS_LIMIT", "unit": 3, "number": 5,
                     "percentage": 10, "nextResetTime": bad}
                ]}
            }));
            assert_eq!(w.len(), 1, "{bad:?}");
            assert_eq!(w[0].used_pct, 10.0);
            assert_eq!(w[0].resets_at, None);
        }
    }

    #[test]
    fn zero_and_negative_time_limit_grants_are_not_windows() {
        for grant in [0, -5] {
            let w = parse_limits(&serde_json::json!({
                "data": {"limits": [
                    {"type": "TIME_LIMIT", "unit": 5, "number": 1,
                     "usage": grant, "currentValue": 1}
                ]}
            }));
            assert!(w.is_empty(), "{grant:?}");
        }
    }

    /// A TIME_LIMIT with a positive grant but no readable count shows a 0%
    /// window rather than being dropped — the entitlement exists.
    #[test]
    fn time_limit_without_current_value_reads_as_unused() {
        let w = parse_limits(&serde_json::json!({
            "data": {"limits": [
                {"type": "TIME_LIMIT", "unit": 5, "number": 1,
                 "usage": 100, "nextResetTime": 1785048370995i64}
            ]}
        }));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].used_pct, 0.0);
    }

    /// Malformed body on a 200: clear Parse error, never a healthy or
    /// exhausted snapshot fabricated from garbage (story 17).
    #[test]
    fn malformed_success_bodies_are_parse_errors_not_snapshots() {
        for body in [
            serde_json::json!({}),
            serde_json::json!({"data": {"limits": []}}),
            serde_json::json!({"data": {"limits": [
                {"type": "TIME_LIMIT", "unit": 5, "usage": 0}
            ]}}),
            Value::Array(vec![]),
        ] {
            let err = adapter().fetch_parse(200, body).unwrap_err();
            assert!(matches!(err, FetchError::Parse(_)), "{err:?}");
        }
    }

    /// Non-5h/1w lengths get their own windows and ids rather than
    /// overloading Claude's metric names.
    #[test]
    fn other_observed_lengths_get_their_own_labels_and_ids() {
        let w = parse_limits(&serde_json::json!({
            "data": {"limits": [
                {"type": "TOKENS_LIMIT", "unit": 3, "number": 2, "percentage": 30},
                {"type": "TOKENS_LIMIT", "unit": 6, "number": 2, "percentage": 60}
            ]}
        }));
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].metric_id, "hours");
        assert_eq!(w[0].label, "2-hour");
        assert_eq!(w[1].metric_id, "weeks");
        assert_eq!(w[1].label, "2-week");
    }

    /// Regression (review rework): the provider-controlled `number` feeds the
    /// window-length math, and chrono's plain `Duration::hours`/`weeks`
    /// **panic** on out-of-range counts — which would take the whole refresh
    /// loop down. Extreme values must degrade to the generic window instead:
    /// the percentage still surfaces (nothing healthy or exhausted is
    /// fabricated), no length or period marker is invented, and the entry
    /// cannot displace the known windows' order.
    #[test]
    fn extreme_number_values_cannot_panic_and_do_not_fabricate_state() {
        // Saturation points and beyond: `f64 as i64` clamps these to
        // i64::MAX, which the checked Duration constructors must reject.
        let extreme_numbers = [
            9e15,                        // far past any real window
            1.8e19,                      // ≈ i64::MAX after saturation
            f64::MAX,                    // maximal f64 → i64::MAX
            f64::INFINITY,               // not JSON, but defensive
            9_223_372_036_854_775_807.0, // exact i64::MAX
        ];
        for n in extreme_numbers {
            let w = parse_limits(&serde_json::json!({
                "data": {"limits": [
                    {"type": "TOKENS_LIMIT", "unit": 3, "number": n,
                     "percentage": 40, "nextResetTime": 1783061170994i64}
                ]}
            }));
            assert_eq!(w.len(), 1, "{n}");
            assert_eq!(w[0].metric_id, "token_usage", "{n}");
            assert_eq!(w[0].label, "Token usage", "{n}");
            assert_eq!(w[0].used_pct, 40.0, "{n}");
            assert!(w[0].period_start.is_none(), "{n}");
            assert!(w[0].resets_at.is_some(), "{n}");
            assert!(!w[0].informational, "{n}");

            // Same for the weekly unit: no panic, generic degradation.
            let w = parse_limits(&serde_json::json!({
                "data": {"limits": [
                    {"type": "TOKENS_LIMIT", "unit": 6, "number": n, "percentage": 55}
                ]}
            }));
            assert_eq!(w.len(), 1, "{n}");
            assert_eq!(w[0].metric_id, "token_usage", "{n}");
            assert_eq!(w[0].used_pct, 55.0, "{n}");
        }

        // The whole entry through fetch_parse: the snapshot stays usable and
        // the extreme window reads as a plain 40% — not as healthy 0% or a
        // fabricated 100% exhaustion.
        let body = serde_json::json!({
            "data": {"limits": [
                {"type": "TOKENS_LIMIT", "unit": 3, "number": f64::MAX,
                 "percentage": 40, "nextResetTime": 1783061170994i64}
            ]}
        });
        let snap = adapter().fetch_parse(200, body).unwrap();
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].used_pct, 40.0);
        assert_eq!(snap.error, None);
        assert_eq!(snap.status(80.0, 95.0, None), Status::Ok);

        // And the normal windows are untouched by the fix.
        let snap = ok_snapshot(observed_body());
        assert_eq!(snap.windows[0].metric_id, "five_hour");
        assert_eq!(snap.windows[1].metric_id, "weekly");
    }

    /// A missing, zero or negative `number` carries no trustworthy length, so
    /// the window degrades to the generic form rather than pretending a
    /// default of one full week/hour.
    #[test]
    fn unusable_number_degrades_to_the_generic_window() {
        for n in [
            Value::Null,
            Value::from(0),
            Value::from(-3),
            Value::from("week"),
        ] {
            let w = parse_limits(&serde_json::json!({
                "data": {"limits": [
                    {"type": "TOKENS_LIMIT", "unit": 6, "number": n, "percentage": 20}
                ]}
            }));
            assert_eq!(w.len(), 1, "{n:?}");
            assert_eq!(w[0].metric_id, "token_usage", "{n:?}");
            assert_eq!(w[0].used_pct, 20.0, "{n:?}");
            assert!(w[0].period_start.is_none(), "{n:?}");
        }
    }

    /// Regression (issue #183 production follow-up): a live 200 response
    /// whose entries carry `type: "CREDIT_LIMIT"` was reported verbatim by a
    /// user — unit 3/number 5 and unit 6/number 1, percentage-only, no
    /// absolute counts. The parser must treat it exactly like `TOKENS_LIMIT`
    /// (same percentage-only 5-hour/weekly semantics), or the snapshot dies
    /// with "no usable limit entries".
    #[test]
    fn credit_limit_entries_use_the_token_window_semantics() {
        let w = parse_limits(&credit_limit_body());
        assert_eq!(w.len(), 2, "both CREDIT_LIMIT entries parse");

        let five = &w[0];
        assert_eq!(five.metric_id, "five_hour");
        assert_eq!(five.label, "5-hour");
        assert_eq!(five.used_pct, 10.0);
        assert_eq!(
            five.period_start,
            five.resets_at.map(|r| r - Duration::hours(5)),
            "5-hour length derived like TOKENS_LIMIT"
        );

        let weekly = &w[1];
        assert_eq!(weekly.metric_id, "weekly");
        assert_eq!(weekly.label, "Weekly");
        assert_eq!(weekly.used_pct, 12.0);

        // Through the real fetch_parse path: previously a Parse error, now a
        // usable snapshot. No invented totals, no informational marking.
        let snap = adapter().fetch_parse(200, credit_limit_body()).unwrap();
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.error, None);
        assert_eq!(snap.status(80.0, 95.0, None), Status::Ok);
        for window in &snap.windows {
            assert!(window.allowance.is_none(), "percentage-only");
            assert!(!window.informational, "credit windows gate model calls");
        }
    }

    /// The user-captured 200 body verbatim: `data.limits` holds two
    /// `CREDIT_LIMIT` entries — the 5-hour window (unit 3, number 5, 10%)
    /// and the weekly window (unit 6, number 1, 12%). Reset stamps follow the
    /// observed epoch-millisecond shape; the ticket's capture did not include
    /// them.
    fn credit_limit_body() -> Value {
        serde_json::json!({
            "data": {
                "limits": [
                    {"type": "CREDIT_LIMIT", "unit": 3, "number": 5,
                     "percentage": 10, "nextResetTime": 1782932874304i64},
                    {"type": "CREDIT_LIMIT", "unit": 6, "number": 1,
                     "percentage": 12, "nextResetTime": 1783061170994i64}
                ]
            }
        })
    }

    /// Outcome parity (stories 18–21): one Z.ai snapshot, as the adapter
    /// produces it, must be consumed with the same meaning by every shared
    /// surface — threshold colouring, the compact tray/aggregate fold,
    /// explicit headline selection, sorting, and the Android home-screen
    /// widget's per-row projection. The informational web/tool window
    /// participates in display and carries its counts, but never in status,
    /// colour, or alert thresholds.
    #[test]
    fn one_snapshot_drives_tray_headlines_sorting_and_widget_folds_identically() {
        use crate::model::Status;
        use crate::snapshots::SnapshotStore;
        use crate::widget::{project, WidgetAccountSelection, WidgetInstanceConfig, WidgetSize};

        let mut body = observed_body();
        // Web/tool exhausted, 5-hour approaching (88%), weekly comfortable:
        // if the informational window leaked into any fold, status/colour
        // would read Critical and the widget would headline the wrong
        // quantity.
        body["data"]["limits"][0]["currentValue"] = serde_json::json!(100);
        body["data"]["limits"][1]["percentage"] = serde_json::json!(88);
        let snap = ok_snapshot(body);

        // Threshold colouring (the card's bars): Warn from the 5-hour window
        // alone, never Critical from the exhausted informational row.
        assert_eq!(snap.status(80.0, 95.0, None), Status::Warn);

        // Compact tray/aggregate fold.
        let mut cfg = Config::default();
        cfg.providers.get_mut("zai").unwrap().enabled = true;
        let aggregate = crate::refresh::aggregate_status(std::slice::from_ref(&snap), &cfg);
        assert_eq!(aggregate.status, Status::Warn);
        assert!(
            (aggregate.pct - 88.0).abs() < 1e-9,
            "aggregate took {}",
            aggregate.pct
        );

        // Explicit weekly headline selection resolves by its metric id.
        cfg.providers.get_mut("zai").unwrap().mini_summary_metrics =
            Some(vec!["window:weekly".into()]);
        let (status, pct) = cfg.mini_tray_status(&snap, None).unwrap();
        assert_eq!(status, Status::Ok);
        assert!((pct.unwrap() - 52.0).abs() < 1e-9);

        // The Android home-screen widget folds the same snapshot: the row is
        // present, status Warn, and its automatic headline — the worst
        // gating window by percentage — is the Weekly window here, not the
        // exhausted informational web/tool row.
        let store = SnapshotStore::from_snapshots(vec![snap.clone()], aggregate);
        let instance = WidgetInstanceConfig {
            accounts: vec![WidgetAccountSelection {
                provider_id: "zai".into(),
                headlines: None,
            }],
            privacy: false,
        };
        let projection = project(
            Some(&instance),
            &store,
            &cfg,
            WidgetSize::from_dimensions(220.0, 140.0),
            Utc::now(),
        );
        let content = match projection.state {
            crate::widget::WidgetState::Content(c) => c,
            other => panic!("expected widget content, got {other:?}"),
        };
        assert_eq!(content.aggregate.status, Status::Warn);
        let row = &content.rows[0];
        let crate::widget::RowState::Present { status, cells } = &row.state else {
            panic!("expected a present row, got {:?}", row.state);
        };
        assert_eq!(*status, Status::Warn);
        assert_eq!(cells.len(), 1, "automatic pick is one headline");
        assert_eq!(cells[0].label, "Weekly");
        let crate::widget::HeadlineValue::Usage { used_pct, .. } = cells[0].value.clone().unwrap()
        else {
            panic!("expected a usage headline, got {:?}", cells[0].value);
        };
        assert!((used_pct - 52.0).abs() < 1e-9);
    }
}
