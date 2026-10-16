//! Whole recordings: real FFmpeg, real clock, synthetic sources (test doubles from `recording::testing`, no hardware).
//!
//! Like `ffmpeg_real.rs`, these skip when FFmpeg is missing unless `REQUIRE_FFMPEG` is set (CI sets it).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meetingmind_lib::recording::clock::SystemClock;
use meetingmind_lib::recording::disk::DiskThresholds;
use meetingmind_lib::recording::ffmpeg::{self, FfmpegInfo, H264Kind};
use meetingmind_lib::recording::layout::LocalDateTime;
use meetingmind_lib::recording::metadata::{GapReason, MeetingMetadata, Mode, StreamKind};
use meetingmind_lib::recording::session::{
    Session, SessionConfig, SessionDeps, SessionError, SessionSources, StopReason, WarningKind,
    names,
};
use meetingmind_lib::recording::state::RecordingState;
use meetingmind_lib::recording::testing::{SyntheticAudio, SyntheticScreen};
use meetingmind_lib::storage::Database;
use meetingmind_lib::storage::meetings;
use meetingmind_lib::system::disk::DiskSpace;
use meetingmind_lib::system::{PowerManager, SystemError};

fn ffmpeg_or_skip(test: &str) -> Option<(FfmpegInfo, H264Kind)> {
    match ffmpeg::locate().and_then(|p| ffmpeg::probe(&p)) {
        Ok(info) => {
            // MEETINGMIND_TEST_H264 (hardware, openh264 or software) forces an encoder, to mimic a machine without
            // a GPU (a CI runner) on a machine that has one.
            let kind = match std::env::var("MEETINGMIND_TEST_H264").ok().as_deref() {
                Some("hardware") => H264Kind::HardwareMf,
                Some("openh264") => H264Kind::OpenH264,
                Some("software") => H264Kind::SoftwareMf,
                _ => ffmpeg::select_h264(&info).expect("an H.264 encoder works"),
            };
            Some((info, kind))
        }
        Err(error) => {
            if std::env::var_os("REQUIRE_FFMPEG").is_some() {
                panic!("REQUIRE_FFMPEG is set but FFmpeg is not usable: {error}");
            }
            eprintln!("SKIPPED {test}: {error}");
            None
        }
    }
}

fn when() -> LocalDateTime {
    LocalDateTime {
        year: 2026,
        month: 10,
        day: 16,
        hour: 9,
        minute: 5,
    }
}

/// TEST DOUBLE: a disk with a fixed amount of free space that a test can change.
struct FakeDisk(std::sync::atomic::AtomicU64);

impl DiskSpace for FakeDisk {
    fn available_bytes(&self, _path: &Path) -> std::io::Result<u64> {
        Ok(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

struct Rig {
    storage: tempfile::TempDir,
    deps: SessionDeps,
    db: Arc<Database>,
    disk: Arc<FakeDisk>,
}

fn rig(info: FfmpegInfo, kind: H264Kind) -> Rig {
    let storage = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&storage.path().join("test.db")).unwrap());
    let disk = Arc::new(FakeDisk(std::sync::atomic::AtomicU64::new(
        50 * 1024 * 1024 * 1024,
    )));
    let deps = SessionDeps {
        clock: Arc::new(SystemClock::new()),
        ffmpeg: info,
        video_encoder: kind,
        disk: disk.clone(),
        database: Some(db.clone()),
        power: None,
    };
    Rig {
        storage,
        deps,
        db,
        disk,
    }
}

fn config(rig: &Rig, title: &str, mode: Mode) -> SessionConfig {
    let mut config = SessionConfig::new(title, mode, &rig.storage.path().join("Meetings"), when());
    config.fps = 15;
    config.disk_check_interval = Duration::from_millis(200);
    config.finish_timeout = Duration::from_secs(60);
    config
}

fn assert_close(actual: Duration, expected: f64, tolerance: f64, what: &str) {
    let a = actual.as_secs_f64();
    assert!(
        (a - expected).abs() <= tolerance,
        "{what}: {a:.3} s, expected {expected:.3} s (+-{tolerance})"
    );
}

/// What meeting.json says about errors and gaps, to make a failing test explain itself.
fn recorded_problems(folder: &Path) -> String {
    match MeetingMetadata::read(folder) {
        Ok(m) => format!("errors: {:?}; gaps: {:?}", m.errors, m.gaps),
        Err(e) => format!("(meeting.json unreadable: {e})"),
    }
}

fn length(info: &FfmpegInfo, file: &Path) -> Duration {
    let folder = file.parent().unwrap_or(file);
    ffmpeg::measure_duration(&info.path, file).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}
{}",
            file.display(),
            recorded_problems(folder)
        )
    })
}

