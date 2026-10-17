#![cfg(windows)]
//! The Phase 2 soak test: a long recording on the real devices of this machine, to check the acceptance criterion
//! "over a 2-hour recording, audio/video drift stays under 100 ms and memory use stays flat".
//!
//! It records like the app does (`Recorder::for_this_computer`, the default setup: main monitor, microphone, system
//! audio, 1080p, 15 fps) and, once a minute, makes a marker: a small magenta window appears on the main monitor and a
//! quiet 1 kHz tone plays on the speakers at the same moment, for 0.4 s. When the recording is saved, the marker is
//! found in the video (the magenta) and in the system-audio track (the tone); the time between the two is the same
//! for every marker if video and audio stay in step, and it changes if they drift apart. Memory is sampled every
//! minute (this program and its FFmpeg children). Leave the machine alone while it runs; do not lock the screen.
//!
//!     cargo test --release --test soak -- --ignored --nocapture
//!
//! Settings (environment variables): SOAK_MINUTES (default 120), SOAK_MARKER_SECONDS (default 60), SOAK_DIR (default
//! target/soak). SOAK_ANALYZE_ONLY=<run folder> measures a recording that is already there, without recording.
//! The run folder keeps the recording, `memory.csv` and `report.txt`; delete it when done (a 2-hour run is several GB).

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use meetingmind_lib::capture::windows::list_monitors;
use meetingmind_lib::recorder::{Phase, Recorder};
use meetingmind_lib::recording::ffmpeg::{self, FfmpegInfo};
use meetingmind_lib::recording::metadata::MeetingMetadata;
use meetingmind_lib::recording::setup::RecordingSetup;
use meetingmind_lib::storage::Database;
use sysinfo::{ProcessesToUpdate, System};

/// Where the marker window is, in physical pixels of the main monitor.
const WINDOW: (i32, i32, i32, i32) = (100, 100, 300, 300);
const TONE_HZ: f32 = 1000.0;
/// -30 dBFS: quiet, but far above the noise.
const TONE_AMPLITUDE: f32 = 0.03;
const MARKER_LENGTH: Duration = Duration::from_millis(400);

struct Config {
    minutes: u64,
    marker_every: Duration,
    base: PathBuf,
    analyze_only: Option<PathBuf>,
}

impl Config {
    fn from_env() -> Self {
        let number = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        Self {
            minutes: number("SOAK_MINUTES", 120),
            marker_every: Duration::from_secs(number("SOAK_MARKER_SECONDS", 60)),
            base: std::env::var_os("SOAK_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("target")
                        .join("soak")
                }),
            analyze_only: std::env::var_os("SOAK_ANALYZE_ONLY").map(PathBuf::from),
        }
    }
}

// --- the tone: silent until the gate opens ----------------------------------------------------------------------

/// A tone on the default speakers that sounds only while `gate` is true. The stream stays on the creating thread.
struct GatedTone {
    _stream: cpal::Stream,
}

impl GatedTone {
    fn start(gate: Arc<AtomicBool>) -> Self {
        let device = cpal::default_host()
            .default_output_device()
            .expect("default speakers");
        let supported = device.default_output_config().unwrap();
        assert_eq!(
            supported.sample_format(),
            cpal::SampleFormat::F32,
            "this test plays f32"
        );
        let (rate, channels) = (
            supported.sample_rate() as f32,
            supported.channels() as usize,
        );
        let mut phase = 0.0f32;
        let stream = device
            .build_output_stream(
                supported.config(),
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let on = gate.load(Ordering::Relaxed);
                    for frame in data.chunks_mut(channels) {
                        phase = (phase + 2.0 * std::f32::consts::PI * TONE_HZ / rate)
                            % (2.0 * std::f32::consts::PI);
                        frame.fill(if on {
                            TONE_AMPLITUDE * phase.sin()
                        } else {
                            0.0
                        });
                    }
                },
                |_| {},
                None,
            )
            .unwrap();
        stream.play().unwrap();
        Self { _stream: stream }
    }
}

