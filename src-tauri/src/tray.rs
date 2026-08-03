//! Tray icon with runtime-generated status colors, menu, and popup placement.

use quota_core::model::Status;
use tauri::image::Image;
#[cfg(not(target_os = "linux"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(not(target_os = "linux"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime};

#[cfg(not(target_os = "linux"))]
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
        Status::Ok => badge_icon([0x4a, 0xda, 0x7c], fill), // green
        Status::Warn => badge_icon([0xe6, 0xa8, 0x17], fill), // amber
        Status::Critical => badge_icon([0xd6, 0x36, 0x38], fill), // red
        Status::Stale => badge_icon([0x8a, 0x8a, 0x8a], 1.0), // grey
    }
}

#[cfg(not(target_os = "linux"))]
pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id("open", "Open").build(app)?;
    let refresh = MenuItemBuilder::with_id("refresh", "Refresh now").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&open, &refresh, &settings, &quit])
        .build()?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon_for(Status::Stale, 1.0))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_popup(app, None),
            "settings" => {
                // Show first: show_popup resets the view to the usage list,
                // so navigating afterwards is what makes Settings stick.
                show_popup(app, None);
                use tauri::Emitter;
                let _ = app.emit("navigate", "settings");
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
            let app = tray.app_handle();
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } => {
                    // The tray click is deliberately a compact, transient
                    // summary. The context menu's Open item leads to the full
                    // usage/settings window. Hover is handled entirely by the
                    // shell's native tooltip, so no Enter/Leave handling here.
                    toggle_mini(app, Some(position));
                }
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}

/// `tooltip` is the same detailed multiline string `ksni` publishes on Linux,
/// so both platforms show one hover surface listing every reported window and
/// balance.
#[cfg(not(target_os = "linux"))]
pub fn set_status<R: Runtime>(app: &AppHandle<R>, status: Status, fill: f64, tooltip: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(icon_for(status, fill)));
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

/// Clamp a window to the monitor work area next to a tray position, flipping
/// above/below the cursor depending on which half of the screen the tray is in.
fn place_near_tray<R: Runtime>(win: &tauri::WebviewWindow<R>, pos: PhysicalPosition<f64>) {
    let (Ok(size), Ok(Some(monitor))) = (win.outer_size(), win.current_monitor()) else {
        return;
    };
    let area = monitor.work_area();
    let msize = area.size;
    let mpos = area.position;
    let margin = 12.0;
    let x = (pos.x - size.width as f64 / 2.0).clamp(
        mpos.x as f64 + margin,
        mpos.x as f64 + msize.width as f64 - size.width as f64 - margin,
    );
    let y = if pos.y > mpos.y as f64 + msize.height as f64 / 2.0 {
        pos.y - size.height as f64 - margin
    } else {
        pos.y + margin
    };
    let _ = win.set_position(PhysicalPosition::new(x as i32, y as i32));
}

pub fn toggle_mini<R: Runtime>(app: &AppHandle<R>, near: Option<PhysicalPosition<f64>>) {
    let Some(win) = app.get_webview_window("mini") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        hide_mini(app);
    } else {
        show_mini(app, near);
    }
}

/// Hide the mini summary and drop its pin. Pinning is deliberately a
/// per-showing state, not a sticky one: closing the summary is the user
/// dismissing it, so the next tray click should open an ordinary transient
/// summary rather than silently reinstating always-on-top. Every path that
/// hides this window must go through here, or the flag outlives the window.
pub fn hide_mini<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window("mini") else {
        return;
    };
    let _ = win.hide();
    if let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() {
        state
            .mini_pinned
            .store(false, std::sync::atomic::Ordering::Relaxed);
        state
            .reopen_mini_after_popup
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
    let _ = win.set_always_on_top(false);
    // The webview survives hiding, so tell the frontend to un-light its pin
    // button; otherwise it would reopen showing pinned while it is not.
    use tauri::Emitter;
    let _ = app.emit("mini-pinned", false);
}

/// Hide the full popup and restore a pinned mini that it temporarily
/// interrupted. Settings lives inside the full popup, so this covers both
/// entry points without creating a second window lifecycle to keep in sync.
pub fn hide_popup<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let _ = win.hide();
    let resume = app
        .try_state::<std::sync::Arc<crate::AppState>>()
        .map(|state| {
            state
                .reopen_mini_after_popup
                .swap(false, std::sync::atomic::Ordering::Relaxed)
        })
        .unwrap_or(false);
    if resume {
        show_mini(app, None);
    }
}

