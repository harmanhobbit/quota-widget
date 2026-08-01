//! Native StatusNotifierItem tray for Linux. Unlike appindicator, SNI exposes
//! both activation and a tooltip property, which Plasma renders on hover.

use crate::{tray, AppState};
use ksni::TrayMethods;
use quota_core::model::Status;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, PhysicalPosition};

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
        tray::toggle_popup(&self.app, Some(PhysicalPosition::new(x as f64, y as f64)));
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open".into(),
                activate: Box::new(|t| tray::show_popup(&t.app, None)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Refresh now".into(),
                activate: Box::new(|t| {
                    if let Some(s) = t.app.try_state::<Arc<AppState>>() {
                        s.refresh.notify_one()
                    }
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Settings".into(),
                activate: Box::new(|t| {
                    tray::show_popup(&t.app, None);
                    let _ = t.app.emit("navigate", "settings");
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t| t.app.exit(0)),
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

pub fn create_tray(app: AppHandle, _state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        match (QuotaTray {
            app,
            status: Status::Stale,
            fill: 1.0,
            lines: "Quota Widget — waiting for first poll".into(),
        })
        .spawn()
        .await
        {
            Ok(h) => *handle().lock().unwrap() = Some(h),
            Err(e) => eprintln!("failed to start Linux tray: {e}"),
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
