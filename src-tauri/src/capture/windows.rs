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
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::{info, warn};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
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

/// The shortest time between two frames that are worth copying: half a frame of the recording, so the pacer always
/// has a fresh one for its next slot.
fn frame_interval(max_fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / (f64::from(max_fps.max(1)) * 2.0))
}

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

/// A frame older than this when it arrives cannot be right (its own timestamp is not on our time base): its arrival
/// time is used instead.
const MAX_PLAUSIBLE_FRAME_AGE: Duration = Duration::from_secs(2);

/// The performance counter in units of 100 ns, the unit of a frame's own timestamp. `None` if it cannot be read.
fn performance_counter_100ns() -> Option<i64> {
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut frequency = 0i64;
        // SAFETY: writes one integer through a valid pointer.
        match unsafe { QueryPerformanceFrequency(&mut frequency) } {
            Ok(()) => frequency,
            Err(_) => 0,
        }
    });
    if frequency <= 0 {
        return None;
    }
    let mut counter = 0i64;
    // SAFETY: writes one integer through a valid pointer.
    unsafe { QueryPerformanceCounter(&mut counter) }.ok()?;
    Some((i128::from(counter) * 10_000_000 / i128::from(frequency)) as i64)
}

/// When a frame was captured, on the recording clock: `now` minus the frame's age. The age is how much older the
/// frame's own timestamp (100 ns units, same time base as `now_ticks`) is than the moment it was handed over. A frame
/// can arrive late when the system is busy (frames up to 0.9 s late were seen in a 2-hour run); stamping it with its
/// arrival time would put what it shows at the wrong time in the video. An age that makes no sense falls back to `now`.
fn captured_time(now: Duration, now_ticks: i64, stamp_ticks: i64) -> Duration {
    let age_ticks = now_ticks.saturating_sub(stamp_ticks);
    match u64::try_from(age_ticks) {
        Ok(ticks) if Duration::from_nanos(ticks.saturating_mul(100)) <= MAX_PLAUSIBLE_FRAME_AGE => {
            now.saturating_sub(Duration::from_nanos(ticks * 100))
        }
        _ => now,
    }
}

struct HandlerFlags {
    clock: Arc<dyn MonotonicClock>,
    events: SyncSender<ScreenEvent>,
    /// Frames closer together than this are skipped before they are copied. `None` when Windows itself limits how
    /// often frames are delivered (see `begin`): then nothing is skipped here, because a skipped frame is a state of
    /// the screen that is never recorded if nothing changes afterwards (a flash of 0.4 s was missed 13 % of the time).
    min_interval: Option<Duration>,
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
        if let Some(min_interval) = self.flags.min_interval
            && self
                .last_accepted
                .is_some_and(|t| now.duration_since(t) < min_interval)
        {
            return Ok(());
        }
        self.last_accepted = Some(now);
        let arrival = self.flags.clock.now();
        let captured_at = match (frame.timestamp(), performance_counter_100ns()) {
            (Ok(stamp), Some(now_ticks)) => captured_time(arrival, now_ticks, stamp.Duration),
            _ => arrival,
        };
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

/// Starts one capture. `os_throttle` asks Windows to deliver frames no more often than that (the newest state of the
/// screen is what arrives, so nothing is lost); it needs a recent Windows 11, so `begin_best` falls back without it.
fn begin(
    device_name: &str,
    flags: HandlerFlags,
    borderless: bool,
    os_throttle: Option<Duration>,
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
        match os_throttle {
            Some(interval) => MinimumUpdateIntervalSettings::Custom(interval),
            None => MinimumUpdateIntervalSettings::Default,
        },
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );
    Handler::start_free_threaded(settings).map_err(capture_error)
}

