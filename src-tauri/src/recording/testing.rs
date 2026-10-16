//! TEST DOUBLES. Synthetic screen and audio sources for the recording tests and the crash-recovery test program.
//!
//! Compiled only for tests and with the `test-doubles` feature (a dev-dependency turns it on for `cargo test`).
//! Never part of the app: the app records only through real sources. These sources run in real time against the
//! real clock, emit known patterns, and can be told to lock the screen or unplug a "device", so the recording
//! pipeline, the real FFmpeg and the failure handling can be exercised without any hardware.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::{AudioChunk, AudioError, AudioEvent, AudioFormat, AudioSource};
use crate::capture::{
    CaptureError, LossReason, MonitorInfo, ScreenEvent, ScreenSource, VideoFrame,
};
use crate::recording::clock::MonotonicClock;

fn sleep_until(deadline: Instant) {
    let now = Instant::now();
    if deadline > now {
        std::thread::sleep(deadline - now);
    }
}

// ---------------------------------------------------------------------------------------------------------------

/// Lets a test lock or unlock the synthetic screen while it records.
#[derive(Clone, Default)]
pub struct ScreenHandle {
    locked: Arc<AtomicBool>,
}

impl ScreenHandle {
    pub fn lock_screen(&self) {
        self.locked.store(true, Ordering::SeqCst);
    }
    pub fn unlock_screen(&self) {
        self.locked.store(false, Ordering::SeqCst);
    }
}

/// A screen that changes every `change_every` (a mostly still desktop, like the real thing: the first frame at
/// once, then frames only when something changes).
pub struct SyntheticScreen {
    monitor: MonitorInfo,
    change_every: Duration,
    handle: ScreenHandle,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SyntheticScreen {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            monitor: MonitorInfo {
                id: "synthetic-monitor".into(),
                name: "Synthetic monitor".into(),
                width,
                height,
                is_primary: true,
            },
            change_every: Duration::from_millis(700),
            handle: ScreenHandle::default(),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    pub fn with_change_every(mut self, every: Duration) -> Self {
        self.change_every = every;
        self
    }

    pub fn handle(&self) -> ScreenHandle {
        self.handle.clone()
    }
}

fn bgra_frame(width: u32, height: u32, n: u32) -> Vec<u8> {
    let mut data = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let o = ((y * width + x) * 4) as usize;
            let bar = (x + n * 16) % width < 24;
            data[o] = (x * 255 / width) as u8;
            data[o + 1] = (y * 255 / height) as u8;
            data[o + 2] = if bar { 255 } else { 30 };
            data[o + 3] = 255;
        }
    }
    data
}

