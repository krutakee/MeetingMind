//! Turning audio from one device format into another (channels and sample rate).
//!
//! A recording's encoders are started for one format, the one the devices had when it began. If the user plugs in a
//! headset in the middle of a call, Windows moves the default microphone and speakers to it, and the headset may use
//! another sample rate or channel count. Refusing it would leave that track silent for the rest of the recording, so
//! the new device's audio is converted to the recording's format instead and the recording carries on.
//!
//! The conversion is plain: channels are mixed to mono and spread again, and the rate is changed by linear
//! interpolation. That is not studio quality, but speech survives it well, it only runs after a device change, and
//! it is better than a minute of silence. Pure arithmetic, the same on every platform.

use crate::audio::AudioFormat;
use crate::recording::audio_timeline::LinearResampler;

/// Converts interleaved f32 audio from `from` to `to`, chunk after chunk, keeping the state between chunks so the
/// joins are seamless.
pub struct FormatConverter {
    from_channels: usize,
    to_channels: usize,
    resampler: Option<LinearResampler>,
}

impl FormatConverter {
    pub fn new(from: AudioFormat, to: AudioFormat) -> Self {
        let resampler = (from.sample_rate != to.sample_rate).then(|| {
            // Input frames consumed per output frame.
            let step = f64::from(from.sample_rate) / f64::from(to.sample_rate);
            LinearResampler::new(usize::from(to.channels), step)
        });
        Self {
            from_channels: usize::from(from.channels.max(1)),
            to_channels: usize::from(to.channels.max(1)),
            resampler,
        }
    }

    /// The next chunk, in the target format. May be a frame shorter or longer than the exact ratio; what is left over
    /// is carried into the following chunk.
    pub fn convert(&mut self, input: &[f32]) -> Vec<f32> {
        let mapped = map_channels(input, self.from_channels, self.to_channels);
        match &mut self.resampler {
            Some(resampler) => resampler.process(&mapped),
            None => mapped,
        }
    }
}

/// Changes the number of channels. The same count is copied; a mono source is repeated into every channel; anything
/// else is mixed down to one channel and repeated into every output channel.
fn map_channels(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let mut out = Vec::with_capacity(input.len() / from * to);
    for frame in input.chunks_exact(from) {
        let mono = frame.iter().sum::<f32>() / from as f32;
        out.extend(std::iter::repeat_n(mono, to));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(sample_rate: u32, channels: u16) -> AudioFormat {
        AudioFormat {
            sample_rate,
            channels,
        }
    }

    /// `seconds` of a sine at `hz`, one channel, at `rate`.
    fn sine(hz: f64, rate: u32, seconds: f64) -> Vec<f32> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|i| {
                (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() as f32 * 0.5
            })
            .collect()
    }

    /// How many times the signal crosses zero upwards, as a way to read its frequency.
    fn rising_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count()
    }

    #[test]
    fn mono_becomes_stereo_by_repeating_the_samples() {
        let mut converter = FormatConverter::new(format(48_000, 1), format(48_000, 2));

        assert_eq!(
            converter.convert(&[0.1, 0.2, 0.3]),
            vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3]
        );
    }

    #[test]
    fn stereo_becomes_mono_by_averaging() {
        let mut converter = FormatConverter::new(format(48_000, 2), format(48_000, 1));

        let out = converter.convert(&[0.2, 0.4, -0.2, 0.2]);

        assert_eq!(out.len(), 2);
        assert!(
            (out[0] - 0.3).abs() < 1e-6 && out[1].abs() < 1e-6,
            "{out:?}"
        );
    }

    #[test]
    fn more_channels_are_mixed_down_and_spread() {
        let mut converter = FormatConverter::new(format(48_000, 4), format(48_000, 2));

        let out = converter.convert(&[0.4, 0.0, 0.2, 0.2]); // one frame

        assert_eq!(out.len(), 2);
        assert!(
            (out[0] - 0.2).abs() < 1e-6 && (out[1] - 0.2).abs() < 1e-6,
            "{out:?}"
        );
    }

    #[test]
    fn the_same_format_is_passed_through_unchanged() {
        let mut converter = FormatConverter::new(format(48_000, 2), format(48_000, 2));
        let input = vec![0.1, -0.1, 0.2, -0.2];

        assert_eq!(converter.convert(&input), input);
    }

    #[test]
    fn a_44100_hz_microphone_becomes_48000_hz_with_the_same_pitch_and_length() {
        let mut converter = FormatConverter::new(format(44_100, 1), format(48_000, 2));
        let input = sine(440.0, 44_100, 2.0);

        let out = converter.convert(&input);

        let frames = out.len() / 2;
        assert!(
            (frames as i64 - 96_000).abs() <= 2,
            "{frames} frames for 2 s"
        );
        let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        let hz = rising_crossings(&left) as f64 / 2.0;
        assert!((hz - 440.0).abs() < 2.0, "pitch {hz} Hz");
        let peak = left.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.03, "level {peak}");
    }

    #[test]
    fn a_16000_hz_headset_microphone_is_brought_up_to_48000_hz() {
        let mut converter = FormatConverter::new(format(16_000, 1), format(48_000, 1));

        let out = converter.convert(&sine(300.0, 16_000, 1.0));

        assert!(
            (out.len() as i64 - 48_000).abs() <= 3,
            "{} frames",
            out.len()
        );
        assert!((rising_crossings(&out) as f64 - 300.0).abs() <= 2.0);
    }

    #[test]
    fn the_chunk_size_does_not_change_the_result() {
        let input = sine(500.0, 44_100, 1.0);
        let whole = FormatConverter::new(format(44_100, 1), format(48_000, 1)).convert(&input);

        let mut converter = FormatConverter::new(format(44_100, 1), format(48_000, 1));
        let mut pieces = Vec::new();
        for chunk in input.chunks(441) {
            pieces.extend(converter.convert(chunk));
        }

        let common = whole.len().min(pieces.len());
        assert!(whole.len().abs_diff(pieces.len()) <= 2);
        let worst = whole[..common]
            .iter()
            .zip(&pieces[..common])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "chunks change the audio by {worst}");
    }

    #[test]
    fn chunk_joins_make_no_clicks() {
        let mut converter = FormatConverter::new(format(44_100, 1), format(48_000, 1));
        let mut out = Vec::new();
        for chunk in sine(200.0, 44_100, 2.0).chunks(410) {
            out.extend(converter.convert(chunk));
        }

        // A 200 Hz sine at amplitude 0.5 moves by at most about 0.013 per sample at 48 kHz.
        let largest = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(largest < 0.02, "largest step between samples {largest}");
    }

    #[test]
    fn an_empty_chunk_gives_nothing_and_does_not_upset_what_follows() {
        let mut converter = FormatConverter::new(format(44_100, 1), format(48_000, 1));

        assert!(converter.convert(&[]).is_empty());
        assert!(!converter.convert(&sine(440.0, 44_100, 0.1)).is_empty());
    }
}