/// Starts a capture with the best settings this Windows supports: no border and an OS-side frame throttle, then
/// without either where they are not supported. `flags(true)` builds the handler for an OS-throttled capture,
/// `flags(false)` for one that has to skip frames itself.
fn begin_best(
    device_name: &str,
    interval: Duration,
    flags: impl Fn(bool) -> HandlerFlags,
) -> Result<Control, CaptureError> {
    let mut last_error = None;
    for (borderless, throttled) in [(true, true), (false, true), (true, false), (false, false)] {
        let throttle = throttled.then_some(interval);
        match begin(device_name, flags(throttled), borderless, throttle) {
            Ok(control) => {
                info!(borderless, throttled, "screen capture started");
                return Ok(control);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("at least one attempt was made"))
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
        os_throttled: bool,
    ) -> HandlerFlags {
        HandlerFlags {
            clock: clock.clone(),
            events: events.clone(),
            min_interval: (!os_throttled).then(|| frame_interval(self.max_fps)),
            closed: closed.clone(),
        }
    }

    /// Starts a capture with the best settings this Windows supports (see `begin_best`).
    fn begin_capture(&self, flags: impl Fn(bool) -> HandlerFlags) -> Result<Control, CaptureError> {
        begin_best(&self.monitor.id, frame_interval(self.max_fps), flags)
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
        let control =
            self.begin_capture(|throttled| self.flags(&clock, &events, &closed, throttled))?;
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
                    let interval = frame_interval(max_fps);
                    while !stop.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(500));
                        if !closed.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
                            continue;
                        }
                        let flags = |throttled: bool| HandlerFlags {
                            clock: clock.clone(),
                            events: events.clone(),
                            min_interval: (!throttled).then_some(interval),
                            closed: closed.clone(),
                        };
                        // The old capture has ended; a new one can only start once the monitor is back.
                        if let Ok(new_control) = begin_best(&monitor_id, interval, flags) {
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

#[cfg(test)]
mod tests {
    use super::*;

    const TICKS_PER_MS: i64 = 10_000;

    #[test]
    fn a_frame_is_stamped_with_the_time_it_was_captured_not_the_time_it_arrived() {
        let now = Duration::from_secs(100);

        // It was captured 300 ms before it was handed over.
        let at = captured_time(now, 5_000 * TICKS_PER_MS, 4_700 * TICKS_PER_MS);

        assert_eq!(at, Duration::from_millis(99_700));
    }

    #[test]
    fn a_frame_captured_just_now_keeps_the_arrival_time() {
        let now = Duration::from_secs(100);

        assert_eq!(captured_time(now, 777, 777), now);
    }

    #[test]
    fn a_timestamp_in_the_future_or_far_in_the_past_is_not_believed() {
        let now = Duration::from_secs(100);

        // Newer than the counter: impossible.
        assert_eq!(captured_time(now, 1_000, 2_000), now);
        // Ten seconds old when handed over: not a frame time on this time base.
        assert_eq!(
            captured_time(now, 100_000 * TICKS_PER_MS, 90_000 * TICKS_PER_MS),
            now
        );
    }

    #[test]
    fn a_frame_from_before_the_recording_started_is_placed_at_its_start() {
        // Captured 500 ms ago, but the recording clock is only 200 ms old.
        let at = captured_time(
            Duration::from_millis(200),
            5_000 * TICKS_PER_MS,
            4_500 * TICKS_PER_MS,
        );

        assert_eq!(at, Duration::ZERO);
    }

    #[test]
    fn the_performance_counter_moves_forward_in_units_of_100_ns() {
        let first = performance_counter_100ns().expect("the counter can be read");
        std::thread::sleep(Duration::from_millis(50));
        let second = performance_counter_100ns().unwrap();

        let elapsed_ms = (second - first) / TICKS_PER_MS;
        assert!((45..200).contains(&elapsed_ms), "{elapsed_ms} ms");
    }

    #[test]
    fn half_a_frame_of_the_recording_is_the_shortest_interval_worth_copying() {
        assert_eq!(frame_interval(15), Duration::from_secs_f64(1.0 / 30.0));
        assert_eq!(frame_interval(30), Duration::from_secs_f64(1.0 / 60.0));
        assert_eq!(
            frame_interval(0),
            Duration::from_millis(500),
            "never divides by zero"
        );
    }
}