/// Pin to the work area's lower edge: this is immediately above a bottom
/// panel, unlike monitor bounds which place the popup underneath the panel.
pub fn anchor_above_panel<R: Runtime>(win: &tauri::WebviewWindow<R>) {
    let (Ok(size), Ok(Some(monitor))) = (win.outer_size(), win.current_monitor()) else {
        return;
    };
    let area = monitor.work_area();
    let x = area.position.x + area.size.width as i32 - size.width as i32 - 12;
    let y = area.position.y + area.size.height as i32 - size.height as i32;
    let _ = win.set_position(PhysicalPosition::new(x.max(area.position.x), y));
}

/// Resize the mini summary to fit its content and immediately re-anchor it.
///
/// These are one operation, not two: `anchor_above_panel` pins the window's
/// *bottom* edge to the work area, so changing the height moves the top edge.
/// Resizing without re-anchoring in the same step leaves the window sitting
/// wherever its old top-left put it, and the summary visibly jumps.
pub fn resize_mini_to<R: Runtime>(win: &tauri::WebviewWindow<R>, logical_height: f64) {
    let Ok(scale) = win.scale_factor() else {
        return;
    };
    let Ok(size) = win.inner_size() else {
        return;
    };
    // Clamped rather than trusted: the height comes from a DOM measurement, so
    // a mid-render zero or a runaway account list must not produce a window
    // that cannot be seen or cannot be dismissed.
    let height = (logical_height * scale).round().clamp(60.0, 800.0) as u32;
    if height == size.height {
        return;
    }
    let _ = win.set_size(PhysicalSize::new(size.width, height));
    anchor_above_panel(win);
}

/// Show the always-on-top popup, positioned near the tray click when we know
/// it. Dismisses the mini summary first so the two never overlap.
pub fn show_popup<R: Runtime>(app: &AppHandle<R>, near: Option<PhysicalPosition<f64>>) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let mini_visible = app
        .get_webview_window("mini")
        .is_some_and(|mini| mini.is_visible().unwrap_or(false));
    let preserve_pinned_mini = app
        .try_state::<std::sync::Arc<crate::AppState>>()
        .map(|state| {
            state.mini_pinned.load(std::sync::atomic::Ordering::Relaxed)
                && (mini_visible
                    || state
                        .reopen_mini_after_popup
                        .load(std::sync::atomic::Ordering::Relaxed))
        })
        .unwrap_or(false);
    if preserve_pinned_mini {
        if let Some(mini) = app.get_webview_window("mini") {
            let _ = mini.hide();
        }
        if let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() {
            state
                .reopen_mini_after_popup
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    } else {
        hide_mini(app);
    }
    if let Some(pos) = near {
        place_near_tray(&win, pos);
    }
    let _ = win.show();
    let _ = win.set_focus();
    // Hiding to tray keeps the webview alive, so the frontend resets its view
    // here rather than leaving the user back in Settings on next open.
    use tauri::Emitter;
    let _ = app.emit("window-shown", ());
}

/// Show the compact tray-click summary. It is transient unless the mini
/// window itself has been pinned; the full window is never involved.
pub fn show_mini<R: Runtime>(app: &AppHandle<R>, _near: Option<PhysicalPosition<f64>>) {
    let Some(win) = app.get_webview_window("mini") else {
        return;
    };
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    let pinned = app
        .try_state::<std::sync::Arc<crate::AppState>>()
        .map(|state| state.mini_pinned.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false);
    let _ = win.set_always_on_top(pinned);
    // Use the pinned position for both states so toggling the pin changes
    // only click-away behaviour and always-on-top, never the summary's spot.
    let _ = win.show();
    // An invisible X11 window has no reliable current monitor. Position after
    // mapping it so Nix/XWayland builds anchor above Plasma's panel instead of
    // accepting the window manager's top-left default.
    anchor_above_panel(&win);
    let _ = win.set_focus();
    // The webview survives hiding, so a summary scrolled to fully transparent
    // would come back invisible — and an invisible window still eats clicks.
    // `main` gets the same reset from `window-shown` above.
    use tauri::Emitter;
    let _ = app.emit("mini-shown", ());
}
