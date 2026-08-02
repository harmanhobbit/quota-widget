//! Background poll loop: fetch enabled providers, update shared state, drive
//! the tray icon, dispatch alerts, and push snapshots to the webview.

use crate::{tray, AppState};
use futures_util::future::join_all;
use quota_core::alerts::AlertLevel;
use quota_core::model::{Status, UsageSnapshot};
use quota_core::providers::providers_for;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            poll_once(&app, &state).await;
            let interval = state.config.read().await.poll_interval_secs.max(15);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
                _ = state.refresh.notified() => {}
            }
        }
    });
}

async fn poll_once(app: &AppHandle, state: &Arc<AppState>) {
    let cfg = state.config.read().await.clone();
    let ctx = state.provider_ctx(cfg.clone());
    let ctx = &ctx;

    let mut fresh = join_all(
        providers_for(&cfg)
            .into_iter()
            .filter(|provider| {
                let enabled = cfg
                    .providers
                    .get(provider.id())
                    .map(|p| p.enabled)
                    .unwrap_or(false);
                enabled
            })
            .map(|provider| async move {
                match provider.fetch(&ctx).await {
                    Ok(s) => s,
                    Err(e) => UsageSnapshot::failed(provider.id(), provider.name(), e),
                }
            }),
    )
    .await;

    // Update shared state. When a fetch fails but we have older data, keep the
    // stale numbers and attach the new error, so the UI shows greyed-out values
    // with a reason instead of a blank card.
    {
        let mut map = state.snapshots.write().await;
        for s in &mut fresh {
            if let (Some(err), Some(prev)) = (s.error.clone(), map.get(&s.provider_id)) {
                if prev.error.is_none() && !(prev.windows.is_empty() && prev.credits.is_none()) {
                    let mut merged = prev.clone();
                    merged.error = Some(err);
                    *s = merged;
                }
            }
            map.insert(s.provider_id.clone(), s.clone());
        }
    }

    // Tray icon: each account contributes the value selected for its compact
    // mini summary. Automatic retains the old "worst real quota" behaviour;
    // an explicit headline makes that one value the account's tray signal.
    // `in_tray` opts that signal out, while `tray_color` is the global-style
    // alert opt-out that keeps the account visible but neutral in the icon.
    let mut worst = Status::Ok;
    let mut worst_pct: f64 = 0.0;
    let mut tip_lines = Vec::new();
    for s in &fresh {
        let thr = cfg.effective_thresholds(&s.provider_id);
        let low = cfg
            .providers
            .get(&s.provider_id)
            .and_then(|p| p.low_balance_warn);
        if cfg.counts_in_tray(&s.provider_id) && cfg.effective_alerts(&s.provider_id).tray_color {
            if let Some((st, pct)) = mini_tray_status(s, &cfg, thr.warn_pct, thr.critical_pct, low)
            {
                worst = worst.max(st);
                if let Some(pct) = pct {
                    worst_pct = worst_pct.max(pct);
                }
            }
        }
        tip_lines.push(tooltip_line(s));
    }
    let tooltip = if tip_lines.is_empty() {
        "Quota Widget — no providers enabled".into()
    } else {
        tip_lines.join("\n")
    };
    #[cfg(target_os = "linux")]
    crate::tray_linux::set_status(worst, worst_pct / 100.0, tooltip);
    #[cfg(not(target_os = "linux"))]
    tray::set_status(app, worst, worst_pct / 100.0, &tooltip);

    // Alerts (edge-triggered in the engine; dispatch per toggles).
    let mut engine = state.alert_engine.lock().await;
    for s in &fresh {
        let toggles = cfg.effective_alerts(&s.provider_id);
        for event in engine.evaluate(s, &cfg) {
            let title = match event.level {
                AlertLevel::Critical => format!("{} — critical", event.provider_name),
                _ => format!("{} — warning", event.provider_name),
            };
            if toggles.toast {
                let _ = app
                    .notification()
                    .builder()
                    .title(&title)
                    .body(&event.message)
                    .show();
            }
            if toggles.auto_popup {
                tray::show_popup(app, None);
            }
        }
    }
    drop(engine);

    crate::emit_snapshots(app, &fresh);
}

/// Return the status and optional gauge percentage for the configured
/// mini-summary headline. `none` deliberately contributes nothing; this lets
/// an account remain enabled in the main popup without appearing in either
/// compact surface.
fn mini_tray_status(
    snapshot: &UsageSnapshot,
    config: &quota_core::config::Config,
    warn_pct: f64,
    critical_pct: f64,
    low_balance_warn: Option<f64>,
) -> Option<(Status, Option<f64>)> {
    let selected = config
        .providers
        .get(&snapshot.provider_id)
        .and_then(|provider| provider.mini_summary_metric.as_deref());
    if selected == Some("none") {
        return None;
    }
    if snapshot.error.is_some() {
        return Some((Status::Stale, None));
    }
    if let Some(metric_id) = selected.and_then(|value| value.strip_prefix("window:")) {
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.metric_id == metric_id)?;
        let status = if window.used_pct >= critical_pct {
            Status::Critical
        } else if window.used_pct >= warn_pct {
            Status::Warn
        } else {
            Status::Ok
        };
        return Some((status, Some(window.used_pct)));
    }
    if selected == Some("credits") {
        return Some((
            snapshot.status(warn_pct, critical_pct, low_balance_warn),
            None,
        ));
    }

    let pct = snapshot
        .windows
        .iter()
        .filter(|window| !window.informational)
        .map(|window| window.used_pct)
        .max_by(f64::total_cmp);
    Some((
        snapshot.status(warn_pct, critical_pct, low_balance_warn),
        pct,
    ))
}

fn tooltip_line(s: &UsageSnapshot) -> String {
    if s.error.is_some() {
        return format!("{}: unavailable", s.provider_name);
    }
    let mut parts = Vec::new();
    for w in &s.windows {
        parts.push(format!("{} {:.0}%", w.label, w.used_pct));
    }
    if let Some(c) = &s.credits {
        parts.push(format!("{:.2} {}", c.balance, c.unit));
    }
    format!("{}: {}", s.provider_name, parts.join(" · "))
}