fn screen_and_audio() -> SessionSources {
    SessionSources::default()
        .with_screen(SyntheticScreen::new(320, 180))
        .with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 2))
        .with_system_audio(SyntheticAudio::new("Synthetic speakers", 44_100, 2))
}

#[test]
fn a_screen_and_audio_recording_ends_as_one_mp4_with_both_flac_tracks_and_a_complete_record() {
    let Some((info, kind)) = ffmpeg_or_skip("basic") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let sources = screen_and_audio();

    let session = Session::start(
        config(&rig, "Weekly sync", Mode::ScreenAudio),
        sources,
        rig.deps,
    )
    .unwrap();
    let folder = session.folder().to_path_buf();
    let id = session.meeting_id().unwrap();
    std::thread::sleep(Duration::from_millis(4500));
    let snapshot = session.snapshot();
    assert_eq!(snapshot.state, RecordingState::Recording);
    assert!(
        snapshot.microphone_level > 0.1,
        "level meter shows the signal: {}",
        snapshot.microphone_level
    );
    let outcome = session.stop().unwrap();

    // Files: exactly the documented ones, no leftovers.
    let mut files: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            "meeting.json",
            "microphone.flac",
            "recording.mp4",
            "system-audio.flac"
        ],
        "{}",
        recorded_problems(&folder)
    );
    assert_eq!(outcome.file, folder.join("recording.mp4"));
    assert_close(outcome.duration, 4.5, 0.6, "reported duration");
    // Every file has the same length as the media timeline: the property all later timestamps rely on.
    for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
        assert_close(
            length(&info, &folder.join(file)),
            outcome.duration.as_secs_f64(),
            0.15,
            file,
        );
    }

    let metadata = MeetingMetadata::read(&folder).unwrap();
    assert_eq!(metadata.status, "complete");
    assert_eq!(metadata.meeting_id, Some(id));
    assert_eq!(metadata.mode, Mode::ScreenAudio);
    assert_eq!(metadata.video.as_ref().unwrap().width, 320);
    assert_eq!(
        metadata.microphone.as_ref().unwrap().device,
        "Synthetic microphone"
    );
    assert_eq!(metadata.system_audio.as_ref().unwrap().sample_rate, 44_100);
    // The device start-up delay is recorded. How long it is depends on how busy the machine is, so only a loose
    // bound is checked; what matters is the rule: under a second it is an offset, not a gap.
    let mic_offset = metadata.microphone.as_ref().unwrap().start_offset_ms;
    let system_offset = metadata.system_audio.as_ref().unwrap().start_offset_ms;
    assert!(
        (20..5000).contains(&mic_offset),
        "device start-up delay is recorded: {mic_offset} ms"
    );
    assert!(metadata.errors.is_empty(), "{:?}", metadata.errors);
    if mic_offset.max(system_offset) < 1000 {
        assert!(
            metadata.gaps.is_empty(),
            "short start-up delays are not gaps: {:?}",
            metadata.gaps
        );
    }

    let row = rig.db.with_conn(|c| meetings::get(c, id)).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert!(row.duration_ms.unwrap() > 3000);
    assert_eq!(row.folder_path, folder.to_string_lossy());
}

