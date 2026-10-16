//! One recording, from start to the finished file.
//!
//! Threads while recording: one worker per stream (video, microphone, system audio), each owning its FFmpeg encoder,
//! and a disk-space watcher. Devices are never touched here: they come in through `ScreenSource` and `AudioSource`,
//! so everything in this file is the same on Windows and macOS and can be tested with synthetic sources.
//!
//! Pausing stops every source (paused means "not capturing") and cuts the paused time out of the media timeline.
//! Stopping flushes the workers, closes the encoders, mixes the tracks into `recording.mp4` (or `recording.m4a`),
//! and removes the temporary video file only once that has worked.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tracing::{error, info, warn};

use crate::audio::{AudioEvent, AudioFormat, AudioSource};
use crate::capture::{LossReason, ScreenEvent, ScreenSource, VideoFrame};
use crate::error::AppError;
use crate::recording::audio_timeline::{AudioTimeline, TimelineConfig, TimelineEvent};
use crate::recording::clock::{MediaTimeline, MonotonicClock};
use crate::recording::disk::{DiskEvent, DiskGuard, DiskThresholds};
use crate::recording::ffmpeg::{
    self, FfmpegError, FfmpegInfo, FinalizeInputs, H264Kind, SendOutcome, StreamEncoder,
    VideoParams,
};
use crate::recording::layout::{self, LayoutError, LocalDateTime};
use crate::recording::metadata::{
    AudioInfo, GapReason, MeetingMetadata, MetadataError, Mode, PauseEntry, StreamKind, VideoInfo,
    millis,
};
use crate::recording::state::{InvalidTransition, RecordingState, StateMachine};
use crate::recording::timefmt::rfc3339_utc;
use crate::storage::Database;
use crate::storage::meetings::{self, NewMeeting};
use crate::system::disk::DiskSpace;

/// Names of the files in a meeting folder.
pub mod names {
    pub const VIDEO_PART: &str = "video.part.mp4";
    pub const MICROPHONE: &str = "microphone.flac";
    pub const SYSTEM_AUDIO: &str = "system-audio.flac";
    pub const FINAL_VIDEO: &str = "recording.mp4";
    pub const FINAL_AUDIO: &str = "recording.m4a";
}

