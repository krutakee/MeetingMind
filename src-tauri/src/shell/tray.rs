//! The tray icon: one icon that shows whether MeetingMind is idle, recording or paused, with a small menu.
//!
//! The recording and paused icons are the app icon with a coloured dot in the corner, drawn here, so no extra image
//! files are needed.

use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::recorder::Phase;

const TRAY_ID: &str = "main";
const RECORDING_DOT: [u8; 3] = [0xE5, 0x39, 0x35];
const PAUSED_DOT: [u8; 3] = [0xF5, 0xA6, 0x23];

const ITEM_START_STOP: &str = "start-stop";
const ITEM_PAUSE_RESUME: &str = "pause-resume";
const ITEM_OPEN: &str = "open";
const ITEM_QUIT: &str = "quit";

/// Draws a filled circle of `colour` in the bottom-right corner of an RGBA picture, over a thin white ring so it
/// stays visible on any icon. Returns a new picture; the input is untouched.
pub fn with_dot(rgba: &[u8], width: u32, height: u32, colour: [u8; 3]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let radius = f64::from(width.min(height)) * 0.28;
    let ring = (radius * 0.22).max(1.0);
    let (cx, cy) = (
        f64::from(width) - radius - 1.0,
        f64::from(height) - radius - 1.0,
    );
    for y in 0..height {
        for x in 0..width {
            let distance = (f64::from(x) + 0.5 - cx).hypot(f64::from(y) + 0.5 - cy);
            let paint = if distance <= radius {
                colour
            } else if distance <= radius + ring {
                [255, 255, 255]
            } else {
                continue;
            };
            let at = (y * width + x) as usize * 4;
            out[at..at + 3].copy_from_slice(&paint);
            out[at + 3] = 255;
        }
    }
    out
}

/// What the menu and the tooltip say for a phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayLook {
    pub tooltip: &'static str,
    pub start_stop: &'static str,
    pub start_stop_enabled: bool,
    pub pause_resume: &'static str,
    pub pause_resume_enabled: bool,
}

pub fn look_for(phase: Phase) -> TrayLook {
    match phase {
        Phase::Idle => TrayLook {
            tooltip: "MeetingMind",
            start_stop: "Start recording",
            start_stop_enabled: true,
            pause_resume: "Pause",
            pause_resume_enabled: false,
        },
        Phase::Starting => TrayLook {
            tooltip: "MeetingMind - starting",
            start_stop: "Start recording",
            start_stop_enabled: false,
            pause_resume: "Pause",
            pause_resume_enabled: false,
        },
        Phase::Recording => TrayLook {
            tooltip: "MeetingMind - recording",
            start_stop: "Stop recording",
            start_stop_enabled: true,
            pause_resume: "Pause",
            pause_resume_enabled: true,
        },
        Phase::Paused => TrayLook {
            tooltip: "MeetingMind - paused",
            start_stop: "Stop recording",
            start_stop_enabled: true,
            pause_resume: "Resume",
            pause_resume_enabled: true,
        },
        Phase::Finalizing => TrayLook {
            tooltip: "MeetingMind - saving the recording",
            start_stop: "Start recording",
            start_stop_enabled: false,
            pause_resume: "Pause",
            pause_resume_enabled: false,
        },
    }
}

/// The menu items, kept so their text and enabled state can follow the recording.
struct TrayItems {
    start_stop: MenuItem<Wry>,
    pause_resume: MenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let look = look_for(Phase::Idle);
    let start_stop = MenuItem::with_id(
        app,
        ITEM_START_STOP,
        look.start_stop,
        look.start_stop_enabled,
        None::<&str>,
    )?;
    let pause_resume = MenuItem::with_id(
        app,
        ITEM_PAUSE_RESUME,
        look.pause_resume,
        look.pause_resume_enabled,
        None::<&str>,
    )?;
    let open = MenuItem::with_id(app, ITEM_OPEN, "Open MeetingMind", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ITEM_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &start_stop,
            &pause_resume,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(look.tooltip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                super::show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayItems {
        start_stop,
        pause_resume,
    });
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ITEM_START_STOP => super::toggle_start_stop(app),
        ITEM_PAUSE_RESUME => super::toggle_pause(app),
        ITEM_OPEN => super::show_main_window(app),
        ITEM_QUIT => super::quit(app),
        _ => {}
    }
}

