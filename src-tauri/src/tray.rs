//! Tray icon with runtime-generated status colors, menu, and popup placement.

use quota_core::config::{Corner, MiniAnchor, Rect};
use quota_core::model::Status;
use quota_core::settings_return::SettingsReturn;
use tauri::image::Image;
#[cfg(not(target_os = "linux"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(not(target_os = "linux"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, Runtime};

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
            "settings" => open_settings_from_tray(app),
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

/// The monitor the anchor names, or the best available stand-in.
///
/// The stored name is a preference, not a fact about the current layout: when
/// it matches nothing connected — an undocked laptop — this falls back to the
/// primary monitor and the caller leaves the stored name alone, so reconnecting
/// restores the summary without the user asking again. An anchor naming no
/// monitor keeps the pre-existing behaviour of using whichever monitor the
/// window is already on. See docs/adr/0001-monitor-identity-by-name.md.
fn resolve_monitor<R: Runtime>(
    win: &tauri::WebviewWindow<R>,
    anchor: &MiniAnchor,
) -> Option<Monitor> {
    if let Some(name) = anchor.monitor.as_deref() {
        if let Ok(monitors) = win.available_monitors() {
            if let Some(found) = monitors
                .into_iter()
                .find(|m| m.name().is_some_and(|n| n == name))
            {
                return Some(found);
            }
        }
        // Named a monitor that is not here: primary, then current.
        if let Ok(Some(primary)) = win.primary_monitor() {
            return Some(primary);
        }
    }
    win.current_monitor().ok().flatten()
}

/// Pin the window to a corner of its anchored monitor's work area.
///
/// Work area rather than monitor bounds throughout: on the bottom edge that is
/// immediately above a panel, where monitor bounds would put the window
/// underneath it. The 12px inset applies to the left and right edges only, so a
/// bottom-anchored summary still sits flush above the panel as it always has.
pub fn anchor_to<R: Runtime>(win: &tauri::WebviewWindow<R>, anchor: &MiniAnchor) {
    let (Ok(size), Some(monitor)) = (win.outer_size(), resolve_monitor(win, anchor)) else {
        return;
    };
    let _ = win.set_position(corner_position(&monitor, &size, anchor.corner));
}

/// Where a window of `size` sits when pinned to `corner` of `monitor`.
///
/// The 12px inset is horizontal only, so a bottom-anchored summary stays flush
/// above the panel exactly as it always has.
fn corner_position(
    monitor: &Monitor,
    size: &PhysicalSize<u32>,
    corner: Corner,
) -> PhysicalPosition<i32> {
    let area = monitor.work_area();
    let margin = 12;
    let x = if corner.is_right() {
        area.position.x + area.size.width as i32 - size.width as i32 - margin
    } else {
        area.position.x + margin
    };
    let y = if corner.is_bottom() {
        area.position.y + area.size.height as i32 - size.height as i32
    } else {
        area.position.y
    };
    PhysicalPosition::new(x.max(area.position.x), y)
}

/// The anchor the user has chosen, or the default placement when state is not
/// reachable (which is also what every build before the anchor existed did).
pub fn current_anchor<R: Runtime>(app: &AppHandle<R>) -> MiniAnchor {
    app.try_state::<std::sync::Arc<crate::AppState>>()
        .map(|state| state.mini_anchor.lock().unwrap().clone())
        .unwrap_or_default()
}

/// Resize the mini summary to fit its content and immediately re-anchor it.
///
/// These are one operation, not two: a bottom-corner anchor pins the window's
/// *bottom* edge to the work area, so changing the height moves the top edge.
/// Resizing without re-anchoring in the same step leaves the window sitting
/// wherever its old top-left put it, and the summary visibly jumps.
pub fn resize_mini_to<R: Runtime>(
    win: &tauri::WebviewWindow<R>,
    anchor: &MiniAnchor,
    logical_height: f64,
) {
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
    // A window GTK has not mapped yet — `mini` starts hidden — reports its
    // inner width as 0, and forwarding that is the `gtk_window_resize`
    // assertion 'width > 0' printed at every Linux start (#176). Skip the fit:
    // `show_mini` re-anchors on show, and the next measurement once the
    // summary is visible sizes it for real.
    if size.width == 0 {
        return;
    }
    let _ = win.set_size(PhysicalSize::new(size.width, height));
    anchor_to(win, anchor);
}

/// How long the window must stop moving before a drag counts as dropped, and
/// the snap animation's shape. The settle delay is a compromise: long enough
/// that a pause mid-drag is not mistaken for a drop, short enough that the snap
/// does not feel disconnected from letting go.
const DRAG_SETTLE_MS: u64 = 180;
const SNAP_STEPS: u32 = 8;
const SNAP_STEP_MS: u64 = 15;

