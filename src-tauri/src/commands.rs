//! Commands the UI can call. Thin wrappers: the logic lives in `storage` and `system`.

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;
use tracing::debug;

use crate::error::AppError;
use crate::logging::LogGuard;
use crate::storage::{Database, Settings};
use crate::system::{self, CapabilityReport};

/// Folders the app uses, resolved once at startup.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub db_path: PathBuf,
    /// Used when the user has not chosen a storage folder.
    pub default_storage_folder: PathBuf,
}

/// Everything set up during startup. If the database could not be opened, `db` is `None` and `startup_error`
/// explains why; the UI then shows that error instead of the normal screens.
pub struct AppState {
    pub product_name: String,
    pub paths: AppPaths,
    pub db: Option<Database>,
    pub startup_error: Option<String>,
    /// Problems that do not stop the app (for example, file logging could not start).
    pub startup_warnings: Vec<String>,
    /// Held so buffered log lines are flushed on exit. `None` if logging could not start.
    pub _log_guard: Option<LogGuard>,
}

impl AppState {
    fn db(&self) -> Result<&Database, AppError> {
        self.db.as_ref().ok_or_else(|| {
            AppError::Unavailable(
                self.startup_error
                    .clone()
                    .unwrap_or_else(|| "the database is not available".to_string()),
            )
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub product_name: String,
    pub version: String,
    pub data_dir: String,
    pub log_dir: String,
    pub db_path: String,
    pub default_storage_folder: String,
    pub schema_version: Option<u32>,
    pub startup_error: Option<String>,
    pub startup_warnings: Vec<String>,
}

#[tauri::command]
pub fn get_app_info(state: State<'_, AppState>, app: tauri::AppHandle) -> AppInfo {
    debug!(command = "get_app_info");
    AppInfo {
        product_name: state.product_name.clone(),
        version: app.package_info().version.to_string(),
        data_dir: state.paths.data_dir.display().to_string(),
        log_dir: state.paths.log_dir.display().to_string(),
        db_path: state.paths.db_path.display().to_string(),
        default_storage_folder: state.paths.default_storage_folder.display().to_string(),
        schema_version: state.db.as_ref().and_then(|db| db.schema_version().ok()),
        startup_error: state.startup_error.clone(),
        startup_warnings: state.startup_warnings.clone(),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, AppError> {
    debug!(command = "get_settings");
    state.db()?.load_settings()
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, AppError> {
    debug!(command = "update_settings");
    state.db()?.save_settings(&settings)
}

#[tauri::command]
pub fn get_capabilities() -> CapabilityReport {
    debug!(command = "get_capabilities");
    system::platform_probe().probe()
}
