//! The recorder end to end: synthetic screen and audio devices, the real FFmpeg, a real database file.
//! Skips (loudly) when FFmpeg is not installed; set REQUIRE_FFMPEG=1 to make that a failure.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use meetingmind_lib::recorder::{Devices, Phase, Recorder, RecorderError, Tools};
use meetingmind_lib::recording::ffmpeg::{self, H264Kind};
use meetingmind_lib::recording::metadata::{MeetingMetadata, Mode};
use meetingmind_lib::recording::setup::{Quality, RecordingSetup};
use meetingmind_lib::recording::testing::SyntheticDevices;
use meetingmind_lib::storage::Database;
use meetingmind_lib::storage::settings::Settings;
use meetingmind_lib::system::disk::DiskSpace;

fn tools_or_skip(test: &str) -> Option<Tools> {
    match ffmpeg::locate().and_then(|p| ffmpeg::probe(&p)) {
        Ok(ffmpeg) => {
            // MEETINGMIND_TEST_H264 forces an encoder, to mimic a machine without a GPU (a CI runner).
            let h264 = match std::env::var("MEETINGMIND_TEST_H264").ok().as_deref() {
                Some("hardware") => Ok(H264Kind::HardwareMf),
                Some("openh264") => Ok(H264Kind::OpenH264),
                Some("software") => Ok(H264Kind::SoftwareMf),
                _ => ffmpeg::select_h264(&ffmpeg).map_err(|e| e.to_string()),
            };
            Some(Tools { ffmpeg, h264 })
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

/// TEST DOUBLE: a disk with an amount of free space that a test can change.
struct FakeDisk(AtomicU64);

impl DiskSpace for FakeDisk {
    fn available_bytes(&self, _path: &Path) -> std::io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    storage: std::path::PathBuf,
    db: Arc<Database>,
    disk: Arc<FakeDisk>,
    recorder: Recorder,
}

fn rig_with(
    devices: SyntheticDevices,
    detect: impl Fn() -> Result<Tools, String> + Send + Sync + 'static,
) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("Meetings");
    let db = Arc::new(Database::open(&dir.path().join("test.db")).unwrap());
    let disk = Arc::new(FakeDisk(AtomicU64::new(50 * 1024 * 1024 * 1024)));
    let devices: Arc<dyn Devices> = Arc::new(devices);
    let recorder = Recorder::new(
        devices,
        detect,
        db.clone(),
        disk.clone(),
        None,
        storage.clone(),
    );
    Rig {
        _dir: dir,
        storage,
        db,
        disk,
        recorder,
    }
}

fn rig(tools: Tools) -> Rig {
    rig_with(SyntheticDevices::new(640, 360), move || Ok(tools.clone()))
}

fn setup(title: &str) -> RecordingSetup {
    RecordingSetup {
        title: title.into(),
        ..RecordingSetup::default()
    }
}

fn meeting_folders(storage: &Path) -> Vec<String> {
    match std::fs::read_dir(storage) {
        Ok(entries) => entries
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn a_recording_runs_through_pause_and_resume_and_is_saved() {
    let Some(tools) = tools_or_skip("lifecycle") else {
        return;
    };
    let rig = rig(tools);
    assert_eq!(rig.recorder.phase(), Phase::Idle);

    rig.recorder.start(&setup("Weekly sync")).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let status = rig.recorder.status();
    assert_eq!(status.phase, Phase::Recording);
    assert_eq!(status.title.as_deref(), Some("Weekly sync"));
    assert_eq!(status.mode, Some(Mode::ScreenAudio));
    assert!(status.elapsed_ms > 800, "{}", status.elapsed_ms);
    assert!(
        status.microphone_level > 0.1,
        "the level meter shows the signal: {}",
        status.microphone_level
    );
    assert!(
        rig.recorder.active_meeting_id().is_some(),
        "the running recording is known by its meeting id"
    );

    rig.recorder.pause().unwrap();
    assert_eq!(rig.recorder.status().phase, Phase::Paused);
    std::thread::sleep(Duration::from_millis(600));
    rig.recorder.resume().unwrap();
    assert_eq!(rig.recorder.status().phase, Phase::Recording);
    std::thread::sleep(Duration::from_millis(1000));

    let finished = rig.recorder.stop().unwrap();

    assert!(finished.has_video);
    assert_eq!(finished.title, "Weekly sync");
    assert!(Path::new(&finished.file).is_file(), "{}", finished.file);
    assert!(finished.file.ends_with("recording.mp4"));
    assert!(
        (1800..3600).contains(&finished.duration_ms),
        "the paused 0.6 s is not in the recording: {} ms",
        finished.duration_ms
    );
    let status = rig.recorder.status();
    assert_eq!(status.phase, Phase::Idle);
    assert_eq!(status.finished, Some(finished.clone()));
    let metadata = MeetingMetadata::read(Path::new(&finished.folder)).unwrap();
    assert_eq!(metadata.status, "complete");
    assert_eq!(metadata.pauses.len(), 1);
    assert_eq!(rig.recorder.active_meeting_id(), None);
}

#[test]
fn a_blank_title_gets_the_default_and_the_banner_is_cleared_by_the_next_start() {
    let Some(tools) = tools_or_skip("blank title") else {
        return;
    };
    let rig = rig(tools);
    let audio_only = RecordingSetup {
        mode: Mode::AudioOnly,
        title: "   ".into(),
        ..RecordingSetup::default()
    };

    rig.recorder.start(&audio_only).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let first = rig.recorder.stop().unwrap();
    assert_eq!(first.title, "Meeting");
    assert!(!first.has_video);
    assert!(first.file.ends_with("recording.m4a"));
    assert!(rig.recorder.status().finished.is_some());

    rig.recorder.start(&audio_only).unwrap();
    let during = rig.recorder.status();
    assert_eq!(
        during.finished, None,
        "the old banner must not show during a new recording"
    );
    rig.recorder.stop().unwrap();
}

#[test]
fn starting_twice_or_stopping_nothing_is_refused_without_harm() {
    let Some(tools) = tools_or_skip("busy") else {
        return;
    };
    let rig = rig(tools);

    assert!(matches!(
        rig.recorder.stop(),
        Err(RecorderError::NotRecording)
    ));
    assert!(matches!(
        rig.recorder.pause(),
        Err(RecorderError::NotRecording)
    ));

    rig.recorder.start(&setup("One")).unwrap();
    assert!(matches!(
        rig.recorder.start(&setup("Two")),
        Err(RecorderError::Busy)
    ));
    std::thread::sleep(Duration::from_millis(800));
    rig.recorder.stop().unwrap();

    assert_eq!(
        meeting_folders(&rig.storage).len(),
        1,
        "the refused second start created nothing"
    );
}

#[test]
fn an_unplugged_microphone_stops_the_start_and_says_so_and_nothing_is_created() {
    let Some(tools) = tools_or_skip("microphone gone") else {
        return;
    };
    let mut devices = SyntheticDevices::new(640, 360);
    devices.microphone_problem = Some("the device was removed".into());
    let rig = rig_with(devices, move || Ok(tools.clone()));

    let error = rig.recorder.start(&setup("Call")).unwrap_err();

    assert!(matches!(error, RecorderError::Device(_)), "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("microphone") && message.contains("removed"),
        "{message}"
    );
    assert_eq!(rig.recorder.phase(), Phase::Idle, "can try again");
    assert!(meeting_folders(&rig.storage).is_empty());

    // Turning the microphone off lets the same recording go ahead.
    let without = RecordingSetup {
        microphone: false,
        ..setup("Call")
    };
    rig.recorder.start(&without).unwrap();
    rig.recorder.stop().unwrap();
}

#[test]
fn a_monitor_that_is_gone_is_an_error_not_a_silent_fallback() {
    let Some(tools) = tools_or_skip("monitor gone") else {
        return;
    };
    let rig = rig(tools);
    let wanted = RecordingSetup {
        monitor_id: Some("\\\\.\\DISPLAY9".into()),
        ..setup("Demo")
    };

    let error = rig.recorder.start(&wanted).unwrap_err();

    assert!(error.to_string().contains("not connected"), "{error}");
    assert_eq!(rig.recorder.phase(), Phase::Idle);
    assert!(meeting_folders(&rig.storage).is_empty());
}

#[test]
fn an_audio_only_setup_without_any_audio_is_refused_before_anything_opens() {
    let Some(tools) = tools_or_skip("invalid setup") else {
        return;
    };
    let rig = rig(tools);
    let nothing = RecordingSetup {
        mode: Mode::AudioOnly,
        microphone: false,
        system_audio: false,
        ..setup("Nothing")
    };

    let error = rig.recorder.start(&nothing).unwrap_err();

    assert!(matches!(error, RecorderError::InvalidSetup(_)), "{error:?}");
    assert_eq!(rig.recorder.phase(), Phase::Idle);
}

#[test]
fn the_chosen_quality_and_storage_folder_are_used() {
    let Some(tools) = tools_or_skip("quality and folder") else {
        return;
    };
    let rig = rig_with(SyntheticDevices::new(640, 360), move || Ok(tools.clone()));
    let chosen = rig._dir.path().join("Chosen folder");
    rig.db
        .save_settings(&Settings {
            storage_folder: Some(chosen.to_string_lossy().into_owned()),
            ..Settings::default()
        })
        .unwrap();
    let small = RecordingSetup {
        quality: Quality::P720,
        microphone: false,
        system_audio: false,
        ..setup("Small")
    };

    rig.recorder.start(&small).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    let finished = rig.recorder.stop().unwrap();

    assert!(
        Path::new(&finished.folder).starts_with(&chosen),
        "{} is not inside {}",
        finished.folder,
        chosen.display()
    );
    assert!(
        meeting_folders(&rig.storage).is_empty(),
        "the default folder is not used"
    );
    // A 640 x 360 monitor is already below 720 rows: it is never scaled up.
    let video = MeetingMetadata::read(Path::new(&finished.folder))
        .unwrap()
        .video
        .unwrap();
    assert_eq!((video.width, video.height), (640, 360));
}

#[test]
fn a_nearly_full_disk_ends_the_recording_by_itself_and_keeps_everything() {
    let Some(tools) = tools_or_skip("disk full") else {
        return;
    };
    let rig = rig(tools);
    rig.recorder.start(&setup("Long one")).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    assert!(rig.recorder.stop_if_asked().is_none(), "nothing asked yet");

    rig.disk.0.store(100 * 1024 * 1024, Ordering::SeqCst);
    let mut finished = None;
    // The recording checks the disk every 10 seconds.
    for _ in 0..150 {
        std::thread::sleep(Duration::from_millis(200));
        if let Some(result) = rig.recorder.stop_if_asked() {
            finished = Some(result.unwrap());
            break;
        }
    }

    let finished =
        finished.expect("the recording asked to be stopped when the disk was almost full");
    assert!(Path::new(&finished.file).is_file());
    assert_eq!(rig.recorder.phase(), Phase::Idle);
}

#[test]
fn a_failed_search_for_ffmpeg_is_reported_and_retried_not_remembered() {
    let Some(tools) = tools_or_skip("tools retry") else {
        return;
    };
    let working = Arc::new(AtomicBool::new(false));
    let flag = working.clone();
    let rig = rig_with(SyntheticDevices::new(640, 360), move || {
        if flag.load(Ordering::SeqCst) {
            Ok(tools.clone())
        } else {
            Err("ffmpeg.exe was not found".to_string())
        }
    });

    let error = rig.recorder.start(&setup("Retry")).unwrap_err();
    assert!(matches!(error, RecorderError::Tools(_)), "{error:?}");
    assert!(error.to_string().contains("ffmpeg.exe was not found"));
    assert_eq!(rig.recorder.phase(), Phase::Idle);

    working.store(true, Ordering::SeqCst);
    rig.recorder.start(&setup("Retry")).unwrap();
    rig.recorder.stop().unwrap();
}

#[test]
fn errors_reach_the_frontend_as_kind_and_message() {
    let json = serde_json::to_value(RecorderError::Busy).unwrap();

    assert_eq!(
        json,
        serde_json::json!({ "kind": "busy", "message": "a recording is already running" })
    );
}

#[test]
fn cancelling_deletes_the_recording_and_leaves_no_banner() {
    let Some(tools) = tools_or_skip("cancel") else {
        return;
    };
    let rig = rig(tools);
    rig.recorder.start(&setup("Oops")).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(meeting_folders(&rig.storage).len(), 1);

    rig.recorder.cancel().unwrap();

    assert_eq!(rig.recorder.phase(), Phase::Idle);
    assert!(
        meeting_folders(&rig.storage).is_empty(),
        "the folder is gone"
    );
    let status = rig.recorder.status();
    assert_eq!(status.finished, None);
    assert_eq!(status.error, None);
    assert!(matches!(
        rig.recorder.cancel(),
        Err(RecorderError::NotRecording)
    ));
}

#[test]
fn a_notice_from_the_app_shows_with_the_recordings_own_warnings_and_is_gone_afterwards() {
    use meetingmind_lib::recording::session::WarningKind;

    let Some(tools) = tools_or_skip("notice") else {
        return;
    };
    let rig = rig(tools);
    rig.recorder.add_notice(
        WarningKind::OwnWindowVisible,
        "ignored: nothing is recording",
    );
    rig.recorder.start(&setup("Notice")).unwrap();

    rig.recorder.add_notice(
        WarningKind::OwnWindowVisible,
        "MeetingMind's windows may appear in the recording.",
    );
    let status = rig.recorder.status();

    assert!(
        status
            .warnings
            .iter()
            .any(|w| w.kind == WarningKind::OwnWindowVisible && w.message.contains("may appear")),
        "{:?}",
        status.warnings
    );
    assert!(
        status.warnings.iter().all(|w| w.id != 0),
        "ids are never reused"
    );
    rig.recorder.stop().unwrap();
    assert!(rig.recorder.status().warnings.is_empty());

    rig.recorder.start(&setup("Next")).unwrap();
    assert!(
        rig.recorder.status().warnings.is_empty(),
        "a new recording starts without the old notices"
    );
    rig.recorder.stop().unwrap();
}

#[test]
fn the_video_encoder_capability_names_the_encoder_and_a_short_ffmpeg_version() {
    use meetingmind_lib::system::{CapabilityId, CapabilityStatus};

    let Some(tools) = tools_or_skip("encoder capability") else {
        return;
    };
    let rig = rig(tools);

    let capability = rig.recorder.video_encoder_capability();

    assert_eq!(capability.id, CapabilityId::VideoEncoders);
    assert_eq!(capability.status, CapabilityStatus::Detected);
    assert!(
        capability
            .value
            .as_deref()
            .is_some_and(|v| v.contains("h264") || v.contains("openh264")),
        "{capability:?}"
    );
    assert!(
        capability.detail.contains("ffmpeg version"),
        "{}",
        capability.detail
    );
    assert!(
        !capability.detail.contains("Copyright"),
        "the version line is shortened: {}",
        capability.detail
    );
}

#[test]
fn without_ffmpeg_the_video_encoder_capability_says_why_it_is_unavailable() {
    use meetingmind_lib::system::CapabilityStatus;

    let rig = rig_with(SyntheticDevices::new(640, 360), || {
        Err("ffmpeg.exe was not found next to the app".to_string())
    });

    let capability = rig.recorder.video_encoder_capability();

    assert_eq!(capability.status, CapabilityStatus::Unavailable);
    assert_eq!(capability.value, None);
    assert!(
        capability.detail.contains("ffmpeg.exe was not found"),
        "{}",
        capability.detail
    );
}
