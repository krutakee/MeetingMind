//! Commands the UI can call. Thin wrappers: the logic lives in `storage`, `system`, `recorder`, `preview` and `shell`.
//!
//! Commands that open devices, run FFmpeg or wait for the disk are `async` (they run on a thread pool): a plain
//! command runs on the thread that draws the window and would freeze it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, State};
use tracing::debug;

use crate::audio::AudioDeviceInfo;
use crate::capture::MonitorInfo;
use crate::error::AppError;
use crate::logging::LogGuard;
use crate::preview::Preview;
use crate::recorder::{Finished, Recorder, RecorderError, Status};
use crate::recording::recovery;
use crate::recording::setup::RecordingSetup;
use crate::shell;
use crate::storage::meetings;
use crate::storage::presets::{self, Preset};
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
    pub db: Option<Arc<Database>>,
    /// `None` when the database could not be opened: nothing can be recorded then.
    pub recorder: Option<Arc<Recorder>>,
    pub preview: Preview,
    pub startup_error: Option<String>,
    /// Problems that do not stop the app (file logging could not start, a shortcut is taken, ...).
    pub startup_warnings: Mutex<Vec<String>>,
    /// Held so buffered log lines are flushed on exit. `None` if logging could not start.
    pub _log_guard: Option<LogGuard>,
}

impl AppState {
    pub fn db(&self) -> Result<&Database, AppError> {
        self.db.as_deref().ok_or_else(|| {
            AppError::Unavailable(
                self.startup_error
                    .clone()
                    .unwrap_or_else(|| "the database is not available".to_string()),
            )
        })
    }

    pub fn recorder(&self) -> Option<Arc<Recorder>> {
        self.recorder.clone()
    }

    fn recorder_or_error(&self) -> Result<Arc<Recorder>, AppError> {
        self.recorder().ok_or_else(|| {
            AppError::Unavailable(
                "recording is not available: the app did not start correctly".into(),
            )
        })
    }