/// How long a worker keeps waiting for a running encoder to accept one item before giving up on it. Generous on
/// purpose: a hardware encoder can take seconds to start, or wait for another program's encoding session.
const ENCODER_STALL_LIMIT: Duration = Duration::from_secs(30);
/// Once the recording is stopping, how long a worker still waits to hand over the last items.
const ENCODER_STOP_GRACE: Duration = Duration::from_secs(10);
/// Chunks the audio queues can hold: about 40 s of 10 ms chunks, so only a dead encoder overflows them.
const AUDIO_QUEUE: usize = 4000;
const VIDEO_QUEUE: usize = 6;
const EVENT_QUEUE: usize = 256;
const FRAME_EVENT_QUEUE: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{0}")]
    State(#[from] InvalidTransition),
    #[error("{0}")]
    Layout(#[from] LayoutError),
    #[error("{0}")]
    Ffmpeg(#[from] FfmpegError),
    #[error("{0}")]
    Metadata(#[from] MetadataError),
    #[error("{0}")]
    Database(#[from] AppError),
    #[error("file error: {0}")]
    Io(#[from] io::Error),
    #[error("nothing could be recorded: no source started")]
    NothingToRecord,
    #[error(
        "the recording was stopped but could not be finished into one file ({0}); the tracks are kept in {1}"
    )]
    FinalizeFailed(String, PathBuf),
}

pub struct SessionConfig {
    pub title: String,
    pub mode: Mode,
    pub storage_folder: PathBuf,
    /// Local start time, used only for the folder name.
    pub folder_time: LocalDateTime,
    pub fps: u32,
    pub video_bitrate_bps: u32,
    pub app_version: String,
    pub disk_thresholds: DiskThresholds,
    pub disk_check_interval: Duration,
    /// How long to wait for each encoder to finish after capture stops.
    pub finish_timeout: Duration,
}

impl SessionConfig {
    pub fn new(title: &str, mode: Mode, storage_folder: &Path, folder_time: LocalDateTime) -> Self {
        Self {
            title: title.to_string(),
            mode,
            storage_folder: storage_folder.to_path_buf(),
            folder_time,
            fps: 15,
            video_bitrate_bps: 2_500_000,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            disk_thresholds: DiskThresholds::default(),
            disk_check_interval: Duration::from_secs(10),
            finish_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Default)]
pub struct SessionSources {
    pub screen: Option<Box<dyn ScreenSource>>,
    pub microphone: Option<Box<dyn AudioSource>>,
    pub system_audio: Option<Box<dyn AudioSource>>,
}

impl SessionSources {
    pub fn with_screen(mut self, source: impl ScreenSource + 'static) -> Self {
        self.screen = Some(Box::new(source));
        self
    }

    pub fn with_microphone(mut self, source: impl AudioSource + 'static) -> Self {
        self.microphone = Some(Box::new(source));
        self
    }

    pub fn with_system_audio(mut self, source: impl AudioSource + 'static) -> Self {
        self.system_audio = Some(Box::new(source));
        self
    }
}

pub struct SessionDeps {
    pub clock: Arc<dyn MonotonicClock>,
    pub ffmpeg: FfmpegInfo,
    pub video_encoder: H264Kind,
    pub disk: Arc<dyn DiskSpace>,
    pub database: Option<Arc<Database>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WarningKind {
    DiskLow,
    DiskCritical,
    SourceUnavailable,
    SourceLost,
    SourceRestored,
    EncoderFailed,
}

/// Something the user should be told about while recording.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub id: u64,
    pub kind: WarningKind,
    pub stream: Option<StreamKind>,
    pub message: String,
    pub at_media_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// Free disk space fell below the critical threshold. The owner must call `stop` to finish cleanly.
    DiskFull,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub state: RecordingState,
    pub elapsed_ms: u64,
    /// Peak level of the most recent chunk, 0.0 to 1.0.
    pub microphone_level: f32,
    pub system_audio_level: f32,
    pub warnings: Vec<Warning>,
    pub auto_stop: Option<StopReason>,
}

#[derive(Debug, Clone)]
pub struct SessionOutcome {
    pub meeting_id: Option<i64>,
    pub folder: PathBuf,
    /// `recording.mp4` or `recording.m4a`.
    pub file: PathBuf,
    pub duration: Duration,
}

/// State shared by the worker threads and the session handle.
struct Shared {
    clock: Arc<dyn MonotonicClock>,
    timeline: Mutex<MediaTimeline>,
    metadata: Mutex<MeetingMetadata>,
    folder: PathBuf,
    state: Mutex<StateMachine>,
    warnings: Mutex<Vec<Warning>>,
    auto_stop: Mutex<Option<StopReason>>,
    /// Set (before `stop_workers`) to the final media length.
    media_end: Mutex<Option<Duration>>,
    stop_workers: AtomicBool,
    microphone_level: AtomicU32,
    system_level: AtomicU32,
}

impl Shared {
    fn media_now(&self) -> Duration {
        self.timeline
            .lock()
            .unwrap()
            .media_length_at(self.clock.now())
    }

    /// Media time of an event stamped by a source. An event that falls in a pause is placed at the cut point.
    fn event_media_time(&self, at: Duration) -> Duration {
        let timeline = self.timeline.lock().unwrap();
        timeline
            .media_time(at)
            .unwrap_or_else(|| timeline.media_length_at(at))
    }

    fn save_metadata(&self) {
        let metadata = self.metadata.lock().unwrap();
        if let Err(e) = metadata.write(&self.folder) {
            error!(error = %e, "could not write meeting.json");
        }
    }

    fn warn(&self, kind: WarningKind, stream: Option<StreamKind>, message: &str) {
        let at = self.media_now();
        let mut warnings = self.warnings.lock().unwrap();
        let id = warnings.len() as u64 + 1;
        warnings.push(Warning {
            id,
            kind,
            stream,
            message: message.to_string(),
            at_media_ms: millis(at),
        });
    }

    fn record_error(&self, stream: Option<StreamKind>, kind: &str, message: &str) {
        let at = self.media_now();
        self.metadata
            .lock()
            .unwrap()
            .add_error(at, stream, kind, message);
        self.save_metadata();
    }

    fn add_gap(&self, stream: StreamKind, start: Duration, end: Duration, reason: GapReason) {
        self.metadata
            .lock()
            .unwrap()
            .add_gap(stream, start, end, reason);
        self.save_metadata();
    }

    fn level_cell(&self, stream: StreamKind) -> &AtomicU32 {
        if stream == StreamKind::Microphone {
            &self.microphone_level
        } else {
            &self.system_level
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Workers

struct VideoWorkerResult {
    encoder: StreamEncoder,
    ok: bool,
}

struct AudioWorkerResult {
    encoder: StreamEncoder,
    start_offset_ms: u64,
    drift_ppm: Option<f64>,
}

/// Hands data to an encoder, waiting while it is alive but busy. Gives up after `ENCODER_STALL_LIMIT` (or sooner,
/// `ENCODER_STOP_GRACE`, once the recording is stopping). Only a returned `Queued` means the data was accepted.
fn hand_over(
    encoder: &StreamEncoder,
    data: impl Into<Arc<Vec<u8>>>,
    shared: &Shared,
) -> SendOutcome {
    encoder.send_while(data, |waited| {
        let limit = if shared.stop_workers.load(Ordering::Acquire) {
            ENCODER_STOP_GRACE
        } else {
            ENCODER_STALL_LIMIT
        };
        waited < limit
    })
}

fn f32_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn video_worker(
    shared: Arc<Shared>,
    rx: Receiver<ScreenEvent>,
    encoder: StreamEncoder,
    width: u32,
    height: u32,
    fps: u32,
) -> VideoWorkerResult {
    let mut pacer: crate::recording::pacer::VideoPacer<VideoFrame> =
        crate::recording::pacer::VideoPacer::new(fps);
    let black = Arc::new(vec![0u8; (width * height * 4) as usize]);
    let mut ok = true;
    let mut failed_at: Option<Duration> = None;
    let mut reported_size_mismatch = false;
    let mut open_gap: Option<(Duration, GapReason)> = None;

    let mut handle_event =
        |event: ScreenEvent, pacer: &mut crate::recording::pacer::VideoPacer<VideoFrame>| {
            match event {
                ScreenEvent::Frame(frame) => {
                    if frame.width != width || frame.height != height {
                        if !reported_size_mismatch {
                            reported_size_mismatch = true;
                            shared.record_error(
                        Some(StreamKind::Video),
                        "frameSizeChanged",
                        "The monitor changed size during the recording; frames of the new size are not recorded.",
                    );
                        }
                        return;
                    }
                    if let Some(media_time) = shared
                        .timeline
                        .lock()
                        .unwrap()
                        .media_time(frame.captured_at)
                    {
                        pacer.push_frame(media_time, frame);
                    }
                }
                ScreenEvent::Lost { reason, at } => {
                    let (gap_reason, text) = match reason {
                        LossReason::ScreenLocked => (
                            GapReason::ScreenLocked,
                            "The screen is locked; video is black until it is unlocked.",
                        ),
                        LossReason::MonitorLost => (
                            GapReason::MonitorLost,
                            "The monitor was disconnected; video is black until it returns.",
                        ),
                    };
                    let start = shared.event_media_time(at);
                    pacer.begin_gap(start);
                    open_gap = Some((start, gap_reason));
                    shared.warn(WarningKind::SourceLost, Some(StreamKind::Video), text);
                }
                ScreenEvent::Restored { at } => {
                    let end = shared.event_media_time(at);
                    pacer.end_gap(end);
                    if let Some((start, reason)) = open_gap.take() {
                        shared.add_gap(StreamKind::Video, start, end, reason);
                    }
                    shared.warn(
                        WarningKind::SourceRestored,
                        Some(StreamKind::Video),
                        "Video capture resumed.",
                    );
                }
            }
        };

    loop {
        let stopping = shared.stop_workers.load(Ordering::Acquire);
        match rx.recv_timeout(Duration::from_millis(15)) {
            Ok(event) => handle_event(event, &mut pacer),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {}
        }
        while let Ok(event) = rx.try_recv() {
            handle_event(event, &mut pacer);
        }

        let media_now = if stopping {
            shared
                .media_end
                .lock()
                .unwrap()
                .unwrap_or_else(|| shared.media_now())
        } else {
            shared.media_now()
        };
        let first_slot = pacer.slots_emitted();
        for (i, slot) in pacer.advance_to(media_now).into_iter().enumerate() {
            if !ok {
                continue;
            }
            let bytes = match slot {
                crate::recording::pacer::Slot::Frame(frame) => frame.bgra,
                crate::recording::pacer::Slot::Black => black.clone(),
            };
            if hand_over(&encoder, bytes, &shared) != SendOutcome::Queued {
                ok = false;
                // Video is missing from this slot on (the frames before it were accepted).
                failed_at = Some(Duration::from_secs_f64(
                    (first_slot + i as u64) as f64 / fps as f64,
                ));
                shared.record_error(
                    Some(StreamKind::Video),
                    "encoderFailed",
                    "The video encoder stopped accepting frames; video after this point is missing.",
                );
                shared.warn(
                    WarningKind::EncoderFailed,
                    Some(StreamKind::Video),
                    "The video encoder failed.",
                );
            }
        }
        if stopping {
            break;
        }
    }

    let end = shared.media_end.lock().unwrap().unwrap_or_default();
    if let Some((start, reason)) = open_gap.take() {
        shared.add_gap(StreamKind::Video, start, end.max(start), reason);
    }
    if let Some(start) = failed_at {
        shared.add_gap(
            StreamKind::Video,
            start,
            end.max(start),
            GapReason::EncoderFailed,
        );
    }
    VideoWorkerResult { encoder, ok }
}

fn audio_worker(
    shared: Arc<Shared>,
    stream: StreamKind,
    rx: Receiver<AudioEvent>,
    encoder: StreamEncoder,
    format: AudioFormat,
) -> AudioWorkerResult {
    let mut timeline = AudioTimeline::new(TimelineConfig::new(format.sample_rate, format.channels));
    let mut ok = true;
    let mut lost_since: Option<Duration> = None;
    // Set when the device comes back, until the first chunk after it has been placed: the silence that fills the
    // outage up to that chunk belongs to the same device loss.
    let mut filling_after_loss = false;
    let mut first_chunk = true;
    let mut start_offset_ms = 0u64;

    let write = |samples: Vec<f32>, ok: &mut bool, shared: &Shared| {
        if samples.is_empty() || !*ok {
            return;
        }
        if hand_over(&encoder, f32_le_bytes(&samples), shared) != SendOutcome::Queued {
            *ok = false;
            let now = shared.media_now();
            shared.record_error(
                Some(stream),
                "encoderFailed",
                "The audio encoder stopped accepting data; audio after this point is missing.",
            );
            let end = shared.media_end.lock().unwrap().unwrap_or(now).max(now);
            shared.add_gap(stream, now, end, GapReason::EncoderFailed);
            shared.warn(
                WarningKind::EncoderFailed,
                Some(stream),
                "An audio encoder failed.",
            );
        }
    };

    let report_events =
        |timeline: &mut AudioTimeline, lost: bool, shared: &Shared, leading: bool| {
            for event in timeline.take_events() {
                match event {
                    // Silence before the first data is device start-up latency. It is recorded as startOffsetMs, and
                    // only listed as a gap if it is long enough to mean something was wrong.
                    TimelineEvent::Gap { at, length }
                        if leading && at.is_zero() && length < Duration::from_secs(1) => {}
                    TimelineEvent::Gap { at, length } => {
                        let reason = if lost {
                            GapReason::DeviceLost
                        } else {
                            GapReason::SourceStalled
                        };
                        shared.add_gap(stream, at, at + length, reason);
                    }
                    TimelineEvent::Overlap { at, length } => shared.record_error(
                        Some(stream),
                        "audioOverlap",
                        &format!(
                            "The device repeated {} ms of audio at {} ms; the repeat was dropped.",
                            length.as_millis(),
                            at.as_millis()
                        ),
                    ),
                }
            }
        };

    let mut process = |event: AudioEvent,
                       timeline: &mut AudioTimeline,
                       ok: &mut bool,
                       lost_since: &mut Option<Duration>,
                       filling_after_loss: &mut bool| {
        match event {
            AudioEvent::Chunk(chunk) => {
                let Some(media_start) = shared
                    .timeline
                    .lock()
                    .unwrap()
                    .media_time(chunk.captured_at)
                else {
                    return; // captured during a pause (or before the start): not part of the recording
                };
                let peak = chunk
                    .samples
                    .iter()
                    .fold(0.0f32, |m, s| m.max(s.abs()))
                    .min(1.0);
                shared
                    .level_cell(stream)
                    .store(peak.to_bits(), Ordering::Relaxed);
                let out = timeline.place(media_start, &chunk.samples);
                let leading = first_chunk;
                if first_chunk {
                    first_chunk = false;
                    start_offset_ms = (timeline.silent_frames_filled() as f64 * 1000.0
                        / format.sample_rate as f64) as u64;
                }
                report_events(
                    timeline,
                    lost_since.is_some() || *filling_after_loss,
                    &shared,
                    leading,
                );
                *filling_after_loss = false;
                write(out, ok, &shared);
            }
            AudioEvent::DeviceLost { reason, at } => {
                if lost_since.is_none() {
                    *lost_since = Some(shared.event_media_time(at));
                    warn!(?stream, "audio device lost");
                    shared.record_error(Some(stream), "deviceLost", &reason);
                    shared.warn(
                        WarningKind::SourceLost,
                        Some(stream),
                        "An audio device was disconnected; this track is silent until it returns.",
                    );
                }
            }
            AudioEvent::DeviceRestored { .. } => {
                if lost_since.take().is_some() {
                    *filling_after_loss = true;
                    shared.warn(
                        WarningKind::SourceRestored,
                        Some(stream),
                        "The audio device is back.",
                    );
                }
            }
        }
    };

    loop {
        let stopping = shared.stop_workers.load(Ordering::Acquire);
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => process(
                event,
                &mut timeline,
                &mut ok,
                &mut lost_since,
                &mut filling_after_loss,
            ),
            Err(RecvTimeoutError::Timeout) => {
                // A source that delivers nothing for a while (a quiet loopback, a stalled device) still owes the
                // timeline its silence, or this track would end up shorter than the others.
                if !stopping {
                    let media_now = shared.media_now();
                    let behind = media_now.saturating_sub(timeline.written_duration());
                    if behind > Duration::from_secs(1) {
                        let pad =
                            timeline.pad_to(media_now.saturating_sub(Duration::from_millis(200)));
                        report_events(
                            &mut timeline,
                            lost_since.is_some() || filling_after_loss,
                            &shared,
                            false,
                        );
                        write(pad, &mut ok, &shared);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {}
        }
        if stopping {
            while let Ok(event) = rx.try_recv() {
                process(
                    event,
                    &mut timeline,
                    &mut ok,
                    &mut lost_since,
                    &mut filling_after_loss,
                );
            }
            let end = shared
                .media_end
                .lock()
                .unwrap()
                .unwrap_or_else(|| shared.media_now());
            let pad = timeline.pad_to(end);
            report_events(
                &mut timeline,
                lost_since.is_some() || filling_after_loss,
                &shared,
                false,
            );
            write(pad, &mut ok, &shared);
            break;
        }
    }

    if let Some(start) = lost_since.take() {
        let end = shared.media_end.lock().unwrap().unwrap_or(start);
        shared.add_gap(stream, start, end, GapReason::DeviceLost);
    }
    AudioWorkerResult {
        encoder,
        start_offset_ms,
        drift_ppm: timeline.drift_ppm(),
    }
}

fn disk_watcher(
    shared: Arc<Shared>,
    disk: Arc<dyn DiskSpace>,
    folder: PathBuf,
    thresholds: DiskThresholds,
    interval: Duration,
) {
    struct Shim(Arc<dyn DiskSpace>);
    impl DiskSpace for Shim {
        fn available_bytes(&self, path: &Path) -> io::Result<u64> {
            self.0.available_bytes(path)
        }
    }
    let mut guard = DiskGuard::new(Shim(disk), folder, thresholds);
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(50).min(interval);
    while !shared.stop_workers.load(Ordering::Acquire) {
        std::thread::sleep(step);
        waited += step;
        if waited < interval {
            continue;
        }
        waited = Duration::ZERO;
        match guard.check() {
            Ok(DiskEvent::None) => {}
            Ok(DiskEvent::Low { available }) => {
                shared.warn(WarningKind::DiskLow, None, "Disk space is running low.");
                shared.record_error(
                    None,
                    "diskLow",
                    &format!("{} MB of disk space left.", available / (1024 * 1024)),
                );
            }
            Ok(DiskEvent::Critical { available }) => {
                let mut auto_stop = shared.auto_stop.lock().unwrap();
                if auto_stop.is_none() {
                    *auto_stop = Some(StopReason::DiskFull);
                    drop(auto_stop);
                    shared.warn(
                        WarningKind::DiskCritical,
                        None,
                        "Disk space is critically low; the recording must stop.",
                    );
                    shared.record_error(
                        None,
                        "diskCritical",
                        &format!("{} MB of disk space left.", available / (1024 * 1024)),
                    );
                }
            }
            Err(e) => warn!(error = %e, "could not read free disk space"),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The session handle

enum SourceSlot {
    Screen(Box<dyn ScreenSource>, SyncSender<ScreenEvent>),
    Audio(StreamKind, Box<dyn AudioSource>, SyncSender<AudioEvent>),
}

/// A source that has been opened and has its encoder running, but is not delivering yet.
enum Prepared {
    Video(PreparedVideo),
    Audio(PreparedAudio),
}

struct PreparedVideo {
    source: Box<dyn ScreenSource>,
    encoder: StreamEncoder,
    tx: SyncSender<ScreenEvent>,
    rx: Receiver<ScreenEvent>,
    monitor: crate::capture::MonitorInfo,
    output: PathBuf,
}

struct PreparedAudio {
    stream: StreamKind,
    source: Box<dyn AudioSource>,
    encoder: StreamEncoder,
    tx: SyncSender<AudioEvent>,
    rx: Receiver<AudioEvent>,
    format: AudioFormat,
    output: PathBuf,
}

pub struct Session {
    shared: Arc<Shared>,
    config: SessionConfig,
    deps: SessionDeps,
    sources: Vec<SourceSlot>,
    video_worker: Option<JoinHandle<VideoWorkerResult>>,
    audio_workers: Vec<(StreamKind, JoinHandle<AudioWorkerResult>)>,
    disk_thread: Option<JoinHandle<()>>,
    meeting_id: Option<i64>,
    video_info: Option<VideoInfo>,
    audio_infos: Vec<(StreamKind, String, AudioFormat)>,
    finished: bool,
}

impl Session {
    /// Creates the meeting folder, starts the encoders and sources, and begins recording. A source that cannot
    /// start does not stop the recording: it is reported as a warning and the others carry on. If nothing can start,
    /// the folder is removed and an error is returned.
    pub fn start(
        config: SessionConfig,
        sources: SessionSources,
        deps: SessionDeps,
    ) -> Result<Session, SessionError> {
        let mut machine = StateMachine::new();
        machine.transition(RecordingState::Starting)?;

        let folder = layout::create_meeting_folder(
            &config.storage_folder,
            &config.folder_time,
            &config.title,
        )?;
        let started_at = rfc3339_utc(SystemTime::now());
        let mut metadata = MeetingMetadata::new(
            &config.app_version,
            &config.title,
            config.mode,
            started_at.clone(),
        );

        let meeting_id = match &deps.database {
            Some(db) => {
                let mode = if config.mode == Mode::ScreenAudio {
                    "screen_audio"
                } else {
                    "audio_only"
                };
                let folder_text = folder.to_string_lossy().into_owned();
                let created = db.with_conn(|conn| {
                    meetings::create(
                        conn,
                        &NewMeeting {
                            title: &config.title,
                            started_at: &started_at,
                            mode,
                            folder_path: &folder_text,
                            capture_settings: None,
                        },
                    )
                });
                match created {
                    Ok(id) => Some(id),
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&folder);
                        return Err(e.into());
                    }
                }
            }
            None => None,
        };
        metadata.meeting_id = meeting_id;

        let timeline = MediaTimeline::start(deps.clock.now());
        let shared = Arc::new(Shared {
            clock: deps.clock.clone(),
            timeline: Mutex::new(timeline),
            metadata: Mutex::new(metadata),
            folder: folder.clone(),
            state: Mutex::new(machine),
            warnings: Mutex::new(Vec::new()),
            auto_stop: Mutex::new(None),
            media_end: Mutex::new(None),
            stop_workers: AtomicBool::new(false),
            microphone_level: AtomicU32::new(0),
            system_level: AtomicU32::new(0),
        });

        let mut session = Session {
            shared: shared.clone(),
            config,
            deps,
            sources: Vec::new(),
            video_worker: None,
            audio_workers: Vec::new(),
            disk_thread: None,
            meeting_id,
            video_info: None,
            audio_infos: Vec::new(),
            finished: false,
        };

        let SessionSources {
            screen,
            microphone,
            system_audio,
        } = sources;

        // A: open the devices and start the encoders. Nothing is captured yet, so slow work (opening a device,
        // launching FFmpeg) cannot show up as lost time at the start of the recording.
        let mut prepared = Vec::new();
        if let Some(screen) = screen
            && let Some(p) = session.prepare_video(screen)
        {
            prepared.push(p);
        }
        if let Some(mic) = microphone
            && let Some(p) = session.prepare_audio(StreamKind::Microphone, mic, names::MICROPHONE)
        {
            prepared.push(p);
        }
        if let Some(system) = system_audio
            && let Some(p) =
                session.prepare_audio(StreamKind::SystemAudio, system, names::SYSTEM_AUDIO)
        {
            prepared.push(p);
        }

        // B: the recording clock starts now. C: every source starts, back to back. D: the workers take over.
        *shared.timeline.lock().unwrap() = MediaTimeline::start(session.deps.clock.now());
        let mut running = Vec::new();
        for p in prepared {
            if let Some(p) = session.start_prepared(p) {
                running.push(p);
            }
        }
        for p in running {
            session.spawn_worker(p)?;
        }

        if session.sources.is_empty() {
            let _ = session.discard();
            return Err(SessionError::NothingToRecord);
        }

        {
            let mut metadata = shared.metadata.lock().unwrap();
            metadata.video = session.video_info.clone();
            for (stream, device, format) in &session.audio_infos {
                let info = AudioInfo {
                    device: device.clone(),
                    sample_rate: format.sample_rate,
                    channels: format.channels,
                    start_offset_ms: 0,
                    drift_ppm: None,
                };
                if *stream == StreamKind::Microphone {
                    metadata.microphone = Some(info);
                } else {
                    metadata.system_audio = Some(info);
                }
            }
        }
        shared.save_metadata();

        let disk = session.deps.disk.clone();
        let (shared_for_disk, folder_for_disk) =
            (shared.clone(), session.config.storage_folder.clone());
        let (thresholds, interval) = (
            session.config.disk_thresholds,
            session.config.disk_check_interval,
        );
        session.disk_thread = Some(
            std::thread::Builder::new()
                .name("disk-watcher".into())
                .spawn(move || {
                    disk_watcher(shared_for_disk, disk, folder_for_disk, thresholds, interval)
                })
                .map_err(SessionError::Io)?,
        );

        shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Recording)?;
        info!(meeting_id = ?meeting_id, "recording started");
        Ok(session)
    }

    /// Opens the screen and starts the video encoder. `None` (with a warning) if either cannot start.
    fn prepare_video(&mut self, mut screen: Box<dyn ScreenSource>) -> Option<Prepared> {
        let monitor = screen.monitor();
        if let Err(e) = screen.prepare() {
            self.source_unavailable(
                StreamKind::Video,
                &format!("The screen could not be captured: {e}"),
            );
            return None;
        }
        let output = self.shared.folder.join(names::VIDEO_PART);
        let params = VideoParams {
            width: monitor.width,
            height: monitor.height,
            fps: self.config.fps,
            bitrate_bps: self.config.video_bitrate_bps,
        };
        let args = ffmpeg::video_args(self.deps.video_encoder, &params, &output);
        let encoder =
            match StreamEncoder::spawn("video", &self.deps.ffmpeg.path, &args, VIDEO_QUEUE) {
                Ok(encoder) => encoder,
                Err(e) => {
                    self.source_unavailable(
                        StreamKind::Video,
                        &format!("The video encoder could not start: {e}"),
                    );
                    return None;
                }
            };
        let (tx, rx) = sync_channel(FRAME_EVENT_QUEUE);
        Some(Prepared::Video(PreparedVideo {
            source: screen,
            encoder,
            tx,
            rx,
            monitor,
            output,
        }))
    }

    /// Opens the audio device and starts its FLAC encoder. `None` (with a warning) if either cannot start.
    fn prepare_audio(
        &mut self,
        stream: StreamKind,
        mut source: Box<dyn AudioSource>,
        file_name: &str,
    ) -> Option<Prepared> {
        if let Err(e) = source.prepare() {
            self.source_unavailable(
                stream,
                &format!("The audio device could not be opened: {e}"),
            );
            return None;
        }
        let format = source.format();
        let output = self.shared.folder.join(file_name);
        let args = ffmpeg::audio_track_args(format.sample_rate, format.channels, &output);
        let encoder =
            match StreamEncoder::spawn(file_name, &self.deps.ffmpeg.path, &args, AUDIO_QUEUE) {
                Ok(encoder) => encoder,
                Err(e) => {
                    self.source_unavailable(
                        stream,
                        &format!("The audio encoder could not start: {e}"),
                    );
                    return None;
                }
            };
        let (tx, rx) = sync_channel(EVENT_QUEUE);
        Some(Prepared::Audio(PreparedAudio {
            stream,
            source,
            encoder,
            tx,
            rx,
            format,
            output,
        }))
    }

    /// Starts delivery from a prepared source. On failure the stream's encoder is stopped and its empty file removed.
    fn start_prepared(&mut self, prepared: Prepared) -> Option<Prepared> {
        let clock = self.deps.clock.clone();
        match prepared {
            Prepared::Video(mut v) => match v.source.start(clock, v.tx.clone()) {
                Ok(()) => Some(Prepared::Video(v)),
                Err(e) => {
                    v.encoder.abort();
                    let _ = std::fs::remove_file(&v.output);
                    self.source_unavailable(
                        StreamKind::Video,
                        &format!("The screen could not be captured: {e}"),
                    );
                    None
                }
            },
            Prepared::Audio(mut a) => match a.source.start(clock, a.tx.clone()) {
                Ok(()) => Some(Prepared::Audio(a)),
                Err(e) => {
                    a.encoder.abort();
                    let _ = std::fs::remove_file(&a.output);
                    self.source_unavailable(
                        a.stream,
                        &format!("The audio device could not be opened: {e}"),
                    );
                    None
                }
            },
        }
    }

    /// Hands a running source's channel and encoder to a worker thread and keeps the source for pause and stop.
    fn spawn_worker(&mut self, prepared: Prepared) -> Result<(), io::Error> {
        let shared = self.shared.clone();
        match prepared {
            Prepared::Video(v) => {
                self.video_info = Some(VideoInfo {
                    monitor: v.monitor.id.clone(),
                    width: v.monitor.width,
                    height: v.monitor.height,
                    fps: self.config.fps,
                    encoder: self.deps.video_encoder.label().to_string(),
                });
                let (rx, encoder, fps) = (v.rx, v.encoder, self.config.fps);
                let (width, height) = (v.monitor.width, v.monitor.height);
                self.video_worker = Some(
                    std::thread::Builder::new()
                        .name("video-worker".into())
                        .spawn(move || video_worker(shared, rx, encoder, width, height, fps))?,
                );
                self.sources.push(SourceSlot::Screen(v.source, v.tx));
            }
            Prepared::Audio(a) => {
                self.audio_infos
                    .push((a.stream, a.source.device_name(), a.format));
                let (stream, rx, encoder, format) = (a.stream, a.rx, a.encoder, a.format);
                self.audio_workers.push((
                    stream,
                    std::thread::Builder::new()
                        .name(format!("{stream:?}-worker"))
                        .spawn(move || audio_worker(shared, stream, rx, encoder, format))?,
                ));
                self.sources.push(SourceSlot::Audio(stream, a.source, a.tx));
            }
        }
        Ok(())
    }

    fn source_unavailable(&self, stream: StreamKind, message: &str) {
        warn!(?stream, "a source could not start");
        self.shared
            .record_error(Some(stream), "sourceUnavailable", message);
        self.shared
            .warn(WarningKind::SourceUnavailable, Some(stream), message);
    }

    pub fn state(&self) -> RecordingState {
        self.shared.state.lock().unwrap().state()
    }

    pub fn folder(&self) -> &Path {
        &self.shared.folder
    }

    pub fn meeting_id(&self) -> Option<i64> {
        self.meeting_id
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state(),
            elapsed_ms: millis(self.shared.media_now()),
            microphone_level: f32::from_bits(self.shared.microphone_level.load(Ordering::Relaxed)),
            system_audio_level: f32::from_bits(self.shared.system_level.load(Ordering::Relaxed)),
            warnings: self.shared.warnings.lock().unwrap().clone(),
            auto_stop: *self.shared.auto_stop.lock().unwrap(),
        }
    }

    fn stop_sources(&mut self) {
        for slot in &mut self.sources {
            match slot {
                SourceSlot::Screen(source, _) => source.stop(),
                SourceSlot::Audio(_, source, _) => source.stop(),
            }
        }
    }

    /// Pauses: every source is stopped (so nothing is captured) and the paused time is cut from the recording.
    pub fn pause(&mut self) -> Result<(), SessionError> {
        self.shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Paused)?;
        let now = self.deps.clock.now();
        self.shared
            .timeline
            .lock()
            .unwrap()
            .pause(now)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.stop_sources();
        self.write_pauses();
        Ok(())
    }

    /// Resumes: the timeline continues at the cut point and every source is started again. A source that cannot be
    /// restarted is reported and the recording continues without it.
    pub fn resume(&mut self) -> Result<(), SessionError> {
        self.shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Recording)?;
        let now = self.deps.clock.now();
        self.shared
            .timeline
            .lock()
            .unwrap()
            .resume(now)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let clock = self.deps.clock.clone();
        for slot in &mut self.sources {
            let (stream, result) = match slot {
                SourceSlot::Screen(source, tx) => (
                    StreamKind::Video,
                    source
                        .start(clock.clone(), tx.clone())
                        .map_err(|e| e.to_string()),
                ),
                SourceSlot::Audio(stream, source, tx) => (
                    *stream,
                    source
                        .start(clock.clone(), tx.clone())
                        .map_err(|e| e.to_string()),
                ),
            };
            if let Err(message) = result {
                self.shared
                    .record_error(Some(stream), "resumeFailed", &message);
                self.shared.warn(
                    WarningKind::SourceLost,
                    Some(stream),
                    "A device could not be restarted after the pause.",
                );
            }
        }
        self.write_pauses();
        Ok(())
    }

    fn write_pauses(&self) {
        let now = self.deps.clock.now();
        let records = self.shared.timeline.lock().unwrap().pause_records(now);
        self.shared.metadata.lock().unwrap().pauses = records
            .iter()
            .map(|r| PauseEntry {
                at_media_ms: millis(r.at_media),
                wall_length_ms: millis(r.wall_length),
            })
            .collect();
        self.shared.save_metadata();
    }

    /// Ends capture and joins the workers. Returns the encoders to finish, plus what each worker learned.
    fn end_capture(
        &mut self,
    ) -> (
        Option<VideoWorkerResult>,
        Vec<(StreamKind, AudioWorkerResult)>,
        Duration,
    ) {
        let media_end = self.shared.media_now();
        *self.shared.media_end.lock().unwrap() = Some(media_end);
        self.stop_sources();
        self.shared.stop_workers.store(true, Ordering::Release);
        if let Some(thread) = self.disk_thread.take() {
            let _ = thread.join();
        }
        let video = self.video_worker.take().and_then(|t| t.join().ok());
        let audio = self
            .audio_workers
            .drain(..)
            .filter_map(|(stream, t)| t.join().ok().map(|r| (stream, r)))
            .collect();
        (video, audio, media_end)
    }

    /// Stops the recording and produces the final file.
    pub fn stop(mut self) -> Result<SessionOutcome, SessionError> {
        self.shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Stopping)?;
        let (video, audio, media_end) = self.end_capture();
        let folder = self.shared.folder.clone();
        let timeout = self.config.finish_timeout;

        // Close every encoder so its file is complete.
        let mut video_file_ok = false;
        if let Some(result) = video {
            let ok = result.ok;
            match result.encoder.finish(timeout) {
                Ok(()) => video_file_ok = ok,
                Err(e) => self.shared.record_error(
                    Some(StreamKind::Video),
                    "encoderExited",
                    &e.to_string(),
                ),
            }
        }
        let mut audio_ok = Vec::new();
        for (stream, result) in audio {
            {
                let mut metadata = self.shared.metadata.lock().unwrap();
                let slot = if stream == StreamKind::Microphone {
                    &mut metadata.microphone
                } else {
                    &mut metadata.system_audio
                };
                if let Some(info) = slot {
                    info.start_offset_ms = result.start_offset_ms;
                    info.drift_ppm = result.drift_ppm;
                }
            }
            match result.encoder.finish(timeout) {
                Ok(()) => audio_ok.push(stream),
                Err(e) => self
                    .shared
                    .record_error(Some(stream), "encoderExited", &e.to_string()),
            }
        }

        self.shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Finalizing)?;
        self.set_db_status("finalizing");
        self.write_pauses();

        let video_path = folder.join(names::VIDEO_PART);
        let mic_path = folder.join(names::MICROPHONE);
        let system_path = folder.join(names::SYSTEM_AUDIO);
        let use_video = video_file_ok && video_path.is_file();
        let use_mic = audio_ok.contains(&StreamKind::Microphone) && mic_path.is_file();
        let use_system = audio_ok.contains(&StreamKind::SystemAudio) && system_path.is_file();
        let output = folder.join(if use_video {
            names::FINAL_VIDEO
        } else {
            names::FINAL_AUDIO
        });

        if !use_video && !use_mic && !use_system {
            return self.fail_finalize("no track was recorded successfully", &folder);
        }
        let mux = ffmpeg::run_final_mux(
            &self.deps.ffmpeg.path,
            &FinalizeInputs {
                video: use_video.then_some(video_path.as_path()),
                microphone: use_mic.then_some(mic_path.as_path()),
                system_audio: use_system.then_some(system_path.as_path()),
                output: &output,
                trim_to: Some(media_end),
            },
            |_| {},
        );
        if let Err(e) = mux {
            return self.fail_finalize(&e.to_string(), &folder);
        }
        if use_video {
            let _ = std::fs::remove_file(&video_path);
        }

        let ended_at = rfc3339_utc(SystemTime::now());
        {
            let mut metadata = self.shared.metadata.lock().unwrap();
            metadata.status = "complete".into();
            metadata.ended_at = Some(ended_at.clone());
            metadata.duration_ms = Some(millis(media_end));
        }
        self.shared.save_metadata();
        if let (Some(db), Some(id)) = (&self.deps.database, self.meeting_id)
            && let Err(e) = db.with_conn(|c| {
                meetings::finish(c, id, "complete", &ended_at, millis(media_end) as i64)
            })
        {
            error!(error = %e, "could not update the meeting row");
        }
        self.shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Complete)?;
        self.finished = true;
        info!(meeting_id = ?self.meeting_id, "recording finished");
        Ok(SessionOutcome {
            meeting_id: self.meeting_id,
            folder,
            file: output,
            duration: media_end,
        })
    }

    fn fail_finalize(
        &mut self,
        reason: &str,
        folder: &Path,
    ) -> Result<SessionOutcome, SessionError> {
        self.shared.record_error(None, "finalizeFailed", reason);
        {
            let mut metadata = self.shared.metadata.lock().unwrap();
            metadata.status = "finalizing".into();
        }
        self.shared.save_metadata();
        // The database keeps "finalizing" so that the next launch's recovery tries again with the tracks that are here.
        let _ = self
            .shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Failed);
        self.finished = true;
        Err(SessionError::FinalizeFailed(
            reason.to_string(),
            folder.to_path_buf(),
        ))
    }

    fn set_db_status(&self, status: &str) {
        if let (Some(db), Some(id)) = (&self.deps.database, self.meeting_id)
            && let Err(e) = db.with_conn(|c| meetings::set_status(c, id, status))
        {
            error!(error = %e, "could not update the meeting status");
        }
    }

    /// Discards the recording: stops everything and deletes the meeting folder and its database row.
    pub fn cancel(mut self) -> Result<(), SessionError> {
        self.shared
            .state
            .lock()
            .unwrap()
            .transition(RecordingState::Cancelled)?;
        self.discard()
    }

    fn discard(&mut self) -> Result<(), SessionError> {
        let (video, audio, _) = self.end_capture();
        if let Some(result) = video {
            result.encoder.abort();
        }
        for (_, result) in audio {
            result.encoder.abort();
        }
        // Only ever delete the folder this session created, and only directly inside the storage folder.
        let folder = self.shared.folder.clone();
        if folder.parent() == Some(self.config.storage_folder.as_path()) {
            layout::remove_folder(&folder)?;
        }
        if let (Some(db), Some(id)) = (&self.deps.database, self.meeting_id) {
            db.with_conn(|c| meetings::delete(c, id))?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.finished {
            // Dropped while recording (a panic, or the app closing): stop capture and let the workers exit. Their
            // encoders are killed when dropped, leaving recoverable files, exactly as a crash would.
            self.shared.stop_workers.store(true, Ordering::Release);
            self.stop_sources();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_written_as_little_endian_f32() {
        let bytes = f32_le_bytes(&[1.0, -0.5]);

        assert_eq!(bytes, [0, 0, 0x80, 0x3F, 0, 0, 0, 0xBF]);
    }

    #[test]
    fn the_file_names_are_the_documented_ones() {
        assert_eq!(names::FINAL_VIDEO, "recording.mp4");
        assert_eq!(names::FINAL_AUDIO, "recording.m4a");
        assert_eq!(names::MICROPHONE, "microphone.flac");
        assert_eq!(names::SYSTEM_AUDIO, "system-audio.flac");
        assert_eq!(crate::recording::metadata::FILE_NAME, "meeting.json");
    }
}
