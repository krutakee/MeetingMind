//! The recorder: the one recording that may be running, and everything the UI needs to drive it.
//!
//! Free of any UI framework and of OS APIs. Devices come in through [`Devices`] and FFmpeg through [`Tools`], so the
//! whole life of a recording (start, pause, resume, stop, stopping by itself when the disk is full) is tested with
//! synthetic sources and the real FFmpeg, and the Windows and macOS sources plug in the same way.
//!
//! Anything the user asked for that cannot be opened (a monitor that is gone, a microphone that was unplugged) stops
//! the recording from starting, with a message saying which one. MeetingMind never quietly records fewer tracks than
//! were chosen. What goes wrong after the recording has started is the session's business: it keeps recording and
//! raises warnings.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use tracing::{info, warn};

use crate::audio::{self, AudioDeviceInfo, AudioError, AudioSource};
use crate::capture::{self, CaptureError, MonitorInfo, ScreenSource};
use crate::error::AppError;
use crate::recording::clock::SystemClock;
use crate::recording::ffmpeg::{self, FfmpegInfo, H264Kind};
use crate::recording::metadata::Mode;
use crate::recording::session::{
    Session, SessionConfig, SessionDeps, SessionError, SessionOutcome, SessionSources, StopReason,
    Warning,
};
use crate::recording::setup::RecordingSetup;
use crate::storage::Database;
use crate::system::disk::DiskSpace;
use crate::system::{self, PowerManager};

/// The title used when the user does not type one.
pub const DEFAULT_TITLE: &str = "Meeting";

// ---------------------------------------------------------------------------------------------------------------
// Devices and tools

/// Where screens and audio devices come from. The real one asks the operating system; tests supply synthetic ones.
pub trait Devices: Send + Sync {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    fn screen(
        &self,
        monitor_id: Option<&str>,
        max_fps: u32,
    ) -> Result<Box<dyn ScreenSource>, CaptureError>;
    fn microphones(&self) -> Result<Vec<AudioDeviceInfo>, AudioError>;
    fn microphone(&self, device_id: Option<&str>) -> Result<Box<dyn AudioSource>, AudioError>;
    fn system_audio(&self) -> Result<Box<dyn AudioSource>, AudioError>;
}

/// The real devices of this computer.
pub struct SystemDevices;

impl Devices for SystemDevices {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        capture::list_monitors()
    }

    fn screen(
        &self,
        monitor_id: Option<&str>,
        max_fps: u32,
    ) -> Result<Box<dyn ScreenSource>, CaptureError> {
        capture::open_screen(monitor_id, max_fps)
    }

    fn microphones(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        audio::list_microphones()
    }

    fn microphone(&self, device_id: Option<&str>) -> Result<Box<dyn AudioSource>, AudioError> {
        audio::open_microphone(device_id)
    }

    fn system_audio(&self) -> Result<Box<dyn AudioSource>, AudioError> {
        audio::open_system_audio()
    }
}

/// FFmpeg, found and checked, and the H.264 encoder that works on this computer.
#[derive(Clone)]
pub struct Tools {
    pub ffmpeg: FfmpegInfo,
    /// `Err` says why no H.264 encoder works. Audio-only recordings do not need one.
    pub h264: Result<H264Kind, String>,
}

impl Tools {
    /// Finds the bundled FFmpeg, applies the license check, and tries the H.264 encoders in order of preference.
    /// Slow (a few seconds): the recorder does it once, on first use.
    pub fn detect() -> Result<Tools, String> {
        let path = ffmpeg::locate().map_err(|e| e.to_string())?;
        let ffmpeg = ffmpeg::probe(&path).map_err(|e| e.to_string())?;
        let h264 = ffmpeg::select_h264(&ffmpeg).map_err(|e| e.to_string());
        Ok(Tools { ffmpeg, h264 })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Errors and what the UI is told

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("a recording is already running")]
    Busy,
    #[error("no recording is running")]
    NotRecording,
    #[error("{0}")]
    InvalidSetup(&'static str),
    #[error("recording is not possible: {0}")]
    Tools(String),
    #[error("{0}")]
    Device(String),
    #[error("{0}")]
    Storage(#[from] AppError),
    #[error("{0}")]
    Session(#[from] SessionError),
}

impl RecorderError {
    fn kind(&self) -> &'static str {
        match self {
            RecorderError::Busy => "busy",
            RecorderError::NotRecording => "notRecording",
            RecorderError::InvalidSetup(_) => "invalidSetup",
            RecorderError::Tools(_) => "tools",
            RecorderError::Device(_) => "device",
            RecorderError::Storage(_) => "storage",
            RecorderError::Session(_) => "session",
        }
    }
}

/// Serialized for the frontend as `{ "kind": "...", "message": "..." }`, like every backend error.
impl Serialize for RecorderError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("RecorderError", 2)?;
        state.serialize_field("kind", self.kind())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    /// Devices are being opened.
    Starting,
    Recording,
    Paused,
    /// Capture has ended and the tracks are being put together.
    Finalizing,
}

/// A recording that ended and was saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finished {
    pub meeting_id: Option<i64>,
    pub title: String,
    pub folder: String,
    /// `recording.mp4` or `recording.m4a`.
    pub file: String,
    pub duration_ms: u64,
    pub has_video: bool,
}

