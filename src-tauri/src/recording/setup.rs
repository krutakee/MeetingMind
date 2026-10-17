//! What the user chooses on the Record screen, and what those choices mean for the recording.
//!
//! Pure data and arithmetic, the same on every platform. Presets saved in the database hold a `RecordingSetup` as JSON.

use serde::{Deserialize, Serialize};

use crate::recording::ffmpeg::output_size;
use crate::recording::metadata::Mode;

/// Frame rates the Record screen offers.
pub const FRAME_RATES: [u32; 4] = [5, 10, 15, 30];

/// Bits per pixel and frame the video bitrate is based on: 1080p at 15 fps comes to about 2.5 Mbit/s.
const BITS_PER_PIXEL: f64 = 0.08;
const MIN_VIDEO_BITRATE: u32 = 600_000;
const MAX_VIDEO_BITRATE: u32 = 12_000_000;

/// 24-bit stereo at 48 kHz, uncompressed. A FLAC file is never larger than this, so it is a true upper bound.
const FLAC_TRACK_MAX_BYTES_PER_SECOND: u64 = 48_000 * 2 * 3;
/// The mixed AAC track in the MP4 or M4A (128 kbit/s).
const MIX_BYTES_PER_SECOND: u64 = 128_000 / 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    /// At most 720 rows.
    P720,
    /// At most 1080 rows (the default).
    P1080,
    /// The monitor's own size.
    Native,
}

impl Quality {
    /// The most rows the video may have; `None` keeps the monitor's own size. Never scales up.
    pub fn max_height(self) -> Option<u32> {
        match self {
            Quality::P720 => Some(720),
            Quality::P1080 => Some(1080),
            Quality::Native => None,
        }
    }
}

/// Everything chosen before a recording starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordingSetup {
    /// Empty means "use a default title".
    pub title: String,
    pub mode: Mode,
    /// The monitor to record; `None` is the primary monitor. Ignored when only audio is recorded.
    pub monitor_id: Option<String>,
    pub microphone: bool,
    /// The microphone to use; `None` is the default one.
    pub microphone_id: Option<String>,
    pub system_audio: bool,
    pub quality: Quality,
    pub fps: u32,
}

impl Default for RecordingSetup {
    fn default() -> Self {
        Self {
            title: String::new(),
            mode: Mode::ScreenAudio,
            monitor_id: None,
            microphone: true,
            microphone_id: None,
            system_audio: true,
            quality: Quality::P1080,
            fps: 15,
        }
    }
}

