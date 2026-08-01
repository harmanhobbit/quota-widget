//! Background poll loop: fetch enabled providers, update shared state, drive
//! the tray icon, dispatch alerts, and push snapshots to the webview.

use crate::{tray, AppState};
use quota_core::alerts::AlertLevel;
use quota_core::model::{Status, UsageSnapshot};
use quota_core::providers::all_providers;
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

    let mut fresh: Vec<UsageSnapshot> = Vec::new();
    for provider in all_providers() {
        let enabled = cfg.providers.get(provider.id()).map(|p| p.enabled).unwrap_or(false);
        if !enabled {
            continue;
        }
        let snap = match provider.fetch(&ctx).await {
            Ok(s) => s,
            Err(e) => UsageSnapshot::failed(provider.id(), provider.name(), e),
        };
        fresh.push(snap);
    }

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

    // Tray icon: worst status across providers included in the tray. Two
    // independent opt-outs — `in_tray` excludes the provider from the icon
    // entirely, and `tray_color` keeps it counted but stops it colouring.
    let mut worst = Status::Ok;
    let mut worst_pct: f64 = 0.0;
    let mut tip_lines = Vec::new();
    for s in &fresh {
        let thr = cfg.effective_thresholds(&s.provider_id);
        let low = cfg.providers.get(&s.provider_id).and_then(|p| p.low_balance_warn);
        let st = s.status(thr.warn_pct, thr.critical_pct, low);
        if cfg.counts_in_tray(&s.provider_id) && cfg.effective_alerts(&s.provider_id).tray_color {
            worst = worst.max(st);
            if s.error.is_none() {
                // Informational windows are shown but never drive the gauge.
                for w in s.windows.iter().filter(|w| !w.informational) {
                    worst_pct = worst_pct.max(w.used_pct);
                }
            }
        }
        tip_lines.push(tooltip_line(s));
    }
    let tooltip = if tip_lines.is_empty() { "Quota Widget — no providers enabled".into() } else { tip_lines.join("\n") };
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
                let _ = app.notification().builder().title(&title).body(&event.message).show();
            }
            if toggles.auto_popup {
                tray::show_popup(app, None);
            }
        }
    }
    drop(engine);

    crate::emit_snapshots(app, &fresh);
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
