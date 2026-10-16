//! Spike: open the default microphone and the default speakers' loopback with cpal, run for N seconds, and report
//! how the data and timestamps behave. Audio content is never saved; only counts, timings and signal level.
//! Usage: audio <seconds> [tone]
//! A silent output stream is played so the loopback has packets to deliver (loopback delivers nothing when no
//! program is rendering audio). With "tone", the last 2 seconds carry a very quiet 440 Hz tone (-40 dBFS) so the
//! loopback signal level can be checked.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Error, ErrorKind, InputCallbackInfo, OutputCallbackInfo, SampleFormat};

#[derive(Default)]
struct Stats {
    callbacks: u64,
    frames: u64,
    xruns: u64,
    first_arrival: Option<Instant>,
    last_arrival: Option<Instant>,
    first_capture_ns: Option<u128>,
    last_capture_ns: Option<u128>,
    max_gap_ms: f64,
    sum_sq: f64,
    peak: f32,
    samples: u64,
    errors: Vec<String>,
    /// (arrival seconds since start, capture timestamp ns, audio frames delivered before this callback)
    points: Vec<(Instant, u128, u64)>,
}

fn open_input(
    device: &cpal::Device,
    label: &'static str,
    stats: Arc<Mutex<Stats>>,
) -> anyhow::Result<cpal::Stream> {
    let config = if device.supports_input() {
        device.default_input_config()?
    } else {
        device.default_output_config()? // an input stream on an output device is WASAPI loopback
    };
    println!(
        "{label}: {} | {} Hz, {} ch, {:?}",
        device.description()?,
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    );
    anyhow::ensure!(
        config.sample_format() == SampleFormat::F32,
        "{label}: this spike only handles f32 (got {:?})",
        config.sample_format()
    );
    let channels = config.channels() as u64;
    let stats_cb = stats.clone();
    let stats_err = stats.clone();
    let stream = device.build_input_stream(
        config.into(),
        move |data: &[f32], info: &InputCallbackInfo| {
            let now = Instant::now();
            let mut s = stats_cb.lock().unwrap();
            s.callbacks += 1;
            let frames_before = s.frames;
            s.frames += data.len() as u64 / channels;
            if let Some(prev) = s.last_arrival {
                let gap = now.duration_since(prev).as_secs_f64() * 1000.0;
                if gap > s.max_gap_ms {
                    s.max_gap_ms = gap;
                }
            }
            s.first_arrival.get_or_insert(now);
            s.last_arrival = Some(now);
            let capture_ns = info.timestamp().capture.as_nanos();
            s.first_capture_ns.get_or_insert(capture_ns);
            s.last_capture_ns = Some(capture_ns);
            s.points.push((now, capture_ns, frames_before));
            for &x in data {
                s.sum_sq += (x as f64) * (x as f64);
                s.peak = s.peak.max(x.abs());
            }
            s.samples += data.len() as u64;
        },
        move |err: Error| {
            let mut s = stats_err.lock().unwrap();
            if err.kind() == ErrorKind::Xrun {
                s.xruns += 1;
            } else {
                s.errors.push(format!("{} ({:?})", err, err.kind()));
            }
        },
        None,
    )?;
    Ok(stream)
}

/// Least-squares slope of y against x.
fn slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let num: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let den: f64 = x.iter().map(|a| (a - mx) * (a - mx)).sum();
    num / den
}

