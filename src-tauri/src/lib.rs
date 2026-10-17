//! MeetingMind backend.
//!
//! Module layout (SPEC Section 3):
//! - `system` (and, from Phase 2, `capture` and `audio`): the only modules that may use OS-specific APIs,
//!   always behind traits.
//! - `storage` (and, from Phase 3-4, `transcription` and `library`): identical on Windows and macOS.
//! - `commands`: the thin layer the UI calls. `logging` and `error` are shared plumbing.

pub mod audio;
pub mod capture;
mod commands;
pub mod error;
pub mod logging;
pub mod preview;
pub mod recorder;
pub mod recording;
pub mod shell;
pub mod storage;
pub mod system;

use std::sync::{Arc, Mutex};

use tauri::{Manager, RunEvent, WindowEvent};
use tracing::{error, info, warn};

use commands::{AppPaths, AppState};
use error::AppError;
use recorder::Recorder;
use storage::Database;

const DEFAULT_PRODUCT_NAME: &str = "MeetingMind";

fn resolve_paths(app: &tauri::AppHandle) -> Result<AppPaths, AppError> {
    let path = app.path();
    let data_dir = path
        .app_data_dir()
        .map_err(|e| AppError::Paths(e.to_string()))?;
    // The Downloads folder, not Documents: on many computers Documents is moved into OneDrive, which would upload every
    // recording. The user can choose any other folder in Settings.
    let downloads_dir = path
        .download_dir()
        .map_err(|e| AppError::Paths(e.to_string()))?;
    Ok(AppPaths {
        log_dir: data_dir.join("logs"),
        db_path: data_dir.join("meetingmind.db"),
        default_storage_folder: downloads_dir.join("MeetingMind"),
        data_dir,
    })
}

fn build_state(app: &tauri::AppHandle) -> Result<AppState, AppError> {
    let paths = resolve_paths(app)?;
    std::fs::create_dir_all(&paths.data_dir)?;

    let product_name = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| DEFAULT_PRODUCT_NAME.to_string());

    let mut startup_warnings = Vec::new();
    let log_guard = match logging::init(&paths.log_dir) {
        Ok(guard) => Some(guard),
        Err(err) => {
            startup_warnings.push(format!(
                "File logging is not running, so problems cannot be diagnosed from log files: {err}"
            ));
            None
        }
    };
    info!(version = %app.package_info().version, "starting");

    // A database that cannot be opened must not close the app silently: keep running so the UI can say why.
    let (db, startup_error) = match Database::open(&paths.db_path) {
        Ok(db) => (Some(Arc::new(db)), None),
        Err(err) => {
            error!(error = %err, "database could not be opened");
            (None, Some(err.to_string()))
        }
    };
    if let Some(db) = &db {
        match db.schema_version() {
            Ok(version) => info!(schema_version = version, "database ready"),
            Err(err) => warn!(error = %err, "could not read the database schema version"),
        }
    }

    let recorder = db.as_ref().map(|db| {
        Arc::new(Recorder::for_this_computer(
            db.clone(),
            paths.default_storage_folder.clone(),
        ))
    });

    Ok(AppState {
        product_name,
        paths,
        db,
        recorder,
        preview: preview::Preview::default(),
        startup_error,
        startup_warnings: Mutex::new(startup_warnings),
        _log_guard: log_guard,
    })
}

/// Everything that happens once the app exists: share the state, then install the tray icon, the shortcuts and the
/// status feed. Problems with those are shown to the user, they do not stop the app.
fn set_up(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let state = build_state(app.handle())?;
    let hotkeys = state
        .db
        .as_ref()
        .and_then(|db| db.load_settings().ok())
        .map(|settings| settings.hotkeys)
        .unwrap_or_default();
    let storage_folder = state
        .recorder
        .as_ref()
        .and_then(|recorder| recorder.storage_folder().ok());
    app.manage(state);

    let problems = shell::install(app.handle(), &hotkeys);
    let state = app.state::<AppState>();
    for problem in problems {
        state.add_startup_warning(problem);
    }
    if let Some(folder) = storage_folder.filter(|folder| folder.is_dir()) {
        shell::allow_playback_from(app.handle(), &folder);
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        // Must come first: a second copy of the app hands over to this one instead of starting (two copies could
        // both record, or fight over the database).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            shell::show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| set_up(app))
        .on_window_event(|window, event| {
            // While a recording runs, closing the main window only hides it: the control window and the tray icon stay.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
                && shell::is_busy(window.app_handle())
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_settings,
            commands::update_settings,
            commands::get_capabilities,
            commands::list_devices,
            commands::estimate_recording,
            commands::storage_sync_provider,
            commands::start_preview,
            commands::stop_preview,
            commands::get_preview_frame,
            commands::recording_status,
            commands::start_recording,
            commands::pause_recording,
            commands::resume_recording,
            commands::stop_recording,
            commands::cancel_recording,
            commands::dismiss_recording_result,
            commands::list_presets,
            commands::save_preset,
            commands::delete_preset,
            commands::get_last_setup,
            commands::list_recent_recordings,
            commands::open_folder,
            commands::list_unfinished,
            commands::recover_recording,
            commands::discard_recording,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the MeetingMind application")
        .run(|app, event| {
            // The last window closed (or the user quit) while a recording was running: save it first, then leave.
            if let RunEvent::ExitRequested { api, code, .. } = event
                && code.is_none()
                && shell::is_busy(app)
            {
                api.prevent_exit();
                shell::quit(app);
            }
        });
}
