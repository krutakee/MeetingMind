//! The one clock every stream is stamped against, and the pause arithmetic built on it (SPEC Section 5).
//!
//! Readings are `Duration`s since an arbitrary epoch that is fixed for the process. Sources convert their device
//! timestamps into this clock when they capture. "Media time" is the clock minus the recording start minus all
//! paused time, so it is exactly the position in the saved files.

use std::time::{Duration, Instant};

/// A monotonic time source. Tests replace it with a hand-driven clock.
pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> Duration;
}

/// The real clock. `Instant` reads QueryPerformanceCounter on Windows, the same source cpal uses for audio.
pub struct SystemClock {
    epoch: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for SystemClock {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TimelineError {
    #[error("the recording is already paused")]
    AlreadyPaused,
    #[error("the recording is not paused")]
    NotPaused,
    #[error("a clock reading went backwards")]
    ClockWentBackwards,
}

/// One pause, as recorded in `meeting.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PauseRecord {
    /// Where in the saved media the pause happened (the cut point).
    pub at_media: Duration,
    /// How long the pause lasted on the wall clock. None of this time exists in the files.
    pub wall_length: Duration,
}

/// Converts clock readings to media time, cutting out paused intervals.
#[derive(Debug, Clone)]
pub struct MediaTimeline {
    started_at: Duration,
    /// Completed pauses as clock readings: (paused at, resumed at).
    pauses: Vec<(Duration, Duration)>,
    paused_since: Option<Duration>,
}

impl MediaTimeline {
    pub fn start(at: Duration) -> Self {
        Self {
            started_at: at,
            pauses: Vec::new(),
            paused_since: None,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_since.is_some()
    }

    pub fn pause(&mut self, at: Duration) -> Result<(), TimelineError> {
        if self.paused_since.is_some() {
            return Err(TimelineError::AlreadyPaused);
        }
        if at < self.latest_reading() {
            return Err(TimelineError::ClockWentBackwards);
        }
        self.paused_since = Some(at);
        Ok(())
    }

    pub fn resume(&mut self, at: Duration) -> Result<(), TimelineError> {
        let since = self.paused_since.ok_or(TimelineError::NotPaused)?;
        if at < since {
            return Err(TimelineError::ClockWentBackwards);
        }
        self.pauses.push((since, at));
        self.paused_since = None;
        Ok(())
    }

    fn latest_reading(&self) -> Duration {
        self.pauses
            .last()
            .map(|&(_, resumed)| resumed)
            .unwrap_or(self.started_at)
    }

    /// The media time of a clock reading, or `None` if the reading is before the start or falls inside a pause
    /// (so whatever was captured then must be dropped).
    pub fn media_time(&self, at: Duration) -> Option<Duration> {
        if at < self.started_at {
            return None;
        }
        if let Some(since) = self.paused_since
            && at >= since
        {
            return None;
        }
        let mut cut = Duration::ZERO;
        for &(paused, resumed) in &self.pauses {
            if at >= resumed {
                cut += resumed - paused;
            } else if at >= paused {
                return None;
            }
        }
        Some(at - self.started_at - cut)
    }

    /// Media time reached at clock reading `at`. While paused this stays at the cut point.
    pub fn media_length_at(&self, at: Duration) -> Duration {
        let effective = match self.paused_since {
            Some(since) if at >= since => since,
            _ => at,
        };
        let mut cut = Duration::ZERO;
        for &(paused, resumed) in &self.pauses {
            if effective >= resumed {
                cut += resumed - paused;
            } else if effective >= paused {
                // Inside a completed pause: the media length is frozen at the cut point.
                return paused - self.started_at - cut;
            }
        }
        effective
            .saturating_sub(self.started_at)
            .saturating_sub(cut)
    }

    /// All pauses so far, in order, for `meeting.json`. An open pause is reported up to `now`.
    pub fn pause_records(&self, now: Duration) -> Vec<PauseRecord> {
        let mut records = Vec::new();
        let mut cut = Duration::ZERO;
        for &(paused, resumed) in &self.pauses {
            records.push(PauseRecord {
                at_media: paused - self.started_at - cut,
                wall_length: resumed - paused,
            });
            cut += resumed - paused;
        }
        if let Some(since) = self.paused_since {
            records.push(PauseRecord {
                at_media: since - self.started_at - cut,
                wall_length: now.saturating_sub(since),
            });
        }
        records
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn media_time_without_pauses_is_the_time_since_start() {
        let timeline = MediaTimeline::start(s(100));

        assert_eq!(timeline.media_time(s(100)), Some(s(0)));
        assert_eq!(timeline.media_time(s(107)), Some(s(7)));
    }

    #[test]
    fn readings_before_the_start_have_no_media_time() {
        let timeline = MediaTimeline::start(s(100));

        assert_eq!(timeline.media_time(s(99)), None);
    }

    #[test]
    fn paused_time_is_cut_out_of_everything_after_it() {
        let mut timeline = MediaTimeline::start(s(0));
        timeline.pause(s(10)).unwrap();
        timeline.resume(s(25)).unwrap(); // 15 s paused

        assert_eq!(timeline.media_time(s(9)), Some(s(9)));
        assert_eq!(
            timeline.media_time(s(25)),
            Some(s(10)),
            "resume continues at the cut point"
        );
        assert_eq!(timeline.media_time(s(40)), Some(s(25)));
    }

    #[test]
    fn readings_inside_a_pause_are_dropped() {
        let mut timeline = MediaTimeline::start(s(0));
        timeline.pause(s(10)).unwrap();

        assert_eq!(timeline.media_time(s(10)), None, "open pause");
        assert_eq!(timeline.media_time(s(500)), None, "open pause, much later");
        timeline.resume(s(25)).unwrap();
        assert_eq!(
            timeline.media_time(s(15)),
            None,
            "a late chunk stamped inside the finished pause"
        );
        assert_eq!(timeline.media_time(s(24)), None);
    }

    #[test]
    fn several_pauses_add_up() {
        let mut timeline = MediaTimeline::start(s(0));
        timeline.pause(s(10)).unwrap();
        timeline.resume(s(20)).unwrap(); // 10 s cut
        timeline.pause(s(30)).unwrap();
        timeline.resume(s(35)).unwrap(); // 5 s cut

        // 35 s on the wall clock minus 15 s paused in total.
        assert_eq!(timeline.media_time(s(30)), None);
        assert_eq!(timeline.media_time(s(35)), Some(s(20)));
        assert_eq!(timeline.media_time(s(50)), Some(s(35)));
    }

    #[test]
    fn media_length_freezes_while_paused() {
        let mut timeline = MediaTimeline::start(s(0));
        assert_eq!(timeline.media_length_at(s(8)), s(8));
        timeline.pause(s(10)).unwrap();
        assert_eq!(timeline.media_length_at(s(10)), s(10));
        assert_eq!(
            timeline.media_length_at(s(60)),
            s(10),
            "frozen while paused"
        );
        timeline.resume(s(60)).unwrap();
        assert_eq!(timeline.media_length_at(s(65)), s(15));
    }

    #[test]
    fn pause_and_resume_must_alternate() {
        let mut timeline = MediaTimeline::start(s(0));

        assert_eq!(timeline.resume(s(1)), Err(TimelineError::NotPaused));
        timeline.pause(s(2)).unwrap();
        assert_eq!(timeline.pause(s(3)), Err(TimelineError::AlreadyPaused));
        assert!(timeline.is_paused());
        timeline.resume(s(4)).unwrap();
        assert!(!timeline.is_paused());
    }

    #[test]
    fn a_clock_that_goes_backwards_is_rejected() {
        let mut timeline = MediaTimeline::start(s(10));
        timeline.pause(s(20)).unwrap();

        assert_eq!(
            timeline.resume(s(19)),
            Err(TimelineError::ClockWentBackwards)
        );
        timeline.resume(s(30)).unwrap();
        assert_eq!(
            timeline.pause(s(29)),
            Err(TimelineError::ClockWentBackwards)
        );
    }

    #[test]
    fn pause_records_report_the_cut_points_in_media_time() {
        let mut timeline = MediaTimeline::start(s(100));
        timeline.pause(s(110)).unwrap();
        timeline.resume(s(125)).unwrap();
        timeline.pause(s(145)).unwrap(); // still paused

        let records = timeline.pause_records(s(150));

        assert_eq!(
            records,
            vec![
                PauseRecord {
                    at_media: s(10),
                    wall_length: s(15)
                },
                PauseRecord {
                    at_media: s(30),
                    wall_length: s(5)
                },
            ]
        );
    }

    #[test]
    fn the_system_clock_never_goes_backwards() {
        let clock = SystemClock::new();
        let a = clock.now();
        let b = clock.now();

        assert!(b >= a);
    }
}