#[test]
fn paused_time_is_cut_out_of_every_track_and_listed_in_the_metadata() {
    let Some((info, kind)) = ffmpeg_or_skip("pause") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let mut sources = SessionSources::default();
    let mic = SyntheticAudio::new("Synthetic microphone", 48_000, 1);
    let starts = mic.start_count();
    sources = sources.with_screen(SyntheticScreen::new(320, 180));
    sources = sources.with_microphone(mic);

    let mut session = Session::start(
        config(&rig, "Pause test", Mode::ScreenAudio),
        sources,
        rig.deps,
    )
    .unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_millis(2500));
    session.pause().unwrap();
    assert_eq!(session.state(), RecordingState::Paused);
    std::thread::sleep(Duration::from_millis(3000)); // this time must not appear in any file
    let frozen = session.snapshot().elapsed_ms;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        session.snapshot().elapsed_ms,
        frozen,
        "the elapsed time is frozen while paused"
    );
    session.resume().unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    let outcome = session.stop().unwrap();

    assert_eq!(
        *starts.lock().unwrap(),
        2,
        "every source is started again on resume"
    );
    // About 5 s of media, not 8 s of wall clock.
    assert_close(outcome.duration, 5.0, 0.7, "media length with 3 s paused");
    for file in ["recording.mp4", "microphone.flac"] {
        assert_close(
            length(&info, &folder.join(file)),
            outcome.duration.as_secs_f64(),
            0.15,
            file,
        );
    }
    let metadata = MeetingMetadata::read(&folder).unwrap();
    assert_eq!(metadata.pauses.len(), 1);
    assert_close(
        Duration::from_millis(metadata.pauses[0].at_media_ms),
        2.5,
        0.5,
        "cut point",
    );
    assert_close(
        Duration::from_millis(metadata.pauses[0].wall_length_ms),
        3.3,
        0.5,
        "pause length",
    );
    assert!(
        metadata.gaps.is_empty(),
        "a pause is not a gap: {:?}",
        metadata.gaps
    );
}

#[test]
fn an_audio_only_recording_produces_an_m4a_and_no_video_file() {
    let Some((info, kind)) = ffmpeg_or_skip("audio only") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let mut sources = SessionSources::default();
    sources = sources.with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));

    let session = Session::start(config(&rig, "Call", Mode::AudioOnly), sources, rig.deps).unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_millis(3000));
    let outcome = session.stop().unwrap();

    assert_eq!(outcome.file, folder.join("recording.m4a"));
    let mut files: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        ["meeting.json", "microphone.flac", "recording.m4a"],
        "no empty placeholders"
    );
    assert_close(
        length(&info, &outcome.file),
        outcome.duration.as_secs_f64(),
        0.15,
        "m4a",
    );
}

#[test]
fn unplugging_the_microphone_keeps_the_recording_going_and_logs_the_gap() {
    let Some((info, kind)) = ffmpeg_or_skip("unplug") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let mut sources = SessionSources::default();
    let mic = SyntheticAudio::new("USB microphone", 48_000, 1);
    let plug = mic.handle();
    sources = sources.with_screen(SyntheticScreen::new(320, 180));
    sources = sources.with_microphone(mic);
    sources = sources.with_system_audio(SyntheticAudio::new("Synthetic speakers", 48_000, 2));

    let session =
        Session::start(config(&rig, "Unplug", Mode::ScreenAudio), sources, rig.deps).unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_millis(2000));
    plug.unplug();
    std::thread::sleep(Duration::from_millis(2500));
    let during = session.snapshot();
    assert_eq!(
        during.state,
        RecordingState::Recording,
        "never stops the whole recording silently"
    );
    assert!(
        during
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::SourceLost && w.stream == Some(StreamKind::Microphone))
    );
    plug.replug();
    std::thread::sleep(Duration::from_millis(2000));
    let outcome = session.stop().unwrap();

    // Every track still has the full length; the microphone has silence where it was unplugged.
    for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
        assert_close(
            length(&info, &folder.join(file)),
            outcome.duration.as_secs_f64(),
            0.2,
            file,
        );
    }
    let metadata = MeetingMetadata::read(&folder).unwrap();
    let gap = metadata
        .gaps
        .iter()
        .find(|g| g.stream == StreamKind::Microphone && g.reason == GapReason::DeviceLost)
        .unwrap_or_else(|| panic!("no device-lost gap in {:?}", metadata.gaps));
    assert_close(Duration::from_millis(gap.start_ms), 2.0, 0.5, "gap start");
    assert_close(
        Duration::from_millis(gap.end_ms - gap.start_ms),
        2.5,
        0.6,
        "gap length",
    );
    assert!(metadata.errors.iter().any(|e| e.kind == "deviceLost"));
}

