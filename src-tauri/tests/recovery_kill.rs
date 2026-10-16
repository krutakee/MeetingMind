//! Crash recovery, end to end: a recorder process is killed the hard way mid-recording (together with its FFmpeg
//! children, like a crash or a power cut), and the next "launch" recovers what is on disk.
//!
//! Needs FFmpeg like the other real-FFmpeg tests (skips without it unless REQUIRE_FFMPEG is set).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use meetingmind_lib::recording::ffmpeg::{self, FfmpegInfo};
use meetingmind_lib::recording::metadata::MeetingMetadata;
use meetingmind_lib::recording::recovery;
use meetingmind_lib::recording::session::names;
use meetingmind_lib::storage::Database;
use meetingmind_lib::storage::meetings;

fn ffmpeg_or_skip(test: &str) -> Option<FfmpegInfo> {
    match ffmpeg::locate().and_then(|p| ffmpeg::probe(&p)) {
        Ok(info) => Some(info),
        Err(error) => {
            if std::env::var_os("REQUIRE_FFMPEG").is_some() {
                panic!("REQUIRE_FFMPEG is set but FFmpeg is not usable: {error}");
            }
            eprintln!("SKIPPED {test}: {error}");
            None
        }
    }
}

/// The test program (src/bin/synthetic_recorder.rs): Cargo builds it before running this test.
fn recorder_program() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_synthetic-recorder"))
}

/// Kills the process and all its children immediately, with no chance to clean up.
fn kill_hard(child: &mut std::process::Child) {
    if cfg!(windows) {
        let status = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("taskkill runs");
        assert!(status.success(), "taskkill failed");
    } else {
        child.kill().expect("kill");
    }
    let _ = child.wait();
}

fn size(path: &Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or(0)
}