impl RecordingSetup {
    /// Why this setup cannot be recorded, in words for the user. `None` if it can.
    pub fn problem(&self) -> Option<&'static str> {
        if !FRAME_RATES.contains(&self.fps) {
            return Some("The frame rate must be 5, 10, 15 or 30.");
        }
        if self.mode == Mode::AudioOnly && !self.microphone && !self.system_audio {
            return Some(
                "Choose the microphone, system audio, or both: an audio-only recording needs a source.",
            );
        }
        None
    }

    /// What the video in the file looks like, or `None` when no screen is recorded.
    pub fn video_plan(&self, monitor_size: (u32, u32)) -> Option<VideoPlan> {
        if self.mode != Mode::ScreenAudio {
            return None;
        }
        let (width, height) =
            output_size(monitor_size.0, monitor_size.1, self.quality.max_height());
        Some(VideoPlan {
            width,
            height,
            bitrate_bps: video_bitrate(width, height, self.fps),
        })
    }

    /// The most disk space one hour of this setup can take: video at its bitrate, each audio track at the
    /// uncompressed size (FLAC is smaller, so this never under-promises), and the mixed track.
    pub fn max_bytes_per_hour(&self, monitor_size: (u32, u32)) -> u64 {
        let mut per_second = 0u64;
        if let Some(plan) = self.video_plan(monitor_size) {
            per_second += u64::from(plan.bitrate_bps) / 8;
        }
        let audio_tracks = u64::from(self.microphone) + u64::from(self.system_audio);
        if audio_tracks > 0 {
            per_second += audio_tracks * FLAC_TRACK_MAX_BYTES_PER_SECOND + MIX_BYTES_PER_SECOND;
        }
        per_second * 3600
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPlan {
    /// Size of the video in the file.
    pub width: u32,
    pub height: u32,
    pub bitrate_bps: u32,
}

/// The video bitrate for a picture size and frame rate.
pub fn video_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let wanted = f64::from(width) * f64::from(height) * f64::from(fps) * BITS_PER_PIXEL;
    (wanted as u32).clamp(MIN_VIDEO_BITRATE, MAX_VIDEO_BITRATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_1080p_at_15_fps_with_everything_on() {
        let setup = RecordingSetup::default();

        assert_eq!(setup.quality, Quality::P1080);
        assert_eq!(setup.fps, 15);
        assert!(setup.microphone && setup.system_audio);
        assert_eq!(setup.mode, Mode::ScreenAudio);
        assert_eq!(setup.problem(), None);
    }

    #[test]
    fn a_1080p_picture_at_15_fps_is_about_2_5_megabits() {
        let bitrate = video_bitrate(1920, 1080, 15);

        assert!((2_400_000..2_600_000).contains(&bitrate), "{bitrate}");
    }

    #[test]
    fn the_bitrate_stays_within_sane_limits() {
        assert_eq!(video_bitrate(320, 180, 5), MIN_VIDEO_BITRATE);
        assert_eq!(video_bitrate(7680, 4320, 30), MAX_VIDEO_BITRATE);
    }

    #[test]
    fn a_big_monitor_is_scaled_to_the_chosen_quality() {
        let mut setup = RecordingSetup::default();

        let plan = setup.video_plan((3840, 2160)).unwrap();
        assert_eq!((plan.width, plan.height), (1920, 1080));

        setup.quality = Quality::P720;
        let plan = setup.video_plan((3840, 2160)).unwrap();
        assert_eq!((plan.width, plan.height), (1280, 720));

        setup.quality = Quality::Native;
        let plan = setup.video_plan((3840, 2160)).unwrap();
        assert_eq!((plan.width, plan.height), (3840, 2160));
    }

    #[test]
    fn a_small_monitor_is_not_scaled_up() {
        let plan = RecordingSetup::default().video_plan((1280, 720)).unwrap();

        assert_eq!((plan.width, plan.height), (1280, 720));
    }

    #[test]
    fn an_audio_only_recording_has_no_video_plan() {
        let setup = RecordingSetup {
            mode: Mode::AudioOnly,
            ..RecordingSetup::default()
        };

        assert_eq!(setup.video_plan((1920, 1080)), None);
    }

    #[test]
    fn an_audio_only_recording_needs_an_audio_source() {
        let setup = RecordingSetup {
            mode: Mode::AudioOnly,
            microphone: false,
            system_audio: false,
            ..RecordingSetup::default()
        };

        assert!(setup.problem().unwrap().contains("audio-only"));
    }

    #[test]
    fn a_screen_recording_without_any_audio_is_allowed() {
        let setup = RecordingSetup {
            microphone: false,
            system_audio: false,
            ..RecordingSetup::default()
        };

        assert_eq!(setup.problem(), None);
    }

    #[test]
    fn an_unlisted_frame_rate_is_refused() {
        let setup = RecordingSetup {
            fps: 24,
            ..RecordingSetup::default()
        };

        assert!(setup.problem().unwrap().contains("frame rate"));
    }

    #[test]
    fn the_hourly_estimate_adds_video_and_every_audio_track() {
        let full = RecordingSetup::default().max_bytes_per_hour((1920, 1080));
        let one_track = RecordingSetup {
            microphone: false,
            ..RecordingSetup::default()
        }
        .max_bytes_per_hour((1920, 1080));
        let audio_only = RecordingSetup {
            mode: Mode::AudioOnly,
            ..RecordingSetup::default()
        }
        .max_bytes_per_hour((1920, 1080));

        // One FLAC track is an hour of 24-bit stereo at 48 kHz: just over a gigabyte.
        assert_eq!(full - one_track, FLAC_TRACK_MAX_BYTES_PER_SECOND * 3600);
        assert!(full > audio_only, "video adds to the audio");
        // 1080p at 15 fps is about 1.1 GB per hour of video; two tracks about 2.1 GB: a few GB in all.
        assert!((3_000_000_000..3_600_000_000).contains(&full), "{full}");
    }

    #[test]
    fn a_saved_setup_survives_json_and_missing_fields_take_defaults() {
        let setup = RecordingSetup {
            title: "Standup".into(),
            monitor_id: Some("\\\\.\\DISPLAY2".into()),
            quality: Quality::P720,
            fps: 10,
            ..RecordingSetup::default()
        };

        let json = serde_json::to_string(&setup).unwrap();
        assert!(json.contains("\"quality\":\"p720\""), "{json}");
        assert!(json.contains("\"mode\":\"screenAudio\""), "{json}");
        assert_eq!(
            serde_json::from_str::<RecordingSetup>(&json).unwrap(),
            setup
        );

        let from_an_older_build: RecordingSetup =
            serde_json::from_str(r#"{"title":"Old","fps":10}"#).unwrap();
        assert_eq!(from_an_older_build.title, "Old");
        assert_eq!(from_an_older_build.quality, Quality::P1080);
    }
}
