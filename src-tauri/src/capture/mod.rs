//! Screen capture. The trait is the same everywhere; the implementation behind it is OS-specific
//! (Windows.Graphics.Capture on Windows from M2, ScreenCaptureKit on macOS in Phase 5).

use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use crate::recording::clock::MonotonicClock;

#[cfg(windows)]
pub mod windows;

/// Sends a control event (screen locked, monitor lost, restored), waiting a moment for room. These events are rare
/// and must not be dropped the way a surplus frame may be.
pub fn send_event(events: &SyncSender<ScreenEvent>, event: ScreenEvent) {
    let mut event = event;
    for _ in 0..400 {
        match events.try_send(event) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
            Err(std::sync::mpsc::TrySendError::Full(back)) => {
                event = back;
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    /// Stable identifier (the device name on Windows, for example `\\.\DISPLAY2`). Never an index.
    pub id: String,
    pub name: String,
    /// Physical pixels.
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[cfg(not(windows))]
fn unsupported<T>() -> Result<T, CaptureError> {
    Err(CaptureError::Device(
        "screen capture is not available on this platform yet".into(),
    ))
}

/// All monitors, with their physical pixel sizes.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    #[cfg(windows)]
    return windows::list_monitors();
    #[cfg(not(windows))]
    return unsupported();
}

/// A screen source for one monitor (`None` is the primary one), delivering at most `max_fps` frames per second.
pub fn open_screen(
    monitor_id: Option<&str>,
    max_fps: u32,
) -> Result<Box<dyn ScreenSource>, CaptureError> {
    #[cfg(windows)]
    return Ok(Box::new(match monitor_id {
        Some(id) => windows::WindowsScreenSource::new(id, max_fps)?,
        None => windows::WindowsScreenSource::primary(max_fps)?,
    }));
    #[cfg(not(windows))]
    {
        let _ = (monitor_id, max_fps);
        return unsupported();
    }
}

/// One captured frame: BGRA, tightly packed (`width * 4` bytes per row). Cloning shares the pixels.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// The shared-clock reading when the frame was captured.
    pub captured_at: Duration,
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossReason {
    ScreenLocked,
    MonitorLost,
}

#[derive(Debug, Clone)]
pub enum ScreenEvent {
    Frame(VideoFrame),
    /// The source cannot deliver video from `at` (a shared-clock reading) until `Restored`.
    Lost {
        reason: LossReason,
        at: Duration,
    },
    Restored {
        at: Duration,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("the screen could not be captured: {0}")]
    Device(String),
}

/// A source of screen frames. Frames are pushed into `events` from the source's own thread with `try_send`, so a
/// slow consumer makes the source drop frames instead of growing memory.
pub trait ScreenSource: Send {
    fn monitor(&self) -> MonitorInfo;
    /// Does the slow work (opening the monitor and its capture session) without delivering anything yet. The
    /// recording clock starts after every source is prepared, so slow set-up never shows up as lost time.
    fn prepare(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
    /// Starts delivering events. Called again after a pause (the previous `stop` ended delivery).
    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<ScreenEvent>,
    ) -> Result<(), CaptureError>;
    fn stop(&mut self);
}