impl ScreenSource for SyntheticScreen {
    fn monitor(&self) -> MonitorInfo {
        self.monitor.clone()
    }

    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<ScreenEvent>,
    ) -> Result<(), CaptureError> {
        self.stop.store(false, Ordering::SeqCst);
        let (stop, locked) = (self.stop.clone(), self.handle.locked.clone());
        let (w, h, every) = (self.monitor.width, self.monitor.height, self.change_every);
        self.thread = Some(std::thread::spawn(move || {
            let mut n = 0u32;
            let mut next_change = Instant::now();
            let mut was_locked = false;
            while !stop.load(Ordering::SeqCst) {
                let is_locked = locked.load(Ordering::SeqCst);
                if is_locked != was_locked {
                    was_locked = is_locked;
                    let at = clock.now();
                    let event = if is_locked {
                        ScreenEvent::Lost {
                            reason: LossReason::ScreenLocked,
                            at,
                        }
                    } else {
                        ScreenEvent::Restored { at }
                    };
                    crate::capture::send_event(&events, event);
                    next_change = Instant::now(); // a new frame right after unlocking, like a real redraw
                }
                if !is_locked && Instant::now() >= next_change {
                    let frame = VideoFrame {
                        captured_at: clock.now(),
                        width: w,
                        height: h,
                        bgra: Arc::new(bgra_frame(w, h, n)),
                    };
                    n += 1;
                    match events.try_send(ScreenEvent::Frame(frame)) {
                        Ok(()) | Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                    next_change += every;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------

/// Lets a test unplug and replug the synthetic audio device while it records.
#[derive(Clone, Default)]
pub struct AudioHandle {
    unplugged: Arc<AtomicBool>,
}

impl AudioHandle {
    pub fn unplug(&self) {
        self.unplugged.store(true, Ordering::SeqCst);
    }
    pub fn replug(&self) {
        self.unplugged.store(false, Ordering::SeqCst);
    }
}

/// A 440 Hz sine delivered in 10 ms chunks in real time. `drift_ppm` makes the device clock run fast or slow
/// against the system clock, like a real USB device does.
pub struct SyntheticAudio {
    name: String,
    format: AudioFormat,
    amplitude: f32,
    drift_ppm: f64,
    start_delay: Duration,
    handle: AudioHandle,
    fail_to_start: bool,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    started_log: Arc<Mutex<u32>>,
}

impl SyntheticAudio {
    pub fn new(name: &str, sample_rate: u32, channels: u16) -> Self {
        Self {
            name: name.into(),
            format: AudioFormat {
                sample_rate,
                channels,
            },
            amplitude: 0.3,
            drift_ppm: 0.0,
            start_delay: Duration::from_millis(40),
            handle: AudioHandle::default(),
            fail_to_start: false,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            started_log: Arc::new(Mutex::new(0)),
        }
    }

    pub fn with_drift_ppm(mut self, ppm: f64) -> Self {
        self.drift_ppm = ppm;
        self
    }

    pub fn with_amplitude(mut self, amplitude: f32) -> Self {
        self.amplitude = amplitude;
        self
    }

    /// A device that cannot be opened.
    pub fn failing_to_start(mut self) -> Self {
        self.fail_to_start = true;
        self
    }

    pub fn handle(&self) -> AudioHandle {
        self.handle.clone()
    }

    /// How many times `start` succeeded (a pause and resume start the source again).
    pub fn start_count(&self) -> Arc<Mutex<u32>> {
        self.started_log.clone()
    }
}

impl AudioSource for SyntheticAudio {
    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<AudioEvent>,
    ) -> Result<(), AudioError> {
        if self.fail_to_start {
            return Err(AudioError::Device(
                "synthetic device refused to open".into(),
            ));
        }
        *self.started_log.lock().unwrap() += 1;
        self.stop.store(false, Ordering::SeqCst);
        let (stop, unplugged) = (self.stop.clone(), self.handle.unplugged.clone());
        let format = self.format;
        let (amplitude, drift_ppm, start_delay) =
            (self.amplitude, self.drift_ppm, self.start_delay);
        self.thread = Some(std::thread::spawn(move || {
            const CHUNK_FRAMES: usize = 480;
            let device_rate = format.sample_rate as f64 * (1.0 + drift_ppm / 1e6);
            let chunk_secs = CHUNK_FRAMES as f64 / device_rate;
            let channels = format.channels as usize;
            // Device start-up latency: the first data arrives a little after start, as on real hardware.
            std::thread::sleep(start_delay);
            let mut session_start_wall = Instant::now();
            let mut session_start_clock = clock.now();
            let mut index: u64 = 0; // chunks since this device session began
            let mut sample_counter: u64 = 0; // never reset, so the sine stays continuous
            let mut lost = false;
            while !stop.load(Ordering::SeqCst) {
                if unplugged.load(Ordering::SeqCst) {
                    if !lost {
                        lost = true;
                        crate::audio::send_event(
                            &events,
                            AudioEvent::DeviceLost {
                                reason: "synthetic device unplugged".into(),
                                at: clock.now(),
                            },
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                if lost {
                    lost = false;
                    session_start_wall = Instant::now();
                    session_start_clock = clock.now();
                    crate::audio::send_event(
                        &events,
                        AudioEvent::DeviceRestored {
                            at: session_start_clock,
                        },
                    );
                    index = 0;
                }
                let due = session_start_wall + Duration::from_secs_f64(index as f64 * chunk_secs);
                sleep_until(due);
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let captured_at =
                    session_start_clock + Duration::from_secs_f64(index as f64 * chunk_secs);
                let mut samples = Vec::with_capacity(CHUNK_FRAMES * channels);
                for _ in 0..CHUNK_FRAMES {
                    let v = (2.0 * std::f64::consts::PI * 440.0 * sample_counter as f64
                        / format.sample_rate as f64)
                        .sin() as f32
                        * amplitude;
                    samples.extend(std::iter::repeat_n(v, channels));
                    sample_counter += 1;
                }
                match events.try_send(AudioEvent::Chunk(AudioChunk {
                    captured_at,
                    samples,
                })) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return,
                }
                index += 1;
            }
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
