//! Microphone and system-audio (WASAPI loopback) capture on Windows, with cpal 0.18.
//!
//! Each source owns a device thread. The thread holds the cpal streams (which may not be movable between threads),
//! takes commands from the source object, and watches for the device going away. What the M0 spikes found:
//! - loopback is an input stream built on an *output* device;
//! - loopback delivers nothing while no program plays audio, so a silent render stream is kept open next to it;
//! - data arrives in 10 ms chunks and is stamped with a QueryPerformanceCounter time, which is converted here to the
//!   shared recording clock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, SyncSender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, Error as CpalError, ErrorKind, FromSample, InputCallbackInfo, OutputCallbackInfo,
    Sample, SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig,
};

use super::{AudioChunk, AudioError, AudioEvent, AudioFormat, AudioSource, send_event};
use crate::recording::clock::MonotonicClock;

/// How long the device thread waits between checks for a lost device, and how often it tries to bring one back.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
const REOPEN_INTERVAL: Duration = Duration::from_secs(2);
/// Opening a device can take a second or more (M0 measured about 1.5 s on this machine's SoundWire codec).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

pub use super::AudioDeviceInfo;

fn device_error(e: impl std::fmt::Display) -> AudioError {
    AudioError::Device(e.to_string())
}

fn describe(device: &Device, default_id: &Option<String>) -> Result<AudioDeviceInfo, AudioError> {
    let id = device.id().map_err(device_error)?.to_string();
    let name = device
        .description()
        .map_err(device_error)?
        .name()
        .to_string();
    let is_default = default_id.as_deref() == Some(id.as_str());
    Ok(AudioDeviceInfo {
        id,
        name,
        is_default,
    })
}

/// The audio host, after making sure COM stays up for the whole process. cpal keeps one device enumerator for the
/// process, bound to the COM state of the first thread that used it; if that thread ends and COM shuts down, the next
/// thread to use the enumerator crashes. Every use of cpal goes through here.
fn audio_host() -> cpal::Host {
    crate::system::keep_component_runtime_alive();
    cpal::default_host()
}

fn id_of(device: Option<Device>) -> Option<String> {
    device.and_then(|d| d.id().ok()).map(|id| id.to_string())
}

/// Microphones and other input devices, the default one flagged.
pub fn list_microphones() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let host = audio_host();
    let default_id = id_of(host.default_input_device());
    host.input_devices()
        .map_err(device_error)?
        .map(|device| describe(&device, &default_id))
        .collect()
}

/// Output devices (speakers, headsets): the ones whose sound can be captured as system audio.
pub fn list_speakers() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    let host = audio_host();
    let default_id = id_of(host.default_output_device());
    host.output_devices()
        .map_err(device_error)?
        .map(|device| describe(&device, &default_id))
        .collect()
}