fn report(label: &str, s: &Stats, rate: u32, started: Instant, play_at: Instant) {
    println!("--- {label}");
    println!(
        "callbacks {}  frames {}  xruns {}  errors {:?}",
        s.callbacks, s.frames, s.xruns, s.errors
    );
    let (Some(first), Some(last)) = (s.first_arrival, s.last_arrival) else {
        println!("NO DATA RECEIVED");
        return;
    };
    let _ = started;
    println!(
        "first data arrived {:.0} ms after play()",
        first.duration_since(play_at).as_secs_f64() * 1000.0
    );
    if s.points.len() > 10 {
        let (a0, c0, _) = s.points[0];
        let x: Vec<f64> = s.points.iter().map(|p| p.2 as f64 / rate as f64).collect();
        let y_wall: Vec<f64> = s
            .points
            .iter()
            .map(|p| p.0.duration_since(a0).as_secs_f64())
            .collect();
        let y_cap: Vec<f64> = s.points.iter().map(|p| (p.1 - c0) as f64 / 1e9).collect();
        // wall seconds per audio second; below 1.0 means the device delivers audio faster than the clock.
        let (rw, rc) = (slope(&x, &y_wall), slope(&x, &y_cap));
        println!(
            "REGRESSION over {} callbacks: device clock vs wall clock {:+.1} ppm | device clock vs its own capture timestamps {:+.1} ppm",
            s.points.len(),
            (1.0 / rw - 1.0) * 1e6,
            (1.0 / rc - 1.0) * 1e6
        );
    }
    let wall = last.duration_since(first).as_secs_f64();
    let audio = s.frames as f64 / rate as f64;
    println!(
        "wall clock between first and last callback: {wall:.3}s | audio received: {audio:.3}s | ratio audio/wall = {:.6} ({:+.1} ppm)",
        audio / wall,
        (audio / wall - 1.0) * 1e6
    );
    if let (Some(a), Some(b)) = (s.first_capture_ns, s.last_capture_ns) {
        let ts = (b - a) as f64 / 1e9;
        println!(
            "capture timestamps span {ts:.3}s | vs wall {:+.1} ppm | vs audio frames {:+.1} ppm",
            (ts / wall - 1.0) * 1e6,
            (ts / audio - 1.0) * 1e6
        );
    }
    println!("largest gap between callbacks: {:.1} ms", s.max_gap_ms);
    let rms = (s.sum_sq / s.samples.max(1) as f64).sqrt();
    println!(
        "signal level: rms {:.5} ({:.1} dBFS), peak {:.5}",
        rms,
        20.0 * rms.max(1e-9).log10(),
        s.peak
    );
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().unwrap_or_else(|| "10".into()).parse()?;
    let flags: Vec<String> = args.collect();
    let with_tone = flags.iter().any(|f| f == "tone");
    let no_render = flags.iter().any(|f| f == "norender");

    let host = cpal::default_host();
    println!("host: {}", host.id());
    println!("devices:");
    for d in host.devices()? {
        println!(
            "  {} | input={} output={} | {}",
            d.description()?,
            d.supports_input(),
            d.supports_output(),
            d.id()?
        );
    }
    let mic = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default input device"))?;
    let speakers = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default output device"))?;

    let mic_stats = Arc::new(Mutex::new(Stats::default()));
    let loop_stats = Arc::new(Mutex::new(Stats::default()));
    let started = Instant::now();
    let mic_stream = open_input(&mic, "microphone", mic_stats.clone())?;
    let loop_stream = open_input(&speakers, "loopback  ", loop_stats.clone())?;

    // Keep the render engine busy so loopback receives packets.
    let out_config = speakers.default_output_config()?;
    let out_rate = out_config.sample_rate();
    let out_channels = out_config.channels() as usize;
    anyhow::ensure!(
        out_config.sample_format() == SampleFormat::F32,
        "output not f32"
    );
    let tone_from = Duration::from_secs(seconds.saturating_sub(2));
    let out_started = Instant::now();
    let mut phase = 0.0f32;
    let render = if no_render {
        None
    } else {
        Some(speakers.build_output_stream(
            out_config.into(),
            move |data: &mut [f32], _info: &OutputCallbackInfo| {
                let tone_on = with_tone && out_started.elapsed() >= tone_from;
                for frame in data.chunks_mut(out_channels) {
                    let v = if tone_on {
                        phase += 2.0 * std::f32::consts::PI * 440.0 / out_rate as f32;
                        if phase > 2.0 * std::f32::consts::PI {
                            phase -= 2.0 * std::f32::consts::PI;
                        }
                        0.01 * phase.sin()
                    } else {
                        0.0
                    };
                    frame.fill(v);
                }
            },
            |err: Error| eprintln!("render error: {err}"),
            None,
        )?)
    };

    if let Some(r) = &render {
        r.play()?;
    }
    let play_at = Instant::now();
    mic_stream.play()?;
    loop_stream.play()?;
    println!("running for {seconds}s ...");
    std::thread::sleep(Duration::from_secs(seconds));
    drop(render);
    drop(loop_stream);
    drop(mic_stream);

    let mic_rate = mic.default_input_config()?.sample_rate();
    let loop_rate = speakers.default_output_config()?.sample_rate();
    report(
        "microphone",
        &mic_stats.lock().unwrap(),
        mic_rate,
        started,
        play_at,
    );
    report(
        "loopback",
        &loop_stats.lock().unwrap(),
        loop_rate,
        started,
        play_at,
    );
    Ok(())
}
