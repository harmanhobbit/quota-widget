//! Tray icon with runtime-generated status colors, menu, and popup placement.

use quota_core::model::Status;
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Runtime};

pub const TRAY_ID: &str = "quota-tray";

/// Filled circle on transparent background, anti-aliased at the rim.
/// Generated in code so status recoloring needs no bundled assets.
fn circle_icon(rgb: [u8; 3]) -> Image<'static> {
    const S: usize = 32;
    let c = (S as f64 - 1.0) / 2.0;
    let r = c - 1.0;
    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let d = (((x as f64 - c).powi(2) + (y as f64 - c).powi(2)).sqrt() - r).max(0.0);
            let alpha = (1.0 - d).clamp(0.0, 1.0);
            let i = (y * S + x) * 4;
            rgba[i] = rgb[0];
            rgba[i + 1] = rgb[1];
            rgba[i + 2] = rgb[2];
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    Image::new_owned(rgba, S as u32, S as u32)
}

pub fn icon_for(status: Status) -> Image<'static> {
    match status {
        Status::Ok => circle_icon([0x2e, 0xb8, 0x5c]),       // green
        Status::Warn => circle_icon([0xe6, 0xa8, 0x17]),     // amber
        Status::Critical => circle_icon([0xd6, 0x36, 0x38]), // red
        Status::Stale => circle_icon([0x8a, 0x8a, 0x8a]),    // grey
    }
}

pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id("open", "Open").build(app)?;
    let refresh = MenuItemBuilder::with_id("refresh", "Refresh now").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&open, &refresh, &settings, &quit]).build()?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_for(Status::Stale))
        .tooltip("Quota Widget — waiting for first poll")
        .menu(&menu)
        // Linux appindicator trays deliver only menu interactions — raw click
        // events never arrive — so left-click opens the menu there. On
        // Windows/macOS left-click toggles the popup directly.
        .show_menu_on_left_click(cfg!(target_os = "linux"))
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_popup(app, None),
            "settings" => {
                use tauri::Emitter;
                let _ = app.emit("navigate", "settings");
                show_popup(app, None);
            }
            "refresh" => {
                if let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() {
                    state.refresh.notify_one();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                toggle_popup(tray.app_handle(), Some(position));
            }
        })
        .build(app)?;
    Ok(())
}

pub fn set_status<R: Runtime>(app: &AppHandle<R>, status: Status, tooltip: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(icon_for(status)));
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

fn toggle_popup<R: Runtime>(app: &AppHandle<R>, near: Option<PhysicalPosition<f64>>) {
    let Some(win) = app.get_webview_window("main") else { return };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    } else {
        show_popup(app, near);
    }
}

/// Show the always-on-top popup, positioned near the tray click when we know
/// it (clamped to the monitor work area so it never renders off-screen).
pub fn show_popup<R: Runtime>(app: &AppHandle<R>, near: Option<PhysicalPosition<f64>>) {
    let Some(win) = app.get_webview_window("main") else { return };
    if let Some(pos) = near {
        if let (Ok(size), Ok(Some(monitor))) = (win.outer_size(), win.current_monitor()) {
            let msize = monitor.size();
            let mpos = monitor.position();
            let margin = 12.0;
            let x = (pos.x - size.width as f64 / 2.0)
                .clamp(mpos.x as f64 + margin, mpos.x as f64 + msize.width as f64 - size.width as f64 - margin);
            // Place above the tray (bottom taskbar) unless the click was in the
            // top half of the screen (top taskbar / vertical tray).
            let y = if pos.y > mpos.y as f64 + msize.height as f64 / 2.0 {
                pos.y - size.height as f64 - margin
            } else {
                pos.y + margin
            };
            let _ = win.set_position(PhysicalPosition::new(x as i32, y as i32));
        }
    }
    let _ = win.show();
    let _ = win.set_focus();
}