/// Wait for a dragging mini summary to stop moving, then commit where it
/// landed as the new anchor.
///
/// `gen` is the move counter's value when this was scheduled; a later move
/// bumps it and makes this call a no-op, so only the last move of a drag
/// resolves it. A native drag gives no end event, which is why the drop is
/// inferred from stillness rather than observed.
pub fn settle_mini_drag<R: Runtime>(app: &AppHandle<R>, gen: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(DRAG_SETTLE_MS)).await;
        use std::sync::atomic::Ordering::Relaxed;
        // Cloned out of the borrowed `State`, which must not be held across the
        // await below.
        let Some(state) = app
            .try_state::<std::sync::Arc<crate::AppState>>()
            .map(|s| (*s).clone())
        else {
            return;
        };
        // Something moved after us: that later move owns the drop, not this one.
        if state.mini_drag_gen.load(Relaxed) != gen {
            return;
        }
        state.mini_dragging.store(false, Relaxed);
        commit_drop(&app).await;
    });
}

/// The anchor the mini summary's current position implies, without moving it.
///
/// Where the window already is, is the answer to "which corner is this?" — so
/// both a drag's drop and the moment of pinning derive their anchor from here
/// and neither has to relocate the window to find out. `None` when the geometry
/// or the monitor cannot be read, which is a reason to leave the anchor alone
/// rather than to guess one.
fn anchor_here<R: Runtime>(
    app: &AppHandle<R>,
    win: &tauri::WebviewWindow<R>,
) -> Option<MiniAnchor> {
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
        return None;
    };
    let rect = Rect::new(
        pos.x as f64,
        pos.y as f64,
        size.width as f64,
        size.height as f64,
    );
    let (centre_x, centre_y) = rect.centre();
    // The monitor under the window's centre, so a summary straddling two
    // screens belongs to the one it is mostly on.
    let monitor = win.monitor_from_point(centre_x, centre_y).ok().flatten()?;
    let area = monitor.work_area();
    // Work area, not monitor bounds: the corner derived here is the one the
    // window will later be placed against, and placement avoids panels.
    Some(MiniAnchor::derive(
        rect,
        Rect::new(
            area.position.x as f64,
            area.position.y as f64,
            area.size.width as f64,
            area.size.height as f64,
        ),
        monitor.name().cloned(),
        &current_anchor(app),
    ))
}

/// Adopt the summary's current spot as its anchor without moving it.
///
/// This is what pinning does. Pinning must never relocate the summary the user
/// can already see — that was the whole defect (#72) — so the derived corner is
/// stored purely so a *later* content resize grows from the right work-area
/// edge. Spawned because storing takes the async config lock, and the pin
/// command is a sync IPC call.
pub fn adopt_position_as_anchor<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(win) = app.get_webview_window("mini") else {
            return;
        };
        let Some(anchor) = anchor_here(&app, &win) else {
            return;
        };
        store_anchor(&app, anchor).await;
    });
}

/// Turn the window's current spot into a stored anchor, then move it there.
async fn commit_drop<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window("mini") else {
        return;
    };
    let Some(anchor) = anchor_here(app, &win) else {
        return;
    };
    store_anchor(app, anchor.clone()).await;
    snap_to(&win, &anchor);
}

/// Persist an anchor everywhere it is read from: the sync-loop mirror, the
/// config file, and the frontend's copy.
async fn store_anchor<R: Runtime>(app: &AppHandle<R>, anchor: MiniAnchor) {
    // Cloned out of the borrowed `State` before any await: the guard below is
    // held across one, and a borrow of the app handle must not be.
    let state = app
        .try_state::<std::sync::Arc<crate::AppState>>()
        .map(|s| (*s).clone());
    if let Some(state) = state {
        // Scoped: this is a std Mutex, whose guard is not Send and so must be
        // dropped before the async config lock is taken below.
        {
            *state.mini_anchor.lock().unwrap() = anchor.clone();
        }
        // The write guard is released before emitting: a listener that calls
        // back into config would otherwise deadlock against it.
        let changed = {
            let mut config = state.config.write().await;
            if config.mini_anchor == anchor {
                None
            } else {
                config.mini_anchor = anchor.clone();
                if let Err(e) = config.save(&state.config_dir) {
                    eprintln!("failed to save mini anchor: {e}");
                }
                Some(config.clone())
            }
        };
        // Same channel `set_config` uses: the frontend's copy is refreshed now,
        // so a later Settings save carries this anchor rather than overwriting
        // it with the config it was holding before the drag.
        if let Some(config) = changed {
            use tauri::Emitter;
            let _ = app.emit("config", &config);
        }
    }
}

/// Slide the window to its anchor.
///
/// Deliberately short and uninterruptible. A content-height change from the
/// poller can land mid-flight and re-anchor at the new height; this finishes
/// last and wins, so the window ends in the right corner, at worst a frame or
/// two late in height. Nothing here can leave it in the wrong place.
fn snap_to<R: Runtime>(win: &tauri::WebviewWindow<R>, anchor: &MiniAnchor) {
    let (Ok(from), Ok(size), Some(monitor)) = (
        win.outer_position(),
        win.outer_size(),
        resolve_monitor(win, anchor),
    ) else {
        return;
    };
    let to = corner_position(&monitor, &size, anchor.corner);
    let win = win.clone();
    tauri::async_runtime::spawn(async move {
        for step in 1..=SNAP_STEPS {
            // Ease-out: most of the distance early, so it reads as settling
            // into the corner rather than sliding at a constant speed.
            let t = step as f64 / SNAP_STEPS as f64;
            let eased = 1.0 - (1.0 - t) * (1.0 - t);
            let x = from.x as f64 + (to.x - from.x) as f64 * eased;
            let y = from.y as f64 + (to.y - from.y) as f64 * eased;
            let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
            tokio::time::sleep(std::time::Duration::from_millis(SNAP_STEP_MS)).await;
        }
        // Land exactly on the anchor: the eased steps are rounded, and a
        // resize during the slide would have changed the target's height.
        let _ = win.set_position(to);
    });
}