#[test]
fn a_locked_screen_leaves_a_black_gap_while_audio_continues() {
    let Some((info, kind)) = ffmpeg_or_skip("lock") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let mut sources = SessionSources::default();
    let screen = SyntheticScreen::new(320, 180);
    let lock = screen.handle();
    sources = sources.with_screen(screen);
    sources = sources.with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));

    let session =
        Session::start(config(&rig, "Lock", Mode::ScreenAudio), sources, rig.deps).unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_millis(1500));
    lock.lock_screen();
    std::thread::sleep(Duration::from_millis(2000));
    lock.unlock_screen();
    std::thread::sleep(Duration::from_millis(1500));
    let outcome = session.stop().unwrap();

    for file in ["recording.mp4", "microphone.flac"] {
        assert_close(
            length(&info, &folder.join(file)),
            outcome.duration.as_secs_f64(),
            0.2,
            file,
        );
    }
    let metadata = MeetingMetadata::read(&folder).unwrap();
    let gap = metadata
        .gaps
        .iter()
        .find(|g| g.stream == StreamKind::Video)
        .expect("a video gap");
    assert_eq!(gap.reason, GapReason::ScreenLocked);
    assert_close(
        Duration::from_millis(gap.end_ms - gap.start_ms),
        2.0,
        0.5,
        "lock length",
    );
}

#[test]
fn a_source_that_cannot_start_is_a_warning_and_the_rest_still_records() {
    let Some((info, kind)) = ffmpeg_or_skip("one source fails") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let mut sources = SessionSources::default();
    sources = sources
        .with_microphone(SyntheticAudio::new("Busy microphone", 48_000, 1).failing_to_start());
    sources = sources.with_system_audio(SyntheticAudio::new("Synthetic speakers", 48_000, 2));

    let session =
        Session::start(config(&rig, "Partial", Mode::AudioOnly), sources, rig.deps).unwrap();
    let folder = session.folder().to_path_buf();
    assert!(
        session
            .snapshot()
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::SourceUnavailable
                && w.stream == Some(StreamKind::Microphone))
    );
    std::thread::sleep(Duration::from_millis(2000));
    session.stop().unwrap();

    assert!(
        !folder.join(names::MICROPHONE).exists(),
        "no empty microphone file"
    );
    assert!(folder.join(names::SYSTEM_AUDIO).is_file());
    assert!(folder.join(names::FINAL_AUDIO).is_file());
    let metadata = MeetingMetadata::read(&folder).unwrap();
    assert!(
        metadata
            .errors
            .iter()
            .any(|e| e.kind == "sourceUnavailable")
    );
}

