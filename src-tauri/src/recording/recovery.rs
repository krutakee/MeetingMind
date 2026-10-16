//! Crash recovery (SPEC Sections 5 and 8): a recording that was still `recording` or `finalizing` when the app last
//! ran is repaired and finished, or discarded, on the next launch.
//!
//! What a crash leaves behind: a fragmented MP4 (`video.part.mp4`) that plays up to its last keyframe, and FLAC tracks
//! whose headers were never finished. Recovery rewrites the FLAC headers, cuts `recording.mp4` at the shortest
//! common end (the tracks stop at slightly different moments), and runs the same final mix as a normal stop.
//!
//! The separate FLAC tracks are kept at their full length: real recorded audio is never thrown away, so after a
//! recovery they can be a second or two longer than `recording.mp4`. All tracks start at media time 0, so nothing
//! is shifted.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use crate::error::AppError;
use crate::recording::ffmpeg::{self, FfmpegError, FinalizeInputs};
use crate::recording::metadata::{MeetingMetadata, MetadataError, Mode, millis};
use crate::recording::session::names;
use crate::recording::timefmt::rfc3339_utc;
use crate::storage::Database;
use crate::storage::meetings::{self, MeetingRow};

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("{0}")]
    Database(#[from] AppError),
    #[error("{0}")]
    Ffmpeg(#[from] FfmpegError),
    #[error("{0}")]
    Metadata(#[from] MetadataError),
    #[error("file error: {0}")]
    Io(#[from] io::Error),
    #[error("the meeting folder is missing: {0}")]
    FolderMissing(PathBuf),
    #[error("no usable recording was found in {0}")]
    NothingUsable(PathBuf),
    #[error("refusing to delete {0}: it is not directly inside the storage folder")]
    NotInStorageFolder(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Recovered {
    pub meeting_id: i64,
    pub folder: PathBuf,
    /// `recording.mp4` or `recording.m4a`.
    pub file: PathBuf,
    pub duration: Duration,
}

/// Recordings that were not finished when the app last stopped, oldest first.
pub fn unfinished(db: &Database) -> Result<Vec<MeetingRow>, AppError> {
    db.with_conn(|conn| meetings::unfinished(conn))
}

/// The length of a track if it can be read and is not empty.
fn usable_length(ffmpeg: &Path, file: &Path) -> Option<Duration> {
    if !file.metadata().is_ok_and(|m| m.is_file() && m.len() > 0) {
        return None;
    }
    ffmpeg::measure_duration(ffmpeg, file)
        .ok()
        .filter(|d| !d.is_zero())
}

/// Repairs and finishes an unfinished recording. Safe to run again if it is interrupted.
pub fn recover(
    ffmpeg: &Path,
    db: &Database,
    meeting: &MeetingRow,
) -> Result<Recovered, RecoveryError> {
    let folder = PathBuf::from(&meeting.folder_path);
    if !folder.is_dir() {
        db.with_conn(|c| meetings::set_status(c, meeting.id, "failed"))?;
        return Err(RecoveryError::FolderMissing(folder));
    }

    let video_part = folder.join(names::VIDEO_PART);
    let microphone = folder.join(names::MICROPHONE);
    let system_audio = folder.join(names::SYSTEM_AUDIO);
    let final_video = folder.join(names::FINAL_VIDEO);
    let final_audio = folder.join(names::FINAL_AUDIO);

    let mut metadata = MeetingMetadata::read(&folder).unwrap_or_else(|_| {
        let mode = if meeting.mode == "screen_audio" {
            Mode::ScreenAudio
        } else {
            Mode::AudioOnly
        };
        let mut m = MeetingMetadata::new(
            env!("CARGO_PKG_VERSION"),
            &meeting.title,
            mode,
            meeting.started_at.clone(),
        );
        m.meeting_id = Some(meeting.id);
        m
    });

    // A crash during the final step may have left a finished file: if it reads, there is nothing to rebuild.
    let finished_file = [&final_video, &final_audio]
        .into_iter()
        .find_map(|f| usable_length(ffmpeg, f).map(|d| (f.clone(), d)));

    let (file, duration) = match finished_file {
        Some(found) => found,
        None => {
            let video_length = usable_length(ffmpeg, &video_part);
            let mic_length = usable_length(ffmpeg, &microphone);
            let system_length = usable_length(ffmpeg, &system_audio);
            let lengths: Vec<Duration> = [video_length, mic_length, system_length]
                .into_iter()
                .flatten()
                .collect();
            let common_end = lengths
                .iter()
                .copied()
                .min()
                .ok_or_else(|| RecoveryError::NothingUsable(folder.clone()))?;

            // Give FLAC tracks a proper header (length, seeking). If repairing fails the original is kept.
            for (track, length) in [(&microphone, mic_length), (&system_audio, system_length)] {
                if length.is_some() {
                    let temp = track.with_extension("repaired.flac");
                    match ffmpeg::remux_copy(ffmpeg, track, &temp) {
                        Ok(()) => std::fs::rename(&temp, track)?,
                        Err(e) => {
                            warn!(error = %e, "could not repair a FLAC header; keeping the track as it is");
                            let _ = std::fs::remove_file(&temp);
                        }
                    }
                }
            }

            let use_video = video_length.is_some();
            let output = if use_video {
                final_video.clone()
            } else {
                final_audio.clone()
            };
            ffmpeg::run_final_mux(
                ffmpeg,
                &FinalizeInputs {
                    video: use_video.then_some(video_part.as_path()),
                    microphone: mic_length.is_some().then_some(microphone.as_path()),
                    system_audio: system_length.is_some().then_some(system_audio.as_path()),
                    output: &output,
                    trim_to: Some(common_end),
                },
                |_| {},
            )?;
            (output, common_end)
        }
    };

    let _ = std::fs::remove_file(&video_part);

    let ended_at = rfc3339_utc(SystemTime::now());
    metadata.status = "complete".into();
    metadata.recovered = true;
    metadata.ended_at = Some(ended_at.clone());
    metadata.duration_ms = Some(millis(duration));
    metadata.add_error(
        duration,
        None,
        "recovered",
        "Recovered after the app stopped unexpectedly; the last moments before the interruption may be missing.",
    );
    metadata.write(&folder)?;
    db.with_conn(|c| {
        meetings::finish(
            c,
            meeting.id,
            "complete",
            &ended_at,
            millis(duration) as i64,
        )
    })?;
    info!(meeting_id = meeting.id, "unfinished recording recovered");
    Ok(Recovered {
        meeting_id: meeting.id,
        folder,
        file,
        duration,
    })
}

/// Throws an unfinished recording away: deletes its folder and its database row. Only a folder directly inside
/// `storage_folder` is ever deleted.
pub fn discard(
    db: &Database,
    meeting: &MeetingRow,
    storage_folder: &Path,
) -> Result<(), RecoveryError> {
    let folder = PathBuf::from(&meeting.folder_path);
    if folder.parent() != Some(storage_folder) {
        return Err(RecoveryError::NotInStorageFolder(folder));
    }
    if folder.is_dir() {
        crate::recording::layout::remove_folder(&folder)?;
    }
    db.with_conn(|c| meetings::delete(c, meeting.id))?;
    Ok(())
}