/// Makes a marker (flash and tone together) at `first_after`, then every `every`, until `stop` is set.
fn marker_thread(
    stop: Arc<AtomicBool>,
    first_after: Duration,
    every: Duration,
) -> std::thread::JoinHandle<Vec<f64>> {
    std::thread::spawn(move || {
        let window = TestWindow::show(WINDOW.0, WINDOW.1, WINDOW.2, WINDOW.3);
        window.set_visible(false);
        let gate = Arc::new(AtomicBool::new(false));
        let _tone = GatedTone::start(gate.clone());
        let start = Instant::now();
        let mut made = Vec::new();
        let mut next = first_after;
        while !stop.load(Ordering::SeqCst) {
            let elapsed = start.elapsed();
            if elapsed >= next {
                window.set_visible(true);
                gate.store(true, Ordering::SeqCst);
                std::thread::sleep(MARKER_LENGTH);
                gate.store(false, Ordering::SeqCst);
                window.set_visible(false);
                made.push(elapsed.as_secs_f64());
                next += every;
            } else {
                std::thread::sleep(Duration::from_millis(50).min(next - elapsed));
            }
        }
        made
    })
}

// --- memory ------------------------------------------------------------------------------------------------------

struct MemorySample {
    minute: f64,
    own_mb: f64,
    children_mb: f64,
    children: usize,
    cpu_percent: f64,
}

/// Samples this program and its child processes (the FFmpeg encoders) every minute until `stop` is set.
fn memory_thread(
    stop: Arc<AtomicBool>,
    interval: Duration,
) -> std::thread::JoinHandle<Vec<MemorySample>> {
    std::thread::spawn(move || {
        let me = sysinfo::get_current_pid().expect("own process id");
        let mut system = System::new();
        let start = Instant::now();
        let mut samples = Vec::new();
        let mut next = Duration::ZERO;
        while !stop.load(Ordering::SeqCst) {
            if start.elapsed() >= next {
                system.refresh_processes(ProcessesToUpdate::All, true);
                let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
                let own = system.process(me);
                let children: Vec<_> = system
                    .processes()
                    .values()
                    .filter(|p| p.parent() == Some(me))
                    .collect();
                samples.push(MemorySample {
                    minute: start.elapsed().as_secs_f64() / 60.0,
                    own_mb: own.map_or(0.0, |p| mb(p.memory())),
                    children_mb: children.iter().map(|p| mb(p.memory())).sum(),
                    children: children.len(),
                    cpu_percent: own.map_or(0.0, |p| f64::from(p.cpu_usage()))
                        + children
                            .iter()
                            .map(|p| f64::from(p.cpu_usage()))
                            .sum::<f64>(),
                });
                next += interval;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        samples
    })
}

fn write_memory_csv(path: &Path, samples: &[MemorySample]) {
    let mut text = String::from("minute,own_mb,children_mb,children,cpu_percent_of_one_core\n");
    for s in samples {
        text.push_str(&format!(
            "{:.2},{:.1},{:.1},{},{:.1}\n",
            s.minute, s.own_mb, s.children_mb, s.children, s.cpu_percent
        ));
    }
    std::fs::write(path, text).unwrap();
}

fn read_memory_csv(path: &Path) -> Vec<MemorySample> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').collect();
            Some(MemorySample {
                minute: f.first()?.parse().ok()?,
                own_mb: f.get(1)?.parse().ok()?,
                children_mb: f.get(2)?.parse().ok()?,
                children: f.get(3)?.parse().ok()?,
                cpu_percent: f.get(4)?.parse().ok()?,
            })
        })
        .collect()
}

// --- finding the markers in the saved files ----------------------------------------------------------------------

fn is_flash(pixel: [u8; 3]) -> bool {
    pixel[0] > 190 && pixel[1] < 90 && pixel[2] > 190
}

/// Seconds at which the magenta window appears in the video: the first frame of every run of magenta frames.
fn flash_onsets(info: &FfmpegInfo, video: &Path, region: (u32, u32), fps: f64) -> Vec<f64> {
    let (x, y) = region;
    let mut child = Command::new(&info.path)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .args(["-an", "-fps_mode", "passthrough", "-vf"])
        // A small square inside the marker window, averaged to one pixel per frame.
        .arg(format!(
            "crop=16:16:{}:{},scale=1:1:flags=area,format=rgb24",
            x & !1,
            y & !1
        ))
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg runs");
    let mut out = child.stdout.take().unwrap();
    let (mut onsets, mut run, mut frame) = (Vec::new(), 0u32, 0u64);
    let mut pixel = [0u8; 3];
    while out.read_exact(&mut pixel).is_ok() {
        if is_flash(pixel) {
            run += 1;
            if run == 2 {
                // Two frames in a row: a real flash. Its onset is the first of them.
                onsets.push((frame - 1) as f64 / fps);
            }
        } else {
            run = 0;
        }
        frame += 1;
    }
    let _ = child.wait();
    onsets
}

