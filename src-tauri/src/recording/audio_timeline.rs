//! Places one audio stream on the media timeline.
//!
//! Each chunk arrives with the media time of its first sample. The timeline compares that with how much audio has
//! already been written and keeps the two together:
//! - **Gap** (chunk starts later than expected): silence fills the difference, so tracks stay aligned. Gaps of
//!   `min_logged_gap` or more are reported (they end up in `meeting.json`); shorter ones are normal device start-up
//!   latency and are filled silently.
//! - **Overlap** (chunk starts earlier than expected): the repeated start of the chunk is dropped.
//! - **Drift**: a device clock that differs from the system clock delivers slightly too many or too few samples. The
//!   device rate is measured against media time over a sliding window; only when it is off by more than a few ppm is
//!   a tiny rate correction applied (linear interpolation). A device that keeps good time is never touched, so its
//!   audio stays bit-exact. Measured on this machine's built-in devices: about 0.2 ppm.

use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TimelineConfig {
    pub sample_rate: u32,
    pub channels: u16,
    /// Timing differences smaller than this are treated as jitter and ignored.
    pub gap_tolerance: Duration,
    /// Gaps at least this long are reported as events.
    pub min_logged_gap: Duration,
    /// Correction starts when the measured drift exceeds this (parts per million) ...
    pub drift_engage_ppm: f64,
    /// ... and stops when it falls below this.
    pub drift_release_ppm: f64,
    /// The drift is not trusted until this much media time has been measured.
    pub drift_min_window: Duration,
    /// Older measurements than this are forgotten.
    pub drift_max_window: Duration,
}

impl TimelineConfig {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            gap_tolerance: Duration::from_millis(20),
            min_logged_gap: Duration::from_millis(250),
            drift_engage_ppm: 3.0,
            drift_release_ppm: 1.5,
            drift_min_window: Duration::from_secs(30),
            drift_max_window: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimelineEvent {
    /// Audio was missing; silence of `length` was inserted at media time `at`.
    Gap { at: Duration, length: Duration },
    /// The device repeated audio; `length` of it was dropped at media time `at`.
    Overlap { at: Duration, length: Duration },
}

pub struct AudioTimeline {
    config: TimelineConfig,
    channels: usize,
    /// Frames (one sample per channel) written so far.
    written: u64,
    /// Frames received from the device so far, before any correction.
    received: u64,
    /// (media seconds of the chunk start, frames received before it)
    samples: VecDeque<(f64, u64)>,
    since_estimate: u32,
    drift_ppm: Option<f64>,
    resampler: Option<LinearResampler>,
    events: Vec<TimelineEvent>,
    silent_frames_filled: u64,
}

impl AudioTimeline {
    pub fn new(config: TimelineConfig) -> Self {
        let channels = config.channels as usize;
        assert!(channels > 0 && config.sample_rate > 0);
        Self {
            config,
            channels,
            written: 0,
            received: 0,
            samples: VecDeque::new(),
            since_estimate: 0,
            drift_ppm: None,
            resampler: None,
            events: Vec::new(),
            silent_frames_filled: 0,
        }
    }

    fn rate(&self) -> f64 {
        self.config.sample_rate as f64
    }

    fn frames_for(&self, d: Duration) -> u64 {
        (d.as_secs_f64() * self.rate()).round() as u64
    }

    fn duration_of(&self, frames: u64) -> Duration {
        Duration::from_secs_f64(frames as f64 / self.rate())
    }

    /// Takes one interleaved chunk whose first frame is at `media_start` and returns the interleaved samples to
    /// write: possibly leading silence, possibly with the repeated start dropped, possibly rate-corrected.
    pub fn place(&mut self, media_start: Duration, chunk: &[f32]) -> Vec<f32> {
        let frames_in = (chunk.len() / self.channels) as u64;
        let chunk = &chunk[..(frames_in as usize) * self.channels];
        let expected = self.frames_for(media_start);
        let tolerance = self.frames_for(self.config.gap_tolerance);

        self.observe(media_start, frames_in);
        self.received += frames_in;

        let mut out = Vec::new();
        let mut body = chunk;

        if expected > self.written + tolerance {
            let missing = expected - self.written;
            out.resize((missing as usize) * self.channels, 0.0);
            self.silent_frames_filled += missing;
            if self.duration_of(missing) >= self.config.min_logged_gap {
                self.events.push(TimelineEvent::Gap {
                    at: self.duration_of(self.written),
                    length: self.duration_of(missing),
                });
            }
            self.written += missing;
            // Any resampler state refers to audio before the gap.
            self.resampler = None;
        } else if self.written > expected + tolerance {
            let repeated = (self.written - expected).min(frames_in);
            body = &chunk[(repeated as usize) * self.channels..];
            self.events.push(TimelineEvent::Overlap {
                at: self.duration_of(expected),
                length: self.duration_of(repeated),
            });
        }

        match &mut self.resampler {
            Some(resampler) => {
                let corrected = resampler.process(body);
                self.written += (corrected.len() / self.channels) as u64;
                out.extend_from_slice(&corrected);
            }
            None => {
                self.written += (body.len() / self.channels) as u64;
                out.extend_from_slice(body);
            }
        }
        out
    }