/// What the `navigate` event carries. The view is the only thing the frontend
/// needed before the Settings return state existed; the return state rides
/// alongside it because it is decided here, in the one place that can still see
/// what was on screen before the full window covered it.
#[derive(Clone, serde::Serialize)]
struct Navigate {
    view: &'static str,
    return_to: SettingsReturn,
}

/// Tray-menu Settings, for both tray implementations.
///
/// The Settings return state is read *before* anything is shown: showing the
/// full window hides the mini summary, after which nothing distinguishes "the
/// summary was open" from "nothing was open". A visible mini summary is then
/// held for the duration of the visit — including an unpinned one, which the
/// ordinary popup path deliberately dismisses.
pub fn open_settings_from_tray<R: Runtime>(app: &AppHandle<R>) {
    let visible = |label| {
        app.get_webview_window(label)
            .is_some_and(|w| w.is_visible().unwrap_or(false))
    };
    let return_to = SettingsReturn::for_tray_entry(visible("mini"), visible("main"));
    show_popup_holding_mini(app, None, return_to == SettingsReturn::Mini);
    use tauri::Emitter;
    let _ = app.emit(
        "navigate",
        Navigate {
            view: "settings",
            return_to,
        },
    );
}

/// Leave Settings for its captured return state.
///
/// The full window itself needs nothing done to it when the return state is the
/// usage popup — the frontend has already swapped its own view — so this is
/// only about the window lifecycle the frontend cannot reach.
pub fn exit_settings<R: Runtime>(app: &AppHandle<R>, to: SettingsReturn) {
    match to {
        SettingsReturn::Popup => {}
        SettingsReturn::Mini => {
            // The pinned-mini hold is consumed here rather than left armed: the
            // summary is being restored now, so a later hide of the full window
            // must not restore it a second time.
            if let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() {
                state
                    .reopen_mini_after_popup
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
            // Hides the full window itself, and reinstates always-on-top from
            // the pin the summary already had.
            show_mini(app, None);
        }
        SettingsReturn::Hidden => {
            // Nothing was on screen when Settings opened, so nothing may be on
            // screen when it leaves. A pinned-mini hold left armed by an
            // earlier popup would otherwise make `hide_popup` reopen the
            // summary and contradict the captured state.
            if let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() {
                state
                    .reopen_mini_after_popup
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
            hide_popup(app);
        }
    }
}

/// Show the always-on-top popup, positioned near the tray click when we know
/// it. Dismisses the mini summary first so the two never overlap.
pub fn show_popup<R: Runtime>(app: &AppHandle<R>, near: Option<PhysicalPosition<f64>>) {
    show_popup_holding_mini(app, near, false);
}

/// `hold_mini` is set only by the tray's Settings entry, where a visible mini
/// summary is coming back when Settings exits and so must be hidden rather than
/// dismissed — pin and all. Every other caller passes `false` and keeps the
/// long-standing behaviour of dismissing an unpinned summary outright.
fn show_popup_holding_mini<R: Runtime>(
    app: &AppHandle<R>,
    near: Option<PhysicalPosition<f64>>,
    hold_mini: bool,
) {
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
    // A Settings visit holds the summary whether or not it is pinned, because
    // the captured Settings return state is going to bring it back. The
    // ordinary popup path still only holds a pinned one, and dismisses the
    // rest. Either way the window is hidden directly rather than through
    // `hide_mini`, which would drop the pin we are preserving.
    if preserve_pinned_mini || (hold_mini && mini_visible) {
        if let Some(mini) = app.get_webview_window("mini") {
            let _ = mini.hide();
        }
        // Only a pinned summary comes back on an ordinary hide-to-tray. An
        // unpinned one held for Settings returns solely through
        // `exit_settings`, so ✕ or a click-away still means "no window".
        if preserve_pinned_mini {
            if let Some(state) = app.try_state::<std::sync::Arc<crate::AppState>>() {
                state
                    .reopen_mini_after_popup
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
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
    anchor_to(&win, &current_anchor(app));
    let _ = win.set_focus();
    // The show notification for the summary's webview, which survives hiding.
    // Unlike `main`/`window-shown`, this deliberately does **not** reset the
    // scroll fade: the summary keeps the level it was left at for the life of
    // the process, so a tray toggle brings it back exactly as it looked. A
    // fully transparent unpinned summary is recoverable by the same click that
    // opened it, which is why it is allowed to reach zero at all.
    use tauri::Emitter;
    let _ = app.emit("mini-shown", ());
}
