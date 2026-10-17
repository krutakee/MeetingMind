//! The desktop around the recorder: the control window, the tray icon, global shortcuts, the status feed to the UI
//! and the native dialogs. All recording logic lives in `recorder`; this module only connects it to Tauri.
//!
//! Anything the user starts without the Record screen (a shortcut, the tray menu) behaves like pressing Start there:
//! it repeats the setup of the latest recording, shows the consent reminder if that is turned on, and reports a
//! problem in a message box instead of failing silently.

mod control_window;
mod hotkeys;
mod tray;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tracing::{info, warn};

use crate::commands::AppState;
use crate::recorder::{Finished, Phase, Recorder, RecorderError};
use crate::recording::session::WarningKind;
use crate::recording::setup::RecordingSetup;
use crate::storage::settings::Hotkeys;

pub use hotkeys::apply as apply_hotkeys;

/// The event the Record screen and the control window listen to. Its payload is a `recorder::Status`.
pub const STATUS_EVENT: &str = "recorder-status";
const FEED_INTERVAL: Duration = Duration::from_millis(250);
const MAIN_WINDOW: &str = "main";

const CONSENT_MESSAGE: &str = "Before you record: make sure everyone in the meeting knows about it and agrees to being recorded.\n\nYou can turn this reminder off in Settings.";

fn recorder(app: &AppHandle) -> Option<Arc<Recorder>> {
    app.state::<AppState>().recorder.clone()
}

fn emit_status(app: &AppHandle) {
    if let Some(recorder) = recorder(app) {
        let _ = app.emit(STATUS_EVENT, recorder.status());
    }
}

/// Sets up the tray icon, the shortcuts and the status feed. Called once at startup. A shortcut that cannot be
/// registered is returned as a message for the UI; it does not stop the app.
pub fn install(app: &AppHandle, hotkeys: &Hotkeys) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(error) = tray::create(app) {
        warn!(%error, "the tray icon could not be created");
        problems.push(format!("The tray icon could not be created: {error}"));
    }
    if let Err(message) = hotkeys::apply(app, hotkeys) {
        warn!("the global shortcuts could not be registered");
        problems.push(message);
    }
    spawn_status_feed(app.clone());
    problems
}

/// Sends the recording status to every window about four times a second while something is happening, keeps the tray
/// icon in step, and ends a recording that asks to be stopped (the disk is almost full).
fn spawn_status_feed(app: AppHandle) {
    let stopping_by_itself = Arc::new(AtomicBool::new(false));
    let spawned = std::thread::Builder::new()
        .name("status-feed".into())
        .spawn(move || {
            let mut last_phase = Phase::Idle;
            loop {
                std::thread::sleep(FEED_INTERVAL);
                let Some(recorder) = recorder(&app) else {
                    return;
                };
                let status = recorder.status();
                if status.phase != Phase::Idle || last_phase != Phase::Idle {
                    let _ = app.emit(STATUS_EVENT, &status);
                }
                if status.phase != last_phase {
                    tray::update(&app, status.phase);
                    last_phase = status.phase;
                }
                if status.auto_stop.is_some() && !stopping_by_itself.swap(true, Ordering::SeqCst) {
                    let (app, flag) = (app.clone(), stopping_by_itself.clone());
                    std::thread::spawn(move || {
                        end_after_disk_warning(&app);
                        flag.store(false, Ordering::SeqCst);
                    });
                }
            }
        });
    if let Err(error) = spawned {
        warn!(%error, "the status feed could not start");
    }
}