/// Makes the icon, tooltip and menu match the recording phase.
pub fn update(app: &AppHandle, phase: Phase) {
    let look = look_for(phase);
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.start_stop.set_text(look.start_stop);
        let _ = items.start_stop.set_enabled(look.start_stop_enabled);
        let _ = items.pause_resume.set_text(look.pause_resume);
        let _ = items.pause_resume.set_enabled(look.pause_resume_enabled);
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let _ = tray.set_tooltip(Some(look.tooltip));
    let dot = match phase {
        Phase::Recording | Phase::Starting => Some(RECORDING_DOT),
        Phase::Paused => Some(PAUSED_DOT),
        Phase::Idle | Phase::Finalizing => None,
    };
    let Some(base) = app.default_window_icon() else {
        return;
    };
    let icon = match dot {
        Some(colour) => Image::new_owned(
            with_dot(base.rgba(), base.width(), base.height(), colour),
            base.width(),
            base.height(),
        ),
        None => base.clone(),
    };
    let _ = tray.set_icon(Some(icon));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(size: u32) -> Vec<u8> {
        vec![0; (size * size * 4) as usize]
    }

    fn pixel(rgba: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    #[test]
    fn the_dot_is_drawn_in_the_bottom_right_corner_and_nowhere_else() {
        let size = 32;
        let out = with_dot(&blank(size), size, size, [200, 10, 20]);

        // Radius 0.28 * 32 = 8.96, centre (22.04, 22.04): (22, 22) is inside.
        assert_eq!(pixel(&out, size, 22, 22), [200, 10, 20, 255]);
        assert_eq!(
            pixel(&out, size, 0, 0),
            [0, 0, 0, 0],
            "the opposite corner is untouched"
        );
        assert_eq!(pixel(&out, size, 16, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn the_dot_has_a_white_ring_so_it_shows_on_any_icon() {
        let size = 32;
        let out = with_dot(&blank(size), size, size, [200, 10, 20]);

        // Just outside the radius of 8.96 around (22.04, 22.04), along the row: x = 12 is at distance 9.5, in the ring.
        assert_eq!(pixel(&out, size, 12, 22), [255, 255, 255, 255]);
    }

    #[test]
    fn the_input_picture_is_not_changed() {
        let size = 16;
        let original = blank(size);

        let _ = with_dot(&original, size, size, [1, 2, 3]);

        assert!(original.iter().all(|&b| b == 0));
    }

    #[test]
    fn only_a_recording_can_be_paused_or_stopped_from_the_menu() {
        for (phase, can_stop, can_pause) in [
            (Phase::Idle, false, false),
            (Phase::Starting, false, false),
            (Phase::Recording, true, true),
            (Phase::Paused, true, true),
            (Phase::Finalizing, false, false),
        ] {
            let look = look_for(phase);
            let stop_available = look.start_stop == "Stop recording" && look.start_stop_enabled;
            assert_eq!(stop_available, can_stop, "{phase:?}");
            assert_eq!(look.pause_resume_enabled, can_pause, "{phase:?}");
        }
    }

    #[test]
    fn a_paused_recording_offers_resume_and_only_idle_offers_start() {
        assert_eq!(look_for(Phase::Paused).pause_resume, "Resume");
        assert_eq!(look_for(Phase::Recording).pause_resume, "Pause");
        assert!(look_for(Phase::Idle).start_stop_enabled);
        assert!(
            !look_for(Phase::Finalizing).start_stop_enabled,
            "no second recording while one is being saved"
        );
    }
}