#[test]
fn when_nothing_can_start_the_folder_and_the_row_are_removed() {
    let Some((info, kind)) = ffmpeg_or_skip("nothing starts") else {
        return;
    };
    let rig = rig(info, kind);
    let mut sources = SessionSources::default();
    sources = sources
        .with_microphone(SyntheticAudio::new("Busy microphone", 48_000, 1).failing_to_start());
    let meetings_dir = rig.storage.path().join("Meetings");

    let error = Session::start(config(&rig, "Nothing", Mode::AudioOnly), sources, rig.deps)
        .err()
        .expect("must fail");

    assert!(matches!(error, SessionError::NothingToRecord), "{error}");
    assert_eq!(
        std::fs::read_dir(&meetings_dir).unwrap().count(),
        0,
        "no stray folder"
    );
    assert_eq!(
        rig.db.with_conn(|c| meetings::unfinished(c)).unwrap().len(),
        0,
        "no stray row"
    );
}

#[test]
fn critically_low_disk_space_asks_to_stop_and_the_stop_keeps_everything() {
    let Some((info, kind)) = ffmpeg_or_skip("disk") else {
        return;
    };
    let rig = rig(info.clone(), kind);
    let mut sources = SessionSources::default();
    sources = sources.with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));
    let mut cfg = config(&rig, "Disk", Mode::AudioOnly);
    cfg.disk_thresholds = DiskThresholds {
        warn_below: 2 * 1024 * 1024 * 1024,
        stop_below: 500 * 1024 * 1024,
    };

    let session = Session::start(cfg, sources, rig.deps).unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(session.snapshot().auto_stop, None);
    rig.disk
        .0
        .store(100 * 1024 * 1024, std::sync::atomic::Ordering::SeqCst);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while session.snapshot().auto_stop.is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(session.snapshot().auto_stop, Some(StopReason::DiskFull));
    assert!(
        session
            .snapshot()
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::DiskCritical)
    );
    let outcome = session.stop().unwrap();

    assert!(outcome.file.is_file(), "everything recorded so far is kept");
    assert!(outcome.duration > Duration::from_millis(1000));
    let metadata = MeetingMetadata::read(&folder).unwrap();
    assert!(metadata.errors.iter().any(|e| e.kind == "diskCritical"));
}

#[test]
fn cancelling_deletes_the_folder_and_the_row() {
    let Some((info, kind)) = ffmpeg_or_skip("cancel") else {
        return;
    };
    let rig = rig(info, kind);
    let sources = screen_and_audio();
    let session = Session::start(
        config(&rig, "Discard me", Mode::ScreenAudio),
        sources,
        rig.deps,
    )
    .unwrap();
    let folder = session.folder().to_path_buf();
    let id = session.meeting_id().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(folder.is_dir());

    session.cancel().unwrap();

    assert!(!folder.exists(), "the folder is gone");
    assert_eq!(
        rig.db.with_conn(|c| meetings::get(c, id)).unwrap(),
        None,
        "the row is gone"
    );
}

#[test]
fn a_second_recording_with_the_same_title_never_touches_the_first() {
    let Some((info, kind)) = ffmpeg_or_skip("same title") else {
        return;
    };
    let rig_a = rig(info.clone(), kind);
    let mut cfg_sources = SessionSources::default();
    cfg_sources =
        cfg_sources.with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));
    let storage = rig_a.storage.path().join("Meetings");
    let first = Session::start(
        config(&rig_a, "Standup", Mode::AudioOnly),
        cfg_sources,
        rig_a.deps,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let first_outcome = first.stop().unwrap();
    let first_bytes = std::fs::read(&first_outcome.file).unwrap();

    let deps = SessionDeps {
        clock: Arc::new(SystemClock::new()),
        ffmpeg: info,
        video_encoder: kind,
        disk: rig_a.disk.clone(),
        database: Some(rig_a.db.clone()),
        power: None,
    };
    let mut second_sources = SessionSources::default();
    second_sources =
        second_sources.with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));
    let mut second_config = SessionConfig::new("Standup", Mode::AudioOnly, &storage, when());
    second_config.finish_timeout = Duration::from_secs(60);
    let second = Session::start(second_config, second_sources, deps).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let second_outcome = second.stop().unwrap();

    assert_ne!(first_outcome.folder, second_outcome.folder);
    assert!(second_outcome.folder.to_string_lossy().ends_with("_2"));
    assert_eq!(
        std::fs::read(&first_outcome.file).unwrap(),
        first_bytes,
        "the first recording is untouched"
    );
}