/// Everything the Record screen, the control window and the tray icon show. Sent to the UI about four times a
/// second while a recording is active.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    pub title: Option<String>,
    pub mode: Option<Mode>,
    pub elapsed_ms: u64,
    /// Peak level of the latest audio, 0.0 to 1.0.
    pub microphone_level: f32,
    pub system_audio_level: f32,
    pub warnings: Vec<Warning>,
    /// Set when the recording has asked to be stopped (the disk is almost full).
    pub auto_stop: Option<StopReason>,
    /// The latest recording that ended and was saved, until the next one starts.
    pub finished: Option<Finished>,
    /// Why the latest recording could not be finished into one file, until the next one starts.
    pub error: Option<String>,
}

impl Status {
    fn idle(last: &Last) -> Self {
        Self {
            phase: Phase::Idle,
            title: None,
            mode: None,
            elapsed_ms: 0,
            microphone_level: 0.0,
            system_audio_level: 0.0,
            warnings: Vec::new(),
            auto_stop: None,
            finished: last.finished.clone(),
            error: last.error.clone(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------

struct Active {
    session: Session,
    title: String,
    mode: Mode,
}

enum Slot {
    Idle,
    Starting {
        title: String,
        mode: Mode,
    },
    Active(Box<Active>),
    Finalizing {
        title: String,
        mode: Mode,
        elapsed_ms: u64,
    },
}

#[derive(Default)]
struct Last {
    finished: Option<Finished>,
    error: Option<String>,
}

struct State {
    slot: Slot,
    last: Last,
}

pub struct Recorder {
    devices: Arc<dyn Devices>,
    detect_tools: Box<dyn Fn() -> Result<Tools, String> + Send + Sync>,
    tools: Mutex<Option<Tools>>,
    database: Arc<Database>,
    disk: Arc<dyn DiskSpace>,
    power: Option<Arc<dyn PowerManager>>,
    default_storage_folder: PathBuf,
    state: Mutex<State>,
}

impl Recorder {
    pub fn new(
        devices: Arc<dyn Devices>,
        detect_tools: impl Fn() -> Result<Tools, String> + Send + Sync + 'static,
        database: Arc<Database>,
        disk: Arc<dyn DiskSpace>,
        power: Option<Arc<dyn PowerManager>>,
        default_storage_folder: PathBuf,
    ) -> Self {
        Self {
            devices,
            detect_tools: Box::new(detect_tools),
            tools: Mutex::new(None),
            database,
            disk,
            power,
            default_storage_folder,
            state: Mutex::new(State {
                slot: Slot::Idle,
                last: Last::default(),
            }),
        }
    }

    /// The recorder for this computer: real devices, the bundled FFmpeg, real disk and power management.
    pub fn for_this_computer(database: Arc<Database>, default_storage_folder: PathBuf) -> Self {
        Self::new(
            Arc::new(SystemDevices),
            Tools::detect,
            database,
            Arc::new(system::disk::SystemDiskSpace),
            Some(Arc::from(system::platform_power())),
            default_storage_folder,
        )
    }

    pub fn devices(&self) -> &dyn Devices {
        self.devices.as_ref()
    }

    /// FFmpeg and the H.264 encoder, found on first use and remembered once found. A failure is not remembered, so
    /// fixing the problem does not need a restart.
    pub fn tools(&self) -> Result<Tools, String> {
        let mut cached = self.tools.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tools) = cached.as_ref() {
            return Ok(tools.clone());
        }
        let found = (self.detect_tools)()?;
        *cached = Some(found.clone());
        Ok(found)
    }

    /// The folder meetings are saved in: the one chosen in Settings, or the default.
    pub fn storage_folder(&self) -> Result<PathBuf, AppError> {
        Ok(self
            .database
            .load_settings()?
            .storage_folder
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_storage_folder.clone()))
    }

    pub fn free_disk_bytes(&self) -> Option<u64> {
        let folder = self.storage_folder().ok()?;
        self.disk.available_bytes(&folder).ok()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn phase(&self) -> Phase {
        match &self.lock().slot {
            Slot::Idle => Phase::Idle,
            Slot::Starting { .. } => Phase::Starting,
            Slot::Active(active) => phase_of(&active.session),
            Slot::Finalizing { .. } => Phase::Finalizing,
        }
    }

    pub fn status(&self) -> Status {
        let state = self.lock();
        match &state.slot {
            Slot::Idle => Status::idle(&state.last),
            Slot::Starting { title, mode } => Status {
                phase: Phase::Starting,
                title: Some(title.clone()),
                mode: Some(*mode),
                ..Status::idle(&Last::default())
            },
            Slot::Active(active) => {
                let snapshot = active.session.snapshot();
                Status {
                    phase: phase_of(&active.session),
                    title: Some(active.title.clone()),
                    mode: Some(active.mode),
                    elapsed_ms: snapshot.elapsed_ms,
                    microphone_level: snapshot.microphone_level,
                    system_audio_level: snapshot.system_audio_level,
                    warnings: snapshot.warnings,
                    auto_stop: snapshot.auto_stop,
                    finished: None,
                    error: None,
                }
            }
            Slot::Finalizing {
                title,
                mode,
                elapsed_ms,
            } => Status {
                phase: Phase::Finalizing,
                title: Some(title.clone()),
                mode: Some(*mode),
                elapsed_ms: *elapsed_ms,
                ..Status::idle(&Last::default())
            },
        }
    }

    /// Opens the devices and starts recording. Takes a second or two, so call it off the UI thread.
    pub fn start(&self, setup: &RecordingSetup) -> Result<(), RecorderError> {
        let title = match setup.title.trim() {
            "" => DEFAULT_TITLE.to_string(),
            title => title.to_string(),
        };
        {
            let mut state = self.lock();
            if !matches!(state.slot, Slot::Idle) {
                return Err(RecorderError::Busy);
            }
            if let Some(problem) = setup.problem() {
                return Err(RecorderError::InvalidSetup(problem));
            }
            state.slot = Slot::Starting {
                title: title.clone(),
                mode: setup.mode,
            };
            state.last = Last::default();
        }

        match self.open_and_start(setup, &title) {
            Ok(session) => {
                self.lock().slot = Slot::Active(Box::new(Active {
                    session,
                    title,
                    mode: setup.mode,
                }));
                Ok(())
            }
            Err(error) => {
                warn!(%error, "a recording could not be started");
                self.lock().slot = Slot::Idle;
                Err(error)
            }
        }
    }

    fn open_and_start(
        &self,
        setup: &RecordingSetup,
        title: &str,
    ) -> Result<Session, RecorderError> {
        let tools = self.tools().map_err(RecorderError::Tools)?;
        let storage_folder = self.storage_folder()?;

        let mut sources = SessionSources::default();
        let mut config =
            SessionConfig::new(title, setup.mode, &storage_folder, system::local_now());
        config.fps = setup.fps;
        let mut h264 = H264Kind::HardwareMf; // unused when no video is recorded

        if setup.mode == Mode::ScreenAudio {
            h264 = tools.h264.clone().map_err(|e| {
                RecorderError::Tools(format!(
                    "no H.264 video encoder works on this computer ({e})"
                ))
            })?;
            let monitor = self.chosen_monitor(setup.monitor_id.as_deref())?;
            let screen = self
                .devices
                .screen(Some(&monitor.id), setup.fps)
                .map_err(|e| RecorderError::Device(e.to_string()))?;
            sources = sources.with_boxed_screen(screen);
            let plan = setup
                .video_plan((monitor.width, monitor.height))
                .expect("a screen recording has a video plan");
            config.video_bitrate_bps = plan.bitrate_bps;
            config.max_output_height = setup.quality.max_height();
        }
        if setup.microphone {
            let microphone = self
                .devices
                .microphone(setup.microphone_id.as_deref())
                .map_err(|e| {
                    RecorderError::Device(format!("The microphone cannot be used: {e}"))
                })?;
            sources = sources.with_boxed_microphone(microphone);
        }
        if setup.system_audio {
            let speakers = self.devices.system_audio().map_err(|e| {
                RecorderError::Device(format!("System audio cannot be recorded: {e}"))
            })?;
            sources = sources.with_boxed_system_audio(speakers);
        }

        let deps = SessionDeps {
            clock: Arc::new(SystemClock::new()),
            ffmpeg: tools.ffmpeg,
            video_encoder: h264,
            disk: self.disk.clone(),
            database: Some(self.database.clone()),
            power: self.power.clone(),
        };
        Ok(Session::start(config, sources, deps)?)
    }

    /// The monitor to record: the one asked for, or the primary one.
    fn chosen_monitor(&self, wanted: Option<&str>) -> Result<MonitorInfo, RecorderError> {
        let monitors = self
            .devices
            .monitors()
            .map_err(|e| RecorderError::Device(e.to_string()))?;
        let found = match wanted {
            Some(id) => monitors.iter().find(|m| m.id == id).cloned(),
            None => monitors
                .iter()
                .find(|m| m.is_primary)
                .or(monitors.first())
                .cloned(),
        };
        found.ok_or_else(|| {
            RecorderError::Device(match wanted {
                Some(_) => "The chosen monitor is not connected. Choose another monitor.".into(),
                None => "No monitor was found.".into(),
            })
        })
    }

    pub fn pause(&self) -> Result<(), RecorderError> {
        let mut state = self.lock();
        match &mut state.slot {
            Slot::Active(active) => Ok(active.session.pause()?),
            _ => Err(RecorderError::NotRecording),
        }
    }

    pub fn resume(&self) -> Result<(), RecorderError> {
        let mut state = self.lock();
        match &mut state.slot {
            Slot::Active(active) => Ok(active.session.resume()?),
            _ => Err(RecorderError::NotRecording),
        }
    }

    /// Stops and saves the recording. Takes a while for a long recording (the tracks are put together), so call it
    /// off the UI thread; `status` reports `Finalizing` meanwhile.
    pub fn stop(&self) -> Result<Finished, RecorderError> {
        let active = {
            let mut state = self.lock();
            match std::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active(active) => {
                    state.slot = Slot::Finalizing {
                        title: active.title.clone(),
                        mode: active.mode,
                        elapsed_ms: active.session.snapshot().elapsed_ms,
                    };
                    active
                }
                other => {
                    state.slot = other;
                    return Err(RecorderError::NotRecording);
                }
            }
        };

        let Active { session, title, .. } = *active;
        let outcome = session.stop();

        let mut state = self.lock();
        state.slot = Slot::Idle;
        match outcome {
            Ok(outcome) => {
                let finished = finished_from(title, &outcome);
                info!(duration_ms = finished.duration_ms, "recording saved");
                state.last = Last {
                    finished: Some(finished.clone()),
                    error: None,
                };
                Ok(finished)
            }
            Err(error) => {
                warn!(%error, "a recording could not be finished");
                state.last = Last {
                    finished: None,
                    error: Some(error.to_string()),
                };
                Err(error.into())
            }
        }
    }

    /// Stops the recording if it has asked to be stopped (the disk is almost full). Returns what `stop` returned, or
    /// `None` if nothing was asked. The app calls this regularly.
    pub fn stop_if_asked(&self) -> Option<Result<Finished, RecorderError>> {
        let asked = match &self.lock().slot {
            Slot::Active(active) => active.session.snapshot().auto_stop.is_some(),
            _ => false,
        };
        asked.then(|| self.stop())
    }

    /// Forgets the "recording saved" banner (the UI has shown it).
    pub fn dismiss_result(&self) {
        self.lock().last = Last::default();
    }

    /// The id of the meeting being recorded or finalized, if any: it is not an unfinished recording to recover.
    pub fn active_meeting_id(&self) -> Option<i64> {
        match &self.lock().slot {
            Slot::Active(active) => active.session.meeting_id(),
            _ => None,
        }
    }
}

fn phase_of(session: &Session) -> Phase {
    use crate::recording::state::RecordingState as S;
    match session.state() {
        S::Paused => Phase::Paused,
        S::Stopping | S::Finalizing => Phase::Finalizing,
        S::Idle | S::Starting => Phase::Starting,
        S::Recording | S::Complete | S::Failed | S::Cancelled => Phase::Recording,
    }
}

fn finished_from(title: String, outcome: &SessionOutcome) -> Finished {
    Finished {
        meeting_id: outcome.meeting_id,
        title,
        folder: outcome.folder.to_string_lossy().into_owned(),
        file: outcome.file.to_string_lossy().into_owned(),
        duration_ms: u64::try_from(outcome.duration.as_millis()).unwrap_or(u64::MAX),
        has_video: outcome.file.extension().is_some_and(|e| e == "mp4"),
    }
}
