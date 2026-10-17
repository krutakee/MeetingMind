#![cfg(windows)]
//! Hardware smoke tests: the real monitor, microphone and speakers of the machine running them. Ignored by default
//! because CI runners have no such devices. Run them on a Windows computer with:
//!
//!     cargo test --test hardware_smoke -- --ignored --nocapture --test-threads=1
//!
//! While they run: a small magenta window appears near the top-left of the primary monitor, and a quiet 440 Hz tone
//! plays on the speakers for a few seconds. Recordings are made in a temporary folder, measured (length, size, the
//! color of one pixel, the loudness of the system-audio track) and deleted. Their content is not otherwise looked at.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use meetingmind_lib::audio::windows::{CpalAudioSource, list_microphones, list_speakers};
use meetingmind_lib::capture::windows::{WindowsScreenSource, list_monitors};
use meetingmind_lib::recorder::Recorder;
use meetingmind_lib::recording::ffmpeg::{self, FfmpegInfo, H264Kind};
use meetingmind_lib::recording::metadata::{MeetingMetadata, Mode};
use meetingmind_lib::recording::session::{Session, SessionConfig, SessionSources};
use meetingmind_lib::recording::setup::RecordingSetup;
use meetingmind_lib::storage::Database;
use meetingmind_lib::system::{
    self, CapabilityId, CapabilityStatus, set_window_excluded_from_capture,
};

mod common;
use common::*;

// ---------------------------------------------------------------------------------------------------------------

#[test]
#[ignore = "needs a real monitor, microphone and speakers"]
fn this_machine_has_the_devices_and_the_capabilities_are_detected() {
    let monitors = list_monitors().unwrap();
    let microphones = list_microphones().unwrap();
    let speakers = list_speakers().unwrap();
    println!("monitors: {monitors:#?}");
    println!("microphones: {microphones:#?}");
    println!("speakers: {speakers:#?}");
    assert!(
        monitors.iter().any(|m| m.is_primary),
        "a primary monitor is reported"
    );
    assert!(monitors.iter().all(|m| m.width > 0 && m.height > 0));
    assert!(!speakers.is_empty(), "system audio needs an output device");

    let report = system::platform_probe().probe();
    for capability in &report.capabilities {
        println!(
            "{:?}: {:?} {:?} - {}",
            capability.id, capability.status, capability.value, capability.detail
        );
    }
    let status = |id| {
        report
            .capabilities
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .status
    };
    assert_eq!(
        status(CapabilityId::CaptureExclusion),
        CapabilityStatus::Detected
    );
    assert_eq!(
        status(CapabilityId::SystemAudioCapture),
        CapabilityStatus::Detected
    );
}

#[test]
#[ignore = "needs a real monitor, microphone and speakers"]
fn a_real_recording_of_screen_microphone_and_system_audio() {
    make_process_dpi_aware();
    let (info, kind) = ffmpeg_info();
    let storage = tempfile::tempdir().unwrap();
    let monitor = list_monitors()
        .unwrap()
        .into_iter()
        .find(|m| m.is_primary)
        .unwrap();
    // A known marker on screen: the window sits at (100,100), 300x200 physical pixels, on the primary monitor.
    let window = TestWindow::show(100, 100, 300, 200);
    let tone = Tone::start(0.03); // about -30 dBFS

    let sources = SessionSources::default()
        .with_screen(WindowsScreenSource::new(&monitor.id, 15).unwrap())
        .with_microphone(CpalAudioSource::microphone(None).unwrap())
        .with_system_audio(CpalAudioSource::system_audio(None).unwrap());
    let mut config = SessionConfig::new(
        "Smoke test",
        Mode::ScreenAudio,
        storage.path(),
        system::local_now(),
    );
    config.fps = 15;
    let session = Session::start(config, sources, make_deps(info.clone(), kind)).unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_secs(8));
    let snapshot = session.snapshot();
    println!(
        "while recording: elapsed {} ms, mic level {:.3}, system level {:.3}",
        snapshot.elapsed_ms, snapshot.microphone_level, snapshot.system_audio_level
    );
    let outcome = session.stop().unwrap();
    drop(tone);

    let metadata = MeetingMetadata::read(&folder).unwrap();
    println!("encoder: {}", metadata.video.as_ref().unwrap().encoder);
    println!("microphone: {:?}", metadata.microphone);
    println!("system audio: {:?}", metadata.system_audio);
    println!("gaps: {:?}  errors: {:?}", metadata.gaps, metadata.errors);

    // Every file has the length of the recording.
    for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
        assert_close(
            length(&info, &folder.join(file)),
            outcome.duration.as_secs_f64(),
            0.3,
            file,
        );
    }
    assert_close(outcome.duration, 8.0, 1.0, "recording length");
    assert!(metadata.errors.is_empty(), "{:?}", metadata.errors);

    // The video is the monitor's real size and shows the marker window.
    assert_eq!(metadata.video.as_ref().unwrap().width, monitor.width);
    assert_eq!(metadata.video.as_ref().unwrap().height, monitor.height);
    let pixel = pixel_at(&info, &folder.join("recording.mp4"), 3.0, 250, 200);
    assert!(
        is_magenta(pixel),
        "the marker window is in the video: pixel {pixel:?}"
    );

    // The system-audio track carries the tone that was playing (loopback works), at about the level it was played.
    let loudness = max_volume_db(&info, &folder.join("system-audio.flac"));
    println!("system-audio.flac loudest sample: {loudness:.1} dB");
    // Loopback captures everything the computer plays, so another program's sound (a notification, a call) can make
    // the track louder than the tone. The test only needs the tone to be present and the track not to clip.
    assert!(
        (-40.0..=-3.0).contains(&loudness),
        "a tone of about -30 dBFS (or louder, if something else played) expected, got {loudness} dB"
    );
    drop(window);
}