    /// Feeds the drift estimator and starts or stops the correction.
    fn observe(&mut self, media_start: Duration, frames_in: u64) {
        let _ = frames_in;
        let t = media_start.as_secs_f64();
        // One measurement per second is plenty for a rate estimate and keeps the window small.
        if let Some(&(latest, _)) = self.samples.back()
            && t - latest < 1.0
        {
            return;
        }
        self.samples.push_back((t, self.received));
        let max = self.config.drift_max_window.as_secs_f64();
        while let Some(&(oldest, _)) = self.samples.front() {
            if t - oldest > max {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        self.since_estimate += 1;
        if self.since_estimate < 5 {
            return;
        }
        self.since_estimate = 0;

        let (first_t, _) = *self.samples.front().expect("just pushed");
        if t - first_t < self.config.drift_min_window.as_secs_f64() {
            return;
        }
        let Some(slope) = least_squares_slope(&self.samples) else {
            return;
        };
        // Frames the device delivers per second of media time, relative to its nominal rate.
        let ppm = (slope / self.rate() - 1.0) * 1e6;
        self.drift_ppm = Some(ppm);

        let engaged = self.resampler.is_some();
        if !engaged && ppm.abs() > self.config.drift_engage_ppm {
            self.resampler = Some(LinearResampler::new(self.channels, 1.0 + ppm / 1e6));
        } else if engaged && ppm.abs() < self.config.drift_release_ppm {
            self.resampler = None;
        } else if let Some(resampler) = &mut self.resampler {
            resampler.set_step(1.0 + ppm / 1e6);
        }
    }

    /// Silence that brings the stream up to `media_time`, or nothing if it is already there or beyond. Used at the end
    /// of a recording so that every track has the same length, and when a source delivers nothing for a while.
    pub fn pad_to(&mut self, media_time: Duration) -> Vec<f32> {
        let target = self.frames_for(media_time);
        if target <= self.written {
            return Vec::new();
        }
        let missing = target - self.written;
        self.silent_frames_filled += missing;
        if self.duration_of(missing) >= self.config.min_logged_gap {
            self.events.push(TimelineEvent::Gap {
                at: self.duration_of(self.written),
                length: self.duration_of(missing),
            });
        }
        self.written = target;
        self.resampler = None;
        vec![0.0; (missing as usize) * self.channels]
    }

    pub fn written_frames(&self) -> u64 {
        self.written
    }

    pub fn written_duration(&self) -> Duration {
        self.duration_of(self.written)
    }

    /// The latest drift estimate in parts per million (positive: the device clock runs fast), once measured.
    pub fn drift_ppm(&self) -> Option<f64> {
        self.drift_ppm
    }

    pub fn is_correcting_drift(&self) -> bool {
        self.resampler.is_some()
    }

    /// Total silence inserted, including short start-up gaps that are not reported as events.
    pub fn silent_frames_filled(&self) -> u64 {
        self.silent_frames_filled
    }

    pub fn take_events(&mut self) -> Vec<TimelineEvent> {
        std::mem::take(&mut self.events)
    }
}

fn least_squares_slope(points: &VecDeque<(f64, u64)>) -> Option<f64> {
    let n = points.len() as f64;
    if points.len() < 2 {
        return None;
    }
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1 as f64).sum::<f64>() / n;
    let num: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 as f64 - my)).sum();
    let den: f64 = points.iter().map(|p| (p.0 - mx) * (p.0 - mx)).sum();
    (den > 0.0).then(|| num / den)
}