/// The default output device, or `None` if the computer has none.
pub fn default_speakers() -> Result<Option<AudioDeviceInfo>, AudioError> {
    let host = audio_host();
    match host.default_output_device() {
        Some(device) => describe(&device, &id_of(Some(device.clone()))).map(Some),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Microphone,
    Loopback,
}

fn find_device(host: &cpal::Host, kind: Kind, id: Option<&str>) -> Option<Device> {
    let default = || match kind {
        Kind::Microphone => host.default_input_device(),
        Kind::Loopback => host.default_output_device(),
    };
    match id {
        None => default(),
        Some(id) => {
            let mut devices: Vec<Device> = match kind {
                Kind::Microphone => host.input_devices().ok()?.collect(),
                Kind::Loopback => host.output_devices().ok()?.collect(),
            };
            devices.retain(|d| d.id().map(|i| i.to_string() == id).unwrap_or(false));
            devices.pop()
        }
    }
}

fn default_config(device: &Device, kind: Kind) -> Result<SupportedStreamConfig, AudioError> {
    match kind {
        Kind::Microphone => device.default_input_config(),
        // Loopback captures what the output device plays, in the output device's own format.
        Kind::Loopback => device.default_output_config(),
    }
    .map_err(device_error)
}

// ---------------------------------------------------------------------------------------------------------------

/// Where a stream sends its data. Empty while stopped or paused, so a running stream's callbacks drop everything.
struct Sink {
    clock: Arc<dyn MonotonicClock>,
    events: SyncSender<AudioEvent>,
}

type SharedSink = Arc<Mutex<Option<Sink>>>;

enum Command {
    Prepare(Sender<Result<(), String>>),
    Start(
        Arc<dyn MonotonicClock>,
        SyncSender<AudioEvent>,
        Sender<Result<(), String>>,
    ),
    Stop(Sender<()>),
    Shutdown,
}

/// A microphone or a system-audio loopback source.
pub struct CpalAudioSource {
    name: String,
    format: AudioFormat,
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl CpalAudioSource {
    /// A microphone. `None` means the default input device.
    pub fn microphone(device_id: Option<&str>) -> Result<Self, AudioError> {
        Self::new(Kind::Microphone, device_id)
    }

    /// System audio: whatever an output device plays. `None` means the default output device.
    pub fn system_audio(device_id: Option<&str>) -> Result<Self, AudioError> {
        Self::new(Kind::Loopback, device_id)
    }

    fn new(kind: Kind, device_id: Option<&str>) -> Result<Self, AudioError> {
        let host = audio_host();
        let device = find_device(&host, kind, device_id).ok_or_else(|| {
            AudioError::Device(match kind {
                Kind::Microphone => "no microphone was found".to_string(),
                Kind::Loopback => "no audio output device was found".to_string(),
            })
        })?;
        let config = default_config(&device, kind)?;
        let format = AudioFormat {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        };
        let name = device
            .description()
            .map_err(device_error)?
            .name()
            .to_string();
        let id = device.id().map_err(device_error)?.to_string();
        let follow_default = device_id.is_none();

        let (commands, rx) = channel();
        let worker = Worker {
            kind,
            device_id: id,
            follow_default,
            format,
            sink: Arc::new(Mutex::new(None)),
            lost: Arc::new(AtomicBool::new(false)),
            capture: None,
            render: None,
            last_reopen: None,
        };
        let thread = std::thread::Builder::new()
            .name(format!("audio-{kind:?}"))
            .spawn(move || worker.run(rx))
            .map_err(device_error)?;
        Ok(Self {
            name,
            format,
            commands,
            thread: Some(thread),
        })
    }

    fn request<T>(&self, make: impl FnOnce(Sender<T>) -> Command) -> Result<T, AudioError> {
        let (reply_tx, reply_rx) = channel();
        self.commands
            .send(make(reply_tx))
            .map_err(|_| device_error("the audio thread has stopped"))?;
        reply_rx
            .recv_timeout(COMMAND_TIMEOUT)
            .map_err(|_| device_error("the audio device did not answer in time"))
    }
}

impl AudioSource for CpalAudioSource {
    fn device_name(&self) -> String {
        self.name.clone()
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn prepare(&mut self) -> Result<(), AudioError> {
        self.request(Command::Prepare)?.map_err(AudioError::Device)
    }

    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<AudioEvent>,
    ) -> Result<(), AudioError> {
        self.request(|reply| Command::Start(clock, events, reply))?
            .map_err(AudioError::Device)
    }

    fn stop(&mut self) {
        let _ = self.request(Command::Stop);
    }
}

impl Drop for CpalAudioSource {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------

struct Worker {
    kind: Kind,
    device_id: String,
    /// True if the user chose "default device": after a loss, the current default is used.
    follow_default: bool,
    format: AudioFormat,
    sink: SharedSink,
    lost: Arc<AtomicBool>,
    capture: Option<Stream>,
    /// Loopback only: a stream that plays silence so the output device is active and loopback delivers data.
    render: Option<Stream>,
    last_reopen: Option<Instant>,
}

impl Worker {
    fn run(mut self, commands: std::sync::mpsc::Receiver<Command>) {
        loop {
            match commands.recv_timeout(WATCH_INTERVAL) {
                Ok(Command::Prepare(reply)) => {
                    let _ = reply.send(self.open());
                }
                Ok(Command::Start(clock, events, reply)) => {
                    let _ = reply.send(self.start(clock, events));
                }
                Ok(Command::Stop(reply)) => {
                    self.stop();
                    let _ = reply.send(());
                }
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    self.stop();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => self.try_to_bring_the_device_back(),
            }
        }
    }

    /// Builds the streams (without delivering anything). Slow: this is what `prepare` is for.
    fn open(&mut self) -> Result<(), String> {
        self.capture = None;
        self.render = None;
        let host = audio_host();
        let wanted = if self.follow_default {
            None
        } else {
            Some(self.device_id.as_str())
        };
        let device =
            find_device(&host, self.kind, wanted).ok_or("the audio device is not available")?;
        let config = default_config(&device, self.kind).map_err(|e| e.to_string())?;
        let format = AudioFormat {
            sample_rate: config.sample_rate(),
            channels: config.channels(),
        };
        if format != self.format {
            // The recording's encoder was started for the old format; samples in another format cannot be written to it.
            return Err(format!(
                "the device now uses {} Hz, {} channels instead of {} Hz, {} channels",
                format.sample_rate, format.channels, self.format.sample_rate, self.format.channels
            ));
        }
        let capture = build_capture_stream(&device, &config, self.sink.clone(), self.lost.clone())?;
        let _ = capture.pause(); // nothing flows until `start`
        self.capture = Some(capture);
        if self.kind == Kind::Loopback {
            let render = build_silent_render(&device)?;
            let _ = render.pause();
            self.render = Some(render);
        }
        Ok(())
    }

    fn start(
        &mut self,
        clock: Arc<dyn MonotonicClock>,
        events: SyncSender<AudioEvent>,
    ) -> Result<(), String> {
        if self.capture.is_none() {
            self.open()?;
        }
        self.lost.store(false, Ordering::SeqCst);
        *self.sink.lock().unwrap() = Some(Sink { clock, events });
        // The silent render stream first, so the loopback has packets to deliver from the start.
        if let Some(render) = &self.render {
            render.play().map_err(|e| e.to_string())?;
        }
        if let Some(capture) = &self.capture {
            capture.play().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn stop(&mut self) {
        *self.sink.lock().unwrap() = None;
        if let Some(capture) = &self.capture {
            let _ = capture.pause();
        }
        if let Some(render) = &self.render {
            let _ = render.pause();
        }
    }

    /// While recording, a lost device is retried every couple of seconds.
    fn try_to_bring_the_device_back(&mut self) {
        if !self.lost.load(Ordering::SeqCst) || self.sink.lock().unwrap().is_none() {
            return;
        }
        if self
            .last_reopen
            .is_some_and(|t| t.elapsed() < REOPEN_INTERVAL)
        {
            return;
        }
        self.last_reopen = Some(Instant::now());
        let (clock, events) = match self.sink.lock().unwrap().as_ref() {
            Some(sink) => (sink.clock.clone(), sink.events.clone()),
            None => return,
        };
        if self.open().is_err() {
            return;
        }
        let started = self.start(clock.clone(), events.clone());
        if started.is_ok() {
            send_event(&events, AudioEvent::DeviceRestored { at: clock.now() });
        }
    }
}

fn build_capture_stream(
    device: &Device,
    supported: &SupportedStreamConfig,
    sink: SharedSink,
    lost: Arc<AtomicBool>,
) -> Result<Stream, String> {
    let config = supported.config();
    match supported.sample_format() {
        SampleFormat::F32 => capture::<f32>(device, config, sink, lost),
        SampleFormat::I16 => capture::<i16>(device, config, sink, lost),
        SampleFormat::I32 => capture::<i32>(device, config, sink, lost),
        other => Err(format!("the sample format {other} is not supported yet")),
    }
}

fn capture<T>(
    device: &Device,
    config: StreamConfig,
    sink: SharedSink,
    lost: Arc<AtomicBool>,
) -> Result<Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let data_sink = sink.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], info: &InputCallbackInfo| {
                // Copy what is needed out of the lock quickly; sending happens without holding it.
                let (clock, events) = match data_sink.lock().unwrap().as_ref() {
                    Some(sink) => (sink.clock.clone(), sink.events.clone()),
                    None => return,
                };
                let timestamp = info.timestamp();
                // How long ago the first sample of this chunk was captured, in the device's QPC time.
                let age = timestamp
                    .callback
                    .saturating_duration_since(timestamp.capture);
                let captured_at = clock.now().saturating_sub(age);
                let samples: Vec<f32> = data.iter().map(|&s| f32::from_sample(s)).collect();
                // A full queue means the consumer is behind: drop this chunk (it becomes a logged gap), never block.
                let _ = events.try_send(AudioEvent::Chunk(AudioChunk {
                    captured_at,
                    samples,
                }));
            },
            move |error: CpalError| on_stream_error(&error, &sink, &lost),
            None,
        )
        .map_err(|e| e.to_string())
}

/// An overrun is a glitch the timeline already handles; anything else means the device is gone or unusable.
fn on_stream_error(error: &CpalError, sink: &SharedSink, lost: &AtomicBool) {
    if error.kind() == ErrorKind::Xrun {
        return;
    }
    if lost.swap(true, Ordering::SeqCst) {
        return; // already reported
    }
    let target = sink
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| (s.clock.clone(), s.events.clone()));
    if let Some((clock, events)) = target {
        send_event(
            &events,
            AudioEvent::DeviceLost {
                reason: error.to_string(),
                at: clock.now(),
            },
        );
    }
}

fn build_silent_render(device: &Device) -> Result<Stream, String> {
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    match supported.sample_format() {
        SampleFormat::F32 => silent::<f32>(device, config),
        SampleFormat::I16 => silent::<i16>(device, config),
        SampleFormat::I32 => silent::<i32>(device, config),
        other => Err(format!(
            "the output sample format {other} is not supported yet"
        )),
    }
}

fn silent<T: SizedSample + Send + 'static>(
    device: &Device,
    config: StreamConfig,
) -> Result<Stream, String> {
    device
        .build_output_stream(
            config,
            |data: &mut [T], _: &OutputCallbackInfo| data.fill(T::EQUILIBRIUM),
            |_: CpalError| {}, // a dead render stream shows up as a dead capture stream
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cpal keeps one device enumerator for the whole process, created on the first thread that asks and bound to that
    /// thread's COM state. When that thread ends and nothing else keeps COM alive, COM shuts down and the next thread to
    /// use the enumerator crashes (access violation). Device lookups must therefore keep the component runtime alive
    /// themselves. One short-lived thread after another is exactly the situation (a test runner, a worker per request).
    #[test]
    fn device_lookups_on_one_short_lived_thread_after_another_do_not_crash() {
        for round in 0..6 {
            std::thread::spawn(|| {
                // Whether devices exist does not matter here (a CI runner has none); surviving does.
                let _ = default_speakers();
                let _ = list_microphones();
                let _ = list_speakers();
            })
            .join()
            .unwrap_or_else(|_| panic!("lookup thread {round} panicked"));
        }
    }
}
