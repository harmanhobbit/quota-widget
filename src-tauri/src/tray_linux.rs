//! Native StatusNotifierItem tray for Linux. Unlike appindicator, SNI exposes
//! both activation and a tooltip property, which Plasma renders on hover.

use crate::{tray, AppState};
use ksni::TrayMethods;
use quota_core::model::Status;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition};

struct QuotaTray {
    app: AppHandle,
    status: Status,
    fill: f64,
    lines: String,
}

impl ksni::Tray for QuotaTray {
    fn id(&self) -> String {
        "quota-widget".into()
    }
    fn title(&self) -> String {
        "Quota Widget".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let image = tray::icon_for(self.status, self.fill);
        let mut data = image.rgba().to_vec();
        for px in data.chunks_exact_mut(4) {
            px.rotate_right(1);
        }
        vec![ksni::Icon {
            width: image.width() as i32,
            height: image.height() as i32,
            data,
        }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Quota Widget".into(),
            description: self.lines.clone(),
            ..Default::default()
        }
    }
    fn activate(&mut self, x: i32, y: i32) {
        tray::toggle_mini(&self.app, Some(PhysicalPosition::new(x as f64, y as f64)));
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open".into(),
                activate: Box::new(|t: &mut QuotaTray| tray::show_popup(&t.app, None)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Refresh now".into(),
                activate: Box::new(|t: &mut QuotaTray| {
                    if let Some(s) = t.app.try_state::<Arc<AppState>>() {
                        s.refresh.notify_one()
                    }
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Settings".into(),
                activate: Box::new(|t: &mut QuotaTray| tray::open_settings_from_tray(&t.app)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut QuotaTray| t.app.exit(0)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

static HANDLE: OnceLock<Mutex<Option<ksni::Handle<QuotaTray>>>> = OnceLock::new();
fn handle() -> &'static Mutex<Option<ksni::Handle<QuotaTray>>> {
    HANDLE.get_or_init(|| Mutex::new(None))
}

// The StatusNotifierWatcher a tray registers against is not guaranteed to be on
// the session bus when the app starts. Under Plasma 6 the watcher name
// `org.kde.StatusNotifierWatcher` is owned by kded6, which starts *after*
// `graphical-session.target` is reached — so an app launched by the session can
// win the race and either find no watcher at all or catch it mid-activation and
// get a transient `org.freedesktop.DBus.Error.NoReply: Remote peer
// disconnected`. `spawn()` makes exactly one registration attempt, so a
// momentary absence used to become a permanent window fallback for the whole
// session. Retry with capped exponential backoff so a late watcher is picked up.
const RETRY_BUDGET: Duration = Duration::from_secs(30);
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

pub fn create_tray(app: AppHandle, _state: Arc<AppState>) {
    let fallback = app.clone();
    tauri::async_runtime::spawn(async move {
        // Retry initial registration until it succeeds or the budget is spent.
        // Once it succeeds we do *not* need to watch for the watcher restarting
        // mid-session (e.g. a kded6 restart): ksni's own service loop already
        // subscribes to NameOwnerChanged for org.kde.StatusNotifierWatcher and
        // re-registers when it reappears. This loop only closes the startup gap.
        let start = Instant::now();
        let mut backoff = INITIAL_BACKOFF;
        let mut attempt = 0u32;
        let registered = loop {
            attempt += 1;
            let tray = QuotaTray {
                app: app.clone(),
                status: Status::Stale,
                fill: 1.0,
                lines: "Quota Widget — waiting for first poll".into(),
            };
            match tray.spawn().await {
                Ok(h) => break Some(h),
                Err(e) => {
                    // Bound the retrying so a genuinely tray-less desktop still
                    // reaches the window fallback in reasonable time rather than
                    // spinning forever.
                    let remaining = RETRY_BUDGET.saturating_sub(start.elapsed());
                    if remaining.is_zero() {
                        eprintln!(
                            "failed to start Linux tray after {attempt} attempt(s) \
                             over {:?} ({e}); showing the main window instead",
                            start.elapsed()
                        );
                        break None;
                    }
                    eprintln!(
                        "Linux tray registration attempt {attempt} failed ({e}); \
                         retrying in {backoff:?}"
                    );
                    // Async sleep only — no block_on: a sync zbus path here would
                    // build a nested multi-thread runtime inside this tokio worker
                    // and panic (see the ksni note in Cargo.toml).
                    tokio::time::sleep(backoff.min(remaining)).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        };

        match registered {
            Some(h) => {
                if attempt > 1 {
                    eprintln!("Linux tray registered on attempt {attempt}");
                }
                *handle().lock().unwrap() = Some(h);
            }
            // Launch is tray-first, which only works if there is a tray. A
            // desktop with no StatusNotifierItem host would otherwise leave a
            // running process with no way to reach it, so the main window
            // becomes the point of access instead.
            None => tray::show_popup(&fallback, None),
        }
    });
}

pub fn set_status(status: Status, fill: f64, lines: String) {
    if let Some(h) = handle().lock().unwrap().as_ref().cloned() {
        tauri::async_runtime::spawn(async move {
            let _ = h
                .update(|t| {
                    t.status = status;
                    t.fill = fill;
                    t.lines = lines;
                })
                .await;
        });
    }
}
