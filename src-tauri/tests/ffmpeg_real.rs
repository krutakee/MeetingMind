//! Tests that run the real FFmpeg sidecar (installed by `pnpm fetch:ffmpeg`).
//!
//! If FFmpeg is not available these tests print "SKIPPED" and pass, so a developer without the sidecar can still run
//! `cargo test`. CI sets `REQUIRE_FFMPEG=1`, which turns a missing FFmpeg into a failure, so a skipped test can never
//! be mistaken for a passing one.

use std::path::Path;
use std::time::{Duration, Instant};

/// The H.264 encoder to test with. `MEETINGMIND_TEST_H264` (hardware, openh264 or software) forces one, to mimic a
/// machine without a GPU (a CI runner) on a machine that has one.
fn pick_encoder(info: &FfmpegInfo) -> H264Kind {
    match std::env::var("MEETINGMIND_TEST_H264").ok().as_deref() {
        Some("hardware") => H264Kind::HardwareMf,
        Some("openh264") => H264Kind::OpenH264,
        Some("software") => H264Kind::SoftwareMf,
        _ => ffmpeg::select_h264(info).expect("some H.264 encoder works"),
    }
}

use meetingmind_lib::recording::ffmpeg::{
    self, FfmpegInfo, FinalizeInputs, H264Kind, SendOutcome, StreamEncoder, VideoParams,
};

fn ffmpeg_or_skip(test: &str) -> Option<FfmpegInfo> {
    match ffmpeg::locate().and_then(|path| ffmpeg::probe(&path)) {
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

const W: u32 = 640;
const H: u32 = 360;
const FPS: u32 = 15;

/// One synthetic BGRA frame: a gradient with a bar that moves with `i`.
fn frame(i: u32) -> Vec<u8> {
    let mut data = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let o = ((y * W + x) * 4) as usize;
            let bar = (x + i * 8) % W < 40;
            data[o] = (x * 255 / W) as u8; // B
            data[o + 1] = (y * 255 / H) as u8; // G
            data[o + 2] = if bar { 255 } else { 40 }; // R
            data[o + 3] = 255;
        }
    }
    data
}