/// Stateful linear-interpolation resampler for ratios within a few hundred ppm of 1.
/// `step` is how many input frames one output frame advances (1.0 = unchanged).
struct LinearResampler {
    channels: usize,
    step: f64,
    /// Position of the next output frame, in input frames relative to the start of the next chunk.
    pos: f64,
    /// The last input frame of the previous chunk (index -1 of the next chunk).
    last: Option<Vec<f32>>,
}

impl LinearResampler {
    fn new(channels: usize, step: f64) -> Self {
        Self {
            channels,
            step,
            pos: 0.0,
            last: None,
        }
    }

    fn set_step(&mut self, step: f64) {
        self.step = step;
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        let frames = input.len() / ch;
        if frames == 0 {
            return Vec::new();
        }
        let sample = |last: &Option<Vec<f32>>, i: i64, c: usize| -> f32 {
            if i < 0 {
                match last {
                    Some(frame) => frame[c],
                    None => input[c],
                }
            } else {
                input[(i as usize) * ch + c]
            }
        };

        let mut out = Vec::with_capacity(input.len() + ch);
        // The last input frame stays unread until the next chunk supplies its neighbour.
        let limit = (frames - 1) as f64;
        while self.pos < limit {
            let i0 = self.pos.floor();
            let frac = (self.pos - i0) as f32;
            for c in 0..ch {
                let a = sample(&self.last, i0 as i64, c);
                let b = sample(&self.last, i0 as i64 + 1, c);
                out.push(a + (b - a) * frac);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        self.last = Some(input[(frames - 1) * ch..].to_vec());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const CHUNK: usize = 480; // 10 ms

    fn timeline(channels: u16) -> AudioTimeline {
        AudioTimeline::new(TimelineConfig::new(RATE, channels))
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    /// A 440 Hz sine, frame `i` of the stream, as one channel value.
    fn sine(frame: u64) -> f32 {
        (2.0 * std::f64::consts::PI * 440.0 * frame as f64 / RATE as f64).sin() as f32 * 0.5
    }

    fn chunk_at(first_frame: u64, frames: usize) -> Vec<f32> {
        (0..frames as u64).map(|i| sine(first_frame + i)).collect()
    }

    #[test]
    fn a_device_with_good_timing_is_written_bit_exact() {
        let mut t = timeline(1);
        let mut written = Vec::new();

        for i in 0..1000u64 {
            let start = secs((i * CHUNK as u64) as f64 / RATE as f64);
            written.extend(t.place(start, &chunk_at(i * CHUNK as u64, CHUNK)));
        }

        let expected = chunk_at(0, 1000 * CHUNK);
        assert_eq!(written, expected, "audio must be untouched");
        assert!(t.take_events().is_empty());
        assert_eq!(t.silent_frames_filled(), 0);
        assert!(!t.is_correcting_drift());
    }

    #[test]
    fn small_timing_jitter_changes_nothing() {
        let mut t = timeline(1);
        let mut written = Vec::new();

        for i in 0..400u64 {
            let jitter = if i % 2 == 0 { 0.006 } else { -0.006 }; // +-6 ms, under the 20 ms tolerance
            let start = secs(((i * CHUNK as u64) as f64 / RATE as f64 + jitter).max(0.0));
            written.extend(t.place(start, &chunk_at(i * CHUNK as u64, CHUNK)));
        }

        assert_eq!(written, chunk_at(0, 400 * CHUNK));
        assert!(t.take_events().is_empty());
    }

    #[test]
    fn a_missing_second_is_filled_with_silence_and_reported() {
        let mut t = timeline(1);
        let mut written = Vec::new();
        for i in 0..100u64 {
            let start = secs((i * CHUNK as u64) as f64 / RATE as f64);
            written.extend(t.place(start, &chunk_at(i * CHUNK as u64, CHUNK)));
        }
        // The device stops for 1 s (100 chunks), then continues with the true timestamps.
        let resume = 200u64;
        let start = secs((resume * CHUNK as u64) as f64 / RATE as f64);
        written.extend(t.place(start, &chunk_at(resume * CHUNK as u64, CHUNK)));

        assert_eq!(written.len(), 101 * CHUNK + 100 * CHUNK);
        let silence = &written[100 * CHUNK..200 * CHUNK];
        assert!(silence.iter().all(|&s| s == 0.0));
        let events = t.take_events();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TimelineEvent::Gap { at, length } => {
                assert!(
                    (at.as_secs_f64() - 1.0).abs() < 1e-6,
                    "gap starts at 1 s, got {at:?}"
                );
                assert!(
                    (length.as_secs_f64() - 1.0).abs() < 1e-6,
                    "gap is 1 s, got {length:?}"
                );
            }
            other => panic!("expected a gap, got {other:?}"),
        }
    }

    #[test]
    fn a_short_start_up_delay_is_filled_silently() {
        let mut t = timeline(1);

        // The first data arrives 60 ms after the recording started (device start-up latency).
        let out = t.place(secs(0.060), &chunk_at(0, CHUNK));

        assert_eq!(
            out.len(),
            2880 + CHUNK,
            "60 ms of leading silence keeps this track aligned with the others"
        );
        assert!(out[..2880].iter().all(|&s| s == 0.0));
        assert!(
            t.take_events().is_empty(),
            "60 ms is below the reporting threshold"
        );
        assert_eq!(t.silent_frames_filled(), 2880);
    }

    #[test]
    fn a_repeated_start_is_dropped_and_reported() {
        let mut t = timeline(1);
        let mut written = Vec::new();
        for i in 0..100u64 {
            let start = secs((i * CHUNK as u64) as f64 / RATE as f64);
            written.extend(t.place(start, &chunk_at(i * CHUNK as u64, CHUNK)));
        }
        // The next chunk claims to start 100 ms (4800 frames) too early, and is 20 chunks long.
        let early_start = secs(1.0 - 0.1);
        let out = t.place(early_start, &chunk_at(0, 20 * CHUNK));

        assert_eq!(out.len(), 20 * CHUNK - 4800);
        assert_eq!(
            out[0],
            sine(4800),
            "the first 4800 frames of the chunk were dropped"
        );
        let events = t.take_events();
        assert!(
            matches!(events.as_slice(), [TimelineEvent::Overlap { length, .. }] if (length.as_secs_f64() - 0.1).abs() < 1e-6)
        );
    }

    #[test]
    fn padding_brings_a_short_stream_up_to_the_media_length() {
        let mut t = timeline(2);
        t.place(secs(0.0), &vec![0.5; 2 * CHUNK]);

        let pad = t.pad_to(secs(2.0));

        assert_eq!(pad.len(), 2 * (2 * RATE as usize - CHUNK));
        assert!(pad.iter().all(|&s| s == 0.0));
        assert_eq!(t.written_frames(), 2 * RATE as u64);
        assert!(
            matches!(t.take_events().as_slice(), [TimelineEvent::Gap { .. }]),
            "1.99 s of silence is reported"
        );
    }

    #[test]
    fn padding_does_nothing_when_the_stream_is_already_long_enough() {
        let mut t = timeline(1);
        t.place(secs(0.0), &chunk_at(0, 2 * RATE as usize));

        assert!(t.pad_to(secs(1.0)).is_empty());
        assert!(t.pad_to(secs(2.0)).is_empty());
        assert_eq!(t.written_frames(), 2 * RATE as u64);
    }

    #[test]
    fn a_tiny_final_padding_is_applied_but_not_reported() {
        let mut t = timeline(1);
        t.place(secs(0.0), &chunk_at(0, 100 * CHUNK)); // 1.000 s

        let pad = t.pad_to(secs(1.004));

        assert_eq!(pad.len(), 192, "4 ms at 48 kHz");
        assert!(t.take_events().is_empty());
    }

    #[test]
    fn stereo_silence_covers_every_channel() {
        let mut t = timeline(2);

        let out = t.place(secs(1.0), &vec![0.25; 2 * CHUNK]);

        assert_eq!(out.len(), 2 * (RATE as usize + CHUNK));
        assert!(out[..2 * RATE as usize].iter().all(|&s| s == 0.0));
        assert!(out[2 * RATE as usize..].iter().all(|&s| s == 0.25));
    }

    /// Simulates a device whose clock runs `ppm` fast (positive) or slow (negative) against the system clock and
    /// feeds `seconds` of it through the timeline. Returns the timeline and everything written.
    fn run_drifting_device(ppm: f64, seconds: u64) -> (AudioTimeline, Vec<f32>) {
        let mut t = timeline(1);
        let device_rate = RATE as f64 * (1.0 + ppm / 1e6);
        let chunks = (seconds as f64 * device_rate / CHUNK as f64) as u64;
        let mut written = Vec::new();
        for i in 0..chunks {
            // True (media) time at which this chunk's first sample was captured.
            let start = secs((i * CHUNK as u64) as f64 / device_rate);
            // The device's own sample stream: a 440 Hz sine in *device* sample numbers, played at the device rate.
            // Its true frequency is therefore 440 * (1 + ppm/1e6) Hz, which is the real-world situation.
            written.extend(t.place(start, &chunk_at(i * CHUNK as u64, CHUNK)));
        }
        (t, written)
    }

    #[test]
    fn a_fast_device_clock_is_measured_and_the_length_stays_on_media_time() {
        let (mut t, written) = run_drifting_device(200.0, 600);

        let media_seconds = 600.0;
        let written_seconds = written.len() as f64 / RATE as f64;
        // Without correction a +200 ppm device would be 120 ms long over 10 minutes.
        assert!(
            (written_seconds - media_seconds).abs() < 0.025,
            "off by {:.1} ms",
            (written_seconds - media_seconds) * 1000.0
        );
        let ppm = t.drift_ppm().expect("drift was measured");
        assert!((ppm - 200.0).abs() < 10.0, "measured {ppm:.1} ppm");
        assert!(t.is_correcting_drift());
        assert!(
            t.take_events().is_empty(),
            "correction must not look like gaps"
        );
    }

    #[test]
    fn a_slow_device_clock_is_corrected_too() {
        let (mut t, written) = run_drifting_device(-150.0, 600);

        let written_seconds = written.len() as f64 / RATE as f64;
        assert!(
            (written_seconds - 600.0).abs() < 0.025,
            "off by {:.1} ms",
            (written_seconds - 600.0) * 1000.0
        );
        let ppm = t.drift_ppm().expect("drift was measured");
        assert!((ppm + 150.0).abs() < 10.0, "measured {ppm:.1} ppm");
        assert!(t.take_events().is_empty());
    }

    /// The plan's acceptance simulation: two hours with a device clock 20 ppm off (the size of error a typical USB
    /// device has). Uncorrected, 20 ppm is 144 ms over two hours, more than the 100 ms budget.
    fn check_two_hours_at(ppm: f64) {
        const BIG_CHUNK: usize = 960; // 20 ms; the logic does not depend on the chunk size
        let mut t = timeline(1);
        let device_rate = RATE as f64 * (1.0 + ppm / 1e6);
        let chunks = (7200.0 * device_rate / BIG_CHUNK as f64) as u64;
        let content = vec![0.1f32; BIG_CHUNK]; // the content does not matter for length
        let mut written_frames = 0u64;
        for i in 0..chunks {
            let start = secs((i * BIG_CHUNK as u64) as f64 / device_rate);
            written_frames += t.place(start, &content).len() as u64;
        }

        let media_seconds = chunks as f64 * BIG_CHUNK as f64 / device_rate;
        let written_seconds = written_frames as f64 / RATE as f64;
        let error_ms = (written_seconds - media_seconds) * 1000.0;
        assert!(
            error_ms.abs() < 20.0,
            "{ppm:+} ppm: off by {error_ms:.1} ms after 2 h"
        );
        assert!(
            t.take_events().is_empty(),
            "{ppm:+} ppm: correction must not look like gaps"
        );
    }

    #[test]
    fn two_hours_with_a_fast_device_clock_stay_within_20_ms() {
        check_two_hours_at(20.0);
    }

    #[test]
    fn two_hours_with_a_slow_device_clock_stay_within_20_ms() {
        check_two_hours_at(-20.0);
    }

    #[test]
    fn rate_correction_adds_no_clicks() {
        let (_t, written) = run_drifting_device(200.0, 120);

        // A 440 Hz sine at amplitude 0.5 changes by at most about 0.0288 between neighbouring samples. Allow a margin
        // for the small frequency shift, but a discontinuity (a dropped or repeated sample jump) would be far larger.
        let max_step = written
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.031, "largest sample-to-sample step {max_step}");
    }

    #[test]
    fn drift_below_the_threshold_leaves_the_audio_alone() {
        // 1 ppm is under the 3 ppm engage threshold: nothing may be resampled.
        let (t, _written) = run_drifting_device(1.0, 300);

        assert!(!t.is_correcting_drift());
        assert!(t.drift_ppm().is_some());
    }
}
