//! The control window: small, always on top, no title bar. It exists only while recording, and MeetingMind's own
//! windows are hidden from the recording while it does.

use tauri::{
    AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::system;

pub const LABEL: &str = "control";
const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 84.0;
/// Distance from the top edge of the screen, in logical pixels.
const TOP_MARGIN: f64 = 12.0;

/// Opens the control window at the top centre of the primary monitor and hides it from screen capture before it is
/// first shown, so not one frame of it can reach the recording. Returns `Ok(false)` if it is open but could not be
/// hidden from capture (an old Windows build): the caller warns the user.
pub fn open(app: &AppHandle) -> tauri::Result<bool> {
    if let Some(existing) = app.get_webview_window(LABEL) {
        let _ = existing.show();
        return Ok(true);
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("MeetingMind recording")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()?;
    place_at_top_centre(app, &window);
    let hidden = system::set_app_window_excluded_from_capture(&window, true).is_ok();
    window.show()?;
    Ok(hidden)
}

fn place_at_top_centre(app: &AppHandle, window: &WebviewWindow) {
    let Ok(Some(monitor)) = app.primary_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let origin = monitor.position();
    let x = f64::from(origin.x) + (f64::from(monitor.size().width) - WIDTH * scale) / 2.0;
    let y = f64::from(origin.y) + TOP_MARGIN * scale;
    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

/// Closes the control window if it is open.
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.destroy();
    }
}

/// Hides (or shows again) every window of the app in screen recordings. Returns false if any window could not be
/// changed.
pub fn set_all_hidden_from_capture(app: &AppHandle, hidden: bool) -> bool {
    // Every window is changed (no short-circuit), then the answer is whether all of them worked.
    let results: Vec<bool> = app
        .webview_windows()
        .values()
        .map(|window| system::set_app_window_excluded_from_capture(window, hidden).is_ok())
        .collect();
    results.into_iter().all(|ok| ok)
}
