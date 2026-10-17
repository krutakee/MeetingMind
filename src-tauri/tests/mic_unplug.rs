#![cfg(windows)]
//! Acceptance check: unplugging the microphone in the middle of a recording does not stop the recording.
//! Interactive, so it is ignored by default; it needs an external USB or 3.5 mm microphone (or headset) and a person to
//! plug it out and in when it asks. Run it on a Windows computer with the microphone connected:
//!
//!     cargo test --test mic_unplug -- --ignored --nocapture
//!
//! MIC_NAME (optional) is part of the microphone's name, to choose it when several are connected; without it the first
//! microphone that is not the computer's built-in array is used.
//!
//! What it checks in the saved files: the recording is complete and as long as the media timeline, the microphone track
//! still has that length, the unplugging is listed in `meeting.json` (a `deviceLost` gap on the microphone, nothing
//! lost on the screen or the system audio), and the recording carried on without being asked to.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use meetingmind_lib::recorder::{Phase, Recorder};
use meetingmind_lib::recording::ffmpeg;
use meetingmind_lib::recording::metadata::{GapReason, MeetingMetadata, StreamKind};
use meetingmind_lib::recording::session::WarningKind;
use meetingmind_lib::recording::setup::RecordingSetup;
use meetingmind_lib::storage::Database;

fn warning_seen(recorder: &Recorder, kind: WarningKind) -> bool {
    recorder
        .status()
        .warnings
        .iter()
        .any(|w| w.kind == kind && w.stream == Some(StreamKind::Microphone))
}

/// Waits until `condition` holds, printing a reminder every 15 s. Panics after `limit`.
fn wait_for(what: &str, limit: Duration, recorder: &Recorder, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    let mut reminded = Duration::ZERO;
    while !condition() {
        assert!(start.elapsed() < limit, "gave up waiting for: {what}");
        assert_eq!(
            recorder.status().phase,
            Phase::Recording,
            "the recording stopped while waiting for: {what}"
        );
        if start.elapsed() - reminded >= Duration::from_secs(15) {
            reminded = start.elapsed();
            println!("  ... still waiting for: {what}");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs an external microphone and a person to unplug it"]
fn unplugging_the_microphone_does_not_stop_the_recording() {
    // The recorder logs (never any meeting content) why a device could not be reopened; show it on stderr.
    let _ = tracing_subscriber::fmt()
        .with_env_filter("meetingmind_lib=info")
        .with_writer(std::io::stderr)
        .try_init();
    let run = std::env::temp_dir().join(format!("meetingmind-mic-unplug-{}", std::process::id()));
    std::fs::create_dir_all(&run).unwrap();
    let database = Arc::new(Database::open(&run.join("test.db")).unwrap());
    let recorder = Recorder::for_this_computer(database, run.join("meetings"));

    let microphones = recorder
        .devices()
        .microphones()
        .expect("the microphones can be listed");
    println!("microphones found:");
    for m in &microphones {
        println!("  {} (default: {}) [{}]", m.name, m.is_default, m.id);
    }
    let wanted = std::env::var("MIC_NAME").ok().map(|s| s.to_lowercase());
    let chosen = microphones
        .iter()
        .find(|m| match &wanted {
            Some(part) => m.name.to_lowercase().contains(part),
            None => !m.name.to_lowercase().contains("array"),
        })
        .expect("an external microphone is connected (set MIC_NAME to choose one)");
    println!("using: {}", chosen.name);

    let setup = RecordingSetup {
        title: "mic unplug".into(),
        microphone_id: Some(chosen.id.clone()),
        ..RecordingSetup::default()
    };
    recorder.start(&setup).expect("the recording starts");
    println!("recording. Letting it settle for 10 s.");
    std::thread::sleep(Duration::from_secs(10));

    println!("\n>>> UNPLUG THE MICROPHONE NOW <<<\n");
    wait_for(
        "the microphone to be reported lost",
        Duration::from_secs(180),
        &recorder,
        || warning_seen(&recorder, WarningKind::SourceLost),
    );
    println!("reported lost. The recording must carry on; waiting 15 s to see that it does.");
    std::thread::sleep(Duration::from_secs(15));
    assert_eq!(
        recorder.status().phase,
        Phase::Recording,
        "the recording stopped when the microphone was unplugged"
    );

    println!("\n>>> PLUG THE MICROPHONE BACK IN NOW <<<\n");
    wait_for(
        "the microphone to be reported back",
        Duration::from_secs(180),
        &recorder,
        || warning_seen(&recorder, WarningKind::SourceRestored),
    );
    println!("reported back. Recording 10 more seconds.");
    std::thread::sleep(Duration::from_secs(10));

    let finished = recorder.stop().expect("the recording is saved");
    println!("saved: {}", finished.file);

    let folder = Path::new(&finished.folder);
    let metadata = MeetingMetadata::read(folder).unwrap();
    println!("gaps: {:?}", metadata.gaps);
    println!("errors: {:?}", metadata.errors);
    let info = ffmpeg::locate().and_then(|p| ffmpeg::probe(&p)).unwrap();
    let media = metadata.duration_ms.unwrap() as f64 / 1000.0;
    for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
        let len = ffmpeg::measure_duration(&info.path, &folder.join(file))
            .unwrap()
            .as_secs_f64();
        println!("{file}: {len:.3} s (media {media:.3} s)");
        assert!(
            (len - media).abs() < 0.25,
            "{file} is not as long as the media timeline"
        );
    }
    assert_eq!(metadata.status, "complete");
    assert!(
        metadata
            .gaps
            .iter()
            .any(|g| g.stream == StreamKind::Microphone
                && g.reason == GapReason::DeviceLost
                && g.end_ms - g.start_ms >= 10_000),
        "the unplugging is listed as a deviceLost gap on the microphone: {:?}",
        metadata.gaps
    );
    // Plugging a USB headset in or out also moves the default output, and the system audio follows it: that takes about a
    // second to rebuild. What must not happen (it did, on 2026-10-17, when the headset had another sample rate) is the
    // system audio staying silent after the change.
    assert!(
        !metadata
            .gaps
            .iter()
            .any(|g| g.stream != StreamKind::Microphone && g.end_ms - g.start_ms > 3000),
        "the screen and the system audio carried on: {:?}",
        metadata.gaps
    );
    let _ = std::fs::remove_dir_all(&run);
}
