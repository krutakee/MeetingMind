//! Spike: capture one monitor with Windows.Graphics.Capture for N seconds and report what arrives.
//! Usage: wgc <monitor index, 1-based> <seconds> [border]
//! Reports frame rate, frame size, intervals between frames, how the frame timestamps relate to the wall clock,
//! and what it costs to copy a frame to CPU memory. Frame contents are not saved.

use std::time::{Duration, Instant};

use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

struct Probe {
    started: Instant,
    run_for: Duration,
    arrivals: Vec<Instant>,
    frame_times_100ns: Vec<i64>,
    copy_ms: Vec<f64>,
    size: (u32, u32),
}

impl GraphicsCaptureApiHandler for Probe {
    type Flags = f64;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            started: Instant::now(),
            run_for: Duration::from_secs_f64(ctx.flags),
            arrivals: Vec::new(),
            frame_times_100ns: Vec::new(),
            copy_ms: Vec::new(),
            size: (0, 0),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let arrived = Instant::now();
        if self.arrivals.is_empty() {
            println!(
                "first frame arrived {:.0} ms after capture started",
                self.started.elapsed().as_secs_f64() * 1000.0
            );
        }
        self.size = (frame.width(), frame.height());
        self.frame_times_100ns.push(frame.timestamp()?.Duration);
        // Cost of getting the pixels into CPU memory (what the recorder must do for every frame it keeps).
        let copy_start = Instant::now();
        let mut buffer = frame.buffer()?;
        let bytes = buffer.as_raw_buffer().len();
        std::hint::black_box(bytes);
        self.copy_ms
            .push(copy_start.elapsed().as_secs_f64() * 1000.0);
        self.arrivals.push(arrived);

        if self.started.elapsed() >= self.run_for {
            // on_closed is not called when we stop the capture ourselves, so report here.
            self.report();
            control.stop();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.report();
        Ok(())
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

impl Probe {
    fn report(&self) {
        let n = self.arrivals.len();
        let secs = self.started.elapsed().as_secs_f64();
        println!("frame size: {}x{}", self.size.0, self.size.1);
        println!(
            "frames: {n} in {secs:.1}s = {:.1} fps average",
            n as f64 / secs
        );
        if n < 2 {
            println!("(too few frames to measure intervals; the screen was static)");
            return;
        }
        let mut intervals: Vec<f64> = self
            .frame_times_100ns
            .windows(2)
            .map(|w| (w[1] - w[0]) as f64 / 10_000.0)
            .collect();
        intervals.sort_by(|a, b| a.total_cmp(b));
        println!(
            "interval between frames (ms, from frame timestamps): min {:.1}  median {:.1}  p95 {:.1}  max {:.1}",
            intervals[0],
            percentile(&intervals, 0.5),
            percentile(&intervals, 0.95),
            intervals[intervals.len() - 1]
        );
        // If frame timestamps and the wall clock advance at the same rate, (arrival - timestamp) only jitters.
        let t0 = self.frame_times_100ns[0];
        let a0 = self.arrivals[0];
        let lat: Vec<f64> = (0..n)
            .map(|i| {
                let ts_s = (self.frame_times_100ns[i] - t0) as f64 / 1e7;
                let arr_s = self.arrivals[i].duration_since(a0).as_secs_f64();
                (arr_s - ts_s) * 1000.0
            })
            .collect();
        let (lo, hi) = lat
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        println!(
            "arrival minus frame-timestamp, relative to first frame (ms): min {lo:.1}  max {hi:.1}"
        );
        let mut copies = self.copy_ms.clone();
        copies.sort_by(|a, b| a.total_cmp(b));
        println!(
            "copy of one frame to CPU memory (ms): median {:.2}  p95 {:.2}  max {:.2}",
            percentile(&copies, 0.5),
            percentile(&copies, 0.95),
            copies[copies.len() - 1]
        );
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let index: usize = args.next().unwrap_or_else(|| "1".into()).parse()?;
    let seconds: f64 = args.next().unwrap_or_else(|| "10".into()).parse()?;
    let border = match args.next().as_deref() {
        Some("border") => DrawBorderSettings::WithBorder,
        Some("noborder") => DrawBorderSettings::WithoutBorder,
        _ => DrawBorderSettings::Default,
    };

    let monitors = Monitor::enumerate()?;
    println!("monitors found by windows-capture: {}", monitors.len());
    for m in &monitors {
        println!(
            "  #{} {} {}x{} @ {} Hz",
            m.index()?,
            m.device_name()?,
            m.width()?,
            m.height()?,
            m.refresh_rate()?
        );
    }
    let monitor = Monitor::from_index(index)?;
    println!("capturing monitor #{index} for {seconds}s");

    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        border,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        seconds,
    );
    Probe::start(settings).map_err(|e| anyhow::anyhow!("capture failed: {e}"))?;
    Ok(())
}
