//! Background poll loop: schedule quota-core's shared refresh operation, then
//! present its outcome — update shared state, drive the tray icon, dispatch
//! alerts, and push snapshots to the webview. Scheduling and presentation are
//! the desktop host's job; the fetch/order/aggregate/alert decisions
//! themselves live in `quota_core::refresh`, so Android can drive the same
//! operation from its own scheduler without duplicating any of that logic.

use crate::{tray, AppState};
use quota_core::alerts::{self, AlertLevel};
use quota_core::model::UsageSnapshot;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
#[cfg(not(target_os = "linux"))]
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

    let prior = state.snapshots.read().await.clone();
    let mut engine = state.alert_engine.lock().await;
    let outcome = quota_core::refresh::refresh(&ctx, &prior, &mut engine).await;
    drop(engine);

    for write in &outcome.credential_writes {
        if let Err(e) = &write.result {
            eprintln!("failed to persist rotated secret {}: {e}", write.key);
        }
    }

    // Update shared state with the ordered, stale-merged snapshots the
    // operation produced.
    {
        let mut map = state.snapshots.write().await;
        for s in &outcome.snapshots {
            map.insert(s.provider_id.clone(), s.clone());
        }
    }

    // Tray icon: the operation already folded every tray-eligible account into
    // one aggregate status and percentage.
    let tip_lines: Vec<String> = outcome.snapshots.iter().map(tooltip_line).collect();
    let tooltip = if tip_lines.is_empty() {
        "Quota Widget — no providers enabled".into()
    } else {
        tip_lines.join("\n")
    };
    #[cfg(target_os = "linux")]
    crate::tray_linux::set_status(
        outcome.aggregate.status,
        outcome.aggregate.pct / 100.0,
        tooltip,
    );
    #[cfg(not(target_os = "linux"))]
    tray::set_status(
        app,
        outcome.aggregate.status,
        outcome.aggregate.pct / 100.0,
        &tooltip,
    );

    // Alerts: dispatch per the presentation policy, which folds the
    // per-account toggles together with the tray-first launch rule — a state
    // the first poll merely *found* must never throw the main window at a user
    // who has not asked for anything.
    for event in &outcome.alerts {
        let toggles = cfg.effective_alerts(&event.provider_id);
        let how = alerts::presentation(event, &toggles);
        let title = match event.level {
            AlertLevel::Critical => format!("{} — critical", event.provider_name),
            _ => format!("{} — warning", event.provider_name),
        };
        if how.notify {
            // On Linux, tauri-plugin-notification's `show()` wraps notify-rust's
            // *sync* `show()` in `tauri::async_runtime::spawn`. That sync path
            // calls `zbus::block_on`, which — with zbus's `tokio` feature
            // (enabled by ksni) — builds a nested multi-thread runtime and
            // panics from inside a tokio worker ("Cannot start a runtime from
            // within a runtime"). notify-rust's `show_async()` instead speaks
            // zbus over the runtime already driving this task, so it is safe
            // to await here. Windows and macOS don't go through zbus, so the
            // plugin's path is fine there.
            #[cfg(target_os = "linux")]
            {
                let _ = notify_rust::Notification::new()
                    .summary(&title)
                    .body(&event.message)
                    .show_async()
                    .await;
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = app
                    .notification()
                    .builder()
                    .title(&title)
                    .body(&event.message)
                    .show();
            }
        }
        if how.open_window {
            tray::show_popup(app, None);
        }
    }

    crate::emit_snapshots(app, &outcome.snapshots);
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
