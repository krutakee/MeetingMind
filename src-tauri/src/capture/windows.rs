//! Screen capture on Windows with Windows.Graphics.Capture (the `windows-capture` crate).
//!
//! What the M0 spikes found, and how it is handled here:
//! - frames arrive only when something on the monitor changes (and may arrive faster than needed), so a frame is
//!   copied only if enough time has passed since the last one; the recording's pacer repeats frames for static screens;
//! - a monitor is chosen by its device name (`\\.\DISPLAY2`), never by an index: the library's index order does not
//!   match the display numbering;
//! - sizes are physical pixels.
//!
//! The workstation being locked is reported through the session watcher, and a monitor that disappears through the
//! capture ending on its own; a supervisor thread then starts capturing again when the monitor comes back.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::warn;
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::{
    CaptureError, LossReason, MonitorInfo, ScreenEvent, ScreenSource, VideoFrame, send_event,
};
use crate::recording::clock::MonotonicClock;
use crate::system::{SessionEvent, SessionWatcher, watch_session};

type HandlerError = Box<dyn std::error::Error + Send + Sync>;
type Control = CaptureControl<Handler, HandlerError>;

fn capture_error(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Device(e.to_string())
}

fn info_of(
    monitor: &Monitor,
    primary_device: &Option<String>,
) -> Result<MonitorInfo, CaptureError> {
    let id = monitor.device_name().map_err(capture_error)?;
    let name = monitor.name().unwrap_or_else(|_| id.clone());
    Ok(MonitorInfo {
        is_primary: primary_device.as_deref() == Some(id.as_str()),
        id,
        name,
        width: monitor.width().map_err(capture_error)?,
        height: monitor.height().map_err(capture_error)?,
    })
}

/// All monitors, with their physical pixel sizes, in the library's order.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    crate::system::keep_component_runtime_alive();
    let primary = Monitor::primary().ok().and_then(|m| m.device_name().ok());
    Monitor::enumerate()
        .map_err(capture_error)?
        .iter()
        .map(|monitor| info_of(monitor, &primary))
        .collect()
}

fn find_monitor(device_name: &str) -> Result<Monitor, CaptureError> {
    for monitor in Monitor::enumerate().map_err(capture_error)? {
        if monitor
            .device_name()
            .map(|n| n == device_name)
            .unwrap_or(false)
        {
            return Ok(monitor);
        }
    }
    Err(CaptureError::Device(format!(
        "the monitor {device_name} was not found"
    )))
}

// ---------------------------------------------------------------------------------------------------------------

struct HandlerFlags {
    clock: Arc<dyn MonotonicClock>,
    events: SyncSender<ScreenEvent>,
    /// Frames closer together than this are skipped before they are copied.
    min_interval: Duration,
    /// Set when the capture ends by itself (the monitor went away).
    closed: Arc<AtomicBool>,
}

struct Handler {
    flags: HandlerFlags,
    last_accepted: Option<Instant>,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = HandlerFlags;
    type Error = HandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            flags: ctx.flags,
            last_accepted: None,
            scratch: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();
        if self
            .last_accepted
            .is_some_and(|t| now.duration_since(t) < self.flags.min_interval)
        {
            return Ok(());
        }
        self.last_accepted = Some(now);
        // The frame was captured a few milliseconds ago (M0: arrival within about 15 ms of its own timestamp); the
        // arrival time is close enough for a 15 fps recording and avoids converting between clocks.
        let captured_at = self.flags.clock.now();
        let (width, height) = (frame.width(), frame.height());
        let buffer = frame.buffer()?;
        let pixels = buffer.as_nopadding_buffer(&mut self.scratch).to_vec();
        // A full queue means the recorder is behind: dropping this frame is right (the pacer repeats the last one).
        let _ = self.flags.events.try_send(ScreenEvent::Frame(VideoFrame {
            captured_at,
            width,
            height,
            bgra: Arc::new(pixels),
        }));
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.flags.closed.store(true, Ordering::SeqCst);
        send_event(
            &self.flags.events,
            ScreenEvent::Lost {
                reason: LossReason::MonitorLost,
                at: self.flags.clock.now(),
            },
        );
        Ok(())
    }
}

fn begin(
    device_name: &str,
    flags: HandlerFlags,
    borderless: bool,
) -> Result<Control, CaptureError> {
    let monitor = find_monitor(device_name)?;
    let border = if borderless {
        DrawBorderSettings::WithoutBorder
    } else {
        DrawBorderSettings::Default
    };
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        border,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );
    Handler::start_free_threaded(settings).map_err(capture_error)
}

// ---------------------------------------------------------------------------------------------------------------

struct Running {
    control: Arc<Mutex<Option<Control>>>,
    stop: Arc<AtomicBool>,
    supervisor: Option<JoinHandle<()>>,
    _session_watcher: Option<SessionWatcher>,
}