#[test]
#[ignore = "needs a real monitor"]
fn the_apps_own_window_disappears_from_the_recording_once_it_is_excluded() {
    make_process_dpi_aware();
    let (info, kind) = ffmpeg_info();
    let storage = tempfile::tempdir().unwrap();
    let monitor = list_monitors()
        .unwrap()
        .into_iter()
        .find(|m| m.is_primary)
        .unwrap();
    let window = TestWindow::show(100, 100, 300, 200);

    let sources =
        SessionSources::default().with_screen(WindowsScreenSource::new(&monitor.id, 15).unwrap());
    let config = SessionConfig::new(
        "Exclusion test",
        Mode::ScreenAudio,
        storage.path(),
        system::local_now(),
    );
    let session = Session::start(config, sources, make_deps(info.clone(), kind)).unwrap();
    let folder = session.folder().to_path_buf();
    std::thread::sleep(Duration::from_secs(3)); // visible for the first 3 s
    set_window_excluded_from_capture(window.raw(), true)
        .expect("the window can be hidden from capture");
    std::thread::sleep(Duration::from_secs(3)); // hidden for the rest
    let outcome = session.stop().unwrap();

    let video = folder.join("recording.mp4");
    assert_close(outcome.duration, 6.0, 1.0, "recording length");
    let before = pixel_at(&info, &video, 1.5, 250, 200);
    let after = pixel_at(&info, &video, 4.5, 250, 200);
    println!("pixel while visible: {before:?}, after excluding the window: {after:?}");
    assert!(
        is_magenta(before),
        "the window is in the video before it is excluded: {before:?}"
    );
    assert!(
        !is_magenta(after),
        "the window must not be in the video once excluded: {after:?}"
    );
    drop(window);
}

#[test]
#[ignore = "needs a real monitor, microphone and speakers"]
fn pausing_a_real_recording_cuts_the_paused_time_from_every_track() {
    let (info, kind) = ffmpeg_info();
    let storage = tempfile::tempdir().unwrap();
    let monitor = list_monitors()
        .unwrap()
        .into_iter()
        .find(|m| m.is_primary)
        .unwrap();
    let sources = SessionSources::default()
        .with_screen(WindowsScreenSource::new(&monitor.id, 15).unwrap())
        .with_microphone(CpalAudioSource::microphone(None).unwrap())
        .with_system_audio(CpalAudioSource::system_audio(None).unwrap());
    let config = SessionConfig::new(
        "Pause smoke test",
        Mode::ScreenAudio,
        storage.path(),
        system::local_now(),
    );
    let mut session = Session::start(config, sources, make_deps(info.clone(), kind)).unwrap();
    let folder = session.folder().to_path_buf();

    std::thread::sleep(Duration::from_secs(3));
    session.pause().unwrap();
    std::thread::sleep(Duration::from_secs(3)); // must not be in any file
    session.resume().unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let outcome = session.stop().unwrap();

    assert_close(outcome.duration, 6.0, 1.0, "media length with 3 s paused");
    for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
        assert_close(
            length(&info, &folder.join(file)),
            outcome.duration.as_secs_f64(),
            0.35,
            file,
        );
    }
    let metadata = MeetingMetadata::read(&folder).unwrap();
    println!(
        "pauses: {:?}  gaps: {:?}  errors: {:?}",
        metadata.pauses, metadata.gaps, metadata.errors
    );
    assert_eq!(metadata.pauses.len(), 1);
}

// --- several recordings in one app run: the app records many meetings without being restarted ----------------

fn quick_recording(
    info: &FfmpegInfo,
    kind: H264Kind,
    screen: bool,
    audio: bool,
    seconds: u64,
) -> Duration {
    let storage = tempfile::tempdir().unwrap();
    let mut sources = SessionSources::default();
    if screen {
        let monitor = list_monitors()
            .unwrap()
            .into_iter()
            .find(|m| m.is_primary)
            .unwrap();
        sources = sources.with_screen(WindowsScreenSource::new(&monitor.id, 15).unwrap());
    }
    if audio {
        sources = sources
            .with_microphone(CpalAudioSource::microphone(None).unwrap())
            .with_system_audio(CpalAudioSource::system_audio(None).unwrap());
    }
    let config = SessionConfig::new(
        "Quick",
        Mode::ScreenAudio,
        storage.path(),
        system::local_now(),
    );
    let session = Session::start(config, sources, make_deps(info.clone(), kind)).unwrap();
    std::thread::sleep(Duration::from_secs(seconds));
    let outcome = session.stop().unwrap();
    assert!(outcome.file.is_file());
    outcome.duration
}