fn end_after_disk_warning(app: &AppHandle) {
    match stop_recording(app) {
        Ok(_) => tell(
            app,
            MessageDialogKind::Warning,
            "The recording was stopped because the disk is almost full. Everything recorded so far was saved.",
        ),
        Err(error) => tell(app, MessageDialogKind::Error, &error.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Starting and ending a recording

/// Starts a recording. `consent_given` says the user has just confirmed the consent reminder; when the reminder is
/// turned on and this is false, nothing starts.
pub fn start_recording(
    app: &AppHandle,
    setup: RecordingSetup,
    consent_given: bool,
) -> Result<(), RecorderError> {
    let state = app.state::<AppState>();
    let recorder = state.recorder().ok_or_else(unavailable)?;
    let db = state.db()?;
    if db.load_settings()?.consent_reminder && !consent_given {
        return Err(RecorderError::ConsentRequired);
    }

    state.preview.stop();
    // Hide the app's own windows before the first frame can be captured, not after.
    let all_hidden = control_window::set_all_hidden_from_capture(app, true);
    if let Err(error) = recorder.start(&setup) {
        control_window::set_all_hidden_from_capture(app, false);
        emit_status(app);
        return Err(error);
    }
    if let Err(error) = db.save_last_setup(&setup) {
        warn!(%error, "the setup of this recording could not be remembered");
    }

    let control_hidden = match control_window::open(app) {
        Ok(hidden) => hidden,
        Err(error) => {
            warn!(%error, "the control window could not be opened");
            true // not open, so not in the video
        }
    };
    if setup.mode == crate::recording::metadata::Mode::ScreenAudio
        && !(all_hidden && control_hidden)
    {
        recorder.add_notice(
            WarningKind::OwnWindowVisible,
            "MeetingMind's own windows cannot be hidden from the recording on this computer, so they may appear in the video. Windows 10 version 2004 or newer is needed.",
        );
    }
    tray::update(app, recorder.phase());
    emit_status(app);
    Ok(())
}

pub fn pause_recording(app: &AppHandle) -> Result<(), RecorderError> {
    let recorder = recorder(app).ok_or_else(unavailable)?;
    let result = recorder.pause();
    tray::update(app, recorder.phase());
    emit_status(app);
    result
}

pub fn resume_recording(app: &AppHandle) -> Result<(), RecorderError> {
    let recorder = recorder(app).ok_or_else(unavailable)?;
    let result = recorder.resume();
    tray::update(app, recorder.phase());
    emit_status(app);
    result
}

/// Stops and saves the recording. The control window closes at once; the Record screen shows "saving" until done.
pub fn stop_recording(app: &AppHandle) -> Result<Finished, RecorderError> {
    let recorder = recorder(app).ok_or_else(unavailable)?;
    control_window::close(app);
    let result = recorder.stop();
    after_recording_ended(app, &recorder);
    if let Ok(finished) = &result
        && let Some(storage_folder) = Path::new(&finished.folder).parent()
    {
        allow_playback_from(app, storage_folder);
    }
    result
}

/// Throws the recording away. The UI has already asked the user to confirm.
pub fn cancel_recording(app: &AppHandle) -> Result<(), RecorderError> {
    let recorder = recorder(app).ok_or_else(unavailable)?;
    control_window::close(app);
    let result = recorder.cancel();
    after_recording_ended(app, &recorder);
    result
}

fn after_recording_ended(app: &AppHandle, recorder: &Recorder) {
    control_window::set_all_hidden_from_capture(app, false);
    tray::update(app, recorder.phase());
    emit_status(app);
}

fn unavailable() -> RecorderError {
    RecorderError::Device("the app did not start correctly, so recording is not available".into())
}

// ---------------------------------------------------------------------------------------------------------------
// Shortcuts and the tray menu

/// Start, or stop the recording that is running. Used by the shortcut and the tray menu.
pub fn toggle_start_stop(app: &AppHandle) {
    let app = app.clone();
    // Never block the event loop: starting takes a second or two and the dialogs wait for the user.
    std::thread::spawn(move || {
        let Some(recorder) = recorder(&app) else {
            return;
        };
        match recorder.phase() {
            Phase::Idle => start_like_the_record_screen(&app),
            Phase::Recording | Phase::Paused => {
                if let Err(error) = stop_recording(&app) {
                    tell(&app, MessageDialogKind::Error, &error.to_string());
                }
            }
            Phase::Starting | Phase::Finalizing => {}
        }
    });
}

/// Pause a recording, or resume a paused one.
pub fn toggle_pause(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(recorder) = recorder(&app) else {
            return;
        };
        let result = match recorder.phase() {
            Phase::Recording => pause_recording(&app),
            Phase::Paused => resume_recording(&app),
            _ => return,
        };
        if let Err(error) = result {
            tell(&app, MessageDialogKind::Error, &error.to_string());
        }
    });
}

fn start_like_the_record_screen(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(db) = state.db() else {
        return;
    };
    let setup = db.last_setup().ok().flatten().unwrap_or_default();
    let reminder = db
        .load_settings()
        .map(|s| s.consent_reminder)
        .unwrap_or(true);
    if reminder && !confirm(app, CONSENT_MESSAGE, "Start recording") {
        return;
    }
    if let Err(error) = start_recording(app, setup, true) {
        tell(app, MessageDialogKind::Error, &error.to_string());
    }
}

fn confirm(app: &AppHandle, message: &str, ok_label: &str) -> bool {
    app.dialog()
        .message(message)
        .title("MeetingMind")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            ok_label.to_string(),
            "Cancel".to_string(),
        ))
        .blocking_show()
}

/// A message box for something the user must know when no window of the app is in front of them.
fn tell(app: &AppHandle, kind: MessageDialogKind, message: &str) {
    app.dialog()
        .message(message)
        .title("MeetingMind")
        .kind(kind)
        .show(|_| {});
}

// ---------------------------------------------------------------------------------------------------------------
// The main window and leaving the app

/// Brings the main window to the front, showing it again if it was hidden while recording.
pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// True while a recording needs the app to stay alive (also when the last window is closed).
pub fn is_busy(app: &AppHandle) -> bool {
    recorder(app).is_some_and(|r| r.phase() != Phase::Idle)
}

/// Leaves the app. A recording that is running is stopped and saved first, so closing MeetingMind never loses one.
pub fn quit(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(recorder) = recorder(&app) {
            if matches!(recorder.phase(), Phase::Recording | Phase::Paused) {
                info!("saving the recording before quitting");
                if let Err(error) = stop_recording(&app) {
                    warn!(%error, "the recording could not be saved before quitting");
                }
            }
            // A recording that was already being saved: wait for it, but not forever. Saving a 2-hour recording took
            // 134 to 151 s on the development computer.
            for _ in 0..3000 {
                if recorder.phase() == Phase::Idle {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        app.exit(0);
    });
}

/// Lets the Record screen play recordings from the storage folder (the asset protocol serves only folders allowed here).
pub fn allow_playback_from(app: &AppHandle, storage_folder: &Path) {
    if let Err(error) = app
        .asset_protocol_scope()
        .allow_directory(storage_folder, true)
    {
        warn!(%error, "playback from the storage folder could not be allowed");
    }
}
