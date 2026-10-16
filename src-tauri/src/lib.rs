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
pub mod recording;
pub mod storage;
pub mod system;

use tauri::Manager;
use tracing::{error, info, warn};

use commands::{AppPaths, AppState};
use error::AppError;
use storage::Database;

const DEFAULT_PRODUCT_NAME: &str = "MeetingMind";

fn resolve_paths(app: &tauri::AppHandle) -> Result<AppPaths, AppError> {
    let path = app.path();
    let data_dir = path
        .app_data_dir()
        .map_err(|e| AppError::Paths(e.to_string()))?;
    let documents_dir = path
        .document_dir()
        .map_err(|e| AppError::Paths(e.to_string()))?;
    Ok(AppPaths {
        log_dir: data_dir.join("logs"),
        db_path: data_dir.join("meetingmind.db"),
        default_storage_folder: documents_dir.join("MeetingMind").join("Meetings"),
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
        Ok(db) => (Some(db), None),
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

    Ok(AppState {
        product_name,
        paths,
        db,
        startup_error,
        startup_warnings,
        _log_guard: log_guard,
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = build_state(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_settings,
            commands::update_settings,
            commands::get_capabilities,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the MeetingMind application");
}