/// Seconds at which a 1 kHz tone starts in an audio file, by the strength of 1 kHz in every 10 ms.
fn tone_onsets(info: &FfmpegInfo, audio: &Path) -> Vec<f64> {
    const RATE: usize = 16_000;
    const BLOCK: usize = RATE / 100;
    let mut child = Command::new(&info.path)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(audio)
        .args(["-ac", "1", "-ar", "16000", "-f", "f32le", "-"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg runs");
    let mut out = child.stdout.take().unwrap();
    // Goertzel filter at 1 kHz over one block.
    let coefficient = 2.0 * (2.0 * std::f64::consts::PI * f64::from(TONE_HZ) / RATE as f64).cos();
    let mut strengths = Vec::new();
    let mut bytes = vec![0u8; BLOCK * 4];
    while out.read_exact(&mut bytes).is_ok() {
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for chunk in bytes.as_chunks::<4>().0 {
            let x = f64::from(f32::from_le_bytes(*chunk));
            let s0 = x + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        strengths.push((s1 * s1 + s2 * s2 - coefficient * s1 * s2).max(0.0).sqrt());
    }
    let _ = child.wait();

    let mut sorted = strengths.clone();
    sorted.sort_by(f64::total_cmp);
    let peak = sorted
        .get(sorted.len().saturating_sub(1 + sorted.len() / 1000))
        .copied()
        .unwrap_or(0.0);
    let threshold = (peak * 0.5).max(0.05);
    let mut onsets: Vec<f64> = Vec::new();
    for (i, pair) in strengths.windows(2).enumerate() {
        if pair[0] <= threshold && pair[1] > threshold {
            let t = (i + 1) as f64 / 100.0;
            // One tone is 0.4 s long; anything within a few seconds of the last onset is the same marker.
            if onsets.last().is_none_or(|last| t - last > 5.0) {
                onsets.push(t);
            }
        }
    }
    onsets
}

/// Pairs every video onset with the nearest audio onset within 1.5 s: `video - audio`, in seconds.
fn offsets(video: &[f64], audio: &[f64]) -> Vec<f64> {
    video
        .iter()
        .filter_map(|v| {
            audio
                .iter()
                .map(|a| v - a)
                .filter(|d| d.abs() < 1.5)
                .min_by(|x, y| x.abs().total_cmp(&y.abs()))
        })
        .collect()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

/// Least-squares slope of `value` against `time`.
fn slope(points: &[(f64, f64)]) -> f64 {
    let n = points.len() as f64;
    let (mx, my) = (
        points.iter().map(|p| p.0).sum::<f64>() / n,
        points.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let num: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let den: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if den > 0.0 { num / den } else { 0.0 }
}

// --- the measurements and the verdict ---------------------------------------------------------------------------

/// Whether each criterion held (the measurements themselves are printed and written to report.txt).
struct Verdict {
    drift_ok: Option<bool>,
    memory_ok: Option<bool>,
    lengths_ok: bool,
}

fn analyze(run: &Path, marker_every: Duration, minutes_recorded: Option<f64>) -> Verdict {
    let info = ffmpeg::locate()
        .and_then(|p| ffmpeg::probe(&p))
        .expect("FFmpeg is installed");
    let folder = std::fs::read_dir(run.join("meetings"))
        .expect("the run has a meetings folder")
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("a meeting folder");
    let metadata = MeetingMetadata::read(&folder).expect("meeting.json");
    let video = metadata.video.as_ref().expect("a video was recorded");
    let mut report = String::new();
    let mut say = |line: String| {
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    };

    say(format!(
        "meeting: status {}, media length {} ms",
        metadata.status,
        metadata.duration_ms.unwrap_or(0)
    ));
    say(format!(
        "video: {}x{} at {} fps, {}",
        video.width, video.height, video.fps, video.encoder
    ));
    for (name, audio) in [
        ("microphone", &metadata.microphone),
        ("system audio", &metadata.system_audio),
    ] {
        if let Some(a) = audio {
            say(format!(
                "{name}: {}, {} Hz, {} ch, start offset {} ms, measured clock error {:.2} ppm",
                a.device,
                a.sample_rate,
                a.channels,
                a.start_offset_ms,
                a.drift_ppm.unwrap_or(0.0)
            ));
        }
    }
    say(format!(
        "pauses: {}, gaps: {}, errors: {}",
        metadata.pauses.len(),
        metadata.gaps.len(),
        metadata.errors.len()
    ));
    for gap in metadata.gaps.iter().take(10) {
        say(format!(
            "  gap: {:?} {:?} {}..{} ms",
            gap.stream, gap.reason, gap.start_ms, gap.end_ms
        ));
    }
    for error in metadata.errors.iter().take(10) {
        say(format!(
            "  error: {} at {} ms: {}",
            error.kind, error.at_media_ms, error.message
        ));
    }

    // Lengths: every track as long as the media timeline.
    let media = metadata.duration_ms.unwrap_or(0) as f64 / 1000.0;
    let mut lengths_ok = true;
    for file in ["recording.mp4", "microphone.flac", "system-audio.flac"] {
        let len = ffmpeg::measure_duration(&info.path, &folder.join(file))
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let ok = (len - media).abs() < 0.25;
        lengths_ok &= ok;
        say(format!(
            "length {file}: {len:.3} s (media {media:.3} s) {}",
            if ok { "ok" } else { "MISMATCH" }
        ));
    }

    // Drift: the time between the flash and the tone, marker by marker.
    let monitor = list_monitors()
        .unwrap()
        .into_iter()
        .find(|m| m.is_primary)
        .expect("a main monitor");
    let scale = f64::from(video.width) / f64::from(monitor.width);
    let centre = (
        ((f64::from(WINDOW.0) + f64::from(WINDOW.2) / 2.0) * scale) as u32 - 8,
        ((f64::from(WINDOW.1) + f64::from(WINDOW.3) / 2.0) * scale) as u32 - 8,
    );
    let flashes = flash_onsets(
        &info,
        &folder.join("recording.mp4"),
        centre,
        f64::from(video.fps),
    );
    let tones = tone_onsets(&info, &folder.join("system-audio.flac"));
    let mic_tones = tone_onsets(&info, &folder.join("microphone.flac"));
    say(format!(
        "markers found: {} flashes in the video, {} tones in the system audio, {} tones heard by the microphone",
        flashes.len(),
        tones.len(),
        mic_tones.len()
    ));
    let pairs = offsets(&flashes, &tones);
    let mut drift_ok = None;
    if pairs.len() >= 4 {
        let n = pairs.len().min(10);
        let (first, last) = (
            mean(&pairs[..n]) * 1000.0,
            mean(&pairs[pairs.len() - n..]) * 1000.0,
        );
        let spread = (pairs.iter().cloned().fold(f64::MIN, f64::max)
            - pairs.iter().cloned().fold(f64::MAX, f64::min))
            * 1000.0;
        let points: Vec<(f64, f64)> = pairs
            .iter()
            .enumerate()
            .map(|(i, d)| (i as f64 * marker_every.as_secs_f64() / 3600.0, d * 1000.0))
            .collect();
        let change = last - first;
        say(format!(
            "video minus audio, mean of the first {n} markers: {first:.1} ms; of the last {n}: {last:.1} ms"
        ));
        say(format!(
            "change between them: {change:+.1} ms (limit 100 ms); slope {:+.1} ms per hour; spread of all {} pairs: {spread:.1} ms",
            slope(&points),
            pairs.len()
        ));
        drift_ok = Some(change.abs() < 100.0);
        let all: Vec<String> = pairs.iter().map(|d| format!("{:.0}", d * 1000.0)).collect();
        say(format!("every offset (ms): {}", all.join(" ")));
        if mic_tones.len() >= 4 {
            let mic_pairs = offsets(&tones, &mic_tones);
            if mic_pairs.len() >= 4 {
                let n = mic_pairs.len().min(10);
                say(format!(
                    "system audio minus what the microphone heard, first {n}: {:.1} ms, last {n}: {:.1} ms (acoustic path, for information)",
                    mean(&mic_pairs[..n]) * 1000.0,
                    mean(&mic_pairs[mic_pairs.len() - n..]) * 1000.0
                ));
            }
        }
    } else {
        say(format!(
            "too few marker pairs ({}) to judge drift",
            pairs.len()
        ));
    }

    // Memory.
    let samples = read_memory_csv(&run.join("memory.csv"));
    let mut memory_ok = None;
    if let Some(total) = minutes_recorded.or_else(|| samples.last().map(|s| s.minute)) {
        let window = |from: f64, to: f64| -> Vec<f64> {
            samples
                .iter()
                .filter(|s| s.minute >= from && s.minute <= to)
                .map(|s| s.own_mb + s.children_mb)
                .collect()
        };
        let peak = samples
            .iter()
            .map(|s| s.own_mb + s.children_mb)
            .fold(0.0, f64::max);
        let cpu = mean(
            &samples
                .iter()
                .skip(2)
                .map(|s| s.cpu_percent)
                .collect::<Vec<_>>(),
        );
        say(format!(
            "memory (this program plus FFmpeg): peak {peak:.0} MB; average CPU {cpu:.0} % of one core; {} samples",
            samples.len()
        ));
        if total >= 30.0 {
            let (early, late) = (mean(&window(10.0, 15.0)), mean(&window(total - 5.0, total)));
            let growth = (late - early) / early * 100.0;
            say(format!(
                "memory, minutes 10 to 15: {early:.0} MB; last five minutes: {late:.0} MB; growth {growth:+.1} % (limit 5 %)"
            ));
            memory_ok = Some(growth < 5.0);
        } else {
            say("memory growth is judged only for runs of 30 minutes or more".to_string());
        }
    }
    std::fs::write(run.join("report.txt"), &report).unwrap();
    Verdict {
        drift_ok,
        memory_ok,
        lengths_ok,
    }
}

// --- the run ---------------------------------------------------------------------------------------------------

#[test]
#[ignore = "takes two hours, needs a real monitor, microphone and speakers, and an idle machine"]
fn two_hour_recording_keeps_audio_and_video_together_and_memory_flat() {
    let config = Config::from_env();
    if let Some(run) = &config.analyze_only {
        analyze(run, config.marker_every, None);
        return;
    }

    make_process_dpi_aware();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let run = config.base.join(format!("run-{stamp}"));
    std::fs::create_dir_all(run.join("meetings")).unwrap();
    let database = Arc::new(Database::open(&run.join("soak.db")).unwrap());
    let recorder = Recorder::for_this_computer(database, run.join("meetings"));
    println!("run folder: {}", run.display());

    let stop_helpers = Arc::new(AtomicBool::new(false));
    let markers = marker_thread(
        stop_helpers.clone(),
        Duration::from_secs(20),
        config.marker_every,
    );
    let memory = memory_thread(stop_helpers.clone(), Duration::from_secs(60));

    let setup = RecordingSetup {
        title: "soak".into(),
        ..RecordingSetup::default()
    };
    recorder.start(&setup).expect("the recording starts");
    let began = Instant::now();
    let planned = Duration::from_secs(config.minutes * 60);
    let mut next_report = Duration::from_secs(300);
    while began.elapsed() < planned {
        std::thread::sleep(Duration::from_secs(1));
        let status = recorder.status();
        assert_eq!(
            status.phase,
            Phase::Recording,
            "the recording stopped by itself: {:?}",
            status.warnings
        );
        if began.elapsed() >= next_report {
            println!(
                "{:>5.1} min: media {:.1} s for {:.1} s of wall time, {} warnings",
                began.elapsed().as_secs_f64() / 60.0,
                status.elapsed_ms as f64 / 1000.0,
                began.elapsed().as_secs_f64(),
                status.warnings.len()
            );
            next_report += Duration::from_secs(300);
        }
    }

    let stopping = Instant::now();
    let finished = recorder.stop().expect("the recording is saved");
    println!(
        "saved after {:.1} s of finishing: {}",
        stopping.elapsed().as_secs_f64(),
        finished.file
    );
    stop_helpers.store(true, Ordering::SeqCst);
    let marker_times = markers.join().unwrap();
    let samples = memory.join().unwrap();
    write_memory_csv(&run.join("memory.csv"), &samples);
    println!(
        "{} markers made, {} memory samples",
        marker_times.len(),
        samples.len()
    );

    let verdict = analyze(
        &run,
        config.marker_every,
        Some(planned.as_secs_f64() / 60.0),
    );
    assert!(
        verdict.lengths_ok,
        "a track is not as long as the media timeline"
    );
    if let Some(ok) = verdict.drift_ok {
        assert!(ok, "audio and video drifted by 100 ms or more");
    }
    if let Some(ok) = verdict.memory_ok {
        assert!(ok, "memory grew by 5 % or more");
    }
}
