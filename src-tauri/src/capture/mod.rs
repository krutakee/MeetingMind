//! Screen capture. The trait is the same everywhere; the implementation behind it is OS-specific
//! (Windows.Graphics.Capture on Windows from M2, ScreenCaptureKit on macOS in Phase 5).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
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

/// All monitors, with their physical pixel sizes.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    platform::list_monitors()
}

/// A screen source for one monitor (`None` is the primary one), delivering at most `max_fps` frames per second.
pub fn open_screen(
    monitor_id: Option<&str>,
    max_fps: u32,
) -> Result<Box<dyn ScreenSource>, CaptureError> {
    platform::open_screen(monitor_id, max_fps)
}

#[cfg(windows)]
mod platform {
    use super::{CaptureError, MonitorInfo, ScreenSource, windows};

    pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
        windows::list_monitors()
    }

    pub fn open_screen(
        monitor_id: Option<&str>,
        max_fps: u32,
    ) -> Result<Box<dyn ScreenSource>, CaptureError> {
        Ok(Box::new(match monitor_id {
            Some(id) => windows::WindowsScreenSource::new(id, max_fps)?,
            None => windows::WindowsScreenSource::primary(max_fps)?,
        }))
    }
}

/// Screen capture for macOS arrives in Phase 5.
#[cfg(not(windows))]
mod platform {
    use super::{CaptureError, MonitorInfo, ScreenSource};

    fn unsupported<T>() -> Result<T, CaptureError> {
        Err(CaptureError::Device(
            "screen capture is not available on this platform yet".into(),
        ))
    }

    pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
        unsupported()
    }

    pub fn open_screen(
        _monitor_id: Option<&str>,
        _max_fps: u32,
    ) -> Result<Box<dyn ScreenSource>, CaptureError> {
        unsupported()
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

/// Hands screen frames to the recorder without ever losing the newest state of the screen.
///
/// A screen only produces a frame when something on it changes. If the recorder's queue is full at that moment (the
/// video encoder stalled for a few hundred milliseconds) and the new frame is thrown away, the recording keeps showing
/// the old picture until the next change, which on a still screen may be minutes away: a slide change that never
/// appears. Found in a 2-hour run on real hardware: about 2 % of 0.4 s flashes were missing from the video. So a frame
/// that does not fit is kept (a newer one replaces it) and sent again as soon as the queue has room, by `flush`, which
/// the source calls from a thread of its own. Memory stays bounded: at most one frame is ever kept.
pub struct FrameRelay {
    events: SyncSender<ScreenEvent>,
    held: Mutex<Option<VideoFrame>>,
    held_back: AtomicU64,
}

impl FrameRelay {
    pub fn new(events: SyncSender<ScreenEvent>) -> Self {
        Self {
            events,
            held: Mutex::new(None),
            held_back: AtomicU64::new(0),
        }
    }

    /// Sends `frame`, or keeps it if the queue is full. Whatever was kept before is older, so it is dropped.
    pub fn offer(&self, frame: VideoFrame) {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        match self.events.try_send(ScreenEvent::Frame(frame)) {
            Ok(()) => *held = None,
            Err(TrySendError::Full(ScreenEvent::Frame(frame))) => {
                self.held_back.fetch_add(1, Ordering::Relaxed);
                *held = Some(frame);
            }
            Err(_) => {}
        }
    }

    /// Tries again to send the kept frame, if there is one. Returns false once the recorder has gone away.
    pub fn flush(&self) -> bool {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        let Some(frame) = held.take() else {
            return true;
        };
        match self.events.try_send(ScreenEvent::Frame(frame)) {
            Ok(()) => true,
            Err(TrySendError::Full(ScreenEvent::Frame(frame))) => {
                *held = Some(frame);
                true
            }
            Err(_) => false,
        }
    }

    /// Forgets the kept frame (the capture has stopped: it must not appear after a pause).
    pub fn clear(&self) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// How many times a frame had to wait because the queue was full.
    pub fn held_back(&self) -> u64 {
        self.held_back.load(Ordering::Relaxed)
    }
}

/// A source of screen frames. Frames are pushed into `events` from the source's own thread; a source should use a
/// `FrameRelay` for them, so a full queue delays a frame instead of losing the newest state of the screen.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn frame(at_ms: u64) -> VideoFrame {
        VideoFrame {
            captured_at: Duration::from_millis(at_ms),
            width: 1,
            height: 1,
            bgra: Arc::new(vec![0, 0, 0, 255]),
        }
    }

    fn times(events: &std::sync::mpsc::Receiver<ScreenEvent>) -> Vec<u64> {
        events
            .try_iter()
            .filter_map(|e| match e {
                ScreenEvent::Frame(f) => Some(f.captured_at.as_millis() as u64),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_frame_goes_straight_through_when_the_queue_has_room() {
        let (tx, rx) = sync_channel(2);
        let relay = FrameRelay::new(tx);

        relay.offer(frame(10));

        assert_eq!(times(&rx), vec![10]);
        assert_eq!(relay.held_back(), 0);
    }

    #[test]
    fn a_frame_that_does_not_fit_is_kept_and_sent_when_there_is_room() {
        let (tx, rx) = sync_channel(1);
        let relay = FrameRelay::new(tx);
        relay.offer(frame(10)); // fills the queue
        relay.offer(frame(20)); // does not fit

        assert_eq!(relay.held_back(), 1);
        assert!(relay.flush(), "still waiting: the queue is full");
        assert_eq!(
            times(&rx),
            vec![10],
            "the consumer has only taken the first so far"
        );

        assert!(relay.flush());
        assert_eq!(
            times(&rx),
            vec![20],
            "the kept frame arrives once there is room"
        );
    }

    #[test]
    fn only_the_newest_waiting_frame_is_kept() {
        let (tx, rx) = sync_channel(1);
        let relay = FrameRelay::new(tx);
        relay.offer(frame(10));
        relay.offer(frame(20));
        relay.offer(frame(30)); // replaces 20
        let _ = times(&rx);

        relay.flush();

        assert_eq!(
            times(&rx),
            vec![30],
            "the old picture is not shown after the new one"
        );
        assert_eq!(relay.held_back(), 2);
    }

    #[test]
    fn a_frame_that_goes_through_makes_an_older_kept_one_obsolete() {
        let (tx, rx) = sync_channel(1);
        let relay = FrameRelay::new(tx);
        relay.offer(frame(10));
        relay.offer(frame(20)); // kept
        assert_eq!(times(&rx), vec![10]);

        relay.offer(frame(30)); // there is room now: 30 goes, 20 is old news

        assert_eq!(times(&rx), vec![30]);
        relay.flush();
        assert!(times(&rx).is_empty(), "20 is never sent after 30");
    }

    #[test]
    fn flushing_with_nothing_kept_does_nothing() {
        let (tx, rx) = sync_channel(1);
        let relay = FrameRelay::new(tx);

        assert!(relay.flush());
        assert!(times(&rx).is_empty());
    }

    #[test]
    fn a_cleared_relay_forgets_the_kept_frame() {
        let (tx, rx) = sync_channel(1);
        let relay = FrameRelay::new(tx);
        relay.offer(frame(10));
        relay.offer(frame(20));
        let _ = times(&rx);

        relay.clear();
        relay.flush();

        assert!(
            times(&rx).is_empty(),
            "a frame from before a pause must not appear after it"
        );
    }

    #[test]
    fn once_the_recorder_is_gone_flushing_says_so() {
        let (tx, rx) = sync_channel(1);
        let relay = FrameRelay::new(tx);
        relay.offer(frame(10));
        relay.offer(frame(20));
        drop(rx);

        assert!(!relay.flush(), "the thread that flushes can stop");
    }
}
