//! `meeting.json`: what happened during a recording (SPEC Section 7). Documented in `docs/meeting-json.md`.
//!
//! The file is rewritten as the recording goes (atomically: write a temporary file, then rename it over the old one),
//! so after a crash it still describes everything up to the last update and recovery can read it.
//! It never contains transcript text.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const FILE_NAME: &str = "meeting.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    ScreenAudio,
    AudioOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamKind {
    Video,
    Microphone,
    SystemAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GapReason {
    /// A microphone, headset or output device went away.
    DeviceLost,
    /// The workstation was locked.
    ScreenLocked,
    /// The recorded monitor was disconnected.
    MonitorLost,
    /// The source stopped delivering data without saying why; silence or black was inserted.
    SourceStalled,
    /// Video frames were dropped because the encoder could not keep up.
    DroppedFrames,
    /// The encoder for this stream failed.
    EncoderFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub monitor: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub encoder: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Silence inserted at the start because the device delivered its first data late (device start-up).
    pub start_offset_ms: u64,
    /// Measured device clock error against the system clock, once known.
    pub drift_ppm: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseEntry {
    /// Where in the saved media the pause happened.
    pub at_media_ms: u64,
    /// How long the pause lasted. This time is not in the files.
    pub wall_length_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GapEntry {
    pub stream: StreamKind,
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: GapReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorEntry {
    pub at_media_ms: u64,
    pub stream: Option<StreamKind>,
    /// A short stable code, for example `encoderExited`.
    pub kind: String,
    /// A human-readable message. Never transcript text.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingMetadata {
    pub schema_version: u32,
    pub app_version: String,
    /// The row id in the app database, once it exists.
    pub meeting_id: Option<i64>,
    pub title: String,
    pub mode: Mode,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub ended_at: Option<String>,
    /// Length of the saved media (paused time excluded).
    pub duration_ms: Option<u64>,
    /// `recording`, `finalizing`, `complete`, `failed` or `cancelled`.
    pub status: String,
    /// True if the app found this recording unfinished after a crash and recovered it.
    pub recovered: bool,
    pub video: Option<VideoInfo>,
    pub microphone: Option<AudioInfo>,
    pub system_audio: Option<AudioInfo>,
    pub pauses: Vec<PauseEntry>,
    pub gaps: Vec<GapEntry>,
    pub errors: Vec<ErrorEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetadataError {
    #[error("could not read or write {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0} is not valid meeting metadata: {1}")]
    Invalid(PathBuf, serde_json::Error),
    #[error(
        "{0} was written by a newer version of MeetingMind (schema {1}; this build understands up to {SCHEMA_VERSION})"
    )]
    NewerSchema(PathBuf, u32),
}

pub fn millis(d: Duration) -> u64 {
    d.as_millis() as u64
}

impl MeetingMetadata {
    pub fn new(app_version: &str, title: &str, mode: Mode, started_at: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            app_version: app_version.to_string(),
            meeting_id: None,
            title: title.to_string(),
            mode,
            started_at,
            ended_at: None,
            duration_ms: None,
            status: "recording".to_string(),
            recovered: false,
            video: None,
            microphone: None,
            system_audio: None,
            pauses: Vec::new(),
            gaps: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Records a gap. A gap that begins where the previous gap of the same stream and reason ended (within 50 ms)
    /// extends it, so one outage is one entry however many pieces it was reported in.
    pub fn add_gap(
        &mut self,
        stream: StreamKind,
        start: Duration,
        end: Duration,
        reason: GapReason,
    ) {
        let (start_ms, end_ms) = (millis(start), millis(end.max(start)));
        if let Some(previous) = self.gaps.iter_mut().rev().find(|g| g.stream == stream)
            && previous.reason == reason
            && start_ms <= previous.end_ms + 50
        {
            previous.end_ms = previous.end_ms.max(end_ms);
            return;
        }
        self.gaps.push(GapEntry {
            stream,
            start_ms,
            end_ms,
            reason,
        });
    }

    pub fn add_error(
        &mut self,
        at: Duration,
        stream: Option<StreamKind>,
        kind: &str,
        message: &str,
    ) {
        self.errors.push(ErrorEntry {
            at_media_ms: millis(at),
            stream,
            kind: kind.to_string(),
            message: message.to_string(),
        });
    }

    /// Writes `meeting.json` in `folder` atomically: the old file is replaced only once the new one is on disk.
    pub fn write(&self, folder: &Path) -> Result<(), MetadataError> {
        let final_path = folder.join(FILE_NAME);
        let temp_path = folder.join(format!("{FILE_NAME}.tmp"));
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |e| MetadataError::Io(path.clone(), e)
        };

        let json =
            serde_json::to_vec_pretty(self).expect("metadata is plain data and always serializes");
        let mut file = fs::File::create(&temp_path).map_err(io(&temp_path))?;
        file.write_all(&json).map_err(io(&temp_path))?;
        file.sync_all().map_err(io(&temp_path))?;
        drop(file);
        fs::rename(&temp_path, &final_path).map_err(io(&final_path))?;
        Ok(())
    }

    pub fn read(folder: &Path) -> Result<Self, MetadataError> {
        let path = folder.join(FILE_NAME);
        let bytes = fs::read(&path).map_err(|e| MetadataError::Io(path.clone(), e))?;
        let metadata: Self =
            serde_json::from_slice(&bytes).map_err(|e| MetadataError::Invalid(path.clone(), e))?;
        if metadata.schema_version > SCHEMA_VERSION {
            return Err(MetadataError::NewerSchema(path, metadata.schema_version));
        }
        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MeetingMetadata {
        let mut m = MeetingMetadata::new(
            "0.1.0",
            "Weekly sync",
            Mode::ScreenAudio,
            "2026-10-16T09:05:03.250Z".into(),
        );
        m.meeting_id = Some(7);
        m.video = Some(VideoInfo {
            monitor: r"\\.\DISPLAY2".into(),
            width: 1920,
            height: 1080,
            fps: 15,
            encoder: "h264_mf (hardware)".into(),
        });
        m.microphone = Some(AudioInfo {
            device: "Microphone Array".into(),
            sample_rate: 48_000,
            channels: 2,
            start_offset_ms: 38,
            drift_ppm: Some(0.1),
        });
        m.pauses.push(PauseEntry {
            at_media_ms: 60_000,
            wall_length_ms: 15_000,
        });
        m.add_gap(
            StreamKind::Microphone,
            Duration::from_secs(90),
            Duration::from_secs(95),
            GapReason::DeviceLost,
        );
        m.add_error(
            Duration::from_secs(95),
            Some(StreamKind::Microphone),
            "deviceLost",
            "The microphone was unplugged.",
        );
        m
    }

    #[test]
    fn metadata_survives_a_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let original = sample();

        original.write(dir.path()).unwrap();
        let read_back = MeetingMetadata::read(dir.path()).unwrap();

        assert_eq!(read_back, original);
    }

    #[test]
    fn the_json_field_names_are_the_documented_ones() {
        let json = serde_json::to_value(sample()).unwrap();

        for key in [
            "schemaVersion",
            "appVersion",
            "meetingId",
            "title",
            "mode",
            "startedAt",
            "endedAt",
            "durationMs",
            "status",
            "recovered",
            "video",
            "microphone",
            "systemAudio",
            "pauses",
            "gaps",
            "errors",
        ] {
            assert!(json.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(json["mode"], "screenAudio");
        assert_eq!(json["gaps"][0]["stream"], "microphone");
        assert_eq!(json["gaps"][0]["reason"], "deviceLost");
        assert_eq!(json["gaps"][0]["startMs"], 90_000);
        assert_eq!(json["pauses"][0]["atMediaMs"], 60_000);
        assert_eq!(json["microphone"]["startOffsetMs"], 38);
        assert_eq!(json["video"]["monitor"], r"\\.\DISPLAY2");
    }

    #[test]
    fn a_write_replaces_the_previous_file_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = sample();
        m.write(dir.path()).unwrap();

        m.status = "complete".into();
        m.duration_ms = Some(123_456);
        m.write(dir.path()).unwrap();

        assert_eq!(
            MeetingMetadata::read(dir.path()).unwrap().status,
            "complete"
        );
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![FILE_NAME.to_string()],
            "no .tmp file may be left behind"
        );
    }

    #[test]
    fn a_crash_between_temp_write_and_rename_leaves_the_old_file_readable() {
        // Simulate the crash: a half-written temporary file sits next to a good meeting.json.
        let dir = tempfile::tempdir().unwrap();
        sample().write(dir.path()).unwrap();
        fs::write(
            dir.path().join(format!("{FILE_NAME}.tmp")),
            b"{ \"schemaVersion\": 1, \"title\": \"half",
        )
        .unwrap();

        let read_back = MeetingMetadata::read(dir.path()).unwrap();

        assert_eq!(read_back.title, "Weekly sync");
    }

    #[test]
    fn garbage_is_reported_as_invalid_not_as_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"not json at all").unwrap();

        let error = MeetingMetadata::read(dir.path()).unwrap_err();

        assert!(matches!(error, MetadataError::Invalid(..)), "{error}");
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();

        assert!(matches!(
            MeetingMetadata::read(dir.path()).unwrap_err(),
            MetadataError::Io(..)
        ));
    }

    #[test]
    fn a_file_from_a_newer_app_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut json = serde_json::to_value(sample()).unwrap();
        json["schemaVersion"] = serde_json::json!(SCHEMA_VERSION + 1);
        fs::write(
            dir.path().join(FILE_NAME),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();

        let error = MeetingMetadata::read(dir.path()).unwrap_err();

        assert!(
            matches!(error, MetadataError::NewerSchema(_, v) if v == SCHEMA_VERSION + 1),
            "{error}"
        );
    }

    #[test]
    fn unknown_extra_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut json = serde_json::to_value(sample()).unwrap();
        json["somethingAddedLater"] = serde_json::json!({"x": 1});
        fs::write(
            dir.path().join(FILE_NAME),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();

        assert_eq!(
            MeetingMetadata::read(dir.path()).unwrap().title,
            "Weekly sync"
        );
    }

    #[test]
    fn a_gap_reported_in_pieces_is_one_entry() {
        let mut m = sample();
        m.gaps.clear();

        m.add_gap(
            StreamKind::Microphone,
            Duration::from_secs(2),
            Duration::from_millis(2900),
            GapReason::DeviceLost,
        );
        m.add_gap(
            StreamKind::Microphone,
            Duration::from_millis(2900),
            Duration::from_millis(3900),
            GapReason::DeviceLost,
        );
        m.add_gap(
            StreamKind::Microphone,
            Duration::from_millis(3900),
            Duration::from_millis(4500),
            GapReason::DeviceLost,
        );

        assert_eq!(m.gaps.len(), 1);
        assert_eq!((m.gaps[0].start_ms, m.gaps[0].end_ms), (2000, 4500));
    }

    #[test]
    fn separate_outages_other_streams_and_other_reasons_stay_separate() {
        let mut m = sample();
        m.gaps.clear();

        m.add_gap(
            StreamKind::Microphone,
            Duration::from_secs(2),
            Duration::from_secs(3),
            GapReason::DeviceLost,
        );
        m.add_gap(
            StreamKind::Microphone,
            Duration::from_secs(10),
            Duration::from_secs(11),
            GapReason::DeviceLost,
        );
        m.add_gap(
            StreamKind::SystemAudio,
            Duration::from_secs(3),
            Duration::from_secs(4),
            GapReason::DeviceLost,
        );
        m.add_gap(
            StreamKind::Microphone,
            Duration::from_secs(11),
            Duration::from_secs(12),
            GapReason::SourceStalled,
        );

        assert_eq!(m.gaps.len(), 4);
    }

    #[test]
    fn a_gap_that_ends_before_it_starts_is_clamped() {
        let mut m = sample();
        m.gaps.clear();

        m.add_gap(
            StreamKind::Video,
            Duration::from_secs(10),
            Duration::from_secs(5),
            GapReason::ScreenLocked,
        );

        assert_eq!(m.gaps[0].start_ms, 10_000);
        assert_eq!(m.gaps[0].end_ms, 10_000);
    }
}