pub struct WindowsScreenSource {
    monitor: MonitorInfo,
    /// The most frames per second worth copying.
    max_fps: u32,
    running: Option<Running>,
}

impl WindowsScreenSource {
    /// Captures the monitor with this device name (see `list_monitors`). `max_fps` is the recording's frame rate;
    /// the source delivers at most about twice that.
    pub fn new(monitor_id: &str, max_fps: u32) -> Result<Self, CaptureError> {
        crate::system::keep_component_runtime_alive();
        let primary = Monitor::primary().ok().and_then(|m| m.device_name().ok());
        let monitor = info_of(&find_monitor(monitor_id)?, &primary)?;
        Ok(Self {
            monitor,
            max_fps: max_fps.max(1),
            running: None,
        })
    }

    /// Captures the primary monitor.
    pub fn primary(max_fps: u32) -> Result<Self, CaptureError> {
        let primary = list_monitors()?
            .into_iter()
            .find(|m| m.is_primary)
            .ok_or_else(|| CaptureError::Device("no primary monitor was found".into()))?;
        Self::new(&primary.id, max_fps)
    }

    fn flags(
        &self,
        clock: &Arc<dyn MonotonicClock>,
        events: &SyncSender<ScreenEvent>,
        closed: &Arc<AtomicBool>,
    ) -> HandlerFlags {
        HandlerFlags {
            clock: clock.clone(),
            events: events.clone(),
            min_interval: Duration::from_secs_f64(1.0 / (self.max_fps as f64 * 2.0)),
            closed: closed.clone(),
        }
    }

    /// Starts a capture, preferring no border around the monitor and falling back to the default where that is
    /// not supported.
    fn begin_capture(&self, flags: impl Fn() -> HandlerFlags) -> Result<Control, CaptureError> {
        begin(&self.monitor.id, flags(), true).or_else(|_| begin(&self.monitor.id, flags(), false))
    }
}

impl ScreenSource for WindowsScreenSource {
    fn monitor(&self) -> MonitorInfo {
        self.monitor.clone()
    }

    fn prepare(&mut self) -> Result<(), CaptureError> {
        // Check that the monitor is still there; the capture itself starts in `start`, in about 0.2 s (M0).
        find_monitor(&self.monitor.id).map(|_| ())
    }

    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<ScreenEvent>,
    ) -> Result<(), CaptureError> {
        self.stop();
        let closed = Arc::new(AtomicBool::new(false));
        let control = self.begin_capture(|| self.flags(&clock, &events, &closed))?;
        let control = Arc::new(Mutex::new(Some(control)));
        let stop = Arc::new(AtomicBool::new(false));

        // Screen lock and unlock: delivered as events so the recording shows black for the gap.
        let lock_clock = clock.clone();
        let lock_events = events.clone();
        let session_watcher = match watch_session(move |event| {
            let at = lock_clock.now();
            let event = match event {
                SessionEvent::Locked => ScreenEvent::Lost {
                    reason: LossReason::ScreenLocked,
                    at,
                },
                SessionEvent::Unlocked => ScreenEvent::Restored { at },
            };
            send_event(&lock_events, event);
        }) {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                warn!(error = %e, "screen lock events are not available; a locked screen will show as a still picture");
                None
            }
        };

        // If the capture ends by itself (monitor unplugged), start it again when the monitor is back.
        let supervisor = {
            let (control, stop, closed) = (control.clone(), stop.clone(), closed.clone());
            let (monitor_id, max_fps) = (self.monitor.id.clone(), self.max_fps);
            std::thread::Builder::new()
                .name("screen-supervisor".into())
                .spawn(move || {
                    let min_interval = Duration::from_secs_f64(1.0 / (max_fps as f64 * 2.0));
                    while !stop.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(500));
                        if !closed.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
                            continue;
                        }
                        let flags = || HandlerFlags {
                            clock: clock.clone(),
                            events: events.clone(),
                            min_interval,
                            closed: closed.clone(),
                        };
                        // The old capture has ended; a new one can only start once the monitor is back.
                        if let Ok(new_control) = begin(&monitor_id, flags(), true)
                            .or_else(|_| begin(&monitor_id, flags(), false))
                        {
                            closed.store(false, Ordering::SeqCst);
                            *control.lock().unwrap() = Some(new_control);
                            send_event(&events, ScreenEvent::Restored { at: clock.now() });
                        }
                    }
                })
                .map_err(capture_error)?
        };

        self.running = Some(Running {
            control,
            stop,
            supervisor: Some(supervisor),
            _session_watcher: session_watcher,
        });
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut running) = self.running.take() {
            running.stop.store(true, Ordering::SeqCst);
            drop(running._session_watcher.take());
            if let Some(supervisor) = running.supervisor.take() {
                let _ = supervisor.join();
            }
            if let Some(control) = running.control.lock().unwrap().take() {
                let _ = control.stop();
            }
        }
    }
}

impl Drop for WindowsScreenSource {
    fn drop(&mut self) {
        self.stop();
    }
}