/// Interleaved stereo f32 little-endian: a 440 Hz sine of the given length.
fn sine_bytes(seconds: f64, sample_rate: u32) -> Vec<u8> {
    let frames = (seconds * sample_rate as f64) as usize;
    let mut out = Vec::with_capacity(frames * 8);
    for n in 0..frames {
        let v = ((2.0 * std::f64::consts::PI * 440.0 * n as f64 / sample_rate as f64).sin() * 0.3)
            as f32;
        out.extend_from_slice(&v.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Sends `seconds` of stereo sine in 10 ms chunks, retrying when the queue is full. (The real recorder gets audio in
/// real time and never overflows the queue; a test that sends it all at once must wait instead of dropping.)
fn feed_audio(encoder: &StreamEncoder, seconds: f64) {
    for chunk in sine_bytes(seconds, 48_000).chunks(48_000 * 8 / 100) {
        loop {
            match encoder.try_send(chunk.to_vec()) {
                SendOutcome::Queued => break,
                SendOutcome::Dropped => std::thread::sleep(Duration::from_millis(2)),
                SendOutcome::EncoderFailed => panic!("the audio encoder failed"),
            }
        }
    }
}

fn params() -> VideoParams {
    VideoParams {
        width: W,
        height: H,
        fps: FPS,
        bitrate_bps: 1_500_000,
    }
}

fn assert_close(actual: Duration, expected: f64, tolerance: f64, what: &str) {
    let a = actual.as_secs_f64();
    assert!(
        (a - expected).abs() <= tolerance,
        "{what}: {a:.3} s, expected {expected:.3} s (+-{tolerance})"
    );
}

/// Feeds `frames` frames to a new video encoder. `Err` if the encoder could not run here (it exited while being fed).
fn record_video(
    ffmpeg: &FfmpegInfo,
    kind: H264Kind,
    frames: u32,
    output: &Path,
) -> Result<StreamEncoder, String> {
    let encoder = StreamEncoder::spawn(
        "video",
        &ffmpeg.path,
        &ffmpeg::video_args(kind, &params(), output),
        8,
    )
    .unwrap();
    for i in 0..frames {
        // Feed at full speed but never overrun the queue, as the real recorder would drop instead.
        loop {
            match encoder.try_send(frame(i)) {
                SendOutcome::Queued => break,
                SendOutcome::Dropped => std::thread::sleep(Duration::from_millis(5)),
                SendOutcome::EncoderFailed => {
                    return Err(format!("{} exited while being fed", kind.label()));
                }
            }
        }
    }
    Ok(encoder)
}

#[test]
fn the_real_ffmpeg_passes_the_license_check_and_picks_an_h264_encoder() {
    let Some(info) = ffmpeg_or_skip("license") else {
        return;
    };

    assert!(info.version.starts_with("ffmpeg version"));
    assert!(!info.configuration.contains("--enable-gpl"));
    let kind = ffmpeg::select_h264(&info).expect("some H.264 encoder works");
    eprintln!("selected H.264 encoder on this machine: {}", kind.label());
}

#[test]
fn video_frames_become_a_playable_fragmented_mp4_of_the_right_length() {
    let Some(info) = ffmpeg_or_skip("video") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("video.mp4");
    let kind = pick_encoder(&info);

    record_video(&info, kind, 45, &out)
        .expect("the selected encoder runs")
        .finish(Duration::from_secs(30))
        .unwrap();

    assert_close(
        ffmpeg::measure_duration(&info.path, &out).unwrap(),
        3.0,
        0.2,
        "3 s of video at 15 fps",
    );
}

#[test]
fn every_h264_encoder_that_works_here_produces_the_same_length() {
    let Some(info) = ffmpeg_or_skip("each encoder") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut tried = 0;
    for kind in H264Kind::PREFERENCE {
        let out = dir.path().join(format!("{kind:?}.mp4"));
        let finished = record_video(&info, kind, 30, &out).and_then(|encoder| {
            encoder
                .finish(Duration::from_secs(30))
                .map_err(|e| e.to_string())
        });
        match finished {
            Ok(()) => {
                tried += 1;
                assert_close(
                    ffmpeg::measure_duration(&info.path, &out).unwrap(),
                    2.0,
                    0.2,
                    kind.label(),
                );
            }
            Err(error) => eprintln!("{} is not available here: {error}", kind.label()),
        }
    }
    assert!(tried >= 1, "at least one encoder must work");
}

#[test]
fn audio_becomes_flac_of_the_right_length_and_the_header_is_valid() {
    let Some(info) = ffmpeg_or_skip("flac") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("microphone.flac");
    let encoder = StreamEncoder::spawn(
        "mic",
        &info.path,
        &ffmpeg::audio_track_args(48_000, 2, &out),
        64,
    )
    .unwrap();

    feed_audio(&encoder, 3.0);
    encoder.finish(Duration::from_secs(30)).unwrap();

    assert_close(
        ffmpeg::measure_duration(&info.path, &out).unwrap(),
        3.0,
        0.02,
        "3 s of audio",
    );
    assert!(out.metadata().unwrap().len() > 1000);
}

#[test]
fn the_final_mux_mixes_two_tracks_and_copies_the_video() {
    let Some(info) = ffmpeg_or_skip("mux") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (video, mic, system) = (
        dir.path().join("v.mp4"),
        dir.path().join("mic.flac"),
        dir.path().join("sys.flac"),
    );
    let kind = pick_encoder(&info);
    record_video(&info, kind, 45, &video)
        .expect("the selected encoder runs")
        .finish(Duration::from_secs(30))
        .unwrap();
    for (name, track) in [("mic", &mic), ("sys", &system)] {
        let e = StreamEncoder::spawn(
            name,
            &info.path,
            &ffmpeg::audio_track_args(48_000, 2, track),
            64,
        )
        .unwrap();
        feed_audio(&e, 3.0);
        e.finish(Duration::from_secs(30)).unwrap();
    }
    let output = dir.path().join("recording.mp4");
    let mut last_progress = Duration::ZERO;

    ffmpeg::run_final_mux(
        &info.path,
        &FinalizeInputs {
            video: Some(&video),
            microphone: Some(&mic),
            system_audio: Some(&system),
            output: &output,
            trim_to: None,
        },
        |p| last_progress = p,
    )
    .unwrap();

    assert_close(
        ffmpeg::measure_duration(&info.path, &output).unwrap(),
        3.0,
        0.25,
        "final recording.mp4",
    );
    assert!(
        last_progress > Duration::from_secs(2),
        "progress was reported: {last_progress:?}"
    );
}

#[test]
fn an_audio_only_mix_and_a_trim_work() {
    let Some(info) = ffmpeg_or_skip("m4a") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mic = dir.path().join("mic.flac");
    let e = StreamEncoder::spawn(
        "mic",
        &info.path,
        &ffmpeg::audio_track_args(48_000, 2, &mic),
        64,
    )
    .unwrap();
    feed_audio(&e, 4.0);
    e.finish(Duration::from_secs(30)).unwrap();
    let output = dir.path().join("recording.m4a");

    ffmpeg::run_final_mux(
        &info.path,
        &FinalizeInputs {
            video: None,
            microphone: Some(&mic),
            system_audio: None,
            output: &output,
            trim_to: Some(Duration::from_secs(2)),
        },
        |_| {},
    )
    .unwrap();

    assert_close(
        ffmpeg::measure_duration(&info.path, &output).unwrap(),
        2.0,
        0.1,
        "trimmed to 2 s",
    );
}

#[test]
fn an_encoder_that_fails_is_reported_with_its_error_text() {
    let Some(info) = ffmpeg_or_skip("failure") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let args =
        ffmpeg::audio_track_args(48_000, 2, &dir.path().join("no-such-folder").join("x.flac"));
    let encoder = StreamEncoder::spawn("mic", &info.path, &args, 4).unwrap();

    // FFmpeg exits at once (it cannot create the output). Sending keeps working until the pipe breaks.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut failed = false;
    while Instant::now() < deadline {
        if encoder.try_send(vec![0u8; 4096]) == SendOutcome::EncoderFailed {
            failed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        failed,
        "the failure must be noticed so the recorder can log a gap and carry on"
    );
    assert!(encoder.has_failed());

    let error = encoder.finish(Duration::from_secs(10)).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("mic failed"), "{message}");
    assert!(
        message.to_lowercase().contains("no such file") || message.contains("Error"),
        "{message}"
    );
}

/// A hard kill of FFmpeg while it is writing (what a crash or power cut does). The fragmented MP4 must still decode,
/// and remuxing must turn it into a normal file. The same for FLAC. This is what Phase 2 recovery relies on.
#[test]
fn files_survive_a_hard_kill_and_can_be_repaired() {
    let Some(info) = ffmpeg_or_skip("kill") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("video.mp4");
    let flac = dir.path().join("mic.flac");
    let kind = pick_encoder(&info);

    let video_encoder = StreamEncoder::spawn(
        "video",
        &info.path,
        &ffmpeg::video_args(kind, &params(), &video),
        8,
    )
    .unwrap();
    let flac_encoder = StreamEncoder::spawn(
        "mic",
        &info.path,
        &ffmpeg::audio_track_args(48_000, 2, &flac),
        512,
    )
    .unwrap();
    feed_audio(&flac_encoder, 6.0);
    for i in 0..120 {
        loop {
            match video_encoder.try_send(frame(i)) {
                SendOutcome::Queued => break,
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        // Stop once real data is on disk (a keyframe interval has been written), as a crash would find it.
        if video.metadata().map(|m| m.len()).unwrap_or(0) > 20_000
            && flac.metadata().map(|m| m.len()).unwrap_or(0) > 20_000
        {
            break;
        }
    }
    video_encoder.abort();
    flac_encoder.abort();

    let video_length =
        ffmpeg::measure_duration(&info.path, &video).expect("killed fragmented MP4 still reads");
    let flac_length = ffmpeg::measure_duration(&info.path, &flac).expect("killed FLAC still reads");
    assert!(
        video_length > Duration::from_millis(500),
        "video kept {video_length:?}"
    );
    assert!(
        flac_length > Duration::from_millis(500),
        "audio kept {flac_length:?}"
    );

    let repaired_video = dir.path().join("video-fixed.mp4");
    let repaired_flac = dir.path().join("mic-fixed.flac");
    ffmpeg::remux_copy(&info.path, &video, &repaired_video).unwrap();
    ffmpeg::remux_copy(&info.path, &flac, &repaired_flac).unwrap();
    assert_close(
        ffmpeg::measure_duration(&info.path, &repaired_video).unwrap(),
        video_length.as_secs_f64(),
        0.2,
        "repaired video",
    );
    assert_close(
        ffmpeg::measure_duration(&info.path, &repaired_flac).unwrap(),
        flac_length.as_secs_f64(),
        0.05,
        "repaired flac",
    );
}