#[test]
#[ignore = "needs a real monitor"]
fn two_screen_recordings_in_a_row() {
    let (info, kind) = ffmpeg_info();
    for _ in 0..2 {
        assert_close(
            quick_recording(&info, kind, true, false, 2),
            2.0,
            0.8,
            "screen recording",
        );
    }
}

#[test]
#[ignore = "needs a microphone and speakers"]
fn two_audio_recordings_in_a_row() {
    let (info, kind) = ffmpeg_info();
    for _ in 0..2 {
        assert_close(
            quick_recording(&info, kind, false, true, 2),
            2.0,
            0.8,
            "audio recording",
        );
    }
}

#[test]
#[ignore = "needs a real monitor, microphone and speakers"]
fn an_audio_recording_followed_by_a_screen_recording() {
    let (info, kind) = ffmpeg_info();
    assert_close(
        quick_recording(&info, kind, false, true, 2),
        2.0,
        0.8,
        "audio recording",
    );
    assert_close(
        quick_recording(&info, kind, true, false, 2),
        2.0,
        0.8,
        "screen recording",
    );
}

/// Plan check "also checked": a one-minute screen, microphone and system-audio recording of every monitor of this
/// machine, through the same `Recorder` the app uses (default setup: 1080p at 15 fps). Prints the encoder, the
/// size of the video, the length of every file, the CPU use and the memory; nothing about the content is looked at.
#[test]
#[ignore = "needs every monitor of this machine, a microphone and speakers; one minute per monitor"]
fn a_one_minute_recording_of_every_monitor() {
    let monitors = list_monitors().unwrap();
    assert!(!monitors.is_empty(), "this machine has a monitor");
    let run = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::open(&run.path().join("test.db")).unwrap());
    let recorder = Recorder::for_this_computer(database, run.path().join("meetings"));
    let tools = recorder
        .tools()
        .expect("FFmpeg is installed (pnpm fetch:ffmpeg)");
    println!(
        "video encoder: {:?}; {}",
        tools.h264,
        tools
            .ffmpeg
            .version
            .split(" Copyright")
            .next()
            .unwrap_or("")
    );

    for monitor in &monitors {
        println!(
            "\n--- {} ({}x{}{}) ---",
            monitor.name,
            monitor.width,
            monitor.height,
            if monitor.is_primary { ", main" } else { "" }
        );
        let stop = Arc::new(AtomicBool::new(false));
        let sampler = memory_thread(stop.clone(), Duration::from_secs(10));
        let setup = RecordingSetup {
            title: monitor.name.clone(),
            monitor_id: Some(monitor.id.clone()),
            ..RecordingSetup::default()
        };
        recorder.start(&setup).expect("the recording starts");
        std::thread::sleep(Duration::from_secs(60));
        let finished = recorder.stop().expect("the recording is saved");
        stop.store(true, Ordering::SeqCst);
        let samples = sampler.join().unwrap();

        let folder = std::path::Path::new(&finished.folder);
        let metadata = MeetingMetadata::read(folder).unwrap();
        let video = metadata.video.as_ref().unwrap();
        let (width, height) = ffmpeg::output_size(monitor.width, monitor.height, Some(1080));
        assert_eq!(
            (video.width, video.height),
            (width, height),
            "the video has the size the 1080p setting gives"
        );
        assert_eq!(video.monitor, monitor.id, "the chosen monitor was recorded");
        let media = metadata.duration_ms.unwrap() as f64 / 1000.0;
        assert!(
            (media - 60.0).abs() < 1.5,
            "recorded {media:.2} s for one minute"
        );
        let info = ffmpeg::locate().and_then(|p| ffmpeg::probe(&p)).unwrap();
        for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
            let len = ffmpeg::measure_duration(&info.path, &folder.join(file))
                .unwrap()
                .as_secs_f64();
            println!("  {file}: {len:.3} s");
            assert!(
                (len - media).abs() < 0.25,
                "{file} is as long as the media timeline"
            );
        }
        let cpu = samples.iter().skip(1).map(|s| s.cpu_percent).sum::<f64>()
            / samples.len().saturating_sub(1).max(1) as f64;
        let peak = samples
            .iter()
            .map(|s| s.own_mb + s.children_mb)
            .fold(0.0, f64::max);
        println!(
            "  {}x{} {}; media {media:.2} s; gaps {}, errors {}; CPU {cpu:.0} % of one core; memory peak {peak:.0} MB",
            video.width,
            video.height,
            video.encoder,
            metadata.gaps.len(),
            metadata.errors.len()
        );
        assert!(
            metadata.errors.is_empty(),
            "no errors: {:?}",
            metadata.errors
        );
    }
}