    pub fn add_startup_warning(&self, message: String) {
        self.startup_warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(message);
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
pub fn get_app_info(state: State<'_, AppState>, app: AppHandle) -> AppInfo {
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
        startup_warnings: state
            .startup_warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, AppError> {
    debug!(command = "get_settings");
    state.db()?.load_settings()
}

/// Saves the settings. New shortcuts are registered first, so a combination that another program already owns is
/// refused with a clear message and nothing changes.
#[tauri::command(async)]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, AppError> {
    debug!(command = "update_settings");
    let db = state.db()?;
    let previous = db.load_settings()?;
    let hotkeys_changed = settings.hotkeys != previous.hotkeys;
    if hotkeys_changed && let Err(reason) = shell::apply_hotkeys(&app, &settings.hotkeys) {
        let _ = shell::apply_hotkeys(&app, &previous.hotkeys);
        return Err(AppError::InvalidSetting {
            field: "hotkeys",
            reason,
        });
    }
    let saved = match db.save_settings(&settings) {
        Ok(saved) => saved,
        Err(error) => {
            if hotkeys_changed {
                let _ = shell::apply_hotkeys(&app, &previous.hotkeys);
            }
            return Err(error);
        }
    };
    if let Some(folder) = &saved.storage_folder
        && Path::new(folder).is_dir()
    {
        shell::allow_playback_from(&app, Path::new(folder));
    }
    Ok(saved)
}

#[tauri::command(async)]
pub fn get_capabilities(state: State<'_, AppState>) -> CapabilityReport {
    debug!(command = "get_capabilities");
    let mut report = system::platform_probe().probe();
    if let Some(recorder) = state.recorder() {
        system::replace_capability(&mut report, recorder.video_encoder_capability());
    }
    report
}

// ---------------------------------------------------------------------------------------------------------------
// Devices, estimate and live preview

/// The monitors and microphones to choose from. Each list can fail on its own; the other is still returned.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLists {
    pub monitors: Vec<MonitorInfo>,
    pub monitors_problem: Option<String>,
    pub microphones: Vec<AudioDeviceInfo>,
    pub microphones_problem: Option<String>,
}

#[tauri::command(async)]
pub fn list_devices(state: State<'_, AppState>) -> Result<DeviceLists, AppError> {
    debug!(command = "list_devices");
    let recorder = state.recorder_or_error()?;
    let (monitors, monitors_problem) = match recorder.devices().monitors() {
        Ok(monitors) => (monitors, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let (microphones, microphones_problem) = match recorder.devices().microphones() {
        Ok(microphones) => (microphones, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    Ok(DeviceLists {
        monitors,
        monitors_problem,
        microphones,
        microphones_problem,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    /// The most disk space one hour can take with these choices.
    pub max_bytes_per_hour: u64,
    /// The size of the video in the file; absent for an audio-only recording.
    pub video_width: Option<u32>,
    pub video_height: Option<u32>,
    pub free_bytes: Option<u64>,
    pub storage_folder: String,
}

/// What the choices will cost, and how much room the storage folder has.
#[tauri::command(async)]
pub fn estimate_recording(
    state: State<'_, AppState>,
    setup: RecordingSetup,
) -> Result<Estimate, AppError> {
    let recorder = state.recorder_or_error()?;
    let monitor = recorder
        .devices()
        .monitors()
        .ok()
        .and_then(|monitors| match setup.monitor_id.as_deref() {
            Some(id) => monitors.into_iter().find(|m| m.id == id),
            None => {
                let primary = monitors.iter().position(|m| m.is_primary).unwrap_or(0);
                monitors.into_iter().nth(primary)
            }
        })
        .map(|m| (m.width, m.height))
        // Without a monitor to measure, a full HD screen is assumed, which the screen says is only an estimate.
        .unwrap_or((1920, 1080));
    let plan = setup.video_plan(monitor);
    Ok(Estimate {
        max_bytes_per_hour: setup.max_bytes_per_hour(monitor),
        video_width: plan.map(|p| p.width),
        video_height: plan.map(|p| p.height),
        free_bytes: recorder.free_disk_bytes(),
        storage_folder: recorder.storage_folder()?.display().to_string(),
    })
}

#[tauri::command(async)]
pub fn start_preview(
    state: State<'_, AppState>,
    monitor_id: Option<String>,
) -> Result<(), AppError> {
    let recorder = state.recorder_or_error()?;
    state
        .preview
        .start(recorder.devices(), monitor_id.as_deref())
        .map_err(AppError::Unavailable)
}

#[tauri::command]
pub fn stop_preview(state: State<'_, AppState>) {
    state.preview.stop();
}

/// The newest preview picture if it is newer than `since`: 8 bytes of sequence number, 4 of width, 4 of height (all
/// little-endian), then RGBA pixels. Empty when there is nothing newer.
#[tauri::command]
pub fn get_preview_frame(state: State<'_, AppState>, since: u64) -> tauri::ipc::Response {
    let bytes = match state.preview.latest_after(since) {
        Some(frame) => {
            let mut bytes = Vec::with_capacity(16 + frame.rgba.len());
            bytes.extend_from_slice(&frame.sequence.to_le_bytes());
            bytes.extend_from_slice(&frame.width.to_le_bytes());
            bytes.extend_from_slice(&frame.height.to_le_bytes());
            bytes.extend_from_slice(&frame.rgba);
            bytes
        }
        None => Vec::new(),
    };
    tauri::ipc::Response::new(bytes)
}

// ---------------------------------------------------------------------------------------------------------------
// Recording

#[tauri::command]
pub fn recording_status(state: State<'_, AppState>) -> Result<Status, AppError> {
    Ok(state.recorder_or_error()?.status())
}

/// `consent_given` is true when the user has just confirmed the consent reminder in the UI.
#[tauri::command(async)]
pub fn start_recording(
    app: AppHandle,
    setup: RecordingSetup,
    consent_given: bool,
) -> Result<(), RecorderError> {
    shell::start_recording(&app, setup, consent_given)
}

#[tauri::command(async)]
pub fn pause_recording(app: AppHandle) -> Result<(), RecorderError> {
    shell::pause_recording(&app)
}

#[tauri::command(async)]
pub fn resume_recording(app: AppHandle) -> Result<(), RecorderError> {
    shell::resume_recording(&app)
}

#[tauri::command(async)]
pub fn stop_recording(app: AppHandle) -> Result<Finished, RecorderError> {
    shell::stop_recording(&app)
}

#[tauri::command(async)]
pub fn cancel_recording(app: AppHandle) -> Result<(), RecorderError> {
    shell::cancel_recording(&app)
}

/// The Record screen has shown "recording saved": forget it.
#[tauri::command]
pub fn dismiss_recording_result(state: State<'_, AppState>) {
    if let Some(recorder) = state.recorder() {
        recorder.dismiss_result();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Presets and the setup of the latest recording

#[tauri::command]
pub fn list_presets(state: State<'_, AppState>) -> Result<Vec<Preset>, AppError> {
    state.db()?.with_conn(|conn| presets::list(conn))
}

#[tauri::command]
pub fn save_preset(
    state: State<'_, AppState>,
    name: String,
    setup: RecordingSetup,
) -> Result<Preset, AppError> {
    state
        .db()?
        .with_conn(|conn| presets::save(conn, &name, &setup))
}

#[tauri::command]
pub fn delete_preset(state: State<'_, AppState>, id: i64) -> Result<(), AppError> {
    state.db()?.with_conn(|conn| presets::delete(conn, id))
}

/// The setup of the latest recording, for the Record screen to start from.
#[tauri::command]
pub fn get_last_setup(state: State<'_, AppState>) -> Result<Option<RecordingSetup>, AppError> {
    state.db()?.last_setup()
}

// ---------------------------------------------------------------------------------------------------------------
// Recent recordings

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRecording {
    pub id: i64,
    pub title: String,
    pub started_at: String,
    pub duration_ms: Option<i64>,
    pub folder: String,
    /// The recording file, or `None` if it is no longer there (moved or deleted by the user).
    pub file: Option<String>,
    pub has_video: bool,
}

fn recording_file(folder: &Path) -> Option<(PathBuf, bool)> {
    [("recording.mp4", true), ("recording.m4a", false)]
        .into_iter()
        .map(|(name, video)| (folder.join(name), video))
        .find(|(path, _)| path.is_file())
}

#[tauri::command]
pub fn list_recent_recordings(
    state: State<'_, AppState>,
    limit: u32,
) -> Result<Vec<RecentRecording>, AppError> {
    let rows = state
        .db()?
        .with_conn(|conn| meetings::recent_complete(conn, limit.min(50)))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let found = recording_file(Path::new(&row.folder_path));
            RecentRecording {
                id: row.id,
                title: row.title,
                started_at: row.started_at,
                duration_ms: row.duration_ms,
                folder: row.folder_path,
                has_video: found.as_ref().is_some_and(|(_, video)| *video),
                file: found.map(|(path, _)| path.to_string_lossy().into_owned()),
            }
        })
        .collect())
}

/// Opens a meeting folder in the file manager. Only folders inside the storage folder are opened.
#[tauri::command(async)]
pub fn open_folder(state: State<'_, AppState>, path: String) -> Result<(), AppError> {
    let storage = state.recorder_or_error()?.storage_folder()?;
    let refuse =
        || AppError::Refused("only folders inside the storage folder can be opened".into());
    let folder = std::fs::canonicalize(&path).map_err(|_| refuse())?;
    let storage = std::fs::canonicalize(storage).map_err(|_| refuse())?;
    if !folder.starts_with(&storage) || !folder.is_dir() {
        return Err(refuse());
    }
    tauri_plugin_opener::open_path(folder.to_string_lossy().as_ref(), None::<&str>)
        .map_err(|e| AppError::Unavailable(format!("the folder could not be opened: {e}")))
}

// ---------------------------------------------------------------------------------------------------------------
// Recovery of recordings that were not finished (the app or the computer stopped)

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unfinished {
    pub id: i64,
    pub title: String,
    pub started_at: String,
    pub folder: String,
}

/// Recordings the app found unfinished. Empty while a recording is running or being saved, because that one is not
/// unfinished, only busy.
#[tauri::command]
pub fn list_unfinished(state: State<'_, AppState>) -> Result<Vec<Unfinished>, AppError> {
    let recorder = state.recorder_or_error()?;
    if recorder.phase() != crate::recorder::Phase::Idle {
        return Ok(Vec::new());
    }
    Ok(recovery::unfinished(state.db()?)?
        .into_iter()
        .map(|row| Unfinished {
            id: row.id,
            title: row.title,
            started_at: row.started_at,
            folder: row.folder_path,
        })
        .collect())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredRecording {
    pub meeting_id: i64,
    pub folder: String,
    pub file: String,
    pub duration_ms: u64,
}

fn unfinished_row(state: &AppState, id: i64) -> Result<meetings::MeetingRow, AppError> {
    recovery::unfinished(state.db()?)?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(|| AppError::Refused("that recording is not waiting for recovery".into()))
}

fn recovery_error(error: recovery::RecoveryError) -> AppError {
    AppError::Unavailable(error.to_string())
}

#[tauri::command(async)]
pub fn recover_recording(
    state: State<'_, AppState>,
    id: i64,
) -> Result<RecoveredRecording, AppError> {
    let recorder = state.recorder_or_error()?;
    let row = unfinished_row(&state, id)?;
    let tools = recorder.tools().map_err(AppError::Unavailable)?;
    let recovered =
        recovery::recover(&tools.ffmpeg.path, state.db()?, &row).map_err(recovery_error)?;
    Ok(RecoveredRecording {
        meeting_id: recovered.meeting_id,
        folder: recovered.folder.display().to_string(),
        file: recovered.file.display().to_string(),
        duration_ms: u64::try_from(recovered.duration.as_millis()).unwrap_or(u64::MAX),
    })
}

#[tauri::command(async)]
pub fn discard_recording(state: State<'_, AppState>, id: i64) -> Result<(), AppError> {
    let recorder = state.recorder_or_error()?;
    let row = unfinished_row(&state, id)?;
    recovery::discard(state.db()?, &row, &recorder.storage_folder()?).map_err(recovery_error)
}
