//! Audio capture (microphone and system-audio loopback). The trait is the same everywhere; the implementation behind
//! it is OS-specific (cpal on WASAPI on Windows from M2).

use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use crate::recording::clock::MonotonicClock;

#[cfg(windows)]
pub mod windows;

/// Sends a control event (device lost or restored), waiting a moment for room. These events are rare and must not
/// be dropped the way a surplus audio chunk may be.
pub fn send_event(events: &SyncSender<AudioEvent>, event: AudioEvent) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Interleaved f32 samples. `captured_at` is the shared-clock reading of the first sample.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub captured_at: Duration,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    Chunk(AudioChunk),
    /// The device went away (unplugged, disabled, failed) at `at` (a shared-clock reading). Delivery stops until
    /// `DeviceRestored`.
    DeviceLost {
        reason: String,
        at: Duration,
    },
    DeviceRestored {
        at: Duration,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("the audio device could not be used: {0}")]
    Device(String),
}

/// A source of audio. Chunks are pushed into `events` with `try_send` from the device's thread, so a slow consumer
/// makes the source drop data (reported by the consumer as a gap) instead of blocking the device.
pub trait AudioSource: Send {
    /// Human-readable device name, for `meeting.json` and the UI.
    fn device_name(&self) -> String;
    fn format(&self) -> AudioFormat;
    /// Does the slow work (opening the device) without delivering anything yet. The recording clock starts after
    /// every source is prepared, so slow set-up never shows up as silence at the start.
    fn prepare(&mut self) -> Result<(), AudioError> {
        Ok(())
    }
    /// Starts delivering events. Called again after a pause (the previous `stop` ended delivery).
    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<AudioEvent>,
    ) -> Result<(), AudioError>;
    fn stop(&mut self);
}