/// TEST DOUBLE: counts how many keep-awake guards are alive, and what each asked for.
struct FakePower {
    alive: Arc<std::sync::atomic::AtomicUsize>,
    display_requests: Arc<std::sync::Mutex<Vec<bool>>>,
    refuse: bool,
}

struct FakeGuard(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for FakeGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl PowerManager for FakePower {
    fn keep_awake(&self, keep_display_on: bool) -> Result<Box<dyn Send>, SystemError> {
        self.display_requests.lock().unwrap().push(keep_display_on);
        if self.refuse {
            return Err(SystemError::Os("the power request was refused".into()));
        }
        self.alive.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::new(FakeGuard(self.alive.clone())))
    }
}

fn fake_power(
    refuse: bool,
) -> (
    Arc<FakePower>,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::Mutex<Vec<bool>>>,
) {
    let alive = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    (
        Arc::new(FakePower {
            alive: alive.clone(),
            display_requests: requests.clone(),
            refuse,
        }),
        alive,
        requests,
    )
}

#[test]
fn the_computer_is_kept_awake_while_recording_and_released_afterwards() {
    let Some((info, kind)) = ffmpeg_or_skip("keep awake") else {
        return;
    };
    let mut rig = rig(info, kind);
    let (power, alive, requests) = fake_power(false);
    rig.deps.power = Some(power);
    let sources = SessionSources::default()
        .with_screen(SyntheticScreen::new(320, 180))
        .with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));

    let session =
        Session::start(config(&rig, "Awake", Mode::ScreenAudio), sources, rig.deps).unwrap();
    assert_eq!(
        alive.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "kept awake during the recording"
    );
    std::thread::sleep(Duration::from_millis(1200));
    session.stop().unwrap();

    assert_eq!(
        alive.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "released when the recording ends"
    );
    assert_eq!(
        *requests.lock().unwrap(),
        vec![true],
        "recording the screen also keeps the display on"
    );
}

#[test]
fn audio_only_recording_lets_the_display_turn_off() {
    let Some((info, kind)) = ffmpeg_or_skip("keep awake audio only") else {
        return;
    };
    let mut rig = rig(info, kind);
    let (power, _alive, requests) = fake_power(false);
    rig.deps.power = Some(power);
    let sources = SessionSources::default().with_microphone(SyntheticAudio::new(
        "Synthetic microphone",
        48_000,
        1,
    ));

    let session = Session::start(config(&rig, "Call", Mode::AudioOnly), sources, rig.deps).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    session.stop().unwrap();

    assert_eq!(*requests.lock().unwrap(), vec![false]);
}

#[test]
fn a_refused_power_request_is_a_warning_and_the_recording_still_works() {
    let Some((info, kind)) = ffmpeg_or_skip("keep awake refused") else {
        return;
    };
    let mut rig = rig(info, kind);
    let (power, _alive, _requests) = fake_power(true);
    rig.deps.power = Some(power);
    let sources = SessionSources::default().with_microphone(SyntheticAudio::new(
        "Synthetic microphone",
        48_000,
        1,
    ));

    let session =
        Session::start(config(&rig, "No power", Mode::AudioOnly), sources, rig.deps).unwrap();
    let folder = session.folder().to_path_buf();
    assert!(
        session
            .snapshot()
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::KeepAwakeFailed)
    );
    std::thread::sleep(Duration::from_millis(1000));
    let outcome = session.stop().unwrap();

    assert!(outcome.file.is_file());
    assert!(
        MeetingMetadata::read(&folder)
            .unwrap()
            .errors
            .iter()
            .any(|e| e.kind == "keepAwakeFailed")
    );
}