/// Starts the recorder, waits until real data is on disk in every track, kills it, and returns the meeting folder.
fn record_and_crash(storage: &Path, db_path: &Path) -> PathBuf {
    let mut child = Command::new(recorder_program())
        .arg(storage)
        .arg(db_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("recorder starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let folder = PathBuf::from(
        line.trim()
            .strip_prefix("READY ")
            .unwrap_or_else(|| panic!("unexpected output: {line:?}")),
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ready = size(&folder.join(names::VIDEO_PART)) > 30_000
            && size(&folder.join(names::MICROPHONE)) > 30_000
            && size(&folder.join(names::SYSTEM_AUDIO)) > 30_000;
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no data reached the disk in time"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(1500)); // some more, so the crash really is mid-recording
    kill_hard(&mut child);
    folder
}

fn assert_close(actual: Duration, expected: f64, tolerance: f64, what: &str) {
    let a = actual.as_secs_f64();
    assert!(
        (a - expected).abs() <= tolerance,
        "{what}: {a:.3} s, expected {expected:.3} s (+-{tolerance})"
    );
}

#[test]
fn a_recording_killed_mid_way_is_found_repaired_and_made_playable() {
    let Some(info) = ffmpeg_or_skip("kill and recover") else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let meetings_dir = storage.path().join("Meetings");
    let db_path = storage.path().join("app.db");

    let folder = record_and_crash(&meetings_dir, &db_path);

    // "Next launch": the database still says the meeting was being recorded.
    let db = Database::open(&db_path).unwrap();
    let found = recovery::unfinished(&db).unwrap();
    assert_eq!(
        found.len(),
        1,
        "exactly the crashed recording is unfinished"
    );
    assert_eq!(found[0].status, "recording");
    assert_eq!(PathBuf::from(&found[0].folder_path), folder);
    assert!(
        folder.join(names::VIDEO_PART).is_file(),
        "the fragmented video is still there"
    );
    assert!(
        !folder.join(names::FINAL_VIDEO).exists(),
        "nothing was finalized"
    );

    let recovered = recovery::recover(&info.path, &db, &found[0]).unwrap();

    // One playable file cut at the common end; the lossless tracks keep all the audio that was recorded.
    assert_eq!(recovered.file, folder.join(names::FINAL_VIDEO));
    assert!(
        recovered.duration > Duration::from_secs(2),
        "kept {:?}",
        recovered.duration
    );
    let final_length =
        ffmpeg::measure_duration(&info.path, &folder.join(names::FINAL_VIDEO)).unwrap();
    assert_close(
        final_length,
        recovered.duration.as_secs_f64(),
        0.35,
        "recording.mp4",
    );
    for file in [names::MICROPHONE, names::SYSTEM_AUDIO] {
        let length = ffmpeg::measure_duration(&info.path, &folder.join(file)).unwrap();
        let end = recovered.duration.as_secs_f64();
        assert!(
            length.as_secs_f64() >= end - 0.35 && length.as_secs_f64() <= end + 3.0,
            "{file}: {:.3} s must cover the recording ({end:.3} s) and not be wildly longer",
            length.as_secs_f64()
        );
    }
    assert!(
        !folder.join(names::VIDEO_PART).exists(),
        "the temporary video is removed once the final file works"
    );

    // The record says what happened.
    let metadata = MeetingMetadata::read(&folder).unwrap();
    assert!(metadata.recovered);
    assert_eq!(metadata.status, "complete");
    assert!(metadata.errors.iter().any(|e| e.kind == "recovered"));
    let row = db
        .with_conn(|c| meetings::get(c, recovered.meeting_id))
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "complete");
    assert!(
        recovery::unfinished(&db).unwrap().is_empty(),
        "nothing is left to recover"
    );

    // Running recovery again changes nothing.
    let again = recovery::recover(&info.path, &db, &row).unwrap();
    assert_close(
        again.duration,
        recovered.duration.as_secs_f64(),
        0.05,
        "second recovery",
    );
}

#[test]
fn a_crashed_recording_can_be_discarded() {
    let Some(_info) = ffmpeg_or_skip("kill and discard") else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let meetings_dir = storage.path().join("Meetings");
    let db_path = storage.path().join("app.db");
    let folder = record_and_crash(&meetings_dir, &db_path);
    let db = Database::open(&db_path).unwrap();
    let found = recovery::unfinished(&db).unwrap();

    recovery::discard(&db, &found[0], &meetings_dir).unwrap();

    assert!(!folder.exists());
    assert!(recovery::unfinished(&db).unwrap().is_empty());
}

#[test]
fn discard_refuses_a_folder_outside_the_storage_folder() {
    let storage = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let db = Database::open(&storage.path().join("app.db")).unwrap();
    let important = elsewhere.path().join("important");
    std::fs::create_dir(&important).unwrap();
    std::fs::write(important.join("keep.txt"), b"precious").unwrap();
    let id = db
        .with_conn(|c| {
            meetings::create(
                c,
                &meetings::NewMeeting {
                    title: "x",
                    started_at: "2026-10-16T09:00:00.000Z",
                    mode: "audio_only",
                    folder_path: &important.to_string_lossy(),
                    capture_settings: None,
                },
            )
        })
        .unwrap();
    let row = db.with_conn(|c| meetings::get(c, id)).unwrap().unwrap();

    let error = recovery::discard(&db, &row, storage.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("not directly inside the storage folder"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(important.join("keep.txt")).unwrap(),
        b"precious",
        "nothing was deleted"
    );
}

#[test]
fn a_meeting_whose_folder_is_gone_is_marked_failed() {
    let Some(info) = ffmpeg_or_skip("folder gone") else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let db = Database::open(&storage.path().join("app.db")).unwrap();
    let missing = storage.path().join("Meetings").join("gone");
    let id = db
        .with_conn(|c| {
            meetings::create(
                c,
                &meetings::NewMeeting {
                    title: "x",
                    started_at: "2026-10-16T09:00:00.000Z",
                    mode: "audio_only",
                    folder_path: &missing.to_string_lossy(),
                    capture_settings: None,
                },
            )
        })
        .unwrap();
    let row = db.with_conn(|c| meetings::get(c, id)).unwrap().unwrap();

    let error = recovery::recover(&info.path, &db, &row).unwrap_err();

    assert!(
        matches!(error, recovery::RecoveryError::FolderMissing(_)),
        "{error}"
    );
    assert_eq!(
        db.with_conn(|c| meetings::get(c, id))
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
    assert!(
        recovery::unfinished(&db).unwrap().is_empty(),
        "it is not offered again at every launch"
    );
}

#[test]
fn a_folder_with_nothing_usable_is_reported_not_invented() {
    let Some(info) = ffmpeg_or_skip("nothing usable") else {
        return;
    };
    let storage = tempfile::tempdir().unwrap();
    let db = Database::open(&storage.path().join("app.db")).unwrap();
    let folder = storage.path().join("Meetings").join("empty");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(names::MICROPHONE), b"").unwrap(); // zero bytes: the crash came before any data
    let id = db
        .with_conn(|c| {
            meetings::create(
                c,
                &meetings::NewMeeting {
                    title: "x",
                    started_at: "2026-10-16T09:00:00.000Z",
                    mode: "audio_only",
                    folder_path: &folder.to_string_lossy(),
                    capture_settings: None,
                },
            )
        })
        .unwrap();
    let row = db.with_conn(|c| meetings::get(c, id)).unwrap().unwrap();

    let error = recovery::recover(&info.path, &db, &row).unwrap_err();

    assert!(
        matches!(error, recovery::RecoveryError::NothingUsable(_)),
        "{error}"
    );
    assert!(
        !folder.join(names::FINAL_AUDIO).exists(),
        "no empty placeholder file"
    );
}
