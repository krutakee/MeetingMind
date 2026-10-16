//! TEST PROGRAM for the crash-recovery test (tests/recovery_kill.rs). Not part of the app; built only with the
//! `test-doubles` feature.
//!
//! Starts a recording from synthetic sources (test doubles, no hardware), prints `READY <meeting folder>` on stdout,
//! and then records until it is killed. The test kills it the hard way and checks what recovery makes of the wreckage.
//!
//! Usage: synthetic_recorder <storage folder> <database file>

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use meetingmind_lib::recording::clock::SystemClock;
use meetingmind_lib::recording::ffmpeg;
use meetingmind_lib::recording::layout::LocalDateTime;
use meetingmind_lib::recording::metadata::Mode;
use meetingmind_lib::recording::session::{Session, SessionConfig, SessionDeps, SessionSources};
use meetingmind_lib::recording::testing::{SyntheticAudio, SyntheticScreen};
use meetingmind_lib::storage::Database;
use meetingmind_lib::system::disk::SystemDiskSpace;

fn main() {
    let mut args = std::env::args().skip(1);
    let storage = PathBuf::from(args.next().expect("storage folder"));
    let db_path = PathBuf::from(args.next().expect("database file"));

    let ffmpeg_path = ffmpeg::locate().expect("FFmpeg is installed");
    let info = ffmpeg::probe(&ffmpeg_path).expect("FFmpeg passes the license check");
    // MEETINGMIND_TEST_H264 (hardware, openh264 or software) forces an encoder; this program is test-only.
    let video_encoder = match std::env::var("MEETINGMIND_TEST_H264").ok().as_deref() {
        Some("hardware") => ffmpeg::H264Kind::HardwareMf,
        Some("openh264") => ffmpeg::H264Kind::OpenH264,
        Some("software") => ffmpeg::H264Kind::SoftwareMf,
        _ => ffmpeg::select_h264(&info).expect("an H.264 encoder works"),
    };
    let database = Arc::new(Database::open(&db_path).expect("database opens"));

    let mut sources = SessionSources::default();
    sources = sources.with_screen(
        SyntheticScreen::new(320, 180).with_change_every(std::time::Duration::from_millis(200)),
    );
    sources = sources.with_microphone(SyntheticAudio::new("Synthetic microphone", 48_000, 1));
    sources = sources.with_system_audio(SyntheticAudio::new("Synthetic speakers", 48_000, 2));

    let config = SessionConfig::new(
        "Crash test",
        Mode::ScreenAudio,
        &storage,
        LocalDateTime {
            year: 2026,
            month: 10,
            day: 16,
            hour: 9,
            minute: 5,
        },
    );
    let session = Session::start(
        config,
        sources,
        SessionDeps {
            clock: Arc::new(SystemClock::new()),
            ffmpeg: info,
            video_encoder,
            disk: Arc::new(SystemDiskSpace),
            database: Some(database),
        },
    )
    .expect("recording starts");

    println!("READY {}", session.folder().display());
    std::io::stdout().flush().expect("flush");
    // Record until the test kills this process.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}
