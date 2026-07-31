//! Tray icon with runtime-generated status colors, menu, and popup placement.

use quota_core::model::Status;
use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Runtime};

pub const TRAY_ID: &str = "quota-tray";

/// The same rounded-square gauge badge as the app icon, drawn at runtime:
/// slate background, a 270° gauge arc whose lit portion reflects the worst
/// usage window (`fill` 0..1) in the status color, and a light center dot.
fn badge_icon(arc_rgb: [u8; 3], fill: f64) -> Image<'static> {
    const S: usize = 32;
    let sf = S as f64;
    let c = (sf - 1.0) / 2.0;
    let half = sf * 0.44;
    let corner = sf * 0.22;
    let (arc_inner, arc_outer) = (sf * 0.26, sf * 0.40);
    let bg = [0x25u8, 0x2b, 0x3a];
    let track = [0x3au8, 0x44, 0x58];
    let dot = [0xe8u8, 0xec, 0xf4];
    // gauge sweeps 225° → -45° (bottom-left, over the top, to bottom-right)
    let start = 5.0 * std::f64::consts::PI / 4.0;
    let sweep = 3.0 * std::f64::consts::PI / 2.0;
    let fill = fill.clamp(0.0, 1.0);

    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let dx = x as f64 - c;
            let dy = y as f64 - c;
            // rounded-square coverage (signed distance, 1px anti-alias band)
            let qx = (dx.abs() - (half - corner)).max(0.0);
            let qy = (dy.abs() - (half - corner)).max(0.0);
            let dist = (qx * qx + qy * qy).sqrt() - corner;
            let cov = (0.5 - dist).clamp(0.0, 1.0);
            if cov == 0.0 {
                continue;
            }
            let mut px = bg;
            let rad = (dx * dx + dy * dy).sqrt();
            if rad >= arc_inner && rad <= arc_outer {
                // angle measured clockwise from the gauge start (y axis flipped)
                let ang = (-dy).atan2(dx);
                let rel = (start - ang).rem_euclid(std::f64::consts::PI * 2.0);
                if rel <= sweep {
                    px = if rel <= sweep * fill { arc_rgb } else { track };
                }
            }
            if rad < sf * 0.07 {
                px = dot;
            }
            let i = (y * S + x) * 4;
            rgba[i] = px[0];
            rgba[i + 1] = px[1];
            rgba[i + 2] = px[2];
            rgba[i + 3] = (cov * 255.0) as u8;
        }
    }
    Image::new_owned(rgba, S as u32, S as u32)
}

/// `fill` is the worst usage fraction (0..1) across enabled providers; stale
/// state shows a full grey arc so it reads as "switched off", not "empty".
pub fn icon_for(status: Status, fill: f64) -> Image<'static> {
    match status {
        Status::Ok => badge_icon([0x4a, 0xda, 0x7c], fill),       // green
        Status::Warn => badge_icon([0xe6, 0xa8, 0x17], fill),     // amber
        Status::Critical => badge_icon([0xd6, 0x36, 0x38], fill), // red
        Status::Stale => badge_icon([0x8a, 0x8a, 0x8a], 1.0),     // grey
    }
}

pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id("open", "Open").build(app)?;
    let refresh = MenuItemBuilder::with_id("refresh", "Refresh now").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&open, &refresh, &settings, &quit]).build()?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_for(Status::Stale, 1.0))
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

pub fn set_status<R: Runtime>(app: &AppHandle<R>, status: Status, fill: f64, tooltip: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(icon_for(status, fill)));
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
